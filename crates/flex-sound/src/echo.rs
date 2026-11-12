//! Echoes: what was heard comes back, again and again, darker and quieter each time.

use crate::dsp::{Biquad, Line};
use crate::{Scene, Settings, SoundEffect, Stems};

/// The longest echo, in seconds.
const LONGEST: f32 = 1.0;

/// "time" (ms) says how long until it comes back, "repeats" (0–90 %) how much comes back each
/// time, "on" whether it's everything or the voices.
pub struct Echo {
    rate: f32,
    /// One line for the voices, then one per channel.
    lines: Vec<(Line, Biquad)>,
    /// The time, eased, so turning it bends the echo rather than clicking.
    time: f32,
}

impl Echo {
    pub fn new(rate: u32) -> Self {
        Self { rate: rate as f32, lines: Vec::new(), time: 0.0 }
    }
}

impl SoundEffect for Echo {
    fn process(&mut self, stems: &mut Stems, settings: &Settings, _: &Scene) {
        let rate = self.rate;
        let wanted = settings.number("time").map_or(0.3, |t| t / 1000.0).clamp(0.02, LONGEST) * rate;
        let repeats = settings.number("repeats").map_or(0.45, |r| r / 100.0).clamp(0.0, 0.9);
        let voices = settings.choice("on") == Some("voices");
        if self.time == 0.0 {
            self.time = wanted;
        }
        if !voices {
            stems.mix_down();
        }
        let channels = stems.background.len();
        self.lines.resize_with(channels + 1, || (Line::new((LONGEST * rate) as usize + 2), Biquad::lowpass(rate, 3500.0, 0.7)));
        let start = self.time;
        let step = 1.0 - (-(stems.voice.len() as f32) / (0.3 * rate)).exp();
        self.time += (wanted - self.time) * step;
        let frames = stems.voice.len() as f32;
        let run = |line: &mut Line, dark: &mut Biquad, x: &mut f32, i: usize| {
            let delay = start + (self.time - start) * i as f32 / frames;
            let back = line.at(delay - 1.0);
            // What goes round again is a little darker each time.
            line.push(*x + dark.next(back) * repeats);
            *x += back * 0.55;
        };
        if voices {
            let (line, dark) = &mut self.lines[0];
            for (i, (v, e)) in stems.voice.iter_mut().zip(&mut stems.echo).enumerate() {
                *v += *e;
                *e = 0.0;
                run(line, dark, v, i);
            }
        } else {
            for (c, (line, dark)) in stems.background.iter_mut().zip(&mut self.lines[1..]) {
                for (i, x) in c.iter_mut().enumerate() {
                    run(line, dark, x, i);
                }
            }
        }
    }
}
