//! Hearing when someone's talking, with Silero VAD (github.com/snakers4/silero-vad, MIT): a
//! small network that says, every 32 ms of sound at 16 kHz, how likely it is that it's speech.
//! It's run here straight from the weights in its ONNX file (`silero_vad_16k_op15.onnx`): a
//! spectrum, four small convolutions and an LSTM cell. (Its graphs have conditionals tract can't
//! run.)

use std::collections::HashMap;

use crate::resample::Down;

/// Where Flex downloads it from on first use (Silero's repository, at the v6.2.3 release), and
/// what to expect.
pub const FILE: &str = "silero_vad_16k_op15.onnx";
pub const URL: &str = "https://raw.githubusercontent.com/snakers4/silero-vad/5cd7945676eb32225748052e2e6a0580e4686a08/src/silero_vad/data/silero_vad_16k_op15.onnx";
pub const BYTES: u64 = 1_289_603;
pub const SHA256: &str = "7ed98ddbad84ccac4cd0aeb3099049280713df825c610a8ed34543318f1b2c49";

/// Sound it's given, and what it hears it at: a third of that.
pub const RATE: u32 = 48_000;
/// How much it hears at a time at 16 kHz, and how much of what came before it's shown with that.
const WINDOW: usize = 512;
const CONTEXT: usize = 64;
/// The spectrum: windows of 256 every 128, so 129 bins over four windows.
const STFT: usize = 256;
const STFT_HOP: usize = 128;
const BINS: usize = STFT / 2 + 1;
const HIDDEN: usize = 128;

/// One of its convolutions over time: weights [out][in][3], padded by one each side.
struct Conv {
    inputs: usize,
    outputs: usize,
    stride: usize,
    weight: Vec<f32>,
    bias: Vec<f32>,
}

impl Conv {
    /// `x` [inputs][len] → [outputs][len / stride], through a ReLU.
    fn run(&self, x: &[f32], len: usize) -> (Vec<f32>, usize) {
        let out_len = len.div_ceil(self.stride);
        let mut y = vec![0.0; self.outputs * out_len];
        for o in 0..self.outputs {
            for t in 0..out_len {
                let mut sum = self.bias[o];
                for i in 0..self.inputs {
                    let w = &self.weight[(o * self.inputs + i) * 3..][..3];
                    for (k, w) in w.iter().enumerate() {
                        let at = (t * self.stride + k) as isize - 1;
                        if (0..len as isize).contains(&at) {
                            sum += w * x[i * len + at as usize];
                        }
                    }
                }
                y[o * out_len + t] = sum.max(0.0);
            }
        }
        (y, out_len)
    }
}

/// The network, loaded once and shared by every stream.
pub struct Model {
    /// The spectrum's basis: [258][256], the real parts' then the imaginary parts'.
    stft: Vec<f32>,
    convs: [Conv; 4],
    /// The LSTM cell's weights, [4 × HIDDEN][HIDDEN] for the input and the last output, and
    /// its biases added together.
    w_ih: Vec<f32>,
    w_hh: Vec<f32>,
    bias: Vec<f32>,
    out_w: Vec<f32>,
    out_b: f32,
}

impl Model {
    /// Loads it from Silero's `silero_vad_16k_op15.onnx`.
    pub fn load(bytes: &[u8]) -> Result<Self, String> {
        let mut t = read_onnx_weights(bytes)?;
        let mut take = |name: &str, shape: &[usize]| match t.remove(name) {
            Some((s, v)) if s == shape => Ok(v),
            Some((s, _)) => Err(format!("{name} is {s:?}, not {shape:?}")),
            None => Err(format!("{name} is missing")),
        };
        let mut conv = |n: usize, inputs: usize, outputs: usize, stride: usize| -> Result<Conv, String> {
            Ok(Conv { inputs, outputs, stride, weight: take(&format!("model.encoder.{}.reparam_conv.weight", n - 1), &[outputs, inputs, 3])?, bias: take(&format!("model.encoder.{}.reparam_conv.bias", n - 1), &[outputs])? })
        };
        let convs = [conv(1, BINS, 128, 1)?, conv(2, 128, 64, 2)?, conv(3, 64, 64, 2)?, conv(4, 64, HIDDEN, 1)?];
        let stft = take("model.stft.forward_basis_buffer", &[2 * BINS, 1, STFT])?;
        let w_ih = take("model.decoder.rnn.weight_ih", &[4 * HIDDEN, HIDDEN])?;
        let w_hh = take("model.decoder.rnn.weight_hh", &[4 * HIDDEN, HIDDEN])?;
        let bias = take("model.decoder.rnn.bias_ih", &[4 * HIDDEN])?.iter().zip(take("model.decoder.rnn.bias_hh", &[4 * HIDDEN])?).map(|(a, b)| a + b).collect();
        let out_w = take("model.decoder.decoder.2.weight", &[1, HIDDEN, 1])?;
        let out_b = take("model.decoder.decoder.2.bias", &[1])?[0];
        Ok(Self { stft, convs, w_ih, w_hh, bias, out_w, out_b })
    }

    /// One window (with its context before it): how likely it's speech, and the LSTM's state
    /// (h then c) carried on.
    fn run(&self, heard: &[f32], h: &mut [f32], c: &mut [f32]) -> f32 {
        // The spectrum, after padding the end by reflecting it.
        let n = heard.len();
        let padded: Vec<f32> = heard.iter().copied().chain((0..CONTEXT).map(|j| heard[n - 2 - j])).collect();
        let frames = (padded.len() - STFT) / STFT_HOP + 1;
        let mut x = vec![0.0; BINS * frames];
        for f in 0..frames {
            let w = &padded[f * STFT_HOP..][..STFT];
            for b in 0..BINS {
                let dot = |row: usize| self.stft[row * STFT..][..STFT].iter().zip(w).map(|(a, b)| a * b).sum::<f32>();
                let (re, im) = (dot(b), dot(BINS + b));
                x[b * frames + f] = (re * re + im * im).sqrt();
            }
        }
        let mut len = frames;
        for conv in &self.convs {
            (x, len) = conv.run(&x, len);
        }
        debug_assert_eq!(len, 1);
        // The LSTM cell: input, forget, cell and output gates.
        let mut gates = self.bias.clone();
        for (g, gate) in gates.iter_mut().enumerate() {
            let wi = &self.w_ih[g * HIDDEN..][..HIDDEN];
            let wh = &self.w_hh[g * HIDDEN..][..HIDDEN];
            *gate += wi.iter().zip(&x).map(|(a, b)| a * b).sum::<f32>() + wh.iter().zip(h.iter()).map(|(a, b)| a * b).sum::<f32>();
        }
        let sigmoid = |v: f32| 1.0 / (1.0 + (-v).exp());
        let mut out = self.out_b;
        for k in 0..HIDDEN {
            let (i, f, g, o) = (sigmoid(gates[k]), sigmoid(gates[HIDDEN + k]), gates[2 * HIDDEN + k].tanh(), sigmoid(gates[3 * HIDDEN + k]));
            c[k] = f * c[k] + i * g;
            h[k] = o * c[k].tanh();
            out += self.out_w[k] * h[k].max(0.0);
        }
        sigmoid(out)
    }
}

/// Weights by name: (shape, values).
type Weights = HashMap<String, (Vec<usize>, Vec<f32>)>;

/// An ONNX file's f32 weights.
fn read_onnx_weights(bytes: &[u8]) -> Result<Weights, String> {
    use tract_onnx::prelude::Framework;
    let proto = tract_onnx::onnx().proto_model_for_read(&mut &bytes[..]).map_err(|e| e.to_string())?;
    let graph = proto.graph.ok_or("the ONNX file has no graph")?;
    let mut out = HashMap::new();
    for t in graph.initializer {
        let shape: Vec<usize> = t.dims.iter().map(|&d| d.max(0) as usize).collect();
        let values: Vec<f32> = if !t.float_data.is_empty() {
            t.float_data.clone()
        } else {
            t.raw_data.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
        };
        // Floats only (the graph's own integer constants are skipped).
        if t.data_type == 1 && values.len() == shape.iter().product::<usize>() {
            out.insert(t.name.clone(), (shape, values));
        }
    }
    Ok(out)
}

/// One stream: give it mono sound at `RATE`, and it says how likely it is someone's talking.
pub struct Detector {
    down: Down,
    /// The sound at 16 kHz, with the context first.
    heard: Vec<f32>,
    h: Vec<f32>,
    c: Vec<f32>,
    speech: f32,
}

impl Default for Detector {
    fn default() -> Self {
        Self { down: Down::default(), heard: vec![0.0; CONTEXT], h: vec![0.0; HIDDEN], c: vec![0.0; HIDDEN], speech: 0.0 }
    }
}

impl Detector {
    /// How likely it is that the last 32 ms or so were speech, 0–1.
    pub fn speech(&self) -> f32 {
        self.speech
    }

    pub fn push(&mut self, model: &Model, sound: &[f32]) {
        for &x in sound {
            let Some(y) = self.down.push(x) else { continue };
            self.heard.push(y);
            if self.heard.len() == CONTEXT + WINDOW {
                self.speech = model.run(&self.heard, &mut self.h, &mut self.c);
                self.heard.drain(..WINDOW);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a clip through: `FLEX_VAD` is silero_vad_16k_op15.onnx, `FLEX_VAD_IN` raw mono f32
    /// at 48 kHz; prints what it hears every 0.1 s.
    #[test]
    #[ignore]
    fn hears_a_clip() {
        let model = Model::load(&std::fs::read(std::env::var("FLEX_VAD").unwrap()).unwrap()).unwrap();
        let bytes = std::fs::read(std::env::var("FLEX_VAD_IN").unwrap()).unwrap();
        let input: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        let mut d = Detector::default();
        let mut probs = Vec::new();
        let start = std::time::Instant::now();
        let mut line = String::new();
        // A window at a time, so each answer's for one window.
        for part in input.as_chunks::<{ WINDOW * crate::resample::FACTOR }>().0 {
            d.push(&model, part);
            probs.push(d.speech());
            if probs.len() % 3 == 0 {
                line.push(b" .:-=+*#%@"[(d.speech() * 9.99) as usize] as char);
            }
        }
        let took = start.elapsed().as_secs_f32();
        println!("{:.4} of real time\n{line}", took / (input.len() as f32 / RATE as f32));
        let path = std::env::var("FLEX_VAD_IN").unwrap() + ".speech";
        std::fs::write(path, probs.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>()).unwrap();
    }
}
