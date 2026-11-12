//! Finds how the camera sits relative to the ground, from depth alone: the floor, or a table or
//! whatever else is flat and level. Depth here is proportional to 1 / distance plus an unknown
//! offset, and anything flat is then exactly `a x + b y + c` across the picture, so level
//! surfaces can be found without knowing the lens.

/// One point in this many of the depth map, each way.
const GRID: usize = 8;
const ATTEMPTS: usize = 160;
/// How close to the plane, in depth (0–1), a point has to be to count as on it.
const ON_PLANE: f32 = 0.012;
/// The depth has to get nearer down the picture by at least this much per picture height.
const LEAST_SLOPE: f32 = 0.25;
/// How far the ground can lean, as the tangent of the angle: about 20°. Films rarely go further.
const MOST_ROLL: f32 = 0.36;
/// The shape of picture assumed when judging lean, close enough for that.
const ASPECT: f32 = 1.78;
/// Where the ground reaches depth 0, in picture heights, for it to be believable.
const HORIZON: (f32, f32) = (0.25, 2.0);
/// How much of the usable picture has to lie on it: from nothing to sure.
const SHARE: (f32, f32) = (0.06, 0.2);
/// How much of the picture the ground has to span, up and across.
const SPAN: (f32, f32) = (0.3, 0.12);
/// How far up the picture the middle of the ground can be: higher, it's a wall behind someone.
const HIGHEST: f32 = 0.55;
/// How much better a new plane has to be than the last, so it doesn't flit between surfaces.
const STICKY: f32 = 1.15;
/// How much of each new fit is taken, and how quickly being sure comes and goes.
const FOLLOW: f32 = 0.12;
const CONFIDENCE_FOLLOW: f32 = 0.08;

#[derive(Clone, Copy)]
struct Point {
    x: f32,
    y: f32,
    depth: f32,
}

/// `a x + b y + c`, over the picture from 0 to 1 with y going up.
type Plane = [f32; 3];

fn at(plane: Plane, p: &Point) -> f32 {
    plane[0] * p.x + plane[1] * p.y + plane[2]
}

fn believable([a, b, c]: Plane) -> bool {
    if b > -LEAST_SLOPE || (a / ASPECT).abs() > MOST_ROLL * -b {
        return false;
    }
    let horizon = -(c + 0.5 * a) / b;
    (HORIZON.0..HORIZON.1).contains(&horizon)
}

fn through(p: &Point, q: &Point, r: &Point) -> Option<Plane> {
    let (ux, uy, ud) = (q.x - p.x, q.y - p.y, q.depth - p.depth);
    let (vx, vy, vd) = (r.x - p.x, r.y - p.y, r.depth - p.depth);
    let det = ux * vy - uy * vx;
    if det.abs() < 1e-3 {
        return None;
    }
    let a = (ud * vy - uy * vd) / det;
    let b = (ux * vd - ud * vx) / det;
    Some([a, b, p.depth - a * p.x - b * p.y])
}

/// The plane fitting `points` best, by least squares.
fn fitted(points: &[&Point]) -> Option<Plane> {
    let (mut xx, mut xy, mut yy, mut x, mut y, mut xd, mut yd, mut d) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for p in points {
        let (px, py, pd) = (f64::from(p.x), f64::from(p.y), f64::from(p.depth));
        xx += px * px;
        xy += px * py;
        yy += py * py;
        x += px;
        y += py;
        xd += px * pd;
        yd += py * pd;
        d += pd;
    }
    let n = points.len() as f64;
    let det = xx * (yy * n - y * y) - xy * (xy * n - y * x) + x * (xy * y - yy * x);
    if det.abs() < 1e-9 {
        return None;
    }
    let a = (xd * (yy * n - y * y) - xy * (yd * n - y * d) + x * (yd * y - yy * d)) / det;
    let b = (xx * (yd * n - y * d) - xd * (xy * n - y * x) + x * (xy * d - yd * x)) / det;
    let c = (xx * (yy * d - y * yd) - xy * (xy * d - yd * x) + xd * (xy * y - yy * x)) / det;
    Some([a as f32, b as f32, c as f32])
}

/// How well a plane does: nearer the bottom of the picture counts for more, as floors are.
fn score(plane: Plane, points: &[Point]) -> f32 {
    points.iter().filter(|p| (at(plane, p) - p.depth).abs() < ON_PLANE).map(|p| 1.5 - p.y).sum()
}

/// Works out where the ground is, and how sure that is, frame after frame.
pub struct Ground {
    plane: Plane,
    confidence: f32,
    random: u32,
}

impl Default for Ground {
    fn default() -> Self {
        Self { plane: [0.0; 3], confidence: 0.0, random: 0x9e37_79b9 }
    }
}

impl Ground {
    /// A new shot: nothing's known about this one yet.
    pub fn forget(&mut self) {
        self.confidence = 0.0;
    }

    fn roll(&mut self, below: usize) -> usize {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 17;
        self.random ^= self.random << 5;
        self.random as usize % below
    }

    fn fit(&mut self, points: &[Point]) -> Option<(Plane, f32)> {
        if points.len() < 12 {
            return None;
        }
        let mut best = None;
        let mut best_score = 0.0;
        if self.confidence > 0.0 && believable(self.plane) {
            best = Some(self.plane);
            best_score = score(self.plane, points) * STICKY;
        }
        for _ in 0..ATTEMPTS {
            let (p, q, r) = (self.roll(points.len()), self.roll(points.len()), self.roll(points.len()));
            let Some(plane) = through(&points[p], &points[q], &points[r]).filter(|plane| believable(*plane)) else { continue };
            let score = score(plane, points);
            if score > best_score {
                best = Some(plane);
                best_score = score;
            }
        }
        let on: Vec<&Point> = points.iter().filter(|p| best.is_some_and(|plane| (at(plane, p) - p.depth).abs() < ON_PLANE * 1.5)).collect();
        let span = |of: fn(&Point) -> f32| on.iter().map(|p| of(p)).fold(1.0f32, f32::min)..on.iter().map(|p| of(p)).fold(0.0f32, f32::max);
        let (across, up) = (span(|p| p.x), span(|p| p.y));
        if on.len() < 12 || across.end - across.start < SPAN.0 || up.end - up.start < SPAN.1 {
            return None;
        }
        if on.iter().map(|p| p.y).sum::<f32>() / on.len() as f32 > HIGHEST {
            return None;
        }
        let plane = fitted(&on).filter(|plane| believable(*plane))?;
        let share = on.len() as f32 / points.len() as f32;
        Some((plane, ((share - SHARE.0) / (SHARE.1 - SHARE.0)).clamp(0.0, 1.0)))
    }

    /// Takes in a new depth map (bottom row first) and any masks with it (RGBA: person, wall,
    /// sky), and returns the ground as a, b, c and how sure, from 0 to 1.
    pub fn update(&mut self, values: &[f32], masks: Option<&[u8]>, (width, height): (usize, usize)) -> [f32; 4] {
        let mut points = Vec::with_capacity(width * height / (GRID * GRID));
        for row in (GRID / 2..height).step_by(GRID) {
            for column in (GRID / 2..width).step_by(GRID) {
                let at = row * width + column;
                let depth = values[at];
                let usable = masks.is_none_or(|m| m[at * 4] < 100 && m[at * 4 + 1] < 140 && m[at * 4 + 2] < 100);
                if usable && depth > 0.005 && depth < 0.995 {
                    points.push(Point { x: column as f32 / width as f32, y: row as f32 / height as f32, depth });
                }
            }
        }
        match self.fit(&points) {
            Some((plane, sure)) => {
                let follow = if self.confidence <= 0.0 { 1.0 } else { FOLLOW };
                for (now, new) in self.plane.iter_mut().zip(plane) {
                    *now += (new - *now) * follow;
                }
                self.confidence += (sure - self.confidence) * CONFIDENCE_FOLLOW.max(if follow == 1.0 { 0.25 } else { 0.0 });
            }
            None => self.confidence = (self.confidence - CONFIDENCE_FOLLOW * 0.5).max(0.0),
        }
        [self.plane[0], self.plane[1], self.plane[2], self.confidence]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (usize, usize) = (160, 90);

    fn scene(plane: Plane) -> Vec<f32> {
        let mut values = vec![0.0; SIZE.0 * SIZE.1];
        for y in 0..SIZE.1 {
            for x in 0..SIZE.0 {
                let (u, v) = (x as f32 / SIZE.0 as f32, y as f32 / SIZE.1 as f32);
                // The floor up to the back wall, which faces the camera.
                values[y * SIZE.0 + x] = (plane[0] * u + plane[1] * v + plane[2]).max(0.3);
            }
        }
        values
    }

    fn settled(values: &[f32], masks: Option<&[u8]>) -> [f32; 4] {
        let mut ground = Ground::default();
        let mut out = [0.0; 4];
        for _ in 0..60 {
            out = ground.update(values, masks, SIZE);
        }
        out
    }

    #[test]
    fn finds_a_level_floor() {
        let [a, b, c, sure] = settled(&scene([0.0, -1.6, 1.0]), None);
        assert!(a.abs() < 0.02 && (b + 1.6).abs() < 0.03 && (c - 1.0).abs() < 0.03, "{a} {b} {c}");
        assert!(sure > 0.9, "{sure}");
    }

    #[test]
    fn sees_the_camera_leaning() {
        let [a, b, ..] = settled(&scene([0.5, -1.6, 0.9]), None);
        assert!((a - 0.5).abs() < 0.03 && (b + 1.6).abs() < 0.04, "{a} {b}");
    }

    #[test]
    fn a_wall_facing_the_camera_is_not_the_ground() {
        let flat = vec![0.4; SIZE.0 * SIZE.1];
        assert_eq!(settled(&flat, None)[3], 0.0);
    }

    #[test]
    fn a_ceiling_is_not_the_ground() {
        assert_eq!(settled(&scene([0.0, 1.6, -0.6]), None)[3], 0.0);
    }

    #[test]
    fn people_are_left_out() {
        let mut values = scene([0.0, -1.6, 1.0]);
        let mut masks = vec![0u8; SIZE.0 * SIZE.1 * 4];
        for y in 0..SIZE.1 {
            for x in 40..120 {
                values[y * SIZE.0 + x] = 0.9 - y as f32 * 0.002 + x as f32 * 0.004;
                masks[(y * SIZE.0 + x) * 4] = 255;
            }
        }
        let [a, b, ..] = settled(&values, Some(&masks));
        assert!(a.abs() < 0.03 && (b + 1.6).abs() < 0.04, "{a} {b}");
    }

    #[test]
    fn being_sure_goes_at_a_new_shot() {
        let mut ground = Ground::default();
        let values = scene([0.0, -1.6, 1.0]);
        for _ in 0..30 {
            ground.update(&values, None, SIZE);
        }
        ground.forget();
        assert!(ground.update(&vec![0.4; SIZE.0 * SIZE.1], None, SIZE)[3] == 0.0);
    }
}
