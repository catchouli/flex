//! Plugins: programs in a `plugins/<id>/` folder with a `plugin.toml`, which Flex starts
//! when switched on in Settings and talks to over stdin/stdout (see the `flex-plugin`
//! crate for the protocol). Nothing here runs unless someone installs and enables one.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use flex_plugin::{Choice, HostMessage, Light, LogLevel, Playback, PluginMessage, Settings};
use serde::Deserialize;

use crate::Db;

/// `plugin.toml`.
#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    /// The program: in the plugin's folder (".exe" is added on Windows), else a bare name
    /// like "python3" is looked up on the PATH.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables for the program.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Which messages it wants: "light", "playback".
    #[serde(default)]
    pub subscribe: Vec<String>,
    #[serde(default)]
    pub light: LightSpec,
    #[serde(default)]
    pub settings: Vec<SettingSpec>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LightSpec {
    /// Colours wanted per update.
    #[serde(default = "one")]
    pub zones: usize,
}

fn one() -> usize {
    1
}

impl Default for LightSpec {
    fn default() -> Self {
        Self { zones: 1 }
    }
}

/// A setting the plugin offers, shown in Settings → Plugins.
#[derive(Clone, Debug, Deserialize)]
pub struct SettingSpec {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: SettingKind,
    /// A line of help under the setting.
    #[serde(default)]
    pub detail: String,
    pub default: Option<serde_json::Value>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    #[serde(default)]
    pub unit: String,
    /// For `choice`: the starting options (the plugin can replace them).
    #[serde(default)]
    pub options: Vec<Choice>,
    /// For `text`: a password or key, shown only as "set" and typed in hidden.
    #[serde(default)]
    pub secret: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKind {
    Bool,
    Choice,
    Number,
    Text,
    /// Not stored: pressing it sends an action.
    Button,
}

impl Manifest {
    pub fn subscribes(&self, kind: &str) -> bool {
        self.subscribe.iter().any(|s| s == kind)
    }
}

/// A plugin found on disk.
#[derive(Clone, Debug)]
pub struct Installed {
    /// Its folder's name.
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
}

/// Where plugins are looked for: `plugins` next to Flex's program (bundled with a
/// release), then `plugins` in the data folder (installed by hand), which wins when both
/// have the same plugin.
pub fn plugin_dirs() -> Vec<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|d| d.join("plugins")));
    beside.into_iter().chain([user_plugin_dir()]).collect()
}

pub fn user_plugin_dir() -> PathBuf {
    crate::data_dir().join("plugins")
}

/// Every plugin in `dirs`, by id; later folders replace earlier ones.
pub fn discover(dirs: &[PathBuf]) -> Vec<Installed> {
    let mut found: Vec<Installed> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.join("plugin.toml").is_file()).collect();
        entries.sort();
        for dir in entries {
            let id = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
            match read_manifest(&dir) {
                Ok(manifest) => {
                    found.retain(|p| p.id != id);
                    found.push(Installed { id, dir, manifest });
                }
                Err(e) => log::warn!("plugin {}: {e:#}", dir.display()),
            }
        }
    }
    found
}

fn read_manifest(dir: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(dir.join("plugin.toml")).context("reading plugin.toml")?;
    toml::from_str(&text).context("reading plugin.toml")
}

/// The plugin's program: `command` in its folder (or an absolute path), else a bare
/// program name for the system to find on the PATH.
fn program(plugin: &Installed) -> PathBuf {
    let command = &plugin.manifest.command;
    let path = plugin.dir.join(command);
    let path = if cfg!(windows) && path.extension().is_none() { path.with_extension("exe") } else { path };
    let bare = Path::new(command).components().count() == 1;
    if bare && !path.exists() { PathBuf::from(command) } else { path }
}

/// Messages to one plugin. Light updates aren't queued: only the newest is kept, so a
/// slow plugin never holds anything up.
#[derive(Default)]
struct Outbox {
    state: Mutex<OutState>,
    ready: Condvar,
}

#[derive(Default)]
struct OutState {
    queue: VecDeque<HostMessage>,
    light: Option<Light>,
    /// No more messages: the writer sends what's queued, then closes stdin.
    closed: bool,
}

impl Outbox {
    fn lock(&self) -> std::sync::MutexGuard<'_, OutState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, msg: HostMessage) {
        let mut state = self.lock();
        if !state.closed {
            state.queue.push_back(msg);
            self.ready.notify_one();
        }
    }

    fn set_light(&self, light: Light) {
        let mut state = self.lock();
        if !state.closed {
            state.light = Some(light);
            self.ready.notify_one();
        }
    }

    fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_one();
    }

    /// The next message to write; `None` once closed and empty.
    fn next(&self) -> Option<HostMessage> {
        let mut state = self.lock();
        loop {
            if let Some(msg) = state.queue.pop_front() {
                return Some(msg);
            }
            if state.closed {
                return None;
            }
            if let Some(light) = state.light.take() {
                return Some(HostMessage::Light(light));
            }
            state = self.ready.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
}

enum Incoming {
    Message(PluginMessage),
    Exited,
}

struct Process {
    child: Child,
    outbox: Arc<Outbox>,
    started: Instant,
}

impl Process {
    /// Asks the plugin to exit, and kills it if it hasn't within `grace`.
    fn stop(mut self, grace: Duration) {
        self.outbox.send(HostMessage::Shutdown);
        self.outbox.close();
        std::thread::spawn(move || {
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline {
                if !matches!(self.child.try_wait(), Ok(None)) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        });
    }
}

/// A plugin and what Flex knows about it.
pub struct Plugin {
    pub installed: Installed,
    pub enabled: bool,
    /// Saved settings over the defaults from `plugin.toml`.
    pub settings: Settings,
    /// The plugin's last status line: (ok, text).
    pub status: Option<(bool, String)>,
    /// Options for `choice` settings, as sent by the plugin.
    pub choices: HashMap<String, Vec<Choice>>,
    process: Option<Process>,
    /// Bumped per start, so messages from an old process are ignored.
    generation: u64,
    restart_at: Option<Instant>,
    failures: u32,
}

impl Plugin {
    pub fn running(&self) -> bool {
        self.process.is_some()
    }

    pub fn subscribes(&self, kind: &str) -> bool {
        self.installed.manifest.subscribes(kind)
    }

    /// Options for a choice setting: the plugin's own list, else the manifest's.
    pub fn options<'a>(&'a self, spec: &'a SettingSpec) -> &'a [Choice] {
        self.choices.get(&spec.key).map_or(&spec.options, Vec::as_slice)
    }
}

fn enabled_key(id: &str) -> String {
    format!("plugin.{id}.enabled")
}

fn settings_key(id: &str) -> String {
    format!("plugin.{id}.settings")
}

/// Runs the enabled plugins and routes messages to and from them.
pub struct Host {
    plugins: Vec<Plugin>,
    tx: Sender<(usize, u64, Incoming)>,
    rx: Receiver<(usize, u64, Incoming)>,
    /// Called from plugin threads when a message comes in (to repaint the UI).
    waker: Arc<dyn Fn() + Send + Sync>,
    toasts: Vec<Toast>,
}

/// A message to show on screen (see `PluginMessage::Toast`).
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    /// How long to show it; `None` for the usual few seconds.
    pub seconds: Option<f64>,
    /// Replaces (or with empty text, removes) an earlier toast with the same key.
    pub key: Option<String>,
}

impl Toast {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), seconds: None, key: None }
    }
}

/// How long a plugin may take to exit after `shutdown`.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// Running this long counts as having started fine, resetting the restart back-off.
const HEALTHY_AFTER: Duration = Duration::from_secs(60);

impl Host {
    pub fn new(dirs: &[PathBuf], db: &Db, waker: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, rx) = channel();
        let mut host = Self { plugins: Vec::new(), tx, rx, waker: Arc::new(waker), toasts: Vec::new() };
        host.refresh(dirs, db);
        host
    }

    /// Looks for newly installed plugins (and forgets removed ones that aren't running).
    pub fn refresh(&mut self, dirs: &[PathBuf], db: &Db) {
        let found = discover(dirs);
        self.plugins.retain(|p| p.running() || found.iter().any(|f| f.id == p.installed.id));
        for installed in found {
            if let Some(p) = self.plugins.iter_mut().find(|p| p.installed.id == installed.id) {
                p.installed = installed;
                continue;
            }
            let id = &installed.id;
            let enabled = db.setting(&enabled_key(id)).ok().flatten().is_some_and(|v| v == "true");
            let mut settings: Settings = installed
                .manifest
                .settings
                .iter()
                .filter_map(|s| Some((s.key.clone(), s.default.clone()?)))
                .collect();
            let saved = db.setting(&settings_key(id)).ok().flatten();
            if let Some(Ok(serde_json::Value::Object(saved))) = saved.map(|s| serde_json::from_str(&s)) {
                settings.extend(saved);
            }
            log::info!("plugin {id}: {} ({})", installed.manifest.name, if enabled { "on" } else { "off" });
            self.plugins.push(Plugin {
                installed,
                enabled,
                settings,
                status: None,
                choices: HashMap::new(),
                process: None,
                generation: 0,
                restart_at: enabled.then(Instant::now),
                failures: 0,
            });
        }
    }

    pub fn plugins(&self) -> &[Plugin] {
        &self.plugins
    }

    /// Whether any running plugin wants screen colours.
    pub fn wants_light(&self) -> bool {
        self.plugins.iter().any(|p| p.running() && p.subscribes("light"))
    }

    /// Sends light to every running `light` subscriber; `make` gives the colours for a
    /// number of zones.
    pub fn send_light(&self, mut make: impl FnMut(usize) -> Light) {
        let mut made: Vec<(usize, Light)> = Vec::new();
        for p in self.plugins.iter().filter(|p| p.subscribes("light")) {
            let Some(process) = &p.process else { continue };
            let zones = p.installed.manifest.light.zones.max(1);
            let light = match made.iter().find(|(z, _)| *z == zones) {
                Some((_, light)) => light.clone(),
                None => {
                    let light = make(zones);
                    made.push((zones, light.clone()));
                    light
                }
            };
            process.outbox.set_light(light);
        }
    }

    pub fn send_playback(&self, playback: &Playback) {
        for p in self.plugins.iter().filter(|p| p.subscribes("playback")) {
            if let Some(process) = &p.process {
                process.outbox.send(HostMessage::Playback(playback.clone()));
            }
        }
    }

    pub fn set_enabled(&mut self, i: usize, on: bool, db: &Db) {
        let Some(p) = self.plugins.get_mut(i) else { return };
        if let Err(e) = db.set_setting(&enabled_key(&p.installed.id), if on { "true" } else { "false" }) {
            log::warn!("saving plugin setting: {e:#}");
        }
        p.enabled = on;
        p.failures = 0;
        if on {
            p.restart_at = Some(Instant::now());
        } else {
            p.restart_at = None;
            p.status = None;
            if let Some(process) = p.process.take() {
                process.stop(STOP_GRACE);
            }
        }
    }

    pub fn set_setting(&mut self, i: usize, key: &str, value: serde_json::Value, db: &Db) {
        let Some(p) = self.plugins.get_mut(i) else { return };
        p.settings.insert(key.to_owned(), value);
        save_settings(p, db);
        if let Some(process) = &p.process {
            process.outbox.send(HostMessage::Settings { settings: p.settings.clone() });
        }
    }

    /// A `button` setting was pressed.
    pub fn action(&mut self, i: usize, key: &str) {
        if let Some(process) = self.plugins.get(i).and_then(|p| p.process.as_ref()) {
            process.outbox.send(HostMessage::Action { id: key.to_owned() });
        }
    }

    /// Messages plugins asked to show on screen.
    pub fn take_toasts(&mut self) -> Vec<Toast> {
        std::mem::take(&mut self.toasts)
    }

    /// Handles messages from plugins and (re)starts plugins that are due. Returns when
    /// to call again for a pending restart.
    pub fn poll(&mut self, db: &Db) -> Option<Duration> {
        while let Ok((i, generation, incoming)) = self.rx.try_recv() {
            let Some(p) = self.plugins.get_mut(i).filter(|p| p.generation == generation) else { continue };
            let id = &p.installed.id;
            match incoming {
                Incoming::Message(PluginMessage::Status { ok, text }) => p.status = Some((ok, text)),
                Incoming::Message(PluginMessage::Choices { key, options }) => {
                    p.choices.insert(key, options);
                }
                Incoming::Message(PluginMessage::Set { settings }) => {
                    p.settings.extend(settings);
                    save_settings(p, db);
                }
                Incoming::Message(PluginMessage::Toast { text, seconds, key }) => {
                    // Keys are per plugin, so plugins can't clear each other's toasts.
                    let key = key.map(|k| format!("plugin:{id}:{k}"));
                    let text = if text.is_empty() { text } else { format!("{}: {text}", p.installed.manifest.name) };
                    self.toasts.push(Toast { text, seconds, key });
                }
                Incoming::Message(PluginMessage::Log { level, text }) => {
                    let level = match level {
                        LogLevel::Error => log::Level::Error,
                        LogLevel::Warn => log::Level::Warn,
                        LogLevel::Info => log::Level::Info,
                        LogLevel::Debug => log::Level::Debug,
                    };
                    log::log!(level, "plugin {id}: {text}");
                }
                Incoming::Message(PluginMessage::Unknown) => {}
                Incoming::Exited => {
                    let Some(mut process) = p.process.take() else { continue };
                    let code = process.child.wait().ok().and_then(|s| s.code());
                    log::warn!("plugin {id} exited (code {code:?})");
                    if process.started.elapsed() > HEALTHY_AFTER {
                        p.failures = 0;
                    }
                    p.failures += 1;
                    let wait = Duration::from_secs(1 << p.failures.min(6)).min(Duration::from_secs(60));
                    p.status = Some((false, format!("Stopped unexpectedly; restarting in {}s", wait.as_secs())));
                    p.restart_at = p.enabled.then(|| Instant::now() + wait);
                }
            }
        }
        let now = Instant::now();
        for i in 0..self.plugins.len() {
            let p = &self.plugins[i];
            if p.enabled && p.process.is_none() && p.restart_at.is_some_and(|t| t <= now) {
                self.start(i);
            }
        }
        self.plugins.iter().filter_map(|p| p.restart_at.filter(|_| p.process.is_none())).min().map(|t| t.saturating_duration_since(now))
    }

    fn start(&mut self, i: usize) {
        let (tx, waker) = (self.tx.clone(), Arc::clone(&self.waker));
        let p = &mut self.plugins[i];
        p.restart_at = None;
        p.generation += 1;
        match spawn(i, p.generation, &p.installed, &p.settings, tx, waker) {
            Ok(process) => {
                log::info!("plugin {}: started", p.installed.id);
                p.process = Some(process);
            }
            Err(e) => {
                log::warn!("plugin {}: {e:#}", p.installed.id);
                p.status = Some((false, format!("Couldn't start: {e:#}")));
                p.failures += 1;
                p.restart_at = Some(Instant::now() + Duration::from_secs(60));
            }
        }
    }

    /// Asks every plugin to exit and waits (up to two seconds) so they can turn things
    /// off. Called when Flex quits.
    pub fn shutdown(&mut self) {
        let mut children = Vec::new();
        for p in &mut self.plugins {
            p.enabled = false;
            if let Some(process) = p.process.take() {
                process.outbox.send(HostMessage::Shutdown);
                process.outbox.close();
                children.push(process.child);
            }
        }
        let deadline = Instant::now() + STOP_GRACE;
        while !children.is_empty() && Instant::now() < deadline {
            children.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
            std::thread::sleep(Duration::from_millis(20));
        }
        for mut child in children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn save_settings(p: &Plugin, db: &Db) {
    let json = serde_json::Value::Object(p.settings.clone()).to_string();
    if let Err(e) = db.set_setting(&settings_key(&p.installed.id), &json) {
        log::warn!("saving plugin settings: {e:#}");
    }
}

fn spawn(
    i: usize,
    generation: u64,
    plugin: &Installed,
    settings: &Settings,
    tx: Sender<(usize, u64, Incoming)>,
    waker: Arc<dyn Fn() + Send + Sync>,
) -> Result<Process> {
    let path = program(plugin);
    let mut cmd = Command::new(&path);
    cmd.args(&plugin.manifest.args)
        .envs(&plugin.manifest.env)
        .current_dir(&plugin.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().with_context(|| format!("running {}", path.display()))?;
    let (Some(mut stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        anyhow::bail!("no pipes to the plugin");
    };
    let id = plugin.id.clone();

    let outbox = Arc::new(Outbox::default());
    outbox.send(HostMessage::Hello { api: flex_plugin::API, flex_version: env!("CARGO_PKG_VERSION").to_owned(), settings: settings.clone() });
    let writer = Arc::clone(&outbox);
    std::thread::Builder::new().name(format!("plugin-{id}-in")).spawn(move || {
        while let Some(msg) = writer.next() {
            let Ok(mut line) = serde_json::to_vec(&msg) else { continue };
            line.push(b'\n');
            if stdin.write_all(&line).and_then(|()| stdin.flush()).is_err() {
                break;
            }
        }
    })?;

    let log_id = id.clone();
    std::thread::Builder::new().name(format!("plugin-{id}-err")).spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            log::info!("plugin {log_id}: {line}");
        }
    })?;

    std::thread::Builder::new().name(format!("plugin-{id}-out")).spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<PluginMessage>(&line) {
                Ok(msg) => {
                    let _ = tx.send((i, generation, Incoming::Message(msg)));
                    waker();
                }
                Err(_) => log::info!("plugin {id}: {line}"),
            }
        }
        let _ = tx.send((i, generation, Incoming::Exited));
        waker();
    })?;

    Ok(Process { child, outbox, started: Instant::now() })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
        name = "Test lights"
        command = "run-me"
        subscribe = ["light"]
        [light]
        zones = 3
        [[settings]]
        key = "max"
        label = "Maximum"
        type = "number"
        default = 80
        [[settings]]
        key = "find"
        label = "Find"
        type = "button"
    "#;

    #[test]
    fn manifests_parse_with_defaults() {
        let m: Manifest = toml::from_str(MANIFEST).unwrap();
        assert!(m.subscribes("light") && !m.subscribes("playback"));
        assert_eq!(m.light.zones, 3);
        assert_eq!(m.settings[0].kind, SettingKind::Number);
        assert_eq!(m.settings[0].default, Some(serde_json::json!(80)));
        let m: Manifest = toml::from_str("name = \"x\"\ncommand = \"y\"").unwrap();
        assert_eq!(m.light.zones, 1);
        assert!(m.settings.is_empty());
    }

    #[test]
    fn programs_come_from_the_folder_or_the_path() {
        let dir = std::env::temp_dir().join(format!("flex-plugin-program-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = if cfg!(windows) { "here.exe" } else { "here" };
        std::fs::write(dir.join(exe), "").unwrap();
        let plugin = |command: &str| Installed {
            id: "p".into(),
            dir: dir.clone(),
            manifest: toml::from_str(&format!("name = \"p\"\ncommand = {command:?}")).unwrap(),
        };
        assert_eq!(program(&plugin("here")), dir.join(exe));
        assert_eq!(program(&plugin("python3")), PathBuf::from("python3"));
        let tool = if cfg!(windows) { "bin/tool.exe" } else { "bin/tool" };
        assert_eq!(program(&plugin("bin/tool")), dir.join(tool));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn later_folders_win() {
        let root = std::env::temp_dir().join(format!("flex-plugins-{}", std::process::id()));
        for (dir, name) in [("a", "Bundled"), ("b", "Installed")] {
            let p = root.join(dir).join("lights");
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("plugin.toml"), format!("name = \"{name}\"\ncommand = \"x\"")).unwrap();
        }
        std::fs::create_dir_all(root.join("a/broken")).unwrap();
        std::fs::write(root.join("a/broken/plugin.toml"), "not toml [").unwrap();
        let found = discover(&[root.join("a"), root.join("b"), root.join("missing")]);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manifest.name, "Installed");
    }

    #[test]
    fn outbox_keeps_only_the_newest_light() {
        let outbox = Outbox::default();
        let light = |b| Light { zones: vec![[255, 0, 0]], brightness: b };
        outbox.set_light(light(0.1));
        outbox.send(HostMessage::Action { id: "a".into() });
        outbox.set_light(light(0.2));
        assert_eq!(outbox.next(), Some(HostMessage::Action { id: "a".into() }));
        assert_eq!(outbox.next(), Some(HostMessage::Light(light(0.2))));
        outbox.close();
        assert_eq!(outbox.next(), None);
    }

    /// Not a real test: the plugin end of `runs_a_plugin_process`, which starts this test
    /// binary again with `FLEX_FAKE_PLUGIN` set.
    #[test]
    fn fake_plugin() {
        if std::env::var_os("FLEX_FAKE_PLUGIN").is_none() {
            return;
        }
        struct Echo;
        impl flex_plugin::Plugin for Echo {
            fn hello(&mut self, host: &flex_plugin::Host, settings: &Settings) {
                host.status(true, format!("max {}", settings["max"]));
            }
            fn light(&mut self, host: &flex_plugin::Host, light: &Light) {
                host.status(true, format!("{} zones at {}", light.zones.len(), light.brightness));
            }
            fn action(&mut self, host: &flex_plugin::Host, id: &str) {
                let mut s = Settings::new();
                s.insert("found".into(), serde_json::json!(id));
                host.set(s);
                host.choices("device", vec![Choice { id: "1".into(), label: "Strip".into() }]);
            }
        }
        flex_plugin::run(&mut Echo);
        std::process::exit(0);
    }

    #[test]
    fn runs_a_plugin_process() {
        let root = std::env::temp_dir().join(format!("flex-plugin-run-{}", std::process::id()));
        let dir = root.join("echo");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = std::env::current_exe().unwrap();
        let manifest = format!(
            "name = \"Echo\"\ncommand = {exe:?}\nargs = [\"--exact\", \"plugins::tests::fake_plugin\", \"--nocapture\"]\n\
             subscribe = [\"light\"]\nenv = {{ FLEX_FAKE_PLUGIN = \"1\" }}\n[light]\nzones = 2\n\
             [[settings]]\nkey = \"max\"\nlabel = \"Max\"\ntype = \"number\"\ndefault = 80\n",
            exe = exe.display().to_string()
        );
        std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
        let db = Db::open_in_memory().unwrap();
        let mut host = Host::new(std::slice::from_ref(&root), &db, || {});
        assert!(!host.plugins()[0].running(), "off until switched on");
        host.set_enabled(0, true, &db);

        let wait_for = |host: &mut Host, what: &str, done: &dyn Fn(&Plugin) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !done(&host.plugins()[0]) {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                host.poll(&db);
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait_for(&mut host, "hello", &|p| p.status.as_ref().is_some_and(|s| s.1 == "max 80"));
        assert!(host.wants_light());
        host.send_light(|zones| Light { zones: vec![[1, 2, 3]; zones], brightness: 0.5 });
        wait_for(&mut host, "light", &|p| p.status.as_ref().is_some_and(|s| s.1 == "2 zones at 0.5"));
        host.action(0, "scan");
        wait_for(&mut host, "set", &|p| p.settings.get("found").is_some() && p.choices.contains_key("device"));
        assert!(db.setting("plugin.echo.settings").unwrap().unwrap().contains("\"found\":\"scan\""));

        host.set_enabled(0, false, &db);
        assert!(!host.plugins()[0].running() && !host.wants_light());
        // Settings survive a restart of Flex.
        let host = Host::new(std::slice::from_ref(&root), &db, || {});
        assert!(!host.plugins()[0].enabled);
        assert_eq!(host.plugins()[0].settings["found"], serde_json::json!("scan"));
        drop(host);
        let _ = std::fs::remove_dir_all(&root);
    }
}
