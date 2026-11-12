//! Sound effects (flex-sound), in the sound mpv plays. While an effect that might change the
//! sound is on, Flex's patched libmpv's "flexsound" filter sits at the end of mpv's audio
//! filters and hands the sound to the engine to change in place. Each frame, Flex tells the
//! engine which sound effects to play (the one playing, and during a fade the one fading out)
//! and what's on screen.
//!
//! The first time, its networks (DeepFilterNet3 to find the voice, Silero VAD to hear when
//! someone's talking, and DDSP-VST's when an effect follows the voice's pitch or plays an
//! instrument) are downloaded and loaded on a
//! thread of their own, and handed to the engine when they're ready; until then the voice is
//! split by channel. They take sound at 48 kHz, so mpv's sound is turned to that ahead of the
//! filter.

use std::ffi::{c_double, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use flex_core::effects::{ActiveSound, Effects};
use flex_sound::{Networks, Scene, Settings, Sound, Split, Turn, Wanted, ddsp, deepfilter, vad};

use crate::mpv::Mpv;
use crate::worker::fetch_model;

/// The filters' labels in mpv: the engine, and the change to 48 kHz ahead of it.
const LABEL: &str = "flex-sound";
const RATE_LABEL: &str = "flex-sound-rate";

/// How the networks are getting on, for F3.
#[derive(Clone, Debug, Default)]
pub enum Status {
    #[default]
    Off,
    Downloading(&'static str, u8),
    Loading,
    Ready,
    Failed(String),
}

impl Status {
    pub fn describe(&self) -> String {
        match self {
            Status::Off => "off".into(),
            Status::Downloading(what, p) => format!("downloading {what} ({p}%)…"),
            Status::Loading => "loading…".into(),
            Status::Ready => "voice found by DeepFilterNet3 (MIT/Apache-2.0), speech heard by Silero VAD (MIT)".into(),
            Status::Failed(e) => format!("⚠ {e} (the voice is split by channel)"),
        }
    }
}

static STATUS: Mutex<Status> = Mutex::new(Status::Off);
/// The pitch network's and the instruments', as a line of their own.
static INSTRUMENTS: Mutex<String> = Mutex::new(String::new());

pub fn status() -> Status {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// How the pitch network and the instruments are getting on, if they've been asked for.
pub fn instruments_status() -> Option<String> {
    Some(INSTRUMENTS.lock().unwrap_or_else(|e| e.into_inner()).clone()).filter(|s| !s.is_empty())
}

fn set_status(status: Status) {
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = status;
}

fn set_instruments_status(line: String) {
    *INSTRUMENTS.lock().unwrap_or_else(|e| e.into_inner()) = line;
}

/// What the loading thread's asked to load.
enum Ask {
    /// The voice and speech networks.
    Basics,
    /// The pitch network.
    Pitch,
    /// An instrument, by id (and the pitch network, the first time).
    Instrument(String),
}

/// The loading thread: loads what it's asked for, one at a time, and hands the engine all it
/// has after each.
fn load(sound: Sound, asks: Receiver<Ask>) {
    let mut networks = Networks::default();
    for ask in asks {
        match ask {
            Ask::Basics => match load_basics() {
                Ok((voice, speech)) => {
                    (networks.voice, networks.speech) = (Some(voice), Some(speech));
                    sound.set_networks(networks.clone());
                    set_status(Status::Ready);
                    log::info!("sound effects: the voice and speech networks are ready");
                }
                Err(e) => {
                    log::warn!("sound effects: {e:#}");
                    set_status(Status::Failed(format!("{e:#}")));
                }
            },
            Ask::Pitch => match load_pitch(&mut networks) {
                Ok(()) => {
                    sound.set_networks(networks.clone());
                    set_instruments_status(pitch_ready(&networks));
                    log::info!("sound effects: the pitch network is ready");
                }
                Err(e) => {
                    log::warn!("sound effects: the pitch network: {e:#}");
                    set_instruments_status(format!("⚠ the pitch network: {e:#}"));
                }
            },
            Ask::Instrument(id) => match load_instrument(&mut networks, &id) {
                Ok(()) => {
                    sound.set_networks(networks.clone());
                    set_instruments_status(pitch_ready(&networks));
                    log::info!("sound effects: the {id} is ready");
                }
                Err(e) => {
                    log::warn!("sound effects: the {id}: {e:#}");
                    set_instruments_status(format!("⚠ the {id}: {e:#}"));
                }
            },
        }
    }
}

/// Downloads (the first time) and loads the voice and speech networks.
fn load_basics() -> anyhow::Result<(Arc<deepfilter::Model>, Arc<vad::Model>)> {
    let voice = fetch_model(deepfilter::FILE, deepfilter::URL, "the voice network", deepfilter::BYTES, deepfilter::SHA256, |p| set_status(Status::Downloading("the voice network", p)))?;
    let speech = fetch_model(vad::FILE, vad::URL, "the speech network", vad::BYTES, vad::SHA256, |p| set_status(Status::Downloading("the speech network", p)))?;
    set_status(Status::Loading);
    let voice = deepfilter::Model::load(std::fs::File::open(voice)?).map_err(|e| anyhow::anyhow!("loading the voice network: {e}"))?;
    let speech = vad::Model::load(&std::fs::read(speech)?).map_err(|e| anyhow::anyhow!("loading the speech network: {e}"))?;
    Ok((Arc::new(voice), Arc::new(speech)))
}

/// What F3 says once the pitch network (and any instruments) are ready.
fn pitch_ready(networks: &Networks) -> String {
    let ready: Vec<&str> = networks.instruments.iter().map(|(i, _)| i.as_str()).collect();
    if ready.is_empty() { "heard by DDSP-VST's pitch network (Apache-2.0)".into() } else { format!("heard by DDSP-VST's pitch network, {} played by DDSP-VST (Apache-2.0)", ready.join(", ")) }
}

/// Downloads (the first time) and loads the pitch network, if it isn't yet.
fn load_pitch(networks: &mut Networks) -> anyhow::Result<()> {
    if networks.pitch.is_none() {
        let path = fetch_model(ddsp::PITCH_FILE, ddsp::PITCH_URL, "the pitch network", ddsp::PITCH_BYTES, ddsp::PITCH_SHA256, |p| set_instruments_status(format!("downloading the pitch network ({p}%)…")))?;
        networks.pitch = Some(Arc::new(ddsp::Pitch::load(&std::fs::read(path)?).map_err(|e| anyhow::anyhow!("loading the pitch network: {e}"))?));
    }
    Ok(())
}

/// Downloads (the first time) and loads an instrument, and the pitch network if it isn't yet.
fn load_instrument(networks: &mut Networks, id: &str) -> anyhow::Result<()> {
    load_pitch(networks)?;
    let zip = fetch_model(ddsp::INSTRUMENTS_FILE, ddsp::INSTRUMENTS_URL, "the instruments", ddsp::INSTRUMENTS_BYTES, ddsp::INSTRUMENTS_SHA256, |p| set_instruments_status(format!("downloading the instruments ({p}%)…")))?;
    set_instruments_status(format!("loading the {id}…"));
    let file = ddsp::instrument_file(std::io::BufReader::new(std::fs::File::open(zip)?), id).map_err(|e| anyhow::anyhow!(e))?;
    let instrument = ddsp::Instrument::load(&file).map_err(|e| anyhow::anyhow!("loading it: {e}"))?;
    networks.instruments.push((id.to_string(), Arc::new(instrument)));
    Ok(())
}

/// Set once Flex has found the libmpv can't change the sound, for the panel to say so.
static CANT: AtomicBool = AtomicBool::new(false);

/// Whether sound effects can't play, because this libmpv isn't Flex's patched one.
pub fn unavailable() -> bool {
    CANT.load(Ordering::Relaxed)
}

#[derive(Default)]
pub struct SoundEffects {
    /// The engine, once mpv's been given it; none if this libmpv can't change the sound.
    sound: Option<Option<&'static Sound>>,
    /// Whether the filter's in mpv.
    added: bool,
    /// The loading thread, once started, and what it's been asked for.
    loader: Option<Sender<Ask>>,
    asked: HashSet<String>,
}

impl SoundEffects {
    /// Call once per frame.
    pub fn tick(&mut self, mpv: &Mpv, effects: &Effects, kinds: &[f32]) {
        if !effects.needs().sound {
            self.stop(mpv);
            return;
        }
        let Some(sound) = *self.sound.get_or_insert_with(|| {
            // Made once and kept: mpv's audio thread may be using it until the program ends.
            let sound: &'static Sound = Box::leak(Box::new(Sound::new()));
            let ctx = std::ptr::from_ref(sound).cast_mut().cast();
            mpv.set_sound(change, reset, ctx).then_some(sound).or_else(|| {
                log::warn!("sound effects: this libmpv can't change the sound (it takes Flex's patched one)");
                CANT.store(true, Ordering::Relaxed);
                None
            })
        }) else {
            return;
        };
        let channels = mpv.get_i64("audio-params/channel-count").unwrap_or(2).max(1) as usize;
        let layout = mpv.get_string("audio-params/channels").unwrap_or_default();
        sound.set(Wanted {
            chosen: effects.active_sound().map(turn),
            fading: effects.sound_fading().map(|(from, t)| (from.map(turn), t)),
            split: Split::for_layout(&layout, channels),
        });
        sound.set_scene(Scene { faces: Vec::new(), kinds: kinds.to_vec() });
        // The networks: the voice and speech ones always, the pitch one for the effects that
        // follow the voice's pitch, and the instruments being played.
        let mut wants = vec!["basics".to_string()];
        for s in effects.active_sound().into_iter().chain(effects.sound_fading().and_then(|(from, _)| from)) {
            let turn = turn(s);
            if flex_sound::wants_pitch(&turn.effect, &turn.settings) {
                wants.push("pitch".into());
            }
            if turn.effect == "instrument" {
                wants.push(format!("instrument:{}", turn.settings.choice("instrument").unwrap_or("flute")));
            }
        }
        for want in wants {
            if self.asked.contains(&want) {
                continue;
            }
            let loader = self.loader.get_or_insert_with(|| {
                let (tx, rx) = mpsc::channel();
                let sound = sound.clone();
                if let Err(e) = std::thread::Builder::new().name("sound networks".into()).spawn(move || load(sound, rx)) {
                    set_status(Status::Failed(e.to_string()));
                }
                tx
            });
            let ask = match want.strip_prefix("instrument:") {
                Some(id) => {
                    set_instruments_status(format!("loading the {id}…"));
                    Ask::Instrument(id.to_string())
                }
                None if want == "pitch" => {
                    set_instruments_status("loading the pitch network…".into());
                    Ask::Pitch
                }
                None => {
                    set_status(Status::Loading);
                    Ask::Basics
                }
            };
            let _ = loader.send(ask);
            self.asked.insert(want);
        }
        if !self.added {
            self.added = true;
            let rate = format!("@{RATE_LABEL}:format=srate={}", deepfilter::RATE);
            if let Err(e) = mpv.command(&["af", "add", &rate]) {
                log::warn!("sound effects: mpv wouldn't change the sound to 48 kHz: {e:#}");
            }
            match mpv.command(&["af", "add", &format!("@{LABEL}:flexsound")]) {
                Ok(()) => log::info!("sound effects: on"),
                Err(e) => log::warn!("sound effects: mpv wouldn't take the filter: {e:#}"),
            }
        }
    }

    /// How late the sound comes out of the filter, in seconds, to be made up with mpv's
    /// audio-delay.
    pub fn latency(&self) -> f32 {
        if self.added { flex_sound::LATENCY_SECS } else { 0.0 }
    }

    /// Takes the filter out of mpv, when no effect wants it.
    pub fn stop(&mut self, mpv: &Mpv) {
        if std::mem::take(&mut self.added) {
            let _ = mpv.command(&["af", "remove", &format!("@{LABEL}")]);
            let _ = mpv.command(&["af", "remove", &format!("@{RATE_LABEL}")]);
        }
        if let Some(Some(sound)) = self.sound {
            sound.set(Wanted::default());
        }
    }
}

fn turn(sound: ActiveSound) -> Turn {
    Turn { effect: sound.id, settings: Settings { numbers: sound.numbers, choices: sound.choices } }
}

/// What mpv calls with the sound to change.
unsafe extern "C" fn change(ctx: *mut c_void, samples: *mut f32, channels: c_int, frames: c_int, rate: c_int, _pts: c_double) {
    let (channels, frames) = (channels.max(1) as usize, frames.max(0) as usize);
    if ctx.is_null() || samples.is_null() || frames == 0 {
        return;
    }
    // SAFETY: ctx is the Sound given with it, kept for good; mpv passes `frames` frames of
    // `channels` floats, ours to change for this call.
    let (sound, samples) = unsafe { (&*ctx.cast::<Sound>(), std::slice::from_raw_parts_mut(samples, frames * channels)) };
    sound.change(samples, channels, rate.max(1) as u32);
}

/// What mpv calls when it clears the filter (a seek).
unsafe extern "C" fn reset(ctx: *mut c_void) {
    // SAFETY: as in `change`.
    if let Some(sound) = unsafe { ctx.cast::<Sound>().as_ref() } {
        sound.reset();
    }
}
