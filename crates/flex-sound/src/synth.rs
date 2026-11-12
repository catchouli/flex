//! Synth voices: everyone's voice played by a synth that follows its pitch (heard by DDSP's
//! pitch network) and loudness. A chiptune (a square wave in semitones, with a noise channel
//! for the consonants), a theremin (a sine gliding up high, wavering), or a vocoder (a
//! synth's sound shaped by the voice's, band by band: the classic robot).

use std::f32::consts::PI;
use std::sync::Arc;

use crate::ddsp::Pitch;
use crate::dsp::{Biquad, Line, Rng, follow};
use crate::pitch::Tracker;
use crate::{Delay, Networks, Scene, Settings, SoundEffect, Stems};

/// The sounds, by id: what the "sound" setting's options are.
pub const SOUNDS: [&str; 3] = ["chiptune", "theremin", "vocoder"];

/// How long a note's held through what isn't voiced, in seconds.
const HOLD_SECS: f32 = 0.16;
/// The theremin's middle, in Hz, and how it wavers.
const THEREMIN_HZ: f32 = 630.0;
const VIBRATO_HZ: f32 = 5.5;
const VIBRATO_CENTS: f32 = 25.0;
/// The vocoder's bands.
const BANDS: usize = 16;
const LOWEST_BAND_HZ: f32 = 120.0;
const HIGHEST_BAND_HZ: f32 = 7000.0;
const TALK_SECS: f32 = 0.03;
const QUIET_SECS: f32 = 0.3;

/// "sound" picks the synth, "amount" how much of it against the voice as it was.
pub struct Synth {
    rate: f32,
    late: usize,
    pitch: Option<Arc<Pitch>>,
    tracker: Tracker,
    line: Line,
    /// The voice's loudness, followed.
    level: f32,
    /// The note playing (Hz), the theremin's glide (log2 Hz), and how long since there was a pitch.
    hz: f32,
    glide: Option<f32>,
    unvoiced: f32,
    phase: f32,
    vibrato: f32,
    /// The chiptune's noise channel.
    lfsr: u16,
    noise_hold: f32,
    noise: f32,
    rng: Rng,
    /// The vocoder's bands: the voice's filter and loudness, and the synth's filter.
    voice_bands: Vec<(Biquad, f32)>,
    synth_bands: Vec<Biquad>,
    talking: f32,
    input: Vec<f32>,
    background: Vec<Delay>,
}

impl Synth {
    pub fn new(rate: u32) -> Self {
        let late = crate::most_late(rate);
        let r = rate as f32;
        let band = |i: usize| LOWEST_BAND_HZ * (HIGHEST_BAND_HZ / LOWEST_BAND_HZ).powf(i as f32 / (BANDS - 1) as f32);
        Self {
            rate: r,
            late,
            pitch: None,
            tracker: Tracker::default(),
            line: Line::new(late + 64),
            level: 0.0,
            hz: 110.0,
            glide: None,
            unvoiced: 1.0,
            phase: 0.0,
            vibrato: 0.0,
            lfsr: 1,
            noise_hold: 0.0,
            noise: 0.0,
            rng: Rng::new(0x5e7),
            voice_bands: (0..BANDS).map(|i| (Biquad::bandpass(r, band(i), 5.0), 0.0)).collect(),
            synth_bands: (0..BANDS).map(|i| Biquad::bandpass(r, band(i), 5.0)).collect(),
            talking: 0.0,
            input: Vec::new(),
            background: Vec::new(),
        }
    }

    /// One sample of the synth, for the voice's sample `x` as late as it plays.
    fn play(&mut self, sound: &str, x: f32, voiced: bool) -> f32 {
        let rate = self.rate;
        let attack = follow(0.005, rate);
        let release = follow(0.05, rate);
        self.level += (x.abs() - self.level) * if x.abs() > self.level { attack } else { release };
        if !voiced {
            self.unvoiced += 1.0 / rate;
        }
        let held = self.unvoiced < HOLD_SECS;
        match sound {
            "theremin" => {
                let Some(glide) = self.glide else { return 0.0 };
                self.vibrato = (self.vibrato + VIBRATO_HZ / rate).fract();
                let hz = 2f32.powf(glide + VIBRATO_CENTS / 1200.0 * (2.0 * PI * self.vibrato).sin());
                self.phase = (self.phase + hz / rate).fract();
                let p = 2.0 * PI * self.phase;
                let wave = p.sin() + 0.15 * (2.0 * p).sin() + 0.05 * (3.0 * p).sin();
                // It doesn't stop for a consonant, only once the voice has.
                let fade = if held { 1.0 } else { (1.0 - (self.unvoiced - HOLD_SECS) / 0.1).max(0.0) };
                wave * self.level * 1.4 * fade
            }
            "vocoder" => {
                // A sawtooth at the voice's note, and noise for its consonants.
                self.phase = (self.phase + self.hz / rate).fract();
                let carrier = if held { 2.0 * self.phase - 1.0 } else { 0.0 } + self.rng.noise() * if held { 0.05 } else { 0.4 };
                let mut y = 0.0;
                for ((voice, loud), synth) in self.voice_bands.iter_mut().zip(&mut self.synth_bands) {
                    let v = voice.next(x).abs();
                    *loud += (v - *loud) * if v > *loud { attack } else { follow(0.03, rate) };
                    y += synth.next(carrier) * *loud;
                }
                y * 5.5
            }
            _ => {
                // A square wave (a quarter on), loud in sixteen steps; noise where there's
                // no pitch, from a 15-bit shift register like an old console's.
                let level = (self.level * 16.0 * 4.0).round().min(16.0) / 16.0 / 4.0;
                if held {
                    self.phase = (self.phase + self.hz / rate).fract();
                    (if self.phase < 0.25 { 1.0 } else { -1.0 / 3.0 }) * level * 1.6
                } else {
                    self.noise_hold += 8000.0 / rate;
                    if self.noise_hold >= 1.0 {
                        self.noise_hold -= 1.0;
                        let bit = (self.lfsr ^ (self.lfsr >> 1)) & 1;
                        self.lfsr = (self.lfsr >> 1) | (bit << 14);
                        self.noise = if self.lfsr & 1 == 1 { 1.0 } else { -1.0 };
                    }
                    self.noise * level * 0.6
                }
            }
        }
    }
}

impl SoundEffect for Synth {
    fn latency(&self) -> usize {
        self.late
    }

    fn use_networks(&mut self, networks: &Networks) {
        self.pitch.clone_from(&networks.pitch);
    }

    fn process(&mut self, stems: &mut Stems, settings: &Settings, _: &Scene) {
        let sound = settings.choice("sound").unwrap_or("chiptune");
        let amount = settings.number("amount").map_or(1.0, |a| a / 100.0).clamp(0.0, 1.0);
        let frames = stems.voice.len();
        let secs = frames as f32 / self.rate;
        if let Some(p) = &self.pitch {
            self.tracker.push(p, &stems.voice);
        }
        let voiced = self.tracker.f0.is_some();
        if let Some(f0) = self.tracker.f0 {
            self.unvoiced = 0.0;
            // In semitones, but for the theremin, which glides, moved by whole octaves to
            // where a theremin plays.
            self.hz = 440.0 * 2f32.powf(((12.0 * (f0 / 440.0).log2()).round()) / 12.0);
            let octaves = self.tracker.average.map_or(0.0, |a| (THEREMIN_HZ.log2() - a).round());
            let to = f0.log2() + octaves;
            let glide = self.glide.get_or_insert(to);
            *glide += (to - *glide) * (secs / 0.04).min(1.0);
        } else if self.unvoiced > HOLD_SECS + 0.2 {
            self.glide = None;
        }
        // On while someone's talking, or the voice has a clear pitch (it's the voice's).
        let pitched = if self.unvoiced < HOLD_SECS { 1.0 } else { 0.0 };
        let wanted = ((stems.speech - 0.3) / 0.4).clamp(0.0, 1.0).max(pitched);
        let start = self.talking;
        self.talking += (wanted - self.talking) * (secs / if wanted > self.talking { TALK_SECS } else { QUIET_SECS }).min(1.0);
        self.input.clear();
        self.input.extend(stems.voice.iter().zip(&stems.echo).map(|(v, e)| v + e));
        let late = self.late;
        let input = std::mem::take(&mut self.input);
        for (i, ((x, y), e)) in input.iter().zip(&mut stems.voice).zip(&mut stems.echo).enumerate() {
            self.line.push(*x);
            let dry = self.line.at(late as f32);
            let played = self.play(sound, dry, voiced);
            let t = amount * (start + (self.talking - start) * i as f32 / frames as f32);
            *y = dry * (1.0 - t) + played * t;
            *e = 0.0;
        }
        self.input = input;
        self.background.resize_with(stems.background.len(), || Delay::new(late));
        for (d, c) in self.background.iter_mut().zip(&mut stems.background) {
            d.run(c);
        }
    }
}
