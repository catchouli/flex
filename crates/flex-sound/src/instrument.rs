//! Playing everyone's voice on an instrument, with DDSP-VST (`ddsp`): the voice's pitch and
//! loudness, every 20 ms, played by the instrument's network. Voices are moved by whole octaves
//! into the instrument's range, and brought to the loudness it was trained on.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::ddsp::{self, Controls, Harmonic, Instrument, Noise, Pitch};
use crate::resample::{Down, Up};
use crate::{Delay, Networks, Scene, Settings, SoundEffect, Stems};

/// How late what it plays is, at 48 kHz: the pitch network's frame is 64 ms, and it plays what
/// it heard in the middle of it.
pub const LATE_SECS: f32 = 0.05;
/// How sure of a pitch the network has to be to play it.
const SURE: f32 = 0.3;
/// How long a note's held through what isn't voiced (a consonant, a breath), in frames of 20 ms,
/// and how quickly it fades meanwhile (dB a frame).
const HOLD_FRAMES: u32 = 8;
const HOLD_FALL_DB: f32 = 4.0;
/// How quickly the octave a speaker's moved by follows their voice, in frames of 20 ms.
const OCTAVE_FRAMES: f32 = 100.0;
/// How quickly the loudest the voice has been comes down, in dB a frame, and the quietest it's
/// taken to be: what's left in the voice when no one's talking isn't brought up to an
/// instrument's loudness.
const PEAK_FALL_DB: f32 = 0.05;
const QUIETEST_PEAK_DB: f32 = -35.0;
/// The quietest a voice has to be to be played.
const QUIETEST_DB: f32 = -55.0;
/// How quickly it comes in when someone starts talking, and goes once they stop.
const TALK_SECS: f32 = 0.03;
const QUIET_SECS: f32 = 0.3;

pub struct InstrumentPlayer {
    rate: u32,
    networks: Networks,
    /// The instrument playing, by id, and its network.
    playing: Option<(String, Arc<Instrument>)>,
    down: Down,
    up: Up,
    /// The last frame's worth of the voice at 16 kHz, and how much of it is new.
    heard: VecDeque<f32>,
    new: usize,
    /// What's been played, at 48 kHz, waiting to go out.
    played: VecDeque<f32>,
    harmonic: Harmonic,
    noise: Noise,
    gru: Vec<f32>,
    /// The voice's pitch held through what isn't voiced, its average (log2 Hz), and the
    /// loudest it's been lately (dB).
    f0: f32,
    average: Option<f32>,
    peak: f32,
    /// How loud the last voiced frame was played, and how many frames since.
    held_db: f32,
    unvoiced: u32,
    talking: f32,
    dry: Delay,
    echo: Delay,
    background: Vec<Delay>,
    synth: Vec<f32>,
    up_out: Vec<f32>,
}

impl InstrumentPlayer {
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            networks: Networks::default(),
            playing: None,
            down: Down::default(),
            up: Up::default(),
            heard: std::iter::repeat_n(0.0, ddsp::FRAME).collect(),
            new: 0,
            // Enough that there's always a block's worth, until the next 20 ms are played.
            played: std::iter::repeat_n(0.0, 2 * ddsp::HOP * crate::resample::FACTOR).collect(),
            harmonic: Harmonic::default(),
            noise: Noise::default(),
            gru: vec![0.0; 512],
            f0: 0.0,
            average: None,
            peak: -80.0,
            held_db: -80.0,
            unvoiced: HOLD_FRAMES + 1,
            talking: 0.0,
            dry: Delay::new(late(rate)),
            echo: Delay::new(late(rate)),
            background: Vec::new(),
            synth: vec![0.0; ddsp::HOP],
            up_out: Vec::new(),
        }
    }

    /// Plays the last frame heard, onto the end of `played`.
    fn play(&mut self, pitch: &Pitch, instrument: &Instrument) {
        let frame: Vec<f32> = self.heard.iter().copied().collect();
        let heard = pitch.hear(&frame);
        let voiced = heard.confidence >= SURE && heard.db > QUIETEST_DB;
        if voiced {
            self.f0 = heard.f0_hz;
            let log = heard.f0_hz.log2();
            let average = self.average.get_or_insert(log);
            *average += (log - *average) / OCTAVE_FRAMES;
            self.peak = self.peak.max(heard.db);
        }
        self.peak = (self.peak - PEAK_FALL_DB).max(QUIETEST_PEAK_DB);
        // Moved by the octaves that bring the speaker's usual pitch nearest the middle of the
        // instrument's.
        let (lo, hi) = instrument.range;
        let middle = (lo.log2() + hi.log2()) / 2.0;
        let octaves = self.average.map_or(0.0, |a| (middle - a).round());
        let f0 = self.f0.max(20.0) * 2f32.powf(octaves);
        let db = if voiced {
            self.unvoiced = 0;
            self.held_db = (heard.db + instrument.power.1 - self.peak).clamp(-80.0, 0.0);
            self.held_db
        } else {
            // Held, fading, through a short gap, so a sentence doesn't start the note again at
            // every consonant.
            self.unvoiced = self.unvoiced.saturating_add(1);
            if self.unvoiced <= HOLD_FRAMES { (self.held_db - HOLD_FALL_DB * self.unvoiced as f32).max(-80.0) } else { -80.0 }
        };
        let controls: Controls = instrument.play(ddsp::pitch_scaled(f0), db / 80.0 + 1.0, &mut self.gru);
        let mut noise = vec![0.0; ddsp::HOP];
        self.harmonic.render(&controls, f0, &mut self.synth);
        self.noise.render(&controls.noise, &mut noise);
        self.up_out.clear();
        for (h, n) in self.synth.iter().zip(&noise) {
            self.up.push(h + n, &mut self.up_out);
        }
        self.played.extend(&self.up_out);
    }
}

fn late(rate: u32) -> usize {
    (LATE_SECS * rate as f32) as usize
}

impl SoundEffect for InstrumentPlayer {
    fn latency(&self) -> usize {
        late(self.rate)
    }

    fn use_networks(&mut self, networks: &Networks) {
        self.networks.clone_from(networks);
    }

    fn process(&mut self, stems: &mut Stems, settings: &Settings, _: &Scene) {
        let mix = settings.number("amount").map_or(1.0, |a| a / 100.0);
        // On while someone's talking: as soon as the voice has a clear pitch (the voice is
        // DeepFilterNet's, so it's speech), or once the speech network's sure.
        let pitched = if self.unvoiced <= HOLD_FRAMES { 1.0 } else { 0.0 };
        let wanted = ((stems.speech - 0.3) / 0.4).clamp(0.0, 1.0).max(pitched);
        let secs = stems.voice.len() as f32 / stems.rate.max(1) as f32;
        let follow = secs / if wanted > self.talking { TALK_SECS } else { QUIET_SECS };
        let start = self.talking;
        self.talking += (wanted - self.talking) * follow.min(1.0);

        // The instrument picked, once its network's here; a new one starts from silence.
        let id = settings.choice("instrument").unwrap_or("flute");
        if self.playing.as_ref().is_none_or(|(p, _)| p != id) {
            self.playing = self.networks.instruments.iter().find(|(i, _)| i == id).map(|(i, n)| (i.clone(), n.clone()));
            self.gru.fill(0.0);
        }
        let ready = self.rate == ddsp::RATE * crate::resample::FACTOR as u32 && self.playing.is_some() && self.networks.pitch.is_some();

        let frames = stems.voice.len();
        if ready {
            for &x in &stems.voice {
                if let Some(y) = self.down.push(x) {
                    self.heard.pop_front();
                    self.heard.push_back(y);
                    self.new += 1;
                    if self.new == ddsp::HOP {
                        self.new = 0;
                        let pitch = self.networks.pitch.clone().expect("checked above");
                        let instrument = self.playing.as_ref().expect("checked above").1.clone();
                        self.play(&pitch, &instrument);
                    }
                }
            }
        }
        self.dry.run(&mut stems.voice);
        self.echo.run(&mut stems.echo);
        // The voice, and its echo, go as the instrument comes in.
        for (i, (v, e)) in stems.voice.iter_mut().zip(&mut stems.echo).enumerate() {
            let played = if ready { self.played.pop_front().unwrap_or(0.0) } else { 0.0 };
            let t = mix * (start + (self.talking - start) * i as f32 / frames as f32);
            *v = *v * (1.0 - t) + played * t;
            *e *= 1.0 - t;
        }
        let late = late(self.rate);
        self.background.resize_with(stems.background.len(), || Delay::new(late));
        for (d, c) in self.background.iter_mut().zip(&mut stems.background) {
            d.run(c);
        }
    }
}
