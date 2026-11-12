//! Hearing a voice's pitch (`Tracker`, with DDSP's pitch network), and moving it (`Shift`): a
//! pitch-synchronous overlap-add shifter (TD-PSOLA). Each grain is two of the voice's periods,
//! cut out where the voice is now and laid down closer together (higher) or further apart
//! (lower), so the voice's own sound (its formants) stays where it was. Squeezing each grain
//! moves the formants too, for a bigger or smaller person.

use std::collections::VecDeque;
use std::f32::consts::PI;

use crate::ddsp::{self, Pitch};
use crate::dsp::Line;
use crate::resample::Down;

/// How sure of a pitch the network has to be, and how loud the voice, to count as voiced.
pub const SURE: f32 = 0.3;
pub const QUIETEST_DB: f32 = -55.0;
/// The lowest pitch a grain's cut to: lower voices get a grain that isn't their period (a
/// little rougher), so a grain always fits in what's been heard.
const LOWEST_HZ: f32 = 75.0;
/// The grain's period where there's no pitch (consonants, breaths, music).
const UNVOICED_SECS: f32 = 0.008;

/// A voice's pitch, every 20 ms, from 64 ms of it.
pub struct Tracker {
    down: Down,
    heard: VecDeque<f32>,
    new: usize,
    /// The last pitch heard (Hz) if it was voiced, and how loud it was (dB).
    pub f0: Option<f32>,
    pub db: f32,
    /// The voice's usual pitch lately, as log2 Hz.
    pub average: Option<f32>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self { down: Down::default(), heard: std::iter::repeat_n(0.0, ddsp::FRAME).collect(), new: 0, f0: None, db: -80.0, average: None }
    }
}

impl Tracker {
    /// Hears `sound`, at 48 kHz.
    pub fn push(&mut self, pitch: &Pitch, sound: &[f32]) {
        for &x in sound {
            let Some(y) = self.down.push(x) else { continue };
            self.heard.pop_front();
            self.heard.push_back(y);
            self.new += 1;
            if self.new == ddsp::HOP {
                self.new = 0;
                let frame: Vec<f32> = self.heard.iter().copied().collect();
                let heard = pitch.hear(&frame);
                self.db = heard.db;
                self.f0 = (heard.confidence >= SURE && heard.db > QUIETEST_DB).then_some(heard.f0_hz);
                if let Some(f0) = self.f0 {
                    let average = self.average.get_or_insert(f0.log2());
                    *average += (f0.log2() - *average) / 100.0;
                }
            }
        }
    }
}

/// How late what's heard should be played at least, for a grain to fit before it: `Shift` is
/// handed the time it plays, at least this long before the newest sample.
pub fn shift_late(rate: u32) -> usize {
    // A grain reaches a period either side of where it's cut, which can be half a period from
    // where it's laid, and squeezing makes it reach further.
    (rate as f32 / LOWEST_HZ * (1.0 + 0.5 + 1.0 / MOST_SQUEEZED)) as usize + 2
}

/// The least a grain's squeezed (its formants moved down).
pub const MOST_SQUEEZED: f32 = 0.75;

struct Grain {
    /// Where it's laid, where it was cut from (both in samples since the start), and half its
    /// length as cut.
    out: f64,
    from: f64,
    half: f32,
    squeeze: f32,
}

/// One shifted voice, reading what's been heard from a `Line` its owner keeps.
#[derive(Default)]
pub struct Shift {
    /// Where the next grain's laid, and where the last was cut from.
    next: Option<f64>,
    cut: f64,
    grains: Vec<Grain>,
}

impl Shift {
    /// The sound at time `t` (samples since the start), from `line`, whose newest sample is at
    /// `now`: the voice, of period `period` samples, `ratio` times the pitch, and its formants
    /// `squeeze` times as high.
    pub fn next(&mut self, line: &Line, now: f64, t: f64, period: f32, ratio: f32, squeeze: f32) -> f32 {
        let squeeze = squeeze.clamp(MOST_SQUEEZED, 1.5);
        let ratio = ratio.clamp(0.25, 4.0);
        let next = self.next.get_or_insert_with(|| {
            self.cut = t;
            t
        });
        while *next - (period / squeeze) as f64 <= t {
            // Cut where the voice is when it's laid, whole periods on from the last cut, so
            // the grains line up with each other.
            let p = period as f64;
            if (self.cut - *next).abs() > 4.0 * p {
                self.cut = *next;
            }
            while self.cut + p / 2.0 < *next {
                self.cut += p;
            }
            self.grains.push(Grain { out: *next, from: self.cut, half: period, squeeze });
            *next += p / ratio as f64;
        }
        let mut y = 0.0;
        for g in &self.grains {
            let d = ((t - g.out) as f32) * g.squeeze;
            if d.abs() < g.half {
                let w = 0.5 + 0.5 * (PI * d / g.half).cos();
                y += w * line.at((now - g.from - d as f64) as f32);
            }
        }
        self.grains.retain(|g| ((t - g.out) as f32) * g.squeeze < g.half);
        // Grains every period / ratio, each period / squeeze either side: on average they add
        // up to ratio / squeeze.
        y * (squeeze / ratio).clamp(0.25, 4.0)
    }
}

/// A grain's period for a pitch, at `rate`: the voice's period, or a short one where there's
/// no pitch.
pub fn period(f0: Option<f32>, rate: u32) -> f32 {
    match f0 {
        Some(hz) => rate as f32 / hz.max(LOWEST_HZ),
        None => UNVOICED_SECS * rate as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn run(input: &[f32], hz: Option<f32>, ratio: f32, squeeze: f32) -> Vec<f32> {
        let late = shift_late(RATE);
        let mut line = Line::new(late * 2 + 64);
        let mut shift = Shift::default();
        input
            .iter()
            .enumerate()
            .map(|(i, x)| {
                line.push(*x);
                let now = i as f64;
                shift.next(&line, now, now - late as f64, period(hz, RATE), ratio, squeeze)
            })
            .collect()
    }

    #[test]
    fn unshifted_it_gives_the_sound_back_as_late_as_it_says() {
        let input: Vec<f32> = (0..20_000).map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5).collect();
        let late = shift_late(RATE);
        for hz in [None, Some(110.0), Some(240.0)] {
            let out = run(&input, hz, 1.0, 1.0);
            for i in late + 2000..input.len() {
                assert!((out[i] - input[i - late]).abs() < 1e-4, "{hz:?} at {i}: {} vs {}", out[i], input[i - late]);
            }
        }
    }

    /// A buzz at `hz`: a pulse every period, through a resonance at 800 Hz (a formant).
    fn buzz(hz: f32, len: usize) -> Vec<f32> {
        let mut f = crate::dsp::Biquad::bandpass(RATE as f32, 800.0, 4.0);
        (0..len).map(|i| f.next(if (i as f32 * hz / RATE as f32).fract() < hz / RATE as f32 { 1.0 } else { 0.0 })).collect()
    }

    /// The strongest frequency between `lo` and `hi`.
    fn loudest(sound: &[f32], lo: f32, hi: f32) -> f32 {
        let power = |hz: f32| {
            let w = 2.0 * PI * hz / RATE as f32;
            let (re, im) = sound.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, x)| (re + x * (w * i as f32).cos(), im + x * (w * i as f32).sin()));
            re * re + im * im
        };
        (lo as usize..hi as usize).map(|h| h as f32).max_by(|a, b| power(*a).total_cmp(&power(*b))).unwrap_or(0.0)
    }

    #[test]
    fn it_moves_the_pitch_and_keeps_the_formant_unless_squeezed() {
        let input = buzz(150.0, RATE as usize / 2);
        let tail = |v: Vec<f32>| v[RATE as usize / 4..].to_vec();
        let up = tail(run(&input, Some(150.0), 1.5, 1.0));
        // The pulses come 1.5 times as often: the spectrum's lines 225 Hz apart.
        let first = loudest(&up, 180.0, 270.0);
        assert!((first - 225.0).abs() < 4.0, "{first} Hz");
        // The formant hasn't moved; squeezed, it has.
        let formant = loudest(&up, 500.0, 1300.0);
        assert!((formant - 800.0).abs() < 120.0, "formant at {formant} Hz");
        let squeezed = tail(run(&input, Some(150.0), 1.5, 1.3));
        let moved = loudest(&squeezed, 500.0, 1500.0);
        assert!(moved > 950.0, "squeezed formant at {moved} Hz");
    }
}
