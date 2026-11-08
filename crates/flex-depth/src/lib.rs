//! Depth from a single picture: Depth Anything V2 Small (a DINOv2 ViT-S/14 backbone and a DPT
//! head, Apache-2.0) as hand-written compute shaders (depth.wgsl) run through wgpu: Vulkan,
//! Metal or DX12, with no other runtime. A port of transformers'
//! `DepthAnythingForDepthEstimation` for one picture of a fixed size; tools/reference.py
//! makes the reference outputs the tests compare against, stage by stage.
//!
//! The approach (one storage buffer of activations, a uniform slot per dispatch, the whole
//! picture as one submission) and the core kernels come from Maisy's neurofield
//! (github.com/bombpersons/neurofield), with her permission. Here the weights get a buffer
//! of their own: together with the activations they'd pass wgpu's guaranteed 128 MB.
//!
//! Output is relative inverse depth ("disparity"): bigger is nearer, in no fixed units.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// The model this is a port of, and the version of its weights it was checked against.
pub const MODEL: &str = "depth-anything/Depth-Anything-V2-Small-hf";
pub const REVISION: &str = "5426e4f0f36572d16453bbda7a8389317b1bef99";
/// `model.safetensors` at that revision.
pub const WEIGHTS_URL: &str =
    "https://huggingface.co/depth-anything/Depth-Anything-V2-Small-hf/resolve/5426e4f0f36572d16453bbda7a8389317b1bef99/model.safetensors";
pub const WEIGHTS_SHA256: &str = "3152477ce0d8d6978d76b995120de97cb5b928701fd0f817769f59e249a16b70";
pub const WEIGHTS_BYTES: u64 = 99_173_660;

pub const PATCH: usize = 14;
const D: usize = 384;
const HEADS: usize = 6;
const LAYERS: usize = 12;
/// Blocks whose output feeds the head (stage3, 6, 9, 12).
const TAPS: [usize; 4] = [2, 5, 8, 11];
const NECK: [usize; 4] = [48, 96, 192, 384];
const FUSION: usize = 64;
const HEAD: usize = 32;
/// The position embedding's grid (518 / 14).
const POS_GRID: usize = 37;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];
const LN_EPS: f32 = 1e-6;

const NONE: u32 = u32::MAX;
const GELU: u32 = 1;
const ACCUMULATE: u32 = 2;
const RELU: u32 = 4;
const RELU_IN: u32 = 8;
const B_MEM: u32 = 16;
/// Bytes per uniform slot (wgpu's minimum dynamic-offset alignment).
const SLOT: u64 = 256;
const MAX_OPS: usize = 1024;
/// Dispatches per submission. Run as one the network holds the GPU for tens of milliseconds
/// and whatever else is drawing (the video!) waits behind it; split up, and waiting for each
/// part, other work gets in between.
const OPS_PER_SUBMIT: usize = 8;

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
}

#[derive(Clone, Copy, Debug)]
enum Kernel {
    Matmul,
    LayerNorm,
    Softmax,
    Conv3x3,
    ConvTranspose,
    Resize,
    Copy,
}

const KERNELS: [&str; 7] = ["matmul", "layer_norm", "softmax", "conv3x3", "conv_transpose", "resize", "copy"];

// ---- Weights --------------------------------------------------------------------------------

/// A safetensors file's f32 tensors: name → (shape, values).
type Tensors = HashMap<String, (Vec<usize>, Vec<f32>)>;

fn read_safetensors(bytes: &[u8]) -> Result<Tensors> {
    let len = u64::from_le_bytes(bytes.get(..8).context("not a safetensors file")?.try_into()?) as usize;
    let header: serde_json::Value = serde_json::from_slice(bytes.get(8..8 + len).context("truncated safetensors header")?)?;
    let data = &bytes[8 + len..];
    let mut out = HashMap::new();
    for (name, info) in header.as_object().context("bad safetensors header")? {
        if name == "__metadata__" {
            continue;
        }
        let dtype = info["dtype"].as_str().unwrap_or("");
        let shape: Vec<usize> = info["shape"].as_array().context("no shape")?.iter().filter_map(|v| v.as_u64().map(|v| v as usize)).collect();
        let offsets = info["data_offsets"].as_array().context("no offsets")?;
        let (start, end) = (offsets[0].as_u64().unwrap_or(0) as usize, offsets[1].as_u64().unwrap_or(0) as usize);
        let raw = data.get(start..end).with_context(|| format!("{name}: data out of range"))?;
        let values = match dtype {
            "F32" => raw.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect(),
            "U8" => raw.iter().map(|b| f32::from(*b)).collect(),
            other => bail!("{name}: {other} tensors aren't supported"),
        };
        out.insert(name.clone(), (shape, values));
    }
    Ok(out)
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
        let mut t = vec![0.0; inp * out];
        for o in 0..out {
            let s = scale.map_or(1.0, |s| s[o]);
            for i in 0..inp {
                t[i * out + o] = v[o * inp + i] * s;
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

fn linear(up: &mut Upload, wt: &Weights, name: &str, scale: Option<&[f32]>) -> Result<Linear> {
    let (inp, out, t) = wt.transposed(&format!("{name}.weight"), scale)?;
    let mut b = wt.vec(&format!("{name}.bias"))?;
    if let Some(s) = scale {
        b.iter_mut().zip(s).for_each(|(b, s)| *b *= s);
    }
    Ok(Linear { w: up.push(&t), b: up.push(&b), inp, out })
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
/// [c][oh][ow]: how DINOv2 stretches its position embedding to other picture sizes.
fn bicubic(src: &[f32], c: usize, (ih, iw): (usize, usize), (oh, ow): (usize, usize)) -> Vec<f32> {
    const A: f64 = -0.75;
    let near = |x: f64| ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0;
    let far = |x: f64| ((A * x - 5.0 * A) * x + 8.0 * A) * x - 4.0 * A;
    let taps = |dst: usize, inn: usize, out: usize| {
        let real = inn as f64 / out as f64 * (dst as f64 + 0.5) - 0.5;
        let i = real.floor();
        let t = real - i;
        let w = [far(t + 1.0), near(t), near(1.0 - t), far(2.0 - t)];
        let idx = [-1, 0, 1, 2].map(|d| (i as i64 + d).clamp(0, inn as i64 - 1) as usize);
        (idx, w)
    };
    let mut out = vec![0.0; c * oh * ow];
    for y in 0..oh {
        let (iy, wy) = taps(y, ih, oh);
        for x in 0..ow {
            let (ix, wx) = taps(x, iw, ow);
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

struct Block {
    n1: Norm,
    /// [D][3D]: query, key and value side by side.
    qkv: Linear,
    out: Linear,
    n2: Norm,
    fc1: Linear,
    fc2: Linear,
}

struct Fusion {
    res1: (Conv, Conv),
    res2: (Conv, Conv),
    proj: Linear,
}

// ---- The plan -------------------------------------------------------------------------------

/// Where things are in `mem`.
#[derive(Debug, Default, Clone)]
struct Layout {
    patches: u32,
    x: u32,
    xn: u32,
    qkv: u32,
    scores: u32,
    attn: u32,
    hidden: u32,
    feat: [u32; 4],
    proj: [u32; 4],
    reassembled: [u32; 4],
    neck: [u32; 4],
    fused: [u32; 4],
    g: u32,
    t: u32,
    u: u32,
    h1: u32,
    up: u32,
    h2: u32,
    out: u32,
    end: u32,
}

#[derive(Default)]
struct Ops {
    list: Vec<(Kernel, Params, [u32; 3])>,
}

fn groups(n: usize, per: usize) -> u32 {
    n.div_ceil(per) as u32
}

impl Ops {
    fn push(&mut self, kernel: Kernel, p: Params, g: [u32; 3]) {
        self.list.push((kernel, p, g));
    }

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

    fn norm(&mut self, a: u32, rows: usize, n: Norm, c: u32) {
        let p = Params {
            m: rows as u32,
            n: D as u32,
            a_off: a,
            a_rs: D as u32,
            b_off: n.g,
            bias_off: n.b,
            c_off: c,
            c_rs: D as u32,
            f0: LN_EPS,
            ..Default::default()
        };
        self.push(Kernel::LayerNorm, p, [rows as u32, 1, 1]);
    }

    /// Self-attention over `rows` tokens: q, k and v side by side in `qkv` [rows][3D], the
    /// heads' outputs side by side in `attn` [rows][D].
    fn attention(&mut self, s: &Layout, rows: usize) {
        let dh = D / HEADS;
        let rs = 3 * D;
        let scores = Params {
            m: rows as u32,
            k: dh as u32,
            n: rows as u32,
            batch: HEADS as u32,
            a_off: s.qkv,
            a_bs: dh as u32,
            a_rs: rs as u32,
            a_cs: 1,
            b_off: s.qkv + D as u32,
            b_bs: dh as u32,
            b_rs: 1,
            b_cs: rs as u32,
            c_off: s.scores,
            c_bs: (rows * rows) as u32,
            c_rs: rows as u32,
            c_cs: 1,
            bias_off: NONE,
            flags: B_MEM,
            ..Default::default()
        };
        self.push(Kernel::Matmul, scores, [groups(rows, 32), groups(rows, 32), HEADS as u32]);
        let soft = Params { m: rows as u32, n: rows as u32, batch: HEADS as u32, a_off: s.scores, f0: 1.0 / (dh as f32).sqrt(), ..Default::default() };
        self.push(Kernel::Softmax, soft, [rows as u32, HEADS as u32, 1]);
        let mix = Params {
            m: rows as u32,
            k: rows as u32,
            n: dh as u32,
            batch: HEADS as u32,
            a_off: s.scores,
            a_bs: (rows * rows) as u32,
            a_rs: rows as u32,
            a_cs: 1,
            b_off: s.qkv + 2 * D as u32,
            b_bs: dh as u32,
            b_rs: rs as u32,
            b_cs: 1,
            c_off: s.attn,
            c_bs: dh as u32,
            c_rs: D as u32,
            c_cs: 1,
            bias_off: NONE,
            flags: B_MEM,
            ..Default::default()
        };
        self.push(Kernel::Matmul, mix, [groups(dh, 32), groups(rows, 32), HEADS as u32]);
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
        self.push(Kernel::Conv3x3, p, [groups(ow, 16), groups(oh, 16), groups(cout, 8)]);
    }

    fn conv_transpose(&mut self, a: u32, ch: usize, (h, w): (usize, usize), cv: Conv, s: usize, c: u32) {
        let p = Params { m: h as u32, k: ch as u32, n: w as u32, a_off: a, b_off: cv.w, bias_off: cv.b, c_off: c, x0: ch as u32, x1: s as u32, ..Default::default() };
        self.push(Kernel::ConvTranspose, p, [groups(w * s, 16), groups(h * s, 16), ch as u32]);
    }

    fn resize(&mut self, a: u32, ch: usize, (h, w): (usize, usize), (oh, ow): (usize, usize), c: u32) {
        let p = Params { m: h as u32, k: ch as u32, n: w as u32, a_off: a, c_off: c, x0: oh as u32, x1: ow as u32, ..Default::default() };
        self.push(Kernel::Resize, p, [groups(ow, 16), groups(oh, 16), ch as u32]);
    }

    /// C = A (A in `w`), or with `from_mem` C = A in `mem`, or with ACCUMULATE C += A.
    fn copy(&mut self, a: u32, len: usize, c: u32, flags: u32) {
        let p = Params { m: len as u32, a_off: a, c_off: c, flags, ..Default::default() };
        let blocks = len.div_ceil(64);
        self.push(Kernel::Copy, p, [blocks.min(65535 * 64 / 64) as u32, blocks.div_ceil(65535) as u32, 1]);
    }
}

// ---- The network ----------------------------------------------------------------------------

pub struct Depth {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: Vec<wgpu::ComputePipeline>,
    mem: wgpu::Buffer,
    bind: wgpu::BindGroup,
    readback: wgpu::Buffer,
    ops: Ops,
    s: Layout,
    /// The picture's size in patches (width, height).
    grid: (usize, usize),
    adapter: String,
}

impl Depth {
    /// Loads `model.safetensors` for pictures `grid` patches wide and high (each patch is
    /// 14 pixels; 37 × 21 is 518 × 294).
    pub fn load(weights: &Path, grid: (usize, usize)) -> Result<Self> {
        let bytes = std::fs::read(weights).with_context(|| format!("reading {}", weights.display()))?;
        Self::from_bytes(&bytes, grid)
    }

    pub fn from_bytes(bytes: &[u8], (gw, gh): (usize, usize)) -> Result<Self> {
        if gw == 0 || gh == 0 {
            bail!("an empty picture");
        }
        let wt = Weights { t: read_safetensors(bytes)? };
        let n = gw * gh;
        let tokens = n + 1;
        let mut up = Upload::default();

        // Patch embedding, and the position embedding stretched to this grid, with the
        // class token's own added to its row.
        let (pe_in, pe_out, pe_w) = wt.transposed("backbone.embeddings.patch_embeddings.projection.weight", None)?;
        let pe = Linear { w: up.push(&pe_w), b: up.push(&wt.vec("backbone.embeddings.patch_embeddings.projection.bias")?), inp: pe_in, out: pe_out };
        let pos = &wt.get("backbone.embeddings.position_embeddings")?.1;
        let cls = wt.vec("backbone.embeddings.cls_token")?;
        if pos.len() != (POS_GRID * POS_GRID + 1) * D {
            bail!("unexpected position embedding size");
        }
        // [37*37][D] → [D][37][37] → stretched → [N][D].
        let mut grid_pos = vec![0.0; D * POS_GRID * POS_GRID];
        for i in 0..POS_GRID * POS_GRID {
            for c in 0..D {
                grid_pos[c * POS_GRID * POS_GRID + i] = pos[(1 + i) * D + c];
            }
        }
        let stretched = bicubic(&grid_pos, D, (POS_GRID, POS_GRID), (gh, gw));
        let mut start = vec![0.0; tokens * D];
        for c in 0..D {
            start[c] = cls[c] + pos[c];
            for i in 0..n {
                start[(1 + i) * D + c] = stretched[c * n + i];
            }
        }
        let start_at = up.push(&start);

        let mut blocks = Vec::new();
        for i in 0..LAYERS {
            let p = format!("backbone.encoder.layer.{i}");
            let mut qkv_t = vec![0.0; D * 3 * D];
            let mut qkv_b = Vec::with_capacity(3 * D);
            for (part, name) in ["query", "key", "value"].iter().enumerate() {
                let (_, _, t) = wt.transposed(&format!("{p}.attention.attention.{name}.weight"), None)?;
                for row in 0..D {
                    qkv_t[row * 3 * D + part * D..][..D].copy_from_slice(&t[row * D..][..D]);
                }
                qkv_b.extend(wt.vec(&format!("{p}.attention.attention.{name}.bias"))?);
            }
            let ls1 = wt.vec(&format!("{p}.layer_scale1.lambda1"))?;
            let ls2 = wt.vec(&format!("{p}.layer_scale2.lambda1"))?;
            blocks.push(Block {
                n1: norm(&mut up, &wt, &format!("{p}.norm1"))?,
                qkv: Linear { w: up.push(&qkv_t), b: up.push(&qkv_b), inp: D, out: 3 * D },
                out: linear(&mut up, &wt, &format!("{p}.attention.output.dense"), Some(&ls1))?,
                n2: norm(&mut up, &wt, &format!("{p}.norm2"))?,
                fc1: linear(&mut up, &wt, &format!("{p}.mlp.fc1"), None)?,
                fc2: linear(&mut up, &wt, &format!("{p}.mlp.fc2"), Some(&ls2))?,
            });
        }
        let final_norm = norm(&mut up, &wt, "backbone.layernorm")?;
        let mut reassemble_proj = Vec::new();
        let mut resize = Vec::new();
        let mut neck = Vec::new();
        for k in 0..4 {
            reassemble_proj.push(linear(&mut up, &wt, &format!("neck.reassemble_stage.layers.{k}.projection"), None)?);
            resize.push(if k == 2 { None } else { Some(conv(&mut up, &wt, &format!("neck.reassemble_stage.layers.{k}.resize"))?) });
            neck.push(conv(&mut up, &wt, &format!("neck.convs.{k}"))?);
        }
        let mut fusion = Vec::new();
        for j in 0..4 {
            let p = format!("neck.fusion_stage.layers.{j}");
            let rcu = |up: &mut Upload, r: &str| -> Result<(Conv, Conv)> {
                Ok((conv(up, &wt, &format!("{p}.{r}.convolution1"))?, conv(up, &wt, &format!("{p}.{r}.convolution2"))?))
            };
            fusion.push(Fusion { res1: rcu(&mut up, "residual_layer1")?, res2: rcu(&mut up, "residual_layer2")?, proj: linear(&mut up, &wt, &format!("{p}.projection"), None)? });
        }
        let head1 = conv(&mut up, &wt, "head.conv1")?;
        let head2 = conv(&mut up, &wt, "head.conv2")?;
        let head3 = linear(&mut up, &wt, "head.conv3", None)?;

        // Sizes of each level: reassembled at 4x, 2x, 1x and 1/2 the patch grid.
        let level = |k: usize| match k {
            0 => (gh * 4, gw * 4),
            1 => (gh * 2, gw * 2),
            2 => (gh, gw),
            _ => ((gh - 1) / 2 + 1, (gw - 1) / 2 + 1),
        };
        let fused_size = |j: usize| if j < 3 { level(2 - j) } else { (level(0).0 * 2, level(0).1 * 2) };
        let (out_h, out_w) = (gh * PATCH, gw * PATCH);

        let mut end = 0u32;
        let mut alloc = |len: usize| {
            let at = end;
            end += len as u32;
            at
        };
        let mut s = Layout {
            patches: alloc(n * pe_in),
            x: alloc(tokens * D),
            xn: alloc(tokens * D),
            qkv: alloc(tokens * 3 * D),
            scores: alloc(HEADS * tokens * tokens),
            attn: alloc(tokens * D),
            hidden: alloc(tokens * 4 * D),
            ..Default::default()
        };
        for (k, &channels) in NECK.iter().enumerate() {
            s.feat[k] = alloc(tokens * D);
            s.proj[k] = alloc(channels * n);
            let (h, w) = level(k);
            s.reassembled[k] = if k == 2 { s.proj[k] } else { alloc(channels * h * w) };
            s.neck[k] = alloc(FUSION * h * w);
        }
        for j in 0..4 {
            let (h, w) = fused_size(j);
            s.fused[j] = alloc(FUSION * h * w);
        }
        let biggest_in = FUSION * level(0).0 * level(0).1;
        s.g = alloc(biggest_in);
        s.t = alloc(biggest_in);
        s.u = alloc(FUSION * fused_size(3).0 * fused_size(3).1);
        let (fh, fw) = fused_size(3);
        s.h1 = alloc(HEAD * fh * fw);
        s.up = alloc(HEAD * out_h * out_w);
        s.h2 = alloc(HEAD * out_h * out_w);
        s.out = alloc(out_h * out_w);
        s.end = end;

        // The forward pass.
        let mut ops = Ops::default();
        ops.copy(start_at, tokens * D, s.x, 0);
        ops.linear((s.patches, pe_in), n, pe, (s.x + D as u32, D), ACCUMULATE);
        let mut tap = 0;
        for (i, b) in blocks.iter().enumerate() {
            ops.norm(s.x, tokens, b.n1, s.xn);
            ops.linear((s.xn, D), tokens, b.qkv, (s.qkv, 3 * D), 0);
            ops.attention(&s, tokens);
            ops.linear((s.attn, D), tokens, b.out, (s.x, D), ACCUMULATE);
            ops.norm(s.x, tokens, b.n2, s.xn);
            ops.linear((s.xn, D), tokens, b.fc1, (s.hidden, 4 * D), GELU);
            ops.linear((s.hidden, 4 * D), tokens, b.fc2, (s.x, D), ACCUMULATE);
            if TAPS.contains(&i) {
                ops.norm(s.x, tokens, final_norm, s.feat[tap]);
                tap += 1;
            }
        }
        for k in 0..4 {
            // Tokens (without the class token) to channels-first [C][gh][gw].
            let l = reassemble_proj[k];
            let p = Params {
                m: n as u32,
                k: D as u32,
                n: l.out as u32,
                batch: 1,
                a_off: s.feat[k] + D as u32,
                a_rs: D as u32,
                a_cs: 1,
                b_off: l.w,
                b_rs: l.out as u32,
                b_cs: 1,
                c_off: s.proj[k],
                c_rs: 1,
                c_cs: n as u32,
                bias_off: l.b,
                ..Default::default()
            };
            ops.push(Kernel::Matmul, p, [groups(l.out, 32), groups(n, 32), 1]);
            match (k, resize[k]) {
                (0, Some(cv)) => ops.conv_transpose(s.proj[k], NECK[k], (gh, gw), cv, 4, s.reassembled[k]),
                (1, Some(cv)) => ops.conv_transpose(s.proj[k], NECK[k], (gh, gw), cv, 2, s.reassembled[k]),
                (3, Some(cv)) => ops.conv3x3(s.proj[k], NECK[k], (gh, gw), cv, NECK[k], s.reassembled[k], 2, 0),
                _ => {}
            }
            ops.conv3x3(s.reassembled[k], NECK[k], level(k), neck[k], FUSION, s.neck[k], 1, 0);
        }
        // Fusion, from the coarsest level up: x = previous + rcu1(this level); x = rcu2(x);
        // resized to the next level (or doubled) and projected.
        for (j, f) in fusion.iter().enumerate() {
            let k = 3 - j;
            let (h, w) = level(k);
            let len = FUSION * h * w;
            if j == 0 {
                ops.copy(s.neck[k], len, s.g, B_MEM);
            } else {
                ops.copy(s.fused[j - 1], len, s.g, B_MEM);
                ops.copy(s.neck[k], len, s.g, ACCUMULATE);
                ops.conv3x3(s.neck[k], FUSION, (h, w), f.res1.0, FUSION, s.t, 1, RELU_IN);
                ops.conv3x3(s.t, FUSION, (h, w), f.res1.1, FUSION, s.g, 1, RELU_IN | ACCUMULATE);
            }
            ops.conv3x3(s.g, FUSION, (h, w), f.res2.0, FUSION, s.t, 1, RELU_IN);
            ops.conv3x3(s.t, FUSION, (h, w), f.res2.1, FUSION, s.g, 1, RELU_IN | ACCUMULATE);
            let size = fused_size(j);
            ops.resize(s.g, FUSION, (h, w), size, s.u);
            ops.conv1x1(s.u, size.0 * size.1, f.proj, s.fused[j], 0);
        }
        ops.conv3x3(s.fused[3], FUSION, (fh, fw), head1, HEAD, s.h1, 1, 0);
        ops.resize(s.h1, HEAD, (fh, fw), (out_h, out_w), s.up);
        ops.conv3x3(s.up, HEAD, (out_h, out_w), head2, HEAD, s.h2, 1, RELU);
        ops.conv1x1(s.h2, out_h * out_w, head3, s.out, RELU);
        if ops.list.len() > MAX_OPS {
            bail!("{} dispatches, room for {MAX_OPS}", ops.list.len());
        }

        // The GPU.
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| anyhow::anyhow!("no GPU adapter: {e}"))?;
        let w_bytes = up.data.len() as u64 * 4;
        let mem_bytes = s.end as u64 * 4;
        let needed = w_bytes.max(mem_bytes);
        let mut limits = wgpu::Limits::default();
        let most = adapter.limits();
        if needed > most.max_storage_buffer_binding_size as u64 {
            bail!("the GPU allows {} MB buffers; depth needs {} MB", most.max_storage_buffer_binding_size >> 20, needed >> 20);
        }
        limits.max_storage_buffer_binding_size = limits.max_storage_buffer_binding_size.max(needed);
        limits.max_buffer_size = limits.max_buffer_size.max(needed);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("flex-depth"),
            required_limits: limits,
            ..Default::default()
        }))
        .map_err(|e| anyhow::anyhow!("GPU device: {e}"))?;

        let storage = |label, size: u64, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
        let w = storage("weights", w_bytes, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST);
        queue.write_buffer(&w, 0, bytemuck::cast_slice(&up.data));
        let mem = storage("mem", mem_bytes, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC);
        let params = storage("params", SLOT * MAX_OPS as u64, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let readback = storage("readback", mem_bytes, wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST);
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flex-depth"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Params>() as u64),
                    },
                ),
                entry(1, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }),
                entry(2, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }),
            ],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flex-depth"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &params,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<Params>() as u64),
                    }),
                },
                wgpu::BindGroupEntry { binding: 1, resource: mem.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: w.as_entire_binding() },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flex-depth"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("depth.wgsl").into()),
        });
        let pipelines = KERNELS
            .iter()
            .map(|entry| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
            .collect();
        // The parameters never change: write them once.
        let mut slots = vec![0u8; ops.list.len() * SLOT as usize];
        for (i, (_, p, _)) in ops.list.iter().enumerate() {
            slots[i * SLOT as usize..][..std::mem::size_of::<Params>()].copy_from_slice(bytemuck::bytes_of(p));
        }
        queue.write_buffer(&params, 0, &slots);
        drop(w); // the bind group keeps it
        Ok(Self { device, queue, pipelines, mem, bind, readback, ops, s, grid: (gw, gh), adapter: adapter.get_info().name })
    }

    /// The picture size it takes, in pixels.
    pub fn size(&self) -> (usize, usize) {
        (self.grid.0 * PATCH, self.grid.1 * PATCH)
    }

    /// The GPU it runs on.
    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    /// Depth for a picture of exactly `size()`, as RGB bytes row by row from the top:
    /// one value per pixel, the same way round.
    pub fn run(&mut self, rgb: &[u8]) -> Result<Vec<f32>> {
        self.submit(rgb)?;
        let (w, h) = self.size();
        self.read(self.s.out, w * h)
    }

    /// Uploads the picture and runs every dispatch.
    fn submit(&mut self, rgb: &[u8]) -> Result<()> {
        let (gw, gh) = self.grid;
        let (w, h) = self.size();
        if rgb.len() != w * h * 3 {
            bail!("expected {w}x{h} RGB, got {} bytes", rgb.len());
        }
        // Normalised and cut into patches: [patch][channel][14][14].
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
        self.queue.write_buffer(&self.mem, self.s.patches as u64 * 4, bytemuck::cast_slice(&patches));
        for (part, ops) in self.ops.list.chunks(OPS_PER_SUBMIT).enumerate() {
            let mut enc = self.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                for (i, (kernel, _, g)) in ops.iter().enumerate() {
                    if g.contains(&0) {
                        continue;
                    }
                    pass.set_pipeline(&self.pipelines[*kernel as usize]);
                    let slot = (part * OPS_PER_SUBMIT + i) as u64 * SLOT;
                    pass.set_bind_group(0, &self.bind, &[slot as u32]);
                    pass.dispatch_workgroups(g[0], g[1], g[2]);
                }
            }
            self.queue.submit([enc.finish()]);
            self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        Ok(())
    }

    /// Reads `len` floats of `mem` from `at` (waiting for the GPU).
    fn read(&self, at: u32, len: usize) -> Result<Vec<f32>> {
        let size = len as u64 * 4;
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&self.mem, at as u64 * 4, &self.readback, 0, size);
        self.queue.submit([enc.finish()]);
        let slice = self.readback.slice(..size);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow::anyhow!("{e}"))?;
        let out = bytemuck::cast_slice(&slice.get_mapped_range().map_err(|e| anyhow::anyhow!("{e}"))?).to_vec();
        self.readback.unmap();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Compares every stage with PyTorch's (needs the weights and a reference file from
    /// tools/reference.py, and a GPU):
    /// `FLEX_DEPTH_WEIGHTS=…/model.safetensors FLEX_DEPTH_REF=ref.safetensors cargo test -p flex-depth matches_pytorch -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn matches_pytorch() {
        let weights = std::env::var("FLEX_DEPTH_WEIGHTS").expect("FLEX_DEPTH_WEIGHTS");
        let reference = read_safetensors(&std::fs::read(std::env::var("FLEX_DEPTH_REF").expect("FLEX_DEPTH_REF")).unwrap()).unwrap();
        let (shape, rgb) = &reference["rgb"];
        let (h, w) = (shape[0], shape[1]);
        let rgb: Vec<u8> = rgb.iter().map(|v| *v as u8).collect();
        let started = std::time::Instant::now();
        let mut depth = Depth::load(Path::new(&weights), (w / PATCH, h / PATCH)).unwrap();
        eprintln!("loaded in {:?} on {}", started.elapsed(), depth.adapter());
        depth.submit(&rgb).unwrap();

        let s = depth.s.clone();
        let mut stages: Vec<(String, u32)> = Vec::new();
        for k in 0..4 {
            stages.push((format!("features.{k}"), s.feat[k]));
            stages.push((format!("reassemble.{k}"), s.reassembled[k]));
            stages.push((format!("neck_conv.{k}"), s.neck[k]));
            stages.push((format!("fusion.{k}"), s.fused[k]));
        }
        stages.push(("depth".into(), s.out));
        let mut worst = 0.0f32;
        for (name, at) in stages {
            let expected = &reference[&name].1;
            let got = depth.read(at, expected.len()).unwrap();
            let scale = expected.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let err = got.iter().zip(expected).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
            eprintln!("{name:14} max |ref| {scale:10.4}  max error {err:.2e}  relative {:.2e}", err / scale);
            worst = worst.max(err / scale);
        }
        assert!(worst < 1e-3, "worst relative error {worst:.2e}");

        // Speed, once warmed up.
        let runs = 10;
        let started = std::time::Instant::now();
        for _ in 0..runs {
            depth.run(&rgb).unwrap();
        }
        eprintln!("{:.1} ms a picture ({w}x{h})", started.elapsed().as_secs_f64() * 1000.0 / runs as f64);
    }
}
