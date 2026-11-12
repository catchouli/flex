//! Voices and the world apart: the background dropping while people talk, the voices taken
//! out, or only the voices.

use crate::dsp::follow;
use crate::{Scene, Settings, SoundEffect, Stems};

/// The modes, by id: what the "mode" setting's options are.
pub const MODES: [&str; 3] = ["focus", "no_voices", "only_voices"];

/// "mode" picks which, "amount" how much.
pub struct Dialogue {
    rate: f32,
    /// How far down the background is, 0–1, following whether someone's talking.
    ducked: f32,
}

impl Dialogue {
    pub fn new(rate: u32) -> Self {
        Self { rate: rate as f32, ducked: 0.0 }
    }
}

impl SoundEffect for Dialogue {
    fn process(&mut self, stems: &mut Stems, settings: &Settings, _: &Scene) {
        let amount = settings.number("amount").map_or(1.0, |a| a / 100.0).clamp(0.0, 1.0);
        let frames = stems.voice.len();
        let (voice, background) = match settings.choice("mode").unwrap_or("focus") {
            "no_voices" => ((1.0 - amount, 1.0 - amount), (1.0, 1.0)),
            "only_voices" => ((1.0, 1.0), (1.0 - amount, 1.0 - amount)),
            _ => {
                // Down by up to 18 dB while someone's talking: quickly down, slowly back.
                let talking = ((stems.speech - 0.3) / 0.4).clamp(0.0, 1.0);
                let secs = if talking > self.ducked { 0.08 } else { 0.6 };
                let start = self.ducked;
                self.ducked += (talking - self.ducked) * (1.0 - (1.0 - follow(secs, self.rate)).powi(frames as i32));
                let gain = |d: f32| 1.0 - d * amount * (1.0 - 0.125);
                ((1.0, 1.0), (gain(start), gain(self.ducked)))
            }
        };
        let ramp = |(a, b): (f32, f32), i: usize| a + (b - a) * i as f32 / frames as f32;
        for (i, (v, e)) in stems.voice.iter_mut().zip(&mut stems.echo).enumerate() {
            *v *= ramp(voice, i);
            *e *= ramp(voice, i);
        }
        for c in &mut stems.background {
            for (i, x) in c.iter_mut().enumerate() {
                *x *= ramp(background, i);
            }
        }
    }
}
