//! Faces for video effects that ask for them (`faces = true`, or `mesh = true` for a closer
//! look): networks on the GPU through flex-depth, in a thread of its own. The player's paint callback hands it small frames
//! (see `effects::Renderer`) whenever it's free, and picks up the faces as they come.
//!
//! A detector on its own jitters and misses frames, which would look bad in a shader, so
//! `Tracker` keeps each face in one slot, smooths it, and fades it in and out. Smoothing looks
//! at the frames either side of each one, so it adds no lag, and reports say which frame
//! they're about, so the picture can be held back until the faces for it are known.
//!
//! The closer look happens on the very frame the faces were found in, and goes out in the same
//! report, so a mesh is always drawn over the picture it was made from.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use flex_depth::face::{Detection, Face, HEAD_SIZE, MESH_SIZE};

pub use flex_depth::face::{HEIGHT, MESH_POINTS, WIDTH};

use crate::worker::{Job, Worker, push_capped};

pub const MAX_FACES: usize = 8;
/// Everything known about a face, all of it smoothed and blended the same way. First its box
/// (x0, y0, x1, y1) and five points (x, y), from 0 to 1 over the picture, top down.
pub const BOX_COORDS: usize = 14;
/// Then the mesh, each point x, y (as above) and z (in picture widths, smaller nearer).
pub const MESH_AT: usize = BOX_COORDS;
/// How open the person's right eye, left eye and mouth are, from 0 to 1.
pub const OPEN_AT: usize = MESH_AT + MESH_POINTS * 3;
/// Which way the head points: x to the right of the picture, y up, z towards the viewer.
pub const TURN_AT: usize = OPEN_AT + 3;
/// Whether there's a mesh and a direction: 1 or 0, in between once smoothed.
pub const HAS_MESH: usize = TURN_AT + 3;
pub const HAS_TURN: usize = HAS_MESH + 1;
pub const COORDS: usize = HAS_TURN + 1;
/// How much larger than the detector's the picture is for a closer look.
pub const DETAIL_SCALE: usize = 2;
/// The most faces meshed a frame, largest first, and the smallest worth meshing (pixels across
/// in the larger picture).
const MOST_MESHES: usize = 3;
const SMALLEST_MESH: f32 = 40.0;
/// How far around a face its mesh's square reaches.
const ROI_MARGIN: f32 = 1.5;
/// An eye is shut at about the first of these (lid gap over eye width), open at the sum; a
/// mouth (lip gap over mouth width) is as open as it gets at the last.
const EYE_SHUT: f32 = 0.1;
const EYE_RANGE: f32 = 0.2;
const MOUTH_RANGE: f32 = 0.6;
/// Least time between detections, and the least it can be as a multiple of how long one takes,
/// so a slow GPU isn't kept busy with this.
const MIN_INTERVAL: Duration = Duration::from_millis(20);
const INTERVAL_PER_RUN: f32 = 3.0;
const DETAILED_INTERVAL_PER_RUN: f32 = 1.5;
/// Reports waiting for the player to collect them.
const MOST_WAITING: usize = 16;
/// How much a box must overlap a face already being tracked to be that face.
const SAME_FACE: f32 = 0.2;
/// Detections in a row before a new face shows, which filters out one-off false ones.
const CONFIRM: u32 = 2;
/// How long a face that's gone missing keeps its slot (seconds), so it has the same one if it's
/// found again. It isn't shown meanwhile: only a single missed detection, with the face found
/// either side of it, is filled in.
const HOLD: f64 = 0.3;
/// How much the detections around a report count, from the report's own outwards: reports
/// are this many detections behind the newest, less one. A closer look takes longer, so it
/// looks only one detection either side, to keep the picture from being held back further.
const WEIGHTS: [f32; 3] = [3.0, 2.0, 1.0];
const DETAILED_REACH: usize = 2;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Off,
    Starting,
    Ready {
        adapter: String,
        /// How long the last detection took all told, how much of that went on the meshes and
        /// on which way heads point, and how many faces are showing.
        ms: f32,
        mesh: f32,
        head: f32,
        faces: usize,
    },
    Failed(String),
}

impl Status {
    pub fn describe(&self) -> String {
        match self {
            Status::Off => "off".into(),
            Status::Starting => "starting…".into(),
            Status::Ready { adapter, ms, mesh, head, faces } if *mesh + *head > 0.0 => format!("{faces} found, {ms:.0} ms a frame (mesh {mesh:.0}, head {head:.0}) on {adapter}"),
            Status::Ready { adapter, ms, faces, .. } => format!("{faces} found, {ms:.0} ms a frame on {adapter}"),
            Status::Failed(e) => format!("⚠ {e}"),
        }
    }
}

/// A face as effects get it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub coords: [f32; COORDS],
    /// 1 where the face was found, 0 where it wasn't (in between when blended between frames).
    pub seen: f32,
}

pub type Slots = [Option<Shown>; MAX_FACES];

/// The faces in one video frame.
#[derive(Clone, Debug)]
pub struct Report {
    /// The frame's position in the video (seconds).
    pub time: f32,
    /// The first after a cut or seek: don't blend it with the one before.
    pub after_cut: bool,
    pub slots: Slots,
}

struct Input {
    /// RGBA, bottom row first (as read back from GL), `WIDTH` × `HEIGHT`.
    rgba: Vec<u8>,
    /// How many times `WIDTH` × `HEIGHT` it is.
    scale: usize,
    /// How much of that the picture fills, from the top left.
    filled: (f32, f32),
    /// The frame's position in the video (seconds).
    time: f32,
    /// There's been a scene cut since the last one handed over.
    cut: bool,
}

#[derive(Default)]
pub struct State {
    status: Status,
    /// Whether anything's asked for the closer look lately.
    detail: bool,
    input: Option<Input>,
    busy: bool,
    reports: VecDeque<Report>,
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

impl Link {
    /// An effect wants faces, and maybe a closer look at them: starts the worker if it isn't
    /// running. Call every frame.
    pub fn wanted(&self, detail: bool) {
        self.want("faces", |state| state.detail = detail, |link| link.work());
    }

    /// How many times `WIDTH` × `HEIGHT` frames should be.
    pub fn scale(&self) -> usize {
        if self.lock().detail { DETAIL_SCALE } else { 1 }
    }

    /// Whether the worker would take a frame now.
    pub fn ready_for_frame(&self) -> bool {
        let state = self.lock();
        matches!(state.status, Status::Ready { .. }) && state.input.is_none() && !state.busy
    }

    /// Hands over a frame: RGBA, bottom row first, `scale` times `WIDTH` × `HEIGHT`, with the
    /// picture filling `filled` of it from the top left. `time` is its position in the video.
    pub fn submit(&self, rgba: Vec<u8>, scale: usize, filled: (f32, f32), time: f32, cut: bool) {
        self.lock().input = Some(Input { rgba, scale, filled, time, cut });
        self.notify();
    }

    /// The reports that have come in since the last call, oldest first.
    pub fn take_reports(&self) -> Vec<Report> {
        self.lock().reports.drain(..).collect()
    }

    fn publish(&self, reports: impl IntoIterator<Item = Report>) {
        push_capped(&mut self.lock().reports, reports, MOST_WAITING);
    }

    fn work(&self) {
        let mut face = match Face::new() {
            Ok(face) => face,
            Err(e) => {
                log::warn!("faces: {e:#}");
                let mut state = self.lock();
                state.status = Status::Failed(format!("{e:#}"));
                state.running = false;
                return;
            }
        };
        log::info!("faces: running on {}", face.adapter());
        self.lock().status = Status::Ready { adapter: face.adapter().to_owned(), ms: 0.0, mesh: 0.0, head: 0.0, faces: 0 };

        let start = Instant::now();
        let mut tracker = Tracker::default();
        let mut closer = Closer::default();
        let mut rgb = vec![0u8; WIDTH * HEIGHT * 3];
        let mut large = Vec::new();
        let mut last_time: Option<f32> = None;
        let mut after_cut = true;
        while let Some(input) = self.next(|state| {
            let input = state.input.take()?;
            state.busy = true;
            Some(input)
        }) {
            let detailed = input.scale > 1;
            if detailed {
                large.resize(WIDTH * HEIGHT * input.scale * input.scale * 3, 0);
                top_down(&input.rgba, WIDTH * input.scale, HEIGHT * input.scale, &mut large);
                halve(&large, WIDTH * 2, HEIGHT * 2, &mut rgb);
            } else {
                top_down(&input.rgba, WIDTH, HEIGHT, &mut rgb);
            }
            let seeked = last_time.is_some_and(|t| input.time < t || input.time > t + 1.0);
            if input.cut || seeked || tracker.detailed != detailed {
                self.publish(tracker.finish());
                tracker = Tracker { detailed, ..Default::default() };
                closer = Closer::default();
                after_cut = true;
            }
            last_time = Some(input.time);

            let started = Instant::now();
            let turn = crate::gpu::turn(crate::gpu::Net::Faces);
            let found = face.run(&rgb).and_then(|found| {
                let mut coords: Vec<[f32; COORDS]> = found.iter().map(|d| coords(d, input.filled)).collect();
                if detailed {
                    closer.look(&mut face, &large, &found, input.filled, &mut coords)?;
                }
                Ok(coords)
            });
            let found = match found {
                Ok(found) => found,
                Err(e) => {
                    log::warn!("faces: {e:#}");
                    let mut state = self.lock();
                    state.status = Status::Failed(format!("{e:#}"));
                    state.busy = false;
                    break;
                }
            };
            let ms = turn.ms();
            drop(turn);
            let report = tracker.update(&found, start.elapsed().as_secs_f64(), input.time).map(|mut report| {
                report.after_cut = std::mem::take(&mut after_cut);
                report
            });
            self.publish(report);
            self.lock().status = Status::Ready { adapter: face.adapter().to_owned(), ms, mesh: closer.mesh_ms, head: closer.head_ms, faces: tracker.showing() };
            let per_run = if detailed { DETAILED_INTERVAL_PER_RUN } else { INTERVAL_PER_RUN };
            let own = MIN_INTERVAL.max(Duration::from_secs_f32(ms * per_run / 1000.0));
            std::thread::sleep(crate::gpu::rest(crate::gpu::Net::Faces, own).saturating_sub(started.elapsed()));
            self.lock().busy = false;
        }
        drop(face);
        log::info!("faces: stopped");
        self.stop();
    }
}

/// RGBA with its rows bottom-up, as read back from GL, to RGB with them top-down.
pub(crate) fn top_down(rgba: &[u8], width: usize, height: usize, rgb: &mut [u8]) {
    for y in 0..height {
        let src = &rgba[(height - 1 - y) * width * 4..][..width * 4];
        for (x, px) in src.as_chunks::<4>().0.iter().enumerate() {
            rgb[(y * width + x) * 3..][..3].copy_from_slice(&px[..3]);
        }
    }
}

/// RGB at half the size, averaging each two by two.
fn halve(from: &[u8], width: usize, height: usize, to: &mut [u8]) {
    for y in 0..height / 2 {
        for x in 0..width / 2 {
            for c in 0..3 {
                let at = |yy: usize, xx: usize| u16::from(from[(yy * width + xx) * 3 + c]);
                let sum = at(2 * y, 2 * x) + at(2 * y, 2 * x + 1) + at(2 * y + 1, 2 * x) + at(2 * y + 1, 2 * x + 1);
                to[(y * (width / 2) + x) * 3 + c] = ((sum + 2) / 4) as u8;
            }
        }
    }
}

/// A square of a picture, turned to level a face: where to look for a mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Roi {
    x: f32,
    y: f32,
    side: f32,
    angle: f32,
}

impl Roi {
    /// From a detection (in the larger picture's pixels).
    fn around(d: &Detection) -> Self {
        let ([rx, ry], [lx, ly]) = (d.points[0], d.points[1]);
        Self { x: d.x + d.width / 2.0, y: d.y + d.height / 2.0, side: d.width.max(d.height) * ROI_MARGIN, angle: (ly - ry).atan2(lx - rx) }
    }

    /// Fitted around a mesh, as MediaPipe does it (in the same pixels).
    fn fitted(points: &[[f32; 3]]) -> Self {
        let angle = (points[263][1] - points[33][1]).atan2(points[263][0] - points[33][0]);
        let (sin, cos) = (-angle).sin_cos();
        let (mut low, mut high) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in points {
            let turned = [p[0] * cos - p[1] * sin, p[0] * sin + p[1] * cos];
            for i in 0..2 {
                low[i] = low[i].min(turned[i]);
                high[i] = high[i].max(turned[i]);
            }
        }
        let middle = [(low[0] + high[0]) / 2.0, (low[1] + high[1]) / 2.0];
        let (sin, cos) = angle.sin_cos();
        Self { x: middle[0] * cos - middle[1] * sin, y: middle[0] * sin + middle[1] * cos, side: (high[0] - low[0]).max(high[1] - low[1]) * ROI_MARGIN, angle }
    }

    /// The picture's position of a point (x, y) in a square `size` across.
    fn place(&self, x: f32, y: f32, size: usize) -> [f32; 2] {
        let k = self.side / size as f32;
        let (sin, cos) = self.angle.sin_cos();
        let (u, v) = ((x - size as f32 / 2.0) * k, (y - size as f32 / 2.0) * k);
        [self.x + u * cos - v * sin, self.y + u * sin + v * cos]
    }
}

/// `rgb` under an affine map from a square of `size` to the picture, given as where the
/// square's corner and its next pixel across and down land, averaging a few spots a pixel when
/// shrinking by `shrink` so as not to miss anything. Black outside the picture.
fn resample(rgb: &[u8], width: usize, height: usize, size: usize, shrink: f32, out: &mut [u8], to: impl Fn(f32, f32) -> [f32; 2]) {
    let taps = ((shrink + 0.5) as usize).clamp(1, 3);
    let origin = to(0.0, 0.0);
    let (across, down) = (to(1.0, 0.0), to(0.0, 1.0));
    let (ux, uy) = (across[0] - origin[0], across[1] - origin[1]);
    let (vx, vy) = (down[0] - origin[0], down[1] - origin[1]);
    let (w, h) = (width as i32, height as i32);
    let part = 1.0 / taps as f32;
    let mut sum = [0u32; 3];
    for v in 0..size {
        for u in 0..size {
            sum.fill(0);
            for tv in 0..taps {
                for tu in 0..taps {
                    let (cu, cv) = (u as f32 + (tu as f32 + 0.5) * part, v as f32 + (tv as f32 + 0.5) * part);
                    // In 1/256ths of a pixel, measured from pixel centres.
                    let fx = ((origin[0] + ux * cu + vx * cv - 0.5) * 256.0) as i32;
                    let fy = ((origin[1] + uy * cu + vy * cv - 0.5) * 256.0) as i32;
                    let (x0, y0) = (fx >> 8, fy >> 8);
                    if x0 < 0 || y0 < 0 || x0 + 1 >= w || y0 + 1 >= h {
                        continue;
                    }
                    let (ax, ay) = ((fx & 255) as u32, (fy & 255) as u32);
                    let top = (y0 * w + x0) as usize * 3;
                    let row = width * 3;
                    for (c, sum) in sum.iter_mut().enumerate() {
                        let (a, b) = (u32::from(rgb[top + c]), u32::from(rgb[top + 3 + c]));
                        let (d, e) = (u32::from(rgb[top + row + c]), u32::from(rgb[top + row + 3 + c]));
                        *sum += ((a * (256 - ax) + b * ax) * (256 - ay) + (d * (256 - ax) + e * ax) * ay) >> 8;
                    }
                }
            }
            let count = (taps * taps) as u32 * 256;
            for c in 0..3 {
                out[(v * size + u) * 3 + c] = ((sum[c] + count / 2) / count) as u8;
            }
        }
    }
}

/// How open the person's right eye, left eye and mouth are, from their mesh (in pixels).
fn openness(points: &[[f32; 3]]) -> [f32; 3] {
    let gap = |a: usize, b: usize, c: usize, d: usize| {
        let far = |i: usize, j: usize| (points[i][0] - points[j][0]).hypot(points[i][1] - points[j][1]);
        far(a, b) / far(c, d).max(1e-3)
    };
    let eye = |g: f32| ((g - EYE_SHUT) / EYE_RANGE).clamp(0.0, 1.0);
    [eye(gap(159, 145, 33, 133)), eye(gap(386, 374, 362, 263)), (gap(13, 14, 61, 291) / MOUTH_RANGE).clamp(0.0, 1.0)]
}

/// Which way a head points, from its rotation, or nothing if it looks to be facing away
/// (where the network's answers are often nonsense).
fn facing(rotation: &[[f32; 3]; 3]) -> Option<[f32; 3]> {
    let forward = rotation[2];
    (forward[2] > 0.0).then_some(forward)
}

/// A face looked at closely last time.
#[derive(Clone, Copy)]
struct Seen {
    detection: Detection,
    roi: Option<Roi>,
    head_at: u64,
}

fn same_detection(a: &Detection, b: &Detection) -> f32 {
    let w = ((a.x + a.width).min(b.x + b.width) - a.x.max(b.x)).max(0.0);
    let h = ((a.y + a.height).min(b.y + b.height) - a.y.max(b.y)).max(0.0);
    w * h / (a.width * a.height + b.width * b.height - w * h).max(1e-6)
}

/// The closer look: a mesh of each of the larger faces, and the angle of one head a detection,
/// taking turns. The smoothing's window fills in the turns between, without any lag.
#[derive(Default)]
struct Closer {
    last: Vec<Seen>,
    detection: u64,
    /// How long the last look spent on meshes and on heads (ms).
    mesh_ms: f32,
    head_ms: f32,
}

/// How many detections between one head being looked at and the next.
const HEAD_EVERY: u64 = 2;

impl Closer {
    /// Fills in `out` for each of `found`, looking at `large`, this same frame at
    /// `DETAIL_SCALE` times the detector's size.
    fn look(&mut self, face: &mut Face, large: &[u8], found: &[Detection], filled: (f32, f32), out: &mut [[f32; COORDS]]) -> anyhow::Result<()> {
        self.detection += 1;
        self.mesh_ms = 0.0;
        self.head_ms = 0.0;
        let scale = DETAIL_SCALE as f32;
        let (width, height) = (WIDTH * DETAIL_SCALE, HEIGHT * DETAIL_SCALE);
        let across = (filled.0 * width as f32).max(1.0);
        let down = (filled.1 * height as f32).max(1.0);
        let mut order: Vec<usize> = (0..found.len()).filter(|i| found[*i].width * scale >= SMALLEST_MESH).collect();
        order.sort_by(|a, b| found[*b].width.total_cmp(&found[*a].width));
        order.truncate(MOST_MESHES);

        let mut square = vec![0u8; MESH_SIZE * MESH_SIZE * 3];
        let mut seen: Vec<Seen> = Vec::new();
        for &i in &order {
            let mut d = found[i];
            d.x *= scale;
            d.y *= scale;
            d.width *= scale;
            d.height *= scale;
            d.points = d.points.map(|[x, y]| [x * scale, y * scale]);
            let before = self.last.iter().map(|s| (same_detection(&s.detection, &d), s)).filter(|(amount, _)| *amount > 0.3).max_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, s)| *s);
            // Where it was last time, moved along with what was found in this frame.
            let moved = before.and_then(|b| b.roi.map(|roi| Roi { x: roi.x + d.x + d.width / 2.0 - b.detection.x - b.detection.width / 2.0, y: roi.y + d.y + d.height / 2.0 - b.detection.y - b.detection.height / 2.0, ..roi }));
            let mesh_ms = &mut self.mesh_ms;
            let mut run = |roi: Roi| -> anyhow::Result<(flex_depth::face::Mesh, Vec<[f32; 3]>)> {
                resample(large, width, height, MESH_SIZE, roi.side / MESH_SIZE as f32, &mut square, |u, v| roi.place(u, v, MESH_SIZE));
                let timer = Instant::now();
                let mesh = face.mesh(&square)?;
                *mesh_ms += timer.elapsed().as_secs_f32() * 1000.0;
                let placed = mesh.points.iter().map(|p| { let [x, y] = roi.place(p[0], p[1], MESH_SIZE); [x, y, p[2] * roi.side / MESH_SIZE as f32] }).collect();
                Ok((mesh, placed))
            };
            let (mesh, placed) = match moved {
                Some(roi) => run(roi)?,
                None => {
                    let first = run(Roi::around(&d))?;
                    if first.0.present > 0.0 { run(Roi::fitted(&first.1))? } else { first }
                }
            };
            let roi = (mesh.present > 0.0).then(|| Roi::fitted(&placed));
            if mesh.present > 0.0 {
                let c = &mut out[i];
                for (n, p) in placed.iter().enumerate() {
                    c[MESH_AT + n * 3] = p[0] / across;
                    c[MESH_AT + n * 3 + 1] = p[1] / down;
                    c[MESH_AT + n * 3 + 2] = p[2] / across;
                }
                c[OPEN_AT..OPEN_AT + 3].copy_from_slice(&openness(&placed));
                c[HAS_MESH] = 1.0;
            }
            seen.push(Seen { detection: d, roi, head_at: before.map_or(0, |b| b.head_at) });
        }
        let turn = self.detection.is_multiple_of(HEAD_EVERY) || seen.iter().any(|s| s.head_at == 0);
        if let Some((n, s)) = seen.iter_mut().enumerate().filter(|_| turn).min_by_key(|(_, s)| s.head_at) {
            s.head_at = self.detection;
            let d = s.detection;
            let mut square = vec![0u8; HEAD_SIZE * HEAD_SIZE * 3];
            let (kx, ky) = (d.width / HEAD_SIZE as f32, d.height / HEAD_SIZE as f32);
            resample(large, width, height, HEAD_SIZE, kx.max(ky), &mut square, |u, v| [d.x + u * kx, d.y + v * ky]);
            let timer = Instant::now();
            let turned = face.head(&square)?;
            self.head_ms += timer.elapsed().as_secs_f32() * 1000.0;
            if let Some(forward) = facing(&turned) {
                let c = &mut out[order[n]];
                c[TURN_AT..TURN_AT + 3].copy_from_slice(&forward);
                c[HAS_TURN] = 1.0;
            }
        }
        self.last = seen;
        Ok(())
    }
}

/// A detection's coordinates over the picture, which fills `filled` of what the model saw.
fn coords(d: &Detection, filled: (f32, f32)) -> [f32; COORDS] {
    let (sx, sy) = (1.0 / (filled.0 * WIDTH as f32).max(1.0), 1.0 / (filled.1 * HEIGHT as f32).max(1.0));
    let mut c = [0.0; COORDS];
    c[..4].copy_from_slice(&[d.x * sx, d.y * sy, (d.x + d.width) * sx, (d.y + d.height) * sy]);
    for (n, [x, y]) in d.points.iter().enumerate() {
        c[4 + n * 2] = x * sx;
        c[5 + n * 2] = y * sy;
    }
    c
}

fn overlap(a: &[f32; COORDS], b: &[f32; COORDS]) -> f32 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = w * h;
    inter / ((a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter).max(1e-9)
}

/// A face being followed.
struct Track {
    /// What's been detected lately: which detection, and where.
    recent: VecDeque<(u64, [f32; COORDS])>,
    updated: f64,
    hits: u32,
}

impl Track {
    fn new(detection: u64, face: [f32; COORDS], now: f64) -> Self {
        Self { recent: VecDeque::from([(detection, face)]), updated: now, hits: 1 }
    }

    fn newest(&self) -> &[f32; COORDS] {
        &self.recent[self.recent.len() - 1].1
    }

    /// Whether it's to be shown at `detection`: confirmed, and found there, or found on both
    /// sides of a single miss there.
    fn shown_at(&self, detection: u64) -> bool {
        let found = |d: u64| self.recent.iter().any(|(at, _)| *at == d);
        self.hits >= CONFIRM && (found(detection) || (detection > 0 && found(detection - 1) && found(detection + 1)))
    }

    fn follow(&mut self, detection: u64, target: &[f32; COORDS], now: f64) {
        self.recent.push_back((detection, *target));
        if self.recent.len() > 2 * WEIGHTS.len() - 1 {
            self.recent.pop_front();
        }
        self.updated = now;
        self.hits += 1;
    }

    /// Where it was at `detection`: an average of the detections either side, which leaves
    /// movement where it was and only takes out the jitter. A mesh or direction that wasn't
    /// there for some of them is averaged over the ones it was there for. `reach` is how many
    /// of `WEIGHTS` to use.
    fn at(&self, detection: u64, reach: usize) -> [f32; COORDS] {
        let (mut sum, mut total, mut with_mesh, mut with_turn) = ([0.0; COORDS], 0.0, 0.0, 0.0);
        for (at, face) in &self.recent {
            let Some(weight) = WEIGHTS[..reach].get(at.abs_diff(detection) as usize) else { continue };
            total += weight;
            let has_mesh = face[HAS_MESH] > 0.0;
            let has_turn = face[HAS_TURN] > 0.0;
            with_mesh += if has_mesh { *weight } else { 0.0 };
            with_turn += if has_turn { *weight } else { 0.0 };
            for (n, (sum, v)) in sum.iter_mut().zip(face).enumerate() {
                let counts = match n {
                    n if n < MESH_AT => true,
                    n if n < TURN_AT => has_mesh,
                    n if n < HAS_MESH => has_turn,
                    _ => false,
                };
                if counts {
                    *sum += v * weight;
                }
            }
        }
        if total == 0.0 {
            return *self.newest(); // being held after going missing
        }
        let mut out = sum;
        for (n, v) in out.iter_mut().enumerate() {
            *v /= match n {
                n if n < MESH_AT => total,
                n if n < TURN_AT => f32::max(with_mesh, 1e-6),
                _ => f32::max(with_turn, 1e-6),
            };
        }
        out[HAS_MESH] = with_mesh / total;
        // Heads take turns being looked at: when none is near enough to average, use the
        // nearest. Which way a head points changes slowly compared to how fast they come.
        if with_turn == 0.0
            && let Some((_, face)) = self.recent.iter().filter(|(_, f)| f[HAS_TURN] > 0.0).min_by_key(|(at, _)| at.abs_diff(detection))
        {
            out[TURN_AT..HAS_MESH].copy_from_slice(&face[TURN_AT..HAS_MESH]);
            with_turn = 1.0;
        }
        out[HAS_TURN] = if with_turn > 0.0 { 1.0 } else { 0.0 };
        out
    }
}

/// Keeps faces in their slots from one detection to the next.
#[derive(Default)]
struct Tracker {
    slots: [Option<Track>; MAX_FACES],
    detections: u64,
    /// Detections not reported yet: which, and the frame's position.
    unreported: VecDeque<(u64, f32)>,
    /// Having a closer look, so reporting sooner.
    detailed: bool,
}

impl Tracker {
    /// `found` is what's been detected at `now` (seconds) in the frame at `time` in the video.
    /// Returns the report for an earlier frame, now that the frames after it are known.
    fn update(&mut self, found: &[[f32; COORDS]], now: f64, time: f32) -> Option<Report> {
        let detection = self.detections;
        self.detections += 1;

        // The most overlapping pairs first, so a face can't be taken by a worse match.
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (s, track) in self.slots.iter().enumerate() {
            let Some(track) = track else { continue };
            for (f, face) in found.iter().enumerate() {
                let amount = overlap(track.newest(), face);
                if amount >= SAME_FACE {
                    pairs.push((amount, s, f));
                }
            }
        }
        pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
        let (mut slot_taken, mut face_taken) = ([false; MAX_FACES], vec![false; found.len()]);
        for (_, s, f) in pairs {
            if !slot_taken[s] && !face_taken[f] {
                slot_taken[s] = true;
                face_taken[f] = true;
                if let Some(track) = &mut self.slots[s] {
                    track.follow(detection, &found[f], now);
                }
            }
        }
        for (face, _) in found.iter().zip(&face_taken).filter(|(_, taken)| !**taken) {
            if let Some(free) = self.slots.iter_mut().find(|s| s.is_none()) {
                *free = Some(Track::new(detection, *face, now));
            }
        }

        for slot in &mut self.slots {
            if slot.as_ref().is_some_and(|track| now - track.updated > HOLD) {
                *slot = None;
            }
        }

        self.unreported.push_back((detection, time));
        (self.unreported.len() >= self.reach()).then(|| self.report()).flatten()
    }

    fn reach(&self) -> usize {
        if self.detailed { DETAILED_REACH } else { WEIGHTS.len() }
    }

    fn report(&mut self) -> Option<Report> {
        let (detection, time) = self.unreported.pop_front()?;
        let reach = self.reach();
        let mut slots: Slots = [None; MAX_FACES];
        for (slot, out) in self.slots.iter().zip(&mut slots) {
            *out = slot.as_ref().map(|track| Shown { coords: track.at(detection, reach), seen: if track.shown_at(detection) { 1.0 } else { 0.0 } });
        }
        Some(Report { time, after_cut: false, slots })
    }

    /// The frames not reported yet, as there won't be any more after them.
    fn finish(&mut self) -> Vec<Report> {
        std::iter::from_fn(|| self.report()).collect()
    }

    fn showing(&self) -> usize {
        self.slots.iter().flatten().filter(|t| t.hits >= CONFIRM && t.recent.back().is_some_and(|(at, _)| *at + 1 >= self.detections)).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(x: f32, y: f32) -> [f32; COORDS] {
        let mut c = [0.0; COORDS];
        c[..4].copy_from_slice(&[x, y, x + 0.1, y + 0.2]);
        c
    }

    const STEP: f64 = 1.0 / 24.0;

    /// The tracker fed detections `from..to`, returning its reports in order.
    fn feed(t: &mut Tracker, from: usize, to: usize, found: impl Fn(usize) -> Vec<[f32; COORDS]>) -> Vec<Report> {
        (from..to).filter_map(|i| t.update(&found(i), i as f64 * STEP, i as f32 * STEP as f32)).collect()
    }

    #[test]
    fn a_face_shows_from_its_first_frame_once_confirmed() {
        let mut t = Tracker::default();
        let reports = feed(&mut t, 0, 16, |_| vec![face(0.2, 0.2)]);
        assert_eq!(reports[0].time, 0.0, "reports start at the first frame");
        assert!(reports.iter().all(|r| r.slots[0].unwrap().seen == 1.0), "known to be real by the time it's reported");
    }

    #[test]
    fn a_one_off_false_detection_never_shows_and_goes_away() {
        let mut t = Tracker::default();
        let reports = feed(&mut t, 0, 24, |i| if i == 0 { vec![face(0.7, 0.7)] } else { vec![] });
        assert!(reports.iter().all(|r| r.slots[0].is_none_or(|s| s.seen == 0.0)));
        assert!(t.slots[0].is_none());
    }

    #[test]
    fn faces_keep_their_slots_whatever_order_they_come_in() {
        let mut t = Tracker::default();
        let reports = feed(&mut t, 0, 6, |i| if i % 2 == 0 { vec![face(0.1, 0.1), face(0.6, 0.6)] } else { vec![face(0.6, 0.6), face(0.1, 0.1)] });
        let last = &reports.last().unwrap().slots;
        assert!((last[0].unwrap().coords[0] - 0.1).abs() < 0.01);
        assert!((last[1].unwrap().coords[0] - 0.6).abs() < 0.01);
    }

    #[test]
    fn a_single_miss_is_filled_in_but_a_lost_face_goes_at_once() {
        let mut t = Tracker::default();
        let reports = feed(&mut t, 0, 40, |i| if i < 12 || i == 13 { vec![face(0.2, 0.2)] } else { vec![] });
        let seen = |i: usize| reports[i].slots[0].map_or(0.0, |s| s.seen);
        assert_eq!(seen(12), 1.0, "just one miss");
        assert_eq!(seen(13), 1.0);
        assert_eq!(seen(14), 0.0, "lost, so gone straight away");
        assert!(t.slots[0].is_none(), "and its slot freed after a while");
    }

    #[test]
    fn jitter_is_steadied_and_movement_has_no_lag() {
        let mut t = Tracker::default();
        let reports = feed(&mut t, 0, 60, |i| vec![face(0.3 + if i % 2 == 0 { 0.004 } else { -0.004 }, 0.3)]);
        let x = reports.last().unwrap().slots[0].unwrap().coords[0];
        assert!((x - 0.3).abs() < 0.0015, "{x}");
        // Moving steadily: each report is exactly where the face was at that frame.
        let mut t = Tracker::default();
        let at = |i: usize| 0.2 + i as f32 * 0.004;
        let reports = feed(&mut t, 0, 30, |i| vec![face(at(i), 0.3)]);
        for (i, report) in reports.iter().enumerate().skip(2) {
            assert!((report.slots[0].unwrap().coords[0] - at(i)).abs() < 1e-4, "frame {i}");
            assert!((report.time - i as f32 * STEP as f32).abs() < 1e-4);
        }
    }

    #[test]
    fn nothing_is_left_unreported_at_a_cut() {
        let mut t = Tracker::default();
        let reports = feed(&mut t, 0, 6, |_| vec![face(0.2, 0.2)]);
        assert_eq!(reports.len(), 4);
        assert_eq!(t.finish().len(), 2);
    }

    #[test]
    fn coordinates_are_over_the_picture_not_the_padding() {
        let d = Detection { x: 160.0, y: 44.0, width: 32.0, height: 44.0, score: 1.0, points: [[160.0, 44.0]; 5] };
        // A picture filling half the width and all the height.
        let c = coords(&d, (0.5, 1.0));
        assert!((c[0] - 0.5).abs() < 1e-5 && (c[1] - 0.125).abs() < 1e-5, "{c:?}");
        assert!((c[2] - 0.6).abs() < 1e-5, "{c:?}");
    }

    #[test]
    fn a_square_maps_back_to_the_picture() {
        let roi = Roi { x: 100.0, y: 50.0, side: 40.0, angle: std::f32::consts::FRAC_PI_2 };
        assert_eq!(roi.place(128.0, 128.0, 256), [100.0, 50.0]);
        let [x, y] = roi.place(256.0, 128.0, 256);
        assert!((x - 100.0).abs() < 1e-3 && (y - 70.0).abs() < 1e-3, "{x} {y}");
    }

    #[test]
    fn a_fitted_square_is_level_with_the_eyes_and_around_the_points() {
        let mut points = vec![[10.0, 20.0, 0.0]; MESH_POINTS];
        points[33] = [0.0, 0.0, 0.0];
        points[263] = [40.0, 0.0, 0.0];
        let roi = Roi::fitted(&points);
        assert_eq!(roi.angle, 0.0);
        assert!((roi.x - 20.0).abs() < 1e-3 && (roi.y - 10.0).abs() < 1e-3, "{roi:?}");
        assert!((roi.side - 40.0 * ROI_MARGIN).abs() < 1e-3, "{roi:?}");
    }

    #[test]
    fn eyes_and_mouths_open_and_shut() {
        let mut points = vec![[0.0, 0.0, 0.0]; MESH_POINTS];
        points[133] = [10.0, 0.0, 0.0];
        points[263] = [10.0, 0.0, 0.0];
        points[291] = [10.0, 0.0, 0.0];
        points[145] = [0.0, 3.0, 0.0]; // right eye wide open
        points[374] = [0.0, 0.5, 0.0]; // left eye shut
        points[14] = [0.0, 6.0, 0.0]; // mouth as open as it gets
        let [right, left, mouth] = openness(&points);
        assert_eq!((right, left, mouth), (1.0, 0.0, 1.0));
    }

    #[test]
    fn a_head_facing_away_says_nothing() {
        assert_eq!(facing(&[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]), Some([0.0, 0.0, 1.0]));
        assert_eq!(facing(&[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]]), None);
    }

    #[test]
    fn a_mesh_is_averaged_only_over_the_frames_that_had_one() {
        let with_mesh = |i: usize| {
            let mut c = face(0.2, 0.2);
            if i.is_multiple_of(2) {
                c[MESH_AT] = 0.5;
                c[HAS_MESH] = 1.0;
                c[HAS_TURN] = 1.0;
                c[TURN_AT] = 0.4;
            }
            c
        };
        let mut t = Tracker { detailed: true, ..Default::default() };
        let reports = feed(&mut t, 0, 10, |i| vec![with_mesh(i)]);
        for report in &reports {
            let c = report.slots[0].unwrap().coords;
            assert!((c[MESH_AT] - 0.5).abs() < 1e-5, "not pulled towards nothing");
            assert!((c[TURN_AT] - 0.4).abs() < 1e-5);
            assert_eq!(c[HAS_TURN], 1.0);
            assert!(c[HAS_MESH] > 0.3 && c[HAS_MESH] < 0.8);
        }
    }

    #[test]
    fn a_direction_is_kept_between_the_turns_heads_take() {
        let with_turn = |i: usize| {
            let mut c = face(0.2, 0.2);
            if i == 0 {
                c[TURN_AT] = 0.4;
                c[HAS_TURN] = 1.0;
            }
            c
        };
        let mut t = Tracker { detailed: true, ..Default::default() };
        let reports = feed(&mut t, 0, 5, |i| vec![with_turn(i)]);
        let c = reports.last().unwrap().slots[0].unwrap().coords;
        assert_eq!(c[HAS_TURN], 1.0);
        assert!((c[TURN_AT] - 0.4).abs() < 1e-5);
    }

    #[test]
    fn resampling_lands_on_the_right_pixels() {
        // A picture that's dark on the left and light on the right, shown as is.
        let mut rgb = vec![0u8; 8 * 8 * 3];
        for (i, px) in rgb.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            *px = [if i % 8 < 4 { 10 } else { 250 }; 3];
        }
        let mut out = vec![0u8; 4 * 4 * 3];
        resample(&rgb, 8, 8, 4, 1.0, &mut out, |u, v| [u + 2.0, v + 2.0]);
        assert_eq!(out[4 * 3], 10);
        assert_eq!(out[7 * 3], 250);
    }

    #[test]
    fn a_closer_look_reports_sooner() {
        let mut t = Tracker { detailed: true, ..Default::default() };
        let reports = feed(&mut t, 0, 6, |_| vec![face(0.2, 0.2)]);
        assert_eq!(reports.len(), 5);
    }

    #[test]
    fn halving_averages() {
        let mut to = [0u8; 3];
        halve(&[0, 0, 0, 40, 40, 40, 80, 80, 80, 200, 200, 200], 2, 2, &mut to);
        assert_eq!(to, [80, 80, 80]);
    }
}
