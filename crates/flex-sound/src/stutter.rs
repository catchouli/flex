//! Stutter: now and then a short slice of what was just heard is played again a few times
//! (sometimes backwards), skipping what came meanwhile, like a scratched CD.

use crate::dsp::{Line, Rng};
use crate::{Scene, Settings, SoundEffect, Stems};

/// The longest slice, in seconds, and how long its edges fade.
const LONGEST: f32 = 0.25;
const EDGE_SECS: f32 = 0.003;

#[derive(Clone, Copy)]
struct Slice {
    len: f32,
    repeats: u32,
    backwards: bool,
    /// How long it's been playing, in samples.
    played: f32,
}

/// "often" (0–100) says how often, "on" whether it's everything or the voices (which then
/// stutter only while someone's talking).
pub struct Stutter {
    rate: f32,
    lines: Vec<Line>,
    slice: Option<Slice>,
    rng: Rng,
    /// Blocks until the next can start.
    rest: u32,
}

impl Stutter {
    pub fn new(rate: u32) -> Self {
        Self { rate: rate as f32, lines: Vec::new(), slice: None, rng: Rng::new(0x5757), rest: 0 }
    }

    /// One channel through the slice playing (if any) from its state at the block's start;
    /// gives back its state at the end.
    fn run(slice: Option<Slice>, line: &mut Line, xs: &mut [f32], edge: f32) -> Option<Slice> {
        let mut slice = slice;
        for x in xs {
            line.push(*x);
            let Some(s) = &mut slice else { continue };
            let into = s.played % s.len;
            let at = if s.backwards { s.len - into } else { into };
            // How far back the slice's sample is: it began `len` before the slice started.
            let delay = s.played + s.len - at;
            let whole = s.len * s.repeats as f32;
            let fade = (into.min(s.len - into) / edge).min(1.0);
            let mix = (s.played.min(whole - s.played) / edge).clamp(0.0, 1.0);
            *x = *x * (1.0 - mix) + line.at(delay) * fade * mix;
            s.played += 1.0;
            if s.played >= whole {
                slice = None;
            }
        }
        slice
    }
}

impl SoundEffect for Stutter {
    fn process(&mut self, stems: &mut Stems, settings: &Settings, _: &Scene) {
        let often = settings.number("often").map_or(0.4, |a| a / 100.0).clamp(0.0, 1.0);
        let voices = settings.choice("on") == Some("voices");
        let rate = self.rate;
        let frames = stems.voice.len();
        if voices {
            for (v, e) in stems.voice.iter_mut().zip(&mut stems.echo) {
                *v += *e;
                *e = 0.0;
            }
        } else {
            stems.mix_down();
        }
        let count = if voices { 1 } else { stems.background.len() };
        self.lines.resize_with(count, || Line::new(((LONGEST * 2.0 + 0.05) * rate) as usize));
        self.rest = self.rest.saturating_sub(1);
        let may = !voices || stems.speech > 0.5;
        if self.slice.is_none() && self.rest == 0 && may && self.rng.unit() < often * 2.0 * frames as f32 / rate {
            let lens = [0.05, 0.08, 0.12, 0.16, 0.25];
            self.slice = Some(Slice {
                len: lens[(self.rng.unit() * lens.len() as f32) as usize % lens.len()] * rate,
                repeats: 2 + (self.rng.unit() * 4.0) as u32,
                backwards: self.rng.unit() < 0.25,
                played: 0.0,
            });
            // Not straight after another.
            self.rest = 30;
        }
        let edge = EDGE_SECS * rate;
        let channels: Vec<&mut Vec<f32>> = if voices { vec![&mut stems.voice] } else { stems.background.iter_mut().collect() };
        let mut after = self.slice;
        for (c, line) in channels.into_iter().zip(&mut self.lines) {
            after = Self::run(self.slice, line, c, edge);
        }
        self.slice = after;
    }
}
