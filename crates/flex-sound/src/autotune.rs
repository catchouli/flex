//! Auto-tune: the voice's pitch snapped to the notes of a scale, with harmonies sung along.
//! The pitch is heard by DDSP's pitch network and moved by `pitch::Shift`, which keeps the
//! voice's own sound.

use std::sync::Arc;

use crate::ddsp::Pitch;
use crate::dsp::{Line, follow};
use crate::pitch::{self, Shift, Tracker};
use crate::{Delay, Networks, Scene, Settings, SoundEffect, Stems};

/// The keys, by id (C to B), and the scales: what the "key" and "scale" settings' options are.
pub const KEYS: [&str; 12] = ["c", "cs", "d", "ds", "e", "f", "fs", "g", "gs", "a", "as", "b"];
pub const SCALES: [(&str, &[i32]); 5] = [
    ("chromatic", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
    ("major", &[0, 2, 4, 5, 7, 9, 11]),
    ("minor", &[0, 2, 3, 5, 7, 8, 10]),
    ("pentatonic", &[0, 2, 4, 7, 9]),
    ("blues", &[0, 3, 5, 6, 7, 10]),
];
/// The harmonies, by id: each voice as steps along the scale (or, in the chromatic scale,
/// semitones), or whole octaves.
pub const HARMONIES: [(&str, &[Harmony]); 5] = [
    ("none", &[]),
    ("third", &[Harmony::Steps(2, 4)]),
    ("triad", &[Harmony::Steps(2, 4), Harmony::Steps(4, 7)]),
    ("octave", &[Harmony::Octaves(-1)]),
    ("choir", &[Harmony::Steps(2, 4), Harmony::Steps(4, 7), Harmony::Octaves(-1)]),
];

#[derive(Clone, Copy, Debug)]
pub enum Harmony {
    /// Steps along the scale, or semitones in the chromatic scale.
    Steps(i32, i32),
    Octaves(i32),
}

/// How far off a note the voice has to go before it's snapped to the next, in semitones.
const STICK: f32 = 0.65;
/// How quickly the harmonies come in when there's a pitch, and go when there isn't.
const VOICED_SECS: f32 = 0.03;
/// Each harmony a little out from the others, in cents, so they sound like more voices.
const DETUNE: [f32; 3] = [7.0, -6.0, 4.0];
const TALK_SECS: f32 = 0.03;
const QUIET_SECS: f32 = 0.4;

/// A pitch in semitones (MIDI's numbering: 69 is A 440).
fn semitones(hz: f32) -> f32 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

/// The scale's notes, as semitones from C.
fn scale(id: &str) -> &'static [i32] {
    SCALES.iter().find(|(s, _)| *s == id).map_or(SCALES[0].1, |(_, n)| n)
}

/// Whether `note` is in the scale starting at `key`.
fn in_scale(note: i32, key: i32, notes: &[i32]) -> bool {
    notes.contains(&(note - key).rem_euclid(12))
}

/// The note in the scale nearest `pitch`.
fn nearest(pitch: f32, key: i32, notes: &[i32]) -> i32 {
    let around = pitch.round() as i32;
    (around - 6..=around + 6).filter(|n| in_scale(*n, key, notes)).min_by(|a, b| (*a as f32 - pitch).abs().total_cmp(&(*b as f32 - pitch).abs())).unwrap_or(around)
}

/// `steps` notes of the scale on from `note` (which is in it).
fn step(note: i32, steps: i32, key: i32, notes: &[i32]) -> i32 {
    let mut n = note;
    for _ in 0..steps.abs() {
        n += steps.signum();
        while !in_scale(n, key, notes) {
            n += steps.signum();
        }
    }
    n
}

/// "key", "scale", "snap" (0–100: how hard it snaps), "harmony" and "amount".
pub struct AutoTune {
    rate: u32,
    late: usize,
    pitch: Option<Arc<Pitch>>,
    tracker: Tracker,
    line: Line,
    now: f64,
    lead: Shift,
    harmonies: [Shift; 3],
    /// The note it's snapped to, and where the voice is being taken (both in semitones).
    note: Option<i32>,
    sung: f32,
    voiced: f32,
    talking: f32,
    input: Vec<f32>,
    background: Vec<Delay>,
}

impl AutoTune {
    pub fn new(rate: u32) -> Self {
        let late = crate::most_late(rate);
        debug_assert!(late >= pitch::shift_late(rate));
        Self {
            rate,
            late,
            pitch: None,
            tracker: Tracker::default(),
            line: Line::new(late * 2 + 64),
            now: 0.0,
            lead: Shift::default(),
            harmonies: Default::default(),
            note: None,
            sung: 0.0,
            voiced: 0.0,
            talking: 0.0,
            input: Vec::new(),
            background: Vec::new(),
        }
    }
}

impl SoundEffect for AutoTune {
    fn latency(&self) -> usize {
        self.late
    }

    fn use_networks(&mut self, networks: &Networks) {
        self.pitch.clone_from(&networks.pitch);
    }

    fn process(&mut self, stems: &mut Stems, settings: &Settings, _: &Scene) {
        let key = settings.choice("key").and_then(|k| KEYS.iter().position(|x| *x == k)).unwrap_or(0) as i32;
        let notes = scale(settings.choice("scale").unwrap_or("chromatic"));
        let snap = settings.number("snap").map_or(1.0, |s| s / 100.0).clamp(0.0, 1.0);
        let harmony = settings.choice("harmony").unwrap_or("none");
        let harmony = HARMONIES.iter().find(|(h, _)| *h == harmony).map_or(&[][..], |(_, v)| v);
        let amount = settings.number("amount").map_or(1.0, |a| a / 100.0).clamp(0.0, 1.0);
        let rate = self.rate as f32;
        let frames = stems.voice.len();
        let secs = frames as f32 / rate;

        let wanted = ((stems.speech - 0.3) / 0.4).clamp(0.0, 1.0);
        let mix_from = self.talking;
        self.talking += (wanted - self.talking) * (secs / if wanted > self.talking { TALK_SECS } else { QUIET_SECS }).min(1.0);
        if let Some(p) = &self.pitch {
            self.tracker.push(p, &stems.voice);
        }
        // The note: kept until the voice has gone well off it, and reached as quickly as the
        // snap says (at full snap, straight away: the robotic sound).
        let f0 = self.tracker.f0;
        let heard = f0.map(semitones);
        if let Some(h) = heard {
            let note = match self.note {
                Some(n) if (h - n as f32).abs() < STICK && in_scale(n, key, notes) => n,
                _ => nearest(h, key, notes),
            };
            if self.note.is_none() {
                self.sung = h;
            }
            self.note = Some(note);
            let glide = (1.0 - snap) * 0.2;
            self.sung += (note as f32 - self.sung) * if glide <= 0.0 { 1.0 } else { (secs / glide).min(1.0) };
        } else {
            self.note = None;
        }
        let ratios = |to: f32| heard.map_or(1.0, |h| 2f32.powf((to - h) / 12.0));
        let lead = ratios(self.sung);
        // The harmonies' notes, from the lead's.
        let mut sings = [1.0f32; 3];
        for ((s, h), cents) in sings.iter_mut().zip(harmony).zip(DETUNE) {
            let note = self.note.unwrap_or(0);
            let to = match *h {
                Harmony::Steps(steps, semis) => (if notes.len() == 12 { note + semis } else { step(note, steps, key, notes) }) as f32,
                Harmony::Octaves(o) => (note + 12 * o) as f32,
            };
            *s = ratios(to + self.sung - note as f32 + cents / 100.0);
        }
        let period = pitch::period(f0, self.rate);
        let voicing = follow(VOICED_SECS, rate);
        let target = if f0.is_some() { 1.0 } else { 0.0 };
        // The voice's echo is moved with it, so the voice as it was isn't heard in it.
        self.input.clear();
        self.input.extend(stems.voice.iter().zip(&stems.echo).map(|(v, e)| v + e));
        stems.echo.fill(0.0);
        let gain = 1.0 / (1.0 + 0.3 * harmony.len() as f32).sqrt();
        for (i, (x, y)) in self.input.iter().zip(&mut stems.voice).enumerate() {
            self.line.push(*x);
            self.now += 1.0;
            let t = self.now - self.late as f64;
            let dry = self.line.at(self.late as f32);
            let mut wet = self.lead.next(&self.line, self.now, t, period, lead, 1.0);
            self.voiced += (target - self.voiced) * voicing;
            for (shift, ratio) in self.harmonies.iter_mut().zip(sings).take(harmony.len()) {
                wet += 0.6 * self.voiced * shift.next(&self.line, self.now, t, period, ratio, 1.0);
            }
            let along = i as f32 / frames as f32;
            let changed = amount * (mix_from + (self.talking - mix_from) * along);
            *y = dry + (wet * gain - dry) * changed;
        }
        // The background's kept back as long as the voice.
        let late = self.late;
        self.background.resize_with(stems.background.len(), || Delay::new(late));
        for (d, c) in self.background.iter_mut().zip(&mut stems.background) {
            d.run(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    #[test]
    fn notes_and_steps_follow_the_scale() {
        let major = scale("major");
        // C major from C: 60 is C.
        assert_eq!(nearest(61.4, 0, major), 62);
        assert_eq!(nearest(60.4, 0, major), 60);
        assert_eq!(step(60, 2, 0, major), 64);
        assert_eq!(step(64, 2, 0, major), 67);
        assert_eq!(step(62, 2, 0, major), 65);
        // A minor's thirds.
        let minor = scale("minor");
        assert_eq!(step(57, 2, 9, minor), 60);
    }

    /// With the pitch network (`FLEX_DDSP_PITCH`), a voice sliding from 150 to 250 Hz comes out
    /// in steps: hard snapped, nearly every pitch heard is on a note.
    #[test]
    #[ignore]
    fn snaps_a_slide_to_notes() {
        let pitch = Arc::new(Pitch::load(&std::fs::read(std::env::var("FLEX_DDSP_PITCH").unwrap()).unwrap()).unwrap());
        let mut tune = AutoTune::new(RATE);
        tune.use_networks(&Networks { pitch: Some(pitch.clone()), ..Default::default() });
        let settings = Settings { numbers: vec![("snap".into(), 100.0)], choices: vec![("scale".into(), "chromatic".into())] };
        let len = RATE as usize * 3;
        let mut phase = 0.0f32;
        let mut f = crate::dsp::Biquad::bandpass(RATE as f32, 700.0, 2.0);
        let input: Vec<f32> = (0..len)
            .map(|i| {
                let hz = 150.0 * (250.0f32 / 150.0).powf(i as f32 / len as f32);
                phase += hz / RATE as f32;
                let pulse = if phase.fract() < hz / RATE as f32 { 1.0 } else { 0.0 };
                f.next(pulse) * 0.5
            })
            .collect();
        let mut out = Vec::new();
        for block in input.chunks(480) {
            let mut stems = Stems { rate: RATE, speech: 1.0, voice: block.to_vec(), echo: vec![0.0; block.len()], background: vec![], weights: vec![] };
            tune.process(&mut stems, &settings, &Scene::default());
            out.extend(stems.voice);
        }
        let heard = |sound: &[f32]| {
            let mut t = Tracker::default();
            let mut offs = Vec::new();
            for block in sound.chunks(960) {
                t.push(&pitch, block);
                if let Some(f0) = t.f0 {
                    let s = semitones(f0);
                    offs.push((s - s.round()).abs());
                }
            }
            offs.iter().skip(20).filter(|o| **o < 0.15).count() as f32 / offs.len().saturating_sub(20).max(1) as f32
        };
        let (before, after) = (heard(&input), heard(&out));
        println!("on a note: {before:.2} going in, {after:.2} coming out");
        assert!(after > 0.8 && after > before + 0.3);
    }
}
