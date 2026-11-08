//! Video effects: shaders Flex runs over the video (the drawing is in the app). Each is
//! a `<id>.frag` in an `effects` folder, with an optional `<id>.toml` for its name,
//! options and settings. One effect is chosen at a time (or none); its settings are kept
//! per effect.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use flex_plugin::Settings;
use serde::Deserialize;

use crate::Db;
use crate::plugins::{SettingKind, SettingSpec};

/// `<id>.toml`.
#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    /// Shown in Settings; defaults to the file's name.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Give the shader its own previous output (`u_prev`), for trails and feedback.
    #[serde(default)]
    pub feedback: bool,
    /// Give the shader the sound. Costs reading the file's audio a second time.
    #[serde(default)]
    pub audio: bool,
    /// Give the shader depth (`u_depth`), from a neural network on the GPU. Costs GPU time
    /// and a 99 MB download the first time.
    #[serde(default)]
    pub depth: bool,
    /// Size to work at, relative to the screen (0.25–1): lower is lighter on the GPU.
    #[serde(default = "full")]
    pub scale: f32,
    #[serde(default)]
    pub settings: Vec<SettingSpec>,
}

fn full() -> f32 {
    1.0
}

impl Default for Manifest {
    fn default() -> Self {
        Self { name: String::new(), description: String::new(), feedback: false, audio: false, depth: false, scale: 1.0, settings: Vec::new() }
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
    pub scale: f32,
    /// `s_<key>` = number, bool (0/1) or choice (its index).
    pub uniforms: Vec<(String, f32)>,
}

const CHOSEN_KEY: &str = "video_effect";

fn settings_key(id: &str) -> String {
    format!("video_effect.{id}")
}

/// The effects found, which one is on, and their settings.
#[derive(Default)]
pub struct Effects {
    effects: Vec<Effect>,
    /// The chosen effect's id (kept even if its file has gone).
    chosen: Option<String>,
    settings: HashMap<String, Settings>,
    /// The chosen effect's shader state, from the app: (ok, text).
    pub status: Option<(bool, String)>,
}

impl Effects {
    pub fn new(dirs: &[PathBuf], db: &Db) -> Self {
        let chosen = db.setting(CHOSEN_KEY).ok().flatten().filter(|s| !s.is_empty());
        let mut effects = Self { chosen, ..Self::default() };
        effects.refresh(dirs, db);
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
        log::info!("video effects: {}", self.effects.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(", "));
    }

    pub fn list(&self) -> &[Effect] {
        &self.effects
    }

    pub fn chosen(&self) -> Option<&Effect> {
        let id = self.chosen.as_deref()?;
        self.effects.iter().find(|e| e.id == id)
    }

    pub fn choose(&mut self, id: Option<&str>, db: &Db) {
        self.chosen = id.map(str::to_owned);
        self.status = None;
        if let Err(e) = db.set_setting(CHOSEN_KEY, id.unwrap_or("")) {
            log::warn!("saving the video effect: {e:#}");
        }
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
        self.settings.get(id)?.get(key)
    }

    pub fn set_setting(&mut self, id: &str, key: &str, value: serde_json::Value, db: &Db) {
        self.set_value(id, key, value);
        self.save(id, db);
    }

    /// Changes a setting without saving it yet (e.g. while a slider is dragged).
    pub fn set_value(&mut self, id: &str, key: &str, value: serde_json::Value) {
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
        let e = self.chosen()?;
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
                    _ => value.and_then(serde_json::Value::as_f64).unwrap_or(s.min.unwrap_or(0.0)) as f32,
                };
                (format!("s_{}", s.key), v)
            })
            .collect();
        let scale = e.manifest.scale;
        Some(ActiveEffect {
            id: e.id.clone(),
            name: e.manifest.name.clone(),
            shader: e.shader.clone(),
            feedback: e.manifest.feedback,
            audio: e.manifest.audio,
            depth: e.manifest.depth,
            scale: if scale.is_finite() { scale.clamp(0.25, 1.0) } else { 1.0 },
            uniforms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
