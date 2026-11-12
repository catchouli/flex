//! Flex's networks as hand-written compute shaders run through wgpu: Vulkan, Metal or DX12,
//! with no other runtime. Depth from a single picture is MoGe-2 (`moge`: a DINOv2 ViT-S/14
//! encoder, here, and its own neck and heads), with Flex's own head on its features for what's
//! what; the other modules are the networks for faces, flow, restyling and sound. Each is checked
//! against its original, stage by stage, with reference outputs from its tools/ script.
//!
//! The approach (one storage buffer of activations, a uniform slot per dispatch, the whole
//! picture as one submission) and the core kernels come from Maisy's neurofield
//! (github.com/bombpersons/neurofield), with her permission. Here the weights get a buffer
//! of their own: together with the activations they'd pass wgpu's guaranteed 128 MB.
//!
//! Output is relative inverse depth ("disparity"): bigger is nearer, in no fixed units.

pub mod animegan;
pub mod face;
pub mod models;
pub mod sound;
pub mod moge;
mod torch_file;
pub mod magenta;
mod proto;
mod saved_model;
pub mod flow;
pub mod track;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use anyhow::{Context, Result, bail, ensure};

struct SharedGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: String,
    most_buffer: u64,
}

/// The one GPU device all the networks use, made when the first one wants it and freed with
/// the last.
#[derive(Clone)]
pub struct Gpu(Arc<SharedGpu>);

static SHARED_GPU: Mutex<Weak<SharedGpu>> = Mutex::new(Weak::new());

impl Gpu {
    pub fn shared() -> Result<Self> {
        let mut shared = SHARED_GPU.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(gpu) = shared.upgrade() {
            return Ok(Self(gpu));
        }
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| anyhow::anyhow!("no GPU adapter: {e}"))?;
        let most = adapter.limits();
        let most_buffer = (most.max_storage_buffer_binding_size as u64).min(1 << 30);
        let mut limits = wgpu::Limits::default();
        limits.max_storage_buffer_binding_size = limits.max_storage_buffer_binding_size.max(most_buffer as _);
        limits.max_buffer_size = limits.max_buffer_size.max(most_buffer as _);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("flex-networks"),
            required_limits: limits,
            ..Default::default()
        }))
        .map_err(|e| anyhow::anyhow!("GPU device: {e}"))?;
        let gpu = Arc::new(SharedGpu { device, queue, adapter: adapter.get_info().name, most_buffer });
        *shared = Arc::downgrade(&gpu);
        Ok(Self(gpu))
    }
}

pub const PATCH: usize = 14;
const D: usize = 384;
const HEADS: usize = 6;
const LAYERS: usize = 12;
/// The position embedding's grid (518 / 14).
const POS_GRID: usize = 37;
/// What the softmax kernel has room for.
const MAX_TOKENS: usize = 64 * 32;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];
const LN_EPS: f32 = 1e-6;

const NONE: u32 = u32::MAX;
const GELU: u32 = 1;
const ACCUMULATE: u32 = 2;
const RELU: u32 = 4;
const RELU_IN: u32 = 8;
/// Leaky ReLU (slope 0.1) after the bias, as flow's convolutions have.
const LEAKY: u32 = 64;
/// Edges repeated in a 3x3 convolution (see depth.wgsl).
const EDGE: u32 = 32;
/// Mirrored edges in a 3x3 convolution (see depth.wgsl).
const REFLECT: u32 = 128;
/// Leaky ReLU (slope 0.2) after the bias, or after ACCUMULATE's add (see depth.wgsl).
const LEAKY2: u32 = 1024;
const LEAKY2_LAST: u32 = 2048;
/// Bytes per uniform slot (wgpu's minimum dynamic-offset alignment).
const SLOT: u64 = 256;
/// About how long (ms, on an RX 580) each submission should keep the GPU. Run as one the network
/// holds the GPU for tens of milliseconds and whatever else is drawing (the video!) waits behind
/// it; split up, other work gets in between. But each split costs a little.
const SUBMIT_MS: f64 = 4.0;

/// One dispatch's sizes and offsets (in floats); see depth.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    m: u32,
    k: u32,
    n: u32,
    batch: u32,
    a_off: u32,
    a_bs: u32,
    a_rs: u32,
    a_cs: u32,
    b_off: u32,
    b_bs: u32,
    b_rs: u32,
    b_cs: u32,
    c_off: u32,
    c_bs: u32,
    c_rs: u32,
    c_cs: u32,
    bias_off: u32,
    flags: u32,
    x0: u32,
    x1: u32,
    f0: f32,
    f1: f32,
    f2: f32,
    f3: f32,
    /// A second batch (see depth.wgsl): how far apart its inputs, weights and outputs are.
    a_os: u32,
    b_os: u32,
    c_os: u32,
    pad: u32,
}

#[derive(Clone, Copy, Debug)]
enum Kernel {
    Matmul,
    /// With B in `mem` instead of the weights.
    MatmulMem,
    LayerNorm,
    Softmax,
    Conv3x3,
    /// The same, 16 outputs a thread.
    Conv3x3Wide,
    WinoIn,
    WinoOut,
    Resize,
    Copy,
    // The restyle networks' (animegan.wgsl).
    Conv9,
    Conv7,
    Conv5,
    ConvK3,
    Stats,
    Join,
    Layer,
    AgNorm,
    RowSoftmax,
    Columns,
    Double,
    Weave,
    Pack,
    // The flow network's (flow.wgsl).
    FlowConv,
    Half,
    FlowDouble,
    Map,
    Transpose,
    Channels,
    Correlate,
    Expect,
    Im2col,
    Sum,
    // The face networks' (face.wgsl), which take `face::Params`.
    FaceConv,
    Pool,
    AddUp,
    Gap,
    ConvReduce,
    Conv1Tile,
    SumParts,
}

/// Each `Kernel`'s entry point, and whether its `B_IN_MEM` is set.
const KERNELS: [(&str, bool); 40] = [
    ("matmul", false),
    ("matmul", true),
    ("layer_norm", false),
    ("softmax", false),
    ("conv3x3", false),
    ("conv3x3_16", false),
    ("wino_in", false),
    ("wino_out", false),
    ("resize", false),
    ("copy", false),
    ("ag_conv9", false),
    ("ag_conv7", false),
    ("ag_conv5", false),
    ("ag_conv3", false),
    ("ag_stats", false),
    ("ag_join", false),
    ("ag_layer", false),
    ("ag_norm", false),
    ("ag_softmax", false),
    ("ag_columns", false),
    ("ag_double", false),
    ("ag_weave", false),
    ("ag_pack", false),
    ("flow_conv", false),
    ("flow_half", false),
    ("flow_double", false),
    ("flow_map", false),
    ("flow_transpose", false),
    ("flow_channels", false),
    ("flow_correlate", false),
    ("flow_expect", false),
    ("flow_im2col", false),
    ("flow_sum", false),
    ("conv", false),
    ("pool", false),
    ("add_up", false),
    ("gap", false),
    ("conv_reduce", false),
    ("conv1_tile", false),
    ("sum_parts", false),
];

/// The kernels most networks use: depth.wgsl's, and the restyle networks'.
const DEPTH_WGSL: &str = concat!(include_str!("depth.wgsl"), include_str!("animegan.wgsl"));

// ---- Weights --------------------------------------------------------------------------------

/// A safetensors file's f32 tensors: name → (shape, values).
type Tensors = HashMap<String, (Vec<usize>, Vec<f32>)>;

/// A safetensors file's header, and the data after it.
fn safetensors_header(bytes: &[u8]) -> Result<(serde_json::Value, &[u8])> {
    let len = u64::from_le_bytes(bytes.get(..8).context("not a safetensors file")?.try_into()?);
    let end = usize::try_from(len).ok().and_then(|len| len.checked_add(8)).context("bad safetensors header")?;
    let header = serde_json::from_slice(bytes.get(8..end).context("truncated safetensors header")?)?;
    Ok((header, &bytes[end..]))
}

fn read_safetensors(bytes: &[u8]) -> Result<Tensors> {
    let (header, data) = safetensors_header(bytes)?;
    let mut out = HashMap::new();
    for (name, info) in header.as_object().context("bad safetensors header")? {
        if name == "__metadata__" {
            continue;
        }
        let dtype = info["dtype"].as_str().unwrap_or("");
        let number = |v: &serde_json::Value| v.as_u64().and_then(|v| usize::try_from(v).ok()).with_context(|| format!("{name}: bad header"));
        let shape: Vec<usize> = info["shape"].as_array().with_context(|| format!("{name}: no shape"))?.iter().map(number).collect::<Result<_>>()?;
        let (start, end) = match info["data_offsets"].as_array().map(Vec::as_slice) {
            Some([start, end]) => (number(start)?, number(end)?),
            _ => bail!("{name}: no offsets"),
        };
        let raw = data.get(start..end).with_context(|| format!("{name}: data out of range"))?;
        // Bytes each, and the values.
        let (size, values): (usize, Vec<f32>) = match dtype {
            "F32" => (4, raw.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()),
            "U8" => (1, raw.iter().map(|b| f32::from(*b)).collect()),
            "F16" => (2, raw.as_chunks::<2>().0.iter().map(|b| face::from_f16(u16::from_le_bytes(*b))).collect()),
            other => bail!("{name}: {other} tensors aren't supported"),
        };
        let count = shape.iter().try_fold(1usize, |n, d| n.checked_mul(*d));
        ensure!(raw.len() == size * values.len() && count == Some(values.len()), "{name}: {} bytes for {dtype} {shape:?}", raw.len());
        out.insert(name.clone(), (shape, values));
    }
    Ok(out)
}

/// [rows][cols] as [cols][rows]: a PyTorch weight [out][in] as the matrix multiply's [in][out].
fn transpose(v: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    let mut t = vec![0.0; rows * cols];
    for r in 0..rows {
        for c in 0..cols {
            t[c * rows + r] = v[r * cols + c];
        }
    }
    t
}

/// Floats in `mem`, handed out in order.
#[derive(Default)]
struct Mem {
    end: u32,
}

impl Mem {
    fn take(&mut self, len: usize) -> u32 {
        let at = self.end;
        self.end += len as u32;
        at
    }
}

/// Weights as laid out for the kernels, built up in one buffer.
#[derive(Default)]
struct Upload {
    data: Vec<f32>,
}

impl Upload {
    fn push(&mut self, v: &[f32]) -> u32 {
        let at = self.data.len() as u32;
        self.data.extend_from_slice(v);
        at
    }
}

struct Weights {
    t: Tensors,
}

impl Weights {
    fn get(&self, name: &str) -> Result<&(Vec<usize>, Vec<f32>)> {
        self.t.get(name).with_context(|| format!("weights: no {name}"))
    }

    /// A PyTorch linear or 1x1 convolution weight [out, in, ...] as [in][out] (for x W),
    /// each output column scaled by `scale` (layer scale folded in).
    fn transposed(&self, name: &str, scale: Option<&[f32]>) -> Result<(usize, usize, Vec<f32>)> {
        let (shape, v) = self.get(name)?;
        let (out, inp) = (shape[0], shape[1..].iter().product::<usize>());
        let mut t = transpose(v, out, inp);
        if let Some(scale) = scale {
            for row in t.chunks_mut(out) {
                row.iter_mut().zip(scale).for_each(|(v, s)| *v *= s);
            }
        }
        Ok((inp, out, t))
    }

    fn vec(&self, name: &str) -> Result<Vec<f32>> {
        Ok(self.get(name)?.1.clone())
    }
}

/// A linear layer in `w`: weight [inp][out] and bias.
#[derive(Clone, Copy)]
struct Linear {
    w: u32,
    b: u32,
    inp: usize,
    out: usize,
}

#[derive(Clone, Copy)]
struct Norm {
    g: u32,
    b: u32,
}

/// A convolution's weights (PyTorch layout) and bias (or NONE) in `w`.
#[derive(Clone, Copy)]
struct Conv {
    w: u32,
    b: u32,
}

/// A 3x3 convolution's weights as Winograd F(2x2, 3x3) wants them: U [16][cin][cout] (each
/// 3x3 g becomes G g G^T), and the bias (or NONE).
#[derive(Clone, Copy)]
struct Wino {
    u: u32,
    b: u32,
    cin: usize,
    cout: usize,
}

impl Wino {
    /// Floats of scratch it needs on an h x w picture.
    fn scratch(cin: usize, cout: usize, (h, w): (usize, usize)) -> usize {
        16 * (cin + cout) * h.div_ceil(2) * w.div_ceil(2)
    }
}

/// `weights` [cout][cin][3][3] for `conv3x3_wino`.
fn wino(up: &mut Upload, weights: &[f32], bias: Option<&[f32]>, cout: usize, cin: usize) -> Wino {
    Wino { u: up.push(&wino_u(weights, cout, cin)), b: bias.map_or(NONE, |b| up.push(b)), cin, cout }
}

/// `weights` [cout][cin][3][3] transformed for Winograd's F(2x2, 3x3): [16][cin][cout].
fn wino_u(weights: &[f32], cout: usize, cin: usize) -> Vec<f32> {
    const G: [[f32; 3]; 4] = [[1.0, 0.0, 0.0], [0.5, 0.5, 0.5], [0.5, -0.5, 0.5], [0.0, 0.0, 1.0]];
    let mut u = vec![0.0f32; 16 * cin * cout];
    for co in 0..cout {
        for ci in 0..cin {
            let g = &weights[(co * cin + ci) * 9..][..9];
            for i in 0..4 {
                for j in 0..4 {
                    let mut v = 0.0;
                    for a in 0..3 {
                        for b in 0..3 {
                            v += G[i][a] * g[a * 3 + b] * G[j][b];
                        }
                    }
                    u[((i * 4 + j) * cin + ci) * cout + co] = v;
                }
            }
        }
    }
    u
}

/// A linear layer (or 1x1 convolution), its bias NONE if it has none.
fn linear(up: &mut Upload, wt: &Weights, name: &str, scale: Option<&[f32]>) -> Result<Linear> {
    let (inp, out, t) = wt.transposed(&format!("{name}.weight"), scale)?;
    let w = up.push(&t);
    let b = match wt.t.get(&format!("{name}.bias")) {
        Some((_, b)) => {
            let mut b = b.clone();
            if let Some(s) = scale {
                b.iter_mut().zip(s).for_each(|(b, s)| *b *= s);
            }
            up.push(&b)
        }
        None => NONE,
    };
    Ok(Linear { w, b, inp, out })
}

fn norm(up: &mut Upload, wt: &Weights, name: &str) -> Result<Norm> {
    Ok(Norm { g: up.push(&wt.vec(&format!("{name}.weight"))?), b: up.push(&wt.vec(&format!("{name}.bias"))?) })
}

fn conv(up: &mut Upload, wt: &Weights, name: &str) -> Result<Conv> {
    let w = up.push(&wt.vec(&format!("{name}.weight"))?);
    let b = match wt.t.get(&format!("{name}.bias")) {
        Some((_, b)) => up.push(b),
        None => NONE,
    };
    Ok(Conv { w, b })
}

/// PyTorch's bicubic resize (align_corners = false, A = -0.75) of a [c][ih][iw] grid to
/// [c][oh][ow].
#[cfg(test)]
fn bicubic(src: &[f32], c: usize, (ih, iw): (usize, usize), (oh, ow): (usize, usize)) -> Vec<f32> {
    bicubic_scaled(src, c, (ih, iw), (oh, ow), (ih as f64 / oh as f64, iw as f64 / ow as f64))
}

/// The same with each output pixel `ratio` (y, x) input pixels apart: what PyTorch does when
/// given a scale factor rather than a size, and how DINOv2 stretches its position embedding to
/// other picture sizes.
fn bicubic_scaled(src: &[f32], c: usize, (ih, iw): (usize, usize), (oh, ow): (usize, usize), (ry, rx): (f64, f64)) -> Vec<f32> {
    const A: f64 = -0.75;
    let near = |x: f64| ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0;
    let far = |x: f64| ((A * x - 5.0 * A) * x + 8.0 * A) * x - 4.0 * A;
    let taps = |dst: usize, inn: usize, ratio: f64| {
        let real = ratio * (dst as f64 + 0.5) - 0.5;
        let i = real.floor();
        let t = real - i;
        let w = [far(t + 1.0), near(t), near(1.0 - t), far(2.0 - t)];
        let idx = [-1, 0, 1, 2].map(|d| (i as i64 + d).clamp(0, inn as i64 - 1) as usize);
        (idx, w)
    };
    let mut out = vec![0.0; c * oh * ow];
    for y in 0..oh {
        let (iy, wy) = taps(y, ih, ry);
        for x in 0..ow {
            let (ix, wx) = taps(x, iw, rx);
            for ch in 0..c {
                let plane = &src[ch * ih * iw..];
                let mut acc = 0.0f64;
                for (a, &yy) in iy.iter().enumerate() {
                    let mut row = 0.0f64;
                    for (b, &xx) in ix.iter().enumerate() {
                        row += f64::from(plane[yy * iw + xx]) * wx[b];
                    }
                    acc += row * wy[a];
                }
                out[ch * oh * ow + y * ow + x] = acc as f32;
            }
        }
    }
    out
}

/// A transformer block, its layer scales folded into `out` and `fc2`.
struct Block {
    n1: Norm,
    /// [D][3D]: query, key and value side by side.
    qkv: Linear,
    out: Linear,
    n2: Norm,
    fc1: Linear,
    fc2: Linear,
}

/// DINOv2's ViT-S/14, the encoder Depth Anything V2 and MoGe-2 share.
struct Encoder {
    /// The patch embedding.
    patch: Linear,
    /// The tokens it starts from, in `w`: the class token's own and its position, then each
    /// patch's position.
    start: u32,
    blocks: Vec<Block>,
    /// The norm each tapped block's output goes through.
    norm: Norm,
}

/// Where the encoder works in `mem`.
#[derive(Debug, Default, Clone)]
struct EncoderMem {
    patches: u32,
    x: u32,
    xn: u32,
    qkv: u32,
    scores: u32,
    attn: u32,
    hidden: u32,
}

impl Encoder {
    /// For a grid of (gw, gh) patches: the weights named `patch` (the patch embedding), `pos`
    /// (the position embedding, stretched as DINOv2 does it, by a scale factor `pad` patches
    /// bigger than the grid), `cls` (the class token) and `norm`, and each block from `block`.
    fn load(
        up: &mut Upload,
        wt: &Weights,
        (gw, gh): (usize, usize),
        [patch, pos, cls, norm_name]: [&str; 4],
        pad: f64,
        mut block: impl FnMut(&mut Upload, usize) -> Result<Block>,
    ) -> Result<Self> {
        let n = gw * gh;
        let (pe_in, pe_out, pe_w) = wt.transposed(&format!("{patch}.weight"), None)?;
        let patch = Linear { w: up.push(&pe_w), b: up.push(&wt.vec(&format!("{patch}.bias"))?), inp: pe_in, out: pe_out };
        let pos = &wt.get(pos)?.1;
        let cls = wt.vec(cls)?;
        if pos.len() != (POS_GRID * POS_GRID + 1) * D {
            bail!("unexpected position embedding size");
        }
        // [37*37][D] → [D][37][37] → stretched → [N][D], with the class token's own added to
        // its row.
        let grid_pos = transpose(&pos[D..], POS_GRID * POS_GRID, D);
        let ratio = (POS_GRID as f64 / (gh as f64 + pad), POS_GRID as f64 / (gw as f64 + pad));
        let stretched = bicubic_scaled(&grid_pos, D, (POS_GRID, POS_GRID), (gh, gw), ratio);
        let mut start = vec![0.0; (n + 1) * D];
        for c in 0..D {
            start[c] = cls[c] + pos[c];
            for i in 0..n {
                start[(1 + i) * D + c] = stretched[c * n + i];
            }
        }
        let start = up.push(&start);
        let blocks = (0..LAYERS).map(|i| block(up, i)).collect::<Result<_>>()?;
        Ok(Self { patch, start, blocks, norm: norm(up, wt, norm_name)? })
    }

    fn mem(&self, m: &mut Mem, n: usize) -> EncoderMem {
        let tokens = n + 1;
        EncoderMem {
            patches: m.take(n * self.patch.inp),
            x: m.take(tokens * D),
            xn: m.take(tokens * D),
            qkv: m.take(tokens * 3 * D),
            scores: m.take(HEADS * tokens * tokens),
            attn: m.take(tokens * D),
            hidden: m.take(tokens * 4 * D),
        }
    }

    /// The forward pass over `n` patches, each of the `taps` blocks' output normed to `out`.
    fn ops(&self, ops: &mut Ops, s: &EncoderMem, n: usize, taps: &[usize], out: &[u32]) {
        let tokens = n + 1;
        ops.copy(self.start, tokens * D, s.x, 0);
        ops.linear((s.patches, self.patch.inp), n, self.patch, (s.x + D as u32, D), ACCUMULATE);
        let mut tap = 0;
        for (i, b) in self.blocks.iter().enumerate() {
            ops.norm(s.x, tokens, b.n1, s.xn);
            ops.linear((s.xn, D), tokens, b.qkv, (s.qkv, 3 * D), 0);
            ops.attention(s.qkv, s.scores, s.attn, tokens);
            ops.linear((s.attn, D), tokens, b.out, (s.x, D), ACCUMULATE);
            ops.norm(s.x, tokens, b.n2, s.xn);
            ops.linear((s.xn, D), tokens, b.fc1, (s.hidden, 4 * D), GELU);
            ops.linear((s.hidden, 4 * D), tokens, b.fc2, (s.x, D), ACCUMULATE);
            if taps.contains(&i) {
                ops.norm(s.x, tokens, self.norm, out[tap]);
                tap += 1;
            }
        }
    }
}

/// The layers of the network in `tools/moge/segment`, which finds people, walls, sky and more
/// from two of the encoder's taps and 64 channels of features at 8 times the patch grid.
struct MaskHead {
    token9: Linear,
    token12: Linear,
    context: Conv,
    mid: Conv,
    fused: Linear,
    fine1: Conv,
    fine2: Conv,
    out: Linear,
    /// fine1's and fine2's channels, and the classes it tells apart.
    width: usize,
    classes: usize,
}

/// Where the mask head works in `mem`, and its logits [classes][8 gh][8 gw].
#[derive(Debug, Default, Clone)]
struct MaskMem {
    token_sum: u32,
    context: u32,
    context_up: u32,
    mid: u32,
    mid_up: u32,
    fine1: u32,
    fine2: u32,
    logits: u32,
}

impl MaskHead {
    /// From its safetensors.
    fn load(up: &mut Upload, bytes: &[u8]) -> Result<Self> {
        let wt = Weights { t: read_safetensors(bytes)? };
        Ok(Self {
            token9: linear(up, &wt, "token9", None)?,
            token12: linear(up, &wt, "token12", None)?,
            context: conv(up, &wt, "context")?,
            mid: conv(up, &wt, "mid")?,
            fused: linear(up, &wt, "fused", None)?,
            fine1: conv(up, &wt, "fine1")?,
            fine2: conv(up, &wt, "fine2")?,
            out: linear(up, &wt, "out", None)?,
            width: wt.get("fine1.weight")?.0[0],
            classes: wt.get("out.weight")?.0[0],
        })
    }

    fn mem(&self, m: &mut Mem, (gh, gw): (usize, usize)) -> MaskMem {
        let (n, near, fine) = (gh * gw, 16 * gh * gw, 64 * gh * gw);
        MaskMem {
            token_sum: m.take(128 * n),
            context: m.take(128 * n),
            context_up: m.take(128 * near),
            mid: m.take(64 * near),
            mid_up: m.take(64 * fine),
            fine1: m.take(self.width * fine),
            fine2: m.take(self.width * fine),
            logits: m.take(self.classes * fine),
        }
    }

    /// From the two taps' tokens ([tokens][D] each, the class token first) and the features at
    /// `features`, on a grid of (gh, gw) patches.
    fn ops(&self, ops: &mut Ops, s: &MaskMem, (gh, gw): (usize, usize), taps: [u32; 2], features: u32) {
        let n = gh * gw;
        let (near, fine) = ((gh * 4, gw * 4), (gh * 8, gw * 8));
        ops.tokens_to_image(taps[0] + D as u32, n, self.token9, s.token_sum, 0);
        ops.tokens_to_image(taps[1] + D as u32, n, self.token12, s.token_sum, ACCUMULATE);
        ops.conv3x3(s.token_sum, 128, (gh, gw), self.context, 128, s.context, 1, RELU_IN | RELU);
        ops.resize(s.context, 128, (gh, gw), near, s.context_up);
        ops.conv3x3(s.context_up, 128, near, self.mid, 64, s.mid, 1, RELU);
        ops.resize(s.mid, 64, near, fine, s.mid_up);
        ops.conv1x1(features, fine.0 * fine.1, self.fused, s.mid_up, ACCUMULATE);
        ops.conv3x3(s.mid_up, 64, fine, self.fine1, self.width, s.fine1, 1, RELU_IN | RELU);
        ops.conv3x3(s.fine1, self.width, fine, self.fine2, self.width, s.fine2, 1, RELU);
        ops.conv1x1(s.fine2, fine.0 * fine.1, self.out, s.logits, 0);
    }
}

// ---- The plan -------------------------------------------------------------------------------

/// A plan's dispatches: each one's kernel, parameters and workgroups.
struct Ops<P = Params> {
    list: Vec<(Kernel, P, [u32; 3])>,
}

impl<P> Default for Ops<P> {
    fn default() -> Self {
        Self { list: Vec::new() }
    }
}

fn groups(n: usize, per: usize) -> u32 {
    n.div_ceil(per) as u32
}

impl<P> Ops<P> {
    fn push(&mut self, kernel: Kernel, p: P, g: [u32; 3]) {
        self.list.push((kernel, p, g));
    }
}

impl Ops {
    /// C [rows][l.out] (=, or += with ACCUMULATE) act(A l.w + l.b), A [rows][l.inp] in mem.
    fn linear(&mut self, a: (u32, usize), rows: usize, l: Linear, c: (u32, usize), flags: u32) {
        let p = Params {
            m: rows as u32,
            k: l.inp as u32,
            n: l.out as u32,
            batch: 1,
            a_off: a.0,
            a_rs: a.1 as u32,
            a_cs: 1,
            b_off: l.w,
            b_rs: l.out as u32,
            b_cs: 1,
            c_off: c.0,
            c_rs: c.1 as u32,
            c_cs: 1,
            bias_off: l.b,
            flags,
            ..Default::default()
        };
        self.push(Kernel::Matmul, p, [groups(l.out, 32), groups(rows, 32), 1]);
    }

    /// Tokens [n][l.inp] at `a` through a 1x1 convolution, to channels-first [l.out][n] at `c`.
    fn tokens_to_image(&mut self, a: u32, n: usize, l: Linear, c: u32, flags: u32) {
        let p = Params {
            m: n as u32,
            k: l.inp as u32,
            n: l.out as u32,
            batch: 1,
            a_off: a,
            a_rs: l.inp as u32,
            a_cs: 1,
            b_off: l.w,
            b_rs: l.out as u32,
            b_cs: 1,
            c_off: c,
            c_rs: 1,
            c_cs: n as u32,
            bias_off: l.b,
            flags,
            ..Default::default()
        };
        self.push(Kernel::Matmul, p, [groups(l.out, 32), groups(n, 32), 1]);
    }

    /// A 1x1 convolution over channels-first [l.inp][pixels] at `a` to [l.out][pixels] at `c`.
    fn conv1x1(&mut self, a: u32, pixels: usize, l: Linear, c: u32, flags: u32) {
        let p = Params {
            m: pixels as u32,
            k: l.inp as u32,
            n: l.out as u32,
            batch: 1,
            a_off: a,
            a_rs: 1,
            a_cs: pixels as u32,
            b_off: l.w,
            b_rs: l.out as u32,
            b_cs: 1,
            c_off: c,
            c_rs: 1,
            c_cs: pixels as u32,
            bias_off: l.b,
            flags,
            ..Default::default()
        };
        self.push(Kernel::Matmul, p, [groups(l.out, 32), groups(pixels, 32), 1]);
    }

    /// The encoder's layer norm of [rows][D].
    fn norm(&mut self, a: u32, rows: usize, n: Norm, c: u32) {
        self.layer_norm(a, (rows, D), n, c, LN_EPS);
    }

    fn layer_norm(&mut self, a: u32, (rows, width): (usize, usize), n: Norm, c: u32, eps: f32) {
        let p = Params {
            m: rows as u32,
            n: width as u32,
            a_off: a,
            a_rs: width as u32,
            b_off: n.g,
            bias_off: n.b,
            c_off: c,
            c_rs: width as u32,
            f0: eps,
            ..Default::default()
        };
        self.push(Kernel::LayerNorm, p, [rows as u32, 1, 1]);
    }

    /// Self-attention over `rows` tokens: q, k and v side by side in `qkv` [rows][3D], the
    /// heads' outputs side by side in `attn` [rows][D], through `scores_at`.
    fn attention(&mut self, qkv: u32, scores_at: u32, attn: u32, rows: usize) {
        let dh = D / HEADS;
        let rs = 3 * D;
        let scores = Params {
            m: rows as u32,
            k: dh as u32,
            n: rows as u32,
            batch: HEADS as u32,
            a_off: qkv,
            a_bs: dh as u32,
            a_rs: rs as u32,
            a_cs: 1,
            b_off: qkv + D as u32,
            b_bs: dh as u32,
            b_rs: 1,
            b_cs: rs as u32,
            c_off: scores_at,
            c_bs: (rows * rows) as u32,
            c_rs: rows as u32,
            c_cs: 1,
            bias_off: NONE,
            ..Default::default()
        };
        self.push(Kernel::MatmulMem, scores, [groups(rows, 32), groups(rows, 32), HEADS as u32]);
        let soft = Params { m: rows as u32, n: rows as u32, batch: HEADS as u32, a_off: scores_at, f0: 1.0 / (dh as f32).sqrt(), ..Default::default() };
        self.push(Kernel::Softmax, soft, [rows as u32, HEADS as u32, 1]);
        let mix = Params {
            m: rows as u32,
            k: rows as u32,
            n: dh as u32,
            batch: HEADS as u32,
            a_off: scores_at,
            a_bs: (rows * rows) as u32,
            a_rs: rows as u32,
            a_cs: 1,
            b_off: qkv + 2 * D as u32,
            b_bs: dh as u32,
            b_rs: rs as u32,
            b_cs: 1,
            c_off: attn,
            c_bs: dh as u32,
            c_rs: D as u32,
            c_cs: 1,
            bias_off: NONE,
            ..Default::default()
        };
        self.push(Kernel::MatmulMem, mix, [groups(dh, 32), groups(rows, 32), HEADS as u32]);
    }

    /// 3x3 convolution (padding 1) of [cin][h][w] at `a` to [cout][h'][w'] at `c`.
    #[allow(clippy::too_many_arguments)]
    fn conv3x3(&mut self, a: u32, cin: usize, (h, w): (usize, usize), cv: Conv, cout: usize, c: u32, stride: usize, flags: u32) {
        let (oh, ow) = ((h - 1) / stride + 1, (w - 1) / stride + 1);
        let p = Params {
            m: h as u32,
            k: cin as u32,
            n: w as u32,
            a_off: a,
            b_off: cv.w,
            bias_off: cv.b,
            c_off: c,
            x0: cout as u32,
            x1: stride as u32,
            flags,
            ..Default::default()
        };
        if cout >= 32 {
            self.push(Kernel::Conv3x3Wide, p, [groups(ow, 32), groups(oh, 16), groups(cout, 16)]);
        } else {
            self.push(Kernel::Conv3x3, p, [groups(ow, 16), groups(oh, 16), groups(cout, 8)]);
        }
    }

    /// A 3x3 convolution (stride 1, padding 1) by Winograd: [cv.cin][h][w] at `a` to
    /// [cv.cout][h][w] at `c`, through `scratch` (a `Wino::scratch` of the biggest). Flags as
    /// for `conv3x3`.
    fn conv3x3_wino(&mut self, a: u32, (h, w): (usize, usize), cv: Wino, c: u32, scratch: u32, flags: u32) {
        let tiles = h.div_ceil(2) * w.div_ceil(2);
        let v = scratch;
        let m = scratch + (16 * cv.cin * tiles) as u32;
        let input = Params { m: h as u32, k: cv.cin as u32, n: w as u32, a_off: a, c_off: v, flags: flags & (EDGE | RELU_IN | REFLECT), ..Default::default() };
        self.push(Kernel::WinoIn, input, [groups(tiles, 64), cv.cin as u32, 1]);
        // For each of the 16: M^T [tiles][cout] = V^T [tiles][cin] U [cin][cout].
        let mul = Params {
            m: tiles as u32,
            k: cv.cin as u32,
            n: cv.cout as u32,
            batch: 16,
            a_off: v,
            a_bs: (cv.cin * tiles) as u32,
            a_rs: 1,
            a_cs: tiles as u32,
            b_off: cv.u,
            b_bs: (cv.cin * cv.cout) as u32,
            b_rs: cv.cout as u32,
            b_cs: 1,
            c_off: m,
            c_bs: (cv.cout * tiles) as u32,
            c_rs: 1,
            c_cs: tiles as u32,
            bias_off: NONE,
            ..Default::default()
        };
        self.push(Kernel::Matmul, mul, [groups(cv.cout, 32), groups(tiles, 32), 16]);
        let output = Params { m: h as u32, n: w as u32, x0: cv.cout as u32, a_off: m, c_off: c, bias_off: cv.b, flags: flags & (RELU | GELU | LEAKY | ACCUMULATE | LEAKY2 | LEAKY2_LAST), ..Default::default() };
        self.push(Kernel::WinoOut, output, [groups(tiles, 64), cv.cout as u32, 1]);
    }

    fn resize(&mut self, a: u32, ch: usize, (h, w): (usize, usize), (oh, ow): (usize, usize), c: u32) {
        let p = Params { m: h as u32, k: ch as u32, n: w as u32, a_off: a, c_off: c, x0: oh as u32, x1: ow as u32, ..Default::default() };
        self.push(Kernel::Resize, p, [groups(ow, 16), groups(oh, 16), ch as u32]);
    }

    /// A transposed convolution with kernel = stride = 2 as four matrix multiplies, one for each
    /// place in the 2x2 it makes: [l.inp][h][w] at `a` to [l.out][2h][2w] at `c`, `taps` in
    /// (dy, dx) order. Flags as for `linear`.
    fn conv_transpose2(&mut self, a: u32, (h, w): (usize, usize), taps: [Linear; 4], c: u32, flags: u32) {
        for (t, l) in taps.into_iter().enumerate() {
            let (dy, dx) = (t / 2, t % 2);
            let p = Params {
                m: w as u32,
                k: l.inp as u32,
                n: l.out as u32,
                batch: h as u32,
                a_off: a,
                a_bs: w as u32,
                a_rs: 1,
                a_cs: (h * w) as u32,
                b_off: l.w,
                b_rs: l.out as u32,
                b_cs: 1,
                c_off: c + (dy * 2 * w + dx) as u32,
                c_bs: (4 * w) as u32,
                c_rs: 2,
                c_cs: (4 * h * w) as u32,
                bias_off: l.b,
                flags,
                ..Default::default()
            };
            self.push(Kernel::Matmul, p, [groups(l.out, 32), groups(w, 32), h as u32]);
        }
    }

    /// Makes the last `ops` dispatches run `n` times over, the i-th time with its input, weights
    /// (and bias) and output `i` times these strides further on. For the matrix multiply, 3x3
    /// convolutions and Winograd's steps.
    fn repeat_last(&mut self, ops: usize, n: u32, strides: &[(u32, u32, u32)]) {
        let len = self.list.len();
        for ((_, p, g), (a, b, c)) in self.list[len - ops..].iter_mut().zip(strides) {
            (p.a_os, p.b_os, p.c_os) = (*a, *b, *c);
            g[2] *= n;
        }
    }

    /// C = A (A in `w`), or with ACCUMULATE C += A (A in `mem`).
    fn copy(&mut self, a: u32, len: usize, c: u32, flags: u32) {
        let p = Params { m: len as u32, a_off: a, c_off: c, flags, ..Default::default() };
        let blocks = len.div_ceil(64);
        self.push(Kernel::Copy, p, [blocks.min(65535) as u32, blocks.div_ceil(65535) as u32, 1]);
    }
}

/// A shader's kernels made into pipelines (just those asked for), and the layout of what
/// they're given: a uniform slot of parameters `P` (at a dynamic offset), `mem`, and the
/// weights. Networks with the same kernels can share one.
#[derive(Clone)]
struct Kernels {
    gpu: Gpu,
    layout: wgpu::BindGroupLayout,
    /// Each `Kernel`'s, if made.
    pipelines: Vec<Option<wgpu::ComputePipeline>>,
}

impl Kernels {
    fn new<P>(label: &str, code: &str, used: impl IntoIterator<Item = Kernel>) -> Result<Self> {
        let gpu = Gpu::shared()?;
        let device = &gpu.0.device;
        let mut wanted = [false; KERNELS.len()];
        for kernel in used {
            wanted[kernel as usize] = true;
        }
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty, count: None };
        let storage = |read_only| wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &[
                entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<P>() as u64) }),
                entry(1, storage(false)),
                entry(2, storage(true)),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let source = wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(code.into()) };
        // SAFETY: these are our own shaders, and every offset and size they use comes from a
        // plan, which fits its `mem` and weights by construction. The checks wgpu would add go
        // on every array access, and cost 15% of the whole network.
        let module = unsafe { device.create_shader_module_trusted(source, wgpu::ShaderRuntimeChecks::unchecked()) };
        let pipelines = KERNELS
            .iter()
            .zip(wanted)
            .map(|((entry, b_in_mem), wanted)| {
                wanted.then(|| {
                    let constants: &[(&str, f64)] = if *b_in_mem { &[("B_IN_MEM", 1.0)] } else { &[] };
                    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                        label: Some(entry),
                        layout: Some(&pipeline_layout),
                        module: &module,
                        entry_point: Some(entry),
                        compilation_options: wgpu::PipelineCompilationOptions { constants, ..Default::default() },
                        cache: None,
                    })
                })
            })
            .collect();
        Ok(Self { gpu, layout, pipelines })
    }

    fn pipeline(&self, kernel: Kernel) -> &wgpu::ComputePipeline {
        self.pipelines[kernel as usize].as_ref().expect("a kernel the plan uses")
    }
}

/// A network's plan on the GPU: its weights, one buffer for everything else, and the
/// dispatches that run it, each with its parameters (`P`) in a uniform slot of its own.
struct Runner<P = Params> {
    device: wgpu::Device,
    queue: wgpu::Queue,
    kernels: Kernels,
    mem: wgpu::Buffer,
    bind: wgpu::BindGroup,
    readback: Readback,
    ops: Ops<P>,
    /// The dispatches each submission `run` makes.
    parts: Vec<std::ops::Range<usize>>,
    adapter: String,
}

/// Roughly how long a dispatch takes on an RX 580 (ms): what splitting into submissions goes by.
fn cost(kernel: Kernel, p: &Params) -> f64 {
    let (m, k, n) = (f64::from(p.m), f64::from(p.k), f64::from(p.n));
    let work = match kernel {
        Kernel::Matmul | Kernel::MatmulMem => m * k * n * f64::from(p.batch.max(1)),
        Kernel::Conv3x3 | Kernel::Conv3x3Wide => m * n * k * f64::from(p.x0) * 9.0 / f64::from(p.x1.max(1) * p.x1.max(1)),
        Kernel::Conv9 => m * n * k * f64::from(p.x0.next_multiple_of(8)) * 81.0,
        Kernel::Conv7 => m * n * k * f64::from(p.x0.next_multiple_of(8)) * 49.0,
        Kernel::Conv5 => m * n * k * f64::from(p.x0.next_multiple_of(8)) * 25.0,
        Kernel::ConvK3 => m * n * k * f64::from(p.x0.next_multiple_of(8)) * 9.0,
        _ => 0.0,
    };
    0.03 + work / 0.8e9
}

impl Runner {
    /// A plan of depth.wgsl's and animegan.wgsl's kernels, `mem_len` floats of activations;
    /// `what` names the network in errors.
    fn new(up: &Upload, ops: Ops, mem_len: u32, what: &str) -> Result<Self> {
        let kernels = Kernels::new::<Params>("depth.wgsl", DEPTH_WGSL, ops.list.iter().map(|o| o.0))?;
        let mut parts = Vec::new();
        let (mut start, mut ms) = (0, 0.0);
        for (i, (kernel, p, _)) in ops.list.iter().enumerate() {
            ms += cost(*kernel, p);
            if ms >= SUBMIT_MS || i + 1 == ops.list.len() {
                parts.push(start..i + 1);
                (start, ms) = (i + 1, 0.0);
            }
        }
        Self::with(kernels, &up.data, ops, parts, mem_len, what)
    }
}

impl<P: bytemuck::Pod> Runner<P> {
    /// `ops` of `kernels`, with these weights and `mem_len` floats of activations, `run` in
    /// `parts`; `what` names the network in errors.
    fn with(kernels: Kernels, weights: &[f32], ops: Ops<P>, parts: Vec<std::ops::Range<usize>>, mem_len: u32, what: &str) -> Result<Self> {
        let gpu = &kernels.gpu.0;
        let (device, queue) = (gpu.device.clone(), gpu.queue.clone());
        let w_bytes = weights.len() as u64 * 4;
        let mem_bytes = u64::from(mem_len) * 4;
        let needed = w_bytes.max(mem_bytes);
        if needed > gpu.most_buffer {
            bail!("the GPU allows {} MB buffers; {what} needs {} MB", gpu.most_buffer >> 20, needed >> 20);
        }
        let storage = |label, size: u64, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
        let w = storage("weights", w_bytes, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST);
        queue.write_buffer(&w, 0, bytemuck::cast_slice(weights));
        let mem = storage("mem", mem_bytes, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC);
        // The parameters never change: write them once.
        let size = std::mem::size_of::<P>();
        let mut slots = vec![0u8; ops.list.len().max(1) * SLOT as usize];
        for (i, (_, p, _)) in ops.list.iter().enumerate() {
            slots[i * SLOT as usize..][..size].copy_from_slice(bytemuck::bytes_of(p));
        }
        let params = storage("params", slots.len() as u64, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        queue.write_buffer(&params, 0, &slots);
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(what),
            layout: &kernels.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &params, offset: 0, size: wgpu::BufferSize::new(size as u64) }) },
                wgpu::BindGroupEntry { binding: 1, resource: mem.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: w.as_entire_binding() },
            ],
        });
        let adapter = gpu.adapter.clone();
        Ok(Self { device, queue, kernels, mem, bind, readback: Readback::default(), ops, parts, adapter })
    }

    /// Puts `data` in `mem` at `at`.
    fn write(&self, at: u32, data: &[f32]) {
        self.queue.write_buffer(&self.mem, u64::from(at) * 4, bytemuck::cast_slice(data));
    }

    /// Dispatch `i`, `times` times over.
    fn dispatch(&self, pass: &mut wgpu::ComputePass, i: usize, times: usize) {
        let (kernel, _, g) = &self.ops.list[i];
        if g.contains(&0) {
            return;
        }
        pass.set_pipeline(self.kernels.pipeline(*kernel));
        pass.set_bind_group(0, &self.bind, &[(i as u64 * SLOT) as u32]);
        for _ in 0..times {
            pass.dispatch_workgroups(g[0], g[1], g[2]);
        }
    }

    /// The dispatches `record` makes, as one submission's work.
    fn encode(&self, record: impl FnOnce(&mut wgpu::ComputePass)) -> wgpu::CommandBuffer {
        let mut enc = self.device.create_command_encoder(&Default::default());
        record(&mut enc.begin_compute_pass(&Default::default()));
        enc.finish()
    }

    /// Submits dispatches `ops`, without waiting for them.
    fn submit(&self, ops: std::ops::Range<usize>) -> wgpu::SubmissionIndex {
        self.queue.submit([self.encode(|pass| ops.for_each(|i| self.dispatch(pass, i, 1)))])
    }

    /// Runs every dispatch, a part at a time with the next part queued behind it (so the GPU
    /// never waits on us, and other work still gets in between).
    fn run(&self) -> Result<()> {
        let mut queued: Option<wgpu::SubmissionIndex> = None;
        for part in &self.parts {
            let this = self.submit(part.clone());
            if let Some(before) = queued.replace(this) {
                self.wait(before)?;
            }
        }
        match queued {
            Some(last) => self.wait(last),
            None => Ok(()),
        }
    }

    fn wait(&self, submission: wgpu::SubmissionIndex) -> Result<()> {
        self.device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None }).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(())
    }

    /// For tuning: how long `once` takes (ms), on average over `runs` after one to warm up.
    #[cfg(test)]
    fn time(runs: usize, once: impl Fn()) -> f64 {
        once();
        let started = std::time::Instant::now();
        for _ in 0..runs {
            once();
        }
        started.elapsed().as_secs_f64() * 1000.0 / runs as f64
    }

    /// For tuning: how long the first `n` dispatches take together (ms), as one submission.
    #[cfg(test)]
    fn time_first(&self, n: usize, runs: usize) -> f64 {
        Self::time(runs, || {
            self.submit(0..n);
            self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        })
    }

    /// For tuning: the whole plan, each dispatch run `k` times in a row (ms).
    #[cfg(test)]
    fn time_repeated(&self, k: usize, runs: usize) -> f64 {
        Self::time(runs, || {
            self.queue.submit([self.encode(|pass| (0..self.ops.list.len()).for_each(|i| self.dispatch(pass, i, k)))]);
            self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        })
    }

    /// For tuning: how long each of dispatches `ops` takes (ms), each run `times` times in a row.
    #[cfg(test)]
    fn profile(&self, ops: std::ops::Range<usize>, times: usize) -> Vec<f64> {
        let once = |i: usize, n: usize| {
            let work = self.encode(|pass| self.dispatch(pass, i, n));
            let started = std::time::Instant::now();
            self.queue.submit([work]);
            self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            started.elapsed().as_secs_f64() * 1000.0
        };
        ops.map(|i| {
            once(i, 1);
            // The difference between many and one takes out the cost of submitting.
            (once(i, times + 1) - once(i, 1)) / times as f64
        })
        .collect()
    }

    /// Prints where the time goes in all the dispatches, grouped by `label`'s name for each.
    #[cfg(test)]
    fn report(&self, label: impl Fn(usize, &Kernel, &P) -> String) {
        self.report_part(0..self.ops.list.len(), label);
    }

    /// The same for dispatches `ops`.
    #[cfg(test)]
    fn report_part(&self, ops: std::ops::Range<usize>, label: impl Fn(usize, &Kernel, &P) -> String) {
        let times = self.profile(ops.clone(), 10);
        let mut groups: Vec<(String, f64, usize)> = Vec::new();
        for (i, t) in ops.zip(&times) {
            let (k, p, _) = &self.ops.list[i];
            let name = label(i, k, p);
            match groups.iter_mut().find(|g| g.0 == name) {
                Some(g) => {
                    g.1 += t;
                    g.2 += 1;
                }
                None => groups.push((name, *t, 1)),
            }
        }
        groups.sort_by(|a, b| b.1.total_cmp(&a.1));
        eprintln!("total {:.1} ms over {} dispatches", times.iter().sum::<f64>(), times.len());
        for (name, t, n) in groups.iter().take(25) {
            eprintln!("{t:7.2} ms  {n:3}x  {name}");
        }
    }

    /// Reads `len` floats of `mem` from `at` (waiting for the GPU).
    fn read(&self, at: u32, len: usize) -> Result<Vec<f32>> {
        self.readback.read(&self.device, &self.queue, &self.mem, at, len)
    }
}

/// Somewhere to copy floats out of `mem` to read them: made on the first read, and grown to
/// the biggest since. (The size of all of `mem` would be hundreds of MB that are never read.)
#[derive(Default)]
struct Readback(Mutex<Option<wgpu::Buffer>>);

impl Readback {
    /// Reads `len` floats of `mem` from `at` (waiting for the GPU).
    fn read(&self, device: &wgpu::Device, queue: &wgpu::Queue, mem: &wgpu::Buffer, at: u32, len: usize) -> Result<Vec<f32>> {
        let size = len as u64 * 4;
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut held = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.as_ref().is_none_or(|b| b.size() < size) {
            let usage = wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST;
            *held = Some(device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size, usage, mapped_at_creation: false }));
        }
        let readback = held.as_ref().expect("made above");
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(mem, u64::from(at) * 4, readback, 0, size);
        queue.submit([enc.finish()]);
        let slice = readback.slice(..size);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow::anyhow!("{e}"))?;
        let out = bytemuck::cast_slice(&slice.get_mapped_range().map_err(|e| anyhow::anyhow!("{e}"))?).to_vec();
        readback.unmap();
        Ok(out)
    }
}

/// A picture `grid` patches wide and high, as RGB bytes row by row from the top, normalised
/// and cut into patches: [patch][channel][14][14].
fn patches_of(rgb: &[u8], (gw, gh): (usize, usize)) -> Result<Vec<f32>> {
    let (w, h) = (gw * PATCH, gh * PATCH);
    if rgb.len() != w * h * 3 {
        bail!("expected {w}x{h} RGB, got {} bytes", rgb.len());
    }
    let per = 3 * PATCH * PATCH;
    let mut patches = vec![0.0f32; gw * gh * per];
    for py in 0..gh {
        for px in 0..gw {
            let base = (py * gw + px) * per;
            for c in 0..3 {
                for ky in 0..PATCH {
                    for kx in 0..PATCH {
                        let (x, y) = (px * PATCH + kx, py * PATCH + ky);
                        let v = f32::from(rgb[(y * w + x) * 3 + c]) / 255.0;
                        patches[base + c * PATCH * PATCH + ky * PATCH + kx] = (v - MEAN[c]) / STD[c];
                    }
                }
            }
        }
    }
    Ok(patches)
}

/// Logits [classes][pixels] as how likely each pixel is to be each class.
fn softmax_planes(mut planes: Vec<f32>, pixels: usize) -> Vec<f32> {
    let classes = planes.len() / pixels;
    let mut logits = vec![0.0f32; classes];
    for i in 0..pixels {
        for (c, l) in logits.iter_mut().enumerate() {
            *l = planes[c * pixels + i];
        }
        let top = logits.iter().fold(f32::MIN, |a, b| a.max(*b));
        let total: f32 = logits.iter().map(|l| (l - top).exp()).sum();
        for (c, l) in logits.iter().enumerate() {
            planes[c * pixels + i] = (l - top).exp() / total;
        }
    }
    planes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn dispatch_overhead() {
        for n in [1usize, 191, 382] {
            let up = Upload { data: vec![0.0; 64] };
            let mut ops = Ops::default();
            for _ in 0..n {
                ops.copy(0, 64, 64, 0);
            }
            let run = Runner::new(&up, ops, 4096, "test").unwrap();
            eprintln!("{n} tiny dispatches: {:.2} ms", run.time_first(n, 20));
        }
    }

    /// Both 3x3 kernels (under 32 outputs and over) mirror the edges with REFLECT, at either
    /// stride, the same as working it out here (needs a GPU).
    #[test]
    #[ignore]
    fn conv3x3_reflects() {
        let (cin, h, w) = (3usize, 7usize, 9usize);
        let input: Vec<f32> = (0..cin * h * w).map(|i| ((i * 37 % 101) as f32 - 50.0) / 25.0).collect();
        for (cout, stride) in [(8usize, 1usize), (8, 2), (32, 1), (32, 2)] {
            let weights: Vec<f32> = (0..cout * cin * 9).map(|i| ((i * 53 % 97) as f32 - 48.0) / 50.0).collect();
            let bias: Vec<f32> = (0..cout).map(|o| o as f32 / 10.0).collect();
            let mut up = Upload::default();
            let cv = Conv { w: up.push(&weights), b: up.push(&bias) };
            let (oh, ow) = ((h - 1) / stride + 1, (w - 1) / stride + 1);
            let out = (cin * h * w) as u32;
            let mut ops = Ops::default();
            ops.conv3x3(0, cin, (h, w), cv, cout, out, stride, REFLECT);
            let run = Runner::new(&up, ops, out + (cout * oh * ow) as u32, "test").unwrap();
            run.write(0, &input);
            run.run().unwrap();
            let got = run.read(out, cout * oh * ow).unwrap();
            let mirror = |i: isize, n: usize| (if i < 0 { -i } else if i >= n as isize { 2 * (n as isize - 1) - i } else { i }) as usize;
            let mut worst = 0.0f32;
            for o in 0..cout {
                for y in 0..oh {
                    for x in 0..ow {
                        let mut s = bias[o];
                        for c in 0..cin {
                            for tap in 0..9 {
                                let yy = mirror((y * stride + tap / 3) as isize - 1, h);
                                let xx = mirror((x * stride + tap % 3) as isize - 1, w);
                                s += input[(c * h + yy) * w + xx] * weights[(o * cin + c) * 9 + tap];
                            }
                        }
                        worst = worst.max((got[(o * oh + y) * ow + x] - s).abs());
                    }
                }
            }
            assert!(worst < 1e-4, "{cout} outputs, stride {stride}: off by {worst}");
        }
    }

    #[test]
    fn bicubic_matches_torch_on_a_known_case() {
        // torch.nn.functional.interpolate(torch.arange(16.).reshape(1,1,4,4), size=(2,3),
        // mode="bicubic", align_corners=False)
        let src: Vec<f32> = (0..16).map(|v| v as f32).collect();
        let out = bicubic(&src, 1, (4, 4), (2, 3));
        let expected = [1.751_157_3, 3.125, 4.498_843, 10.501_157, 11.875, 13.248_845];
        for (a, b) in out.iter().zip(expected) {
            assert!((a - b).abs() < 1e-4, "{out:?}");
        }
        // Same size: unchanged.
        assert_eq!(bicubic(&src, 1, (4, 4), (4, 4)), src);
    }
}
