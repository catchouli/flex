//! MoGe-2 ViT-S (Microsoft, MIT; github.com/microsoft/MoGe): a DINOv2 ViT-S/14 encoder and
//! convolutional heads that give, for each pixel, where it is in 3D (up to a shift in distance
//! that `recover_shift` works out) and which way its surface faces. A port of `MoGeModel.forward`
//! (moge/model/v2.py) for one picture of a fixed size, run by the kernels in depth.wgsl;
//! tools/moge/reference.py makes the reference outputs the tests compare against.
//!
//! Two changes make it cheaper, both checked against the original:
//!
//! - The heads' last level is worked out at half its size: the original doubles the picture and
//!   then convolves it. Everything on that level is linear (a convolution, the neck's features
//!   added, and the output layer), so each head's level becomes two 3x3 convolutions straight to
//!   its three outputs, and the neck's own last level is never needed. Its normals are within a
//!   degree or two of the original's.
//! - The mask head (which says where there's no surface, such as the sky) isn't run: Flex's own
//!   masks say where the sky is. Those come from the head in tools/moge/segment, when it's given.
//!
//! Everything linear between the encoder and the first convolution is folded together too.

use anyhow::{Result, bail};

use crate::{ACCUMULATE, Block, Conv, D, EDGE, Encoder, EncoderMem, Linear, MAX_TOKENS, MaskHead, MaskMem, Mem, NONE, Ops, PATCH, RELU_IN, Runner, Upload, Weights, Wino, norm, patches_of};

/// The model this is a port of, and the version of its weights it was checked against.
pub const MODEL: &str = "Ruicheng/moge-2-vits-normal";
pub const REVISION: &str = "26b477f41595707c5db6770294c0d1721e8ed4ed";
/// `model.pt` at that revision.
pub const WEIGHTS_URL: &str = "https://huggingface.co/Ruicheng/moge-2-vits-normal/resolve/26b477f41595707c5db6770294c0d1721e8ed4ed/model.pt";
pub const WEIGHTS_SHA256: &str = "79a16621928c2bf0ed04659218c55c01075e950507f40bb3332fb4c873d3e1dc";
pub const WEIGHTS_BYTES: u64 = 140_550_416;

/// The head that finds people, walls, sky and more on MoGe-2's features (tools/moge/segment),
/// in Flex's models folder.
pub const MASK_HEAD: &str = "moge-segment.safetensors";

/// The head, from the models folder.
pub fn mask_head() -> Result<Vec<u8>> {
    crate::models::load(MASK_HEAD)
}

/// What its head tells apart: each pixel is one of SCENE; a person's pixels one of PARTS (what
/// they're made of) and one of BODY (which part of the body); and CENTRES, which person.
pub const SCENE: [&str; 13] = ["other", "person", "wall", "sky", "ground", "ceiling", "water", "plant", "window", "vehicle", "animal", "building", "furniture"];
pub const PARTS: [&str; 3] = ["skin", "hair", "clothes"];
pub const BODY: [&str; 6] = ["head", "torso", "arm", "hand", "leg", "foot"];
/// How near each pixel is to a person's centre (1 at the centre), and for a person's pixels the
/// way to their centre, in picture heights (x to the right, y down).
pub const CENTRES: [&str; 3] = ["centre", "to centre x", "to centre y"];
const OUTPUTS: usize = SCENE.len() + PARTS.len() + BODY.len() + CENTRES.len();

/// How likely each pixel is to be each class, as planes of `Moge::out_size()`, rows from the top.
pub struct Masks {
    /// SCENE's classes (each pixel's add up to 1).
    pub scene: Vec<f32>,
    /// PARTS' and BODY's, of a person (each pixel's add up to 1 in each).
    pub parts: Vec<f32>,
    pub body: Vec<f32>,
    /// CENTRES' three planes, as the head gives them.
    pub centres: Vec<f32>,
}

/// Blocks whose (normed) output feeds the neck.
const TAPS: [usize; 2] = [5, 11];
/// Channels at each level: the patch grid, then doubled three times.
const CHANNELS: [usize; 4] = [384, 256, 128, 64];
/// How much bigger than the patch grid the outputs are.
pub const OUT_SCALE: usize = 8;
const HEADS_OUT: [&str; 2] = ["normal_head", "points_head"];

/// Where things are in `mem`.
#[derive(Debug, Default, Clone)]
struct Layout {
    enc: EncoderMem,
    /// The normed outputs of the TAPS blocks, [tokens][D] each (the class token first).
    tokens: [u32; 2],
    /// The neck's features at each level, [CHANNELS[i]][gh << i][gw << i].
    neck: [u32; 4],
    /// A head's features at levels 1 to 3.
    head: [u32; 4],
    up: u32,
    t: u32,
    /// For the Winograd convolutions.
    scratch: u32,
    /// [3][8 gh][8 gw] each.
    normal: u32,
    points: u32,
    /// The masks' head's (as in lib.rs).
    mask: MaskMem,
    end: u32,
}

/// What MoGe-2 makes of a picture, at OUT_SCALE times its patch grid.
pub struct Geometry {
    pub width: usize,
    pub height: usize,
    /// Distance along the view (z), row by row from the top, after `recover_shift`.
    pub depth: Vec<f32>,
    /// Which way each pixel's surface faces, x right, y down, z away from the camera (unit length).
    pub normals: Vec<[f32; 3]>,
    /// The focal length relative to half the picture's diagonal.
    pub focal: f32,
}

pub struct Moge {
    run: Runner,
    s: Layout,
    grid: (usize, usize),
    with_masks: bool,
    mask_classes: usize,
    /// Where each part of the network ends in the dispatches, for tuning.
    #[cfg(test)]
    marks: Vec<(&'static str, usize)>,
}

/// A 3x3 convolution with as many outputs as inputs, one way or the other.
#[derive(Clone, Copy)]
enum Conv3 {
    Wino(Wino),
    Direct(Conv, usize),
}

impl Conv3 {
    /// Where its weights start.
    fn weights(self) -> u32 {
        match self {
            Conv3::Wino(w) => w.u,
            Conv3::Direct(c, _) => c.w,
        }
    }

    fn push(self, ops: &mut Ops, a: u32, size: (usize, usize), c: u32, scratch: u32, flags: u32) {
        match self {
            Conv3::Wino(w) => ops.conv3x3_wino(a, size, w, c, scratch, flags),
            Conv3::Direct(cv, ch) => ops.conv3x3(a, ch, size, cv, ch, c, 1, flags),
        }
    }
}

/// a [n][k] times b [k][m].
fn matmul(a: &[f32], b: &[f32], n: usize, k: usize, m: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; n * m];
    for i in 0..n {
        for t in 0..k {
            let v = a[i * k + t];
            if v == 0.0 {
                continue;
            }
            for (o, w) in out[i * m..(i + 1) * m].iter_mut().zip(&b[t * m..(t + 1) * m]) {
                *o += v * w;
            }
        }
    }
    out
}

/// MoGe's `normalized_view_plane_uv`: u, v at each pixel of a w x h map of a picture with this
/// aspect ratio, as [2][h][w].
fn view_plane_uv((h, w): (usize, usize), aspect: f64) -> Vec<f32> {
    let diagonal = (1.0 + aspect * aspect).sqrt();
    let (sx, sy) = (aspect / diagonal, 1.0 / diagonal);
    let mut uv = vec![0.0f32; 2 * h * w];
    for y in 0..h {
        for x in 0..w {
            uv[y * w + x] = (sx * (2 * x + 1) as f64 / w as f64 - sx) as f32;
            uv[h * w + y * w + x] = (sy * (2 * y + 1) as f64 / h as f64 - sy) as f32;
        }
    }
    uv
}

/// A [inp][out] weight's effect on a per-pixel [inp][pixels] input, plus a bias: [out][pixels].
fn per_pixel(wt: &[f32], bias: &[f32], input: &[f32], inp: usize, out: usize, pixels: usize) -> Vec<f32> {
    let mut c = vec![0.0f32; out * pixels];
    for o in 0..out {
        for p in 0..pixels {
            c[o * pixels + p] = bias[o] + (0..inp).map(|i| wt[i * out + o] * input[i * pixels + p]).sum::<f32>();
        }
    }
    c
}

/// A 3x3 convolution [cout][cin][3][3] followed by the [cout][n] (out x in) matrix `then`:
/// a [n][cin][3][3] convolution.
fn compose3x3(conv: &[f32], cout: usize, cin: usize, then: &[f32], n: usize) -> Vec<f32> {
    let per = cin * 9;
    let mut out = vec![0.0f32; n * per];
    for o in 0..n {
        for c in 0..cout {
            let s = then[o * cout + c];
            for (d, v) in out[o * per..(o + 1) * per].iter_mut().zip(&conv[c * per..(c + 1) * per]) {
                *d += s * v;
            }
        }
    }
    out
}

impl Moge {
    /// Loads MoGe-2 from `model.pt` for pictures `grid` patches wide and high (each patch is 14
    /// pixels), with `mask_head` (tools/moge/segment's safetensors) to find people, walls and sky.
    pub fn load(weights: &std::path::Path, grid: (usize, usize), mask_head: Option<&[u8]>) -> Result<Self> {
        let bytes = std::fs::read(weights).map_err(|e| anyhow::anyhow!("reading {}: {e}", weights.display()))?;
        Self::from_bytes(&bytes, grid, mask_head)
    }

    pub fn from_bytes(bytes: &[u8], (gw, gh): (usize, usize), mask_head: Option<&[u8]>) -> Result<Self> {
        if gw == 0 || gh == 0 {
            bail!("an empty picture");
        }
        let wt = Weights { t: crate::torch_file::read(bytes, Some("model"))? };
        let n = gw * gh;
        let tokens = n + 1;
        if tokens > MAX_TOKENS {
            bail!("{gw} x {gh} patches is too many: room for {MAX_TOKENS} tokens");
        }
        let aspect = gw as f64 / gh as f64;
        let size = |l: usize| (gh << l, gw << l);
        let pixels = |l: usize| (gh << l) * (gw << l);
        let mut up = Upload::default();
        let enc = "encoder.backbone";

        // The position embedding is stretched as DINOv2 does it, by a scale factor a tenth of a
        // patch bigger than the grid.
        let lin = |up: &mut Upload, name: &str, scale: Option<&[f32]>| crate::linear(up, &wt, name, scale);
        let names = ["patch_embed.proj", "pos_embed", "cls_token", "norm"].map(|name| format!("{enc}.{name}"));
        let encoder = Encoder::load(&mut up, &wt, (gw, gh), names.each_ref().map(String::as_str), 0.1, |up, i| {
            let p = format!("{enc}.blocks.{i}");
            let ls1 = wt.vec(&format!("{p}.ls1.gamma"))?;
            let ls2 = wt.vec(&format!("{p}.ls2.gamma"))?;
            Ok(Block {
                n1: norm(up, &wt, &format!("{p}.norm1"))?,
                qkv: lin(up, &format!("{p}.attn.qkv"), None)?,
                out: lin(up, &format!("{p}.attn.proj"), Some(&ls1))?,
                n2: norm(up, &wt, &format!("{p}.norm2"))?,
                fc1: lin(up, &format!("{p}.mlp.fc1"), None)?,
                fc2: lin(up, &format!("{p}.mlp.fc2"), Some(&ls2))?,
            })
        })?;

        // The neck's first level: the two taps projected and summed, with the view plane's u and
        // v beside them, through a 1x1 convolution. All linear, so one matrix from each tap and
        // a per-pixel constant.
        let (_, _, into_neck) = wt.transposed("neck.input_blocks.0.weight", None)?; // [386][384]
        let neck_bias = wt.vec("neck.input_blocks.0.bias")?;
        let from_features = &into_neck[..D * D];
        let from_uv = &into_neck[D * D..];
        let mut level0 = vec![0.0f32; D * n];
        let mut tap_from = Vec::new();
        let mut bias = neck_bias.clone();
        for k in 0..2 {
            let (_, _, proj) = wt.transposed(&format!("encoder.output_projections.{k}.weight"), None)?;
            let pb = wt.vec(&format!("encoder.output_projections.{k}.bias"))?;
            tap_from.push(Linear { w: up.push(&matmul(&proj, from_features, D, D, D)), b: NONE, inp: D, out: D });
            for (b, v) in bias.iter_mut().zip(matmul(&pb, from_features, 1, D, D)) {
                *b += v;
            }
        }
        let uv0 = view_plane_uv(size(0), aspect);
        level0.copy_from_slice(&per_pixel(from_uv, &bias, &uv0, 2, D, n));
        let level0_at = up.push(&level0);

        // 3x3 convolutions with as many outputs as inputs: by Winograd where that's quicker
        // (on the RX 580, with 128 channels or more).
        let conv = |up: &mut Upload, name: &str| -> Result<Conv3> {
            let (shape, v) = wt.get(&format!("{name}.weight"))?;
            Ok(if shape[0] >= 128 {
                Conv3::Wino(crate::wino(up, v, Some(&wt.get(&format!("{name}.bias"))?.1), shape[0], shape[1]))
            } else {
                Conv3::Direct(crate::conv(up, &wt, name)?, shape[0])
            })
        };
        // A transposed convolution's four taps as [in][out] matrices, each first multiplied by
        // `before` ([in0][in]) with `before_bias` if given.
        let taps = |up: &mut Upload, name: &str, before: Option<(&[f32], &[f32])>| -> Result<[Linear; 4]> {
            let (shape, v) = wt.get(&format!("{name}.weight"))?;
            let (cin, cout) = (shape[0], shape[1]);
            let b = wt.vec(&format!("{name}.bias"))?;
            let mut out = Vec::new();
            for t in 0..4 {
                let mut m = vec![0.0f32; cin * cout];
                for i in 0..cin {
                    for o in 0..cout {
                        m[i * cout + o] = v[(i * cout + o) * 4 + t];
                    }
                }
                let (m, bias) = match before {
                    Some((w0, b0)) => {
                        let inp = w0.len() / cin;
                        let mut bias = matmul(b0, &m, 1, cin, cout);
                        bias.iter_mut().zip(&b).for_each(|(a, b)| *a += b);
                        (matmul(w0, &m, inp, cin, cout), bias)
                    }
                    None => (m, b.clone()),
                };
                out.push(Linear { w: up.push(&m), b: up.push(&bias), inp: m.len() / cout, out: cout });
            }
            Ok([out[0], out[1], out[2], out[3]])
        };
        struct Level {
            /// Doubling from the level below.
            taps: [Linear; 4],
            conv: Conv3,
            /// The neck's u, v constant, or a head's 1x1 convolution of the neck's features.
            input: Result<u32, Linear>,
            res: (Conv3, Conv3),
        }
        struct Stack {
            levels: Vec<Level>,
            /// The last level, folded: from this stack's level 3, and (heads) from the neck's.
            own: Conv,
            from_neck: Conv,
            constant: u32,
        }
        let neck_stack = |up: &mut Upload| -> Result<Vec<Level>> {
            let mut levels = Vec::new();
            #[allow(clippy::needless_range_loop)] // i names the weights and sizes too
            for i in 1..4 {
                let (_, _, w_uv) = wt.transposed(&format!("neck.input_blocks.{i}.weight"), None)?;
                let b = wt.vec(&format!("neck.input_blocks.{i}.bias"))?;
                let constant = per_pixel(&w_uv, &b, &view_plane_uv(size(i), aspect), 2, CHANNELS[i], pixels(i));
                levels.push(Level {
                    taps: taps(up, &format!("neck.resamplers.{}.0", i - 1), None)?,
                    conv: conv(up, &format!("neck.resamplers.{}.1", i - 1))?,
                    input: Ok(up.push(&constant)),
                    res: (conv(up, &format!("neck.res_blocks.{i}.0.layers.2"))?, conv(up, &format!("neck.res_blocks.{i}.0.layers.5"))?),
                });
            }
            Ok(levels)
        };
        let neck = neck_stack(&mut up)?;
        // The neck's last level as the heads see it: N (the doubling's 3x3, at half size here)
        // plus the u, v constant at that size.
        let (neck_last, neck_last_b) = (wt.vec("neck.resamplers.3.1.weight")?, wt.vec("neck.resamplers.3.1.bias")?);
        let (_, _, w_uv4) = wt.transposed("neck.input_blocks.4.weight", None)?;
        let neck4_const = per_pixel(&w_uv4, &wt.vec("neck.input_blocks.4.bias")?, &view_plane_uv(size(3), aspect), 2, 32, pixels(3));
        let mut heads = Vec::new();
        for h in HEADS_OUT {
            let (_, _, in0) = wt.transposed(&format!("{h}.input_blocks.0.weight"), None)?;
            let in0_b = wt.vec(&format!("{h}.input_blocks.0.bias"))?;
            let mut levels = Vec::new();
            #[allow(clippy::needless_range_loop)] // i names the weights and sizes too
            for i in 1..4 {
                levels.push(Level {
                    taps: if i == 1 { taps(&mut up, &format!("{h}.resamplers.0.0"), Some((&in0, &in0_b)))? } else { taps(&mut up, &format!("{h}.resamplers.{}.0", i - 1), None)? },
                    conv: conv(&mut up, &format!("{h}.resamplers.{}.1", i - 1))?,
                    input: Err(lin(&mut up, &format!("{h}.input_blocks.{i}"), None)?),
                    res: (conv(&mut up, &format!("{h}.res_blocks.{i}.0.layers.2"))?, conv(&mut up, &format!("{h}.res_blocks.{i}.0.layers.5"))?),
                });
            }
            // out = O (R(x3) + bR + I4 (N(neck3) + bN + c4) + bI4) + bO.
            let o = wt.vec(&format!("{h}.output_blocks.4.weight"))?; // [3][32]
            let bo = wt.vec(&format!("{h}.output_blocks.4.bias"))?;
            let i4 = wt.vec(&format!("{h}.input_blocks.4.weight"))?; // [32][32]
            let bi4 = wt.vec(&format!("{h}.input_blocks.4.bias"))?;
            let (r, br) = (wt.vec(&format!("{h}.resamplers.3.1.weight"))?, wt.vec(&format!("{h}.resamplers.3.1.bias"))?);
            let o_i4 = matmul(&o, &i4, 3, 32, 32); // [3][32]
            let own = compose3x3(&r, 32, 64, &o, 3);
            let from_neck = compose3x3(&neck_last, 32, 64, &o_i4, 3);
            let p3 = pixels(3);
            let mut constant = vec![0.0f32; 3 * p3];
            for k in 0..3 {
                let fixed = bo[k] + (0..32).map(|c| o[k * 32 + c] * (br[c] + bi4[c]) + o_i4[k * 32 + c] * neck_last_b[c]).sum::<f32>();
                for p in 0..p3 {
                    constant[k * p3 + p] = fixed + (0..32).map(|c| o_i4[k * 32 + c] * neck4_const[c * p3 + p]).sum::<f32>();
                }
            }
            heads.push(Stack {
                levels,
                own: Conv { w: up.push(&own), b: NONE },
                from_neck: Conv { w: up.push(&from_neck), b: NONE },
                constant: up.push(&constant),
            });
        }

        let mask_head = match mask_head {
            Some(bytes) => {
                let head = MaskHead::load(&mut up, bytes)?;
                if head.classes != OUTPUTS {
                    bail!("a mask head with {} outputs, not {OUTPUTS}", head.classes);
                }
                Some(head)
            }
            None => None,
        };
        let mask_classes = mask_head.as_ref().map_or(0, |h| h.classes);

        // Memory.
        let mut m = Mem::default();
        let mut s = Layout { enc: encoder.mem(&mut m, n), ..Default::default() };
        s.tokens = [m.take(tokens * D), m.take(tokens * D)];
        #[allow(clippy::needless_range_loop)] // i gives the size too
        for i in 0..4 {
            s.neck[i] = m.take(CHANNELS[i] * pixels(i));
            // Both heads' (they run together), one after the other.
            s.head[i] = if i == 0 { 0 } else { m.take(2 * CHANNELS[i] * pixels(i)) };
        }
        let biggest = (1..4).map(|i| CHANNELS[i] * pixels(i)).max().unwrap_or(0);
        let scratch = (1..3).map(|i| Wino::scratch(CHANNELS[i], CHANNELS[i], size(i))).max().unwrap_or(0);
        s.up = m.take(2 * biggest);
        s.t = m.take(2 * biggest);
        s.scratch = m.take(2 * scratch);
        s.normal = m.take(3 * pixels(3));
        s.points = m.take(3 * pixels(3));
        if let Some(h) = &mask_head {
            s.mask = h.mem(&mut m, (gh, gw));
        }
        s.end = m.end;

        // The forward pass: the encoder, as for Depth Anything.
        let mut ops = Ops::default();
        encoder.ops(&mut ops, &s.enc, n, &TAPS, &s.tokens);
        let mut marks = vec![("encoder", ops.list.len())];
        // The neck's first level.
        ops.copy(level0_at, D * n, s.neck[0], 0);
        for (k, l) in tap_from.iter().enumerate() {
            ops.tokens_to_image(s.tokens[k] + D as u32, n, *l, s.neck[0], ACCUMULATE);
        }
        // Up a level: doubled by the taps into `up`, then x = 3x3(up) + input, then a residual
        // unit x += 3x3(relu(3x3(relu(x)))).
        let level = |ops: &mut Ops, l: &Level, i: usize, from: u32, x: u32| {
            let sz = size(i);
            ops.conv_transpose2(from, size(i - 1), l.taps, s.up, 0);
            match l.input {
                Ok(constant) => ops.copy(constant, CHANNELS[i] * pixels(i), x, 0),
                Err(lin) => ops.conv1x1(s.neck[i], pixels(i), lin, x, 0),
            }
            l.conv.push(ops, s.up, sz, x, s.scratch, EDGE | ACCUMULATE);
            l.res.0.push(ops, x, sz, s.t, s.scratch, EDGE | RELU_IN);
            l.res.1.push(ops, s.t, sz, x, s.scratch, EDGE | RELU_IN | ACCUMULATE);
        };
        for (k, l) in neck.iter().enumerate() {
            level(&mut ops, l, k + 1, s.neck[k], s.neck[k + 1]);
        }
        marks.push(("neck", ops.list.len()));
        // People, walls and sky, from the taps and the neck's level 3 (as tools/moge/segment).
        if let Some(h) = &mask_head {
            h.ops(&mut ops, &s.mask, (gh, gw), s.tokens, s.neck[3]);
            marks.push(("masks", ops.list.len()));
        }
        // The two heads together: each dispatch is the normal head's, run a second time for the
        // points head with its weights, input and output that far along. The heads' weights were
        // loaded one head after the other in the same order, so they're all the same distance on.
        let w_step = heads[1].own.w - heads[0].own.w;
        assert!(heads[1].constant - heads[0].constant == w_step && heads[1].levels[2].res.1.weights() - heads[0].levels[2].res.1.weights() == w_step);
        let (u, step) = (biggest as u32, scratch as u32);
        let conv_twice = |ops: &mut Ops, cv: Conv3, a: u32, a_step: u32, sz: (usize, usize), c: u32, c_step: u32, flags: u32| {
            cv.push(ops, a, sz, c, s.scratch, flags);
            match cv {
                Conv3::Wino(_) => ops.repeat_last(3, 2, &[(a_step, 0, step), (step, w_step, step), (step, w_step, c_step)]),
                Conv3::Direct(..) => ops.repeat_last(1, 2, &[(a_step, w_step, c_step)]),
            }
        };
        for (k, l) in heads[0].levels.iter().enumerate() {
            let i = k + 1;
            let (sz, here) = (size(i), (CHANNELS[i] * pixels(i)) as u32);
            let (from, from_step) = if k == 0 { (s.neck[0], 0) } else { (s.head[k], (CHANNELS[k] * pixels(k)) as u32) };
            let x = s.head[i];
            ops.conv_transpose2(from, size(k), l.taps, s.up, 0);
            ops.repeat_last(4, 2, &[(from_step, w_step, u); 4]);
            let Err(input) = l.input else { unreachable!() };
            ops.conv1x1(s.neck[i], pixels(i), input, x, 0);
            ops.repeat_last(1, 2, &[(0, w_step, here)]);
            conv_twice(&mut ops, l.conv, s.up, u, sz, x, here, EDGE | ACCUMULATE);
            conv_twice(&mut ops, l.res.0, x, here, sz, s.t, u, EDGE | RELU_IN);
            conv_twice(&mut ops, l.res.1, s.t, u, sz, x, here, EDGE | RELU_IN | ACCUMULATE);
        }
        let out_step = (3 * pixels(3)) as u32;
        for (h, out) in heads.iter().zip([s.normal, s.points]) {
            ops.copy(h.constant, 3 * pixels(3), out, 0);
        }
        let h = &heads[0];
        ops.conv3x3(s.head[3], CHANNELS[3], size(3), h.own, 3, s.normal, 1, EDGE | ACCUMULATE);
        ops.repeat_last(1, 2, &[((CHANNELS[3] * pixels(3)) as u32, w_step, out_step)]);
        ops.conv3x3(s.neck[3], CHANNELS[3], size(3), h.from_neck, 3, s.normal, 1, EDGE | ACCUMULATE);
        ops.repeat_last(1, 2, &[(0, w_step, out_step)]);

        marks.push(("both heads", ops.list.len()));
        let with_masks = mask_head.is_some();
        let run = Runner::new(&up, ops, s.end, "MoGe-2")?;
        Ok(Self { run, s, grid: (gw, gh), with_masks, mask_classes, #[cfg(test)] marks })
    }

    /// The picture size it takes, in pixels.
    pub fn size(&self) -> (usize, usize) {
        (self.grid.0 * PATCH, self.grid.1 * PATCH)
    }

    /// The size of what it gives back.
    pub fn out_size(&self) -> (usize, usize) {
        (self.grid.0 * OUT_SCALE, self.grid.1 * OUT_SCALE)
    }

    pub fn adapter(&self) -> &str {
        &self.run.adapter
    }

    /// Runs on a picture of exactly `size()`, as RGB bytes row by row from the top, leaving the
    /// results on the GPU for `geometry`.
    pub fn submit(&mut self, rgb: &[u8]) -> Result<()> {
        let patches = patches_of(rgb, self.grid)?;
        self.run.write(self.s.enc.patches, &patches);
        self.run.run()
    }

    /// The raw outputs: normals and points (x, y, log z) as [3][h][w] planes.
    pub fn raw(&self) -> Result<(Vec<f32>, Vec<f32>)> {
        let (w, h) = self.out_size();
        Ok((self.run.read(self.s.normal, 3 * w * h)?, self.run.read(self.s.points, 3 * w * h)?))
    }

    /// Whether it finds people, walls and sky.
    pub fn has_masks(&self) -> bool {
        self.with_masks
    }

    /// After `submit`, how likely each pixel is to be each class.
    pub fn masks(&self) -> Result<Masks> {
        if !self.with_masks {
            bail!("loaded without masks");
        }
        let (w, h) = self.out_size();
        let pixels = w * h;
        let mut logits = self.run.read(self.s.mask.logits, self.mask_classes * pixels)?;
        let centres = logits.split_off((OUTPUTS - CENTRES.len()) * pixels);
        let body = crate::softmax_planes(logits.split_off((SCENE.len() + PARTS.len()) * pixels), pixels);
        let parts = crate::softmax_planes(logits.split_off(SCENE.len() * pixels), pixels);
        Ok(Masks { scene: crate::softmax_planes(logits, pixels), parts, body, centres })
    }

    /// After `submit`, the picture's depth and normals.
    pub fn geometry(&self) -> Result<Geometry> {
        let (w, h) = self.out_size();
        let (normal, points) = self.raw()?;
        let pixels = w * h;
        let normals = (0..pixels)
            .map(|i| {
                let v = [normal[i], normal[pixels + i], normal[2 * pixels + i]];
                let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
                [v[0] / len, v[1] / len, v[2] / len]
            })
            .collect();
        let xyz: Vec<[f32; 3]> = (0..pixels)
            .map(|i| {
                let z = points[2 * pixels + i].exp();
                [points[i] * z, points[pixels + i] * z, z]
            })
            .collect();
        let (shift, focal) = recover_shift(&xyz, (w, h));
        Ok(Geometry { width: w, height: h, depth: xyz.iter().map(|p| p[2] + shift).collect(), normals, focal })
    }
}

/// MoGe's `recover_focal_shift`: the shift in z (and the focal length, relative to half the
/// diagonal) that best makes the points line up with the pixels they're at, from a sample of
/// about 64 x 64 of them. Least squares in the shift, by Gauss-Newton with damping, the focal
/// length following from it in closed form.
pub fn recover_shift(xyz: &[[f32; 3]], (w, h): (usize, usize)) -> (f32, f32) {
    let uv = view_plane_uv((h, w), w as f64 / h as f64);
    let (sy, sx) = (h.div_ceil(64).max(1), w.div_ceil(64).max(1));
    let mut samples: Vec<([f64; 2], [f64; 3])> = Vec::new();
    for y in (sy / 2..h).step_by(sy) {
        for x in (sx / 2..w).step_by(sx) {
            let i = y * w + x;
            let p = xyz[i];
            if p.iter().all(|v| v.is_finite()) {
                samples.push(([f64::from(uv[i]), f64::from(uv[h * w + i])], [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]));
            }
        }
    }
    let residuals = |shift: f64, out: &mut Vec<f64>| -> f64 {
        let (mut num, mut den) = (0.0, 0.0);
        for (uv, p) in &samples {
            let z = (p[2] + shift).max(1e-6);
            let (px, py) = (p[0] / z, p[1] / z);
            num += px * uv[0] + py * uv[1];
            den += px * px + py * py;
        }
        let f = num / den.max(1e-12);
        out.clear();
        for (uv, p) in &samples {
            let z = (p[2] + shift).max(1e-6);
            out.push(f * p[0] / z - uv[0]);
            out.push(f * p[1] / z - uv[1]);
        }
        f
    };
    let cost = |r: &[f64]| r.iter().map(|v| v * v).sum::<f64>();
    let (mut shift, mut lambda) = (0.0f64, 1e-3);
    let (mut r, mut r2) = (Vec::new(), Vec::new());
    residuals(shift, &mut r);
    let mut c = cost(&r);
    for _ in 0..50 {
        let step = 1e-6 * (1.0 + shift.abs());
        residuals(shift + step, &mut r2);
        let jac: Vec<f64> = r2.iter().zip(&r).map(|(a, b)| (a - b) / step).collect();
        let jj: f64 = jac.iter().map(|j| j * j).sum();
        let jr: f64 = jac.iter().zip(&r).map(|(j, r)| j * r).sum();
        let delta = -jr / (jj * (1.0 + lambda)).max(1e-30);
        residuals(shift + delta, &mut r2);
        let c2 = cost(&r2);
        if c2 < c {
            shift += delta;
            std::mem::swap(&mut r, &mut r2);
            let done = (c - c2) < 1e-3 * c;
            c = c2;
            lambda *= 0.3;
            if done {
                break;
            }
        } else {
            lambda *= 10.0;
        }
    }
    let focal = residuals(shift, &mut r);
    (shift as f32, focal as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read_safetensors;

    #[test]
    fn view_plane_uv_matches_moge() {
        // normalized_view_plane_uv(width=4, height=2): u from -0.6708 to 0.6708, v ±0.2236.
        let uv = view_plane_uv((2, 4), 2.0);
        let expected_u = [-0.670_820_4, -0.223_606_8, 0.223_606_8, 0.670_820_4];
        for (x, e) in expected_u.iter().enumerate() {
            assert!((uv[x] - e).abs() < 1e-6, "{uv:?}");
        }
        assert!((uv[8] + 0.223_606_8).abs() < 1e-6 && (uv[12] - 0.223_606_8).abs() < 1e-6, "{uv:?}");
    }

    #[test]
    fn recovers_a_known_shift() {
        // Points of a slanted plane seen through a lens of focal 1.3, then shifted by -2 in z.
        let (w, h) = (40, 24);
        let uv = view_plane_uv((h, w), w as f64 / h as f64);
        let xyz: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (u, v) = (uv[i], uv[w * h + i]);
                let z = 5.0 + 2.0 * v + u;
                [u * z / 1.3, v * z / 1.3, z - 2.0]
            })
            .collect();
        let (shift, focal) = recover_shift(&xyz, (w, h));
        assert!((shift - 2.0).abs() < 1e-3 && (focal - 1.3).abs() < 1e-3, "shift {shift} focal {focal}");
    }

    /// Compares every stage with PyTorch's (needs the weights, a GPU, and a file from
    /// tools/moge/reference.py):
    /// `FLEX_MOGE_WEIGHTS=…/model.pt FLEX_MOGE_REF=ref.safetensors cargo test --release -p flex-depth moge_matches -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn moge_matches_pytorch() {
        let weights = std::env::var("FLEX_MOGE_WEIGHTS").expect("FLEX_MOGE_WEIGHTS");
        let reference = read_safetensors(&std::fs::read(std::env::var("FLEX_MOGE_REF").expect("FLEX_MOGE_REF")).unwrap()).unwrap();
        let (shape, rgb) = &reference["rgb"];
        let (h, w) = (shape[0], shape[1]);
        let rgb: Vec<u8> = rgb.iter().map(|v| *v as u8).collect();
        let started = std::time::Instant::now();
        let head = std::env::var("FLEX_MOGE_HEAD").ok().map(|p| std::fs::read(p).unwrap());
        let mut moge = Moge::load(std::path::Path::new(&weights), (w / PATCH, h / PATCH), head.as_deref()).unwrap();
        eprintln!("loaded in {:?} on {}", started.elapsed(), moge.adapter());
        moge.submit(&rgb).unwrap();
        let s = moge.s.clone();
        let mut stages: Vec<(String, u32)> = vec![(format!("tokens.{}", TAPS[0]), s.tokens[0]), (format!("tokens.{}", TAPS[1]), s.tokens[1])];
        for i in 0..4 {
            stages.push((format!("neck.{i}"), s.neck[i]));
        }
        if moge.with_masks {
            stages.push(("logits".into(), s.mask.logits));
        }
        stages.push(("normal.half".into(), s.normal));
        stages.push(("points.half".into(), s.points));
        let mut worst = 0.0f32;
        for (name, at) in stages {
            let expected = &reference[&name].1;
            let got = moge.run.read(at, expected.len()).unwrap();
            let scale = expected.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let err = got.iter().zip(expected).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
            eprintln!("{name:12} max |ref| {scale:10.4}  max error {err:.2e}  relative {:.2e}", err / scale);
            worst = worst.max(err / scale);
        }
        assert!(worst < 2e-3, "worst relative error {worst:.2e}");
        let g = moge.geometry().unwrap();
        eprintln!("focal {:.3}, depth from {:.3} to {:.3}", g.focal, g.depth.iter().fold(f32::MAX, |a, b| a.min(*b)), g.depth.iter().fold(f32::MIN, |a, b| a.max(*b)));

        let runs = 10;
        let started = std::time::Instant::now();
        for _ in 0..runs {
            moge.submit(&rgb).unwrap();
            moge.raw().unwrap();
        }
        eprintln!("{:.1} ms a picture ({w}x{h})", started.elapsed().as_secs_f64() * 1000.0 / runs as f64);
        let started = std::time::Instant::now();
        for _ in 0..runs {
            moge.run.run().unwrap();
        }
        eprintln!("{:.1} ms just the dispatches, in {} submissions", started.elapsed().as_secs_f64() * 1000.0 / runs as f64, moge.run.parts.len());
        let started = std::time::Instant::now();
        for _ in 0..runs {
            moge.raw().unwrap();
        }
        eprintln!("{:.1} ms reading back", started.elapsed().as_secs_f64() * 1000.0 / runs as f64);
        let started = std::time::Instant::now();
        for _ in 0..runs {
            patches_of(&rgb, moge.grid).unwrap();
        }
        eprintln!("{:.1} ms cutting patches", started.elapsed().as_secs_f64() * 1000.0 / runs as f64);
        let mut before = 0.0;
        for (name, n) in moge.marks.clone() {
            let ms = moge.run.time_first(n, runs);
            eprintln!("{name:12} {:6.1} ms (to here {ms:.1})", ms - before);
            before = ms;
        }
        let (one, two, three) = (moge.run.time_repeated(1, runs), moge.run.time_repeated(2, runs), moge.run.time_repeated(3, runs));
        eprintln!("one pass {one:.1} ms, each dispatch twice {two:.1}, three times {three:.1}: cold {:.1} ms, hot {:.1}", one - (three - two), three - two);
        if std::env::var("FLEX_PROFILE").is_ok() {
            moge.run.report(|i, k, p| match k {
                crate::Kernel::Conv3x3 => format!("conv3x3 {}->{} at {}x{}", p.k, p.x0, p.n, p.m),
                crate::Kernel::Matmul | crate::Kernel::MatmulMem => format!("{k:?} {}x{}x{} batch {}{}", p.m, p.k, p.n, p.batch, if i < 200 { " (encoder)" } else { "" }),
                _ => format!("{k:?} {} {}", p.m, p.k),
            });
        }
    }
}
