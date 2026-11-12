//! Where the camera is, for effects that ask (`camera = true`), so they can pin things to the
//! world. Points are followed from frame to frame on the GPU (`flex_depth::track`), given 3D
//! positions from depth when they first appear, and the camera solved against them on the CPU.
//! Each shot starts again. A shot's lens and depth offset aren't known, so its first second is
//! solved several ways at once, and the way that fits best is kept.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use flex_depth::track::{Track, corners};

use crate::depth::DepthMap;
use crate::worker::{Job, Worker, push_capped};

const MIN_INTERVAL: Duration = Duration::from_millis(10);
/// The most points followed at once.
const MOST_POINTS: usize = 700;
/// How far off coming back a point can be (pixels), and how different it can look.
const FURTHEST_BACK: f32 = 0.8;
const MOST_DIFFERENT: f32 = 0.07;
/// How far from the picture's edges a point stays followed.
const MARGIN: f32 = 6.0;
/// Points stay this far from people (pixels of the mask around them) and masks count above this.
const AROUND_PEOPLE: usize = 6;
const PEOPLE_MASK: u8 = 110;
/// The lens (degrees across) and depth offsets tried at the start of a shot.
const FOVS: [f64; 3] = [35.0, 50.0, 70.0];
const SHIFTS: [f64; 3] = [0.02, 0.1, 0.3];
/// Which of those to prefer when they're about the same, and by how much.
const USUAL: (f64, f64) = (50.0, 0.1);
const USUAL_BONUS: f64 = 0.02;
/// How many solved frames to judge them over, and how many more before effects are told to fade in.
const JUDGED_FRAMES: u32 = 24;
const READY_FRAMES: f32 = 12.0;
/// Reports waiting for the player to collect them.
const MOST_WAITING: usize = 32;

/// What's in the picture from the camera's point of view.
type V3 = [f64; 3];
type M3 = [[f64; 3]; 3];

const IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

fn apply(r: &M3, v: V3) -> V3 {
    std::array::from_fn(|i| r[i][0] * v[0] + r[i][1] * v[1] + r[i][2] * v[2])
}

fn apply_inverse(r: &M3, v: V3) -> V3 {
    std::array::from_fn(|i| r[0][i] * v[0] + r[1][i] * v[1] + r[2][i] * v[2])
}

fn multiply(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j]))
}

/// The rotation by `w`'s length around `w`.
fn rotation(w: V3) -> M3 {
    let a = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
    if a < 1e-12 {
        return IDENTITY;
    }
    let k = [w[0] / a, w[1] / a, w[2] / a];
    let cross = [[0.0, -k[2], k[1]], [k[2], 0.0, -k[0]], [-k[1], k[0], 0.0]];
    let square = multiply(&cross, &cross);
    std::array::from_fn(|i| std::array::from_fn(|j| IDENTITY[i][j] + a.sin() * cross[i][j] + (1.0 - a.cos()) * square[i][j]))
}

/// Solves `a x = b` for six unknowns.
fn solve6(mut a: [[f64; 7]; 6]) -> Option<[f64; 6]> {
    for i in 0..6 {
        let pivot = (i..6).max_by(|p, q| a[*p][i].abs().total_cmp(&a[*q][i].abs()))?;
        a.swap(i, pivot);
        if a[i][i].abs() < 1e-300 {
            return None;
        }
        let source = a[i];
        for (j, row) in a.iter_mut().enumerate() {
            if j != i {
                let by = row[i] / source[i];
                for (value, from) in row.iter_mut().zip(source).skip(i) {
                    *value -= by * from;
                }
            }
        }
    }
    Some(std::array::from_fn(|i| a[i][6] / a[i][i]))
}

/// A lens: focal length in pixels, and the middle of the picture.
#[derive(Clone, Copy, Debug)]
struct Lens {
    f: f64,
    cx: f64,
    cy: f64,
}

impl Lens {
    fn new(fov: f64, (w, h): (usize, usize)) -> Self {
        Self { f: 0.5 * w as f64 / (fov.to_radians() / 2.0).tan(), cx: (w as f64 - 1.0) / 2.0, cy: (h as f64 - 1.0) / 2.0 }
    }
}

/// How far each point lands from where it's seen, in pixels, with the camera at `r` and `t`.
fn misses(r: &M3, t: V3, world: &[V3], seen: &[[f32; 2]], lens: Lens) -> Vec<f64> {
    world
        .iter()
        .zip(seen)
        .map(|(x, p)| {
            let c = apply(r, *x);
            let c = [c[0] + t[0], c[1] + t[1], c[2] + t[2]];
            if c[2] <= 1e-3 {
                return f64::INFINITY;
            }
            (lens.f * c[0] / c[2] + lens.cx - f64::from(p[0])).hypot(lens.f * c[1] / c[2] + lens.cy - f64::from(p[1]))
        })
        .collect()
}

/// Moves the camera until the points land where they're seen, caring less and less about ones
/// that don't. Returns the camera and how far each point misses.
fn fit_camera(mut r: M3, mut t: V3, world: &[V3], seen: &[[f32; 2]], lens: Lens) -> (M3, V3, Vec<f64>) {
    const SIGMA: f64 = 1.5;
    const ITERATIONS: usize = 12;
    const SOFT_ITERATIONS: usize = 5;
    const DAMPING: f64 = 1e-4;
    if world.len() < 6 {
        return (r, t, vec![f64::INFINITY; world.len()]);
    }
    let f = lens.f;
    for iteration in 0..ITERATIONS {
        let reach = SIGMA * if iteration < SOFT_ITERATIONS { 1.0 } else { 2.5 };
        let mut system = [[0.0f64; 7]; 6];
        for (x, p) in world.iter().zip(seen) {
            let c = apply(&r, *x);
            let (cx, cy, depth) = (c[0] + t[0], c[1] + t[1], c[2] + t[2]);
            let z = depth.max(1e-3);
            let (eu, ev) = (f * cx / z + lens.cx - f64::from(p[0]), f * cy / z + lens.cy - f64::from(p[1]));
            let miss = eu.hypot(ev);
            if depth <= 1e-3 {
                continue;
            }
            let weight = if iteration < SOFT_ITERATIONS {
                (reach / miss.max(1e-9)).min(1.0)
            } else if miss < reach {
                (1.0 - (miss / reach).powi(2)).powi(2)
            } else {
                0.0
            };
            let ju = [-f * cx * cy / (z * z), f + f * cx * cx / (z * z), -f * cy / z, f / z, 0.0, -f * cx / (z * z)];
            let jv = [-f - f * cy * cy / (z * z), f * cx * cy / (z * z), f * cx / z, 0.0, f / z, -f * cy / (z * z)];
            for (row, error) in [(ju, eu), (jv, ev)] {
                for i in 0..6 {
                    for j in 0..6 {
                        system[i][j] += weight * row[i] * row[j];
                    }
                    system[i][6] -= weight * row[i] * error;
                }
            }
        }
        let trace: f64 = (0..6).map(|i| system[i][i]).sum();
        for (i, row) in system.iter_mut().enumerate() {
            row[i] += trace / 6.0 * DAMPING + 1e-9;
        }
        let Some(d) = solve6(system) else { break };
        let turn = rotation([d[0], d[1], d[2]]);
        r = multiply(&turn, &r);
        let moved = apply(&turn, t);
        t = [moved[0] + d[3], moved[1] + d[4], moved[2] + d[5]];
        if d.iter().all(|v| v.abs() < 1e-6) {
            break;
        }
    }
    let left = misses(&r, t, world, seen, lens);
    (r, t, left)
}

/// A depth map to look things up in, at the size points are followed at.
struct DepthAt<'a> {
    map: &'a DepthMap,
    size: (usize, usize),
}

impl DepthAt<'_> {
    fn at(&self, x: f32, y: f32) -> f64 {
        let (w, h) = (crate::depth::WIDTH, crate::depth::HEIGHT);
        let fx = x * (w - 1) as f32 / (self.size.0 - 1).max(1) as f32;
        let fy = y * (h - 1) as f32 / (self.size.1 - 1).max(1) as f32;
        let x0 = (fx.floor() as isize).clamp(0, w as isize - 2) as usize;
        let y0 = (fy.floor() as isize).clamp(0, h as isize - 2) as usize;
        let (tx, ty) = ((fx - x0 as f32).clamp(0.0, 1.0), (fy - y0 as f32).clamp(0.0, 1.0));
        // Its rows run from the bottom.
        let value = |x: usize, y: usize| self.map.values[(h - 1 - y) * w + x].max(1e-3);
        let top = value(x0, y0) * (1.0 - tx) + value(x0 + 1, y0) * tx;
        let bottom = value(x0, y0 + 1) * (1.0 - tx) + value(x0 + 1, y0 + 1) * tx;
        f64::from(top * (1.0 - ty) + bottom * ty)
    }

    fn median(&self) -> f64 {
        let mut values: Vec<f32> = self.map.values.iter().map(|v| v.max(1e-3)).collect();
        let at = values.len() / 2;
        f64::from(*values.select_nth_unstable_by(at, f32::total_cmp).1)
    }
}

/// One guess at a shot's lens and depth offset, and the camera solved with it.
struct Solver {
    fov: f64,
    shift: f64,
    lens: Lens,
    r: M3,
    t: V3,
    /// Turning depth into 1 / distance: `a` × depth + `b`.
    a: Option<f64>,
    b: f64,
    world: HashMap<u32, V3>,
    lost: u32,
    /// Frames solved since it was last lost.
    steady: u32,
    error_sum: f64,
    errors: u32,
}

impl Solver {
    fn new(fov: f64, shift: f64, size: (usize, usize)) -> Self {
        Self { fov, shift, lens: Lens::new(fov, size), r: IDENTITY, t: [0.0; 3], a: None, b: shift, world: HashMap::new(), lost: 0, steady: 0, error_sum: 0.0, errors: 0 }
    }

    /// How far off, on average, with being lost counted against it: lower is better.
    fn score(&self) -> f64 {
        let average = if self.errors > 0 { self.error_sum / f64::from(self.errors) } else { 99.0 };
        average * (1.0 + 0.5 * f64::from(self.lost)) + if (self.fov, self.shift) == USUAL { 0.0 } else { USUAL_BONUS }
    }

    /// Gives points in the picture positions in the world, from depth.
    fn add(&mut self, ids: impl Iterator<Item = u32>, points: impl Iterator<Item = [f32; 2]>, depth: &dyn Fn(f32, f32) -> f64, a: f64) {
        for (id, p) in ids.zip(points) {
            let inverse = (a * depth(p[0], p[1]) + self.b).max(0.02);
            let ray = [(f64::from(p[0]) - self.lens.cx) / self.lens.f / inverse, (f64::from(p[1]) - self.lens.cy) / self.lens.f / inverse, 1.0 / inverse];
            self.world.insert(id, apply_inverse(&self.r, [ray[0] - self.t[0], ray[1] - self.t[1], ray[2] - self.t[2]]));
        }
    }

    /// Takes in where points are in a new frame, returning whether it solved.
    fn step(&mut self, ids: &[u32], points: &[[f32; 2]], depth: &dyn Fn(f32, f32) -> f64, median: f64) -> bool {
        let Some(a) = self.a else {
            let a = 1.0 / median;
            self.a = Some(a);
            self.add(ids.iter().copied(), points.iter().copied(), depth, a);
            return true;
        };
        let (mut world, mut seen, mut new_ids, mut new_points) = (Vec::with_capacity(ids.len()), Vec::with_capacity(ids.len()), Vec::new(), Vec::new());
        for (id, p) in ids.iter().zip(points) {
            match self.world.get(id) {
                Some(x) => {
                    world.push(*x);
                    seen.push(*p);
                }
                None => {
                    new_ids.push(*id);
                    new_points.push(*p);
                }
            }
        }
        let (r, t, left) = fit_camera(self.r, self.t, &world, &seen, self.lens);
        let inliers: Vec<usize> = left.iter().enumerate().filter(|(_, miss)| **miss < 3.0).map(|(i, _)| i).collect();
        if inliers.len() < 25 || (!left.is_empty() && (inliers.len() as f64) < left.len() as f64 * 0.35) {
            self.lost += 1;
            self.steady = 0;
            self.world.clear();
            self.add(ids.iter().copied(), points.iter().copied(), depth, a);
            return false;
        }
        (self.r, self.t) = (r, t);
        let targets: Vec<(f64, f64)> = inliers
            .iter()
            .map(|&i| {
                let c = apply(&self.r, world[i]);
                (1.0 / (c[2] + self.t[2]).max(1e-3), depth(seen[i][0], seen[i][1]))
            })
            .collect();
        // Keep this frame's depth on the same scale as the world.
        let n = targets.len() as f64;
        let (mut a, mut b) = (a, self.b);
        for _ in 0..4 {
            let (mut ss, mut s1, mut w1, mut st, mut t1) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for (target, s) in &targets {
                let weight = 1.0 / (target - (a * s + b)).abs().max(0.02);
                ss += weight * s * s;
                s1 += weight * s;
                w1 += weight;
                st += weight * s * target;
                t1 += weight * target;
            }
            let (p, q) = (ss + 1e-6 * n + 0.01 * n, w1 + 1e-6 * n + 0.01 * n);
            let (u, v) = (st + 0.01 * n * a, t1 + 0.01 * n * b);
            let det = p * q - s1 * s1;
            a = (u * q - v * s1) / det;
            b = (p * v - s1 * u) / det;
        }
        b = b.max(0.0);
        self.a = Some(a);
        self.b = b;
        self.add(new_ids.into_iter(), new_points.into_iter(), depth, a);
        let mut errors: Vec<f64> = inliers.iter().map(|&i| left[i]).collect();
        let at = errors.len() / 2;
        self.error_sum += *errors.select_nth_unstable_by(at, f64::total_cmp).1;
        self.errors += 1;
        self.steady += 1;
        true
    }
}

/// Where the camera is at one frame.
#[derive(Clone, Copy, Debug)]
pub struct Report {
    pub time: f32,
    /// From 0 when effects shouldn't use it to 1.
    pub ready: f32,
    /// World to camera: a point in the camera is `rotation` × the world point + `offset`.
    pub rotation: [[f32; 3]; 3],
    pub offset: [f32; 3],
    /// Focal length, and the width and height it's in pixels of.
    pub lens: [f32; 3],
    /// 1 / distance is the first × depth + the second.
    pub depth: [f32; 2],
}

/// The points being followed in this shot.
#[derive(Default)]
struct Points {
    ids: Vec<u32>,
    at: Vec<[f32; 2]>,
    next_id: u32,
    motion: [f32; 2],
}

/// Which pixels at `size` are too near a person to follow, from masks (RGBA, rows from the bottom).
fn near_people(masks: &[u8], (w, h): (usize, usize)) -> Vec<bool> {
    let (mw, mh) = (crate::depth::WIDTH, crate::depth::HEIGHT);
    let mut people = vec![0.0f32; w * h];
    for y in 0..h {
        let row = mh - 1 - ((y as f32 + 0.5) * mh as f32 / h as f32) as usize;
        for x in 0..w {
            let column = ((x as f32 + 0.5) * mw as f32 / w as f32) as usize;
            people[y * w + x] = f32::from(u8::from(masks[(row * mw + column) * 4] > PEOPLE_MASK));
        }
    }
    flex_depth::track::box_average(&people, w, h, AROUND_PEOPLE).iter().map(|v| *v > 0.02).collect()
}

impl Points {
    /// Follows the points into the newest picture, drops the ones that have lost their way or
    /// are on people, and starts following new ones in the gaps.
    fn advance(&mut self, track: &mut Track, gray: &[f32], size: (usize, usize), bad: Option<&[bool]>) -> anyhow::Result<()> {
        let (w, h) = size;
        if !self.at.is_empty() {
            let guesses = vec![self.motion; self.at.len()];
            let turn = crate::gpu::turn(crate::gpu::Net::Camera);
            let found = track.follow(&self.at, &guesses)?;
            drop(turn);
            let (mut ids, mut at, mut moved) = (Vec::with_capacity(found.len()), Vec::with_capacity(found.len()), Vec::with_capacity(found.len()));
            for ((id, was), f) in self.ids.iter().zip(&self.at).zip(&found) {
                let inside = f.x > MARGIN && f.y > MARGIN && f.x < w as f32 - MARGIN && f.y < h as f32 - MARGIN;
                let clear = bad.is_none_or(|b| !b[(f.y as usize).min(h - 1) * w + (f.x as usize).min(w - 1)]);
                if f.back < FURTHEST_BACK && f.left < MOST_DIFFERENT && inside && clear {
                    ids.push(*id);
                    at.push([f.x, f.y]);
                    moved.push([f.x - was[0], f.y - was[1]]);
                }
            }
            if moved.len() > 8 {
                let middle = moved.len() / 2;
                let of = |i: usize| {
                    let mut values: Vec<f32> = moved.iter().map(|m| m[i]).collect();
                    *values.select_nth_unstable_by(middle, f32::total_cmp).1
                };
                self.motion = [of(0), of(1)];
            }
            (self.ids, self.at) = (ids, at);
        }
        if self.at.len() < MOST_POINTS {
            for p in corners(gray, size, bad, &self.at).into_iter().take(MOST_POINTS - self.at.len()) {
                self.ids.push(self.next_id);
                self.at.push(p);
                self.next_id += 1;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Off,
    Starting,
    Ready { points: usize, inside: f32, ms: f32 },
    Failed(String),
}

impl Status {
    pub fn describe(&self) -> String {
        match self {
            Status::Off => "off".into(),
            Status::Starting => "starting…".into(),
            Status::Ready { points, inside, ms } => format!("{points} points, within {inside:.1} px, {ms:.0} ms a frame"),
            Status::Failed(e) => format!("⚠ {e}"),
        }
    }
}

struct Input {
    /// RGBA, rows from the bottom (as read back from GL), `size`.
    rgba: Vec<u8>,
    size: (usize, usize),
    time: f32,
    cut: bool,
}

#[derive(Default)]
pub struct State {
    status: Status,
    input: Option<Input>,
    busy: bool,
    reports: VecDeque<Report>,
    depth: Option<crate::depth::Link>,
}

impl Job for State {
    type Status = Status;
    fn status(&self) -> &Status {
        &self.status
    }
    fn failed(&self) -> bool {
        matches!(self.status, Status::Failed(_))
    }
    fn starting(&mut self) {
        self.status = Status::Starting;
    }
    fn fail(&mut self, why: String) {
        self.status = Status::Failed(why);
    }
    fn stopped(&mut self) {
        self.reports.clear();
        if matches!(self.status, Status::Ready { .. }) {
            self.status = Status::Off;
        }
    }
}

/// Shared between the UI, the paint callback and the worker.
pub type Link = Worker<State>;

/// How a shot's solving is going.
#[derive(Default)]
struct Shot {
    points: Points,
    /// All the guesses while they're being judged; just the best one after that.
    solvers: Vec<Solver>,
    solved: u32,
    /// When the shot started, so depth from before it isn't used.
    start: f32,
}

impl Link {
    /// An effect wants the camera: starts the worker if it isn't running. `depth` is where its
    /// depth comes from. Call every frame.
    pub fn wanted(&self, depth: crate::depth::Link) {
        self.want("camera", |state| state.depth = Some(depth), |link| link.work());
    }

    pub fn ready_for_frame(&self) -> bool {
        let state = self.lock();
        state.running && state.input.is_none() && !state.busy && !matches!(state.status, Status::Failed(_))
    }

    /// Hands over a frame: RGBA, rows from the bottom, `size`. `cut`: there's been a scene cut
    /// since the last one handed over.
    pub fn submit(&self, rgba: Vec<u8>, size: (usize, usize), time: f32, cut: bool) {
        self.lock().input = Some(Input { rgba, size, time, cut });
        self.notify();
    }

    /// The reports that have come in since the last call, oldest first.
    pub fn take_reports(&self) -> Vec<Report> {
        self.lock().reports.drain(..).collect()
    }

    fn publish(&self, report: Report) {
        push_capped(&mut self.lock().reports, [report], MOST_WAITING);
    }

    fn work(&self) {
        log::info!("camera: running");
        let mut track: Option<Track> = None;
        let mut size = (0, 0);
        let mut shot = Shot::default();
        let mut last_time: Option<f32> = None;
        let mut gray = Vec::new();
        let mut ms = 0.0f32;
        while let Some(input) = self.next(|state| {
            let input = state.input.take()?;
            state.busy = true;
            Some(input)
        }) {
            let started = Instant::now();
            if input.size != size || track.is_none() {
                size = input.size;
                match Track::new(size, MOST_POINTS) {
                    Ok(made) => track = Some(made),
                    Err(e) => {
                        log::warn!("camera: {e:#}");
                        let mut state = self.lock();
                        state.status = Status::Failed(format!("{e:#}"));
                        state.running = false;
                        state.busy = false;
                        return;
                    }
                }
                shot = Shot { start: input.time, ..Default::default() };
                last_time = None;
            }
            let Some(follower) = track.as_mut() else { continue };
            // A cut, or a seek: nothing carries over.
            let follows_on = last_time.is_some_and(|t| (1e-4..0.5).contains(&(input.time - t)));
            if input.cut || !follows_on {
                shot = Shot { start: input.time, ..Default::default() };
            }
            last_time = Some(input.time);
            let w = size.0;
            gray.clear();
            for row in input.rgba.chunks_exact(w * 4).rev() {
                gray.extend(row.as_chunks::<4>().0.iter().map(|p| (f32::from(p[0]) * 0.299 + f32::from(p[1]) * 0.587 + f32::from(p[2]) * 0.114) / 255.0));
            }
            follower.push(&gray);
            let depth = self.lock().depth.clone().and_then(|d| d.newest_map()).filter(|m| m.time >= shot.start - 0.02);
            let bad = depth.as_ref().and_then(|m| m.masks.as_deref()).map(|m| near_people(m, size));
            if let Err(e) = shot.points.advance(follower, &gray, size, bad.as_deref()) {
                log::warn!("camera: {e:#}");
                let mut state = self.lock();
                state.status = Status::Failed(format!("{e:#}"));
                state.running = false;
                state.busy = false;
                return;
            }
            if let Some(map) = &depth {
                self.solve(&mut shot, map, size, input.time);
            }
            let spent = started.elapsed().as_secs_f32() * 1000.0;
            ms = if ms == 0.0 { spent } else { ms + (spent - ms) * 0.1 };
            {
                let mut state = self.lock();
                let inside = shot.solvers.first().map_or(0.0, |s| if s.errors > 0 { (s.error_sum / f64::from(s.errors)) as f32 } else { 0.0 });
                state.status = Status::Ready { points: shot.points.at.len(), inside, ms };
            }
            std::thread::sleep(crate::gpu::rest(crate::gpu::Net::Camera, MIN_INTERVAL).saturating_sub(started.elapsed()));
            self.lock().busy = false;
        }
        log::info!("camera: stopped");
        self.stop();
    }

    fn solve(&self, shot: &mut Shot, map: &DepthMap, size: (usize, usize), time: f32) {
        let view = DepthAt { map, size };
        let depth = |x: f32, y: f32| view.at(x, y);
        if shot.solvers.is_empty() {
            shot.solvers = FOVS.iter().flat_map(|fov| SHIFTS.iter().map(|shift| (*fov, *shift))).map(|(fov, shift)| Solver::new(fov, shift, size)).collect();
        }
        let median = if shot.solvers.iter().any(|s| s.a.is_none()) { view.median() } else { 1.0 };
        for solver in &mut shot.solvers {
            solver.step(&shot.points.ids, &shot.points.at, &depth, median);
        }
        shot.solved += 1;
        if shot.solved == JUDGED_FRAMES
            && let Some(best) = (0..shot.solvers.len()).min_by(|a, b| shot.solvers[*a].score().total_cmp(&shot.solvers[*b].score()))
        {
            let kept = shot.solvers.swap_remove(best);
            log::debug!("camera: lens {} degrees, shift {}, {:.2} px, lost {}", kept.fov, kept.shift, kept.score(), kept.lost);
            shot.solvers = vec![kept];
        }
        if shot.solved >= JUDGED_FRAMES
            && let Some(s) = shot.solvers.first()
        {
            // Having lost its way, the world has changed under any effect using it.
            let ready = ((shot.solved - JUDGED_FRAMES).min(s.steady) as f32 / READY_FRAMES).min(1.0);
            self.publish(Report {
                time,
                ready,
                rotation: s.r.map(|row| row.map(|v| v as f32)),
                offset: s.t.map(|v| v as f32),
                lens: [s.lens.f as f32, size.0 as f32, size.1 as f32],
                depth: [s.a.unwrap_or(1.0) as f32, s.b as f32],
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(r: &M3, t: V3, x: V3, lens: Lens) -> [f32; 2] {
        let c = apply(r, x);
        let z = c[2] + t[2];
        [(lens.f * (c[0] + t[0]) / z + lens.cx) as f32, (lens.f * (c[1] + t[1]) / z + lens.cy) as f32]
    }

    #[test]
    fn a_known_path_through_a_known_room_comes_back_out() {
        let size = (640, 352);
        let lens = Lens::new(50.0, size);
        // Points scattered through a room, from a simple pattern.
        let mut random = 12345u32;
        let mut next = || {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            f64::from(random >> 8) / f64::from(1u32 << 24)
        };
        let room: Vec<V3> = (0..3000).map(|_| [next() * 6.0 - 3.0, next() * 3.0 - 1.5, 2.0 + next() * 7.0]).collect();
        let frames = 90;
        let path = |k: usize| {
            let th = k as f64 / frames as f64;
            (rotation([0.1 * (3.0 * th).sin(), 0.5 * th, 0.04 * (5.0 * th).cos()]), [0.7 * th, 0.1 * (6.0 * th).sin(), -0.5 * th])
        };
        let mut solver = Solver::new(50.0, 0.0, size);
        solver.b = 0.0;
        let mut first = None;
        let mut last = (IDENTITY, [0.0; 3]);
        for k in 0..frames {
            let (r, t) = path(k);
            let mut depths: HashMap<(u32, u32), f64> = HashMap::new();
            let (mut ids, mut points) = (Vec::new(), Vec::new());
            for (i, x) in room.iter().enumerate() {
                let c = apply(&r, *x);
                let z = c[2] + t[2];
                let p = seen(&r, t, *x, lens);
                if z > 0.2 && p[0] > 8.0 && p[1] > 8.0 && p[0] < 632.0 && p[1] < 344.0 {
                    ids.push(i as u32);
                    points.push(p);
                    depths.insert((p[0].to_bits(), p[1].to_bits()), 1.0 / z);
                }
            }
            let lookup = |x: f32, y: f32| depths[&(x.to_bits(), y.to_bits())];
            assert!(solver.step(&ids, &points, &lookup, 1.0), "lost at frame {k}");
            first.get_or_insert((r, t));
            last = (solver.r, solver.t);
        }
        // Compare how far the camera turned and moved from the first frame.
        let (r0, t0) = first.unwrap();
        let (r_end, t_end) = path(frames - 1);
        let centre = |r: &M3, t: V3| apply_inverse(r, [-t[0], -t[1], -t[2]]);
        let (from, to) = (centre(&r0, t0), centre(&r_end, t_end));
        let want: f64 = (0..3).map(|i| (to[i] - from[i]).powi(2)).sum::<f64>().sqrt();
        let got_centre = centre(&last.0, last.1);
        let got: f64 = got_centre.iter().map(|v| v * v).sum::<f64>().sqrt();
        assert!((got / want - 1.0).abs() < 0.1, "moved {got} of {want}");
        assert_eq!(solver.lost, 0);
        assert!(solver.error_sum / f64::from(solver.errors) < 0.5, "{}", solver.error_sum / f64::from(solver.errors));
    }

    #[test]
    fn the_best_lens_is_the_one_that_fits_with_the_usual_one_preferred_when_tied() {
        let size = (640, 352);
        let mut usual = Solver::new(USUAL.0, USUAL.1, size);
        let mut other = Solver::new(35.0, 0.02, size);
        (usual.error_sum, usual.errors, other.error_sum, other.errors) = (1.0, 1, 1.0, 1);
        assert!(usual.score() < other.score());
        other.error_sum = 0.5;
        assert!(other.score() < usual.score());
    }

    #[test]
    fn losing_its_way_starts_the_steady_count_again() {
        let size = (640, 352);
        let mut solver = Solver::new(50.0, 0.0, size);
        solver.a = Some(1.0);
        solver.steady = 40;
        // Too few points to go by.
        assert!(!solver.step(&[1, 2, 3], &[[10.0, 10.0], [20.0, 20.0], [30.0, 30.0]], &|_, _| 1.0, 1.0));
        assert_eq!((solver.steady, solver.lost), (0, 1));
    }

    #[test]
    fn near_people_marks_the_area_around_a_mask() {
        let (mw, mh) = (crate::depth::WIDTH, crate::depth::HEIGHT);
        let mut masks = vec![0u8; mw * mh * 4];
        // A person in the top left, in rows from the bottom.
        for y in mh - 40..mh {
            for x in 0..40 {
                masks[(y * mw + x) * 4] = 255;
            }
        }
        let size = (64, 36);
        let bad = near_people(&masks, size);
        assert!(bad[0]);
        assert!(!bad[35 * 64 + 63]);
    }
}
