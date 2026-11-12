//! Following points from frame to frame, for working out how the camera moves: Lucas-Kanade on
//! an image pyramid as a compute shader, there and back to catch points that have lost their way.
//! `tools/track/reference.py` makes the reference the tests compare against.

use anyhow::Result;
use bytemuck::{Pod, Zeroable};

use super::Gpu;

/// Levels of the pyramid, the full-size picture being the first.
pub const LEVELS: usize = 4;
/// How far around a point to look for a corner, in pixels.
const CORNER_RADIUS: usize = 3;
/// A corner in each cell of this many pixels, at most.
const CELL: usize = 14;
/// How corner-like a point needs to be, and how far in from the picture's edges.
const CORNER: f32 = 4e-4;
const EDGE: usize = 10;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Level {
    w: u32,
    h: u32,
    at: u32,
    unused: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    levels: [Level; LEVELS],
    count: u32,
    from_frame: u32,
    to_frame: u32,
    level_count: u32,
}

/// Where a point went.
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C)]
pub struct Followed {
    pub x: f32,
    pub y: f32,
    /// How different the patch around it looks, on average.
    pub left: f32,
    /// How far from where it started following it back ended up, in pixels.
    pub back: f32,
}

/// Each level's size, in the order they go in a picture's buffer.
fn level_sizes((w, h): (usize, usize)) -> Vec<(usize, usize)> {
    let mut sizes = vec![(w, h)];
    for _ in 1..LEVELS {
        let &(w, h) = sizes.last().unwrap();
        sizes.push((w / 2, h / 2));
    }
    sizes
}

/// Rates of change along each side, as differences of the neighbours (one-sided at the edges).
fn gradients(a: &[f32], w: usize, h: usize) -> (Vec<f32>, Vec<f32>) {
    let mut gx = vec![0.0; a.len()];
    let mut gy = vec![0.0; a.len()];
    for y in 0..h {
        let row = &a[y * w..][..w];
        let out = &mut gx[y * w..][..w];
        out[0] = row[1] - row[0];
        out[w - 1] = row[w - 1] - row[w - 2];
        for x in 1..w - 1 {
            out[x] = (row[x + 1] - row[x - 1]) * 0.5;
        }
    }
    for y in 0..h {
        let (above, below, half) = if y == 0 { (0, 1, 1.0) } else if y == h - 1 { (h - 2, h - 1, 1.0) } else { (y - 1, y + 1, 0.5) };
        for x in 0..w {
            gy[y * w + x] = (a[below * w + x] - a[above * w + x]) * half;
        }
    }
    (gx, gy)
}

/// A picture's brightness (rows from the top) as its levels, each of brightness, x and y
/// gradient. Each level is the one before's pairs of pixels averaged.
fn pyramid(gray: &[f32], size: (usize, usize)) -> Vec<f32> {
    let sizes = level_sizes(size);
    let mut out = Vec::with_capacity(sizes.iter().map(|(w, h)| w * h * 3).sum());
    let mut level = gray.to_vec();
    for (i, &(w, h)) in sizes.iter().enumerate() {
        let (gx, gy) = gradients(&level, w, h);
        out.extend_from_slice(&level);
        out.extend_from_slice(&gx);
        out.extend_from_slice(&gy);
        if let Some(&(nw, nh)) = sizes.get(i + 1) {
            let mut next = Vec::with_capacity(nw * nh);
            for y in 0..nh {
                for x in 0..nw {
                    let at = y * 2 * w + x * 2;
                    next.push((level[at] + level[at + 1] + level[at + w] + level[at + w + 1]) * 0.25);
                }
            }
            level = next;
        }
    }
    out
}

/// `values` averaged over a square reaching `radius` each way (fewer at the edges).
pub fn box_average(values: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let mut sum = vec![0.0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        for x in 0..w {
            sum[(y + 1) * (w + 1) + x + 1] = f64::from(values[y * w + x]) + sum[y * (w + 1) + x + 1] + sum[(y + 1) * (w + 1) + x] - sum[y * (w + 1) + x];
        }
    }
    let mut out = Vec::with_capacity(values.len());
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(radius), (y + radius + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(radius), (x + radius + 1).min(w));
            let total = sum[y1 * (w + 1) + x1] - sum[y0 * (w + 1) + x1] - sum[y1 * (w + 1) + x0] + sum[y0 * (w + 1) + x0];
            out.push((total / ((y1 - y0) * (x1 - x0)) as f64) as f32);
        }
    }
    out
}

/// The best corner in each cell of the picture that has no point in it yet, avoiding the pixels
/// `bad` says to. Rows from the top, `(x, y)` in pixels.
pub fn corners(gray: &[f32], (w, h): (usize, usize), bad: Option<&[bool]>, have: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let (gx, gy) = gradients(gray, w, h);
    let product = |a: &[f32], b: &[f32]| box_average(&a.iter().zip(b).map(|(a, b)| a * b).collect::<Vec<_>>(), w, h, CORNER_RADIUS);
    let (xx, xy, yy) = (product(&gx, &gx), product(&gx, &gy), product(&gy, &gy));
    let mut strength: Vec<f32> = (0..w * h).map(|i| (xx[i] + yy[i] - ((xx[i] - yy[i]).powi(2) + 4.0 * xy[i] * xy[i]).sqrt()) * 0.5).collect();
    for y in 0..h {
        for x in 0..w {
            if y < EDGE || y >= h - EDGE || x < EDGE || x >= w - EDGE || bad.is_some_and(|b| b[y * w + x]) {
                strength[y * w + x] = 0.0;
            }
        }
    }
    let (rows, columns) = (h / CELL, w / CELL);
    let mut taken = vec![false; rows * columns];
    for p in have {
        taken[(p[1] as usize / CELL).min(rows - 1) * columns + (p[0] as usize / CELL).min(columns - 1)] = true;
    }
    let mut out = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            if taken[row * columns + column] {
                continue;
            }
            let mut best = (0.0f32, 0, 0);
            for y in row * CELL..(row + 1) * CELL {
                for x in column * CELL..(column + 1) * CELL {
                    if strength[y * w + x] > best.0 {
                        best = (strength[y * w + x], x, y);
                    }
                }
            }
            if best.0 > CORNER {
                out.push([best.1 as f32, best.2 as f32]);
            }
        }
    }
    out
}

/// Follows points from each picture it's given to the next.
pub struct Track {
    gpu: Gpu,
    size: (usize, usize),
    pipeline: wgpu::ComputePipeline,
    bind: wgpu::BindGroup,
    params: wgpu::Buffer,
    pictures: wgpu::Buffer,
    points: wgpu::Buffer,
    found: wgpu::Buffer,
    read: wgpu::Buffer,
    levels: [Level; LEVELS],
    picture_len: u32,
    /// Which of the two slots has the newest picture, and how many pictures there are.
    newest: usize,
    pictures_given: u32,
    most_points: usize,
}

impl Track {
    /// For pictures of `size`, following up to `most_points` at a time.
    pub fn new(size: (usize, usize), most_points: usize) -> Result<Self> {
        let gpu = Gpu::shared()?;
        let sizes = level_sizes(size);
        let mut levels = [Level::zeroed(); LEVELS];
        let mut at = 0;
        for (level, &(w, h)) in levels.iter_mut().zip(&sizes) {
            *level = Level { w: w as u32, h: h as u32, at, unused: 0 };
            at += (w * h * 3) as u32;
        }
        let device = &gpu.0.device;
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let buffer = |label, bytes: usize, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: bytes as u64, usage, mapped_at_creation: false });
        let params = buffer("track params", size_of::<Params>(), wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let pictures = buffer("track pictures", at as usize * 2 * 4, storage);
        let points = buffer("track points", most_points * 16, storage);
        let found = buffer("track found", most_points * 16, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC);
        let read = buffer("track read", most_points * 16, wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST);
        let source = wgpu::ShaderModuleDescriptor { label: Some("track"), source: wgpu::ShaderSource::Wgsl(include_str!("track.wgsl").into()) };
        let module = unsafe { device.create_shader_module_trusted(source, wgpu::ShaderRuntimeChecks::unchecked()) };
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("track"), layout: None, module: &module, entry_point: Some("main"), compilation_options: Default::default(), cache: None });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("track"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: pictures.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: points.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: found.as_entire_binding() },
            ],
        });
        Ok(Self { size, pipeline, bind, params, pictures, points, found, read, levels, picture_len: at, newest: 0, pictures_given: 0, most_points, gpu })
    }

    /// The next picture: brightness from 0 to 1, rows from the top.
    pub fn push(&mut self, gray: &[f32]) {
        self.newest = 1 - self.newest;
        let levels = pyramid(gray, self.size);
        self.gpu.0.queue.write_buffer(&self.pictures, u64::from(self.picture_len) * 4 * self.newest as u64, bytemuck::cast_slice(&levels));
        self.pictures_given += 1;
    }

    /// Follows `points` from the picture before the last `push` to that one, each with a guess at
    /// how far it moved. There must have been two pictures.
    pub fn follow(&mut self, points: &[[f32; 2]], guesses: &[[f32; 2]]) -> Result<Vec<Followed>> {
        anyhow::ensure!(self.pictures_given >= 2, "needs two pictures");
        anyhow::ensure!(points.len() <= self.most_points && points.len() == guesses.len(), "too many points");
        if points.is_empty() {
            return Ok(Vec::new());
        }
        let device = &self.gpu.0.device;
        let queue = &self.gpu.0.queue;
        let (from, to) = (1 - self.newest, self.newest);
        let params = Params { levels: self.levels, count: points.len() as u32, from_frame: self.picture_len * from as u32, to_frame: self.picture_len * to as u32, level_count: LEVELS as u32 };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        let input: Vec<[f32; 4]> = points.iter().zip(guesses).map(|(p, g)| [p[0], p[1], g[0], g[1]]).collect();
        queue.write_buffer(&self.points, 0, bytemuck::cast_slice(&input));
        let bytes = points.len() as u64 * 16;
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.dispatch_workgroups((points.len() as u32).div_ceil(16), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.found, 0, &self.read, 0, Some(bytes));
        queue.submit([encoder.finish()]);
        let slice = self.read.slice(..bytes);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow::anyhow!("{e}"))?;
        let out = bytemuck::cast_slice::<u8, Followed>(&slice.get_mapped_range().map_err(|e| anyhow::anyhow!("{e}"))?).to_vec();
        self.read.unmap();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read_safetensors;

    #[test]
    fn levels_halve_and_keep_a_flat_picture() {
        let levels = pyramid(&vec![0.5; 64 * 32], (64, 32));
        assert_eq!(levels.len(), (64 * 32 + 32 * 16 + 16 * 8 + 8 * 4) * 3);
        assert!(levels[..64 * 32].iter().all(|v| *v == 0.5));
        assert!(levels[64 * 32..64 * 32 * 3].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn corners_are_found_only_where_there_arent_any_yet() {
        let (w, h) = (112, 84);
        let mut gray = vec![0.0; w * h];
        // A bright square has four corners, one in the middle of each of four cells.
        for y in 35..63 {
            for x in 49..77 {
                gray[y * w + x] = 1.0;
            }
        }
        let found = corners(&gray, (w, h), None, &[]);
        assert_eq!(found.len(), 4, "{found:?}");
        assert_eq!(corners(&gray, (w, h), None, &found).len(), 0);
    }

    /// Checks against `tools/track/reference.py` (ignored: needs its output and a GPU):
    /// `FLEX_TRACK_REF=ref.safetensors cargo test -p flex-depth track_matches_python -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn track_matches_python() {
        let reference = read_safetensors(&std::fs::read(std::env::var("FLEX_TRACK_REF").expect("FLEX_TRACK_REF")).unwrap()).unwrap();
        let get = |name: &str| reference.get(name).unwrap_or_else(|| panic!("{name}")).clone();
        let (shape, a) = get("a");
        let (_, b) = get("b");
        let size = (shape[1], shape[0]);
        let pairs = |values: &[f32]| values.chunks(2).map(|c| [c[0], c[1]]).collect::<Vec<_>>();

        let found = corners(&a, size, None, &[]);
        assert_eq!(found, pairs(&get("corners").1), "corners");

        let points = pairs(&get("points").1);
        let (guess, there, back) = (vec![[0.0, 0.0]; points.len()], get("there").1, get("back").1);
        let left = get("left").1;
        let mut track = Track::new(size, 1024).unwrap();
        track.push(&a);
        track.push(&b);
        let got = track.follow(&points, &guess).unwrap();
        let mut worst = (0.0f32, 0.0f32, 0.0f32);
        for (i, g) in got.iter().enumerate() {
            worst.0 = worst.0.max((g.x - there[i * 2]).abs().max((g.y - there[i * 2 + 1]).abs()));
            worst.1 = worst.1.max((g.left - left[i]).abs());
            let b = (back[i * 2] - points[i][0]).hypot(back[i * 2 + 1] - points[i][1]);
            worst.2 = worst.2.max((g.back - b).abs());
        }
        println!("{} points; worst differences: position {:.4} px, patch {:.5}, coming back {:.4} px", got.len(), worst.0, worst.1, worst.2);
        assert!(worst.0 < 0.05 && worst.1 < 0.005 && worst.2 < 0.1, "{worst:?}");
    }

    /// How long following takes (ignored: wants the GPU to itself, and the same reference).
    #[test]
    #[ignore]
    fn track_timing() {
        let reference = read_safetensors(&std::fs::read(std::env::var("FLEX_TRACK_REF").expect("FLEX_TRACK_REF")).unwrap()).unwrap();
        let (shape, a) = reference.get("a").unwrap().clone();
        let (_, b) = reference.get("b").unwrap().clone();
        let size = (shape[1], shape[0]);
        let some: Vec<[f32; 2]> = reference.get("points").unwrap().1.chunks(2).map(|c| [c[0], c[1]]).collect();
        let points: Vec<[f32; 2]> = some.iter().cycle().take(700).copied().collect();
        let guess = vec![[0.0, 0.0]; points.len()];
        let mut track = Track::new(size, 1024).unwrap();
        for _ in 0..3 {
            track.push(&a);
            track.push(&b);
            track.follow(&points, &guess).unwrap();
        }
        let mut pushes = Vec::new();
        let mut follows = Vec::new();
        for i in 0..40 {
            let t = std::time::Instant::now();
            track.push(if i % 2 == 0 { &a } else { &b });
            pushes.push(t.elapsed().as_secs_f32() * 1000.0);
            let t = std::time::Instant::now();
            track.follow(&points, &guess).unwrap();
            follows.push(t.elapsed().as_secs_f32() * 1000.0);
        }
        pushes.sort_by(f32::total_cmp);
        follows.sort_by(f32::total_cmp);
        println!("700 points there and back: follow {:.1} ms median, {:.1} worst; pyramid and upload {:.1} ms", follows[20], follows[39], pushes[20]);
    }
}
