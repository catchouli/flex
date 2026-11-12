//! Putting the sound somewhere: a room, a hall, a cathedral or a cave (Reverb), or under water.

use std::f32::consts::PI;

use crate::dsp::{Biquad, Line, Reverb, Rng, Space};
use crate::{Scene, Settings, SoundEffect, Stems};

/// The spaces, by id: what the "space" setting's options are.
pub const SPACES: [&str; 6] = ["room", "bathroom", "hall", "cathedral", "cave", "far"];

fn space(id: &str) -> Space {
    match id {
        "bathroom" => Space { seconds: 1.1, size: 0.25, damping: 0.05, gap: 0.002 },
        "hall" => Space { seconds: 2.2, size: 1.0, damping: 0.35, gap: 0.02 },
        "cathedral" => Space { seconds: 5.0, size: 2.0, damping: 0.45, gap: 0.04 },
        "cave" => Space { seconds: 3.2, size: 1.4, damping: 0.15, gap: 0.03 },
        "far" => Space { seconds: 2.5, size: 1.6, damping: 0.6, gap: 0.05 },
        _ => Space { seconds: 0.6, size: 0.4, damping: 0.3, gap: 0.006 },
    }
}

/// The reverb: the "space" setting picks the space, "on" whether it's everything or the voices
/// that are in it, and "amount" how much of it.
pub struct Reverberate {
    rate: u32,
    playing: Option<(String, Reverb)>,
    /// For "far": the sound as it gets there, thinner.
    far: Vec<[Biquad; 2]>,
    mono: Vec<f32>,
    wet: Vec<f32>,
}

impl Reverberate {
    pub fn new(rate: u32) -> Self {
        Self { rate, playing: None, far: Vec::new(), mono: Vec::new(), wet: Vec::new() }
    }
}

impl SoundEffect for Reverberate {
    fn process(&mut self, stems: &mut Stems, settings: &Settings, _: &Scene) {
        let id = settings.choice("space").unwrap_or("room");
        let amount = settings.number("amount").map_or(0.6, |a| a / 100.0).clamp(0.0, 1.0);
        let voices = settings.choice("on") == Some("voices");
        if self.playing.as_ref().is_none_or(|(p, _)| p != id) {
            self.playing = Some((id.to_string(), Reverb::new(self.rate, space(id))));
        }
        let Some((_, reverb)) = &mut self.playing else { return };
        if voices {
            self.mono.clear();
            self.mono.extend(stems.voice.iter().zip(&stems.echo).map(|(v, e)| v + e));
        } else {
            stems.mono(&mut self.mono);
        }
        let channels = stems.background.len();
        let rate = self.rate as f32;
        // Far away, what reaches you straight is quieter and thinner.
        let far = id == "far";
        if far {
            self.far.resize_with(channels + 1, || [Biquad::highpass(rate, 250.0, 0.7), Biquad::lowpass(rate, 1800.0, 0.7)]);
            let thin = |f: &mut [Biquad; 2], x: &mut f32| {
                let low = f[0].next(*x);
                *x = *x * (1.0 - amount) + f[1].next(low) * 0.4 * amount;
            };
            for (c, f) in stems.background.iter_mut().zip(&mut self.far) {
                c.iter_mut().for_each(|x| thin(f, x));
            }
            let f = &mut self.far[channels];
            for (v, e) in stems.voice.iter_mut().zip(&mut stems.echo) {
                *v += *e;
                *e = 0.0;
                thin(f, v);
            }
        }
        let dry = if far { 1.0 } else { 1.0 - 0.3 * amount };
        for c in &mut stems.background {
            c.iter_mut().for_each(|x| *x *= dry);
        }
        stems.voice.iter_mut().chain(&mut stems.echo).for_each(|x| *x *= dry);
        // The space's sound, spread over every channel.
        let gain = amount * if far { 1.6 } else { 1.0 } / (channels as f32 / 2.0).max(1.0).sqrt();
        self.wet.resize(channels, 0.0);
        for (i, x) in self.mono.iter().enumerate() {
            reverb.next(*x, &mut self.wet);
            for (c, w) in stems.background.iter_mut().zip(&self.wet) {
                c[i] += w * gain;
            }
        }
    }
}

/// A bubble going up: a whistle rising as it goes, dying away.
struct Bubble {
    age: f32,
    phase: f32,
    hz: f32,
    rise: f32,
    life: f32,
    loud: f32,
    /// How much of it each of the first two channels gets.
    pan: [f32; 2],
}

/// Under water: everything muffled and wobbling, with bubbles. "depth" says how muffled,
/// "bubbles" how many.
pub struct Underwater {
    rate: f32,
    lines: Vec<Line>,
    filters: Vec<[Biquad; 3]>,
    t: f32,
    rng: Rng,
    bubbles: Vec<Bubble>,
}

impl Underwater {
    pub fn new(rate: u32) -> Self {
        Self { rate: rate as f32, lines: Vec::new(), filters: Vec::new(), t: 0.0, rng: Rng::new(0x5eed), bubbles: Vec::new() }
    }
}

impl SoundEffect for Underwater {
    fn process(&mut self, stems: &mut Stems, settings: &Settings, _: &Scene) {
        let depth = settings.number("depth").map_or(0.6, |a| a / 100.0).clamp(0.0, 1.0);
        let many = settings.number("bubbles").map_or(0.4, |a| a / 100.0).clamp(0.0, 1.0);
        stems.mix_down();
        let rate = self.rate;
        let frames = stems.voice.len();
        let start = self.t;
        self.t += frames as f32 / rate;
        // The muffling sways a little, slowly.
        let cutoff = (4000.0 * (350.0f32 / 4000.0).powf(depth)) * (1.0 + 0.25 * (2.0 * PI * 0.17 * start).sin());
        let make = || [Biquad::lowpass(rate, cutoff, 0.9), Biquad::lowpass(rate, cutoff * 1.3, 0.6), Biquad::peak(rate, 150.0, 0.8, 5.0)];
        let channels = stems.background.len();
        self.lines.resize_with(channels, || Line::new((0.006 * rate) as usize));
        self.filters.resize_with(channels, make);
        let tuned = make();
        for ((c, line), filters) in stems.background.iter_mut().zip(&mut self.lines).zip(&mut self.filters) {
            for (f, t) in filters.iter_mut().zip(tuned) {
                f.retune(t);
            }
            for (i, x) in c.iter_mut().enumerate() {
                // Wobbling: read back at a delay that drifts between 1.5 and 4.5 ms.
                let t = start + i as f32 / rate;
                let wobble = 0.003 + 0.0012 * (2.0 * PI * 0.6 * t).sin() + 0.0003 * (2.0 * PI * 0.23 * t).sin();
                line.push(*x);
                let y = line.at(wobble * rate);
                *x = filters.iter_mut().fold(y, |y, f| f.next(y));
            }
        }
        // Bubbles: now and then, and more while someone's talking.
        let per_second = many * (3.0 + 12.0 * stems.speech);
        if self.rng.unit() < per_second * frames as f32 / rate {
            let pan = self.rng.unit();
            self.bubbles.push(Bubble {
                age: 0.0,
                phase: 0.0,
                hz: 500.0 + 1200.0 * self.rng.unit(),
                rise: 0.4 + 0.8 * self.rng.unit(),
                life: 0.02 + 0.05 * self.rng.unit(),
                loud: 0.02 + 0.06 * self.rng.unit() * (0.5 + many),
                pan: [(1.0 - pan).sqrt(), pan.sqrt()],
            });
        }
        let first_two = channels.min(2);
        for b in &mut self.bubbles {
            for i in 0..frames {
                let hz = b.hz * (1.0 + b.rise * b.age / b.life);
                b.phase = (b.phase + hz / rate).fract();
                let y = (2.0 * PI * b.phase).sin() * b.loud * (-b.age / b.life).exp() * (b.age * 2000.0).min(1.0);
                b.age += 1.0 / rate;
                for (c, p) in stems.background.iter_mut().take(first_two).zip(b.pan) {
                    c[i] += y * if first_two == 1 { 1.0 } else { p };
                }
            }
        }
        self.bubbles.retain(|b| b.age < b.life * 6.0);
    }
}
