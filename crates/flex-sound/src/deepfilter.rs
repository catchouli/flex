//! Finding the voice with DeepFilterNet3 (Schröter et al., github.com/Rikorose/DeepFilterNet,
//! MIT or Apache-2.0): a small network made to take the noise out of speech, run 10 ms at a time
//! on sound at 48 kHz. What it takes out is the background, so the two still add up to exactly
//! the sound that came in.
//!
//! It gives back clean, dry speech, so the voice's reverb, and what it wasn't sure of, are left
//! in what it takes out. That's split off too, as the voice's echo: wherever the voice was most
//! of the sound in a band of the spectrum, lately (following its reverb), that much of the rest
//! there is taken as the voice's.
//!
//! The network is three ONNX graphs run by tract, streamed a frame at a time as DeepFilterNet's
//! own Rust code (libDF) does; the spectrum, features and filtering around them follow libDF's.

use std::collections::VecDeque;
use std::io::Read;
use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use tract_onnx::prelude::tract_data::internal::{bail, ensure};
use tract_onnx::prelude::*;
use tract_pulse::model::{PulsedModel, PulsedModelExt};

/// Where Flex downloads it from on first use (DeepFilterNet's own repository, at a fixed
/// commit), and what to expect.
pub const FILE: &str = "DeepFilterNet3_onnx.tar.gz";
pub const URL: &str = "https://github.com/Rikorose/DeepFilterNet/raw/7b14b895fe7fc42cab72b4a4c1e49c982bd6404b/models/DeepFilterNet3_onnx.tar.gz";
pub const BYTES: u64 = 7_983_136;
pub const SHA256: &str = "c94d91f70911001c946e0fabb4aa9adc37045f45a03b56008cb0c8244cb63616";

/// The sound it takes, and how much it moves on at a time (10 ms).
pub const RATE: u32 = 48_000;
pub const HOP: usize = 480;
const FFT: usize = 960;
const FREQS: usize = FFT / 2 + 1;
/// Bands it hears the whole spectrum in, and the low bins it filters finely.
const ERB: usize = 32;
const MIN_ERB_FREQS: usize = 2;
const DF_BINS: usize = 96;
/// Frames each deep filter reaches over, and how many of them are ahead.
const DF_ORDER: usize = 5;
const LOOKAHEAD: usize = 2;
/// The encoder's width.
const CONV_CH: usize = 64;
/// How quickly the features' running normalisation follows (from the model's `norm_tau` of 1 s).
const NORM_ALPHA: f32 = 0.99;
/// How long the voice's echo is followed after it, in seconds.
const ECHO_SECS: f32 = 0.1;

/// How late the voice comes out, in frames at `RATE`: the spectrum's overlap and the lookahead.
pub const LATENCY: usize = (FFT - HOP) + LOOKAHEAD * HOP;

type Plan = Arc<TypedRunnableModel>;

/// The network, loaded once and shared by every stream.
pub struct Model {
    enc: Plan,
    erb_dec: Plan,
    df_dec: Plan,
}

impl Model {
    /// Loads it from DeepFilterNet's `DeepFilterNet3_onnx.tar.gz`. Takes a second or so.
    pub fn load(targz: impl Read) -> TractResult<Self> {
        let mut graphs: [Vec<u8>; 3] = Default::default();
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(targz));
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.into_owned();
            let slot = ["enc.onnx", "erb_dec.onnx", "df_dec.onnx"].iter().position(|n| path.ends_with(n));
            if let Some(i) = slot {
                entry.read_to_end(&mut graphs[i])?;
            }
        }
        ensure!(graphs.iter().all(|g| !g.is_empty()), "the archive is missing one of DeepFilterNet's graphs");
        let hidden = CONV_CH * ERB / 4;
        let enc = pulsed(&graphs[0], &[&[1, 1, 0, ERB], &[1, 2, 0, DF_BINS]], &["feat_erb", "feat_spec"], &["e0", "e1", "e2", "e3", "emb", "c0", "lsnr"])?;
        let erb_dec = pulsed(
            &graphs[1],
            &[&[1, 0, hidden], &[1, CONV_CH, 0, ERB / 4], &[1, CONV_CH, 0, ERB / 4], &[1, CONV_CH, 0, ERB / 2], &[1, CONV_CH, 0, ERB]],
            &["emb", "e3", "e2", "e1", "e0"],
            &[],
        )?;
        let df_dec = pulsed(&graphs[2], &[&[1, 0, hidden], &[1, CONV_CH, 0, DF_BINS]], &["emb", "c0"], &["coefs"])?;
        Ok(Self { enc, erb_dec, df_dec })
    }
}

/// One graph, streamed a frame at a time. Each input's shape has a 0 where the frames go.
fn pulsed(onnx: &[u8], shapes: &[&[usize]], inputs: &[&str], outputs: &[&str]) -> TractResult<Plan> {
    let mut model = tract_onnx::onnx().with_ignore_output_shapes(true).model_for_read(&mut &onnx[..])?;
    let s = model.symbols.sym("S");
    for (i, shape) in shapes.iter().enumerate() {
        let dims: TVec<TDim> = shape.iter().map(|&d| if d == 0 { s.clone().into() } else { d.into() }).collect();
        model = model.with_input_fact(i, InferenceFact::dt_shape(f32::datum_type(), dims))?;
    }
    model = model.with_input_names(inputs)?;
    if !outputs.is_empty() {
        // Picked by name: some graphs have more outputs than DeepFilterNet uses.
        let named = |o: &OutletId| model.outlet_label(*o).unwrap_or(&model.node(o.node).name).to_string();
        let all: Vec<(String, OutletId)> = model.output_outlets()?.iter().map(|o| (named(o), *o)).collect();
        let picked = outputs.iter().map(|n| all.iter().find(|(name, _)| name == n).map(|(_, o)| *o)).collect::<Option<Vec<_>>>();
        let Some(picked) = picked else { bail!("the graph's outputs are {:?}, not {outputs:?}", all.iter().map(|(n, _)| n).collect::<Vec<_>>()) };
        model.select_output_outlets(&picked)?;
    }
    let typed = model.into_typed()?.into_decluttered()?;
    let pulsed = PulsedModel::new(&typed, s, &1.to_dim())?;
    pulsed.into_typed()?.into_optimized()?.into_runnable()
}

/// The spectrum, 20 ms windows every 10 ms, and back (libDF's Vorbis window, which puts the
/// sound back exactly).
struct Spectrum {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    /// The last hop in, and what the last window left to add to the next hop out.
    analysis: Vec<f32>,
    synthesis: Vec<f32>,
    buf: Vec<f32>,
    scratch: Vec<Complex32>,
}

impl Spectrum {
    fn new() -> Self {
        let mut planner = RealFftPlanner::new();
        let (forward, inverse) = (planner.plan_fft_forward(FFT), planner.plan_fft_inverse(FFT));
        let half = std::f64::consts::FRAC_PI_2;
        let window = (0..FFT)
            .map(|i| {
                let s = (half * (i as f64 + 0.5) / (FFT / 2) as f64).sin();
                (half * s * s).sin() as f32
            })
            .collect();
        let scratch = vec![Complex32::default(); forward.get_scratch_len().max(inverse.get_scratch_len())];
        Self { forward, inverse, window, analysis: vec![0.0; FFT - HOP], synthesis: vec![0.0; FFT - HOP], buf: vec![0.0; FFT], scratch }
    }

    fn analyse(&mut self, hop: &[f32], out: &mut [Complex32]) {
        let keep = FFT - HOP;
        for i in 0..keep {
            self.buf[i] = self.analysis[i] * self.window[i];
        }
        for ((b, x), w) in self.buf[keep..].iter_mut().zip(hop).zip(&self.window[keep..]) {
            *b = x * w;
        }
        self.analysis.copy_from_slice(&hop[HOP - keep..]);
        let _ = self.forward.process_with_scratch(&mut self.buf, out, &mut self.scratch);
        let norm = 2.0 * HOP as f32 / (FFT * FFT) as f32;
        for x in out {
            *x *= norm;
        }
    }

    fn synthesise(&mut self, spectrum: &mut [Complex32], out: &mut [f32]) {
        // The DC and Nyquist bins have to be real for the inverse; they are, near enough.
        spectrum[0].im = 0.0;
        spectrum[FREQS - 1].im = 0.0;
        let _ = self.inverse.process_with_scratch(spectrum, &mut self.buf, &mut self.scratch);
        for (x, w) in self.buf.iter_mut().zip(&self.window) {
            *x *= w;
        }
        for ((o, x), m) in out.iter_mut().zip(&self.buf).zip(&self.synthesis) {
            *o = x + m;
        }
        let keep = FFT - HOP;
        for i in 0..keep {
            self.synthesis[i] = self.buf[HOP + i];
        }
    }
}

/// Which bins each of the `ERB` bands covers, low to high (libDF's `erb_fb`).
fn erb_bands() -> Vec<usize> {
    let to_erb = |f: f32| 9.265 * (f / (24.7 * 9.265)).ln_1p();
    let from_erb = |e: f32| 24.7 * 9.265 * ((e / 9.265).exp() - 1.0);
    let width = RATE as f32 / FFT as f32;
    let step = to_erb(RATE as f32 / 2.0) / ERB as f32;
    let (mut bands, mut prev, mut over) = (vec![0usize; ERB], 0i32, 0i32);
    for (i, band) in bands.iter_mut().enumerate() {
        let bin = (from_erb((i + 1) as f32 * step) / width).round() as i32;
        let mut n = bin - prev - over;
        if n < MIN_ERB_FREQS as i32 {
            over = MIN_ERB_FREQS as i32 - n;
            n = MIN_ERB_FREQS as i32;
        } else {
            over = 0;
        }
        *band = n as usize;
        prev = bin;
    }
    bands[ERB - 1] += 1;
    let total: usize = bands.iter().sum();
    bands[ERB - 1] -= total - FREQS;
    bands
}

/// One stream through the network: mono sound at `RATE`, a hop at a time.
pub struct Separator {
    enc: TypedSimpleState,
    erb_dec: TypedSimpleState,
    df_dec: TypedSimpleState,
    spectrum: Spectrum,
    background: Spectrum,
    echo: Spectrum,
    /// How much of each bin the voice has been, lately.
    share: Vec<f32>,
    bands: Vec<usize>,
    mean_norm: Vec<f32>,
    unit_norm: Vec<f32>,
    /// The last frames' spectra: as they came (for the deep filter and the background), and
    /// with the bands' gains on (for the voice).
    noisy: VecDeque<Vec<Complex32>>,
    gained: VecDeque<Vec<Complex32>>,
    spec: Vec<Complex32>,
    voice: Vec<Complex32>,
    rest: Vec<Complex32>,
    echoed: Vec<Complex32>,
}


impl Separator {
    pub fn new(model: &Model) -> TractResult<Self> {
        let frames = |n| (0..n).map(|_| vec![Complex32::default(); FREQS]).collect::<VecDeque<_>>();
        let (lo, hi) = (-60.0, -90.0);
        let mean_norm = (0..ERB).map(|i| lo + i as f32 * (hi - lo) / (ERB - 1) as f32).collect();
        let (lo, hi) = (0.001, 0.0001);
        let unit_norm = (0..DF_BINS).map(|i| lo + i as f32 * (hi - lo) / (DF_BINS - 1) as f32).collect();
        Ok(Self {
            enc: model.enc.spawn()?,
            erb_dec: model.erb_dec.spawn()?,
            df_dec: model.df_dec.spawn()?,
            spectrum: Spectrum::new(),
            background: Spectrum::new(),
            echo: Spectrum::new(),
            share: vec![0.0; FREQS],
            bands: erb_bands(),
            mean_norm,
            unit_norm,
            noisy: frames(DF_ORDER.max(LOOKAHEAD)),
            gained: frames(DF_ORDER + LOOKAHEAD),
            spec: vec![Complex32::default(); FREQS],
            voice: vec![Complex32::default(); FREQS],
            rest: vec![Complex32::default(); FREQS],
            echoed: vec![Complex32::default(); FREQS],
        })
    }

    /// Takes in a hop of sound and gives back the voice, its echo and the rest of it, all
    /// `LATENCY` frames late. They add up to the sound.
    pub fn run(&mut self, hop: &[f32], voice: &mut [f32], echo: &mut [f32], background: &mut [f32]) -> TractResult<()> {
        debug_assert!(hop.len() == HOP && voice.len() == HOP && echo.len() == HOP && background.len() == HOP);
        self.spectrum.analyse(hop, &mut self.spec);
        let mut oldest = self.noisy.pop_front().unwrap_or_default();
        oldest.clone_from(&self.spec);
        self.noisy.push_back(oldest);
        let mut oldest = self.gained.pop_front().unwrap_or_default();
        oldest.clone_from(&self.spec);
        self.gained.push_back(oldest);

        // Features: each band's loudness, and the low bins' spectrum, both normalised as they go.
        let mut erb = Tensor::zero::<f32>(&[1, 1, 1, ERB])?;
        let mut cplx = Tensor::zero::<f32>(&[1, 2, 1, DF_BINS])?;
        {
            let mut erb = erb.try_as_plain_ram_mut()?;
            let erb = erb.as_slice_mut::<f32>()?;
            let mut at = 0;
            for (b, &n) in self.bands.iter().enumerate() {
                let power: f32 = self.spec[at..at + n].iter().map(|x| x.norm_sqr()).sum::<f32>() / n as f32;
                at += n;
                let db = (power + 1e-10).log10() * 10.0;
                let s = &mut self.mean_norm[b];
                *s = db * (1.0 - NORM_ALPHA) + *s * NORM_ALPHA;
                erb[b] = (db - *s) / 40.0;
            }
            let mut cplx = cplx.try_as_plain_ram_mut()?;
            let cplx = cplx.as_slice_mut::<f32>()?;
            for (f, s) in self.unit_norm.iter_mut().enumerate() {
                let x = self.spec[f];
                *s = x.norm() * (1.0 - NORM_ALPHA) + *s * NORM_ALPHA;
                let x = x / s.sqrt();
                cplx[f] = x.re;
                cplx[DF_BINS + f] = x.im;
            }
        }
        let mut enc = self.enc.run(tvec!(erb.into(), cplx.into()))?;
        let lsnr = enc[6].try_as_plain_ram()?.as_slice::<f32>()?[0];
        let (e0, e1, e2, e3, emb, c0) = (enc.remove(0), enc.remove(0), enc.remove(0), enc.remove(0), enc.remove(0), enc.remove(0));
        // Both decoders always run, so what they remember follows the sound; what they say is
        // used as libDF does, by how much speech the encoder hears over the noise.
        let gains = self.erb_dec.run(tvec!(emb.clone(), e3, e2, e1, e0))?.remove(0);
        let coefs = self.df_dec.run(tvec!(emb, c0))?.remove(0);
        let (no_speech, clean, little_noise) = (lsnr < -10.0, lsnr > 30.0, lsnr > 20.0);

        let target = &mut self.gained[DF_ORDER - 1];
        if no_speech {
            target.fill(Complex32::default());
        } else if !clean {
            let gains = gains.try_as_plain_ram()?;
            let gains = gains.as_slice::<f32>()?;
            let mut at = 0;
            for (&n, &g) in self.bands.iter().zip(gains) {
                for x in &mut target[at..at + n] {
                    *x *= g;
                }
                at += n;
            }
        }
        self.voice.clone_from(target);
        if !no_speech && !clean && !little_noise {
            // The deep filter: each low bin from its last frames, mixed by the network's taps.
            let coefs = coefs.try_as_plain_ram()?;
            let coefs = coefs.as_slice::<f32>()?;
            for f in 0..DF_BINS {
                let mut sum = Complex32::default();
                for (n, frame) in self.noisy.iter().enumerate() {
                    let c = (f * DF_ORDER + n) * 2;
                    sum += frame[f] * Complex32::new(coefs[c], coefs[c + 1]);
                }
                self.voice[f] = sum;
            }
        }
        let noisy = &self.noisy[DF_ORDER.max(LOOKAHEAD) - LOOKAHEAD - 1];
        let fall = (-(HOP as f32) / RATE as f32 / ECHO_SECS).exp();
        for ((((r, e), n), v), share) in self.rest.iter_mut().zip(&mut self.echoed).zip(noisy).zip(&self.voice).zip(&mut self.share) {
            *r = n - v;
            let (pv, pr) = (v.norm_sqr(), r.norm_sqr());
            *share = (pv / (pv + pr + 1e-12)).max(*share * fall);
            *e = *r * *share;
            *r -= *e;
        }
        self.spectrum.synthesise(&mut self.voice, voice);
        self.echo.synthesise(&mut self.echoed, echo);
        self.background.synthesise(&mut self.rest, background);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bands_cover_the_spectrum() {
        let bands = erb_bands();
        assert_eq!(bands.len(), ERB);
        assert_eq!(bands.iter().sum::<usize>(), FREQS);
        assert!(bands.iter().all(|&n| n >= MIN_ERB_FREQS));
    }

    #[test]
    fn the_spectrum_puts_the_sound_back_a_window_late() {
        let mut s = Spectrum::new();
        let input: Vec<f32> = (0..HOP * 20).map(|i| ((i * 7919) % 1000) as f32 / 500.0 - 1.0).collect();
        let mut out = vec![0.0; input.len()];
        let mut spec = vec![Complex32::default(); FREQS];
        for (i, o) in input.chunks(HOP).zip(out.chunks_mut(HOP)) {
            s.analyse(i, &mut spec);
            s.synthesise(&mut spec, o);
        }
        let late = FFT - HOP;
        for i in late..input.len() {
            assert!((out[i] - input[i - late]).abs() < 1e-4, "at {i}: {} vs {}", out[i], input[i - late]);
        }
    }

    /// Runs a clip through: `FLEX_DFN` is DeepFilterNet3_onnx.tar.gz, `FLEX_DFN_IN` raw mono
    /// f32 at 48 kHz; the voice and background are written beside it.
    #[test]
    #[ignore]
    fn separates_a_clip() {
        let model = Model::load(std::fs::File::open(std::env::var("FLEX_DFN").unwrap()).unwrap()).unwrap();
        let path = std::env::var("FLEX_DFN_IN").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let input: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        let mut sep = Separator::new(&model).unwrap();
        let (mut voice, mut echo, mut background) = (vec![0.0; input.len()], vec![0.0; input.len()], vec![0.0; input.len()]);
        let start = std::time::Instant::now();
        for (((i, v), e), b) in input.as_chunks::<HOP>().0.iter().zip(voice.as_chunks_mut::<HOP>().0).zip(echo.as_chunks_mut::<HOP>().0).zip(background.as_chunks_mut::<HOP>().0) {
            sep.run(i, v, e, b).unwrap();
        }
        let took = start.elapsed().as_secs_f32();
        println!("{:.1} s of sound in {took:.2} s ({:.3} of real time)", input.len() as f32 / RATE as f32, took / (input.len() as f32 / RATE as f32));
        let mut worst = 0.0f32;
        for i in LATENCY..input.len() / HOP * HOP {
            worst = worst.max((voice[i] + echo[i] + background[i] - input[i - LATENCY]).abs());
        }
        println!("voice + echo + background vs the sound: worst {worst:e}");
        let write = |name: &str, x: &[f32]| std::fs::write(format!("{path}.{name}"), x.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>()).unwrap();
        write("voice", &voice);
        write("echo", &echo);
        write("background", &background);
        assert!(worst < 1e-3);
    }
}
