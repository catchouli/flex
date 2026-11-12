//! Turning a voice into an instrument with Magenta's DDSP-VST (github.com/magenta/ddsp-vst,
//! Apache-2.0). Every 20 ms of sound at 16 kHz, a small pitch network (CREPE "micro") hears how
//! high and how loud the last 64 ms were; an instrument's network (a GRU trained on that
//! instrument) turns that into how loud each of 60 harmonics and 65 bands of noise should be;
//! and an oscillator bank and a noise filter play it.
//!
//! The networks are TFLite files, run here straight from their weights, as the plugin's graphs
//! lay them out (tract can't read them). What's around them follows the plugin's code.

use std::collections::HashMap;

use tract_tflite::tflite::{self, BuiltinOperator};

/// The sound they work on, and how much they move on and look at at a time.
pub const RATE: u32 = 16_000;
pub const HOP: usize = 320;
pub const FRAME: usize = 1024;
pub const HARMONICS: usize = 60;
pub const NOISE_BANDS: usize = 65;
const GRU: usize = 512;
const FC: usize = 256;
const BINS: usize = 360;

/// Where Flex downloads them from on first use, and what to expect: the pitch network from the
/// plugin's repository at a fixed commit, the instruments from Google's release of them.
pub const PITCH_FILE: &str = "ddsp-extract-features-micro.tflite";
pub const PITCH_URL: &str = "https://raw.githubusercontent.com/magenta/ddsp-vst/f2996e97f9469f3956a6b8e9d2d9b50b6555e1e9/models/pitch-detection/extract_features_micro.tflite";
pub const PITCH_BYTES: u64 = 671_569;
pub const PITCH_SHA256: &str = "98ade06772f9ca6ac64789fd843f14aeddf709084bbc76f060761b141837d0a2";
pub const INSTRUMENTS_FILE: &str = "DDSP-VST-Models.zip";
pub const INSTRUMENTS_URL: &str = "https://storage.googleapis.com/ddsp-vst/releases/DDSP-VST-Models.zip";
pub const INSTRUMENTS_BYTES: u64 = 76_669_561;
pub const INSTRUMENTS_SHA256: &str = "bbb0f4a61be86d8b52460b21210016945899d78f031b99a844a3771e9a2a5308";

/// The instruments, by id, with their file in the zip.
pub const INSTRUMENTS: [(&str, &str); 11] = [
    ("flute", "Flute"),
    ("violin", "Violin"),
    ("trumpet", "Trumpet"),
    ("saxophone", "Saxophone"),
    ("clarinet", "Clarinet"),
    ("trombone", "Trombone"),
    ("tuba", "Tuba"),
    ("bassoon", "Bassoon"),
    ("melodica", "Melodica"),
    ("sitar", "Sitar"),
    ("vowels", "Vowels"),
];

// ---- Reading TFLite weights ---------------------------------------------------------------

/// A constant tensor: its shape and values.
#[derive(Clone, Debug, Default)]
struct Const {
    shape: Vec<usize>,
    values: Vec<f32>,
}

/// One layer of a graph: what it is, its inputs (constant ones with their values) and options.
struct Op {
    code: BuiltinOperator,
    /// Each input: its tensor index in the subgraph, and its value if it's constant.
    inputs: Vec<(i32, Option<Const>)>,
    stride: usize,
}

impl Op {
    fn constant(&self, i: usize) -> Option<&Const> {
        self.inputs.get(i).and_then(|(_, c)| c.as_ref())
    }
}

/// Each subgraph's layers, in order.
fn read_graphs(bytes: &[u8]) -> Result<Vec<Vec<Op>>, String> {
    let model = tflite::root_as_model(bytes).map_err(|e| format!("not a TFLite file: {e}"))?;
    let codes = model.operator_codes().ok_or("no operator codes")?;
    let buffers = model.buffers().ok_or("no buffers")?;
    let mut graphs = Vec::new();
    for g in model.subgraphs().ok_or("no subgraphs")? {
        let tensors = g.tensors().ok_or("no tensors")?;
        let mut ops = Vec::new();
        for op in g.operators().ok_or("no operators")? {
            let code = codes.get(op.opcode_index() as usize);
            let code = code.builtin_code().max(BuiltinOperator(code.deprecated_builtin_code() as i32));
            let inputs = op
                .inputs()
                .map(|ins| {
                    ins.iter()
                        .map(|i| {
                            let value = (i >= 0).then(|| tensors.get(i as usize)).and_then(|t| {
                                let data = buffers.get(t.buffer() as usize).data()?;
                                (t.type_() == tflite::TensorType::FLOAT32 && !data.is_empty()).then(|| Const {
                                    shape: t.shape().map(|s| s.iter().map(|d| d.max(0) as usize).collect()).unwrap_or_default(),
                                    values: data.bytes().as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect(),
                                })
                            });
                            (i, value)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let stride = op.builtin_options_as_conv_2_doptions().map_or(1, |o| o.stride_h().max(1) as usize);
            ops.push(Op { code, inputs, stride });
        }
        graphs.push(ops);
    }
    Ok(graphs)
}

/// A dense layer: weights [outputs][inputs] and biases.
#[derive(Clone, Default)]
struct Dense {
    inputs: usize,
    weight: Vec<f32>,
    bias: Vec<f32>,
}

impl Dense {
    fn from(op: &Op) -> Result<Self, String> {
        let (Some(w), Some(b)) = (op.constant(1), op.constant(2)) else { return Err("a dense layer without its weights".into()) };
        Ok(Self { inputs: w.shape.get(1).copied().unwrap_or(1), weight: w.values.clone(), bias: b.values.clone() })
    }

    fn run(&self, x: &[f32], y: &mut [f32]) {
        for (o, y) in y.iter_mut().enumerate() {
            *y = self.bias[o] + dot(&self.weight[o * self.inputs..][..self.inputs], x);
        }
    }
}

/// A dot product, in eight lanes so it vectorises.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let (a8, a_rest) = a.as_chunks::<8>();
    let (b8, b_rest) = b[..a.len()].as_chunks::<8>();
    let mut lanes = [0.0f32; 8];
    for (x, y) in a8.iter().zip(b8) {
        for l in 0..8 {
            lanes[l] += x[l] * y[l];
        }
    }
    lanes.iter().sum::<f32>() + a_rest.iter().zip(b_rest).map(|(x, y)| x * y).sum::<f32>()
}

// ---- The pitch network ----------------------------------------------------------------------

/// One of CREPE's convolutions over time (ReLU, then batch norm folded to a scale and shift, then
/// max-pooled by two): weights [outputs][kernel][inputs].
struct Conv {
    outputs: usize,
    kernel: usize,
    inputs: usize,
    stride: usize,
    weight: Vec<f32>,
    bias: Vec<f32>,
    scale: Vec<f32>,
    shift: Vec<f32>,
}

impl Conv {
    /// `x` [len][inputs] → [len / stride / 2][outputs], padded as TensorFlow's "SAME".
    fn run(&self, x: &[f32], len: usize) -> (Vec<f32>, usize) {
        let out = len.div_ceil(self.stride);
        let pad = ((out - 1) * self.stride + self.kernel).saturating_sub(len) / 2;
        let mut y = vec![0.0; out * self.outputs];
        for t in 0..out {
            // The part of the kernel that lands inside the sound: one run of weights against one
            // run of the input, as both are laid out kernel-step by kernel-step.
            let start = (t * self.stride) as isize - pad as isize;
            let first = (-start).max(0) as usize;
            let last = ((len as isize - start).min(self.kernel as isize)).max(first as isize) as usize;
            let xs = &x[(start + first as isize) as usize * self.inputs..][..(last - first) * self.inputs];
            for o in 0..self.outputs {
                let w = &self.weight[(o * self.kernel + first) * self.inputs..][..(last - first) * self.inputs];
                let sum = self.bias[o] + dot(w, xs);
                y[t * self.outputs + o] = sum.max(0.0) * self.scale[o] + self.shift[o];
            }
        }
        let pooled = out / 2;
        let mut p = vec![0.0; pooled * self.outputs];
        for t in 0..pooled {
            for o in 0..self.outputs {
                p[t * self.outputs + o] = y[2 * t * self.outputs + o].max(y[(2 * t + 1) * self.outputs + o]);
            }
        }
        (p, pooled)
    }
}

/// What the pitch network hears in a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Heard {
    pub f0_hz: f32,
    /// Its loudness in dB (no lower than -80), and that as the instruments take it (0–1).
    pub db: f32,
    pub loudness: f32,
    /// How sure it is of the pitch: the strongest bin, 0–1.
    pub confidence: f32,
}

/// CREPE micro, with the power the plugin works out beside it.
pub struct Pitch {
    convs: Vec<Conv>,
    classifier: Dense,
    /// Each of the classifier's bins, in cents above 10 Hz.
    cents: Vec<f32>,
}

impl Pitch {
    /// Loads it from DDSP-VST's `extract_features_micro.tflite`.
    pub fn load(bytes: &[u8]) -> Result<Self, String> {
        let graphs = read_graphs(bytes)?;
        let ops = graphs.first().ok_or("no graph")?;
        let mut convs: Vec<Conv> = Vec::new();
        let (mut classifier, mut cents) = (None, None);
        // Batch norm's scale and shift come as a multiply and an add right after each convolution.
        let mut norm = 0;
        for op in ops {
            match op.code {
                BuiltinOperator::CONV_2D => {
                    let (Some(w), Some(b)) = (op.constant(1), op.constant(2)) else { return Err("a convolution without its weights".into()) };
                    let [outputs, kernel, _, inputs] = w.shape[..] else { return Err(format!("a convolution shaped {:?}", w.shape)) };
                    convs.push(Conv { outputs, kernel, inputs, stride: op.stride, weight: w.values.clone(), bias: b.values.clone(), scale: Vec::new(), shift: Vec::new() });
                    norm = 2;
                }
                BuiltinOperator::MUL | BuiltinOperator::ADD if norm > 0 => {
                    let (Some(c), Some(conv)) = (op.constant(1), convs.last_mut()) else { continue };
                    if c.values.len() == conv.outputs {
                        if op.code == BuiltinOperator::MUL { conv.scale = c.values.clone() } else { conv.shift = c.values.clone() }
                        norm -= 1;
                    }
                }
                BuiltinOperator::FULLY_CONNECTED => classifier = Some(Dense::from(op)?),
                BuiltinOperator::GATHER => {
                    if let Some(c) = op.constant(0).filter(|c| c.values.len() == BINS) {
                        cents = Some(c.values.clone());
                    }
                }
                _ => {}
            }
        }
        let classifier = classifier.ok_or("no classifier")?;
        let cents = cents.ok_or("no table of pitches")?;
        if convs.len() != 6 || convs.iter().any(|c| c.scale.len() != c.outputs || c.shift.len() != c.outputs) || classifier.bias.len() != BINS {
            return Err("not the pitch network expected".into());
        }
        Ok(Self { convs, classifier, cents })
    }

    /// Hears a frame of `FRAME` samples at `RATE`.
    pub fn hear(&self, frame: &[f32]) -> Heard {
        let n = frame.len() as f32;
        let mean = frame.iter().sum::<f32>() / n;
        let power = frame.iter().map(|x| x * x).sum::<f32>() / n;
        let db = (10.0 * power.max(1e-8).log10()).max(-80.0);
        let variance = frame.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n;
        let std = if variance.abs() > 0.0 { variance.sqrt() } else { 1e-8 };
        let mut x: Vec<f32> = frame.iter().map(|v| (v - mean) / std).collect();
        let mut len = frame.len();
        for conv in &self.convs {
            (x, len) = conv.run(&x, len);
        }
        let mut p = vec![0.0; BINS];
        self.classifier.run(&x, &mut p);
        for v in &mut p {
            *v = 1.0 / (1.0 + (-*v).exp());
        }
        // The weighted mean of the bins around the strongest.
        let best = (0..BINS).fold(0, |b, i| if p[i] > p[b] { i } else { b });
        let (mut sum, mut weight) = (0.0, 0.0);
        for k in 0..10 {
            let i = (best as isize - 4 + k).clamp(0, BINS as isize - 1) as usize;
            sum += p[i] * self.cents[i];
            weight += p[i];
        }
        let f0_hz = 10.0 * 2f32.powf(sum / weight / 1200.0);
        Heard { f0_hz, db, loudness: db / 80.0 + 1.0, confidence: p[best] }
    }
}

/// A pitch as the instruments take it: MIDI notes over 127.
pub fn pitch_scaled(f0_hz: f32) -> f32 {
    let midi = 12.0 * (f0_hz.clamp(8.18, 12_543.84).log2() - 440f32.log2()) + 69.0;
    midi / 127.0
}

// ---- An instrument --------------------------------------------------------------------------

/// What an instrument says to play for a frame.
#[derive(Clone, Debug, Default)]
pub struct Controls {
    pub amplitude: f32,
    /// How the amplitude's shared out over the harmonics (adding up to 1, none above Nyquist).
    pub harmonics: Vec<f32>,
    pub noise: Vec<f32>,
}

/// Layer norm over a vector, then its scale and shift, then a leaky ReLU.
struct Norm {
    scale: Vec<f32>,
    shift: Vec<f32>,
}

impl Norm {
    fn run(&self, x: &mut [f32]) {
        let n = x.len() as f32;
        let mean = x.iter().sum::<f32>() / n;
        let var = x.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n;
        let r = 1.0 / (var + 0.001).sqrt();
        for ((v, s), b) in x.iter_mut().zip(&self.scale).zip(&self.shift) {
            let y = (*v - mean) * r * s + b;
            *v = if y < 0.0 { 0.2 * y } else { y };
        }
    }
}

/// An instrument's network.
pub struct Instrument {
    pw: Dense,
    f0: Dense,
    norms: [Norm; 3],
    /// The GRU's weights for its input and its last output, gates in the order update, reset,
    /// new (Keras's, with the reset applied after the recurrent weights).
    gru_x: Dense,
    gru_h: Dense,
    out: Dense,
    last: Dense,
    /// The pitches it was trained on, lowest and highest, in Hz, and how loud its notes were
    /// (dB).
    pub range: (f32, f32),
    pub power: (f32, f32),
}

impl Instrument {
    /// Loads one of DDSP-VST's instruments from its `.tflite` (with the zip of its metadata on
    /// the end).
    pub fn load(bytes: &[u8]) -> Result<Self, String> {
        let graphs = read_graphs(bytes)?;
        let main = graphs.first().ok_or("no graph")?;
        let mut dense = Vec::new();
        let mut norms = Vec::new();
        let mut scale = None;
        for op in main {
            match op.code {
                BuiltinOperator::FULLY_CONNECTED => dense.push(Dense::from(op)?),
                BuiltinOperator::MUL => scale = op.constant(1).filter(|c| c.values.len() == FC).map(|c| c.values.clone()),
                BuiltinOperator::ADD => {
                    if let (Some(s), Some(c)) = (scale.take(), op.constant(1).filter(|c| c.values.len() == FC)) {
                        norms.push(Norm { scale: s, shift: c.values.clone() });
                    }
                }
                _ => scale = None,
            }
        }
        // The loop body: the one with the GRU's two dense layers, the second on its last output
        // (the body's fourth input).
        let (mut gru_x, mut gru_h) = (None, None);
        for ops in &graphs[1..] {
            for op in ops.iter().filter(|op| op.code == BuiltinOperator::FULLY_CONNECTED) {
                let d = Dense::from(op)?;
                if op.inputs.first().map(|(i, _)| *i) == Some(3) { gru_h = Some(d) } else { gru_x = Some(d) }
            }
        }
        let (Some(gru_x), Some(gru_h)) = (gru_x, gru_h) else { return Err("no GRU".into()) };
        let [pw, f0, out, last]: [Dense; 4] = dense.try_into().map_err(|_| "not the instrument network expected")?;
        let norms: [Norm; 3] = norms.try_into().map_err(|_| "not the instrument network expected")?;
        if gru_x.bias.len() != 3 * GRU || last.bias.len() != 1 + HARMONICS + NOISE_BANDS {
            return Err("not the instrument network expected".into());
        }
        let (range, power) = metadata(bytes).unwrap_or(((100.0, 1000.0), (-40.0, -20.0)));
        Ok(Self { pw, f0, norms, gru_x, gru_h, out, last, range, power })
    }

    /// One frame: the pitch (as `pitch_scaled`) and loudness (0–1) heard, and the GRU's state.
    pub fn play(&self, f0_scaled: f32, loudness: f32, state: &mut [f32]) -> Controls {
        let mut a = vec![0.0; FC];
        let mut b = vec![0.0; FC];
        self.pw.run(&[loudness], &mut a);
        self.norms[0].run(&mut a);
        self.f0.run(&[f0_scaled], &mut b);
        self.norms[1].run(&mut b);
        let x: Vec<f32> = a.iter().chain(&b).copied().collect();
        let mut gx = vec![0.0; 3 * GRU];
        let mut gh = vec![0.0; 3 * GRU];
        self.gru_x.run(&x, &mut gx);
        self.gru_h.run(state, &mut gh);
        let sigmoid = |v: f32| 1.0 / (1.0 + (-v).exp());
        for k in 0..GRU {
            let z = sigmoid(gx[k] + gh[k]);
            let r = sigmoid(gx[GRU + k] + gh[GRU + k]);
            let n = (gx[2 * GRU + k] + r * gh[2 * GRU + k]).tanh();
            state[k] = z * state[k] + (1.0 - z) * n;
        }
        let all: Vec<f32> = x.iter().chain(state.iter()).copied().collect();
        let mut h = vec![0.0; FC];
        self.out.run(&all, &mut h);
        self.norms[2].run(&mut h);
        let mut y = vec![0.0; 1 + HARMONICS + NOISE_BANDS];
        self.last.run(&h, &mut y);
        // DDSP's "exp sigmoid": 2 sigmoid(x)^ln 10 + 1e-7.
        let exp_sigmoid = |v: f32| 2.0 * sigmoid(v).powf(std::f32::consts::LN_10) + 1e-7;
        let f0_hz = 440.0 * 2f32.powf((f0_scaled * 127.0 - 69.0) / 12.0);
        let mut harmonics: Vec<f32> = (0..HARMONICS).map(|i| if f0_hz * (i + 1) as f32 >= RATE as f32 / 2.0 { 0.0 } else { exp_sigmoid(y[1 + i]) }).collect();
        let total: f32 = harmonics.iter().sum();
        let total = if total == 0.0 { 1e-7 } else { total };
        for h in &mut harmonics {
            *h /= total;
        }
        Controls { amplitude: exp_sigmoid(y[0]), harmonics, noise: y[1 + HARMONICS..].iter().map(|&v| exp_sigmoid(v - 5.0)).collect() }
    }
}

/// The pitches an instrument was trained on, and how loud, from the `metadata.json` in the zip on
/// the end of its file.
fn metadata(bytes: &[u8]) -> Option<((f32, f32), (f32, f32))> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("metadata.json").ok()?, &mut text).ok()?;
    let json: HashMap<String, serde_json::Value> = serde_json::from_str(&text).ok()?;
    let get = |k: &str| json.get(k)?.as_f64().map(|v| v as f32);
    Some(((get("mean_min_pitch_note_hz")?, get("mean_max_pitch_note_hz")?), (get("mean_min_power_note")?, get("mean_max_power_note")?)))
}

/// The instruments' zip: one instrument's file, by id.
pub fn instrument_file(zip: impl std::io::Read + std::io::Seek, id: &str) -> Result<Vec<u8>, String> {
    let (_, name) = INSTRUMENTS.iter().find(|(i, _)| *i == id).ok_or_else(|| format!("no instrument {id}"))?;
    let mut zip = zip::ZipArchive::new(zip).map_err(|e| e.to_string())?;
    let mut file = zip.by_name(&format!("DDSP-VST-Models/{name}.tflite")).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

// ---- Playing it -------------------------------------------------------------------------------

/// The plugin's harmonic synthesiser: an oscillator per harmonic, each frame's values eased in
/// over the first half of the hop and held for the second.
pub struct Harmonic {
    phase: f32,
    f0: Option<f32>,
    amps: Vec<f32>,
}

impl Default for Harmonic {
    fn default() -> Self {
        Self { phase: 0.0, f0: None, amps: vec![0.0; HARMONICS] }
    }
}

fn midway(from: f32, to: f32, i: usize) -> f32 {
    let half = HOP / 2;
    if i < half { from + (to - from) * i as f32 / half as f32 } else { to }
}

impl Harmonic {
    pub fn render(&mut self, c: &Controls, f0_hz: f32, out: &mut [f32]) {
        let amps: Vec<f32> = c.harmonics.iter().map(|h| h * c.amplitude).collect();
        let from_f0 = self.f0.unwrap_or(f0_hz);
        let step = std::f32::consts::TAU / RATE as f32;
        for (i, y) in out.iter_mut().enumerate().take(HOP) {
            self.phase = (self.phase + midway(from_f0, f0_hz, i) * step) % std::f32::consts::TAU;
            let mut sum = 0.0;
            for (h, (&a0, &a1)) in self.amps.iter().zip(&amps).enumerate() {
                sum += (self.phase * (h + 1) as f32).sin() * midway(a0, a1, i);
            }
            *y = sum;
        }
        self.f0 = Some(f0_hz);
        self.amps = amps;
    }
}

/// The plugin's noise: white noise through a filter made from the bands' levels each frame.
pub struct Noise {
    seed: u32,
    /// The filter's impulse response's spectrum, and the noise's.
    fft: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
    ifft: std::sync::Arc<dyn realfft::ComplexToReal<f32>>,
    small: std::sync::Arc<dyn realfft::ComplexToReal<f32>>,
    window: Vec<f32>,
}

/// The filter's impulse response, and the length it's convolved at.
const IR: usize = (NOISE_BANDS - 1) * 2;
const CONVOLVE: usize = 512;

impl Default for Noise {
    fn default() -> Self {
        let mut planner = realfft::RealFftPlanner::<f32>::new();
        // A zero-phase Hann window: centred on the first sample.
        let mut window: Vec<f32> = (0..IR).map(|i| 0.5 * (1.0 - (std::f32::consts::TAU * i as f32 / IR as f32).cos())).collect();
        window.rotate_left(IR / 2);
        Self { seed: 42, fft: planner.plan_fft_forward(CONVOLVE), ifft: planner.plan_fft_inverse(CONVOLVE), small: planner.plan_fft_inverse(IR), window }
    }
}

impl Noise {
    fn random(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    pub fn render(&mut self, bands: &[f32], out: &mut [f32]) {
        use realfft::num_complex::Complex32;
        // The impulse response: the bands as a zero-phase spectrum, windowed, then made causal.
        let mut spec: Vec<Complex32> = bands.iter().map(|&m| Complex32::new(m, 0.0)).collect();
        let mut ir = vec![0.0; IR];
        let _ = self.small.process(&mut spec, &mut ir);
        for (x, w) in ir.iter_mut().zip(&self.window) {
            *x *= w / IR as f32;
        }
        ir.rotate_left(IR / 2);
        let mut padded = vec![0.0; CONVOLVE];
        padded[..IR].copy_from_slice(&ir);
        let mut noise: Vec<f32> = (0..CONVOLVE).map(|_| self.random()).collect();
        let mut a = self.fft.make_output_vec();
        let mut b = self.fft.make_output_vec();
        let _ = self.fft.process(&mut noise, &mut a);
        let _ = self.fft.process(&mut padded, &mut b);
        for (x, y) in a.iter_mut().zip(&b) {
            *x *= y;
        }
        a[0].im = 0.0;
        a[CONVOLVE / 2].im = 0.0;
        let mut y = vec![0.0; CONVOLVE];
        let _ = self.ifft.process(&mut a, &mut y);
        // Past the filter's delay.
        let start = (IR - 1) / 2 - 1;
        for (o, v) in out.iter_mut().zip(&y[start..]) {
            *o = v / CONVOLVE as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &str) -> Vec<f32> {
        std::fs::read(path).unwrap().as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
    }

    /// Against TensorFlow's TFLite interpreter (`ddsp_ref.py`): `FLEX_DDSP_PITCH` and
    /// `FLEX_DDSP_INSTRUMENT` the two .tflite files, `FLEX_DDSP_IN` raw f32 at 16 kHz, and
    /// `FLEX_DDSP_REF` what the reference made of it.
    #[test]
    #[ignore]
    fn matches_tflite() {
        let pitch = Pitch::load(&std::fs::read(std::env::var("FLEX_DDSP_PITCH").unwrap()).unwrap()).unwrap();
        let inst = Instrument::load(&std::fs::read(std::env::var("FLEX_DDSP_INSTRUMENT").unwrap()).unwrap()).unwrap();
        println!("range {:?}", inst.range);
        let x = read(&std::env::var("FLEX_DDSP_IN").unwrap());
        let reference = read(&std::env::var("FLEX_DDSP_REF").unwrap());
        let row = 6 + HARMONICS + NOISE_BANDS;
        let padded: Vec<f32> = std::iter::repeat_n(0.0, FRAME).chain(x).collect();
        let mut state = vec![0.0; GRU];
        let (mut worst_f0, mut worst_db, mut worst_amp, mut worst_h, mut worst_n) = (0f32, 0f32, 0f32, 0f32, 0f32);
        let start = std::time::Instant::now();
        let mut frames = 0;
        for (n, r) in reference.chunks_exact(row).enumerate() {
            let heard = pitch.hear(&padded[n * HOP..][..FRAME]);
            let c = inst.play(pitch_scaled(heard.f0_hz), heard.loudness, &mut state);
            worst_f0 = worst_f0.max(((heard.f0_hz - r[0]) / r[0]).abs());
            worst_db = worst_db.max((heard.db - r[2]).abs());
            worst_amp = worst_amp.max(((c.amplitude - r[5]) / r[5].max(1e-3)).abs());
            worst_h = worst_h.max(c.harmonics.iter().zip(&r[6..6 + HARMONICS]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max));
            worst_n = worst_n.max(c.noise.iter().zip(&r[6 + HARMONICS..]).map(|(a, b)| (a - b).abs() / b.max(1e-4)).fold(0.0, f32::max));
            frames += 1;
        }
        let secs = frames as f32 * HOP as f32 / RATE as f32;
        println!("{frames} frames, {:.3} of real time", start.elapsed().as_secs_f32() / secs);
        println!("worst: f0 {worst_f0:.2e} (relative), dB {worst_db:.2e}, amplitude {worst_amp:.2e} (relative), harmonics {worst_h:.2e}, noise {worst_n:.2e} (relative)");
        assert!(worst_f0 < 1e-3 && worst_db < 1e-3 && worst_amp < 1e-2 && worst_h < 1e-3 && worst_n < 1e-2);
    }

    /// `FLEX_DDSP_ZIP`: DDSP-VST-Models.zip; every instrument in it loads.
    #[test]
    #[ignore]
    fn every_instrument_loads() {
        let zip = std::fs::read(std::env::var("FLEX_DDSP_ZIP").unwrap()).unwrap();
        for (id, _) in INSTRUMENTS {
            let inst = Instrument::load(&instrument_file(std::io::Cursor::new(&zip), id).unwrap()).unwrap_or_else(|e| panic!("{id}: {e}"));
            let mut state = vec![0.0; GRU];
            let c = inst.play(pitch_scaled(220.0), 0.6, &mut state);
            println!("{id}: {:.0}–{:.0} Hz, amplitude {:.3}", inst.range.0, inst.range.1, c.amplitude);
        }
    }

    /// How quickly each instrument gets loud from silence, given a steady note.
    #[test]
    #[ignore]
    fn how_quickly_they_start() {
        let zip = std::fs::read(std::env::var("FLEX_DDSP_ZIP").unwrap()).unwrap();
        for (id, _) in INSTRUMENTS {
            let inst = Instrument::load(&instrument_file(std::io::Cursor::new(&zip), id).unwrap()).unwrap();
            let f0 = (inst.range.0 * inst.range.1).sqrt();
            let mut state = vec![0.0; GRU];
            for _ in 0..25 {
                inst.play(pitch_scaled(f0), 0.0, &mut state);
            }
            let loud = inst.power.1 / 80.0 + 1.0;
            let amps: Vec<String> = (0..20).map(|_| format!("{:.2}", inst.play(pitch_scaled(f0), loud, &mut state).amplitude)).collect();
            println!("{id:10} {}", amps.join(" "));
        }
    }
}
