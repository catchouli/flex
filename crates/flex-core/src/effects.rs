//! Video effects: shaders Flex runs over the video (the drawing is in the app). Each is
//! a `<id>.frag` in an `effects` folder, with an optional `<id>.toml` for its name,
//! options and settings. One effect is chosen at a time, and they can all be switched off
//! without forgetting it; its settings are kept per effect. Shuffle swaps the chosen effect
//! at random, fading from one to the next.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use flex_plugin::{Choice, Settings};
use serde::{Deserialize, Serialize};

use crate::Db;
use crate::plugins::{SettingDrift, SettingKind, SettingSpec};

/// `<id>.toml`.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Manifest {
    /// Shown in Settings; defaults to the file's name.
    pub name: String,
    pub description: String,
    /// Give the shader its own previous output (`u_prev`), for trails and feedback.
    pub feedback: bool,
    /// Give the shader the sound, as measured by a filter inside mpv.
    pub audio: bool,
    /// Give the shader depth (`u_depth`), from a neural network on the GPU. Costs GPU time
    /// and a 99 MB download the first time.
    pub depth: bool,
    /// Give the shader where people, walls and sky are (`person_at`, `wall_at`, `sky_at`), from
    /// a small network that runs on top of depth's.
    pub segment: bool,
    /// Give the shader where faces are (`u_faces`, `face_at`), from a small neural network on
    /// the GPU.
    pub faces: bool,
    /// As well as `faces`, look closer at them: a 478-point mesh (`mesh_point`), how open eyes
    /// and mouths are (`u_face_open`) and which way heads point (`u_face_turn`).
    pub mesh: bool,
    /// Keep the last second or so of frames for `history_at`.
    pub history: bool,
    /// Work out how things are moving, for `flow_at`.
    pub flow: bool,
    /// Build up the room behind people, for `plate_at`. Runs the masks.
    pub plate: bool,
    /// Find scene cuts, for `u_cut` and `u_since_cut`.
    pub cuts: bool,
    /// Measure the picture's palette and brightness.
    pub stats: bool,
    /// Work out where the camera is, so things can be pinned to the world (`world_at`). Runs depth
    /// and the masks.
    pub camera: bool,
    /// The kinds of shot or scene this suits, out of `SUITS`: with shuffle at scene cuts, it comes
    /// up more often in those and less often otherwise.
    pub suits: Vec<String>,
    /// Work out what sort of shot this is and what's going on across the last few (`u_shot_size`,
    /// `u_camera`, `u_handheld`, `u_pace`, `u_scene`). Runs depth, masks, faces and the sound.
    pub scene: bool,
    /// Redraw the whole picture with a network (an anime style, say) before the shader sees it:
    /// `u_video` is the redrawn picture, and `u_original` the real one. Names the choice setting
    /// that picks the network: its options' ids are the networks' (see `RESTYLES`).
    pub restyle: Option<String>,
    /// Size to work at, relative to the screen (0.25–1): lower is lighter on the GPU.
    pub scale: f32,
    /// Whether shuffle picks this effect unless told otherwise: false for test views.
    pub shuffle: bool,
    pub settings: Vec<SettingSpec>,
}

/// What a missing `.toml`, or a field left out of one, comes to.
impl Default for Manifest {
    fn default() -> Self {
        Self { name: String::new(), description: String::new(), suits: Vec::new(), feedback: false, audio: false, depth: false, segment: false, faces: false, mesh: false, history: false, flow: false, plate: false, cuts: false, stats: false, scene: false, camera: false, restyle: None, scale: 1.0, shuffle: true, settings: Vec::new() }
    }
}

/// An effect found on disk.
#[derive(Clone, Debug)]
pub struct Effect {
    /// The file's name without `.frag`.
    pub id: String,
    pub shader: PathBuf,
    pub manifest: Manifest,
    /// Its `.toml` couldn't be read (it runs with the defaults).
    pub problem: Option<String>,
}

impl Effect {
    /// Settings Flex can show and pass to the shader (not text or buttons).
    pub fn settings(&self) -> impl Iterator<Item = &SettingSpec> {
        self.manifest.settings.iter().filter(|s| matches!(s.kind, SettingKind::Number | SettingKind::Bool | SettingKind::Choice))
    }

    /// Whether it can paint in styles from pictures (`ART`), so pictures put in the styles
    /// folder are offered too.
    pub fn takes_art(&self) -> bool {
        self.manifest.settings.iter().any(|s| s.kind == SettingKind::Choice && s.options.iter().any(|o| o.id.starts_with(ART)))
    }
}

/// Where effects are looked for: `effects` next to Flex's program (the ones a release
/// ships), the source folder in debug builds (so `cargo run` sees edits as they're
/// made), then `effects` in the data folder, which wins when two have the same name.
pub fn effect_dirs() -> Vec<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|d| d.join("effects")));
    let source = cfg!(debug_assertions).then(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../effects"));
    beside.into_iter().chain(source).chain([user_effect_dir()]).collect()
}

pub fn user_effect_dir() -> PathBuf {
    crate::data_dir().join("effects")
}

/// "my-cool_effect" → "My cool effect".
fn name_from_id(id: &str) -> String {
    let words = id.replace(['-', '_'], " ");
    let mut chars = words.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Every effect in `dirs`, sorted by name; later folders replace earlier ones.
pub fn discover(dirs: &[PathBuf]) -> Vec<Effect> {
    let mut found: Vec<Effect> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut shaders: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "frag") && p.is_file()).collect();
        shaders.sort();
        for shader in shaders {
            let Some(id) = shader.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
            let toml_path = shader.with_extension("toml");
            let (mut manifest, problem) = match std::fs::read_to_string(&toml_path) {
                Err(_) => (Manifest::default(), None),
                Ok(text) => match toml::from_str::<Manifest>(&text) {
                    Ok(m) => (m, None),
                    Err(e) => {
                        log::warn!("video effect {}: {e}", toml_path.display());
                        (Manifest::default(), Some(format!("{id}.toml: {}", e.message())))
                    }
                },
            };
            if manifest.name.trim().is_empty() {
                manifest.name = name_from_id(&id);
            }
            found.retain(|e| e.id != id);
            found.push(Effect { id, shader, manifest, problem });
        }
    }
    found.sort_by_key(|e| e.manifest.name.to_lowercase());
    found
}

/// The effect to draw, with its settings as shader uniforms.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveEffect {
    pub id: String,
    pub name: String,
    pub shader: PathBuf,
    pub feedback: bool,
    pub audio: bool,
    pub depth: bool,
    pub segment: bool,
    pub faces: bool,
    /// Whether it draws on a restyled picture (see `Manifest::restyle`).
    pub restyle: bool,
    pub scale: f32,
    /// `s_<key>` = number, bool (0/1) or choice (its index).
    pub uniforms: Vec<(String, f32)>,
}

/// What drawing the video needs, across every effect that's on or about to be.
#[derive(Clone, Debug, PartialEq)]
pub struct Needs {
    pub audio: bool,
    /// Depth, which masks are made on top of.
    pub depth: bool,
    pub segment: bool,
    pub faces: bool,
    pub mesh: bool,
    pub history: bool,
    pub flow: bool,
    pub plate: bool,
    pub cuts: bool,
    pub stats: bool,
    pub scene: bool,
    pub camera: bool,
    /// The network to restyle the picture with (see `Manifest::restyle`): the chosen effect's,
    /// or the one fading out's. One at a time: shuffle doesn't start them ahead.
    pub restyle: Option<String>,
    /// Whether a sound effect might play: the sound effects are kept ready while one might.
    pub sound: bool,
    /// The largest size any of them works at (0.25–1).
    pub scale: f32,
}

/// The networks `Manifest::restyle` can pick from, by id.
pub const RESTYLES: [&str; 14] = ["hayao", "hayao-warm", "hayao-vivid", "shinkai", "shinkai-bold", "ghibli", "sketch", "mosaic", "candy", "rain-princess", "udnie", "pointilism", "lines", "lines-anime"];

/// Styles `Manifest::restyle` can pick too: painting in the style of a picture, by Magenta's
/// stylization ("art:starry-night"; "art:morph", from one to the next; see flex-app's `art`).
pub const ART: &str = "art:";
/// The pictures put in the styles folder, "art:mine:<file name>": added to every setting that
/// offers `ART` styles by `Effects::set_art_styles`.
pub const ART_MINE: &str = "art:mine:";

/// The living textures `Manifest::restyle` can pick too, by id: grown on the GL side, not by a
/// network worker (see flex-app's nca.rs).
pub const TEXTURES: [&str; 20] = ["tex-honeycomb", "tex-cobwebs", "tex-bubbles", "tex-swirls", "tex-marble", "tex-cracked", "tex-chrome", "tex-fingerprints", "tex-candy", "tex-zebra", "tex-foam", "tex-mondrian", "tex-leopard", "tex-feathers", "tex-scales", "tex-eyes", "tex-staring", "tex-maze", "tex-coils", "tex-cells"];

/// What a manifest's `suits` can have in it.
pub const SUITS: [&str; 5] = ["close", "wide", "action", "dialogue", "quiet"];
/// How much of the shuffle (one in this many) stays out for having had a turn lately.
const RECENT_SHARE: usize = 3;
/// How likely an effect is, against 1 for one that doesn't say what it suits: this much if
/// it doesn't suit the shot at all, and this much more if it fits perfectly.
const LEAST_WEIGHT: f32 = 0.25;
const MOST_EXTRA_WEIGHT: f32 = 2.75;

/// What a shot's like, as far as is known.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShotHint {
    /// 0 very wide to 1 extreme close-up.
    pub size: Option<f32>,
    /// Action, dialogue, quiet and montage, each 0 to 1.
    pub scene: Option<[f32; 4]>,
}

fn smooth(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How much a kind of shot fits `hint`, from 0 to 1, if that's known.
fn suit_match(suit: &str, hint: ShotHint) -> Option<f32> {
    let score = |i: usize| hint.scene.map(|s| smooth(0.25, 0.7, s[i]));
    match suit {
        "close" => hint.size.map(|s| smooth(0.5, 0.75, s)),
        "wide" => hint.size.map(|s| 1.0 - smooth(0.25, 0.45, s)),
        "action" => score(0),
        "dialogue" => score(1),
        "quiet" => score(2),
        _ => None,
    }
}

/// How likely an effect that suits `suits` is for a shot like `hint`.
fn suit_weight(suits: &[String], hint: ShotHint) -> f32 {
    suits.iter().filter_map(|s| suit_match(s, hint)).reduce(f32::max).map_or(1.0, |fits| LEAST_WEIGHT + MOST_EXTRA_WEIGHT * fits)
}

/// Which of `weights` a roll from 0 to 1 lands on, each as likely as its weight.
fn weighted_pick(weights: &[f32], roll: f32) -> Option<usize> {
    let mut left = roll * weights.iter().sum::<f32>();
    for (i, weight) in weights.iter().enumerate() {
        left -= weight;
        if left < 0.0 {
            return Some(i);
        }
    }
    weights.len().checked_sub(1)
}

/// Effects taking turns at random.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shuffle {
    pub on: bool,
    /// How long each effect stays, in seconds, not counting the fade: somewhere between these.
    pub shortest: f32,
    pub longest: f32,
    /// How long one effect takes to fade into the next, in seconds: 0 is straight away.
    pub fade: f32,
    /// Fade through the plain picture: the effect going eases itself away (each in its own way,
    /// as `u_amount` falls), then the next one eases in, instead of one dissolving into the other.
    pub through_normal: bool,
    /// Change where the shot changes, once the shortest turn is up, instead of at a random
    /// moment. After the longest turn it changes anyway.
    pub at_cuts: bool,
    /// At scene cuts, favour effects that suit the new shot.
    pub by_shot: bool,
    /// How far each effect's settings are moved at random whenever it comes up, in percent:
    /// 0 leaves them alone, 100 can land anywhere in their range.
    pub randomise: f32,
    /// How likely each turn is to come with a sound effect, picked at random, in percent. At 0
    /// the sound effect picked on its own plays throughout.
    pub sounds: f32,
}

impl Default for Shuffle {
    fn default() -> Self {
        Self { on: false, shortest: 20.0, longest: 60.0, fade: 4.0, through_normal: false, at_cuts: false, by_shot: true, randomise: 0.0, sounds: 0.0 }
    }
}

/// Settings wandering slowly on their own, for whichever effect is on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Drift {
    /// How far they go, in percent: 100 can reach anywhere in their range.
    pub amount: f32,
    /// How fast, in percent of the usual pace.
    pub speed: f32,
}

impl Default for Drift {
    fn default() -> Self {
        Self { amount: 0.0, speed: 100.0 }
    }
}

/// How long a setting takes to wander there and back at the usual pace, in seconds: from this
/// long, up to a bit under twice as quick.
const DRIFT_PERIOD: f64 = 90.0;

/// An effect on its way out.
/// How to draw the picture during a fade: the effect going (if it's still showing) with how much
/// of it there is (its `u_amount`, 0 the plain picture to 1 all of it), how far the chosen effect
/// has faded in over it (0–1), and how much of the chosen effect there is.
#[derive(Clone, Debug)]
pub struct Fade {
    pub from: Option<(ActiveEffect, f32)>,
    pub faded_in: f32,
    pub amount: f32,
}

struct Fading {
    from: String,
    /// The sound effect that was playing.
    from_sound: Option<String>,
    elapsed: f32,
}

/// A sound effect, as the panel shows it (flex-sound's `EFFECTS`).
#[derive(Clone, Debug)]
pub struct SoundEffect {
    pub id: String,
    pub name: String,
    pub description: String,
    pub settings: Vec<SettingSpec>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct SoundManifest {
    name: String,
    description: String,
    settings: Vec<SettingSpec>,
}

fn sound_effects() -> Vec<SoundEffect> {
    flex_sound::EFFECTS
        .iter()
        .filter_map(|(id, text)| match toml::from_str::<SoundManifest>(text) {
            Ok(m) => Some(SoundEffect { id: (*id).to_owned(), name: m.name, description: m.description, settings: m.settings }),
            Err(e) => {
                log::warn!("sound effect {id}: {e}");
                None
            }
        })
        .collect()
}

/// Where a sound effect's settings are kept, beside the video effects'.
pub fn sound_settings_id(id: &str) -> String {
    format!("sound:{id}")
}

/// A sound effect playing, with its settings.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveSound {
    pub id: String,
    pub numbers: Vec<(String, f32)>,
    /// Choice settings, by the chosen option's id.
    pub choices: Vec<(String, String)>,
}

const CHOSEN_KEY: &str = "video_effect";
const SOUND_KEY: &str = "video_effect_sound";
/// "1" when sound effects are switched on.
const SOUND_ON_KEY: &str = "video_effect_sound_on";
/// "0" when effects are switched off. Before this existed, off was having none chosen.
const ON_KEY: &str = "video_effects_on";
const SHUFFLE_KEY: &str = "video_effect_shuffle";
const DRIFT_KEY: &str = "video_effect_drift";
/// Kept with an effect's settings: whether shuffle may pick it.
const IN_SHUFFLE_SETTING: &str = "_in_shuffle";
/// Kept with an effect's settings: whether it's in the favourites.
const FAVOURITE_SETTING: &str = "_favourite";

fn settings_key(id: &str) -> String {
    format!("video_effect.{id}")
}

/// The effects found, which one is on, and their settings.
#[derive(Default)]
pub struct Effects {
    effects: Vec<Effect>,
    /// The chosen effect's id (kept even if its file has gone, or effects are off).
    chosen: Option<String>,
    /// Whether effects are switched on at all.
    on: bool,
    settings: HashMap<String, Settings>,
    /// Random settings for this turn of shuffle, in place of `settings`, and never saved.
    rolled: HashMap<String, Settings>,
    /// The chosen effect's shader state, from the app: (ok, text).
    pub status: Option<(bool, String)>,
    shuffle: Shuffle,
    /// Seconds of playing until shuffle moves on, and since it last did.
    until_next: f32,
    turn: f32,
    fading: Option<Fading>,
    /// What shuffle hasn't played yet this time round.
    turns: Vec<String>,
    /// The latest turns, oldest first, which `next_suiting` leaves out.
    recent: VecDeque<String>,
    random: u64,
    drift: Drift,
    /// How far along drifting is, which only moves while playing.
    drift_clock: f64,
    /// `drift.amount` (as 0–1), eased, so turning it on doesn't jolt the picture.
    drift_now: f64,
    /// The pictures in the styles folder, from `set_art_styles`, put back after `refresh`.
    art_mine: Vec<Choice>,
    sounds: Vec<SoundEffect>,
    /// The sound effect picked on its own (kept while they're off).
    sound: Option<String>,
    /// Whether sound effects are switched on, which is separate from the video effects.
    sound_on: bool,
    /// The sound effect shuffle paired with this turn's effect, when it pairs them.
    turn_sound: Option<String>,
}

impl Effects {
    pub fn new(dirs: &[PathBuf], db: &Db) -> Self {
        let chosen = db.setting(CHOSEN_KEY).ok().flatten().filter(|s| !s.is_empty());
        let shuffle = db.setting(SHUFFLE_KEY).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let drift = db.setting(DRIFT_KEY).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let on = db.setting(ON_KEY).ok().flatten().map_or(chosen.is_some(), |s| s != "0");
        let sound = db.setting(SOUND_KEY).ok().flatten().filter(|s| !s.is_empty());
        let sound_on = db.setting(SOUND_ON_KEY).ok().flatten().map_or(sound.is_some(), |s| s == "1");
        let mut effects = Self { chosen, on, shuffle, drift, sound, sound_on, sounds: sound_effects(), ..Self::default() };
        effects.refresh(dirs, db);
        effects.wait();
        effects
    }

    /// Looks for effects again (new, changed or removed files).
    pub fn refresh(&mut self, dirs: &[PathBuf], db: &Db) {
        self.effects = discover(dirs);
        for e in &self.effects {
            let mut settings: Settings = e.manifest.settings.iter().filter_map(|s| Some((s.key.clone(), s.default.clone()?))).collect();
            let saved = db.setting(&settings_key(&e.id)).ok().flatten();
            if let Some(Ok(serde_json::Value::Object(saved))) = saved.map(|s| serde_json::from_str(&s)) {
                settings.extend(saved);
            }
            self.settings.insert(e.id.clone(), settings);
        }
        for sound in &self.sounds {
            let id = sound_settings_id(&sound.id);
            let mut settings: Settings = sound.settings.iter().filter_map(|s| Some((s.key.clone(), s.default.clone()?))).collect();
            if let Some(Ok(serde_json::Value::Object(saved))) = db.setting(&settings_key(&id)).ok().flatten().map(|s| serde_json::from_str(&s)) {
                settings.extend(saved);
            }
            self.settings.insert(id, settings);
        }
        self.offer_art_mine();
        log::info!("video effects: {}", self.effects.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(", "));
    }

    pub fn list(&self) -> &[Effect] {
        &self.effects
    }

    /// The effect that's on: none while they're switched off.
    pub fn chosen(&self) -> Option<&Effect> {
        self.remembered().filter(|_| self.on)
    }

    /// The effect that's on, or would be if they were switched back on.
    pub fn remembered(&self) -> Option<&Effect> {
        let id = self.chosen.as_deref()?;
        self.effects.iter().find(|e| e.id == id)
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Switches effects off, keeping which one, shuffle and drift for when they come back.
    pub fn set_on(&mut self, on: bool, db: &Db) {
        self.on = on;
        self.status = None;
        self.fading = None;
        if on && self.remembered().is_none() {
            self.chosen = self.next_turn();
            self.save_chosen(db);
        }
        self.wait();
        self.save_on(db);
    }

    fn save_on(&self, db: &Db) {
        if let Err(e) = db.set_setting(ON_KEY, if self.on { "1" } else { "0" }) {
            log::warn!("saving whether video effects are on: {e:#}");
        }
    }

    /// Someone chose an effect: it shows at once, and with shuffle on gets a full turn.
    /// Choosing none switches effects off.
    pub fn choose(&mut self, id: Option<&str>, db: &Db) {
        let Some(id) = id else {
            self.set_on(false, db);
            return;
        };
        self.chosen = Some(id.to_owned());
        self.on = true;
        self.status = None;
        self.fading = None;
        self.rolled.clear();
        self.wait();
        self.save_chosen(db);
        self.save_on(db);
    }

    fn save_chosen(&self, db: &Db) {
        if let Err(e) = db.set_setting(CHOSEN_KEY, self.chosen.as_deref().unwrap_or("")) {
            log::warn!("saving the video effect: {e:#}");
        }
    }

    pub fn shuffle(&self) -> Shuffle {
        self.shuffle
    }

    pub fn set_shuffle(&mut self, shuffle: Shuffle, db: &Db) {
        let (started, stopped) = (shuffle.on && !self.shuffle.on, !shuffle.on && self.shuffle.on);
        let restart = started || shuffle.at_cuts != self.shuffle.at_cuts;
        self.shuffle = shuffle;
        self.save_shuffle(db);
        if started && self.chosen().is_none() {
            self.on = true;
            self.save_on(db);
            if self.remembered().is_none() {
                self.chosen = self.next_turn();
            }
            self.status = None;
        }
        if restart {
            self.wait();
        }
        if !shuffle.on || shuffle.randomise <= 0.0 {
            self.rolled.clear();
        }
        if stopped {
            // Stay on whichever came up.
            self.save_chosen(db);
        }
    }

    fn save_shuffle(&self, db: &Db) {
        let saved = serde_json::to_string(&self.shuffle).map_err(anyhow::Error::from).and_then(|json| db.set_setting(SHUFFLE_KEY, &json));
        if let Err(e) = saved {
            log::warn!("saving the video effect shuffle: {e:#}");
        }
    }

    pub fn drift(&self) -> Drift {
        self.drift
    }

    pub fn set_drift(&mut self, drift: Drift, db: &Db) {
        self.drift = drift;
        let saved = serde_json::to_string(&drift).map_err(anyhow::Error::from).and_then(|json| db.set_setting(DRIFT_KEY, &json));
        if let Err(e) = saved {
            log::warn!("saving the video effect drift: {e:#}");
        }
    }

    /// A number setting as it is right now, having drifted from `base`.
    fn drifted(&self, id: &str, spec: &SettingSpec, base: f64) -> f64 {
        if self.drift_now <= 0.0 || spec.drift == SettingDrift::Off {
            return base;
        }
        // Each setting of each effect goes its own way.
        let mut seed = 0xcbf29ce484222325u64;
        for byte in id.bytes().chain(spec.key.bytes()) {
            seed = (seed ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
        let part = |shift: u32| ((seed >> shift) & 0xffff) as f64 / 65536.0;
        let tau = std::f64::consts::TAU;
        let turns = self.drift_clock / DRIFT_PERIOD * (1.0 + 0.8 * part(0));
        let wave = 0.65 * (tau * (turns + part(16))).sin() + 0.35 * (tau * (turns * 2.37 + part(32))).sin();
        let (min, max) = (spec.min.unwrap_or(0.0), spec.max.unwrap_or(100.0));
        let moved = base + (min + (0.5 + 0.5 * wave) * (max - min) - base) * self.drift_now;
        let steps = match spec.drift {
            SettingDrift::Steps => true,
            SettingDrift::Auto => spec.unit.is_empty() && spec.step.is_some_and(|s| s >= 1.0),
            _ => false,
        };
        match spec.step.filter(|_| steps) {
            Some(step) => (min + ((moved - min) / step).round() * step).clamp(min, max),
            None => moved,
        }
    }

    /// A number setting of the chosen effect as it is right now.
    pub fn live_number(&self, spec: &SettingSpec) -> Option<f64> {
        let id = &self.chosen()?.id;
        Some(self.drifted(id, spec, self.value(id, &spec.key)?.as_f64()?))
    }

    pub fn in_shuffle(&self, id: &str) -> bool {
        let default = self.effects.iter().find(|e| e.id == id).is_some_and(|e| e.manifest.shuffle);
        self.value(id, IN_SHUFFLE_SETTING).and_then(serde_json::Value::as_bool).unwrap_or(default)
    }

    pub fn set_in_shuffle(&mut self, id: &str, included: bool, db: &Db) {
        self.set_setting(id, IN_SHUFFLE_SETTING, serde_json::json!(included), db);
    }

    pub fn is_favourite(&self, id: &str) -> bool {
        self.value(id, FAVOURITE_SETTING).and_then(serde_json::Value::as_bool).unwrap_or(false)
    }

    pub fn set_favourite(&mut self, id: &str, favourite: bool, db: &Db) {
        self.set_setting(id, FAVOURITE_SETTING, serde_json::json!(favourite), db);
    }

    /// Call once per frame, with the seconds since the last. The fade keeps going while
    /// paused, but only playing counts towards the next change.
    pub fn step(&mut self, dt: f32, playing: bool) {
        if !self.on {
            return;
        }
        if playing {
            self.turn += dt;
            self.drift_clock += f64::from(dt * self.drift.speed / 100.0);
        }
        let aim = f64::from(self.drift.amount / 100.0).clamp(0.0, 1.0);
        self.drift_now += (aim - self.drift_now) * f64::from(dt * 0.4).min(1.0);
        if let Some(fading) = &mut self.fading {
            fading.elapsed += dt;
            if fading.elapsed < self.shuffle.fade {
                return;
            }
            self.fading = None;
        }
        if !self.shuffle.on || !playing {
            return;
        }
        self.until_next -= dt;
        // Don't sit on one that doesn't compile, or whose file has gone.
        let broken = matches!(self.status, Some((false, _))) || self.chosen().is_none();
        if self.until_next > 0.0 && !broken {
            return;
        }
        self.change_turn(!broken, None);
    }

    /// Picks this turn's settings for an effect, each somewhere between how it's been set and
    /// a random point in its range.
    fn roll_settings(&mut self, id: &str) {
        let amount = f64::from(self.shuffle.randomise / 100.0).clamp(0.0, 1.0);
        let Some(effect) = self.effects.iter().find(|e| e.id == id).cloned().filter(|_| amount > 0.0) else { return };
        let mut rolled = Settings::default();
        // Not the network it restyles with: another might not be downloaded, or have nothing
        // to paint with.
        for spec in effect.settings().filter(|s| effect.manifest.restyle.as_deref() != Some(s.key.as_str())) {
            let current = self.settings.get(id).and_then(|s| s.get(&spec.key));
            let value = match spec.kind {
                SettingKind::Number => {
                    let (min, max) = (spec.min.unwrap_or(0.0), spec.max.unwrap_or(100.0));
                    let base = current.and_then(serde_json::Value::as_f64).unwrap_or(min);
                    let anywhere = min + f64::from(self.roll()) * (max - min);
                    let moved = base + (anywhere - base) * amount;
                    let snapped = spec.step.filter(|s| *s > 0.0).map_or(moved, |step| min + ((moved - min) / step).round() * step);
                    let value = ((snapped * 1e4).round() / 1e4).clamp(min, max);
                    if value.fract() == 0.0 { serde_json::json!(value as i64) } else { serde_json::json!(value) }
                }
                // Flipping half the time at most, so all the way up isn't always opposite.
                SettingKind::Bool => {
                    let on = current.and_then(serde_json::Value::as_bool).unwrap_or(false);
                    serde_json::json!(on != (f64::from(self.roll()) < amount * 0.5))
                }
                SettingKind::Choice if !spec.options.is_empty() && f64::from(self.roll()) < amount => {
                    let at = (self.roll() * spec.options.len() as f32) as usize;
                    serde_json::json!(spec.options[at.min(spec.options.len() - 1)].id)
                }
                _ => continue,
            };
            rolled.insert(spec.key.clone(), value);
        }
        self.rolled.insert(id.to_owned(), rolled);
    }

    /// The next effect for a shot like `hint`: any of them, but mostly ones that suit it, and
    /// none that has had a turn lately.
    fn next_suiting(&mut self, hint: ShotHint) -> Option<String> {
        let chosen = self.chosen.clone();
        let (recent, effects) = (&self.recent, &self.effects);
        let pool: Vec<(&str, f32)> = effects
            .iter()
            .filter(|e| self.in_shuffle(&e.id) && Some(&e.id) != chosen.as_ref() && !recent.contains(&e.id))
            .map(|e| (e.id.as_str(), suit_weight(&e.manifest.suits, hint)))
            .collect();
        let weights: Vec<f32> = pool.iter().map(|p| p.1).collect();
        let pool: Vec<String> = pool.into_iter().map(|p| p.0.to_owned()).collect();
        let roll = self.roll();
        let picked = weighted_pick(&weights, roll).map(|i| pool[i].clone())?;
        log::debug!("shuffle: {picked} for size {:?}, scene {:?}", hint.size, hint.scene);
        self.turns.retain(|id| *id != picked);
        Some(picked)
    }

    fn change_turn(&mut self, may_fade: bool, hint: Option<ShotHint>) {
        let from_sound = self.sound_id().map(str::to_owned);
        self.wait();
        let Some(next) = hint.and_then(|hint| self.next_suiting(hint)).or_else(|| self.next_turn()) else { return };
        let most_recent = self.effects.iter().filter(|e| self.in_shuffle(&e.id)).count() / RECENT_SHARE;
        self.recent.push_back(next.clone());
        while self.recent.len() > most_recent {
            self.recent.pop_front();
        }
        self.rolled.retain(|id, _| Some(id) == self.chosen.as_ref());
        self.roll_settings(&next);
        let from = self.chosen.replace(next);
        self.status = None;
        if self.pairs_sounds() {
            let paired = self.roll() * 100.0 < self.shuffle.sounds;
            let at = (self.roll() * self.sounds.len() as f32) as usize;
            self.turn_sound = self.sounds.get(at).filter(|_| paired).map(|s| s.id.clone());
        }
        self.fading = from.filter(|_| may_fade && self.shuffle.fade > 0.0).map(|from| Fading { from, from_sound, elapsed: 0.0 });
    }

    /// Whether the player should look for scene cuts.
    pub fn wants_cuts(&self) -> bool {
        self.on && self.shuffle.on && self.shuffle.at_cuts
    }

    /// The shot has just changed to one like `hint`: moves to the next effect if this one has
    /// had its turn.
    pub fn scene_changed(&mut self, hint: ShotHint) {
        let shortest = self.shuffle.shortest.min(self.shuffle.longest);
        if !self.wants_cuts() || self.turn < shortest {
            return;
        }
        self.change_turn(true, self.shuffle.by_shot.then_some(hint));
    }

    /// The effect fading out, and how far `active` has faded in over it (0–1), during a fade.
    pub fn fading(&self) -> Option<(ActiveEffect, f32)> {
        let fading = self.fading.as_ref()?;
        let from = self.effects.iter().find(|e| e.id == fading.from)?;
        let t = (fading.elapsed / self.shuffle.fade.max(0.01)).clamp(0.0, 1.0);
        Some((self.to_draw(from), t * t * (3.0 - 2.0 * t)))
    }

    /// How the picture's drawn at this point of a fade (see `Fade`).
    pub fn fade(&self) -> Fade {
        let Some((from, t)) = self.fading() else { return Fade { from: None, faded_in: 1.0, amount: 1.0 } };
        if !self.shuffle.through_normal {
            return Fade { from: Some((from, 1.0)), faded_in: t, amount: 1.0 };
        }
        // The first half eases the old one away, the second eases the new one in.
        let raw = self.fading.as_ref().map_or(1.0, |f| (f.elapsed / self.shuffle.fade.max(0.01)).clamp(0.0, 1.0));
        let ease = |t: f32| t * t * (3.0 - 2.0 * t);
        if raw < 0.5 { Fade { from: Some((from, 1.0 - ease(raw * 2.0))), faded_in: 0.0, amount: 0.0 } } else { Fade { from: None, faded_in: 1.0, amount: ease(raw * 2.0 - 1.0) } }
    }

    /// The sound effects there are.
    pub fn sound_effects(&self) -> &[SoundEffect] {
        &self.sounds
    }

    /// The sound effect picked on its own, whether or not it's playing.
    pub fn chosen_sound(&self) -> Option<&SoundEffect> {
        let id = self.sound.as_deref()?;
        self.sounds.iter().find(|s| s.id == id)
    }

    /// Picks the sound effect to play, switching sound effects on (none switches them off).
    pub fn choose_sound(&mut self, id: Option<&str>, db: &Db) {
        self.sound = id.map(str::to_owned);
        if let Err(e) = db.set_setting(SOUND_KEY, id.unwrap_or("")) {
            log::warn!("saving the sound effect: {e:#}");
        }
        self.set_sound_on(id.is_some(), db);
    }

    pub fn sound_is_on(&self) -> bool {
        self.sound_on
    }

    /// Switches sound effects off or on, keeping which one; on with none picked picks the first.
    pub fn set_sound_on(&mut self, on: bool, db: &Db) {
        self.sound_on = on;
        if on && self.chosen_sound().is_none() {
            self.sound = self.sounds.first().map(|s| s.id.clone());
            if let Err(e) = db.set_setting(SOUND_KEY, self.sound.as_deref().unwrap_or("")) {
                log::warn!("saving the sound effect: {e:#}");
            }
        }
        if let Err(e) = db.set_setting(SOUND_ON_KEY, if on { "1" } else { "0" }) {
            log::warn!("saving whether sound effects are on: {e:#}");
        }
    }

    /// Whether shuffle picks the sound effects, rather than the one picked on its own playing:
    /// only while video effects are on and taking turns.
    pub fn pairs_sounds(&self) -> bool {
        self.on && self.shuffle.on && self.shuffle.sounds > 0.0
    }

    fn sound_id(&self) -> Option<&str> {
        if !self.sound_on {
            None
        } else if self.pairs_sounds() {
            self.turn_sound.as_deref()
        } else {
            self.chosen_sound().map(|s| s.id.as_str())
        }
    }

    /// The sound effect playing now, if any.
    pub fn sound_playing(&self) -> Option<&SoundEffect> {
        let id = self.sound_id()?;
        self.sounds.iter().find(|s| s.id == id)
    }

    /// The sound effect playing, with its settings.
    pub fn active_sound(&self) -> Option<ActiveSound> {
        self.to_play(self.sound_id()?)
    }

    /// During a fade that changes the sound effect, the one fading out (or none), and how far
    /// the new one has faded in over it (0–1): the same as the picture.
    pub fn sound_fading(&self) -> Option<(Option<ActiveSound>, f32)> {
        let fading = self.fading.as_ref()?;
        if fading.from_sound.as_deref() == self.sound_id() {
            return None;
        }
        let t = (fading.elapsed / self.shuffle.fade.max(0.01)).clamp(0.0, 1.0);
        Some((fading.from_sound.as_deref().and_then(|id| self.to_play(id)), t * t * (3.0 - 2.0 * t)))
    }

    fn to_play(&self, id: &str) -> Option<ActiveSound> {
        let sound = self.sounds.iter().find(|s| s.id == id)?;
        let settings_id = sound_settings_id(id);
        let (mut numbers, mut choices) = (Vec::new(), Vec::new());
        for spec in &sound.settings {
            let value = self.value(&settings_id, &spec.key);
            match spec.kind {
                SettingKind::Bool => numbers.push((spec.key.clone(), f32::from(u8::from(value.and_then(serde_json::Value::as_bool).unwrap_or(false))))),
                SettingKind::Choice => {
                    let picked = value.and_then(serde_json::Value::as_str).filter(|v| spec.options.iter().any(|o| o.id == *v));
                    if let Some(id) = picked.or_else(|| spec.options.first().map(|o| o.id.as_str())) {
                        choices.push((spec.key.clone(), id.to_owned()));
                    }
                }
                _ => numbers.push((spec.key.clone(), value.and_then(serde_json::Value::as_f64).unwrap_or(spec.min.unwrap_or(0.0)) as f32)),
            }
        }
        Some(ActiveSound { id: id.to_owned(), numbers, choices })
    }

    /// Everything the chosen effect needs and, with shuffle on, everything any effect it
    /// might pick needs. That keeps the sound filter and the depth model running between
    /// effects, so sound doesn't drop out and the picture's hold doesn't change at each one.
    pub fn needs(&self) -> Needs {
        let fading = self.fading.as_ref().map(|f| f.from.as_str());
        let counted = self.effects.iter().filter(|e| {
            let id = Some(e.id.as_str());
            self.on && (id == self.chosen.as_deref() || id == fading || (self.shuffle.on && self.in_shuffle(&e.id)))
        });
        let mut needs = Needs { audio: false, depth: false, segment: false, faces: false, mesh: false, history: false, flow: false, plate: false, cuts: false, stats: false, scene: false, camera: false, restyle: None, sound: false, scale: 0.25 };
        for e in counted {
            let m = &e.manifest;
            // Shuffle needs to know about the shot to choose an effect that suits it.
            let scene = m.scene || (self.wants_cuts() && self.shuffle.by_shot && !m.suits.is_empty());
            needs.audio |= m.audio || scene;
            needs.depth |= m.depth || m.segment || m.plate || scene || m.camera;
            needs.segment |= m.segment || m.plate || scene || m.camera;
            needs.faces |= m.faces || m.mesh || scene;
            needs.mesh |= m.mesh;
            needs.history |= m.history;
            needs.flow |= m.flow;
            needs.plate |= m.plate;
            needs.cuts |= m.cuts || scene || m.camera;
            needs.camera |= m.camera;
            needs.stats |= m.stats;
            needs.scene |= scene;
            needs.scale = needs.scale.max(usable_scale(m.scale));
        }
        needs.sound = self.sound_on && (self.chosen_sound().is_some() || self.pairs_sounds());
        if self.on {
            needs.restyle = [self.chosen.as_deref(), fading].into_iter().flatten().find_map(|id| self.restyle_for(id));
            // Restyled pictures are steadied along the movement.
            needs.flow |= needs.restyle.is_some();
        }
        needs
    }

    /// The network an effect restyles the picture with, if it does.
    fn restyle_for(&self, id: &str) -> Option<String> {
        let e = self.effects.iter().find(|e| e.id == id)?;
        let key = e.manifest.restyle.as_deref()?;
        let spec = e.settings().find(|s| s.key == key);
        let picked = self.value(id, key).and_then(serde_json::Value::as_str).map(str::to_owned);
        let first = spec.and_then(|s| s.default.as_ref()?.as_str().map(str::to_owned)).or_else(|| spec?.options.first().map(|o| o.id.clone()));
        picked.or(first).filter(|m| RESTYLES.contains(&m.as_str()) || TEXTURES.contains(&m.as_str()) || m.starts_with(ART))
    }

    /// The pictures in the styles folder, offered (after the other `ART` styles) by every
    /// setting that offers `ART` styles, in place of the ones offered before.
    pub fn set_art_styles(&mut self, mine: &[Choice]) {
        self.art_mine = mine.to_vec();
        self.offer_art_mine();
    }

    fn offer_art_mine(&mut self) {
        for e in &mut self.effects {
            for s in &mut e.manifest.settings {
                if s.kind != SettingKind::Choice || !s.options.iter().any(|o| o.id.starts_with(ART) && !o.id.starts_with(ART_MINE)) {
                    continue;
                }
                s.options.retain(|o| !o.id.starts_with(ART_MINE));
                let after = s.options.iter().rposition(|o| o.id.starts_with(ART)).map_or(s.options.len(), |i| i + 1);
                s.options.splice(after..after, self.art_mine.iter().cloned());
            }
        }
    }

    fn roll(&mut self) -> f32 {
        if self.random == 0 {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
            self.random = now | 1;
        }
        // xorshift: the order only has to look random.
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        (self.random >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Starts counting down to the next change.
    fn wait(&mut self) {
        let (a, b) = (self.shuffle.shortest, self.shuffle.longest);
        let (shortest, longest) = (a.min(b).max(0.0), a.max(b));
        self.until_next = if self.shuffle.at_cuts { longest } else { shortest + self.roll() * (longest - shortest) };
        self.turn = 0.0;
    }

    /// The next effect, going through them all in a random order before any comes round
    /// again. `None` if there's no other to go to.
    fn next_turn(&mut self) -> Option<String> {
        let pool: Vec<String> = self.effects.iter().filter(|e| self.in_shuffle(&e.id)).map(|e| e.id.clone()).collect();
        self.turns.retain(|id| pool.contains(id));
        for _ in 0..2 {
            while let Some(id) = self.turns.pop() {
                if Some(&id) != self.chosen.as_ref() {
                    return Some(id);
                }
            }
            self.turns = pool.clone();
            for i in (1..self.turns.len()).rev() {
                let j = (self.roll() * (i + 1) as f32) as usize;
                self.turns.swap(i, j.min(i));
            }
        }
        None
    }

    /// Off → each effect in turn → off.
    pub fn cycle(&mut self, db: &Db) {
        let at = self.chosen().and_then(|c| self.effects.iter().position(|e| e.id == c.id));
        let next = match at {
            None => self.effects.first(),
            Some(i) => self.effects.get(i + 1),
        };
        let id = next.map(|e| e.id.clone());
        self.choose(id.as_deref(), db);
    }

    pub fn value(&self, id: &str, key: &str) -> Option<&serde_json::Value> {
        self.rolled.get(id).and_then(|s| s.get(key)).or_else(|| self.settings.get(id)?.get(key))
    }

    pub fn set_setting(&mut self, id: &str, key: &str, value: serde_json::Value, db: &Db) {
        self.set_value(id, key, value);
        self.save(id, db);
    }

    /// Changes a setting without saving it yet (e.g. while a slider is dragged).
    pub fn set_value(&mut self, id: &str, key: &str, value: serde_json::Value) {
        if let Some(rolled) = self.rolled.get_mut(id) {
            rolled.insert(key.to_owned(), value.clone());
        }
        self.settings.entry(id.to_owned()).or_default().insert(key.to_owned(), value);
    }

    /// Saves an effect's settings.
    pub fn save(&self, id: &str, db: &Db) {
        let Some(settings) = self.settings.get(id) else { return };
        let json = serde_json::Value::Object(settings.clone()).to_string();
        if let Err(e) = db.set_setting(&settings_key(id), &json) {
            log::warn!("saving video effect settings: {e:#}");
        }
    }

    /// The chosen effect, ready to draw.
    pub fn active(&self) -> Option<ActiveEffect> {
        self.chosen().map(|e| self.to_draw(e))
    }

    fn to_draw(&self, e: &Effect) -> ActiveEffect {
        let uniforms = e
            .settings()
            .filter(|s| !s.key.is_empty() && s.key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .map(|s| {
                let value = self.value(&e.id, &s.key);
                let v = match s.kind {
                    SettingKind::Bool => f32::from(u8::from(value.and_then(serde_json::Value::as_bool).unwrap_or(false))),
                    SettingKind::Choice => {
                        let id = value.and_then(serde_json::Value::as_str).unwrap_or("");
                        s.options.iter().position(|o| o.id == id).unwrap_or(0) as f32
                    }
                    _ => self.drifted(&e.id, s, value.and_then(serde_json::Value::as_f64).unwrap_or(s.min.unwrap_or(0.0))) as f32,
                };
                (format!("s_{}", s.key), v)
            })
            .collect();
        ActiveEffect {
            id: e.id.clone(),
            name: e.manifest.name.clone(),
            shader: e.shader.clone(),
            feedback: e.manifest.feedback,
            audio: e.manifest.audio,
            depth: e.manifest.depth,
            segment: e.manifest.segment,
            faces: e.manifest.faces,
            restyle: e.manifest.restyle.is_some(),
            scale: usable_scale(e.manifest.scale),
            uniforms,
        }
    }
}

fn usable_scale(scale: f32) -> f32 {
    if scale.is_finite() { scale.clamp(0.25, 1.0) } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_only_say_they_suit_kinds_of_shot_that_exist() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../effects");
        let mut found = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "toml") {
                let manifest: Manifest = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                found += manifest.suits.len();
                assert!(manifest.suits.iter().all(|s| SUITS.contains(&s.as_str())), "{path:?}");
            }
        }
        assert!(found > 0);
    }

    #[test]
    fn shuffle_saved_before_there_was_a_choice_goes_by_the_shot() {
        let saved: Shuffle = serde_json::from_str(r#"{"on":true,"at_cuts":true}"#).unwrap();
        assert!(saved.by_shot);
    }

    #[test]
    fn shots_favour_the_effects_that_suit_them() {
        let close = vec!["close".to_owned()];
        let wide = vec!["wide".to_owned()];
        let hint = ShotHint { size: Some(0.8), scene: None };
        assert!(suit_weight(&close, hint) > 2.5);
        assert!(suit_weight(&wide, hint) < 0.3);
        assert_eq!(suit_weight(&[], hint), 1.0);
        // Nothing known: as likely as any other.
        assert_eq!(suit_weight(&close, ShotHint::default()), 1.0);
    }

    #[test]
    fn a_pick_goes_by_weight() {
        assert_eq!(weighted_pick(&[1.0, 3.0], 0.2), Some(0));
        assert_eq!(weighted_pick(&[1.0, 3.0], 0.3), Some(1));
        assert_eq!(weighted_pick(&[1.0, 3.0], 1.0), Some(1));
        assert_eq!(weighted_pick(&[], 0.5), None);
    }

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }

    #[test]
    fn finds_effects_and_keeps_their_settings() {
        let root = std::env::temp_dir().join(format!("flex-effects-{}", std::process::id()));
        let (shipped, mine) = (root.join("shipped"), root.join("mine"));
        write(&shipped, "melt.frag", "");
        write(
            &shipped,
            "melt.toml",
            "name = \"Melt\"\nfeedback = true\nscale = 0.1\naudio = true\n\
             [[settings]]\nkey = \"speed\"\nlabel = \"Speed\"\ntype = \"number\"\ndefault = 1.5\n\
             [[settings]]\nkey = \"mirror\"\nlabel = \"Mirror\"\ntype = \"bool\"\ndefault = true\n\
             [[settings]]\nkey = \"mode\"\nlabel = \"Mode\"\ntype = \"choice\"\ndefault = \"b\"\n\
             options = [{ id = \"a\", label = \"A\" }, { id = \"b\", label = \"B\" }]\n\
             [[settings]]\nkey = \"bad key\"\nlabel = \"Bad\"\ntype = \"number\"\n\
             [[settings]]\nkey = \"note\"\nlabel = \"Note\"\ntype = \"text\"\n",
        );
        write(&shipped, "dream.frag", "");
        write(&mine, "dream.frag", "");
        write(&mine, "dream.toml", "this isn't toml [");
        write(&mine, "my-test_one.frag", "");
        write(&mine, "readme.txt", "");

        let found = discover(&[shipped.clone(), mine.clone(), root.join("missing")]);
        let names: Vec<&str> = found.iter().map(|e| e.manifest.name.as_str()).collect();
        assert_eq!(names, ["Dream", "Melt", "My test one"]);
        assert_eq!(found[0].shader, mine.join("dream.frag"), "the data folder wins");
        assert!(found[0].problem.is_some());

        let db = Db::open_in_memory().unwrap();
        let dirs = [shipped, mine];
        let mut effects = Effects::new(&dirs, &db);
        assert_eq!(effects.active(), None, "off to start with");
        effects.choose(Some("melt"), &db);
        let active = effects.active().unwrap();
        assert_eq!((active.feedback, active.audio, active.scale), (true, true, 0.25));
        let uniforms = [("s_speed".to_owned(), 1.5), ("s_mirror".to_owned(), 1.0), ("s_mode".to_owned(), 1.0)];
        assert_eq!(active.uniforms, uniforms);

        effects.set_setting("melt", "speed", serde_json::json!(0.5), &db);
        // Choices and settings survive a restart.
        let mut effects = Effects::new(&dirs, &db);
        assert_eq!(effects.active().unwrap().uniforms[0], ("s_speed".to_owned(), 0.5));
        effects.cycle(&db);
        assert_eq!(effects.chosen().map(|e| e.id.as_str()), Some("my-test_one"));
        effects.cycle(&db);
        assert!(effects.chosen().is_none(), "after the last comes off");
        effects.cycle(&db);
        assert_eq!(effects.chosen().map(|e| e.id.as_str()), Some("dream"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_sound_effects_are_all_there_and_described() {
        let sounds = sound_effects();
        assert_eq!(sounds.len(), flex_sound::EFFECTS.len(), "each one's .toml reads");
        for sound in &sounds {
            assert!(!sound.name.is_empty() && !sound.description.is_empty(), "{}", sound.id);
            assert!(flex_sound::make(&sound.id, 48_000, &Default::default()).is_some(), "{}", sound.id);
        }
        // Each choice's options are ones the engine knows.
        let instruments: Vec<&str> = flex_sound::ddsp::INSTRUMENTS.iter().map(|(id, _)| *id).collect();
        let scales: Vec<&str> = flex_sound::SCALES.iter().map(|(id, _)| *id).collect();
        let harmonies: Vec<&str> = flex_sound::HARMONIES.iter().map(|(id, _)| *id).collect();
        let known: [(&str, &str, &[&str]); 9] = [
            ("voice", "voice", &flex_sound::VOICES),
            ("autotune", "key", &flex_sound::KEYS),
            ("autotune", "scale", &scales),
            ("autotune", "harmony", &harmonies),
            ("synth", "sound", &flex_sound::SOUNDS),
            ("instrument", "instrument", &instruments),
            ("reverb", "space", &flex_sound::SPACES),
            ("old", "style", &flex_sound::STYLES),
            ("dialogue", "mode", &flex_sound::MODES),
        ];
        for (effect, key, ids) in known {
            let sound = sounds.iter().find(|s| s.id == effect).unwrap();
            for option in &sound.settings.iter().find(|s| s.key == key).unwrap().options {
                assert!(ids.contains(&option.id.as_str()), "{effect} has no {key} {}", option.id);
            }
        }
    }

    #[test]
    fn the_sound_effect_picked_plays_with_any_effect() {
        let root = std::env::temp_dir().join(format!("flex-effects-sound-{}", std::process::id()));
        write(&root, "plain.frag", "");
        let db = Db::open_in_memory().unwrap();
        let mut effects = Effects::new(std::slice::from_ref(&root), &db);
        effects.choose(Some("plain"), &db);
        assert!(!effects.needs().sound);
        assert_eq!(effects.active_sound(), None);
        effects.choose_sound(Some("voice"), &db);
        assert!(effects.needs().sound);
        let playing = effects.active_sound().unwrap();
        assert_eq!(playing.choices, vec![("voice".to_owned(), "chipmunk".to_owned())], "its default");
        assert!(playing.numbers.contains(&("amount".to_owned(), 100.0)));
        effects.set_setting(&sound_settings_id("voice"), "voice", serde_json::json!("deep"), &db);
        assert_eq!(effects.active_sound().unwrap().choices, vec![("voice".to_owned(), "deep".to_owned())]);
        // On its own, with the video effects off.
        effects.set_on(false, &db);
        assert_eq!(effects.active_sound().map(|s| s.id), Some("voice".to_owned()));
        assert!(effects.needs().sound);
        // Kept, but silent, while sound effects are off; and remembered.
        effects.set_on(true, &db);
        effects.set_sound_on(false, &db);
        assert_eq!(effects.active_sound(), None);
        assert!(!effects.needs().sound);
        assert_eq!(effects.chosen_sound().map(|s| s.id.as_str()), Some("voice"));
        let again = Effects::new(std::slice::from_ref(&root), &db);
        assert_eq!(again.chosen_sound().map(|s| s.id.as_str()), Some("voice"));
        assert!(!again.sound_is_on());
        assert_eq!(again.value(&sound_settings_id("voice"), "voice"), Some(&serde_json::json!("deep")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shuffle_pairs_sound_effects_and_changes_them_with_the_picture() {
        let root = std::env::temp_dir().join(format!("flex-effects-pair-{}", std::process::id()));
        for id in ["a", "b", "c"] {
            write(&root, &format!("{id}.frag"), "");
        }
        let db = Db::open_in_memory().unwrap();
        let mut effects = Effects::new(std::slice::from_ref(&root), &db);
        effects.choose_sound(Some("voice"), &db);
        let shuffle = Shuffle { on: true, shortest: 10.0, longest: 10.0, fade: 2.0, ..Shuffle::default() };
        // Not pairing: the one picked plays throughout, with no fade of its own.
        effects.set_shuffle(shuffle, &db);
        effects.step(10.5, true);
        assert!(effects.fading().is_some());
        assert!(effects.sound_fading().is_none());
        assert_eq!(effects.active_sound().map(|s| s.id), Some("voice".to_owned()));
        // Always pairing: every turn comes with one, from when the turn changes.
        effects.set_shuffle(Shuffle { sounds: 100.0, ..shuffle }, &db);
        assert!(effects.needs().sound);
        assert_eq!(effects.active_sound(), None, "until the next turn");
        effects.step(10.5, true);
        let paired = effects.active_sound().map(|s| s.id).expect("a sound effect with the turn");
        assert!(flex_sound::EFFECTS.iter().any(|(id, _)| *id == paired));
        let (from, t) = effects.sound_fading().expect("it fades in with the picture");
        assert_eq!((from, t), (None, effects.fading().unwrap().1));
        effects.step(2.0, true);
        assert!(effects.sound_fading().is_none());
        // Hardly ever pairing: most turns have none, whatever's picked on its own.
        effects.set_shuffle(Shuffle { sounds: 1.0, ..shuffle }, &db);
        let mut quiet = false;
        for _ in 0..20 {
            effects.step(12.5, true);
            quiet |= effects.active_sound().is_none();
        }
        assert!(quiet, "at 1%, most turns have none");
        // With the video effects off there are no turns: the one picked plays.
        effects.set_on(false, &db);
        assert_eq!(effects.active_sound().map(|s| s.id), Some("voice".to_owned()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_restyled_effect_asks_for_the_style_its_setting_picks() {
        let root = std::env::temp_dir().join(format!("flex-effects-restyle-{}", std::process::id()));
        write(&root, "anime.frag", "");
        write(
            &root,
            "anime.toml",
            "restyle = \"style\"\n[[settings]]\nkey = \"style\"\nlabel = \"Style\"\ntype = \"choice\"\ndefault = \"hayao\"\n\
             options = [{ id = \"hayao\", label = \"Hayao\" }, { id = \"shinkai\", label = \"Shinkai\" }]\n",
        );
        write(&root, "plain.frag", "");
        let db = Db::open_in_memory().unwrap();
        let mut effects = Effects::new(std::slice::from_ref(&root), &db);
        effects.choose(Some("plain"), &db);
        assert_eq!(effects.needs().restyle, None);
        assert!(!effects.active().unwrap().restyle);
        effects.choose(Some("anime"), &db);
        let needs = effects.needs();
        assert_eq!(needs.restyle.as_deref(), Some("hayao"));
        assert!(needs.flow, "restyled pictures are moved along with the picture");
        assert!(effects.active().unwrap().restyle);
        effects.set_setting("anime", "style", serde_json::json!("shinkai"), &db);
        assert_eq!(effects.needs().restyle.as_deref(), Some("shinkai"));
        effects.set_setting("anime", "style", serde_json::json!("nonsense"), &db);
        assert_eq!(effects.needs().restyle, None, "only styles there are");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pictures_in_the_styles_folder_are_offered_where_painting_styles_are() {
        let root = std::env::temp_dir().join(format!("flex-effects-art-{}", std::process::id()));
        write(&root, "art.frag", "");
        write(
            &root,
            "art.toml",
            "restyle = \"style\"\n[[settings]]\nkey = \"style\"\nlabel = \"Style\"\ntype = \"choice\"\ndefault = \"art:morph\"\n\
             options = [{ id = \"art:morph\", label = \"All of them\" }, { id = \"art:scream\", label = \"The Scream\" }, { id = \"tex-foam\", label = \"Foam\" }]\n",
        );
        write(&root, "anime.frag", "");
        write(&root, "anime.toml", "restyle = \"style\"\n[[settings]]\nkey = \"style\"\nlabel = \"Style\"\ntype = \"choice\"\noptions = [{ id = \"hayao\", label = \"Hayao\" }]\n");
        let db = Db::open_in_memory().unwrap();
        let mut effects = Effects::new(std::slice::from_ref(&root), &db);
        effects.choose(Some("art"), &db);
        assert_eq!(effects.needs().restyle.as_deref(), Some("art:morph"));
        let mine = |names: &[&str]| names.iter().map(|n| Choice { id: format!("{ART_MINE}{n}"), label: (*n).into() }).collect::<Vec<_>>();
        effects.set_art_styles(&mine(&["cat.jpg", "fabric.png"]));
        effects.set_art_styles(&mine(&["cat.jpg"]));
        let ids = |effects: &Effects, id: &str| effects.list().iter().find(|e| e.id == id).unwrap().manifest.settings[0].options.iter().map(|o| o.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&effects, "art"), ["art:morph", "art:scream", "art:mine:cat.jpg", "tex-foam"], "replaced, not added to, and with the other paintings");
        assert_eq!(ids(&effects, "anime"), ["hayao"]);
        effects.refresh(std::slice::from_ref(&root), &db);
        assert_eq!(ids(&effects, "art"), ["art:morph", "art:scream", "art:mine:cat.jpg", "tex-foam"], "still there after looking again");
        assert!(effects.list().iter().find(|e| e.id == "art").unwrap().takes_art());
        effects.set_setting("art", "style", serde_json::json!("art:mine:cat.jpg"), &db);
        assert_eq!(effects.needs().restyle.as_deref(), Some("art:mine:cat.jpg"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fading_through_normal_eases_one_out_then_the_next_in() {
        let root = std::env::temp_dir().join(format!("flex-effects-through-{}", std::process::id()));
        write(&root, "a.frag", "");
        write(&root, "b.frag", "");
        let db = Db::open_in_memory().unwrap();
        let dirs = [root.clone()];
        let mut effects = Effects::new(&dirs, &db);
        effects.set_shuffle(Shuffle { on: true, shortest: 10.0, longest: 10.0, fade: 2.0, through_normal: true, at_cuts: false, by_shot: false, randomise: 0.0, sounds: 0.0 }, &db);
        let first = effects.chosen().unwrap().id.clone();
        effects.step(10.5, true);
        assert_ne!(effects.chosen().unwrap().id, first);
        let fade = effects.fade();
        assert_eq!(fade.from.as_ref().map(|(e, _)| e.id.clone()), Some(first.clone()));
        assert_eq!((fade.faded_in, fade.amount), (0.0, 0.0));
        effects.step(0.5, true);
        assert!((effects.fade().from.unwrap().1 - 0.5).abs() < 1e-4, "halfway out");
        effects.step(0.75, true);
        let fade = effects.fade();
        assert!(fade.from.is_none() && fade.faded_in == 1.0 && fade.amount > 0.0 && fade.amount < 1.0);
        effects.step(1.0, true);
        assert_eq!(effects.fade().amount, 1.0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shuffle_takes_turns_and_fades() {
        let root = std::env::temp_dir().join(format!("flex-effects-shuffle-{}", std::process::id()));
        write(&root, "a.frag", "");
        write(&root, "a.toml", "audio = true\nscale = 0.5\n");
        write(&root, "b.frag", "");
        write(&root, "b.toml", "depth = true\n");
        write(&root, "c.frag", "");
        write(&root, "test.frag", "");
        write(&root, "test.toml", "faces = true\nshuffle = false\n");
        let db = Db::open_in_memory().unwrap();
        let dirs = [root.clone()];
        let mut effects = Effects::new(&dirs, &db);
        assert!(!effects.needs().audio, "nothing's on");

        effects.set_shuffle(Shuffle { on: true, shortest: 10.0, longest: 10.0, fade: 2.0, through_normal: false, at_cuts: false, by_shot: false, randomise: 0.0, sounds: 0.0 }, &db);
        let first = effects.chosen().expect("shuffle starts one").id.clone();
        let needs = effects.needs();
        assert_eq!((needs.audio, needs.depth, needs.faces, needs.scale), (true, true, false, 1.0), "all that it might pick");

        // Stays put while paused, and until its time's up.
        effects.step(100.0, false);
        effects.step(9.0, true);
        assert_eq!(effects.chosen().unwrap().id, first);
        assert!(effects.fading().is_none());

        effects.step(2.0, true);
        let second = effects.chosen().unwrap().id.clone();
        assert_ne!(second, first);
        let (from, amount) = effects.fading().unwrap();
        assert_eq!((from.id.as_str(), amount), (first.as_str(), 0.0));
        effects.step(1.0, false);
        assert_eq!(effects.fading().unwrap().1, 0.5, "the fade carries on while paused");
        effects.step(1.0, true);
        assert!(effects.fading().is_none());

        // Each once before any comes round again, and never the test view.
        let mut seen = vec![first, second];
        effects.step(10.5, true);
        effects.step(2.0, true);
        seen.push(effects.chosen().unwrap().id.clone());
        seen.sort();
        assert_eq!(seen, ["a", "b", "c"]);

        // One that won't compile is left at once, without a fade.
        effects.status = Some((false, "Shader error".into()));
        let broken = effects.chosen().unwrap().id.clone();
        effects.step(0.1, true);
        assert_ne!(effects.chosen().unwrap().id, broken);
        assert!(effects.fading().is_none());

        // At cuts: not before the shortest turn, and only when asked to.
        effects.set_shuffle(Shuffle { on: true, shortest: 10.0, longest: 30.0, fade: 2.0, through_normal: false, at_cuts: true, by_shot: false, randomise: 0.0, sounds: 0.0 }, &db);
        let before = effects.chosen().unwrap().id.clone();
        effects.step(9.0, true);
        effects.scene_changed(ShotHint::default());
        assert_eq!(effects.chosen().unwrap().id, before, "too soon");
        effects.step(2.0, true);
        effects.scene_changed(ShotHint::default());
        assert_ne!(effects.chosen().unwrap().id, before);
        assert_eq!(effects.fading().unwrap().0.id, before, "fades there too");
        let at_cut = effects.chosen().unwrap().id.clone();
        effects.step(29.0, true);
        assert_eq!(effects.chosen().unwrap().id, at_cut, "only the longest turn moves it without a cut");
        effects.step(2.0, true);
        assert_ne!(effects.chosen().unwrap().id, at_cut);
        // A fade of 0 switches at once, whichever way it's changing.
        effects.set_shuffle(Shuffle { on: true, shortest: 1.0, longest: 30.0, fade: 0.0, through_normal: false, at_cuts: true, by_shot: false, randomise: 0.0, sounds: 0.0 }, &db);
        effects.step(2.0, true);
        let sharp = effects.chosen().unwrap().id.clone();
        effects.scene_changed(ShotHint::default());
        assert_ne!(effects.chosen().unwrap().id, sharp);
        assert!(effects.fading().is_none());
        effects.set_shuffle(Shuffle { on: true, shortest: 5.0, longest: 5.0, fade: 0.0, through_normal: false, at_cuts: false, by_shot: false, randomise: 0.0, sounds: 0.0 }, &db);
        let sharp = effects.chosen().unwrap().id.clone();
        effects.step(6.0, true);
        assert_ne!(effects.chosen().unwrap().id, sharp);
        assert!(effects.fading().is_none());
        effects.set_shuffle(Shuffle { on: true, shortest: 10.0, longest: 10.0, fade: 2.0, through_normal: false, at_cuts: false, by_shot: false, randomise: 0.0, sounds: 0.0 }, &db);
        effects.step(2.0, false);
        effects.step(11.0, true);
        let moved = effects.chosen().unwrap().id.clone();
        effects.step(2.5, true);
        effects.scene_changed(ShotHint::default());
        assert_eq!(effects.chosen().unwrap().id, moved, "cuts are ignored unless asked for");

        // Choosing one stops any fade; both are kept.
        effects.set_in_shuffle("c", false, &db);
        effects.choose(Some("a"), &db);
        assert!(Effects::new(&dirs, &db).shuffle().on);
        assert!(!Effects::new(&dirs, &db).in_shuffle("c"));

        // Switched off, nothing runs or changes, and it all comes back where it was.
        effects.set_on(false, &db);
        assert!(effects.chosen().is_none() && effects.active().is_none());
        assert!(!effects.needs().depth && !effects.wants_cuts());
        for _ in 0..200 {
            effects.step(1.0, true);
        }
        assert_eq!(effects.remembered().unwrap().id, "a");
        let reopened = Effects::new(&dirs, &db);
        assert!(!reopened.is_on() && reopened.chosen().is_none() && reopened.shuffle().on);
        effects.set_on(true, &db);
        assert_eq!(effects.chosen().unwrap().id, "a");
        assert!(effects.shuffle().on);

        // Choosing none is switching off, and choosing one switches back on.
        effects.choose(None, &db);
        assert!(!effects.is_on() && effects.shuffle().on);
        effects.choose(Some("b"), &db);
        assert!(effects.is_on() && Effects::new(&dirs, &db).is_on());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn favourites_are_kept_and_nothing_chosen_starts_off() {
        let root = std::env::temp_dir().join(format!("flex-effects-fav-{}", std::process::id()));
        write(&root, "one.frag", "");
        let dirs = [root.clone()];
        let db = Db::open_in_memory().unwrap();
        let mut effects = Effects::new(&dirs, &db);
        assert!(!effects.is_on());
        assert!(!effects.is_favourite("one"));
        effects.set_favourite("one", true, &db);
        assert!(Effects::new(&dirs, &db).is_favourite("one"));
        // Nothing to go back to, so switching on finds one.
        effects.set_on(true, &db);
        assert_eq!(effects.chosen().unwrap().id, "one");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shuffle_can_roll_settings_without_touching_the_saved_ones() {
        let root = std::env::temp_dir().join(format!("flex-effects-roll-{}", std::process::id()));
        let toml = "[[settings]]\nkey = \"size\"\nlabel = \"Size\"\ntype = \"number\"\nmin = 0\nmax = 10\nstep = 2\ndefault = 4\n\
                    [[settings]]\nkey = \"mode\"\nlabel = \"Mode\"\ntype = \"choice\"\ndefault = \"a\"\n\
                    options = [{ id = \"a\", label = \"A\" }, { id = \"b\", label = \"B\" }]\n";
        for name in ["one", "two"] {
            write(&root, &format!("{name}.frag"), "");
            write(&root, &format!("{name}.toml"), toml);
        }
        let db = Db::open_in_memory().unwrap();
        let dirs = [root.clone()];
        let mut effects = Effects::new(&dirs, &db);
        effects.choose(Some("one"), &db);
        let on = Shuffle { on: true, shortest: 1.0, longest: 1.0, fade: 0.0, through_normal: false, at_cuts: false, by_shot: false, randomise: 100.0, sounds: 0.0 };
        effects.set_shuffle(on, &db);

        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            effects.step(2.0, true);
            let id = effects.chosen().unwrap().id.clone();
            let size = effects.value(&id, "size").unwrap().as_i64().expect("on the grid");
            assert!((0..=10).contains(&size) && size % 2 == 0, "{size}");
            seen.insert((size, effects.value(&id, "mode").unwrap().as_str().unwrap().to_owned()));
        }
        assert!(seen.len() >= 8, "{seen:?}");

        // A change made now shows now, and is what's saved; the rolled values never are.
        let id = effects.chosen().unwrap().id.clone();
        effects.set_setting(&id, "size", serde_json::json!(6), &db);
        assert_eq!(effects.value(&id, "size").unwrap(), 6);
        let other = if id == "one" { "two" } else { "one" };
        let restarted = Effects::new(&dirs, &db);
        assert_eq!(restarted.value(&id, "size").unwrap(), 6);
        assert_eq!(restarted.value(other, "size").unwrap(), 4);

        // Half way: never further than half the way to the ends of the range.
        effects.set_shuffle(Shuffle { randomise: 50.0, ..on }, &db);
        for _ in 0..100 {
            effects.step(2.0, true);
            let id = effects.chosen().unwrap().id.clone();
            let base = if id == "one" || other == "one" { Effects::new(&dirs, &db).value(&id, "size").unwrap().as_i64().unwrap() } else { 4 };
            let size = effects.value(&id, "size").unwrap().as_i64().unwrap();
            assert!((size - base).abs() <= 4, "{size} from {base}");
        }

        // Turned off, it's back to the saved ones.
        effects.set_shuffle(Shuffle { randomise: 0.0, ..on }, &db);
        assert_eq!(effects.value(&id, "size").unwrap(), 6);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shuffle_leaves_the_restyle_network_alone() {
        let root = std::env::temp_dir().join(format!("flex-effects-roll-restyle-{}", std::process::id()));
        let toml = "restyle = \"style\"\n\
                    [[settings]]\nkey = \"style\"\nlabel = \"Style\"\ntype = \"choice\"\ndefault = \"hayao\"\n\
                    options = [{ id = \"hayao\", label = \"Hayao\" }, { id = \"art:morph\", label = \"All of them\" }]\n\
                    [[settings]]\nkey = \"mode\"\nlabel = \"Mode\"\ntype = \"choice\"\ndefault = \"a\"\n\
                    options = [{ id = \"a\", label = \"A\" }, { id = \"b\", label = \"B\" }]\n";
        for name in ["one", "two"] {
            write(&root, &format!("{name}.frag"), "");
            write(&root, &format!("{name}.toml"), toml);
        }
        let db = Db::open_in_memory().unwrap();
        let mut effects = Effects::new(std::slice::from_ref(&root), &db);
        effects.choose(Some("one"), &db);
        effects.set_shuffle(Shuffle { on: true, shortest: 1.0, longest: 1.0, fade: 0.0, through_normal: false, at_cuts: false, by_shot: false, randomise: 100.0, sounds: 0.0 }, &db);
        let mut modes = std::collections::HashSet::new();
        for _ in 0..100 {
            effects.step(2.0, true);
            let id = effects.chosen().unwrap().id.clone();
            assert_eq!(effects.value(&id, "style").unwrap(), "hayao");
            modes.insert(effects.value(&id, "mode").unwrap().as_str().unwrap().to_owned());
        }
        assert_eq!(modes.len(), 2, "the other choices are still rolled");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn settings_drift_slowly_and_stay_in_range() {
        let root = std::env::temp_dir().join(format!("flex-effects-drift-{}", std::process::id()));
        write(&root, "a.frag", "");
        write(
            &root,
            "a.toml",
            "[[settings]]\nkey = \"wide\"\nlabel = \"W\"\ntype = \"number\"\nmin = 0\nmax = 100\nstep = 5\nunit = \"%\"\ndefault = 50\n\
             [[settings]]\nkey = \"count\"\nlabel = \"C\"\ntype = \"number\"\nmin = 2\nmax = 12\nstep = 1\ndefault = 6\n\
             [[settings]]\nkey = \"rate\"\nlabel = \"R\"\ntype = \"number\"\nmin = 0\nmax = 10\ndrift = \"off\"\ndefault = 3\n",
        );
        let db = Db::open_in_memory().unwrap();
        let dirs = [root.clone()];
        let mut effects = Effects::new(&dirs, &db);
        effects.choose(Some("a"), &db);
        let uniform = |e: &Effects, i: usize| e.active().unwrap().uniforms[i].1;
        assert_eq!(uniform(&effects, 0), 50.0, "off to start with");

        effects.set_drift(Drift { amount: 100.0, speed: 100.0 }, &db);
        effects.step(0.016, true);
        assert!((uniform(&effects, 0) - 50.0).abs() < 2.0, "eases in rather than jumping");

        let (mut low, mut high, mut last) = (f32::MAX, f32::MIN, None::<f32>);
        for _ in 0..30000 {
            effects.step(0.033, true);
            let wide = uniform(&effects, 0);
            assert!((0.0..=100.0).contains(&wide));
            if let Some(last) = last {
                assert!((wide - last).abs() < 0.5, "slowly");
            }
            last = Some(wide);
            (low, high) = (low.min(wide), high.max(wide));
            let count = uniform(&effects, 1);
            assert!((2.0..=12.0).contains(&count) && count.fract() == 0.0, "{count}");
            assert_eq!(uniform(&effects, 2), 3.0, "rates stay put");
        }
        assert!(high - low > 40.0, "{low} to {high}");

        // Paused, nothing moves.
        let held = uniform(&effects, 0);
        for _ in 0..200 {
            effects.step(0.033, false);
        }
        assert!((uniform(&effects, 0) - held).abs() < 0.01);
        assert_eq!(Effects::new(&dirs, &db).drift().amount, 100.0, "kept");
        let _ = std::fs::remove_dir_all(&root);
    }
}
