//! "8D": the sound goes round your head. For headphones: each ear hears it a little later,
//! quieter and duller when it's on the other side (the head in the way), and both duller when
//! it's behind.

use std::f32::consts::PI;

use crate::dsp::{Line, Reverb, Space};
use crate::{Scene, Settings, SoundEffect, Stems};

/// How much later the far ear hears it than the near one, at most, in seconds.
const MOST_APART: f32 = 0.00066;

/// "turn" says how many seconds it takes to go round, "move" whether it's everything or the
/// voices, and "room" how much of a room it's in.
pub struct Circle {
    rate: f32,
    angle: f32,
    ears: [(Line, f32); 2],
    room: Reverb,
    mono: Vec<f32>,
    wet: [f32; 2],
}

impl Circle {
    pub fn new(rate: u32) -> Self {
        let line = || (Line::new((MOST_APART * rate as f32) as usize + 4), 0.0);
        Self { rate: rate as f32, angle: 0.0, ears: [line(), line()], room: Reverb::new(rate, Space { seconds: 0.9, size: 0.6, damping: 0.4, gap: 0.01 }), mono: Vec::new(), wet: [0.0; 2] }
    }
}

/// For an ear on `side` (−1 left, 1 right), with the sound at `angle` (0 ahead, going right):
/// how late it hears it (s), how loud, and the highest it hears well (Hz).
fn ear(angle: f32, side: f32) -> (f32, f32, f32) {
    let near = (1.0 + side * angle.sin()) / 2.0;
    let behind = (-angle.cos()).max(0.0);
    (MOST_APART * (1.0 - near), 0.45 + 0.55 * near, (1500.0 + 18_500.0 * near * near) * (1.0 - 0.45 * behind))
}

impl SoundEffect for Circle {
    fn process(&mut self, stems: &mut Stems, settings: &Settings, _: &Scene) {
        let turn = settings.number("turn").map_or(8.0, |t| t.clamp(1.0, 60.0));
        let room = settings.number("room").map_or(0.3, |r| r / 100.0).clamp(0.0, 1.0);
        let voices = settings.choice("move") == Some("voices");
        let rate = self.rate;
        let frames = stems.voice.len();
        if voices {
            self.mono.clear();
            self.mono.extend(stems.voice.iter().zip(&stems.echo).map(|(v, e)| v + e));
            stems.voice.fill(0.0);
            stems.echo.fill(0.0);
        } else {
            stems.mono(&mut self.mono);
            stems.set_mono(&[]);
            stems.voice.resize(frames, 0.0);
        }
        let start = self.angle;
        self.angle = (self.angle + 2.0 * PI * frames as f32 / (turn * rate)) % (2.0 * PI);
        let end = if self.angle < start { self.angle + 2.0 * PI } else { self.angle };
        let channels = stems.background.len();
        if channels < 2 {
            // One speaker can't go round: it just gets the room.
            if let Some(c) = stems.background.first_mut() {
                for (y, x) in c.iter_mut().zip(&self.mono) {
                    self.room.next(*x, &mut self.wet);
                    *y += x + self.wet[0] * room;
                }
            }
            return;
        }
        for (e, side) in [-1.0f32, 1.0].into_iter().enumerate() {
            let (_, loud0, bright0) = ear(start, side);
            let (_, loud1, bright1) = ear(end, side);
            let (line, low) = &mut self.ears[e];
            let out = &mut stems.background[e];
            for (i, x) in self.mono.iter().enumerate() {
                let t = i as f32 / frames as f32;
                let angle = start + (end - start) * t;
                line.push(*x);
                let y = line.at(ear(angle, side).0 * rate);
                let bright = bright0 + (bright1 - bright0) * t;
                *low += (y - *low) * (1.0 - (-2.0 * PI * bright / rate).exp());
                out[i] += *low * (loud0 + (loud1 - loud0) * t);
            }
        }
        if room > 0.0 {
            let [l, r, ..] = &mut stems.background[..] else { return };
            for ((x, l), r) in self.mono.iter().zip(l.iter_mut()).zip(r.iter_mut()) {
                self.room.next(*x, &mut self.wet);
                *l += self.wet[0] * room;
                *r += self.wet[1] * room;
            }
        }
    }
}
