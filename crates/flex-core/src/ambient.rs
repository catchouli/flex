//! Ambient light: turning a tiny copy of the video frame into colours for LED strips
//! behind the TV (sent to plugins as `flex_plugin::Light`). Averaging happens in linear
//! light, so a red and green scene comes out yellow, not a muddy brown.

use std::sync::LazyLock;

use flex_plugin::Light;

/// A downscaled frame: RGBA bytes, row 0 at the top.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Linear-light RGB, 0.0–1.0.
pub type Linear = [f32; 3];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The average of the whole picture.
    Whole,
    /// The average of the picture's outer edges, which are next to the strip.
    Edges,
}

/// How much of the picture "Edges" uses, from each side.
const EDGE_FRACTION: f32 = 0.15;

static TO_LINEAR: LazyLock<[f32; 256]> = LazyLock::new(|| std::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0)));

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

impl Grid {
    fn cell(&self, x: usize, y: usize) -> Linear {
        let i = (y * self.width + x) * 4;
        let px = &self.rgba[i..i + 3];
        [TO_LINEAR[px[0] as usize], TO_LINEAR[px[1] as usize], TO_LINEAR[px[2] as usize]]
    }

    fn mean(&self, cells: impl Iterator<Item = (usize, usize)>) -> Linear {
        let (mut sum, mut n) = ([0.0f32; 3], 0usize);
        for (x, y) in cells {
            let c = self.cell(x, y);
            for k in 0..3 {
                sum[k] += c[k];
            }
            n += 1;
        }
        if n == 0 { [0.0; 3] } else { sum.map(|s| s / n as f32) }
    }

    /// One colour for the whole strip.
    pub fn average(&self, mode: Mode) -> Linear {
        let (w, h) = (self.width, self.height);
        match mode {
            Mode::Whole => self.mean((0..h).flat_map(|y| (0..w).map(move |x| (x, y)))),
            Mode::Edges => {
                let ex = ((w as f32 * EDGE_FRACTION).ceil() as usize).max(1);
                let ey = ((h as f32 * EDGE_FRACTION).ceil() as usize).max(1);
                let edge = move |&(x, y): &(usize, usize)| x < ex || x + ex >= w || y < ey || y + ey >= h;
                self.mean((0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(edge))
            }
        }
    }

    /// `n` colours around the edge of the picture, clockwise from the bottom left corner
    /// (up the left side, along the top, down the right side, back along the bottom),
    /// each the average of the cells nearest that stretch of edge.
    pub fn zones(&self, n: usize, mode: Mode) -> Vec<Linear> {
        if n <= 1 {
            return vec![self.average(mode)];
        }
        let (w, h) = (self.width, self.height);
        // The border cells in order, as (x, y).
        let mut ring = Vec::with_capacity(2 * (w + h));
        ring.extend((0..h).rev().map(|y| (0, y)));
        ring.extend((1..w).map(|x| (x, 0)));
        ring.extend((1..h).map(|y| (w - 1, y)));
        ring.extend((1..w - 1).rev().map(|x| (x, h - 1)));
        (0..n)
            .map(|i| {
                let start = i * ring.len() / n;
                let end = ((i + 1) * ring.len() / n).max(start + 1);
                self.mean(ring[start..end.min(ring.len())].iter().copied())
            })
            .collect()
    }
}

/// The user's adjustments, from Settings → Ambient light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    /// 1.0 leaves colours alone; more makes them more vivid.
    pub saturation: f32,
    /// Scales the brightness sent to the light (0.0–1.0).
    pub max_brightness: f32,
    /// Below this brightness (0.0–1.0) the light goes off, so dark scenes stay dark.
    pub dark_cutoff: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self { saturation: 1.2, max_brightness: 1.0, dark_cutoff: 0.03 }
    }
}

/// Turns linear colours into what plugins get: full-brightness colours plus one
/// brightness (that of the brightest zone).
pub fn to_light(zones: &[Linear], tuning: &Tuning) -> Light {
    let srgb: Vec<[f32; 3]> = zones.iter().map(|c| c.map(linear_to_srgb)).collect();
    let value = srgb.iter().flat_map(|c| c.iter().copied()).fold(0.0f32, f32::max);
    let colours = srgb
        .iter()
        .map(|c| {
            let max = c.iter().copied().fold(0.0f32, f32::max);
            if max <= 0.0 {
                return [0, 0, 0];
            }
            // Saturate in HSV terms: push channels away from the brightest one's level.
            let min = c.iter().copied().fold(1.0f32, f32::min);
            let sat = ((max - min) / max * tuning.saturation).clamp(0.0, 1.0);
            let new_min = max * (1.0 - sat);
            let scale = if max > min { (max - new_min) / (max - min) } else { 0.0 };
            c.map(|ch| {
                let ch = if max > min { max - (max - ch) * scale } else { ch };
                (ch / max * 255.0).round().clamp(0.0, 255.0) as u8
            })
        })
        .collect();
    let brightness = if value < tuning.dark_cutoff { 0.0 } else { value * tuning.max_brightness.clamp(0.0, 1.0) };
    Light { zones: colours, brightness }
}

/// Eases colours towards their targets so scene cuts and flicker don't strobe the room.
#[derive(Clone, Debug, Default)]
pub struct Smoother {
    current: Vec<Linear>,
}

impl Smoother {
    /// Moves towards `target` over `dt` seconds with time constant `tau` (0 = jump).
    /// Returns the new colours.
    pub fn step(&mut self, target: &[Linear], dt: f32, tau: f32) -> &[Linear] {
        if self.current.len() != target.len() || tau <= 0.0 {
            self.current = target.to_vec();
            return &self.current;
        }
        let a = 1.0 - (-dt.max(0.0) / tau).exp();
        for (c, t) in self.current.iter_mut().zip(target) {
            for k in 0..3 {
                c[k] += (t[k] - c[k]) * a;
            }
        }
        &self.current
    }

    /// Whether the colours are (visibly) at `target`.
    pub fn settled(&self, target: &[Linear]) -> bool {
        self.current.len() == target.len()
            && self.current.iter().zip(target).all(|(c, t)| (0..3).all(|k| (linear_to_srgb(c[k]) - linear_to_srgb(t[k])).abs() < 0.5 / 255.0))
    }

    pub fn current(&self) -> &[Linear] {
        &self.current
    }
}

/// sRGB bytes to linear light, e.g. for a fixed colour.
pub fn linear(rgb: [u8; 3]) -> Linear {
    rgb.map(|c| TO_LINEAR[c as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(width: usize, height: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> Grid {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                rgba.extend(f(x, y));
                rgba.push(255);
            }
        }
        Grid { width, height, rgba }
    }

    #[test]
    fn averages_in_linear_light() {
        // Half red, half green: linear averaging gives a bright yellow (~188 per channel),
        // naive averaging of the bytes would give a dim 128.
        let g = grid(4, 2, |x, _| if x < 2 { [255, 0, 0] } else { [0, 255, 0] });
        let avg = g.average(Mode::Whole).map(linear_to_srgb);
        assert!((avg[0] * 255.0 - 188.0).abs() < 2.0 && (avg[1] - avg[0]).abs() < 0.01 && avg[2] == 0.0, "{avg:?}");
    }

    #[test]
    fn edges_ignore_the_middle() {
        let g = grid(32, 18, |x, y| if (8..24).contains(&x) && (5..13).contains(&y) { [255, 255, 255] } else { [0, 0, 255] });
        assert_eq!(g.average(Mode::Edges), linear([0, 0, 255]));
        assert!(g.average(Mode::Whole)[0] > 0.1);
    }

    #[test]
    fn zones_go_clockwise_from_bottom_left() {
        // Left half red, right half blue: the first quarter (left side) is red, the
        // third (right side) blue.
        let g = grid(16, 9, |x, _| if x < 8 { [255, 0, 0] } else { [0, 0, 255] });
        let z = g.zones(4, Mode::Edges);
        assert_eq!(z.len(), 4);
        assert!(z[0][0] > 0.9 && z[0][2] == 0.0, "{z:?}");
        assert!(z[2][2] > 0.9 && z[2][0] == 0.0, "{z:?}");
        assert_eq!(g.zones(1, Mode::Whole), vec![g.average(Mode::Whole)]);
        assert_eq!(g.zones(200, Mode::Whole).len(), 200);
    }

    #[test]
    fn light_is_full_brightness_colour_plus_brightness() {
        let plain = Tuning { saturation: 1.0, max_brightness: 1.0, dark_cutoff: 0.0 };
        let l = to_light(&[linear([128, 64, 0])], &plain);
        assert_eq!(l.zones, vec![[255, 128, 0]]);
        assert!((l.brightness - 128.0 / 255.0).abs() < 0.01);
        // Saturation boost pulls the weakest channel down; grey stays grey.
        let vivid = Tuning { saturation: 2.0, ..plain };
        assert_eq!(to_light(&[linear([200, 150, 150])], &vivid).zones, vec![[255, 127, 127]]);
        assert_eq!(to_light(&[linear([90, 90, 90])], &vivid).zones, vec![[255, 255, 255]]);
        // Dark scenes switch off; brightness is scaled by the maximum.
        assert_eq!(to_light(&[linear([3, 3, 3])], &Tuning::default()).brightness, 0.0);
        let half = Tuning { max_brightness: 0.5, ..plain };
        assert!((to_light(&[linear([255, 0, 0])], &half).brightness - 0.5).abs() < 1e-6);
        assert_eq!(to_light(&[[0.0; 3]], &plain).zones, vec![[0, 0, 0]]);
    }

    #[test]
    fn smoother_eases_and_settles() {
        let mut s = Smoother::default();
        let (black, white) = ([[0.0; 3]], [[1.0; 3]]);
        s.step(&black, 0.0, 0.3);
        let after = s.step(&white, 0.3, 0.3)[0][0];
        assert!((after - (1.0 - (-1.0f32).exp())).abs() < 1e-5);
        assert!(!s.settled(&white));
        for _ in 0..100 {
            s.step(&white, 0.1, 0.3);
        }
        assert!(s.settled(&white));
        assert_eq!(s.step(&black, 0.1, 0.0), &black);
    }
}
