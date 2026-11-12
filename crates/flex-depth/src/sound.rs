//! What kind of sound is playing: YAMNet (Apache 2.0, github.com/tensorflow/models,
//! research/audioset/yamnet), which scores a second of sound against 521 kinds from Google's
//! AudioSet, gathered here into a few that effects can use. It's small (a MobileNet on a log mel
//! spectrogram, about 70 million multiply-adds a go), so it runs on the CPU rather than taking
//! the GPU from the picture's networks. `tools/sound/convert.py` makes the weights,
//! `tools/sound/reference.py` the reference the tests compare against.

use std::collections::VecDeque;
use std::ops::RangeInclusive;

use anyhow::{Context, Result, bail};

use super::read_safetensors;

const WEIGHTS: &str = "sound.safetensors";
/// The sound it takes: 16 kHz, one channel.
pub const RATE: u32 = 16_000;
/// What one go looks at: 96 frames of 10 ms, with the rest of the last frame's window (0.975 s).
pub const SAMPLES: usize = 15_600;
/// How often it looks again (0.48 s), as YAMNet does.
pub const HOP: usize = 7_680;
pub const CLASSES: usize = 521;
const WINDOW: usize = 400;
const STEP: usize = 160;
const FFT: usize = 512;
const FRAMES: usize = 96;
const MEL_BANDS: usize = 64;
/// Each separable layer's stride and outputs.
const LAYERS: [(usize, usize); 13] = [(1, 64), (2, 128), (1, 128), (2, 256), (1, 256), (2, 512), (1, 512), (1, 512), (1, 512), (1, 512), (1, 512), (2, 1024), (1, 1024)];

/// The kinds of sound effects are told about, each the most likely of some of AudioSet's
/// classes (as numbered in yamnet_class_map.csv, which follows AudioSet's ontology).
#[allow(clippy::single_range_in_vec_init)]
pub const KINDS: [(&str, &[RangeInclusive<usize>]); 23] = [
    ("speech", &[0..=5]),
    ("shouting", &[6..=11]),
    ("laughter", &[13..=18]),
    ("crying", &[19..=22]),
    ("singing", &[24..=32, 249..=250]),
    ("music", &[132..=276]),
    ("crowd", &[61..=66]),
    ("animals", &[67..=131]),
    ("vehicles", &[294..=347]),
    ("explosions", &[420..=430]),
    ("thunder", &[280..=281]),
    ("water", &[282..=291]),
    ("wind", &[277..=279]),
    ("fire", &[292..=293]),
    ("alarms", &[382..=397]),
    ("impacts", &[454..=455, 460..=464]),
    ("silence", &[494..=494]),
    // How the music feels, which AudioSet also labels.
    ("happy_music", &[271..=271]),
    ("sad_music", &[272..=272]),
    ("tender_music", &[273..=273]),
    ("exciting_music", &[274..=274]),
    ("angry_music", &[275..=275]),
    ("scary_music", &[276..=276]),
];

/// Where the music's moods start in `KINDS`, and how many there are.
const MOODS: usize = 17;
const MOOD_COUNT: usize = 6;
/// The moods' scores are nearly always small (rarely above 0.05 on films), so they're shared
/// out between them instead. Below this much in all, they count for less.
const MOOD_FLOOR: f32 = 0.03;

/// `KINDS`' scores from all the classes' scores. The music's moods share the music's score
/// between them, by how they compare with each other.
pub fn kinds(scores: &[f32]) -> [f32; KINDS.len()] {
    let mut kinds = KINDS.map(|(_, ranges)| ranges.iter().flat_map(|r| scores[r.clone()].iter().copied()).fold(0.0, f32::max));
    let music = kinds[KINDS.iter().position(|k| k.0 == "music").unwrap_or(0)];
    let moods = &mut kinds[MOODS..MOODS + MOOD_COUNT];
    let total: f32 = moods.iter().sum();
    for mood in moods {
        *mood = music * *mood / total.max(MOOD_FLOOR);
    }
    kinds
}

struct Layer {
    stride: usize,
    /// Depthwise 3x3: [9][channels], then its bias.
    dw: Vec<f32>,
    dwb: Vec<f32>,
    /// Pointwise: [in][out], then its bias.
    pw: Vec<f32>,
    pwb: Vec<f32>,
}

pub struct SoundNet {
    names: Vec<String>,
    hann: Vec<f32>,
    /// [FFT / 2 + 1][MEL_BANDS].
    mel: Vec<f32>,
    /// The FFT's twiddles, e^(-2πik/FFT) for k below FFT / 2.
    twiddles: Vec<(f32, f32)>,
    /// The first layer, an ordinary 3x3 convolution on the spectrogram: [9][32], then its bias.
    first: (Vec<f32>, Vec<f32>),
    layers: Vec<Layer>,
    /// [1024][CLASSES], then the bias.
    logits: (Vec<f32>, Vec<f32>),
}

impl SoundNet {
    pub fn new() -> Result<Self> {
        let bytes = super::models::load(WEIGHTS)?;
        let names = read_metadata(&bytes, "classes")?.lines().map(str::to_owned).collect::<Vec<_>>();
        if names.len() != CLASSES {
            bail!("{WEIGHTS}: {} class names", names.len());
        }
        let t = read_safetensors(&bytes)?;
        let get = |name: &str, len: usize| -> Result<Vec<f32>> {
            let (_, v) = t.get(name).with_context(|| format!("{WEIGHTS}: no {name}"))?;
            if v.len() != len {
                bail!("{WEIGHTS}: {name} has {} values, not {len}", v.len());
            }
            Ok(v.clone())
        };
        // [out][9] as [9][out], so a tap's channels are together.
        let taps = |v: Vec<f32>, c: usize| (0..9 * c).map(|i| v[(i % c) * 9 + i / c]).collect::<Vec<_>>();
        let first = (taps(get("l1.w", 32 * 9)?, 32), get("l1.b", 32)?);
        let mut layers = Vec::new();
        let mut channels = 32;
        for (n, &(stride, out)) in LAYERS.iter().enumerate() {
            let p = format!("l{}", n + 2);
            layers.push(Layer {
                stride,
                dw: taps(get(&format!("{p}.dw"), channels * 9)?, channels),
                dwb: get(&format!("{p}.dwb"), channels)?,
                pw: get(&format!("{p}.pw"), channels * out)?,
                pwb: get(&format!("{p}.pwb"), out)?,
            });
            channels = out;
        }
        let logits = (get("logits.w", channels * CLASSES)?, get("logits.b", CLASSES)?);
        let hann = (0..WINDOW).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / WINDOW as f32).cos()).collect();
        let twiddles = (0..FFT / 2).map(|k| (-std::f32::consts::TAU * k as f32 / FFT as f32).sin_cos()).map(|(s, c)| (c, s)).collect();
        Ok(Self { names, hann, mel: mel_matrix(), twiddles, first, layers, logits })
    }

    /// The classes' names, as in AudioSet.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The log mel spectrogram of `SAMPLES` samples: [FRAMES][MEL_BANDS].
    pub fn log_mel(&self, samples: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0; FRAMES * MEL_BANDS];
        let mut re = [0f32; FFT];
        let mut im = [0f32; FFT];
        for (f, row) in out.as_chunks_mut::<MEL_BANDS>().0.iter_mut().enumerate() {
            re.fill(0.0);
            im.fill(0.0);
            for (i, (r, s)) in re.iter_mut().zip(&samples[f * STEP..f * STEP + WINDOW]).enumerate() {
                *r = s * self.hann[i];
            }
            self.fft(&mut re, &mut im);
            for (k, weights) in self.mel.as_chunks::<MEL_BANDS>().0.iter().enumerate() {
                let magnitude = (re[k] * re[k] + im[k] * im[k]).sqrt();
                for (m, w) in row.iter_mut().zip(weights) {
                    *m += magnitude * w;
                }
            }
            for m in row.iter_mut() {
                *m = (*m + 0.001).ln();
            }
        }
        out
    }

    /// Scores (0 to 1) for every class, for `SAMPLES` samples at `RATE`.
    pub fn classify(&self, samples: &[f32]) -> Vec<f32> {
        self.run(&self.log_mel(samples), |_, _| {})
    }

    /// The network on a spectrogram, showing `layer` each layer's output.
    fn run(&self, spectrogram: &[f32], mut layer: impl FnMut(usize, &[f32])) -> Vec<f32> {
        let mut c = self.first.1.len();
        let mut x = Vec::new();
        let mut y = Vec::new();
        let (mut h, mut w) = depthwise(spectrogram, (FRAMES, MEL_BANDS), 1, c, &self.first.0, &self.first.1, 2, &mut x);
        layer(1, &x);
        for (n, l) in self.layers.iter().enumerate() {
            (h, w) = depthwise(&x, (h, w), c, c, &l.dw, &l.dwb, l.stride, &mut y);
            let out = l.pwb.len();
            pointwise(&y, c, &l.pw, &l.pwb, &mut x);
            c = out;
            layer(n + 2, &x);
        }
        let mut mean = vec![0.0; c];
        for pixel in x.chunks_exact(c) {
            for (m, v) in mean.iter_mut().zip(pixel) {
                *m += v / (h * w) as f32;
            }
        }
        let mut scores = Vec::new();
        pointwise_with(&mean, c, &self.logits.0, &self.logits.1, &mut scores, |v| 1.0 / (1.0 + (-v).exp()));
        scores
    }

    /// A fast Fourier transform of `FFT` values, in place.
    fn fft(&self, re: &mut [f32; FFT], im: &mut [f32; FFT]) {
        let bits = FFT.trailing_zeros();
        for i in 0..FFT {
            let j = i.reverse_bits() >> (usize::BITS - bits);
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= FFT {
            let stride = FFT / size;
            for start in (0..FFT).step_by(size) {
                for k in 0..size / 2 {
                    let (wr, wi) = self.twiddles[k * stride];
                    let (a, b) = (start + k, start + k + size / 2);
                    let (tr, ti) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                    (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                    (re[a], im[a]) = (re[a] + tr, im[a] + ti);
                }
            }
            size *= 2;
        }
    }
}

/// tf.signal.linear_to_mel_weight_matrix as YAMNet uses it (64 bands, 125 Hz to 7.5 kHz, the
/// HTK mel scale, the 0 Hz bin left out): [FFT / 2 + 1][MEL_BANDS].
fn mel_matrix() -> Vec<f32> {
    let mel = |hz: f32| 1127.0 * (hz / 700.0).ln_1p();
    let bins = FFT / 2 + 1;
    let (low, high) = (mel(125.0), mel(7500.0));
    let edge = |i: usize| low + (high - low) * i as f32 / (MEL_BANDS + 1) as f32;
    let mut out = vec![0.0; bins * MEL_BANDS];
    for k in 1..bins {
        let at = mel(RATE as f32 / 2.0 * k as f32 / (bins - 1) as f32);
        for b in 0..MEL_BANDS {
            let (lower, centre, upper) = (edge(b), edge(b + 1), edge(b + 2));
            out[k * MEL_BANDS + b] = ((at - lower) / (centre - lower)).min((upper - at) / (upper - centre)).max(0.0);
        }
    }
    out
}

/// A 3x3 convolution of each channel by itself (or, with one channel in, of it into `out`
/// channels), with TensorFlow's "same" padding, then the bias and ReLU. Activations are rows of
/// channels; `k` is [9][out]. Returns the new size.
#[allow(clippy::too_many_arguments)]
fn depthwise(x: &[f32], (h, w): (usize, usize), c: usize, out: usize, k: &[f32], b: &[f32], stride: usize, y: &mut Vec<f32>) -> (usize, usize) {
    let (oh, ow) = (h.div_ceil(stride), w.div_ceil(stride));
    // Stride 1 pads one each side; stride 2 (on even sizes) only one after.
    let before = usize::from(stride == 1);
    y.clear();
    y.resize(oh * ow * out, 0.0);
    for oy in 0..oh {
        for ox in 0..ow {
            let acc = &mut y[(oy * ow + ox) * out..][..out];
            acc.copy_from_slice(b);
            for dy in 0..3 {
                let Some(iy) = (oy * stride + dy).checked_sub(before).filter(|&iy| iy < h) else { continue };
                for dx in 0..3 {
                    let Some(ix) = (ox * stride + dx).checked_sub(before).filter(|&ix| ix < w) else { continue };
                    let tap = &k[(dy * 3 + dx) * out..][..out];
                    let src = &x[(iy * w + ix) * c..][..c];
                    if c == 1 {
                        for (a, t) in acc.iter_mut().zip(tap) {
                            *a += src[0] * t;
                        }
                    } else {
                        for ((a, s), t) in acc.iter_mut().zip(src).zip(tap) {
                            *a += s * t;
                        }
                    }
                }
            }
            for a in acc.iter_mut() {
                *a = a.max(0.0);
            }
        }
    }
    (oh, ow)
}

/// A 1x1 convolution (`k` [in][out]) on rows of `c` channels, then the bias and ReLU.
fn pointwise(x: &[f32], c: usize, k: &[f32], b: &[f32], y: &mut Vec<f32>) {
    pointwise_with(x, c, k, b, y, |v| v.max(0.0));
}

fn pointwise_with(x: &[f32], c: usize, k: &[f32], b: &[f32], y: &mut Vec<f32>, then: impl Fn(f32) -> f32) {
    let out = b.len();
    y.clear();
    y.resize(x.len() / c * out, 0.0);
    for (row, acc) in x.chunks_exact(c).zip(y.chunks_exact_mut(out)) {
        acc.copy_from_slice(b);
        for (i, &v) in row.iter().enumerate() {
            // After a ReLU, many are 0.
            if v != 0.0 {
                for (a, t) in acc.iter_mut().zip(&k[i * out..][..out]) {
                    *a += v * t;
                }
            }
        }
        for a in acc.iter_mut() {
            *a = then(*a);
        }
    }
}

/// A string from a safetensors file's metadata.
fn read_metadata(bytes: &[u8], key: &str) -> Result<String> {
    let (header, _) = super::safetensors_header(bytes)?;
    header["__metadata__"][key].as_str().map(str::to_owned).with_context(|| format!("no {key} in the metadata"))
}

/// Brings sound at any rate down to `RATE`, through a windowed-sinc low-pass that keeps what
/// YAMNet listens to (up to 7.5 kHz).
pub struct Resampler {
    rate: u32,
    /// [PHASES + 1][TAPS]: the filter at each fraction of a sample.
    table: Vec<f32>,
    waiting: VecDeque<f32>,
    /// Where the next output falls, in input samples from the front of `waiting`.
    at: f64,
}

const TAPS: usize = 64;
const PHASES: usize = 256;
const CUTOFF_HZ: f64 = 7_800.0;

impl Resampler {
    pub fn new(rate: u32) -> Self {
        let fc = CUTOFF_HZ.min(0.45 * f64::from(rate)) / f64::from(rate);
        let half = (TAPS / 2) as f64;
        let mut table = vec![0.0; (PHASES + 1) * TAPS];
        for (p, row) in table.as_chunks_mut::<TAPS>().0.iter_mut().enumerate() {
            let frac = p as f64 / PHASES as f64;
            for (j, v) in row.iter_mut().enumerate() {
                // Tap j is input sample (i + j - half + 1), where i is the one at or before the output.
                let d = j as f64 - half + 1.0 - frac;
                let sinc = if d == 0.0 { 1.0 } else { (std::f64::consts::TAU * fc * d).sin() / (std::f64::consts::TAU * fc * d) };
                let window = 0.5 + 0.5 * (std::f64::consts::PI * d / (half + 1.0)).cos();
                *v = (sinc * window) as f32;
            }
            let sum: f32 = row.iter().sum();
            row.iter_mut().for_each(|v| *v /= sum);
        }
        Self { rate, table, waiting: VecDeque::new(), at: (TAPS / 2 - 1) as f64 }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Takes some sound at the rate it was made for, and adds what that gives at `RATE` to `out`.
    pub fn push(&mut self, mono: &[f32], out: &mut Vec<f32>) {
        if self.rate == RATE {
            out.extend_from_slice(mono);
            return;
        }
        self.waiting.extend(mono);
        let step = f64::from(self.rate) / f64::from(RATE);
        let (front, back) = self.waiting.as_slices();
        let get = |i: usize| if i < front.len() { front[i] } else { back[i - front.len()] };
        while (self.at as usize) + TAPS / 2 < self.waiting.len() {
            let i = self.at as usize;
            let phase = ((self.at - i as f64) * PHASES as f64).round() as usize;
            let taps = &self.table[phase * TAPS..][..TAPS];
            let start = i + 1 - TAPS / 2;
            out.push(taps.iter().enumerate().map(|(j, t)| t * get(start + j)).sum());
            self.at += step;
        }
        let done = (self.at as usize).saturating_sub(TAPS / 2 - 1);
        self.waiting.drain(..done);
        self.at -= done as f64;
    }
}

/// Listens as sound comes, and says what kind it is every `HOP` samples once there's enough.
pub struct Listener {
    net: SoundNet,
    resampler: Resampler,
    /// The latest `SAMPLES` at `RATE`.
    heard: VecDeque<f32>,
    new: usize,
    scratch: Vec<f32>,
}

impl Listener {
    pub fn new() -> Result<Self> {
        Ok(Self { net: SoundNet::new()?, resampler: Resampler::new(48_000), heard: VecDeque::new(), new: 0, scratch: Vec::new() })
    }

    pub fn names(&self) -> &[String] {
        self.net.names()
    }

    /// Some more sound, at `rate`.
    pub fn push(&mut self, rate: u32, mono: &[f32]) {
        if rate != self.resampler.rate() {
            self.resampler = Resampler::new(rate);
        }
        self.scratch.clear();
        self.resampler.push(mono, &mut self.scratch);
        self.heard.extend(&self.scratch);
        self.new += self.scratch.len();
        let extra = self.heard.len().saturating_sub(SAMPLES);
        self.heard.drain(..extra);
    }

    /// Every class's score for the latest second, if `HOP` new samples have come since last time.
    pub fn listen(&mut self) -> Option<Vec<f32>> {
        if self.heard.len() < SAMPLES || self.new < HOP {
            return None;
        }
        self.new = 0;
        Some(self.net.classify(self.heard.make_contiguous()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, rate: u32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize).map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin()).collect()
    }

    #[test]
    fn the_fft_matches_a_plain_dft() {
        let net = SoundNet::new().unwrap();
        let signal: Vec<f32> = (0..FFT).map(|i| ((i * 7919) % 101) as f32 / 50.0 - 1.0).collect();
        let (mut re, mut im) = ([0f32; FFT], [0f32; FFT]);
        re.copy_from_slice(&signal);
        net.fft(&mut re, &mut im);
        for k in [0, 1, 5, 100, 256] {
            let (mut r, mut i) = (0f64, 0f64);
            for (n, s) in signal.iter().enumerate() {
                let a = -std::f64::consts::TAU * (k * n) as f64 / FFT as f64;
                r += f64::from(*s) * a.cos();
                i += f64::from(*s) * a.sin();
            }
            assert!((f64::from(re[k]) - r).abs() < 1e-3 && (f64::from(im[k]) - i).abs() < 1e-3, "bin {k}");
        }
    }

    #[test]
    fn resampling_keeps_a_tone_and_drops_what_is_too_high() {
        let mut out = Vec::new();
        let mut r = Resampler::new(48_000);
        for chunk in tone(1000.0, 48_000, 1.0).chunks(480) {
            r.push(chunk, &mut out);
        }
        assert!((out.len() as i64 - 16_000).abs() < 40, "{} samples", out.len());
        let middle = &out[2000..14_000];
        let rms = (middle.iter().map(|v| v * v).sum::<f32>() / middle.len() as f32).sqrt();
        assert!((rms - 0.5 / 2f32.sqrt()).abs() < 0.01, "a 1 kHz tone came out at {rms}");
        // Output n is input sample 31 + 3n, the filter being 64 taps long.
        let err = (2000..14_000).map(|n| (out[n] - 0.5 * (std::f32::consts::TAU * 1000.0 * (31 + 3 * n) as f32 / 48_000.0).sin()).abs()).fold(0.0, f32::max);
        assert!(err < 0.01, "off by {err}");

        out.clear();
        let mut r = Resampler::new(44_100);
        r.push(&tone(12_000.0, 44_100, 1.0), &mut out);
        let rms = (out[2000..14_000].iter().map(|v| v * v).sum::<f32>() / 12_000.0).sqrt();
        assert!(rms < 0.01, "a 12 kHz tone came through at {rms}");
    }

    /// Listens to a recording as Flex would, printing what it hears every half second:
    /// `ffmpeg -i <film> -ss <time> -t 20 -ac 1 -ar 48000 -f f32le out.raw`, then
    /// `FLEX_SOUND_RAW=out.raw cargo test --release -p flex-depth listens_to -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a recording"]
    fn listens_to_a_recording() {
        let bytes = std::fs::read(std::env::var("FLEX_SOUND_RAW").expect("FLEX_SOUND_RAW")).unwrap();
        let samples: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        let mut listener = Listener::new().unwrap();
        for (i, chunk) in samples.chunks(480).enumerate() {
            listener.push(48_000, chunk);
            if let Some(scores) = listener.listen() {
                let mut found: Vec<(&str, f32)> = KINDS.iter().map(|k| k.0).zip(kinds(&scores)).filter(|k| k.1 >= 0.05).collect();
                found.sort_by(|a, b| b.1.total_cmp(&a.1));
                let best = (0..CLASSES).max_by(|&a, &b| scores[a].total_cmp(&scores[b])).unwrap();
                println!("{:5.2} s: {found:.2?} ({})", (i + 1) as f32 / 100.0, listener.names()[best]);
            }
        }
    }

    #[test]
    fn moods_share_the_music() {
        assert_eq!(KINDS[MOODS].0, "happy_music");
        assert_eq!(KINDS[MOODS + MOOD_COUNT - 1].0, "scary_music");
        let mut scores = vec![0.0; CLASSES];
        scores[132] = 0.8;
        scores[272] = 0.03;
        scores[276] = 0.01;
        let k = kinds(&scores);
        assert!((k[MOODS + 1] - 0.6).abs() < 1e-5 && (k[MOODS + 5] - 0.2).abs() < 1e-5, "{k:?}");
        // Hardly any mood at all counts for little.
        scores[272] = 0.003;
        scores[276] = 0.0;
        assert!(kinds(&scores)[MOODS + 1] < 0.1);
    }

    #[test]
    fn hears_a_dial_tone_and_silence() {
        let mut listener = Listener::new().unwrap();
        listener.push(48_000, &tone(440.0, 48_000, 1.2));
        let scores = listener.listen().expect("enough sound");
        let best = (0..CLASSES).max_by(|&a, &b| scores[a].total_cmp(&scores[b])).unwrap();
        assert!(listener.names()[best].starts_with("Telephone") || listener.names()[best] == "Dial tone", "{}", listener.names()[best]);
        assert!(listener.listen().is_none(), "nothing new");

        listener.push(48_000, &vec![0.0; 48_000]);
        let kinds = kinds(&listener.listen().unwrap());
        let silence = KINDS.iter().position(|k| k.0 == "silence").unwrap();
        assert!(kinds[silence] > 0.9, "{kinds:?}");
    }

    /// Against `tools/sound/reference.py`, stage by stage:
    /// `FLEX_SOUND_REF=ref.safetensors cargo test --release -p flex-depth sound_matches -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a reference file"]
    fn sound_matches_the_reference() {
        let reference = read_safetensors(&std::fs::read(std::env::var("FLEX_SOUND_REF").expect("FLEX_SOUND_REF")).unwrap()).unwrap();
        let net = SoundNet::new().unwrap();
        let samples = &reference["samples"].1;
        let spectrogram = net.log_mel(samples);
        let worst = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        let scale = |b: &[f32]| b.iter().map(|v| v.abs()).fold(0.0, f32::max).max(1e-6);
        let e = worst(&spectrogram, &reference["log_mel"].1);
        println!("log mel: worst {e:.5}");
        assert!(e < 1e-3);
        let start = std::time::Instant::now();
        let scores = net.run(&spectrogram, |n, x| {
            let r = &reference[&format!("l{n}")].1;
            assert_eq!(x.len(), r.len(), "layer {n}");
            let e = worst(x, r) / scale(r);
            println!("layer {n}: worst {e:.4} of the largest");
            assert!(e < 0.03, "layer {n}");
        });
        println!("the network took {:?} (with the checks)", start.elapsed());
        let e = worst(&scores, &reference["scores"].1);
        println!("scores: worst {e:.4}");
        assert!(e < 0.02);
    }
}
