//! What sort of shot the picture is in, and what's going on across the last few: built from what
//! the depth, face and movement networks and the sound already say, for effects with
//! `scene = true`. Everything here looks backward, so scene scores take a while to catch up.

use std::collections::VecDeque;
use std::sync::Mutex;

/// How big a face is, against the picture's height, at the start of each band.
const EXTREME: f32 = 0.6;
const CLOSE: f32 = 0.3;
const MEDIUM: f32 = 0.12;
/// With no face to go by: a person this many times taller than their face would be.
const PERSON_PER_FACE: f32 = 7.0;
/// How long measurements take to settle within a shot (seconds).
const SIZE_SECONDS: f32 = 0.4;
const CAMERA_SECONDS: f32 = 0.2;
const STEADY_SECONDS: f32 = 1.0;
/// How much the camera's wobble around its steady path (picture heights a second) counts as
/// fully handheld.
const HANDHELD: f32 = 0.08;
/// How far back scene scores look, and the least they'll divide by (seconds).
const WINDOW: f32 = 30.0;
const SHORTEST_WINDOW: f32 = 5.0;
const SCENE_EVERY: f32 = 0.25;
/// Clues kept for when the picture reaches them, and how far ahead (seconds) to use them.
const MOST_CLUES: usize = 16;
const LOOK_AHEAD: f32 = 0.3;
const JUST_STARTED: f32 = 0.15;
/// How far back to look for a steady beat (seconds), and how quickly the film's usual loudness
/// follows the sound.
const BEATS_WINDOW: f32 = 15.0;
const USUAL_LEVEL_SECONDS: f32 = 120.0;
/// How different two shots' small pictures can be, on average, and still be the same setup.
const SAME_SETUP: f32 = 0.05;
/// The sizes of the small picture of a shot.
const PICTURE_W: usize = 8;
const PICTURE_H: usize = 4;
/// A face about this much of the picture's height goes with people covering all of it, as
/// square roots of areas go.
const PEOPLE_FACE: f32 = 0.55;

/// What the depth worker can tell from a picture.
#[derive(Clone, Copy, Debug, Default)]
pub struct Clues {
    /// How much of the picture's height has people in it, and how much of the picture they cover.
    pub person_height: f32,
    pub people: f32,
    /// A very small version of the picture, as brightness differences from its average, for
    /// recognising when the film goes back to the same setup.
    pub picture: [f32; PICTURE_W * PICTURE_H],
    /// How much of the picture is sky.
    pub sky: f32,
    /// How much more of the picture's sides are near than its middle.
    pub near_sides: f32,
}

impl Clues {
    /// From depth (0 far to 1 near), masks and the picture (RGB), all with rows from the top.
    pub fn of(depth: &[f32], person: &[f32], sky: &[f32], rgb: &[u8], (w, h): (usize, usize)) -> Self {
        let filled = |x: &f32| *x > 0.43;
        let rows: Vec<usize> = person.chunks(w).enumerate().filter(|(_, row)| row.iter().filter(|p| filled(p)).count() * 200 >= w).map(|(y, _)| y).collect();
        let person_height = match (rows.first(), rows.last()) {
            (Some(a), Some(b)) => (b - a + 1) as f32 / h as f32,
            _ => 0.0,
        };
        let people = person.iter().filter(|p| filled(p)).count() as f32 / person.len().max(1) as f32;
        let sky = sky.iter().filter(|s| filled(s)).count() as f32 / sky.len().max(1) as f32;
        let mut picture = [0.0f32; PICTURE_W * PICTURE_H];
        for (i, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
            let cell = (i / w * PICTURE_H / h) * PICTURE_W + (i % w) * PICTURE_W / w;
            picture[cell] += (f32::from(px[0]) * 0.3 + f32::from(px[1]) * 0.59 + f32::from(px[2]) * 0.11) / 255.0 / (w * h / (PICTURE_W * PICTURE_H)).max(1) as f32;
        }
        let average = picture.iter().sum::<f32>() / picture.len() as f32;
        picture.iter_mut().for_each(|v| *v -= average);
        let side = w / 4;
        let near = |from: usize, to: usize| depth.chunks(w).map(|row| row[from..to].iter().filter(|d| **d > 0.7).count()).sum::<usize>() as f32 / ((to - from) * h).max(1) as f32;
        Self { person_height, people, picture, sky, near_sides: near(0, side).max(near(w - side, w)) - near(side, w - side) }
    }
}

/// What the whole picture's movement fits: the camera's, as far as can be told.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fit {
    /// The camera turning right, in picture widths a second.
    pub pan: f32,
    /// The camera turning up, in picture heights a second.
    pub tilt: f32,
    /// The picture growing, as a fraction of itself a second.
    pub zoom: f32,
    /// The picture turning clockwise, in radians a second.
    pub roll: f32,
    /// How much of the picture moves this way.
    pub agree: f32,
}

/// Solves `a x = b` for four unknowns; `None` if there's no one answer.
fn solve(mut a: [[f32; 5]; 4]) -> Option<[f32; 4]> {
    for i in 0..4 {
        let pivot = (i..4).max_by(|p, q| a[*p][i].abs().total_cmp(&a[*q][i].abs()))?;
        a.swap(i, pivot);
        if a[i][i].abs() < 1e-6 {
            return None;
        }
        for j in 0..4 {
            if j != i {
                let by = a[j][i] / a[i][i];
                let source = a[i];
                for (value, from) in a[j].iter_mut().zip(source).skip(i) {
                    *value -= by * from;
                }
            }
        }
    }
    Some(std::array::from_fn(|i| a[i][4] / a[i][i]))
}

/// Fits a camera's movement to `flow`: planes of x then y, `size`, rows from the top, in
/// fractions of the picture a second, y downwards. `aspect` is the picture's width over height.
/// Whatever doesn't fit, like people moving about, counts for less on each pass.
pub fn fit(flow: &[f32], (fw, fh): (usize, usize), aspect: f32) -> Fit {
    // Everything in picture heights, so it's square, from the middle.
    let (xs, ys) = flow.split_at(fw * fh);
    let points: Vec<[f32; 4]> = (0..fh)
        .flat_map(|y| (0..fw).map(move |x| (x, y)))
        .map(|(x, y)| [((x as f32 + 0.5) / fw as f32 - 0.5) * aspect, (y as f32 + 0.5) / fh as f32 - 0.5, xs[y * fw + x] * aspect, ys[y * fw + x]])
        .collect();
    let mut weights = vec![1.0f32; points.len()];
    let mut params = [0.0f32; 4];
    let predict = |p: &[f32; 4], [tx, ty, z, r]: [f32; 4]| (tx + z * p[0] - r * p[1], ty + z * p[1] + r * p[0]);
    for _ in 0..5 {
        let mut a = [[0.0f32; 5]; 4];
        for (p, w) in points.iter().zip(&weights) {
            // The two rows of the model at this point, each with what it should come to.
            for (row, want) in [([1.0, 0.0, p[0], -p[1]], p[2]), ([0.0, 1.0, p[1], p[0]], p[3])] {
                for i in 0..4 {
                    for j in 0..4 {
                        a[i][j] += w * row[i] * row[j];
                    }
                    a[i][4] += w * row[i] * want;
                }
            }
        }
        let Some(found) = solve(a) else { break };
        params = found;
        let mut misses: Vec<f32> = points.iter().map(|p| { let (u, v) = predict(p, params); (p[2] - u).hypot(p[3] - v) }).collect();
        let at = misses.len() / 2;
        let typical = (*misses.select_nth_unstable_by(at, f32::total_cmp).1 * 1.5).max(0.02);
        for (p, w) in points.iter().zip(&mut weights) {
            let (u, v) = predict(p, params);
            let miss = (p[2] - u).hypot(p[3] - v) / typical;
            *w = 1.0 / (1.0 + miss * miss);
        }
    }
    let agree = points.iter().filter(|p| { let (u, v) = predict(p, params); (p[2] - u).hypot(p[3] - v) < 0.03 + 0.25 * u.hypot(v) }).count() as f32 / points.len().max(1) as f32;
    let [tx, ty, zoom, roll] = params;
    Fit { pan: -tx / aspect, tilt: ty, zoom, roll, agree }
}

/// The biggest face in the picture, in fractions of the picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub height: f32,
    pub x: f32,
}

/// What's known at one frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Moment {
    /// The frame's position in the video.
    pub time: f32,
    /// The position of the last scene cut up to here.
    pub last_cut: Option<f32>,
    pub faces: u32,
    pub face: Option<Face>,
    /// Movement, if there's some new.
    pub fit: Option<Fit>,
    /// How fast the quicker things in the picture are moving (picture sizes a second).
    pub speed: Option<f32>,
    pub level: Option<f32>,
    pub beat: bool,
}

/// How big the shot is from 0 (very wide) to 1 (extreme close-up), going by the biggest face or,
/// without one, how tall people are and how much of the picture they cover: a face the detector
/// can't see, side on or lying down, still leaves someone filling the picture.
fn size_of(face: Option<f32>, person_height: f32, people: f32) -> f32 {
    let h = face.unwrap_or((person_height / PERSON_PER_FACE).max(people.sqrt() * PEOPLE_FACE));
    let between = |from: f32, to: f32, a: f32, b: f32| a + (b - a) * ((h - from) / (to - from)).clamp(0.0, 1.0);
    if h < MEDIUM {
        between(0.0, MEDIUM, 0.0, 0.4)
    } else if h < CLOSE {
        between(MEDIUM, CLOSE, 0.4, 0.7)
    } else if h < EXTREME {
        between(CLOSE, EXTREME, 0.7, 0.9)
    } else {
        between(EXTREME, 1.0, 0.9, 1.0)
    }
}

fn band(size: f32) -> &'static str {
    match size {
        s if s >= 0.9 => "extreme close-up",
        s if s >= 0.7 => "close-up",
        s if s >= 0.4 => "medium",
        _ => "wide",
    }
}

fn smooth(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn eased(value: &mut f32, to: f32, dt: f32, seconds: f32) {
    *value += (to - *value) * (dt / seconds).min(1.0);
}

/// One finished shot.
#[derive(Clone, Copy, Debug, Default)]
struct Past {
    end: f32,
    length: f32,
    size: f32,
    motion: f32,
    level: f32,
    faces: f32,
    picture: Option<[f32; PICTURE_W * PICTURE_H]>,
}

/// How different two shots' small pictures are.
fn setup_difference(a: &[f32; PICTURE_W * PICTURE_H], b: &[f32; PICTURE_W * PICTURE_H]) -> f32 {
    a.iter().zip(b).map(|(a, b)| (a - b).abs()).sum::<f32>() / a.len() as f32
}

/// What effects are told.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reading {
    /// 0 very wide to 1 extreme close-up.
    pub size: f32,
    /// Pan, tilt, zoom and roll, a second.
    pub camera: [f32; 4],
    /// 0 to 1.
    pub handheld: f32,
    /// Cuts a minute.
    pub pace: f32,
    /// Action, dialogue, quiet and montage, each 0 to 1.
    pub scene: [f32; 4],
    /// Whether there's anything to go on yet for the shot's size, and for scenes.
    pub knows_size: bool,
    pub knows_scene: bool,
}

/// How a shot is going, so far.
#[derive(Clone, Copy, Debug, Default)]
struct Current {
    start: f32,
    seconds: f32,
    size: Option<f32>,
    /// Sums over the shot, each times the seconds it held for, and for the level how many.
    size_sum: f32,
    motion_sum: f32,
    level_sum: f32,
    level_seconds: f32,
    face_seconds: f32,
    person_height: f32,
    people: f32,
    sky: f32,
    near_sides: f32,
    faces: u32,
    agree: f32,
    picture: Option<[f32; PICTURE_W * PICTURE_H]>,
    clued: bool,
}

impl Current {
    fn use_clues(&mut self, clues: &Clues) {
        self.person_height = clues.person_height;
        self.people = clues.people;
        self.sky = clues.sky;
        self.near_sides = clues.near_sides;
        self.picture.get_or_insert(clues.picture);
        self.clued = true;
    }

    fn past(&self, end: f32) -> Past {
        let per = self.seconds.max(1e-3);
        Past {
            end,
            length: end - self.start,
            size: self.size_sum / per,
            motion: self.motion_sum / per,
            level: if self.level_seconds > 0.0 { self.level_sum / self.level_seconds } else { 0.0 },
            faces: self.face_seconds / per,
            picture: self.picture,
        }
    }
}

/// Keeps the measurements up to date, frame by frame.
#[derive(Debug, Default)]
pub struct Tracker {
    time: Option<f32>,
    began: f32,
    cut: Option<f32>,
    shot: Current,
    camera: [f32; 4],
    steady: [f32; 4],
    handheld: f32,
    speed: f32,
    clues: VecDeque<(f32, Clues)>,
    past: VecDeque<Past>,
    beats: VecDeque<f32>,
    was_beat: bool,
    usual_level: f32,
    reading: Reading,
    scene_at: f32,
}

impl Tracker {
    pub fn reading(&self) -> Reading {
        self.reading
    }

    /// What the depth worker's found about the frame at `time`, usually ahead of the picture.
    pub fn add_clues(&mut self, time: f32, clues: Clues) {
        self.clues.push_back((time, clues));
        if self.clues.len() > MOST_CLUES {
            self.clues.pop_front();
        }
    }

    /// Takes in the clues the picture has reached, and if this shot has none yet, the next ones
    /// waiting: the picture is held back, so they're of it, and Shuffle wants them at the cut.
    fn take_clues(&mut self, time: f32) {
        while let Some(&(at, clues)) = self.clues.front().filter(|c| c.0 <= time) {
            self.clues.pop_front();
            if at >= self.shot.start {
                self.shot.use_clues(&clues);
            }
        }
        if !self.shot.clued
            && self.shot.seconds < JUST_STARTED
            && let Some(&(_, clues)) = self.clues.front().filter(|c| c.0 >= self.shot.start && c.0 < time + LOOK_AHEAD)
        {
            self.shot.use_clues(&clues);
        }
    }

    fn reset(&mut self, time: f32) {
        *self = Self { time: Some(time), began: time, shot: Current { start: time, ..Default::default() }, scene_at: time, ..Default::default() };
    }

    fn new_shot(&mut self, at: f32) {
        let done = self.shot.past(at);
        let versus = |other: Option<&Past>| match (done.picture, other.and_then(|p| p.picture)) {
            (Some(a), Some(b)) => setup_difference(&a, &b),
            _ => -1.0,
        };
        log::debug!(
            "shot at {:.1}: {:.1} s {} ({:.2}), faces {:.0}%, motion {:.2}, level {:.2}, vs last {:.3}, vs before {:.3}",
            self.shot.start, done.length, band(done.size), done.size, done.faces * 100.0, done.motion, done.level,
            versus(self.past.back()), versus(self.past.iter().rev().nth(1))
        );
        if done.length > 0.0 {
            self.past.push_back(done);
        }
        self.shot = Current { start: at, ..Default::default() };
        self.camera = [0.0; 4];
        self.steady = [0.0; 4];
        self.handheld = 0.0;
        self.speed = 0.0;
        self.reading.size = 0.0;
        self.reading.knows_size = false;
    }

    pub fn update(&mut self, m: &Moment) {
        let Some(last) = self.time else { return self.reset(m.time) };
        let dt = m.time - last;
        // A seek or a pause: what's been measured is from elsewhere.
        if !(0.0..1.0).contains(&dt) {
            return self.reset(m.time);
        }
        self.time = Some(m.time);
        if let Some(at) = m.last_cut.filter(|c| *c <= m.time && *c > self.began && self.cut.is_none_or(|was| (c - was).abs() > 0.02)) {
            self.cut = Some(at);
            self.new_shot(at);
        }
        self.take_clues(m.time);
        if dt == 0.0 {
            return;
        }
        let shot = &mut self.shot;
        let raw = size_of(m.face.map(|f| f.height), shot.person_height, shot.people);
        let size = shot.size.get_or_insert(raw);
        eased(size, raw, dt, SIZE_SECONDS);
        shot.seconds += dt;
        shot.size_sum += *size * dt;
        shot.faces = m.faces;
        if m.face.is_some() {
            shot.face_seconds += dt;
        }
        if let Some(level) = m.level {
            shot.level_sum += level * dt;
            shot.level_seconds += dt;
            if self.usual_level == 0.0 {
                self.usual_level = level;
            }
            eased(&mut self.usual_level, level, dt, USUAL_LEVEL_SECONDS);
        }
        if let Some(speed) = m.speed {
            self.speed = speed;
        }
        shot.motion_sum += self.speed * dt;
        if let Some(fit) = m.fit {
            let now = [fit.pan, fit.tilt, fit.zoom, fit.roll];
            let mut wobble = 0.0;
            for ((camera, steady), now) in self.camera.iter_mut().zip(&mut self.steady).zip(now) {
                eased(camera, now, dt, CAMERA_SECONDS);
                eased(steady, now, dt, STEADY_SECONDS);
                wobble += (now - *steady).powi(2);
            }
            eased(&mut self.handheld, (wobble.sqrt() / HANDHELD).min(1.0), dt, STEADY_SECONDS);
            shot.agree = fit.agree;
        }
        if m.beat && !self.was_beat {
            self.beats.push_back(m.time);
        }
        self.was_beat = m.beat;
        self.reading.size = *size;
        self.reading.knows_size = shot.clued || shot.face_seconds > 0.0;
        self.reading.knows_scene = m.time - self.began >= SHORTEST_WINDOW;
        self.reading.camera = self.camera;
        self.reading.handheld = self.handheld;
        if m.time - self.scene_at >= SCENE_EVERY {
            self.scene_at = m.time;
            self.score_scene(m.time);
            *DESCRIPTION.lock().unwrap_or_else(|e| e.into_inner()) = Some(self.describe());
        }
    }

    fn score_scene(&mut self, now: f32) {
        while self.past.front().is_some_and(|p| p.end < now - WINDOW) {
            self.past.pop_front();
        }
        while self.beats.front().is_some_and(|b| *b < now - BEATS_WINDOW) {
            self.beats.pop_front();
        }
        let elapsed = (now - self.began).clamp(SHORTEST_WINDOW, WINDOW);
        let cuts = self.past.len() as f32;
        self.reading.pace = cuts / elapsed * 60.0;
        let mean_length = elapsed / (cuts + 1.0);
        let current = self.shot.past(now);
        let shots = || self.past.iter().chain(std::iter::once(&current));
        let total = shots().map(|p| p.length).sum::<f32>().max(1e-3);
        let mean = |of: &dyn Fn(&Past) -> f32| shots().map(|p| of(p) * p.length).sum::<f32>() / total;
        let (motion, level, faces) = (mean(&|p| p.motion), mean(&|p| p.level), mean(&|p| p.faces));
        let loudness = if self.usual_level > 0.0 { level / self.usual_level } else { 1.0 };
        let (short, long) = (1.0 - smooth(1.5, 6.0, mean_length), smooth(6.0, 15.0, mean_length));
        let (moving, still) = (smooth(0.15, 0.7, motion), 1.0 - smooth(0.05, 0.35, motion));
        let loud = smooth(0.9, 1.6, loudness);
        let soft = 1.0 - smooth(0.6, 1.1, loudness);
        // Two people talking: the film going back to the setup before last, and with a face in it.
        let list: Vec<&Past> = shots().collect();
        let (mut turns, mut chances) = (0.0f32, 0.0f32);
        for w in list.windows(3) {
            if let (Some(a), Some(b), Some(c)) = (w[0].picture, w[1].picture, w[2].picture) {
                chances += 1.0;
                if setup_difference(&a, &c) < SAME_SETUP && setup_difference(&a, &b) >= SAME_SETUP && w[0].faces > 0.5 && w[2].faces > 0.5 {
                    turns += 1.0;
                }
            }
        }
        let turn_taking = if chances > 0.0 { turns / chances } else { 0.0 };
        let steady_beat = {
            let gaps: Vec<f32> = self.beats.iter().zip(self.beats.iter().skip(1)).map(|(a, b)| b - a).collect();
            if gaps.len() < 4 {
                0.0
            } else {
                let average = gaps.iter().sum::<f32>() / gaps.len() as f32;
                let spread = (gaps.iter().map(|g| (g - average).powi(2)).sum::<f32>() / gaps.len() as f32).sqrt();
                1.0 - smooth(0.15, 0.5, spread / average.max(1e-3))
            }
        };
        self.reading.scene = [
            0.4 * moving + 0.35 * short + 0.25 * loud,
            0.6 * turn_taking * (1.0 - moving) + 0.4 * faces * still,
            0.35 * long + 0.35 * still + 0.3 * soft,
            short.min(steady_beat) * 0.85 + (1.0 - faces) * short * 0.15,
        ];
    }

    fn describe(&self) -> String {
        let r = &self.reading;
        let wide_open = self.shot.face_seconds == 0.0 && self.shot.sky > 0.15 && r.size < 0.4;
        let framing = match self.shot.faces {
            0 if wide_open => "establishing".to_owned(),
            0 => "no faces".to_owned(),
            1 if self.shot.near_sides > 0.3 => "over the shoulder?".to_owned(),
            1 => "1 face".to_owned(),
            2 => "two-shot".to_owned(),
            n => format!("group of {n}"),
        };
        let [pan, tilt, zoom, roll] = r.camera;
        let [action, dialogue, quiet, montage] = r.scene;
        format!(
            "Shot: {} ({:.2}), {framing}\nCamera: pan {pan:+.2} tilt {tilt:+.2} zoom {zoom:+.2} roll {roll:+.2}, fits {:.0}%, handheld {:.2}\nScene: {:.0} cuts/min, action {action:.2} dialogue {dialogue:.2} quiet {quiet:.2} montage {montage:.2}",
            band(r.size), r.size, self.shot.agree * 100.0, r.handheld, r.pace
        )
    }
}

static DESCRIPTION: Mutex<Option<String>> = Mutex::new(None);

/// For the F3 window: the latest measurements, a line each.
pub fn describe() -> Vec<String> {
    DESCRIPTION.lock().unwrap_or_else(|e| e.into_inner()).iter().flat_map(|text| text.lines().map(str::to_owned).collect::<Vec<_>>()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (usize, usize) = (64, 36);

    fn flow(of: impl Fn(f32, f32) -> (f32, f32), aspect: f32) -> Vec<f32> {
        let (w, h) = SIZE;
        let at: Vec<(f32, f32)> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| of(((x as f32 + 0.5) / w as f32 - 0.5) * aspect, (y as f32 + 0.5) / h as f32 - 0.5)).collect();
        at.iter().map(|p| p.0 / aspect).chain(at.iter().map(|p| p.1)).collect()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn a_pan_is_found_and_the_right_way_round() {
        // The picture moving left: the camera's turning right.
        let f = fit(&flow(|_, _| (-0.3, 0.0), 16.0 / 9.0), SIZE, 16.0 / 9.0);
        assert!(close(f.pan, 0.3 / (16.0 / 9.0)), "{f:?}");
        assert!(close(f.tilt, 0.0) && close(f.zoom, 0.0) && close(f.roll, 0.0), "{f:?}");
        assert!(f.agree > 0.99);
    }

    #[test]
    fn zoom_and_roll_are_found() {
        let a = 16.0 / 9.0;
        let z = fit(&flow(|x, y| (0.4 * x, 0.4 * y), a), SIZE, a);
        assert!(close(z.zoom, 0.4) && close(z.pan, 0.0) && close(z.roll, 0.0), "{z:?}");
        let r = fit(&flow(|x, y| (-0.5 * y, 0.5 * x), a), SIZE, a);
        assert!(close(r.roll, 0.5) && close(r.zoom, 0.0), "{r:?}");
    }

    #[test]
    fn people_moving_dont_change_the_cameras_movement() {
        let a = 16.0 / 9.0;
        // A quarter of the picture off on its own.
        let f = fit(&flow(|x, y| if x > 0.2 && x < 0.65 && y.abs() < 0.3 { (0.8, -0.5) } else { (0.2, 0.1) }, a), SIZE, a);
        assert!(close(f.pan, -0.2 / a), "{f:?}");
        assert!(close(f.tilt, 0.1), "{f:?}");
        assert!(f.agree > 0.7 && f.agree < 0.95, "{f:?}");
    }

    #[test]
    fn shot_sizes_fall_in_their_bands() {
        assert_eq!(band(size_of(Some(0.7), 0.0, 0.0)), "extreme close-up");
        assert_eq!(band(size_of(Some(0.4), 0.0, 0.0)), "close-up");
        assert_eq!(band(size_of(Some(0.2), 0.0, 0.0)), "medium");
        assert_eq!(band(size_of(Some(0.05), 0.0, 0.0)), "wide");
        // No face: someone standing the height of the picture is medium, and small figures wide.
        assert_eq!(band(size_of(None, 0.95, 0.08)), "medium");
        assert_eq!(band(size_of(None, 0.3, 0.02)), "wide");
        // Someone filling the picture with a face the detector can't see is close.
        assert_eq!(band(size_of(None, 1.0, 0.4)), "close-up");
    }

    fn at(time: f32, last_cut: Option<f32>) -> Moment {
        Moment { time, last_cut, ..Default::default() }
    }

    fn play(t: &mut Tracker, from: f32, to: f32, last_cut: Option<f32>) {
        let mut time = from;
        while time < to {
            t.update(&at(time, last_cut));
            time += 0.04;
        }
    }

    #[test]
    fn clues_ahead_of_the_picture_are_for_the_next_shot() {
        let mut t = Tracker::default();
        let tall = Clues { person_height: 1.0, people: 0.5, ..Default::default() };
        play(&mut t, 0.0, 2.0, None);
        t.add_clues(2.1, tall);
        play(&mut t, 2.0, 2.08, None);
        assert_eq!(t.shot.people, 0.0, "not this shot's");
        t.update(&at(2.1, Some(2.1)));
        t.update(&at(2.12, Some(2.1)));
        assert_eq!(t.shot.people, 0.5);
        assert!(t.reading().knows_size);
    }

    #[test]
    fn pace_counts_cuts_in_the_window() {
        let mut t = Tracker::default();
        play(&mut t, 0.0, 2.0, None);
        for n in 1..=9 {
            let cut = 2.0 * n as f32;
            play(&mut t, cut, cut + 2.0, Some(cut));
        }
        // Nine cuts in 20 seconds.
        assert!((t.reading().pace - 27.0).abs() < 3.0, "{:?}", t.reading());
    }

    #[test]
    fn a_cut_starts_the_size_again_and_a_seek_forgets_everything() {
        let mut t = Tracker::default();
        let face = |time, last_cut| Moment { time, last_cut, faces: 1, face: Some(Face { height: 0.5, x: 0.5 }), ..Default::default() };
        for i in 0..50 {
            t.update(&face(i as f32 * 0.04, None));
        }
        assert!(t.reading().size > 0.7);
        t.update(&at(2.04, Some(2.04)));
        assert!(t.reading().size < 0.1, "{:?}", t.reading());
        t.update(&at(100.0, None));
        assert_eq!(t.past.len(), 0);
        assert_eq!(t.reading(), Reading::default());
    }

    #[test]
    fn people_talking_in_turn_count_as_dialogue() {
        let mut t = Tracker::default();
        let mut time = 0.0;
        let mut last_cut = None;
        for shot in 0..12 {
            let mut clues = Clues::default();
            clues.picture[if shot % 2 == 0 { 0 } else { 20 }] = 3.0;
            for _ in 0..100 {
                t.add_clues(time, clues);
                t.update(&Moment { time, last_cut, faces: 1, face: Some(Face { height: 0.35, x: 0.5 }), speed: Some(0.02), level: Some(0.2), ..Default::default() });
                time += 0.04;
            }
            last_cut = Some(time);
        }
        let [action, dialogue, ..] = t.reading().scene;
        assert!(dialogue > 0.7 && dialogue > action, "{:?}", t.reading());
    }
}
