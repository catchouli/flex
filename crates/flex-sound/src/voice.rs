//! The voice changer: plain signal processing on the voice, with the background left as it
//! was. Every voice comes out the same time late (`Changer::latency`), so switching between
//! them, or mixing one with the voice as it was, lines up.

use std::f32::consts::PI;

use std::collections::VecDeque;
use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use super::{Delay, Networks, Scene, Settings, SoundEffect, Stems};
use crate::ddsp::Pitch;
use crate::dsp::{Biquad, Line, Rng};
use crate::pitch::{self, Shift, Tracker};

/// The voices, by id: what the "voice" setting's options are.
pub const VOICES: [&str; 11] = ["chipmunk", "deep", "robot", "telephone", "monster", "alien", "whisper", "megaphone", "giant", "child", "swap"];

/// The voices that follow the voice's pitch, so need the pitch network.
pub const PITCHED: [&str; 3] = ["giant", "child", "swap"];

/// The sound effect: the "voice" setting picks the voice, and "amount" (0–100) how much of it.
pub struct VoiceChanger {
    changer: Changer,
    background: Vec<Delay>,
    voice: Vec<f32>,
    /// How much it's changing, following whether someone's talking.
    talking: f32,
}

impl VoiceChanger {
    pub fn new(rate: u32) -> Self {
        Self { changer: Changer::new(rate), background: Vec::new(), voice: Vec::new(), talking: 0.0 }
    }
}

/// How quickly the change comes in when someone starts talking, and goes once they stop.
const TALK_SECS: f32 = 0.03;
const QUIET_SECS: f32 = 0.4;

impl SoundEffect for VoiceChanger {
    fn latency(&self) -> usize {
        self.changer.latency()
    }

    fn use_networks(&mut self, networks: &Networks) {
        self.changer.pitch.clone_from(&networks.pitch);
    }

    fn process(&mut self, stems: &mut Stems, settings: &Settings, _: &Scene) {
        let voice = settings.choice("voice").unwrap_or("none");
        let mix = settings.number("amount").map_or(1.0, |a| a / 100.0);
        // Only while someone's talking, so what else ends up in the voice is left alone.
        let wanted = ((stems.speech - 0.3) / 0.4).clamp(0.0, 1.0);
        let secs = stems.voice.len() as f32 / stems.rate.max(1) as f32;
        let follow = secs / if wanted > self.talking { TALK_SECS } else { QUIET_SECS };
        self.talking += (wanted - self.talking) * follow.min(1.0);
        // The voice's echo is changed with it, so the voice as it was isn't heard in it.
        self.voice.clear();
        self.voice.extend(stems.voice.iter().zip(&stems.echo).map(|(v, e)| v + e));
        stems.echo.fill(0.0);
        if PITCHED.contains(&voice) {
            self.changer.hear(&stems.voice);
        }
        self.changer.run(&self.voice, &mut stems.voice, voice, mix * self.talking);
        let late = self.changer.latency();
        self.background.resize_with(stems.background.len(), || Delay::new(late));
        for (d, c) in self.background.iter_mut().zip(&mut stems.background) {
            d.run(c);
        }
    }
}

/// How long a pitch-shifting grain is.
const GRAIN_SECS: f32 = 0.04;
/// How long switching from one voice to another takes.
const SWITCH_SECS: f32 = 0.02;
/// How quickly the mix follows its control.
const MIX_SECS: f32 = 0.02;
/// The robot's ring modulator.
const ROBOT_HZ: f32 = 50.0;
/// The alien's ring modulator, and the monster's growl.
const ALIEN_HZ: f32 = 220.0;
const GROWL_HZ: f32 = 35.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Voice {
    None,
    Chipmunk,
    Deep,
    Robot,
    Telephone,
    Monster,
    Alien,
    Whisper,
    Megaphone,
    Giant,
    Child,
    Swap,
}

impl Voice {
    fn from_id(id: &str) -> Self {
        match id {
            "chipmunk" => Self::Chipmunk,
            "deep" => Self::Deep,
            "robot" => Self::Robot,
            "telephone" => Self::Telephone,
            "monster" => Self::Monster,
            "alien" => Self::Alien,
            "whisper" => Self::Whisper,
            "megaphone" => Self::Megaphone,
            "giant" => Self::Giant,
            "child" => Self::Child,
            "swap" => Self::Swap,
            _ => Self::None,
        }
    }
}

/// A shift in pitch by a ratio: two grains read back from the sound at a sliding delay,
/// each faded in and out so the jump back to the start of the next is never heard.
struct Shifter {
    ratio: f32,
    /// How far through its grain the first reader is (0–1); the other is half a grain on.
    phase: f32,
}

impl Shifter {
    fn new(semitones: f32) -> Self {
        Self { ratio: 2f32.powf(semitones / 12.0), phase: 0.0 }
    }

    /// The next sample, reading `line` from `late` samples back.
    fn next(&mut self, line: &Line, grain: f32, late: f32) -> f32 {
        let mut out = 0.0;
        for p in [self.phase, (self.phase + 0.5).fract()] {
            let gain = (PI * p).sin();
            out += gain * gain * line.at(late + 1.0 + p * grain);
        }
        // Reading faster than it's written raises the pitch: the delay shrinks.
        self.phase = (self.phase + (1.0 - self.ratio) / grain).rem_euclid(1.0);
        out
    }
}

/// One channel's voice changer.
struct Changer {
    rate: f32,
    line: Line,
    grain: f32,
    latency: usize,
    voice: Voice,
    /// The voice being switched from, and how far through the switch it is (0–1).
    from: Option<(Voice, f32)>,
    mix: f32,
    chipmunk: Shifter,
    deep: Shifter,
    monster: Shifter,
    alien: Shifter,
    robot: f32,
    /// The ring modulators' other phases: the alien's and the growl's.
    rings: [f32; 2],
    telephone: [Biquad; 4],
    megaphone: [Biquad; 4],
    monster_low: Biquad,
    whisper: Whisper,
    /// For the voices that follow the pitch: the network, what it's heard, the grain's period
    /// that gives, a shifter for each, and the time (samples since the start).
    pitch: Option<Arc<Pitch>>,
    tracker: Tracker,
    period: f32,
    pitched: [Shift; 3],
    now: f64,
}

impl Changer {
    fn new(rate: u32) -> Self {
        let rate = rate.max(8000) as f32;
        let grain = (GRAIN_SECS * rate).round();
        // As late as an effect can be, for the voices that follow the pitch to have a grain's
        // worth heard; the shifters' readers are on average half a grain behind that.
        let latency = super::most_late(rate as u32).max((grain / 2.0) as usize + 1).max(pitch::shift_late(rate as u32));
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let telephone = [Biquad::highpass(rate, 400.0, q), Biquad::highpass(rate, 400.0, q), Biquad::lowpass(rate, 3000.0, q), Biquad::lowpass(rate, 3000.0, q)];
        let megaphone = [Biquad::highpass(rate, 700.0, q), Biquad::highpass(rate, 700.0, q), Biquad::lowpass(rate, 3200.0, q), Biquad::peak(rate, 1800.0, 2.0, 8.0)];
        let whisper = Whisper::new(rate);
        assert!(whisper.latency() <= latency, "the whisper's no later than the others");
        Self {
            rate,
            line: Line::new(latency * 2 + grain as usize + 4),
            grain,
            latency,
            voice: Voice::None,
            from: None,
            mix: 1.0,
            chipmunk: Shifter::new(8.0),
            deep: Shifter::new(-5.0),
            monster: Shifter::new(-7.0),
            alien: Shifter::new(4.0),
            robot: 0.0,
            rings: [0.0; 2],
            telephone,
            megaphone,
            monster_low: Biquad::peak(rate, 120.0, 0.8, 6.0),
            whisper,
            pitch: None,
            tracker: Tracker::default(),
            period: pitch::period(None, rate as u32),
            pitched: Default::default(),
            now: 0.0,
        }
    }

    /// Hears the voice's pitch, for the voices that follow it.
    fn hear(&mut self, voice: &[f32]) {
        if let Some(p) = &self.pitch {
            self.tracker.push(p, voice);
        }
        self.period = pitch::period(self.tracker.f0, self.rate as u32);
    }

    /// How late the sound comes out, in samples.
    fn latency(&self) -> usize {
        self.latency
    }

    /// Changes `input` into `output` (the same length) with the voice called `voice` (none if
    /// it isn't one of `VOICES`), and `mix` (0–1) of it.
    pub fn run(&mut self, input: &[f32], output: &mut [f32], voice: &str, mix: f32) {
        let voice = Voice::from_id(voice);
        if voice != self.voice {
            self.from = Some((self.voice, 0.0));
            self.voice = voice;
        }
        let mix = if mix.is_finite() { mix.clamp(0.0, 1.0) } else { 1.0 };
        let mix_step = 1.0 - (-1.0 / (MIX_SECS * self.rate)).exp();
        let switch_step = 1.0 / (SWITCH_SECS * self.rate);
        for (x, y) in input.iter().zip(output.iter_mut()) {
            // Whatever comes in, nothing out of range goes back to mpv.
            self.line.push(if x.is_finite() { *x } else { 0.0 });
            self.now += 1.0;
            self.mix += (mix - self.mix) * mix_step;
            let dry = self.line.at(self.latency as f32);
            let mut wet = self.changed(self.voice, dry);
            if let Some((from, done)) = self.from {
                let old = self.changed(from, dry);
                let fade = (done * PI / 2.0).sin();
                wet = old * (1.0 - fade * fade) + wet * fade * fade;
                self.from = (done + switch_step < 1.0).then_some((from, done + switch_step));
            }
            *y = dry + (wet - dry) * self.mix;
        }
    }

    /// One sample of the sound through `voice`; `dry` is it unchanged, as late as every voice is.
    fn changed(&mut self, voice: Voice, dry: f32) -> f32 {
        match voice {
            Voice::None => dry,
            Voice::Chipmunk => self.chipmunk.next(&self.line, self.grain, self.shift_late()),
            Voice::Deep => self.deep.next(&self.line, self.grain, self.shift_late()),
            Voice::Robot => {
                self.robot = (self.robot + ROBOT_HZ / self.rate).fract();
                dry * (2.0 * PI * self.robot).sin() * 1.4
            }
            Voice::Telephone => {
                let narrow = self.telephone.iter_mut().fold(dry, |x, f| f.next(x));
                (narrow * 3.0).tanh() * 0.6
            }
            Voice::Monster => {
                // Lower, rougher and growling.
                let low = self.monster_low.next(self.monster.next(&self.line, self.grain, self.shift_late()));
                self.rings[1] = (self.rings[1] + GROWL_HZ / self.rate).fract();
                (low * 2.5).tanh() * 0.5 * (0.75 + 0.25 * (2.0 * PI * self.rings[1]).sin())
            }
            Voice::Alien => {
                let up = self.alien.next(&self.line, self.grain, self.shift_late());
                self.rings[0] = (self.rings[0] + ALIEN_HZ / self.rate).fract();
                up * (0.5 + 0.7 * (2.0 * PI * self.rings[0]).sin())
            }
            Voice::Whisper => {
                let x = self.line.at((self.latency - self.whisper.latency()) as f32);
                self.whisper.next(x)
            }
            Voice::Megaphone => {
                let narrow = self.megaphone.iter_mut().fold(dry, |x, f| f.next(x));
                (narrow * 5.0).tanh() * 0.4
            }
            // Pitch and formants moved together, as a bigger or smaller person's would be.
            Voice::Giant => self.follow_pitch(0, 0.75, 0.8),
            Voice::Child => self.follow_pitch(1, 1.45, 1.22),
            Voice::Swap => {
                // Low voices up, high ones down.
                let low = self.tracker.average.is_none_or(|a| a < 165f32.log2());
                if low { self.follow_pitch(2, 1.65, 1.17) } else { self.follow_pitch(2, 0.62, 0.86) }
            }
        }
    }

    /// How far back the grain shifters read from, beyond their grain.
    fn shift_late(&self) -> f32 {
        self.latency as f32 - (self.grain / 2.0).floor() - 1.0
    }

    fn follow_pitch(&mut self, which: usize, ratio: f32, squeeze: f32) -> f32 {
        let t = self.now - self.latency as f64;
        self.pitched[which].next(&self.line, self.now, t, self.period, ratio, squeeze)
    }
}

/// A whisper: the voice's spectrum, smoothed so the pitch's lines are gone, given to noise
/// (each window's phases picked at random).
struct Whisper {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    heard: VecDeque<f32>,
    /// What's been made, added up over the windows, and what's done, waiting to go out.
    sum: Vec<f32>,
    out: VecDeque<f32>,
    new: usize,
    spectrum: Vec<Complex32>,
    smooth: Vec<f32>,
    frame: Vec<f32>,
    rng: Rng,
    /// For testing: keep the phases, so it gives back the sound as it came.
    keep: bool,
}

/// About 16 ms windows, four over each sample, and the bins (about 60 Hz each) either side
/// the spectrum's smoothed over.
const WHISPER_SECS: f32 = 0.016;
const WHISPER_SMOOTH: usize = 5;

impl Whisper {
    fn new(rate: f32) -> Self {
        let n = ((WHISPER_SECS * rate) as usize / 4).max(4) * 4;
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(n);
        let inverse = planner.plan_fft_inverse(n);
        // Square roots of a Hann window, used going in and coming out: their product sums to 2
        // over four windows.
        let window = (0..n).map(|i| (PI * i as f32 / n as f32).sin()).collect();
        let spectrum = forward.make_output_vec();
        Self {
            forward,
            inverse,
            window,
            heard: std::iter::repeat_n(0.0, n).collect(),
            sum: vec![0.0; n],
            out: std::iter::repeat_n(0.0, n / 4 - 1).collect(),
            new: 0,
            smooth: vec![0.0; spectrum.len()],
            spectrum,
            frame: vec![0.0; n],
            rng: Rng::new(0x3157),
            keep: false,
        }
    }

    /// How late it is, in samples.
    fn latency(&self) -> usize {
        self.window.len() - 1
    }

    fn next(&mut self, x: f32) -> f32 {
        let n = self.window.len();
        self.heard.pop_front();
        self.heard.push_back(x);
        self.new += 1;
        if self.new == n / 4 {
            self.new = 0;
            for ((f, h), w) in self.frame.iter_mut().zip(&self.heard).zip(&self.window) {
                *f = h * w;
            }
            if self.forward.process(&mut self.frame, &mut self.spectrum).is_ok() {
                if !self.keep {
                    let bins = self.spectrum.len();
                    for (k, s) in self.smooth.iter_mut().enumerate() {
                        let (lo, hi) = (k.saturating_sub(WHISPER_SMOOTH), (k + WHISPER_SMOOTH + 1).min(bins));
                        *s = self.spectrum[lo..hi].iter().map(|c| c.norm()).sum::<f32>() / (hi - lo) as f32;
                    }
                    for (c, s) in self.spectrum.iter_mut().zip(&self.smooth) {
                        *c = Complex32::from_polar(*s, 2.0 * PI * self.rng.unit());
                    }
                    // The first and last bins have to be real.
                    for c in [0, bins - 1] {
                        self.spectrum[c].im = 0.0;
                    }
                }
                if self.inverse.process(&mut self.spectrum, &mut self.frame).is_ok() {
                    // The transforms scale by n; the windows overlapping add up to 2.
                    let scale = 1.0 / (n as f32 * 2.0);
                    for ((s, f), w) in self.sum.iter_mut().zip(&self.frame).zip(&self.window) {
                        *s += f * w * scale;
                    }
                }
            }
            self.out.extend(self.sum.drain(..n / 4));
            self.sum.resize(n, 0.0);
        }
        // Louder, as noise sounds quieter than the voice it came from.
        let y = self.out.pop_front().unwrap_or(0.0);
        if self.keep { y } else { y * 1.6 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn sine(hz: f32, len: usize) -> Vec<f32> {
        (0..len).map(|i| (2.0 * PI * hz * i as f32 / RATE as f32).sin() * 0.5).collect()
    }

    fn run(voice: &str, input: &[f32]) -> Vec<f32> {
        let mut changer = Changer::new(RATE);
        let mut out = vec![0.0; input.len()];
        // In uneven pieces, as libavfilter hands them over.
        let mut at = 0;
        for size in [1, 700, 1024, 333].into_iter().cycle() {
            if at >= input.len() {
                break;
            }
            let end = (at + size).min(input.len());
            changer.run(&input[at..end], &mut out[at..end], voice, 1.0);
            at = end;
        }
        out
    }

    /// The pitch: the loudest frequency between 100 and 800 Hz, to half a hertz.
    fn pitch(sound: &[f32]) -> f32 {
        let power = |hz: f32| {
            let w = 2.0 * PI * hz / RATE as f32;
            let (re, im) = sound.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, x)| (re + x * (w * i as f32).cos(), im + x * (w * i as f32).sin()));
            re * re + im * im
        };
        (200..1600).map(|h| h as f32 / 2.0).max_by(|a, b| power(*a).total_cmp(&power(*b))).unwrap_or(0.0)
    }

    #[test]
    fn no_voice_is_the_sound_exactly_as_late_as_it_says() {
        let input: Vec<f32> = (0..5000).map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5).collect();
        let out = run("none", &input);
        let late = Changer::new(RATE).latency();
        assert!(out[..late].iter().all(|&y| y == 0.0));
        for (y, x) in out[late..].iter().zip(&input) {
            assert!((y - x).abs() < 1e-6);
        }
    }

    #[test]
    fn chipmunk_and_deep_move_the_pitch() {
        let input = sine(220.0, RATE as usize);
        let tail = |v: &[f32]| v[RATE as usize / 4..].to_vec();
        let up = pitch(&tail(&run("chipmunk", &input)));
        let down = pitch(&tail(&run("deep", &input)));
        // Grains that aren't lined up with the pitch spread it over lines about 15 Hz apart
        // (two grains, each restarting every grain / |1 − ratio|), so the loudest can be a line off.
        assert!((up / 220.0 / 2f32.powf(8.0 / 12.0) - 1.0).abs() < 0.05, "chipmunk: {up} Hz");
        assert!((down / 220.0 / 2f32.powf(-5.0 / 12.0) - 1.0).abs() < 0.05, "deep: {down} Hz");
    }

    #[test]
    fn the_telephone_drops_the_low_end() {
        let rms = |v: &[f32]| (v[RATE as usize / 4..].iter().map(|x| x * x).sum::<f32>() / (v.len() - RATE as usize / 4) as f32).sqrt();
        let low = rms(&run("telephone", &sine(80.0, RATE as usize)));
        let middle = rms(&run("telephone", &sine(1000.0, RATE as usize)));
        assert!(low < middle * 0.1, "80 Hz {low}, 1 kHz {middle}");
    }

    #[test]
    fn switching_voices_doesnt_click() {
        let input = sine(300.0, RATE as usize * 2);
        let mut changer = Changer::new(RATE);
        let mut out = vec![0.0; input.len()];
        for (i, (x, y)) in input.chunks(480).zip(out.chunks_mut(480)).enumerate() {
            let voices: Vec<&str> = std::iter::once("none").chain(VOICES).collect();
            changer.run(x, y, voices[i / 10 % voices.len()], 1.0);
        }
        // A 300 Hz sine at 0.5 moves at most about 0.02 a sample; the chipmunk's more.
        let worst = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(worst < 0.1, "biggest jump {worst}");
    }

    #[test]
    fn the_whisper_gives_the_sound_back_as_late_as_it_says_when_it_keeps_the_phases() {
        let mut whisper = Whisper::new(RATE as f32);
        whisper.keep = true;
        let input: Vec<f32> = (0..5000).map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5).collect();
        let out: Vec<f32> = input.iter().map(|x| whisper.next(*x)).collect();
        let late = whisper.latency();
        // Once the windows have filled up.
        for i in late + whisper.window.len()..input.len() {
            assert!((out[i] - input[i - late]).abs() < 1e-4, "at {i}: {} vs {}", out[i], input[i - late]);
        }
    }

    #[test]
    fn nothing_out_of_range_comes_out() {
        let mut changer = Changer::new(RATE);
        let mut out = [0.0; 4];
        for voice in ["none"].into_iter().chain(VOICES) {
            changer.run(&[f32::NAN, f32::INFINITY, 0.5, -0.5], &mut out, voice, f32::NAN);
            assert!(out.iter().all(|y| y.is_finite()));
        }
    }
}
