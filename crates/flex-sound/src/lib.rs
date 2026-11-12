//! Flex's sound effects. The sound mpv is about to play comes here on mpv's audio thread,
//! through Flex's patched libmpv, and goes through 10 ms at a time. Each block is split into the
//! voice and everything else (`split`, then DeepFilterNet3 once Flex has loaded it:
//! `deepfilter`), the chosen effect changes either or both, and they're put back together.
//! Silero VAD (`vad`) says when someone's talking.
//! While video effects fade from one to the next, the sound effects fade at the same time.
//!
//! A sound effect is a `SoundEffect`, made by `make` from its id, and described for the panel
//! in `sounds/<id>.toml` (`EFFECTS`). It's chosen on its own, or paired with video effects by
//! shuffle, and changes at the same moments they do. Effects can also look at what Flex knows
//! about the picture and the sound (`Scene`).

mod autotune;
mod circle;
pub mod deepfilter;
pub mod ddsp;
mod dialogue;
mod dsp;
mod echo;
mod instrument;
mod old;
mod pitch;
mod places;
mod resample;
mod split;
mod stutter;
mod synth;
pub mod vad;
mod voice;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

pub use split::Split;
pub use autotune::{HARMONIES, KEYS, SCALES};
pub use dialogue::MODES;
pub use old::STYLES;
pub use places::SPACES;
pub use synth::SOUNDS;
pub use voice::VOICES;

/// The sound effects, by id, each with its name, description and settings (the same as a video
/// effect's `.toml`, which the panel shows).
pub const EFFECTS: [(&str, &str); 11] = [
    ("voice", include_str!("../sounds/voice.toml")),
    ("instrument", include_str!("../sounds/instrument.toml")),
    ("reverb", include_str!("../sounds/reverb.toml")),
    ("underwater", include_str!("../sounds/underwater.toml")),
    ("echo", include_str!("../sounds/echo.toml")),
    ("old", include_str!("../sounds/old.toml")),
    ("stutter", include_str!("../sounds/stutter.toml")),
    ("circle", include_str!("../sounds/circle.toml")),
    ("dialogue", include_str!("../sounds/dialogue.toml")),
    ("autotune", include_str!("../sounds/autotune.toml")),
    ("synth", include_str!("../sounds/synth.toml")),
];

/// The most an effect may keep the sound back, in seconds. Every effect's sound comes out
/// this late, whatever it takes itself, so they line up when fading from one to another.
const MOST_LATE: f32 = 0.05;
/// How quickly the fade follows Flex's, which moves once a frame.
const FADE_STEP_SECS: f32 = 0.05;

/// Makes a sound effect, by id, for sound at `rate`, with the networks loaded so far.
pub fn make(id: &str, rate: u32, networks: &Networks) -> Option<Box<dyn SoundEffect>> {
    let mut effect: Box<dyn SoundEffect> = match id {
        "voice" => Box::new(voice::VoiceChanger::new(rate)),
        "instrument" => Box::new(instrument::InstrumentPlayer::new(rate)),
        "reverb" => Box::new(places::Reverberate::new(rate)),
        "underwater" => Box::new(places::Underwater::new(rate)),
        "echo" => Box::new(echo::Echo::new(rate)),
        "old" => Box::new(old::OldSound::new(rate)),
        "stutter" => Box::new(stutter::Stutter::new(rate)),
        "circle" => Box::new(circle::Circle::new(rate)),
        "dialogue" => Box::new(dialogue::Dialogue::new(rate)),
        "autotune" => Box::new(autotune::AutoTune::new(rate)),
        "synth" => Box::new(synth::Synth::new(rate)),
        _ => return None,
    };
    effect.use_networks(networks);
    Some(effect)
}

/// Whether the sound effect `id`, with `settings`, needs DDSP's pitch network.
pub fn wants_pitch(id: &str, settings: &Settings) -> bool {
    match id {
        "instrument" | "autotune" | "synth" => true,
        "voice" => settings.choice("voice").is_some_and(|v| voice::PITCHED.contains(&v)),
        _ => false,
    }
}

/// A sound effect.
pub trait SoundEffect: Send {
    /// How late what it gives back is, in frames (no more than `MOST_LATE`). Anything it passes
    /// through unchanged has to be kept back this long too (`Delay` does that).
    fn latency(&self) -> usize {
        0
    }

    /// Whether it changes the voice on its own, so the sound has to be split. Splitting costs
    /// nothing yet, but will once a network does it.
    fn wants_voice(&self) -> bool {
        true
    }

    /// The networks loaded so far, and again whenever more are.
    fn use_networks(&mut self, _: &Networks) {}

    /// Changes one block of sound.
    fn process(&mut self, stems: &mut Stems, settings: &Settings, scene: &Scene);
}

/// One block of sound, split.
#[derive(Clone, Default)]
pub struct Stems {
    pub rate: u32,
    /// How likely it is someone's talking, 0–1 (1 while there's no network to tell).
    pub speech: f32,
    /// The voices, as one channel.
    pub voice: Vec<f32>,
    /// What's left of the voices in the sound (their reverb, and what the network wasn't sure
    /// was voice), as one channel. It goes back with the background unless an effect that
    /// replaces the voice takes it out.
    pub echo: Vec<f32>,
    /// Everything else, channel by channel, as the sound came.
    pub background: Vec<Vec<f32>>,
    /// How much of the voice (and its echo) goes back into each channel.
    pub weights: Vec<f32>,
}

impl Stems {
    /// Puts the voice and its echo back into the channels, for effects that change everything.
    pub fn mix_down(&mut self) {
        for (c, w) in self.background.iter_mut().zip(&self.weights) {
            for ((b, v), e) in c.iter_mut().zip(&self.voice).zip(&self.echo) {
                *b += w * (v + e);
            }
        }
        self.voice.fill(0.0);
        self.echo.fill(0.0);
    }

    /// Everything as one channel: the voice and its echo, and the background's middle.
    pub fn mono(&self, out: &mut Vec<f32>) {
        out.clear();
        out.extend(self.voice.iter().zip(&self.echo).map(|(v, e)| v + e));
        for c in &self.background {
            for (o, b) in out.iter_mut().zip(c) {
                *o += 0.5 * b;
            }
        }
    }

    /// Everything replaced by one channel, played from where the voice is.
    pub fn set_mono(&mut self, mono: &[f32]) {
        self.voice.clear();
        self.voice.extend_from_slice(mono);
        self.echo.fill(0.0);
        for c in &mut self.background {
            c.fill(0.0);
        }
    }
}

/// A sound effect's settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub numbers: Vec<(String, f32)>,
    /// Choice settings, by the chosen option's id.
    pub choices: Vec<(String, String)>,
}

impl Settings {
    pub fn number(&self, key: &str) -> Option<f32> {
        self.numbers.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    pub fn choice(&self, key: &str) -> Option<&str> {
        self.choices.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// What Flex knows about what's playing, for effects to follow.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    /// Faces on screen.
    pub faces: Vec<Face>,
    /// How likely each kind of sound is (flex-depth's `sound::KINDS`), 0–1.
    pub kinds: Vec<f32>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Face {
    /// Its middle, 0–1 across the picture from the left and up from the bottom.
    pub x: f32,
    pub y: f32,
    /// Its height, as a share of the picture's.
    pub size: f32,
    /// How open its mouth is, 0–1, when the faces are looked at closely enough to tell.
    pub mouth: Option<f32>,
}

/// One effect's turn: which, and with what settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Turn {
    pub effect: String,
    pub settings: Settings,
}

/// What Flex wants played.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wanted {
    /// The sound effect playing, if any.
    pub chosen: Option<Turn>,
    /// During a fade, the sound effect fading out (or none), and how far the chosen one has
    /// faded in over it (0–1).
    pub fading: Option<(Option<Turn>, f32)>,
    /// How the voice is found in the sound's channels.
    pub split: Split,
}

/// Sound kept back a fixed number of frames.
pub struct Delay {
    line: Vec<f32>,
    at: usize,
}

impl Delay {
    pub fn new(frames: usize) -> Self {
        Self { line: vec![0.0; frames], at: 0 }
    }

    /// `samples`, `frames` later, in place.
    pub fn run(&mut self, samples: &mut [f32]) {
        if self.line.is_empty() {
            return;
        }
        for s in samples {
            std::mem::swap(s, &mut self.line[self.at]);
            self.at = (self.at + 1) % self.line.len();
        }
    }
}

/// The networks the engine runs, once Flex has loaded them.
#[derive(Clone, Default)]
pub struct Networks {
    /// Finds the voice (DeepFilterNet3).
    pub voice: Option<Arc<deepfilter::Model>>,
    /// Hears when someone's talking (Silero VAD).
    pub speech: Option<Arc<vad::Model>>,
    /// Hears a voice's pitch, and plays it on instruments, by id (DDSP-VST).
    pub pitch: Option<Arc<ddsp::Pitch>>,
    pub instruments: Vec<(String, Arc<ddsp::Instrument>)>,
}

impl Networks {
    fn same(&self, other: &Self) -> bool {
        fn ptr<T>(a: &Option<Arc<T>>) -> *const T {
            a.as_ref().map_or(std::ptr::null(), Arc::as_ptr)
        }
        ptr(&self.voice) == ptr(&other.voice)
            && ptr(&self.speech) == ptr(&other.speech)
            && ptr(&self.pitch) == ptr(&other.pitch)
            && self.instruments.len() == other.instruments.len()
            && self.instruments.iter().zip(&other.instruments).all(|((a, x), (b, y))| a == b && Arc::ptr_eq(x, y))
    }
}

/// How late the split voice and background are, in seconds: DeepFilterNet's lookahead and
/// window. It's the same with or without the network, so turning it on doesn't move the sound.
const SPLIT_SECS: f32 = deepfilter::LATENCY as f32 / deepfilter::RATE as f32;

fn split_late(rate: u32) -> usize {
    (SPLIT_SECS * rate as f32).round() as usize
}

/// After the channels are split by where films put the voice: the voice found properly in that,
/// with the networks, and everything kept back `split_late`.
struct Splitter {
    rate: u32,
    networks: Networks,
    separator: Option<deepfilter::Separator>,
    detector: vad::Detector,
    /// The voice's channels as split, and each channel's background, kept back.
    source: Delay,
    background: Vec<Delay>,
    found: Vec<f32>,
    echo: Vec<f32>,
    rest: Vec<f32>,
}

impl Splitter {
    fn new(rate: u32, channels: usize) -> Self {
        let late = split_late(rate);
        Self {
            rate,
            networks: Networks::default(),
            separator: None,
            detector: vad::Detector::default(),
            source: Delay::new(late),
            background: (0..channels).map(|_| Delay::new(late)).collect(),
            found: Vec::new(),
            echo: Vec::new(),
            rest: Vec::new(),
        }
    }

    fn use_networks(&mut self, networks: &Networks) {
        fn same<T>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
            a.as_ref().zip(b.as_ref()).is_some_and(|(a, b)| Arc::ptr_eq(a, b))
        }
        if self.rate == deepfilter::RATE && !same(&self.networks.voice, &networks.voice) {
            // Starts with what came before forgotten; a hiccup only when the network's first loaded.
            self.separator = networks.voice.as_ref().and_then(|m| deepfilter::Separator::new(m).ok());
        }
        if !same(&self.networks.speech, &networks.speech) {
            self.detector = vad::Detector::default();
        }
        self.networks.clone_from(networks);
    }

    /// `stems` as split by channel, with `weights` saying how much of the voice each channel
    /// had: the voice found in that, and all of it kept back.
    fn run(&mut self, stems: &mut Stems, weights: &[f32]) {
        stems.speech = match (&self.networks.speech, self.rate == vad::RATE) {
            (Some(model), true) => {
                self.detector.push(model, &stems.voice);
                self.detector.speech()
            }
            _ => 1.0,
        };
        for (d, c) in self.background.iter_mut().zip(&mut stems.background) {
            d.run(c);
        }
        let frames = stems.voice.len();
        stems.echo.clear();
        stems.echo.resize(frames, 0.0);
        if let Some(separator) = &mut self.separator
            && frames == deepfilter::HOP
        {
            self.found.resize(frames, 0.0);
            self.echo.resize(frames, 0.0);
            self.rest.resize(frames, 0.0);
            if separator.run(&stems.voice, &mut self.found, &mut self.echo, &mut self.rest).is_ok() {
                // What it didn't take as the voice or its echo goes back where the voice came from.
                self.source.run(&mut stems.voice);
                for (c, w) in stems.background.iter_mut().zip(weights) {
                    for (((b, s), v), e) in c.iter_mut().zip(&stems.voice).zip(&self.found).zip(&self.echo) {
                        *b += w * (s - v - e);
                    }
                }
                stems.voice.copy_from_slice(&self.found);
                stems.echo.copy_from_slice(&self.echo);
                return;
            }
            self.separator = None;
        }
        self.source.run(&mut stems.voice);
    }
}

/// An effect playing, or none (the sound as it is), kept back to `MOST_LATE` all told.
struct Playing {
    turn: Option<Turn>,
    effect: Option<Box<dyn SoundEffect>>,
    /// The voice's, its echo's, then each channel's, delay.
    delays: Vec<Delay>,
    stems: Stems,
}

impl Playing {
    fn new(turn: Option<Turn>, rate: u32, channels: usize, networks: &Networks) -> Self {
        let effect = turn.as_ref().and_then(|t| make(&t.effect, rate, networks));
        let late = most_late(rate) - effect.as_ref().map_or(0, |e| e.latency()).min(most_late(rate));
        let delays = (0..channels + 2).map(|_| Delay::new(late)).collect();
        Self { turn, effect, delays, stems: Stems::default() }
    }

    /// The block through this effect, into `self.stems`.
    fn process(&mut self, stems: &Stems, scene: &Scene) {
        self.stems.clone_from(stems);
        if let Some(effect) = &mut self.effect {
            let settings = self.turn.as_ref().map(|t| &t.settings);
            effect.process(&mut self.stems, settings.unwrap_or(&Settings::default()), scene);
        }
        let [voice, echo, rest @ ..] = &mut self.delays[..] else { unreachable!("there's always the voice's and its echo's") };
        voice.run(&mut self.stems.voice);
        echo.run(&mut self.stems.echo);
        for (d, c) in rest.iter_mut().zip(&mut self.stems.background) {
            d.run(c);
        }
    }

    fn id(&self) -> Option<&str> {
        effect_id(&self.turn)
    }

    fn update(&mut self, turn: &Option<Turn>) {
        if let (Some(mine), Some(t)) = (&mut self.turn, turn) {
            mine.settings.clone_from(&t.settings);
        }
    }
}

fn effect_id(turn: &Option<Turn>) -> Option<&str> {
    turn.as_ref().map(|t| t.effect.as_str())
}

fn most_late(rate: u32) -> usize {
    (MOST_LATE * rate as f32) as usize
}

/// The audio thread's side.
struct Mixer {
    rate: u32,
    channels: usize,
    /// The effect fading in, or on, and the one fading out.
    chosen: Playing,
    fading: Option<Playing>,
    /// How far `chosen` has faded in, following `Wanted`.
    amount: f32,
    wanted: Wanted,
    scene: Scene,
    splitter: Splitter,
    stems: Stems,
    mixed: Vec<f32>,
}

impl Mixer {
    fn new(rate: u32, channels: usize) -> Self {
        Self { rate, channels, chosen: Playing::new(None, rate, channels, &Networks::default()), fading: None, amount: 1.0, wanted: Wanted::default(), scene: Scene::default(), splitter: Splitter::new(rate, channels), stems: Stems::default(), mixed: Vec::new() }
    }

    /// The networks loaded so far, for the split and the effects playing.
    fn use_networks(&mut self, networks: &Networks) {
        self.splitter.use_networks(networks);
        for playing in std::iter::once(&mut self.chosen).chain(self.fading.as_mut()) {
            if let Some(effect) = &mut playing.effect {
                effect.use_networks(networks);
            }
        }
    }

    /// Takes in what Flex wants now.
    fn follow(&mut self, wanted: &Wanted) {
        let (rate, channels) = (self.rate, self.channels);
        let chosen = effect_id(&wanted.chosen);
        match &wanted.fading {
            Some((from, t)) => {
                if self.fading.as_ref().is_none_or(|p| p.id() != effect_id(from)) {
                    // What was playing carries on as the one fading out, rather than starting again.
                    self.fading = Some(if self.chosen.id() == effect_id(from) && chosen != effect_id(from) {
                        std::mem::replace(&mut self.chosen, Playing::new(wanted.chosen.clone(), rate, channels, &self.splitter.networks))
                    } else {
                        Playing::new(from.clone(), rate, channels, &self.splitter.networks)
                    });
                    self.amount = t.clamp(0.0, 1.0);
                }
                if let Some(p) = &mut self.fading {
                    p.update(from);
                }
            }
            None => self.fading = None,
        }
        if self.chosen.id() != chosen {
            self.chosen = Playing::new(wanted.chosen.clone(), rate, channels, &self.splitter.networks);
        }
        self.chosen.update(&wanted.chosen);
        self.wanted.clone_from(wanted);
    }

    /// One block, in place: `channels` channels of `frames` frames, one after another.
    fn process(&mut self, samples: &mut [f32], frames: usize) {
        let weights = self.wanted.split.split(samples, self.channels, frames, &mut self.stems);
        self.stems.rate = self.rate;
        self.stems.weights.clone_from(&weights);
        self.splitter.run(&mut self.stems, &weights);
        let target = self.wanted.fading.as_ref().map_or(1.0, |(_, t)| t.clamp(0.0, 1.0));
        let step = frames as f32 / (FADE_STEP_SECS * self.rate as f32);
        let start = self.amount;
        self.amount += (target - self.amount).clamp(-step, step);
        self.mixed.clear();
        self.mixed.resize(samples.len(), 0.0);
        for (playing, from, to) in [(Some(&mut self.chosen), start, self.amount), (self.fading.as_mut(), 1.0 - start, 1.0 - self.amount)] {
            let Some(playing) = playing else { continue };
            if from <= 0.0 && to <= 0.0 {
                continue;
            }
            playing.process(&self.stems, &self.scene);
            for (c, out) in self.mixed.chunks_mut(frames).enumerate() {
                for (i, y) in out.iter_mut().enumerate() {
                    let gain = from + (to - from) * i as f32 / frames as f32;
                    *y += gain * (playing.stems.background[c][i] + weights[c] * (playing.stems.voice[i] + playing.stems.echo[i]));
                }
            }
        }
        samples.copy_from_slice(&self.mixed);
    }
}

/// One stream of sound: what's waiting for a whole block, and what's come back.
struct Stream {
    rate: u32,
    channels: usize,
    input: VecDeque<f32>,
    output: VecDeque<f32>,
    mixer: Mixer,
    block: Vec<f32>,
}

impl Stream {
    fn new(rate: u32, channels: usize) -> Self {
        // A block's silence ahead, so there's always enough to hand back: what's waiting and
        // what's come back always add up to a block.
        let output = std::iter::repeat_n(0.0, block(rate) * channels).collect();
        Self { rate, channels, input: VecDeque::new(), output, mixer: Mixer::new(rate, channels), block: Vec::new() }
    }

    fn change(&mut self, samples: &mut [f32]) {
        let (frames, channels) = (block(self.rate), self.channels);
        self.input.extend(samples.iter().map(|x| if x.is_finite() { *x } else { 0.0 }));
        while self.input.len() >= frames * channels {
            // Interleaved in, one channel after another for the mixer, and back.
            self.block.clear();
            self.block.resize(frames * channels, 0.0);
            for (i, x) in self.input.drain(..frames * channels).enumerate() {
                self.block[(i % channels) * frames + i / channels] = x;
            }
            self.mixer.process(&mut self.block, frames);
            self.output.extend((0..frames * channels).map(|i| self.block[(i % channels) * frames + i / channels]).map(|y| if y.is_finite() { y } else { 0.0 }));
        }
        let n = samples.len();
        for (y, out) in samples.iter_mut().zip(self.output.drain(..n)) {
            *y = out;
        }
    }
}

/// How many frames the mixer takes at a time: 10 ms.
fn block(rate: u32) -> usize {
    (rate as usize / 100).max(1)
}

/// How late the sound comes out, in seconds: a block, the split, and the most an effect keeps
/// back.
pub const LATENCY_SECS: f32 = 0.01 + SPLIT_SECS + MOST_LATE;

#[derive(Default)]
struct Shared {
    stream: Mutex<Option<Stream>>,
    wanted: Mutex<Wanted>,
    scene: Mutex<Scene>,
    networks: Mutex<Networks>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The sound effects: Flex says what to play (`set`, `set_scene`) and mpv's audio thread has
/// the sound changed (`change`).
#[derive(Clone, Default)]
pub struct Sound {
    shared: Arc<Shared>,
}

impl Sound {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, wanted: Wanted) {
        let mut current = lock(&self.shared.wanted);
        if *current != wanted {
            *current = wanted;
        }
    }

    /// The networks to run, as Flex loads them.
    pub fn set_networks(&self, networks: Networks) {
        *lock(&self.shared.networks) = networks;
    }

    pub fn set_scene(&self, scene: Scene) {
        *lock(&self.shared.scene) = scene;
    }

    /// Changes `samples`, `channels` channels interleaved at `rate`, in place. What comes out
    /// is `LATENCY_SECS` late. Called on mpv's audio thread: it never waits on Flex.
    pub fn change(&self, samples: &mut [f32], channels: usize, rate: u32) {
        let (channels, rate) = (channels.max(1), rate.max(100));
        let mut stream = lock(&self.shared.stream);
        if stream.as_ref().is_none_or(|s| s.rate != rate || s.channels != channels) {
            *stream = Some(Stream::new(rate, channels));
        }
        let Some(stream) = stream.as_mut() else { return };
        if let Ok(wanted) = self.shared.wanted.try_lock()
            && *wanted != stream.mixer.wanted
        {
            stream.mixer.follow(&wanted);
        }
        if let Ok(scene) = self.shared.scene.try_lock() {
            stream.mixer.scene.clone_from(&scene);
        }
        if let Ok(networks) = self.shared.networks.try_lock()
            && !networks.same(&stream.mixer.splitter.networks)
        {
            stream.mixer.use_networks(&networks);
        }
        stream.change(samples);
    }

    /// The sound's started again somewhere else (a seek): what's held back is dropped.
    pub fn reset(&self) {
        *lock(&self.shared.stream) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn turn(effect: &str, voice: &str) -> Option<Turn> {
        Some(Turn { effect: effect.into(), settings: Settings { numbers: vec![], choices: vec![("voice".into(), voice.into())] } })
    }

    /// Runs `seconds` of two-channel noise through, with `wanted`, and gives back what came out.
    fn run(mixer: &mut Mixer, wanted: &Wanted, input: &[f32]) -> Vec<f32> {
        mixer.follow(wanted);
        let frames = block(RATE);
        let mut out = Vec::new();
        for block in input.chunks(frames * 2) {
            let mut b = block.to_vec();
            mixer.process(&mut b, frames);
            out.extend(b);
        }
        out
    }

    fn noise(blocks: usize) -> Vec<f32> {
        let mut x = 1u32;
        (0..blocks * block(RATE) * 2)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32) - 0.5
            })
            .collect()
    }

    /// Back as channel after channel, from blocks of `block(RATE)` frames.
    fn channel(out: &[f32], c: usize) -> Vec<f32> {
        let frames = block(RATE);
        out.chunks(frames * 2).flat_map(|b| b[c * frames..(c + 1) * frames].to_vec()).collect()
    }

    #[test]
    fn interleaved_sound_comes_back_exactly_as_late_as_it_says() {
        let sound = Sound::new();
        let channels = 3;
        let input: Vec<f32> = (0..RATE as usize * channels).map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5).collect();
        let mut out = input.clone();
        // Pieces of uneven sizes, as mpv hands them over.
        let mut at = 0;
        for frames in [1024, 1, 7000, 333, 1536].into_iter().cycle() {
            if at >= out.len() {
                break;
            }
            let end = (at + frames * channels).min(out.len());
            sound.change(&mut out[at..end], channels, RATE);
            at = end;
        }
        let late = (LATENCY_SECS * RATE as f32).round() as usize * channels;
        assert!(out[..late].iter().all(|&y| y == 0.0));
        for (i, (y, x)) in out[late..].iter().zip(&input).enumerate() {
            assert!((y - x).abs() < 1e-6, "at {i}");
        }
    }

    #[test]
    fn without_an_effect_the_sound_is_only_late() {
        let mut mixer = Mixer::new(RATE, 2);
        let input = noise(20);
        let out = run(&mut mixer, &Wanted::default(), &input);
        let late = most_late(RATE) + split_late(RATE);
        for c in 0..2 {
            let (i, o) = (channel(&input, c), channel(&out, c));
            for n in late..i.len() {
                assert!((o[n] - i[n - late]).abs() < 1e-6, "channel {c} at {n}");
            }
        }
    }

    #[test]
    fn a_voice_with_nothing_to_change_is_the_same_as_none() {
        let mut mixer = Mixer::new(RATE, 2);
        let input = noise(20);
        let wanted = Wanted { chosen: turn("voice", "none"), ..Default::default() };
        let out = run(&mut mixer, &wanted, &input);
        let late = most_late(RATE) + split_late(RATE);
        let (i, o) = (channel(&input, 1), channel(&out, 1));
        for n in late..i.len() {
            assert!((o[n] - i[n - late]).abs() < 1e-5, "at {n}");
        }
    }

    #[test]
    fn fading_moves_smoothly_from_one_to_the_other() {
        let mut mixer = Mixer::new(RATE, 2);
        // Both channels the same slow sine, laid out as blocks are: one channel after the other.
        let frames = block(RATE);
        let input: Vec<f32> = (0..200 * frames * 2).map(|i| ((i / (2 * frames) * frames + i % frames) as f32 * 0.03).sin() * 0.5).collect();
        let robot = turn("voice", "robot");
        let mut out = run(&mut mixer, &Wanted { chosen: robot.clone(), ..Default::default() }, &input[..input.len() / 2]);
        for (k, block) in input[input.len() / 2..].chunks(block(RATE) * 2).enumerate() {
            let t = (k as f32 / 50.0).min(1.0);
            out.extend(run(&mut mixer, &Wanted { chosen: None, fading: Some((robot.clone(), t)), ..Default::default() }, block));
        }
        let left = channel(&out, 0);
        let worst = left.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(worst < 0.1, "biggest jump {worst}");
    }

    /// With the networks (`FLEX_DFN`: DeepFilterNet3_onnx.tar.gz, `FLEX_VAD`:
    /// silero_vad_16k_op15.onnx), with no effect the sound still comes back exactly, as late.
    #[test]
    #[ignore]
    fn with_the_networks_the_sound_still_comes_back_exactly() {
        let voice = deepfilter::Model::load(std::fs::File::open(std::env::var("FLEX_DFN").unwrap()).unwrap()).unwrap();
        let speech = vad::Model::load(&std::fs::read(std::env::var("FLEX_VAD").unwrap()).unwrap()).unwrap();
        let mut mixer = Mixer::new(RATE, 2);
        mixer.splitter.use_networks(&Networks { voice: Some(Arc::new(voice)), speech: Some(Arc::new(speech)), ..Default::default() });
        assert!(mixer.splitter.separator.is_some());
        let input = noise(200);
        let out = run(&mut mixer, &Wanted::default(), &input);
        let late = most_late(RATE) + split_late(RATE);
        for c in 0..2 {
            let (i, o) = (channel(&input, c), channel(&out, c));
            for n in late..i.len() {
                assert!((o[n] - i[n - late]).abs() < 1e-5, "channel {c} at {n}");
            }
        }
    }

    /// A clip through the engine with the networks (as above) and a voice: `FLEX_SOUND_IN` raw
    /// mono f32 at 48 kHz, `FLEX_SOUND_VOICE` the voice; written beside it as `.<voice>`.
    #[test]
    #[ignore]
    fn changes_a_clip() {
        let voice = deepfilter::Model::load(std::fs::File::open(std::env::var("FLEX_DFN").unwrap()).unwrap()).unwrap();
        let speech = vad::Model::load(&std::fs::read(std::env::var("FLEX_VAD").unwrap()).unwrap()).unwrap();
        let path = std::env::var("FLEX_SOUND_IN").unwrap();
        let which = std::env::var("FLEX_SOUND_VOICE").unwrap();
        let mono: Vec<f32> = std::fs::read(&path).unwrap().as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        let frames = block(RATE);
        let input: Vec<f32> = mono.chunks_exact(frames).flat_map(|b| b.iter().chain(b).copied().collect::<Vec<_>>()).collect();
        let mut networks = Networks { voice: Some(Arc::new(voice)), speech: Some(Arc::new(speech)), ..Default::default() };
        // `FLEX_SOUND_EFFECT=instrument`, with `FLEX_DDSP_PITCH` and `FLEX_DDSP_ZIP`, plays it on
        // the instrument `FLEX_SOUND_VOICE` instead.
        let effect = std::env::var("FLEX_SOUND_EFFECT").unwrap_or("voice".into());
        if let Ok(path) = std::env::var("FLEX_DDSP_PITCH") {
            networks.pitch = Some(Arc::new(ddsp::Pitch::load(&std::fs::read(path).unwrap()).unwrap()));
        }
        if effect == "instrument" {
            let zip = std::fs::read(std::env::var("FLEX_DDSP_ZIP").unwrap()).unwrap();
            networks.instruments.push((which.clone(), Arc::new(ddsp::Instrument::load(&ddsp::instrument_file(std::io::Cursor::new(&zip), &which).unwrap()).unwrap())));
        }
        let mut mixer = Mixer::new(RATE, 2);
        mixer.use_networks(&networks);
        let mut settings = Settings { numbers: vec![], choices: vec![(if effect == "instrument" { "instrument" } else { "voice" }.into(), which.clone())] };
        // `FLEX_SOUND_SETTINGS`: more settings, "key=value,…" (numbers or choices).
        for pair in std::env::var("FLEX_SOUND_SETTINGS").unwrap_or_default().split(',').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap();
            match v.parse::<f32>() {
                Ok(n) => settings.numbers.push((k.into(), n)),
                Err(_) => settings.choices.push((k.into(), v.into())),
            }
        }
        let chosen = Some(Turn { effect: effect.clone(), settings });
        let start = std::time::Instant::now();
        let out = run(&mut mixer, &Wanted { chosen, ..Default::default() }, &input);
        println!("{:.3} of real time", start.elapsed().as_secs_f32() / (mono.len() as f32 / RATE as f32));
        let left = channel(&out, 0);
        std::fs::write(format!("{path}.{which}"), left.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>()).unwrap();
    }
}
