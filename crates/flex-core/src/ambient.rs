//! Ambient light: turning a tiny copy of the video frame into colours for LED strips
//! behind the TV (sent to plugins as `flex_plugin::Light`). Everything happens in linear
//! light: averaging (so a red and green scene comes out yellow, not a muddy brown), and
//! the output too, since LEDs give out light in proportion to the value they're sent.
//! Sending screen (sRGB) values instead washes colours out towards white. [`Boost`] can
//! brighten that, keeping colours, until channels hit full and wash out.

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

/// Linear light to sRGB, 0.0–1.0 (e.g. to show a light's colour on screen).
pub fn linear_to_srgb(c: f32) -> f32 {
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

/// Oklab (L, a, b) for a linear-light colour: a space where distances match how different
/// colours look (Björn Ottosson's).
fn oklab(c: Linear) -> [f32; 3] {
    let l = (0.412_221_47 * c[0] + 0.536_332_55 * c[1] + 0.051_445_995 * c[2]).cbrt();
    let m = (0.211_903_5 * c[0] + 0.680_699_5 * c[1] + 0.107_396_96 * c[2]).cbrt();
    let s = (0.088_302_46 * c[0] + 0.281_718_85 * c[1] + 0.629_978_7 * c[2]).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Hue buckets for [`Noticeable`]; one more holds greys.
const HUE_BUCKETS: usize = 12;
/// Below this Oklab lightness a pixel is too dark to notice.
const MIN_LIGHTNESS: f32 = 0.15;
/// Below this Oklab chroma a pixel counts as grey.
const MIN_CHROMA: f32 = 0.04;
/// What a grey pixel counts for. A typical colourful pixel counts ~0.2, so a colour wins
/// over a grey scene once it covers about 2% of the picture, but a stray speck doesn't.
const GREY_WEIGHT: f32 = 0.005;
/// The current colour is kept unless another scores this much better (no flicker
/// between two close contenders).
const SWITCH_MARGIN: f32 = 1.35;

/// "The most noticeable colour": instead of averaging the picture (which blends half red,
/// half blue into a purple that isn't on screen), the pixels are sorted into hue buckets,
/// each counting for more the more colourful it is, and the winning bucket's colour is
/// used. Brightness still follows the whole picture. Remembers its bucket between frames.
#[derive(Clone, Debug, Default)]
pub struct Noticeable {
    current: Option<usize>,
}

impl Noticeable {
    pub fn pick(&mut self, grid: &Grid) -> Linear {
        let mut score = [0.0f32; HUE_BUCKETS + 1];
        let mut sum = [[0.0f32; 3]; HUE_BUCKETS + 1];
        for y in 0..grid.height {
            for x in 0..grid.width {
                let c = grid.cell(x, y);
                let [l, a, b] = oklab(c);
                if l < MIN_LIGHTNESS {
                    continue;
                }
                let chroma = a.hypot(b);
                let (bucket, weight) = if chroma < MIN_CHROMA {
                    (HUE_BUCKETS, GREY_WEIGHT)
                } else {
                    let turn = (b.atan2(a) / std::f32::consts::TAU).rem_euclid(1.0);
                    (((turn * HUE_BUCKETS as f32) as usize).min(HUE_BUCKETS - 1), chroma * (0.5 + l))
                };
                score[bucket] += weight;
                for k in 0..3 {
                    sum[bucket][k] += c[k] * weight;
                }
            }
        }
        let (best, best_score) = score.iter().copied().enumerate().fold((0, 0.0), |m, (i, s)| if s > m.1 { (i, s) } else { m });
        if best_score <= 0.0 {
            self.current = None;
            return [0.0; 3];
        }
        let keep = self.current.filter(|&c| score[c] > 0.0 && score[c] * SWITCH_MARGIN >= best_score);
        let chosen = keep.unwrap_or(best);
        self.current = Some(chosen);
        let colour = sum[chosen].map(|v| v / score[chosen]);
        // As bright as the whole picture (so dark scenes stay dim), in the chosen colour.
        let peak = colour.iter().copied().fold(0.0f32, f32::max);
        let target = grid.average(Mode::Whole).iter().copied().fold(0.0f32, f32::max);
        if peak <= 0.0 { [0.0; 3] } else { colour.map(|v| v / peak * target) }
    }
}

/// How bright the light is compared with the picture, from Settings → Ambient light.
/// Applied to the picture's colours before smoothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Boost {
    /// Multiplies the light. Over 1.0, channels pushed past full are capped by
    /// [`to_light`], washing bright colours out towards white.
    pub gain: f32,
    /// Make the light as bright as the picture *looks* rather than as much light as it
    /// gives off: a scene that looks 30% bright is only 7% of the light. Colours keep
    /// their ratios either way.
    pub perceived: bool,
    /// Below this (linear, 0.0–1.0, before boosting) the light goes off, so dark scenes
    /// stay dark. 0.003 is about 12/255 on screen.
    pub dark_cutoff: f32,
}

impl Default for Boost {
    fn default() -> Self {
        Self { gain: 1.0, perceived: false, dark_cutoff: 0.003 }
    }
}

impl Boost {
    pub fn apply(&self, zones: &[Linear]) -> Vec<Linear> {
        let peak = zones.iter().flat_map(|c| c.iter().copied()).fold(0.0f32, f32::max);
        if peak < self.dark_cutoff {
            return vec![[0.0; 3]; zones.len()];
        }
        zones
            .iter()
            .map(|c| {
                let max = c.iter().copied().fold(0.0f32, f32::max);
                let curve = if self.perceived && max > 0.0 { linear_to_srgb(max) / max } else { 1.0 };
                c.map(|ch| ch * curve * self.gain)
            })
            .collect()
    }
}

/// Turns linear colours into what plugins get: full-brightness linear colours plus one
/// brightness (that of the brightest zone). Channels over 1.0 (from a [`Boost`]) are
/// capped. `saturation`: 1.0 leaves colours alone; more makes them more vivid.
pub fn to_light(zones: &[Linear], saturation: f32) -> Light {
    let zones: Vec<Linear> = zones.iter().map(|c| c.map(|ch| ch.clamp(0.0, 1.0))).collect();
    let value = zones.iter().flat_map(|c| c.iter().copied()).fold(0.0f32, f32::max);
    let colours = zones
        .iter()
        .map(|c| {
            let max = c.iter().copied().fold(0.0f32, f32::max);
            if max <= 0.0 {
                return [0, 0, 0];
            }
            // Saturate in HSV terms: push channels away from the brightest one's level.
            let min = c.iter().copied().fold(1.0f32, f32::min);
            let sat = ((max - min) / max * saturation).clamp(0.0, 1.0);
            let new_min = max * (1.0 - sat);
            let scale = if max > min { (max - new_min) / (max - min) } else { 0.0 };
            c.map(|ch| {
                let ch = if max > min { max - (max - ch) * scale } else { ch };
                (ch / max * 255.0).round().clamp(0.0, 255.0) as u8
            })
        })
        .collect();
    Light { zones: colours, brightness: value }
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
        let l = to_light(&[[0.5, 0.25, 0.0]], 1.0);
        assert_eq!(l.zones, vec![[255, 128, 0]]);
        assert!((l.brightness - 0.5).abs() < 1e-6);
        // Output stays linear: a blueish scene (60, 90, 140 on screen) must stay clearly
        // blue on an LED, not the pale (109, 164, 255) that sRGB values give.
        assert_eq!(to_light(&[linear([60, 90, 140])], 1.0).zones, vec![[44, 99, 255]]);
        // Saturation boost pulls the weakest channel down; grey stays grey.
        assert_eq!(to_light(&[[0.8, 0.4, 0.4]], 2.0).zones, vec![[255, 0, 0]]);
        assert_eq!(to_light(&[[0.3; 3]], 2.0).zones, vec![[255, 255, 255]]);
        assert_eq!(to_light(&[[0.0; 3]], 1.0).zones, vec![[0, 0, 0]]);
    }

    #[test]
    fn boost_brightens_then_washes_out() {
        let blue = linear([60, 90, 140]);
        // Doubling keeps the colour and doubles the light.
        let double = to_light(&Boost { gain: 2.0, ..Boost::default() }.apply(&[blue]), 1.0);
        assert_eq!(double.zones, vec![[44, 99, 255]]);
        assert!((double.brightness - 2.0 * blue[2]).abs() < 1e-5);
        // Far too much: blue caps at full and the others catch up, towards white.
        let washed = to_light(&Boost { gain: 20.0, ..Boost::default() }.apply(&[blue]), 1.0);
        assert_eq!(washed.brightness, 1.0);
        assert!(washed.zones[0][0] > 150, "{:?}", washed.zones);
        // "How bright it looks": a scene that looks 30% bright drives the light at 30%,
        // not the 7% of its light, and keeps its colour.
        let looks = Boost { perceived: true, ..Boost::default() };
        let grey = to_light(&looks.apply(&[linear([77, 77, 77])]), 1.0);
        assert!((grey.brightness - 77.0 / 255.0).abs() < 0.01, "{}", grey.brightness);
        assert_eq!(to_light(&looks.apply(&[blue]), 1.0).zones, vec![[44, 99, 255]]);
        // Dark scenes switch off whatever the boost.
        let lots = Boost { gain: 4.0, perceived: true, ..Boost::default() };
        assert_eq!(lots.apply(&[linear([8, 8, 8])]), vec![[0.0; 3]]);
        assert!(lots.apply(&[linear([20, 20, 20])])[0][0] > 0.0);
    }

    #[test]
    fn noticeable_picks_a_colour_thats_on_screen() {
        let hue = |c: Linear| {
            let [_, a, b] = oklab(c);
            b.atan2(a)
        };
        // Half red, half blue: one of them, not the purple an average makes.
        let split = grid(32, 18, |x, _| if x < 16 { [220, 30, 30] } else { [30, 30, 220] });
        let picked = Noticeable::default().pick(&split);
        let (red, blue) = (hue(linear([220, 30, 30])), hue(linear([30, 30, 220])));
        assert!((hue(picked) - red).abs() < 0.2 || (hue(picked) - blue).abs() < 0.2, "{picked:?}");
        let average = split.average(Mode::Whole);
        assert!((hue(average) - red).abs() > 0.3 && (hue(average) - blue).abs() > 0.3, "the average is neither");

        // A small vivid area beats a big grey one.
        let small = grid(32, 18, |x, y| if x < 6 && y < 4 { [255, 160, 0] } else { [120, 120, 120] });
        let picked = Noticeable::default().pick(&small);
        assert!(picked[0] > picked[2] * 3.0, "orange, not grey: {picked:?}");
        // ...at the whole picture's brightness, so it's not suddenly brighter.
        let target = small.average(Mode::Whole).iter().copied().fold(0.0f32, f32::max);
        assert!((picked.iter().copied().fold(0.0f32, f32::max) - target).abs() < 1e-5);

        // A speck of colour (1 pixel) doesn't override a grey scene.
        let speck = Noticeable::default().pick(&grid(32, 18, |x, y| if (x, y) == (3, 3) { [255, 0, 0] } else { [120, 120, 120] }));
        assert!((speck[0] - speck[2]).abs() < 1e-4, "grey: {speck:?}");

        // All grey: grey. All black: black.
        let grey = Noticeable::default().pick(&grid(8, 8, |_, _| [128, 128, 128]));
        assert!((grey[0] - grey[2]).abs() < 1e-4 && grey[0] > 0.1);
        assert_eq!(Noticeable::default().pick(&grid(8, 8, |_, _| [0, 0, 0])), [0.0; 3]);
    }

    #[test]
    fn noticeable_doesnt_flicker_between_close_colours() {
        let mut pick = Noticeable::default();
        let mostly_red = |red_cols: usize| grid(32, 18, move |x, _| if x < red_cols { [220, 30, 30] } else { [30, 30, 220] });
        let is_red = |c: Linear| c[0] > c[2];
        assert!(is_red(pick.pick(&mostly_red(20))));
        // Blue edges ahead a little: stays red.
        assert!(is_red(pick.pick(&mostly_red(15))));
        // Blue clearly wins: switches.
        assert!(!is_red(pick.pick(&mostly_red(8))));
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
