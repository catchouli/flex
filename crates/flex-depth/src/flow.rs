//! How everything in the picture is moving: NeuFlow v2 (Apache 2.0, github.com/neufieldrobotics/
//! NeuFlow_v2, the "mixed" weights) as compute shaders, next to the depth network's. Its last
//! layers, which upscale the result, are left out: it stops at one value per 8 pixels, which is
//! finer than effects use. `tools/flow/convert.py` makes the weights, `tools/flow/reference.py`
//! the reference the tests compare against.

use std::collections::HashMap;
use std::ops::Range;

use anyhow::Result;

use super::{ACCUMULATE, Conv, GELU, Kernel, Kernels, LEAKY, Linear, Mem, NONE, Norm, Ops, Params, Runner, Upload, Weights, Wino, conv, groups, linear, norm, read_safetensors, wino_u};

const WEIGHTS: &str = "flow.safetensors";
/// The picture it takes.
pub const WIDTH: usize = 512;
pub const HEIGHT: usize = 288;
/// What comes out, which is one value per 8 pixels.
pub const FLOW_WIDTH: usize = WIDTH / 8;
pub const FLOW_HEIGHT: usize = HEIGHT / 8;
/// Refinement steps. On film two come out about the same as the eight it was trained with.
const STEPS: usize = 2;
/// Channels of the features at each scale.
const FEATURES: usize = 128;
const CONTEXT: usize = 64;
/// In the block a refinement step reads: correlations, context, memory, flow and radius.
const BLOCK: usize = 212;
const AT_CONTEXT: usize = 81;
const AT_MEMORY: usize = 145;
const AT_FLOW: usize = 209;
const AT_RADIUS: usize = 211;
const RADIUS: f32 = 4.0;
const LN_EPS: f32 = 1e-5;
/// Dispatches a submission: about 4 ms of work on an RX 580, so that what else is drawing can
/// get in between, while each submission's own cost stays small.
const OPS_PER_SUBMIT: usize = 40;

/// Flow's kernels, beside depth.wgsl's.
impl Ops {
    /// The larger convolutions, always with leaky ReLU after.
    #[allow(clippy::too_many_arguments)]
    fn conv(&mut self, a: u32, cin: usize, (h, w): (usize, usize), cv: Conv, cout: usize, c: u32, (size, stride, pad): (usize, usize, usize)) {
        let (oh, ow) = ((h + 2 * pad - size) / stride + 1, (w + 2 * pad - size) / stride + 1);
        let p = Params { m: h as u32, k: cin as u32, n: w as u32, a_off: a, b_off: cv.w, bias_off: cv.b, c_off: c, x0: cout as u32, x1: stride as u32, a_bs: size as u32, a_rs: pad as u32, b_bs: oh as u32, b_rs: ow as u32, flags: LEAKY, ..Default::default() };
        self.push(Kernel::FlowConv, p, [groups(ow, 16), groups(oh, 16), groups(cout, 8)]);
    }

    fn half(&mut self, a: u32, channels: usize, (h, w): (usize, usize), c: u32) {
        let p = Params { m: h as u32, n: w as u32, a_off: a, c_off: c, ..Default::default() };
        self.push(Kernel::Half, p, [groups(w / 2, 16), groups(h / 2, 16), channels as u32]);
    }

    fn double(&mut self, a: u32, channels: usize, (h, w): (usize, usize), c: u32, scale: f32) {
        let p = Params { m: h as u32, n: w as u32, a_off: a, c_off: c, f0: scale, ..Default::default() };
        self.push(Kernel::FlowDouble, p, [groups(w * 2, 16), groups(h * 2, 16), channels as u32]);
    }

    /// `count` values: clamp(v * scale + shift, range), then `flags`.
    #[allow(clippy::too_many_arguments)]
    fn map(&mut self, a: u32, count: usize, c: u32, scale: f32, shift: f32, range: (f32, f32), flags: u32) {
        let p = Params { m: count as u32, a_off: a, c_off: c, f0: scale, f1: shift, f2: range.0, f3: range.1, flags, ..Default::default() };
        self.push(Kernel::Map, p, [groups(count, 64), 1, 1]);
    }

    /// C = A in `mem`, through `map`.
    fn copy_map(&mut self, a: u32, count: usize, c: u32) {
        self.map(a, count, c, 1.0, 0.0, (f32::MIN, f32::MAX), 0);
    }

    fn zero(&mut self, count: usize, c: u32) {
        self.map(c, count, c, 0.0, 0.0, (f32::MIN, f32::MAX), 0);
    }

    fn relu(&mut self, a: u32, count: usize, c: u32) {
        self.map(a, count, c, 1.0, 0.0, (0.0, f32::MAX), 0);
    }

    fn add(&mut self, a: u32, count: usize, c: u32) {
        self.map(a, count, c, 1.0, 0.0, (f32::MIN, f32::MAX), ACCUMULATE);
    }

    fn transpose(&mut self, a: u32, (m, n): (usize, usize), c: u32) {
        let p = Params { m: m as u32, n: n as u32, a_off: a, c_off: c, ..Default::default() };
        self.push(Kernel::Transpose, p, [groups(m * n, 64), 1, 1]);
    }

    /// C[m][n] = A[m][k] x B[k][n], both in `mem`.
    #[allow(clippy::too_many_arguments)]
    fn multiply(&mut self, a: (u32, u32, u32), b: (u32, u32, u32), (m, k, n): (usize, usize, usize), c: (u32, u32)) {
        let p = Params { m: m as u32, k: k as u32, n: n as u32, batch: 1, a_off: a.0, a_rs: a.1, a_cs: a.2, b_off: b.0, b_rs: b.1, b_cs: b.2, c_off: c.0, c_rs: c.1, c_cs: 1, bias_off: NONE, ..Default::default() };
        self.push(Kernel::MatmulMem, p, [groups(n, 32), groups(m, 32), 1]);
    }

    fn softmax(&mut self, a: u32, size: usize, scale: f32) {
        let p = Params { m: size as u32, n: size as u32, a_off: a, f0: scale, ..Default::default() };
        self.push(Kernel::Softmax, p, [size as u32, 1, 1]);
    }
}

/// For a large convolution with many input channels (only one in the backbone): how many parts
/// to split its sum into as a matrix multiply, so that there are enough workgroups. The direct
/// kernel has too few on its small picture, each with a long sum.
fn matmul_parts(kernel: Kernel, p: &Params) -> Option<usize> {
    if !matches!(kernel, Kernel::FlowConv) || p.k < 32 {
        return None;
    }
    let rows = (p.k * p.a_bs * p.a_bs) as usize;
    let tiles = (p.b_bs * p.b_rs).div_ceil(32) as usize * (p.x0 as usize).div_ceil(32);
    let wanted = 144usize.div_ceil(tiles).clamp(1, rows / 256);
    // Parts that divide the sum evenly, each a whole number of the multiply's steps of 16.
    (1..=wanted).rev().find(|n| rows.is_multiple_of(n * 16))
}

/// Floats of scratch a convolution done as a matrix multiply needs.
fn matmul_scratch(p: &Params, parts: usize) -> usize {
    let pixels = (p.b_bs * p.b_rs) as usize;
    (p.k * p.a_bs * p.a_bs) as usize * pixels + parts * p.x0 as usize * pixels
}

/// `ops` with its 3x3 convolutions done by Winograd (`conv3x3_wino`) and its large convolution
/// as a matrix multiply. NeuFlow's pictures are small, so the direct convolutions have too few
/// workgroups to keep the GPU busy, where the matrix multiplies have plenty. Weights are
/// transformed into `up` (once each, remembered in `done`); the working space is at `scratch`.
fn winograd(ops: Ops, up: &mut Upload, done: &mut HashMap<u32, u32>, scratch: u32) -> Ops {
    let mut out = Ops::default();
    for (kernel, p, g) in ops.list {
        if let Some(parts) = matmul_parts(kernel, &p) {
            // A convolution with many input channels as a matrix multiply, its sum split into
            // `parts` side by side: C [pixel][cout] = cols^T [pixel][cin * taps] W^T.
            let (cin, cout, taps, pixels) = (p.k as usize, p.x0 as usize, (p.a_bs * p.a_bs) as usize, (p.b_bs * p.b_rs) as usize);
            let (rows, each) = (cin * taps, cin * taps / parts);
            let cols = scratch;
            let sums = scratch + (rows * pixels) as u32;
            let n = rows * pixels;
            out.push(Kernel::Im2col, Params { c_off: cols, ..p }, [groups(n, 64).min(65535), groups(n, 64 * 65535), 1]);
            let mul = Params {
                m: pixels as u32,
                k: each as u32,
                n: cout as u32,
                batch: parts as u32,
                a_off: cols,
                a_bs: (each * pixels) as u32,
                a_rs: 1,
                a_cs: pixels as u32,
                b_off: p.b_off,
                b_bs: each as u32,
                b_rs: 1,
                b_cs: rows as u32,
                c_off: sums,
                c_bs: (cout * pixels) as u32,
                c_rs: 1,
                c_cs: pixels as u32,
                bias_off: NONE,
                ..Default::default()
            };
            out.push(Kernel::Matmul, mul, [groups(cout, 32), groups(pixels, 32), parts as u32]);
            let sum = Params { k: cout as u32, m: pixels as u32, x0: parts as u32, a_off: sums, c_off: p.c_off, bias_off: p.bias_off, flags: p.flags, ..Default::default() };
            out.push(Kernel::Sum, sum, [groups(cout * pixels, 64), 1, 1]);
            continue;
        }
        if !matches!(kernel, Kernel::Conv3x3 | Kernel::Conv3x3Wide) {
            out.push(kernel, p, g);
            continue;
        }
        let (h, w, cin, cout) = (p.m as usize, p.n as usize, p.k as usize, p.x0 as usize);
        let u = *done.entry(p.b_off).or_insert_with(|| {
            let raw = up.data[p.b_off as usize..][..cout * cin * 9].to_vec();
            up.push(&wino_u(&raw, cout, cin))
        });
        out.conv3x3_wino(p.a_off, (h, w), Wino { u, b: p.bias_off, cin, cout }, p.c_off, scratch, p.flags);
    }
    out
}

/// One picture's input and backbone outputs: everything worked out from it alone.
#[derive(Clone, Copy)]
struct Picture {
    input: u32,
    s16: u32,
    s8: u32,
}

/// A layer of the attention: q, k, v, the merge, its norm, the MLP's two parts and its norm.
struct Layer {
    q: Linear,
    k: Linear,
    v: Linear,
    merge: Linear,
    norm1: Norm,
    /// [2 D][2 D], no bias.
    mlp0: Linear,
    mlp1: Linear,
    norm2: Norm,
}

struct Refine {
    conv1: Conv,
    conv2: Conv,
    layers: [Conv; 5],
    conv3: Conv,
    inner: usize,
}

fn refine_weights(up: &mut Upload, wt: &Weights, name: &str, inner: usize) -> Result<Refine> {
    let conv1 = conv(up, wt, &format!("{name}.conv1.conv"))?;
    let conv2 = conv(up, wt, &format!("{name}.conv2.conv"))?;
    let mut layers = [Conv { w: 0, b: NONE }; 5];
    for (i, l) in layers.iter_mut().enumerate() {
        *l = conv(up, wt, &format!("{name}.conv_layers.{i}.conv"))?;
    }
    let conv3 = conv(up, wt, &format!("{name}.conv3"))?;
    Ok(Refine { conv1, conv2, layers, conv3, inner })
}

/// A step of refinement on `block` ([BLOCK][h][w]), using working space `work`.
fn refine(ops: &mut Ops, r: &Refine, corr: u32, block: u32, (h, w): (usize, usize), work: &[u32], scale: f32) {
    let plane = h * w;
    let flow = block + (AT_FLOW * plane) as u32;
    let p = Params { m: h as u32, n: w as u32, a_off: corr, b_off: flow, c_off: block, f0: scale, ..Default::default() };
    ops.push(Kernel::Correlate, p, [groups(w, 16), groups(h, 16), 1]);
    ops.conv3x3(block, BLOCK, (h, w), r.conv1, 128, work[0], 1, LEAKY);
    ops.conv3x3(work[0], 128, (h, w), r.conv2, r.inner, work[1], 1, LEAKY);
    for (i, layer) in r.layers.iter().enumerate() {
        ops.conv3x3(work[1 + i], r.inner, (h, w), *layer, r.inner, work[2 + i], 1, LEAKY);
    }
    ops.conv3x3(work[6], r.inner, (h, w), r.conv3, 66, work[7], 1, 0);
    ops.map(work[7] + (2 * plane) as u32, 64 * plane, block + (AT_MEMORY * plane) as u32, 1.0, 0.0, (-4.0, 4.0), 0);
    ops.add(work[7], 2 * plane, flow);
}

fn backbone(ops: &mut Ops, up: &mut Upload, wt: &Weights, memory: &mut Mem, picture: Picture) -> Result<()> {
    let (w4, h4) = (WIDTH / 2, HEIGHT / 2);
    let (w8, h8) = (WIDTH / 4, HEIGHT / 4);
    let (w16, h16) = (WIDTH / 8, HEIGHT / 8);
    let s8 = (HEIGHT / 8, WIDTH / 8);
    let s16 = (HEIGHT / 16, WIDTH / 16);
    let half1 = memory.take(3 * h4 * w4);
    let half2 = memory.take(3 * h8 * w8);
    let half3 = memory.take(3 * h16 * w16);
    ops.half(picture.input, 3, (HEIGHT, WIDTH), half1);
    ops.half(half1, 3, (h4, w4), half2);
    ops.half(half2, 3, (h8, w8), half3);

    let plane8 = s8.0 * s8.1;
    let plane16 = s16.0 * s16.1;
    let block = |up: &mut Upload, name: &str| -> Result<(Conv, Conv)> { Ok((conv(up, wt, &format!("backbone.{name}.conv1"))?, conv(up, wt, &format!("backbone.{name}.conv2"))?)) };

    let (one, two) = block(up, "block_8_1")?;
    let joined = memory.take(384 * plane8);
    let first = memory.take(256 * plane8);
    ops.conv(half1, 3, (h4, w4), one, 256, first, (8, 4, 2));
    ops.conv3x3(first, 256, s8, two, 256, joined, 1, LEAKY);

    let (one, two) = block(up, "block_8_2")?;
    let first = memory.take(128 * plane8);
    ops.conv(half2, 3, (h8, w8), one, 128, first, (6, 2, 2));
    ops.conv3x3(first, 128, s8, two, 128, joined + (256 * plane8) as u32, 1, LEAKY);

    let (one, two) = block(up, "block_cat_8")?;
    let first = memory.take(192 * plane8);
    ops.conv3x3(joined, 384, s8, one, 192, first, 1, LEAKY);
    ops.conv3x3(first, 192, s8, two, 192, picture.s8, 1, LEAKY);

    let (one, two) = block(up, "block_16_1")?;
    let joined = memory.take(256 * plane16);
    let first = memory.take(128 * plane16);
    ops.conv(half3, 3, (h16, w16), one, 128, first, (6, 2, 2));
    ops.conv3x3(first, 128, s16, two, 128, joined, 1, LEAKY);

    let (one, two) = block(up, "block_8_16")?;
    let first = memory.take(128 * plane16);
    ops.conv(picture.s8, 192, s8, one, 128, first, (6, 2, 2));
    ops.conv3x3(first, 128, s16, two, 128, joined + (128 * plane16) as u32, 1, LEAKY);

    let (one, two) = block(up, "block_cat_16")?;
    let first = memory.take(190 * plane16);
    ops.conv3x3(joined, 256, s16, one, 190, first, 1, LEAKY);
    ops.conv3x3(first, 190, s16, two, 190, picture.s16, 1, LEAKY);
    Ok(())
}

/// Where the rest of the network keeps things, for a pair of pictures. The same places whichever
/// picture is the current one.
struct Shared {
    tokens: [u32; 2],
    messages: [u32; 2],
    q: u32,
    k: u32,
    v: u32,
    scores: u32,
    merged: u32,
    wide: u32,
    attended: [u32; 2],
    corr16: u32,
    soft: u32,
    block16: u32,
    work16: Vec<u32>,
    merge_in: [u32; 2],
    merge_mid: u32,
    features8: [u32; 2],
    context_in: u32,
    context_mid: u32,
    corr8: u32,
    block8: u32,
    work8: Vec<u32>,
}

struct Tails {
    layers: [Layer; 2],
    /// Each channel's scale and shift.
    channels: Norm,
    merge0: Conv,
    merge2: Conv,
    context0: Conv,
    context2: Conv,
    refine16: Refine,
    refine8: Refine,
}

const D: usize = 192;
const S16: (usize, usize) = (HEIGHT / 16, WIDTH / 16);
const S8: (usize, usize) = (HEIGHT / 8, WIDTH / 8);
const TOKENS: usize = S16.0 * S16.1;
const PIXELS8: usize = S8.0 * S8.1;

/// Everything after the backbone: `now` is the picture being worked out, `last` the one before.
fn rest(ops: &mut Ops, s: &Shared, t: &Tails, now: Picture, last: Picture) {
    let root = (D as f32).sqrt().recip();
    let (m, n) = (TOKENS, D);
    ops.transpose(now.s16, (D, TOKENS), s.tokens[0]);
    ops.transpose(last.s16, (D, TOKENS), s.tokens[1]);
    for layer in &t.layers {
        for side in 0..2 {
            let (src, target) = (s.tokens[side], s.tokens[1 - side]);
            ops.linear((src, n), m, layer.q, (s.q, n), 0);
            ops.linear((target, n), m, layer.k, (s.k, n), 0);
            ops.linear((target, n), m, layer.v, (s.v, n), 0);
            ops.multiply((s.q, n as u32, 1), (s.k, 1, n as u32), (m, n, m), (s.scores, m as u32));
            ops.softmax(s.scores, m, root);
            ops.multiply((s.scores, m as u32, 1), (s.v, n as u32, 1), (m, m, n), (s.merged, n as u32));
            ops.linear((s.merged, n), m, layer.merge, (s.q, n), 0);
            ops.layer_norm(s.q, (m, n), layer.norm1, s.merged, LN_EPS);
            // The MLP's input is the tokens and the message side by side: its weight's first
            // half of rows for one, the second for the other.
            let half = Linear { inp: n, ..layer.mlp0 };
            ops.linear((src, n), m, half, (s.wide, 2 * n), 0);
            ops.linear((s.merged, n), m, Linear { w: half.w + (n * 2 * n) as u32, ..half }, (s.wide, 2 * n), ACCUMULATE);
            ops.map(s.wide, m * 2 * n, s.wide, 1.0, 0.0, (f32::MIN, f32::MAX), GELU);
            ops.linear((s.wide, 2 * n), m, layer.mlp1, (s.q, n), 0);
            ops.layer_norm(s.q, (m, n), layer.norm2, s.messages[side], LN_EPS);
        }
        ops.add(s.messages[0], m * n, s.tokens[0]);
        ops.add(s.messages[1], m * n, s.tokens[1]);
    }
    for side in 0..2 {
        ops.transpose(s.tokens[side], (TOKENS, D), s.attended[side]);
        let p = Params { m: TOKENS as u32, k: D as u32, a_off: s.attended[side], b_off: t.channels.g, bias_off: t.channels.b, c_off: s.attended[side], ..Default::default() };
        ops.push(Kernel::Channels, p, [groups(D * TOKENS, 64), 1, 1]);
    }

    // Matching, and a step of refinement, at the coarser scale.
    let feature = |a: u32, plane: usize| a + (CONTEXT * plane) as u32;
    let f16 = [feature(s.attended[0], TOKENS), feature(s.attended[1], TOKENS)];
    let root16 = (FEATURES as f32).sqrt().recip();
    ops.multiply((f16[0], 1, TOKENS as u32), (f16[1], TOKENS as u32, 1), (TOKENS, FEATURES, TOKENS), (s.corr16, TOKENS as u32));
    ops.copy_map(s.corr16, TOKENS * TOKENS, s.soft);
    ops.softmax(s.soft, TOKENS, root16);
    let flow16 = s.block16 + (AT_FLOW * TOKENS) as u32;
    let p = Params { m: S16.0 as u32, n: S16.1 as u32, a_off: s.soft, c_off: flow16, ..Default::default() };
    ops.push(Kernel::Expect, p, [groups(TOKENS, 64), 1, 1]);
    let context16 = s.block16 + (AT_CONTEXT * TOKENS) as u32;
    ops.relu(s.attended[0], CONTEXT * TOKENS, context16);
    ops.zero(CONTEXT * TOKENS, s.block16 + (AT_MEMORY * TOKENS) as u32);
    refine(ops, &t.refine16, s.corr16, s.block16, S16, &s.work16, root16);

    // Up to the finer scale.
    let flow8 = s.block8 + (AT_FLOW * PIXELS8) as u32;
    ops.double(flow16, 2, S16, flow8, 2.0);
    for (side, picture) in [now, last].into_iter().enumerate() {
        let joined = s.merge_in[side];
        ops.copy_map(feature(picture.s8, PIXELS8), FEATURES * PIXELS8, joined);
        ops.double(f16[side], FEATURES, S16, joined + (FEATURES * PIXELS8) as u32, 1.0);
        ops.conv3x3(joined, 2 * FEATURES, S8, t.merge0, FEATURES, s.merge_mid, 1, GELU);
        ops.conv3x3(s.merge_mid, FEATURES, S8, t.merge2, FEATURES, s.features8[side], 1, 0);
    }
    ops.relu(now.s8, CONTEXT * PIXELS8, s.context_in);
    ops.double(context16, CONTEXT, S16, s.context_in + (CONTEXT * PIXELS8) as u32, 1.0);
    ops.conv3x3(s.context_in, 2 * CONTEXT, S8, t.context0, CONTEXT, s.context_mid, 1, GELU);
    ops.conv3x3(s.context_mid, CONTEXT, S8, t.context2, CONTEXT, s.block8 + (AT_CONTEXT * PIXELS8) as u32, 1, 0);
    ops.multiply((s.features8[0], 1, PIXELS8 as u32), (s.features8[1], PIXELS8 as u32, 1), (PIXELS8, FEATURES, PIXELS8), (s.corr8, PIXELS8 as u32));
    ops.zero(CONTEXT * PIXELS8, s.block8 + (AT_MEMORY * PIXELS8) as u32);
    for _ in 0..STEPS {
        refine(ops, &t.refine8, s.corr8, s.block8, S8, &s.work8, root16);
    }
}

/// Works out the movement between each picture and the one before it.
pub struct Flow {
    run: Runner,
    /// Each picture's backbone, and what's left with each picture as the current one: which of
    /// `run`'s dispatches.
    first: [Range<usize>; 2],
    second: [Range<usize>; 2],
    pictures: [Picture; 2],
    flow: u32,
    /// Which picture was last filled in, if the next can be compared with it.
    last: Option<usize>,
    #[cfg(test)]
    shared: Shared,
}

impl Flow {
    pub fn new() -> Result<Self> {
        let wt = Weights { t: read_safetensors(&super::models::load(WEIGHTS)?)? };
        let mut up = Upload::default();
        let mut memory = Mem::default();
        let plane16 = TOKENS;
        let pictures = [0, 1].map(|_| Picture { input: memory.take(3 * HEIGHT * WIDTH), s16: memory.take(D * plane16), s8: memory.take(D * PIXELS8) });

        // The parts that don't change get written once.
        let mut constants: Vec<(u32, Vec<f32>)> = Vec::new();
        for picture in pictures {
            // Position, y then x, measured from the middle.
            let mut pos = vec![0.0; 2 * plane16];
            for y in 0..S16.0 {
                for x in 0..S16.1 {
                    let at = y * S16.1 + x;
                    pos[at] = y as f32 - (S16.0 / 2) as f32;
                    pos[plane16 + at] = x as f32 - (S16.1 / 2) as f32;
                }
            }
            constants.push((picture.s16 + (190 * plane16) as u32, pos));
        }

        let mut before = [Ops::default(), Ops::default()];
        for (ops, picture) in before.iter_mut().zip(pictures) {
            backbone(ops, &mut up, &wt, &mut memory, picture)?;
        }

        let mut layer = |i: usize| -> Result<Layer> {
            let name = format!("cross_attn_s16.layers.{i}");
            Ok(Layer {
                q: linear(&mut up, &wt, &format!("{name}.q_proj"), None)?,
                k: linear(&mut up, &wt, &format!("{name}.k_proj"), None)?,
                v: linear(&mut up, &wt, &format!("{name}.v_proj"), None)?,
                merge: linear(&mut up, &wt, &format!("{name}.merge"), None)?,
                norm1: norm(&mut up, &wt, &format!("{name}.norm1"))?,
                mlp0: linear(&mut up, &wt, &format!("{name}.mlp.0"), None)?,
                mlp1: linear(&mut up, &wt, &format!("{name}.mlp.2"), None)?,
                norm2: norm(&mut up, &wt, &format!("{name}.norm2"))?,
            })
        };
        let layers = [layer(0)?, layer(1)?];
        let tails = Tails {
            layers,
            channels: Norm { g: up.push(&wt.vec("cross_attn_s16.norm.scale")?), b: up.push(&wt.vec("cross_attn_s16.norm.shift")?) },
            merge0: conv(&mut up, &wt, "merge_s8.0")?,
            merge2: conv(&mut up, &wt, "merge_s8.2")?,
            context0: conv(&mut up, &wt, "context_merge_s8.0")?,
            context2: conv(&mut up, &wt, "context_merge_s8.2")?,
            refine16: refine_weights(&mut up, &wt, "refine_s16", 128)?,
            refine8: refine_weights(&mut up, &wt, "refine_s8", 96)?,
        };

        let work = |memory: &mut Mem, inner: usize, plane: usize| -> Vec<u32> {
            let mut v = vec![memory.take(128 * plane)];
            v.extend((0..6).map(|_| memory.take(inner * plane)));
            v.push(memory.take(66 * plane));
            v
        };
        let row = TOKENS * D;
        let shared = Shared {
            tokens: [memory.take(row), memory.take(row)],
            messages: [memory.take(row), memory.take(row)],
            q: memory.take(row),
            k: memory.take(row),
            v: memory.take(row),
            scores: memory.take(TOKENS * TOKENS),
            merged: memory.take(row),
            wide: memory.take(2 * row),
            attended: [memory.take(row), memory.take(row)],
            corr16: memory.take(TOKENS * TOKENS),
            soft: memory.take(TOKENS * TOKENS),
            block16: memory.take(BLOCK * TOKENS),
            work16: work(&mut memory, 128, TOKENS),
            merge_in: [memory.take(2 * FEATURES * PIXELS8), memory.take(2 * FEATURES * PIXELS8)],
            merge_mid: memory.take(FEATURES * PIXELS8),
            features8: [memory.take(FEATURES * PIXELS8), memory.take(FEATURES * PIXELS8)],
            context_in: memory.take(2 * CONTEXT * PIXELS8),
            context_mid: memory.take(CONTEXT * PIXELS8),
            corr8: memory.take(PIXELS8 * PIXELS8),
            block8: memory.take(BLOCK * PIXELS8),
            work8: work(&mut memory, 96, PIXELS8),
        };
        constants.push((shared.block16 + (AT_RADIUS * TOKENS) as u32, vec![RADIUS; TOKENS]));
        constants.push((shared.block8 + (AT_RADIUS * PIXELS8) as u32, vec![RADIUS; PIXELS8]));
        let mut after = [Ops::default(), Ops::default()];
        for (i, ops) in after.iter_mut().enumerate() {
            rest(ops, &shared, &tails, pictures[i], pictures[1 - i]);
        }
        let scratch_len = before
            .iter()
            .chain(&after)
            .flat_map(|o| &o.list)
            .map(|(k, p, _)| match matmul_parts(*k, p) {
                Some(parts) => matmul_scratch(p, parts),
                None if matches!(k, Kernel::Conv3x3 | Kernel::Conv3x3Wide) => Wino::scratch(p.k as usize, p.x0 as usize, (p.m as usize, p.n as usize)),
                None => 0,
            })
            .max()
            .unwrap_or(0);
        let scratch = memory.take(scratch_len);
        let mut done = HashMap::new();

        // All four jobs in one plan, one after the other.
        let mut ops = Ops::default();
        let ([a, b], [c, d]) = (before, after);
        let mut job = |o: Ops| {
            let o = winograd(o, &mut up, &mut done, scratch);
            let start = ops.list.len();
            ops.list.extend(o.list);
            start..ops.list.len()
        };
        let (first, second) = ([job(a), job(b)], [job(c), job(d)]);
        let kernels = Kernels::new::<Params>("flow.wgsl", concat!(include_str!("depth.wgsl"), include_str!("flow.wgsl")), ops.list.iter().map(|o| o.0))?;
        let run = Runner::with(kernels, &up.data, ops, Vec::new(), memory.end, "flow")?;
        for (at, values) in &constants {
            run.write(*at, values);
        }
        let flow = shared.block8 + (AT_FLOW * PIXELS8) as u32;
        Ok(Self { run, first, second, pictures, flow, last: None, #[cfg(test)] shared })
    }

    pub fn adapter(&self) -> &str {
        &self.run.adapter
    }

    /// The next picture won't follow on from the last: don't compare them.
    pub fn reset(&mut self) {
        self.last = None;
    }

    /// Takes a picture of `WIDTH` × `HEIGHT`, as RGB bytes row by row from the top. Returns how far
    /// each part of it has moved since the last picture, or nothing if there wasn't one: two
    /// planes, x then y, each `FLOW_WIDTH` × `FLOW_HEIGHT` from the top. They're in fractions of
    /// the picture, y downwards, and each place says where it's come from, so what's at
    /// (x, y) now was at (x - dx, y - dy).
    pub fn run(&mut self, rgb: &[u8]) -> Result<Option<Vec<f32>>> {
        self.submit(rgb)?;
        if self.last.is_none() {
            self.last = Some(0);
            return Ok(None);
        }
        let mut flow = self.run.read(self.flow, 2 * PIXELS8)?;
        for (i, v) in flow.iter_mut().enumerate() {
            // The network says where things go from this picture to the last.
            *v = -*v / if i < PIXELS8 { FLOW_WIDTH } else { FLOW_HEIGHT } as f32;
        }
        Ok(Some(flow))
    }

    fn submit(&mut self, rgb: &[u8]) -> Result<()> {
        anyhow::ensure!(rgb.len() == WIDTH * HEIGHT * 3, "expected {WIDTH}x{HEIGHT} RGB, got {} bytes", rgb.len());
        let now = self.last.map_or(0, |last| 1 - last);
        let pixels = WIDTH * HEIGHT;
        let mut planes = vec![0.0f32; 3 * pixels];
        // Its weights expect BGR, from 0 to 1 at this point.
        for (i, p) in rgb.as_chunks::<3>().0.iter().enumerate() {
            planes[i] = f32::from(p[2]) / 255.0;
            planes[pixels + i] = f32::from(p[1]) / 255.0;
            planes[2 * pixels + i] = f32::from(p[0]) / 255.0;
        }
        self.run.write(self.pictures[now].input, &planes);
        self.dispatch(&self.first[now]);
        if self.last.is_some() {
            self.dispatch(&self.second[now]);
            self.last = Some(now);
        }
        Ok(())
    }

    /// In parts, so that what else is drawing can get in between, but without waiting for
    /// each: the GPU takes them in order.
    fn dispatch(&self, ops: &Range<usize>) {
        for start in ops.clone().step_by(OPS_PER_SUBMIT) {
            self.run.submit(start..(start + OPS_PER_SUBMIT).min(ops.end));
        }
    }

    /// For tuning: each dispatch's time (ms) in the backbone and the rest, grouped by what it
    /// is.
    #[cfg(test)]
    fn report(&self) {
        for (what, ops) in [("backbone", &self.first[0]), ("the rest", &self.second[0])] {
            eprintln!("{what}:");
            self.run.report_part(ops.clone(), |_, k, p| match k {
                Kernel::FlowConv => format!("conv {}x{} {}->{} at {}x{}", p.a_bs, p.a_bs, p.k, p.x0, p.b_rs, p.b_bs),
                Kernel::Matmul | Kernel::MatmulMem => format!("{k:?} {}x{}x{} batch {}", p.m, p.k, p.n, p.batch),
                Kernel::Im2col => format!("Im2col {}x{} {} at {}x{}", p.a_bs, p.a_bs, p.k, p.b_rs, p.b_bs),
                Kernel::WinoIn => format!("WinoIn {} at {}x{}", p.k, p.n, p.m),
                Kernel::WinoOut => format!("WinoOut {} at {}x{}", p.x0, p.n, p.m),
                k => format!("{k:?}"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compares each stage with PyTorch's (needs a reference file from tools/flow/reference.py, and
    /// a GPU): `FLEX_FLOW_REF=ref.safetensors cargo test -p flex-depth flow_matches_pytorch -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn flow_matches_pytorch() {
        let reference = read_safetensors(&std::fs::read(std::env::var("FLEX_FLOW_REF").expect("FLEX_FLOW_REF")).unwrap()).unwrap();
        let bytes = |name: &str| -> Vec<u8> { reference[name].1.iter().map(|v| *v as u8).collect() };
        let started = std::time::Instant::now();
        let mut net = Flow::new().unwrap();
        eprintln!("loaded in {:?} on {}", started.elapsed(), net.adapter());
        net.run(&bytes("rgb1")).unwrap();
        let flow = net.run(&bytes("rgb0")).unwrap().unwrap();

        let now = net.last.unwrap();
        let s = &net.shared;
        let stages: Vec<(&str, u32)> = vec![
            ("backbone_16", net.pictures[now].s16),
            ("backbone_8", net.pictures[now].s8),
            ("attention", s.attended[0]),
            ("flow_16", s.block16 + (AT_FLOW * TOKENS) as u32),
            ("corr_16", s.block16),
            ("merged", s.features8[0]),
            ("context_8", s.block8 + (AT_CONTEXT * PIXELS8) as u32),
            ("step_1", s.block8 + (AT_FLOW * PIXELS8) as u32),
        ];
        let mut worst = 0.0f32;
        for (name, at) in stages {
            let expected = &reference[name].1;
            let got = net.run.read(at, expected.len()).unwrap();
            let scale = expected.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let err = got.iter().zip(expected).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
            eprintln!("{name:14} max |ref| {scale:10.4}  max error {err:.2e}  relative {:.2e}", err / scale);
            worst = worst.max(err / scale);
        }
        assert!(worst < 2e-3, "worst relative error {worst:.2e}");
        let expected: Vec<f32> = reference["step_1"].1.iter().enumerate().map(|(i, v)| -v / if i < PIXELS8 { FLOW_WIDTH } else { FLOW_HEIGHT } as f32).collect();
        let err = flow.iter().zip(&expected).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(err < 1e-3, "flow out by {err:.2e}");

        // Speed, once warmed up, with the backbone reused from the picture before.
        let runs = 30;
        let (a, b) = (bytes("rgb0"), bytes("rgb1"));
        let started = std::time::Instant::now();
        for i in 0..runs {
            net.run(if i % 2 == 0 { &a } else { &b }).unwrap();
        }
        eprintln!("{:.1} ms a picture ({WIDTH}x{HEIGHT}, {STEPS} steps)", started.elapsed().as_secs_f64() * 1000.0 / f64::from(runs));
        let gpu_only = |net: &Flow| {
            let started = std::time::Instant::now();
            for _ in 0..runs {
                net.dispatch(&net.first[0]);
                net.dispatch(&net.second[0]);
                net.run.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            }
            started.elapsed().as_secs_f64() * 1000.0 / f64::from(runs)
        };
        eprintln!("{:.1} ms just the dispatches", gpu_only(&net));
        net.report();
    }
}
