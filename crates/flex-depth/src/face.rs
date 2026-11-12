//! Finding faces and looking closer at them, as wgpu compute shaders. YuNet (MIT, from OpenCV's
//! model zoo) gives each face a box and five points. MediaPipe's Face Mesh V2 (Apache 2.0) then
//! gives one with 478 points, and a MobileNetV2 trained for head pose (MIT, by yakhyo, as used
//! by UniFace) says which way it's turned, even side-on where the mesh gives up. The plans and
//! weights are in `.bin` files in `models`, made by `tools/face/convert.py` and `convert_detail.py`.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};

use crate::{Kernel, Kernels, Mem, Ops, Runner, transpose};

const YUNET: &str = "yunet.bin";
const FACE_MESH: &str = "face_mesh.bin";
const HEAD_ANGLE: &str = "head_angle.bin";
/// The picture it takes.
pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 352;
/// The square pictures the closer looks take.
pub const MESH_SIZE: usize = 256;
pub const HEAD_SIZE: usize = 224;
pub const MESH_POINTS: usize = 478;
const STRIDES: [usize; 3] = [8, 16, 32];
/// Below this it's probably not a face. Lower than YuNet's usual 0.9, which misses too many
/// faces in films.
const MIN_SCORE: f32 = 0.6;
/// Boxes overlapping this much are the same face.
const SAME_FACE: f32 = 0.3;
/// Workgroups in a row of a dispatch (as in face.wgsl).
const ROW: usize = 65535;
/// Convolutions with at most this many outputs, each summing at least `REDUCE_SUM` products,
/// share each sum among a workgroup: on their own, too few threads would do too much each.
const REDUCE_OUTPUTS: usize = 4096;
const REDUCE_SUM: usize = 256;
/// About how many workgroups a 1x1 convolution should have (the RX 580 has 36 compute units):
/// fewer tiles than this, and its sum is split into parts.
const TILE_TARGET: usize = 144;
/// What the head angle network was trained on: ImageNet's averages.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    h: u32,
    w: u32,
    cin: u32,
    cout: u32,
    a: u32,
    b: u32,
    c: u32,
    weights: u32,
    bias: u32,
    kernel: u32,
    stride: u32,
    groups: u32,
    act: u32,
    oh: u32,
    ow: u32,
    pad: u32,
    alpha: u32,
    skip_c: u32,
    /// A 1x1 convolution's sum split into this many parts, left at `part_at` (see face.wgsl).
    parts: u32,
    part_at: u32,
}

/// One face, in pixels of the picture given. Points are the right eye, left eye, nose and the
/// right and left corners of the mouth (the person's own right and left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub score: f32,
    pub points: [[f32; 2]; 5],
}

/// A face's 478 points in the square it was given (x and y in pixels, z in about the same
/// units, smaller nearer), and above 0 if it thinks there's a face there at all.
#[derive(Clone, Debug)]
pub struct Mesh {
    pub points: Vec<[f32; 3]>,
    pub present: f32,
}

#[derive(Clone, Copy)]
struct Tensor {
    at: u32,
    channels: usize,
    h: usize,
    w: usize,
}

fn field(op: &serde_json::Value, name: &str) -> Result<usize> {
    op[name].as_u64().map(|v| v as usize).with_context(|| format!("the plan's op has no {name}"))
}

fn name<'a>(op: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    op[key].as_str().with_context(|| format!("the plan's op has no {key}"))
}

/// A half-precision float as a full one.
pub(crate) fn from_f16(bits: u16) -> f32 {
    let sign = u32::from(bits >> 15) << 31;
    let exponent = u32::from(bits >> 10) & 0x1f;
    let fraction = u32::from(bits) & 0x3ff;
    f32::from_bits(match exponent {
        0 if fraction == 0 => sign,
        0 => {
            // Too small to have a leading one: shift until it does.
            let shift = fraction.leading_zeros() - 21;
            sign | (113 - shift) << 23 | ((fraction << shift) & 0x3ff) << 13
        }
        31 => sign | 0x7f80_0000 | fraction << 13,
        _ => sign | (exponent + 112) << 23 | fraction << 13,
    })
}

/// One network: its plan on the GPU, and where the outputs asked for are.
struct Net {
    run: Runner<Params>,
    outputs: Vec<Tensor>,
    /// The part of `mem` that holds them: where it starts, and how many floats.
    span: (u32, usize),
}

impl Net {
    /// `input` is the picture's channels, height and width; `wanted` the outputs to read.
    fn new(kernels: &Kernels, model: &[u8], input: (usize, usize, usize), wanted: &[String]) -> Result<Self> {
        if model.len() < 4 {
            bail!("the face model is empty");
        }
        let plan_len = u32::from_le_bytes([model[0], model[1], model[2], model[3]]) as usize;
        let plan: serde_json::Value = serde_json::from_slice(&model[4..4 + plan_len]).context("reading the face model's plan")?;
        let bytes = &model[4 + plan_len..];
        let mut weights: Vec<f32> = if plan["format"].as_str() == Some("f16") {
            bytes.as_chunks::<2>().0.iter().map(|b| from_f16(u16::from_le_bytes(*b))).collect()
        } else {
            bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
        };

        let mut tensors: HashMap<String, Tensor> = HashMap::new();
        let (channels, height, width) = input;
        let mut m = Mem::default();
        tensors.insert("input".into(), Tensor { at: m.take(channels * height * width), channels, h: height, w: width });
        let mut ops = Ops::default();
        let mut transposed = std::collections::HashSet::new();
        for op in plan["ops"].as_array().context("the plan has no ops")? {
            let input = *tensors.get(name(op, "in")?).context("an op's input comes before it's made")?;
            let (kernel, mut p, out) = match name(op, "op")? {
                "conv" => {
                    let (stride, cout, size) = (field(op, "stride")?, field(op, "cout")?, field(op, "kernel")?);
                    let (oh, ow) = ((input.h - 1) / stride + 1, (input.w - 1) / stride + 1);
                    let act = op["act"].as_u64().unwrap_or(u64::from(op["relu"].as_bool().unwrap_or(false))) as u32;
                    let skip = match op["skip"].as_str() {
                        Some(skip) => tensors.get(skip).context("an op's input comes before it's made")?.at,
                        None => 0,
                    };
                    let p = Params {
                        cin: field(op, "cin")? as u32,
                        cout: cout as u32,
                        weights: field(op, "w")? as u32,
                        bias: field(op, "b")? as u32,
                        kernel: size as u32,
                        stride: stride as u32,
                        groups: field(op, "groups")? as u32,
                        act,
                        b: skip,
                        pad: op["pad"].as_u64().unwrap_or(size as u64 / 2) as u32,
                        alpha: op["alpha"].as_u64().unwrap_or(0) as u32,
                        skip_c: op["skip_c"].as_u64().unwrap_or(0) as u32,
                        ..Default::default()
                    };
                    // Few outputs with long sums share each sum among a workgroup.
                    let reduce = p.groups == 1 && cout * oh * ow <= REDUCE_OUTPUTS && p.cin as usize * size * size >= REDUCE_SUM;
                    let kernel = if reduce {
                        Kernel::ConvReduce
                    } else if size == 1 && p.groups == 1 && stride == 1 {
                        Kernel::Conv1Tile
                    } else {
                        Kernel::FaceConv
                    };
                    if matches!(kernel, Kernel::Conv1Tile) {
                        // conv1_tile reads the weights as [cin][cout].
                        let (cin, at) = (p.cin as usize, p.weights as usize);
                        if !transposed.insert(at) {
                            bail!("two 1x1 convolutions share weights");
                        }
                        let block = transpose(&weights[at..at + cout * cin], cout, cin);
                        weights[at..at + cout * cin].copy_from_slice(&block);
                    }
                    (kernel, p, Tensor { at: 0, channels: cout, h: oh, w: ow })
                }
                "pool" => (Kernel::Pool, Params::default(), Tensor { at: 0, channels: input.channels, h: input.h / 2, w: input.w / 2 }),
                "add_up" => {
                    let up = *tensors.get(name(op, "up")?).context("an op's input comes before it's made")?;
                    if up.h * 2 != input.h || up.w * 2 != input.w {
                        bail!("the picture's size has to be a multiple of 32");
                    }
                    (Kernel::AddUp, Params { b: up.at, ..Default::default() }, Tensor { at: 0, channels: input.channels, h: input.h, w: input.w })
                }
                "gap" => (Kernel::Gap, Params::default(), Tensor { at: 0, channels: input.channels, h: 1, w: 1 }),
                other => bail!("unknown op {other}"),
            };
            let out = Tensor { at: m.take(out.channels * out.h * out.w), ..out };
            if matches!(kernel, Kernel::Conv1Tile) {
                let pixels = out.h * out.w;
                let tiles = pixels.div_ceil(32) * out.channels.div_ceil(32);
                // Enough workgroups to go round, each part summing at least 64 channels.
                let parts = TILE_TARGET.div_ceil(tiles).min(input.channels / 64).max(1);
                p.parts = parts as u32;
                if parts > 1 {
                    p.part_at = m.take(parts * out.channels * pixels);
                }
            }
            // add_up's h and w are B's; the others' are the input's.
            let (h, w) = if matches!(kernel, Kernel::AddUp) { (input.h / 2, input.w / 2) } else { (input.h, input.w) };
            p = Params { h: h as u32, w: w as u32, a: input.at, c: out.at, oh: out.h as u32, ow: out.w as u32, cout: out.channels as u32, ..p };
            // One thread per output (conv_reduce: a workgroup per output), 64 to a workgroup, in
            // rows of ROW workgroups (face.wgsl); conv1_tile as it says.
            let flat = |n: usize| {
                let groups = n.div_ceil(64);
                [groups.min(ROW) as u32, groups.div_ceil(ROW) as u32, 1]
            };
            match kernel {
                Kernel::ConvReduce => ops.push(kernel, p, flat(out.channels * out.h * out.w * 64)),
                Kernel::Conv1Tile => {
                    ops.push(kernel, p, [out.channels.div_ceil(32) as u32, (out.h * out.w).div_ceil(32) as u32, p.parts]);
                    if p.parts > 1 {
                        ops.push(Kernel::SumParts, p, flat(out.channels * out.h * out.w));
                    }
                }
                _ => ops.push(kernel, p, flat(out.channels * out.h * out.w)),
            }
            tensors.insert(name(op, "out")?.to_owned(), out);
        }
        let mut outputs = Vec::new();
        for kind in wanted {
            let name = plan["outputs"][kind].as_str().context("the plan has no output")?;
            outputs.push(*tensors.get(name).context("the plan's output isn't made")?);
        }
        let first = outputs.iter().map(|t| t.at).min().unwrap_or(0);
        let last = outputs.iter().map(|t| t.at as usize + t.channels * t.h * t.w).max().unwrap_or(0);
        let run = Runner::with(kernels.clone(), &weights, ops, Vec::new(), m.end, "faces")?;
        Ok(Self { run, outputs, span: (first, last - first as usize) })
    }

    /// For tuning: each dispatch's time (ms), grouped by what it is, then the whole run in one
    /// submission.
    #[cfg(test)]
    fn report(&self) {
        self.run.report(|_, kernel, p| {
            let kind = match kernel {
                Kernel::FaceConv if p.groups > 1 => format!("depthwise {}x{}", p.kernel, p.kernel),
                Kernel::FaceConv => format!("conv {}x{} {}->{}", p.kernel, p.kernel, p.cin, p.cout),
                Kernel::ConvReduce => format!("shared {}x{} {}->{}", p.kernel, p.kernel, p.cin, p.cout),
                Kernel::Conv1Tile => format!("tiled 1x1 in {} parts", p.parts),
                Kernel::SumParts => "sum parts".to_owned(),
                k => format!("{k:?}"),
            };
            format!("{kind} at {}x{}", p.ow, p.oh)
        });
        eprintln!("{:.2} ms in one submission", self.run.time_first(self.run.ops.list.len(), 5));
    }

    /// The asked-for outputs of a picture, channels first, each as the shaders leave it.
    fn run(&mut self, input: &[f32]) -> Result<Vec<Vec<f32>>> {
        self.run.write(0, input);
        // As one submission: each costs a little, and these are short enough not to hold up
        // what else is drawing.
        self.run.submit(0..self.run.ops.list.len());
        let (first, len) = self.span;
        let floats = self.run.read(first, len)?;
        Ok(self.outputs.iter().map(|t| floats[(t.at - first) as usize..][..t.channels * t.h * t.w].to_vec()).collect())
    }
}

pub struct Face {
    /// face.wgsl's, which all three networks share.
    kernels: Kernels,
    yunet: Net,
    /// Made when first asked for, as most effects only want boxes.
    mesh: Option<Net>,
    head: Option<Net>,
    /// The tensors YuNet's outputs are read from, scale by scale.
    shapes: [[Tensor; 4]; 3],
}

impl Face {
    pub fn new() -> Result<Self> {
        let all = [Kernel::FaceConv, Kernel::Pool, Kernel::AddUp, Kernel::Gap, Kernel::ConvReduce, Kernel::Conv1Tile, Kernel::SumParts];
        let kernels = Kernels::new::<Params>("face.wgsl", include_str!("face.wgsl"), all)?;
        let wanted: Vec<String> = STRIDES.iter().flat_map(|s| ["cls", "obj", "bbox", "kps"].map(|k| format!("{k}_{s}"))).collect();
        let yunet = Net::new(&kernels, &super::models::load(YUNET)?, (3, HEIGHT, WIDTH), &wanted)?;
        let at = |scale: usize, kind: usize| yunet.outputs[scale * 4 + kind];
        let shapes = [0, 1, 2].map(|s| [0, 1, 2, 3].map(|k| at(s, k)));
        Ok(Self { kernels, yunet, mesh: None, head: None, shapes })
    }

    /// The GPU it runs on.
    pub fn adapter(&self) -> &str {
        &self.yunet.run.adapter
    }

    /// Faces in a picture of `WIDTH` × `HEIGHT`, as RGB bytes row by row from the top.
    pub fn run(&mut self, rgb: &[u8]) -> Result<Vec<Detection>> {
        Ok(decode(&self.outputs_for(rgb)?))
    }

    /// The mesh of the face in a square of `MESH_SIZE`, as RGB bytes row by row from the top.
    pub fn mesh(&mut self, rgb: &[u8]) -> Result<Mesh> {
        if rgb.len() != MESH_SIZE * MESH_SIZE * 3 {
            bail!("expected a {MESH_SIZE} square of RGB, got {} bytes", rgb.len());
        }
        if self.mesh.is_none() {
            self.mesh = Some(Net::new(&self.kernels, &super::models::load(FACE_MESH)?, (3, MESH_SIZE, MESH_SIZE), &["points".to_owned(), "present".to_owned()])?);
        }
        let mut input = vec![0.0f32; 3 * MESH_SIZE * MESH_SIZE];
        for (i, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
            for c in 0..3 {
                input[c * MESH_SIZE * MESH_SIZE + i] = f32::from(px[c]) / 255.0;
            }
        }
        let out = self.mesh.as_mut().context("just made")?.run(&input)?;
        let points = out[0].as_chunks::<3>().0.to_vec();
        Ok(Mesh { points, present: out[1][0] })
    }

    /// Which way the head in a square of `HEAD_SIZE` is turned, as a rotation matrix.
    pub fn head(&mut self, rgb: &[u8]) -> Result<[[f32; 3]; 3]> {
        if rgb.len() != HEAD_SIZE * HEAD_SIZE * 3 {
            bail!("expected a {HEAD_SIZE} square of RGB, got {} bytes", rgb.len());
        }
        if self.head.is_none() {
            self.head = Some(Net::new(&self.kernels, &super::models::load(HEAD_ANGLE)?, (3, HEAD_SIZE, HEAD_SIZE), &["six".to_owned()])?);
        }
        let mut input = vec![0.0f32; 3 * HEAD_SIZE * HEAD_SIZE];
        for (i, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
            for c in 0..3 {
                input[c * HEAD_SIZE * HEAD_SIZE + i] = (f32::from(px[c]) / 255.0 - MEAN[c]) / STD[c];
            }
        }
        let out = self.head.as_mut().context("just made")?.run(&input)?;
        Ok(rotation(&out[0]))
    }

    /// Each scale's cls, obj, bbox and kps cell by cell, as the last convolutions leave them
    /// (the scores before their sigmoid).
    fn outputs_for(&mut self, rgb: &[u8]) -> Result<[[Vec<f32>; 4]; 3]> {
        if rgb.len() != WIDTH * HEIGHT * 3 {
            bail!("expected {WIDTH}x{HEIGHT} RGB, got {} bytes", rgb.len());
        }
        // The model was trained on BGR, 0 to 255, channels first.
        let mut input = vec![0.0f32; 3 * HEIGHT * WIDTH];
        for (i, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
            for c in 0..3 {
                input[c * HEIGHT * WIDTH + i] = f32::from(px[2 - c]);
            }
        }
        let out = self.yunet.run(&input)?;
        let shapes = self.shapes;
        Ok([0, 1, 2].map(|s| [0, 1, 2, 3].map(|k| cells(&out[s * 4 + k], shapes[s][k]))))
    }
}

/// Six numbers to a rotation matrix: the first two columns, made square to each other.
fn rotation(six: &[f32]) -> [[f32; 3]; 3] {
    let normal = |v: [f32; 3]| {
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-8);
        v.map(|x| x / n)
    };
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let x = normal([six[0], six[1], six[2]]);
    let z = normal(cross(x, [six[3], six[4], six[5]]));
    let y = cross(z, x);
    std::array::from_fn(|row| [x[row], y[row], z[row]])
}

/// Channels first to cell by cell.
fn cells(src: &[f32], t: Tensor) -> Vec<f32> {
    let plane = t.h * t.w;
    let mut out = vec![0.0; src.len()];
    for cell in 0..plane {
        for c in 0..t.channels {
            out[cell * t.channels + c] = src[c * plane + cell];
        }
    }
    out
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn overlap(a: &Detection, b: &Detection) -> f32 {
    let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
    let (x1, y1) = ((a.x + a.width).min(b.x + b.width), (a.y + a.height).min(b.y + b.height));
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    inter / (a.width * a.height + b.width * b.height - inter).max(1e-6)
}

/// From the network's outputs to faces, the way OpenCV's FaceDetectorYN does it.
fn decode(outputs: &[[Vec<f32>; 4]; 3]) -> Vec<Detection> {
    let mut found = Vec::new();
    for (scale, stride) in outputs.iter().zip(STRIDES) {
        let (cols, rows) = (WIDTH / stride, HEIGHT / stride);
        let [cls, obj, bbox, kps] = scale;
        for r in 0..rows {
            for c in 0..cols {
                let i = r * cols + c;
                let score = (sigmoid(cls[i]) * sigmoid(obj[i])).sqrt();
                if score < MIN_SCORE {
                    continue;
                }
                let s = stride as f32;
                let (cf, rf) = (c as f32, r as f32);
                let (width, height) = (bbox[i * 4 + 2].exp() * s, bbox[i * 4 + 3].exp() * s);
                let mut points = [[0.0; 2]; 5];
                for (n, point) in points.iter_mut().enumerate() {
                    *point = [(kps[i * 10 + 2 * n] + cf) * s, (kps[i * 10 + 2 * n + 1] + rf) * s];
                }
                found.push(Detection { x: (cf + bbox[i * 4]) * s - width / 2.0, y: (rf + bbox[i * 4 + 1]) * s - height / 2.0, width, height, score, points });
            }
        }
    }
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Detection> = Vec::new();
    for face in found {
        if kept.iter().all(|k| overlap(k, &face) < SAME_FACE) {
            kept.push(face);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All the outputs for an empty picture, with one strong cell at `cell` of the first scale.
    fn outputs(cell: Option<(usize, usize)>) -> [[Vec<f32>; 4]; 3] {
        let mut out = STRIDES.map(|s| {
            let n = (WIDTH / s) * (HEIGHT / s);
            [vec![-20.0; n], vec![-20.0; n], vec![0.0; n * 4], vec![0.0; n * 10]]
        });
        if let Some((r, c)) = cell {
            let i = r * WIDTH / 8 + c;
            out[0][0][i] = 10.0;
            out[0][1][i] = 10.0;
            out[0][2][i * 4] = 0.5;
            out[0][2][i * 4 + 2] = 1.0;
            out[0][3][i * 10 + 2] = 0.25;
        }
        out
    }

    #[test]
    fn decodes_a_cell_into_pixels() {
        let faces = decode(&outputs(Some((10, 20))));
        assert_eq!(faces.len(), 1);
        let f = faces[0];
        let width = 1f32.exp() * 8.0;
        assert!((f.width - width).abs() < 1e-3);
        assert!((f.x - (20.5 * 8.0 - width / 2.0)).abs() < 1e-3, "{f:?}");
        assert!((f.y - (10.0 * 8.0 - 4.0)).abs() < 1e-3, "{f:?}");
        assert!((f.points[1][0] - 20.25 * 8.0).abs() < 1e-3, "{f:?}");
        assert!(f.score > 0.99);
    }

    #[test]
    fn nothing_is_found_in_nothing() {
        assert!(decode(&outputs(None)).is_empty());
    }

    #[test]
    fn overlapping_boxes_are_one_face() {
        let mut out = outputs(Some((10, 20)));
        let i = 10 * WIDTH / 8 + 21;
        out[0][0][i] = 2.0;
        out[0][1][i] = 2.0;
        out[0][2][i * 4 + 2] = 1.0;
        let faces = decode(&out);
        assert_eq!(faces.len(), 1, "{faces:?}");
        assert!(faces[0].score > 0.99, "the stronger one stays");
    }

    /// Compares the outputs with ONNX Runtime's (needs a GPU, and the files from
    /// tools/face/convert.py --reference):
    /// `FLEX_FACE_RGB=out.rgb FLEX_FACE_REF=out.bin cargo test -p flex-depth matches_onnx -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn matches_onnx() {
        let rgb = std::fs::read(std::env::var("FLEX_FACE_RGB").expect("FLEX_FACE_RGB")).unwrap();
        let reference = std::fs::read(std::env::var("FLEX_FACE_REF").expect("FLEX_FACE_REF")).unwrap();
        let reference: Vec<f32> = reference.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        let started = std::time::Instant::now();
        let mut face = Face::new().unwrap();
        eprintln!("loaded in {:?} on {}", started.elapsed(), face.adapter());
        let outputs = face.outputs_for(&rgb).unwrap();
        let mut at = 0;
        let mut worst = 0.0f32;
        for (scale, stride) in outputs.iter().zip(STRIDES) {
            for (kind, values) in ["cls", "obj", "bbox", "kps"].iter().zip(scale) {
                // ONNX applies the sigmoid to the scores.
                let got: Vec<f32> = if *kind == "cls" || *kind == "obj" { values.iter().map(|v| sigmoid(*v)).collect() } else { values.clone() };
                let expected = &reference[at..at + got.len()];
                at += got.len();
                let err = got.iter().zip(expected).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
                eprintln!("{kind}_{stride:<3} max error {err:.2e}");
                worst = worst.max(err);
            }
        }
        assert_eq!(at, reference.len());
        assert!(worst < 5e-3, "worst error {worst:.2e}");
        let runs = 30;
        let started = std::time::Instant::now();
        for _ in 0..runs {
            face.run(&rgb).unwrap();
        }
        eprintln!("{:.1} ms a picture", started.elapsed().as_secs_f64() * 1000.0 / runs as f64);
        face.yunet.report();
        eprintln!("{:#?}", face.run(&rgb).unwrap());
    }

    /// Compares a stretch of outputs with a reference file, returning the worst difference.
    fn worst(got: &[f32], reference: &[f32]) -> f32 {
        assert_eq!(got.len(), reference.len());
        got.iter().zip(reference).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()))
    }

    fn floats(var: &str) -> Vec<f32> {
        std::fs::read(std::env::var(var).expect(var)).unwrap().as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
    }

    /// Compares the mesh with TensorFlow Lite's (needs a GPU, and the files from
    /// tools/face/convert_detail.py mesh --reference):
    /// `FLEX_FACE_RGB=out.rgb FLEX_FACE_REF=out.bin cargo test -p flex-depth mesh_matches_tflite -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn mesh_matches_tflite() {
        let rgb = std::fs::read(std::env::var("FLEX_FACE_RGB").expect("FLEX_FACE_RGB")).unwrap();
        let reference = floats("FLEX_FACE_REF");
        let mut face = Face::new().unwrap();
        let started = std::time::Instant::now();
        let mesh = face.mesh(&rgb).unwrap();
        eprintln!("first run, loading included: {:?}", started.elapsed());
        let flat: Vec<f32> = mesh.points.iter().flatten().copied().collect();
        let err = worst(&flat, &reference[..1434]);
        eprintln!("points: max error {err:.2e} pixels; present {} against {}", mesh.present, reference[1434]);
        assert!(err < 0.05, "worst error {err:.2e}");
        assert!((mesh.present - reference[1434]).abs() < 0.01);
        let runs = 60;
        let started = std::time::Instant::now();
        for _ in 0..runs {
            face.mesh(&rgb).unwrap();
        }
        eprintln!("{:.1} ms a face", started.elapsed().as_secs_f64() * 1000.0 / runs as f64);
        face.mesh.as_ref().unwrap().report();
    }

    /// Compares the head angle with ONNX Runtime's (needs a GPU, and the files from
    /// tools/face/convert_detail.py head --reference):
    /// `FLEX_FACE_RGB=out.rgb FLEX_FACE_REF=out.bin cargo test -p flex-depth head_matches_onnx -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn head_matches_onnx() {
        let rgb = std::fs::read(std::env::var("FLEX_FACE_RGB").expect("FLEX_FACE_RGB")).unwrap();
        let reference = floats("FLEX_FACE_REF");
        let mut face = Face::new().unwrap();
        let flat: Vec<f32> = face.head(&rgb).unwrap().iter().flatten().copied().collect();
        let err = worst(&flat, &reference);
        eprintln!("rotation: max error {err:.2e}\n{flat:?}\n{reference:?}");
        // Half-precision weights cost about 0.2 degrees, far below what the network can tell.
        assert!(err < 5e-3, "worst error {err:.2e}");
        let runs = 60;
        let started = std::time::Instant::now();
        for _ in 0..runs {
            face.head(&rgb).unwrap();
        }
        eprintln!("{:.1} ms a face", started.elapsed().as_secs_f64() * 1000.0 / runs as f64);
        face.head.as_ref().unwrap().report();
    }

    #[test]
    fn half_precision_floats_read_right() {
        assert_eq!(from_f16(0x0000), 0.0);
        assert_eq!(from_f16(0x3c00), 1.0);
        assert_eq!(from_f16(0xc000), -2.0);
        assert_eq!(from_f16(0x3e00), 1.5);
        assert_eq!(from_f16(0x7bff), 65504.0);
        assert_eq!(from_f16(0x0400), 2f32.powi(-14));
        assert_eq!(from_f16(0x0001), 2f32.powi(-24));
        assert_eq!(from_f16(0x0200), 2f32.powi(-15));
        assert_eq!(from_f16(0x03ff), 1023.0 * 2f32.powi(-24));
    }

    #[test]
    fn six_numbers_make_a_rotation() {
        let r = rotation(&[2.0, 0.0, 0.0, 1.0, 3.0, 0.0]);
        assert_eq!(r, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    }
}
