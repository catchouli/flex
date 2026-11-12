//! Magenta's arbitrary image stylization (Ghiasi, Lee, Kudlur, Dumoulin and Shlens, "Exploring
//! the structure of a real-time, arbitrary neural artistic stylization network", 2017; Apache-2.0
//! licence), as Kaggle Models has it (google/arbitrary-image-stylization-v1, TensorFlow 1, 256,
//! version 2): a SavedModel, read by `saved_model`.
//!
//! Two networks. The style network (Inception v3 as far as Mixed_6e, then the mean over the
//! picture) turns a picture of a style, 256 x 256, into 768 numbers; it runs once a style, here
//! on the CPU (`StyleNet`). Those numbers give, through a 1x1 convolution each (`Projection`), the
//! scales and shifts of every instance norm in the painting network, which paints each frame
//! (`animegan::AnimeGan`, `Kind::Magenta`, on the GPU). The scales and shifts are linear in the
//! numbers, so styles can be mixed by mixing their numbers. `tools/magenta/reference.py` makes the
//! reference the tests compare against.

use std::collections::HashMap;

use anyhow::{Context, Result, ensure};

use crate::Tensors;

/// The weights Flex downloads, and where from.
pub const MODEL: crate::animegan::Style = crate::animegan::Style {
    id: "magenta",
    label: "Magenta stylization",
    file: "magenta-arbitrary-image-stylization-v1-256-2.tar.gz",
    url: "https://www.kaggle.com/api/v1/models/google/arbitrary-image-stylization-v1/tensorFlow1/256/2/download",
    sha256: "a2509e326d792cab069aceb670ad6fd63fb10218f1ad08bf2f4355d1327bb5b3",
    bytes: 85_816_576,
    credit: CREDIT,
    // Measured on an RX 580: 49 ms at 512x216, against Hayao's 26 at 656x272.
    work: 3.1,
};

pub const CREDIT: &str = "Magenta's arbitrary image stylization (Ghiasi et al.), Apache-2.0 licence";

/// A style is this many numbers.
pub const STYLE: usize = 768;
/// Style pictures are this wide and high.
pub const SIDE: usize = 256;
/// The painting network's residual blocks.
pub const BLOCKS: usize = 20;

/// The SavedModel's float variables, from the .tar.gz.
pub fn read(bytes: &[u8]) -> Result<Tensors> {
    crate::saved_model::read(bytes)
}

fn get<'a>(t: &'a Tensors, name: &str) -> Result<&'a (Vec<usize>, Vec<f32>)> {
    t.get(name).with_context(|| format!("not Magenta's stylization network: no {name}"))
}

/// A convolution's weights [kh][kw][cin][cout] (TensorFlow's) as [cout][cin][kh][kw], scaled for
/// each output by `scale`.
pub(crate) fn channels_first(dims: &[usize], v: &[f32], scale: &[f32]) -> Vec<f32> {
    let (kh, kw, cin, cout) = (dims[0], dims[1], dims[2], dims[3]);
    let mut out = vec![0.0; v.len()];
    for y in 0..kh {
        for x in 0..kw {
            for i in 0..cin {
                for o in 0..cout {
                    out[((o * cin + i) * kh + y) * kw + x] = v[((y * kw + x) * cin + i) * cout + o] * scale[o];
                }
            }
        }
    }
    out
}

/// A convolution followed by a batch norm with its running statistics (and no scale of its own),
/// as one: the weights (channels first, see `channels_first`) and bias.
pub(crate) fn folded(t: &Tensors, name: &str) -> Result<(Vec<usize>, Vec<f32>, Vec<f32>)> {
    let (dims, w) = get(t, &format!("{name}/weights"))?;
    ensure!(dims.len() == 4, "{name}/weights is {dims:?}");
    let cout = dims[3];
    let beta = &get(t, &format!("{name}/BatchNorm/beta"))?.1;
    let mean = &get(t, &format!("{name}/BatchNorm/moving_mean"))?.1;
    let var = &get(t, &format!("{name}/BatchNorm/moving_variance"))?.1;
    ensure!(beta.len() == cout && mean.len() == cout && var.len() == cout, "{name}'s batch norm isn't {cout} channels");
    let scale: Vec<f32> = var.iter().map(|v| 1.0 / (v + BN_EPS).sqrt()).collect();
    let bias = (0..cout).map(|o| beta[o] - mean[o] * scale[o]).collect();
    Ok((dims.clone(), channels_first(dims, w, &scale), bias))
}

/// The batch norms' epsilon (TF-Slim's for Inception, and the painting network's first three).
const BN_EPS: f32 = 1e-3;

// ---- The style network --------------------------------------------------------------------

/// A convolution with its batch norm folded in, then a ReLU6.
struct Layer {
    kh: usize,
    kw: usize,
    cin: usize,
    cout: usize,
    w: Vec<f32>,
    b: Vec<f32>,
}

/// A step along one branch of Inception.
enum Op {
    /// A convolution: its name, stride, and whether it's padded to keep the size (TensorFlow's
    /// "SAME") or not ("VALID").
    Conv(String, usize, bool),
    /// 3x3, stride 1, padded; the average of the pixels inside the picture.
    Avg,
    /// 3x3, stride 2, not padded.
    Max,
}

/// Inception v3 (TF-Slim's) as far as Mixed_6e: the stem, then blocks of branches side by side,
/// joined channel after channel.
fn inception() -> Vec<Vec<Vec<Op>>> {
    let conv = |n: &str, s: usize, same: bool| Op::Conv(format!("InceptionV3/{n}"), s, same);
    let same = |n: String| Op::Conv(format!("InceptionV3/{n}"), 1, true);
    let mut blocks = vec![vec![vec![
        conv("Conv2d_1a_3x3", 2, false),
        conv("Conv2d_2a_3x3", 1, false),
        conv("Conv2d_2b_3x3", 1, true),
        Op::Max,
        conv("Conv2d_3b_1x1", 1, false),
        conv("Conv2d_4a_3x3", 1, false),
        Op::Max,
    ]]];
    for m in ["5b", "5c", "5d"] {
        // Mixed_5c's middle branch has odd names (as TF-Slim has always had them).
        let (b1a, b1b) = if m == "5c" { ("Conv2d_0b_1x1", "Conv_1_0c_5x5") } else { ("Conv2d_0a_1x1", "Conv2d_0b_5x5") };
        let at = |b: u32, n: &str| same(format!("Mixed_{m}/Branch_{b}/{n}"));
        blocks.push(vec![
            vec![at(0, "Conv2d_0a_1x1")],
            vec![at(1, b1a), at(1, b1b)],
            vec![at(2, "Conv2d_0a_1x1"), at(2, "Conv2d_0b_3x3"), at(2, "Conv2d_0c_3x3")],
            vec![Op::Avg, at(3, "Conv2d_0b_1x1")],
        ]);
    }
    blocks.push(vec![
        vec![conv("Mixed_6a/Branch_0/Conv2d_1a_1x1", 2, false)],
        vec![same("Mixed_6a/Branch_1/Conv2d_0a_1x1".into()), same("Mixed_6a/Branch_1/Conv2d_0b_3x3".into()), conv("Mixed_6a/Branch_1/Conv2d_1a_1x1", 2, false)],
        vec![Op::Max],
    ]);
    for m in ["6b", "6c", "6d", "6e"] {
        let at = |b: u32, n: &str| same(format!("Mixed_{m}/Branch_{b}/{n}"));
        blocks.push(vec![
            vec![at(0, "Conv2d_0a_1x1")],
            vec![at(1, "Conv2d_0a_1x1"), at(1, "Conv2d_0b_1x7"), at(1, "Conv2d_0c_7x1")],
            vec![at(2, "Conv2d_0a_1x1"), at(2, "Conv2d_0b_7x1"), at(2, "Conv2d_0c_1x7"), at(2, "Conv2d_0d_7x1"), at(2, "Conv2d_0e_1x7")],
            vec![Op::Avg, at(3, "Conv2d_0b_1x1")],
        ]);
    }
    blocks
}

/// [c][h][w] planes and their size.
#[derive(Clone)]
struct Planes {
    c: usize,
    h: usize,
    w: usize,
    v: Vec<f32>,
}

/// TensorFlow's padding: the size out, and the padding before (top or left).
fn padding(size: usize, k: usize, stride: usize, same: bool) -> (usize, usize) {
    if same {
        let out = size.div_ceil(stride);
        let total = ((out - 1) * stride + k).saturating_sub(size);
        (out, total / 2)
    } else {
        ((size - k) / stride + 1, 0)
    }
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).min(16)
}

impl Layer {
    /// The convolution, then ReLU6, with the work shared out over the CPU's cores by output
    /// channel.
    fn run(&self, x: &Planes, stride: usize, same: bool) -> Planes {
        assert_eq!(x.c, self.cin);
        let (oh, top) = padding(x.h, self.kh, stride, same);
        let (ow, left) = padding(x.w, self.kw, stride, same);
        let mut out = vec![0.0f32; self.cout * oh * ow];
        let per = self.cout.div_ceil(threads());
        std::thread::scope(|s| {
            for (chunk, outs) in out.chunks_mut(per * oh * ow).enumerate() {
                s.spawn(move || {
                    for (j, o) in outs.chunks_mut(oh * ow).enumerate() {
                        let co = chunk * per + j;
                        o.fill(self.b[co]);
                        for ci in 0..self.cin {
                            let plane = &x.v[ci * x.h * x.w..(ci + 1) * x.h * x.w];
                            for ky in 0..self.kh {
                                for kx in 0..self.kw {
                                    let wv = self.w[((co * self.cin + ci) * self.kh + ky) * self.kw + kx];
                                    // Outputs whose input column is inside: ox * stride + kx - left in 0..w.
                                    let lo = left.saturating_sub(kx).div_ceil(stride);
                                    let hi = ((x.w + left).saturating_sub(kx + 1) / stride + 1).min(ow);
                                    if lo >= hi {
                                        continue;
                                    }
                                    for oy in 0..oh {
                                        let iy = (oy * stride + ky) as isize - top as isize;
                                        if iy < 0 || iy >= x.h as isize {
                                            continue;
                                        }
                                        let row = &plane[iy as usize * x.w..];
                                        let orow = &mut o[oy * ow..(oy + 1) * ow];
                                        if stride == 1 {
                                            let start = lo + kx - left;
                                            for (a, b) in orow[lo..hi].iter_mut().zip(&row[start..start + hi - lo]) {
                                                *a += wv * b;
                                            }
                                        } else {
                                            for (ox, a) in orow.iter_mut().enumerate().take(hi).skip(lo) {
                                                *a += wv * row[ox * stride + kx - left];
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        for v in o.iter_mut() {
                            *v = v.clamp(0.0, 6.0);
                        }
                    }
                });
            }
        });
        Planes { c: self.cout, h: oh, w: ow, v: out }
    }
}

/// 3x3 pooling: stride 2 and the maximum, unpadded; or stride 1 and the average of what's
/// inside the picture, padded.
fn pool(x: &Planes, max: bool) -> Planes {
    let (stride, same) = if max { (2, false) } else { (1, true) };
    let (oh, top) = padding(x.h, 3, stride, same);
    let (ow, left) = padding(x.w, 3, stride, same);
    let mut v = vec![0.0; x.c * oh * ow];
    for c in 0..x.c {
        let plane = &x.v[c * x.h * x.w..];
        for oy in 0..oh {
            for ox in 0..ow {
                let (mut sum, mut n, mut most) = (0.0, 0.0, f32::MIN);
                for ky in 0..3 {
                    for kx in 0..3 {
                        let (iy, ix) = ((oy * stride + ky) as isize - top as isize, (ox * stride + kx) as isize - left as isize);
                        if iy >= 0 && ix >= 0 && (iy as usize) < x.h && (ix as usize) < x.w {
                            let p = plane[iy as usize * x.w + ix as usize];
                            sum += p;
                            n += 1.0;
                            most = most.max(p);
                        }
                    }
                }
                v[(c * oh + oy) * ow + ox] = if max { most } else { sum / n };
            }
        }
    }
    Planes { c: x.c, h: oh, w: ow, v }
}

/// The style network, on the CPU.
pub struct StyleNet {
    layers: HashMap<String, Layer>,
    blocks: Vec<Vec<Vec<Op>>>,
}

impl StyleNet {
    pub fn new(t: &Tensors) -> Result<Self> {
        let blocks = inception();
        let mut layers = HashMap::new();
        for op in blocks.iter().flatten().flatten() {
            if let Op::Conv(name, _, _) = op {
                let (dims, w, b) = folded(t, name)?;
                layers.insert(name.clone(), Layer { kh: dims[0], kw: dims[1], cin: dims[2], cout: dims[3], w, b });
            }
        }
        Ok(Self { layers, blocks })
    }

    /// A style's numbers from its picture: [3][256][256], 0 to 1 (`SIDE` square). The picture is
    /// smoothed first (3x3), as the model's own example does.
    pub fn style(&self, picture: &[f32]) -> Result<Vec<f32>> {
        ensure!(picture.len() == 3 * SIDE * SIDE, "a style picture is 3 x {SIDE} x {SIDE}");
        let smooth = pool(&Planes { c: 3, h: SIDE, w: SIDE, v: picture.to_vec() }, false);
        Ok(self.run(&smooth, &mut |_, _| {}))
    }

    /// From a smoothed picture, with each block's output shown to `keep` (for the tests).
    fn run(&self, picture: &Planes, keep: &mut dyn FnMut(usize, &Planes)) -> Vec<f32> {
        let mut x = picture.clone();
        for (i, block) in self.blocks.iter().enumerate() {
            let mut joined = Planes { c: 0, h: 0, w: 0, v: Vec::new() };
            for branch in block {
                let mut y = x.clone();
                for op in branch {
                    y = match op {
                        Op::Conv(name, stride, same) => self.layers[name].run(&y, *stride, *same),
                        Op::Avg => pool(&y, false),
                        Op::Max => pool(&y, true),
                    };
                }
                (joined.h, joined.w) = (y.h, y.w);
                joined.c += y.c;
                joined.v.extend_from_slice(&y.v);
            }
            x = joined;
            keep(i, &x);
        }
        let n = (x.h * x.w) as f32;
        x.v.chunks(x.h * x.w).map(|c| c.iter().sum::<f32>() / n).collect()
    }
}

// ---- From a style to the painting network's norms ------------------------------------------

/// Each of the painting network's instance norms: its name, and channels.
pub(crate) fn norms() -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for b in 1..=BLOCKS {
        for c in 1..=2 {
            out.push((format!("transformer/residual/residual{b}/conv{c}"), 128));
        }
    }
    for (c, n) in [(1, 64), (2, 32), (3, 3)] {
        out.push((format!("transformer/expand/conv{c}/conv"), n));
    }
    out
}

/// The 1x1 convolutions from a style to each norm's scales and shifts.
pub struct Projection {
    /// [STYLE][all]: for each norm in turn, its scales, then its shifts.
    w: Vec<f32>,
    b: Vec<f32>,
}

impl Projection {
    pub fn new(t: &Tensors) -> Result<Self> {
        let mut columns: Vec<(&[f32], &[f32], usize)> = Vec::new();
        for (name, n) in norms() {
            // Conv_1 makes the scales, Conv the shifts.
            for which in ["Conv_1", "Conv"] {
                let at = format!("style_params/{name}/StyleNorm/{which}");
                let (dims, w) = get(t, &format!("{at}/weights"))?;
                ensure!(dims == &[1, 1, STYLE, n], "{at}/weights is {dims:?}");
                let (_, b) = get(t, &format!("{at}/biases"))?;
                ensure!(b.len() == n, "{at}/biases isn't {n}");
                columns.push((w, b, n));
            }
        }
        let all: usize = columns.iter().map(|c| c.2).sum();
        let mut w = vec![0.0; STYLE * all];
        let mut b = Vec::with_capacity(all);
        let mut at = 0;
        for (cw, cb, n) in columns {
            for i in 0..STYLE {
                w[i * all + at..i * all + at + n].copy_from_slice(&cw[i * n..(i + 1) * n]);
            }
            b.extend_from_slice(cb);
            at += n;
        }
        Ok(Self { w, b })
    }

    /// Every norm's scales and shifts for a style, in `norms()` order.
    pub fn apply(&self, style: &[f32]) -> Vec<f32> {
        let mut out = self.b.clone();
        let all = out.len();
        for (i, s) in style.iter().enumerate().take(STYLE) {
            for (o, w) in out.iter_mut().zip(&self.w[i * all..(i + 1) * all]) {
                *o += s * w;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `FLEX_MAGENTA_WEIGHTS` (the .tar.gz) and `FLEX_MAGENTA_REF` (from tools/magenta/reference.py).
    fn files() -> Option<(Tensors, Tensors)> {
        let (w, r) = (std::env::var("FLEX_MAGENTA_WEIGHTS").ok()?, std::env::var("FLEX_MAGENTA_REF").ok()?);
        let t = read(&std::fs::read(w).unwrap()).unwrap();
        let r = crate::read_safetensors(&std::fs::read(r).unwrap()).unwrap();
        Some((t, r))
    }

    fn worst(got: &[f32], want: &[f32]) -> f32 {
        assert_eq!(got.len(), want.len());
        let scale = want.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
        got.iter().zip(want).fold(0.0f32, |m, (a, b)| m.max((a - b).abs())) / scale
    }

    #[test]
    #[ignore]
    fn style_net_matches_tensorflow() {
        let Some((t, r)) = files() else { return };
        assert!(t.len() > 400, "{} variables", t.len());
        let net = StyleNet::new(&t).unwrap();
        let picture = &r["style_picture"].1;
        let names = ["style.conv", "style.5b", "", "style.5d", "style.6a", "style.6b", "", "", "style.6e"];
        let start = std::time::Instant::now();
        let style = net.run(&Planes { c: 3, h: SIDE, w: SIDE, v: picture.clone() }, &mut |i, x| {
            if let Some(name) = names.get(i).filter(|n| !n.is_empty()) {
                let e = worst(&x.v, &r[*name].1);
                println!("{name}: {:?} worst {e:.2e}", (x.c, x.h, x.w));
                assert!(e < 1e-3, "{name}");
            }
        });
        println!("style network: {} ms", start.elapsed().as_millis());
        let e = worst(&style, &r["style"].1);
        println!("style: worst {e:.2e}");
        assert!(e < 1e-3);
    }
}
