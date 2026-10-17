//! Flex plugins: programs Flex starts and talks to over stdin/stdout, one JSON message
//! per line. Flex writes [`HostMessage`]s to the plugin's stdin and reads
//! [`PluginMessage`]s from its stdout; anything the plugin prints to stderr goes to
//! Flex's log.
//!
//! A plugin is a folder with a `plugin.toml` next to its program:
//!
//! ```toml
//! name = "RGB light strip"
//! description = "Backlight from the colours on screen"
//! command = "flex-rgb-light"       # in this folder; ".exe" is added on Windows
//! subscribe = ["light"]            # which messages it wants: light, playback
//!
//! [light]
//! zones = 1                        # colours wanted per update (1 = whole strip)
//!
//! [[settings]]
//! key = "max_brightness"
//! label = "Maximum brightness"
//! type = "number"                  # bool, choice, number, text or button
//! min = 10
//! max = 100
//! step = 10
//! unit = "%"
//! default = 100
//! ```
//!
//! Flex only starts plugins someone has switched on, and only works out screen colours
//! while a running plugin subscribes to `light`. Both sides ignore message types (and
//! fields) they don't know, so plugins keep working as the protocol grows; [`API`] only
//! changes for breaking changes.
//!
//! In Rust, implement [`Plugin`] and call [`run`]. Other languages just read and write
//! the JSON lines below.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::sync::{Arc, Condvar, Mutex};

use serde::{Deserialize, Serialize};
/// For [`Settings`] values.
pub use serde_json;

/// Protocol version, sent in [`HostMessage::Hello`].
pub const API: u32 = 1;

/// A plugin's settings, by key: what the user chose in Flex, or the defaults from
/// `plugin.toml`.
pub type Settings = serde_json::Map<String, serde_json::Value>;

/// Flex to plugin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMessage {
    /// First message after starting.
    Hello { api: u32, flex_version: String, settings: Settings },
    /// The user changed a setting (all settings are sent).
    Settings { settings: Settings },
    /// Colours from the screen, for plugins subscribed to `light`.
    Light(Light),
    /// Playback started, paused, resumed or stopped, for plugins subscribed to `playback`.
    Playback(Playback),
    /// A `button` setting was pressed; `id` is its key.
    Action { id: String },
    /// Flex is quitting or the plugin was switched off: clean up and exit. Flex waits
    /// up to two seconds before killing the process.
    Shutdown,
    #[serde(other)]
    Unknown,
}

/// What a light should show now. Already smoothed and adjusted to the user's ambient
/// light settings, so plugins just pass it on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Light {
    /// As many colours as the plugin's `[light] zones`, at full brightness (the
    /// brightest channel is 255 unless the colour is black), in linear light: the
    /// amount of light per channel, which is what LEDs dimmed by PWM want. For a device
    /// that expects screen (sRGB) values, convert each channel (roughly `c^(1/2.2)`). Zones run clockwise from
    /// the bottom left corner of the screen: up the left edge, along the top, down the
    /// right edge and back along the bottom. With one zone it is the whole picture.
    pub zones: Vec<[u8; 3]>,
    /// How bright to make it, 0.0 (off) to 1.0, also linear (0.5 = half the light).
    pub brightness: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Playback {
    pub state: PlaybackState,
    /// What's playing, e.g. "Frieren · S1E3". When stopped: what was playing.
    #[serde(default)]
    pub title: String,
    /// "video" or "music".
    #[serde(default)]
    pub kind: String,
    /// Where playback is, in seconds (when stopped: where it stopped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// What it is, for a film or an episode Flex has matched online (not for unmatched
    /// files, music or YouTube).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<Media>,
}

/// A film or an episode, as matched online.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Media {
    /// "movie" or "episode".
    pub kind: String,
    /// The film's or show's title, and its year.
    pub title: String,
    #[serde(default)]
    pub year: Option<i32>,
    /// TMDB id of the film or show, when matched there (anime is matched on AniList).
    #[serde(default)]
    pub tmdb: Option<u64>,
    #[serde(default)]
    pub anilist: Option<u64>,
    /// Episodes: season and episode number.
    #[serde(default)]
    pub season: Option<i32>,
    #[serde(default)]
    pub episode: Option<i32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    #[default]
    Playing,
    Paused,
    Stopped,
}

/// Plugin to Flex.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginMessage {
    /// Shown under the plugin's name in Settings, e.g. "Connected to MELK-OA10".
    /// `ok: false` shows it as a problem.
    Status { ok: bool, text: String },
    /// Replaces the options of a `choice` setting, e.g. devices found by a scan.
    Choices { key: String, options: Vec<Choice> },
    /// Changes settings and saves them, e.g. remembering the device it connected to.
    Set { settings: Settings },
    /// A short message shown on screen for a few seconds.
    Toast { text: String },
    /// A line for Flex's log.
    Log { level: LogLevel, text: String },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

/// Sends messages to Flex. Cheap to clone and usable from any thread.
#[derive(Clone)]
pub struct Host {
    out: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl Host {
    pub fn stdout() -> Self {
        Self::new(std::io::stdout())
    }

    pub fn new(out: impl Write + Send + 'static) -> Self {
        Self { out: Arc::new(Mutex::new(Box::new(out))) }
    }

    /// Errors (Flex has gone away) are ignored: the plugin will get EOF on stdin.
    pub fn send(&self, msg: &PluginMessage) {
        let Ok(mut line) = serde_json::to_vec(msg) else { return };
        line.push(b'\n');
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = out.write_all(&line).and_then(|()| out.flush());
    }

    pub fn status(&self, ok: bool, text: impl Into<String>) {
        self.send(&PluginMessage::Status { ok, text: text.into() });
    }

    pub fn choices(&self, key: impl Into<String>, options: Vec<Choice>) {
        self.send(&PluginMessage::Choices { key: key.into(), options });
    }

    pub fn set(&self, settings: Settings) {
        self.send(&PluginMessage::Set { settings });
    }

    pub fn toast(&self, text: impl Into<String>) {
        self.send(&PluginMessage::Toast { text: text.into() });
    }

    pub fn log(&self, level: LogLevel, text: impl Into<String>) {
        self.send(&PluginMessage::Log { level, text: text.into() });
    }
}

/// A plugin written in Rust. Every method has a default that does nothing.
///
/// Methods are called one at a time on one thread. Light updates that arrive while one
/// is being handled are merged, so a slow device only ever gets the newest colours.
pub trait Plugin {
    fn hello(&mut self, _host: &Host, _settings: &Settings) {}
    fn settings(&mut self, _host: &Host, _settings: &Settings) {}
    fn light(&mut self, _host: &Host, _light: &Light) {}
    fn playback(&mut self, _host: &Host, _playback: &Playback) {}
    fn action(&mut self, _host: &Host, _id: &str) {}
    /// Called before [`run`] returns, on [`HostMessage::Shutdown`] or when Flex has gone.
    fn shutdown(&mut self, _host: &Host) {}
}

/// Reads messages from stdin and hands them to `plugin` until Flex says to shut down or
/// closes stdin.
pub fn run(plugin: &mut impl Plugin) {
    run_with(plugin, std::io::BufReader::new(std::io::stdin()), &Host::stdout());
}

/// [`run`] with any input and output (for tests).
pub fn run_with(plugin: &mut impl Plugin, input: impl BufRead + Send + 'static, host: &Host) {
    let inbox = Arc::new(Inbox::default());
    let reader = Arc::clone(&inbox);
    std::thread::spawn(move || {
        for line in input.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<HostMessage>(&line) {
                Ok(HostMessage::Unknown) => {}
                Ok(msg) => reader.push(msg),
                Err(e) => eprintln!("flex-plugin: ignoring bad message ({e}): {line}"),
            }
        }
        reader.close();
    });
    while let Some(msg) = inbox.pop() {
        match msg {
            HostMessage::Hello { settings, .. } => plugin.hello(host, &settings),
            HostMessage::Settings { settings } => plugin.settings(host, &settings),
            HostMessage::Light(light) => plugin.light(host, &light),
            HostMessage::Playback(playback) => plugin.playback(host, &playback),
            HostMessage::Action { id } => plugin.action(host, &id),
            HostMessage::Shutdown => break,
            HostMessage::Unknown => {}
        }
    }
    plugin.shutdown(host);
}

/// Incoming messages, with consecutive light updates merged into the newest.
#[derive(Default)]
struct Inbox {
    state: Mutex<(VecDeque<HostMessage>, bool)>,
    ready: Condvar,
}

impl Inbox {
    fn push(&self, msg: HostMessage) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let queue = &mut state.0;
        if matches!(msg, HostMessage::Light(_)) && matches!(queue.back(), Some(HostMessage::Light(_))) {
            queue.pop_back();
        }
        queue.push_back(msg);
        self.ready.notify_one();
    }

    fn close(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
        self.ready.notify_one();
    }

    /// The next message; `None` once input has ended and everything was handled.
    fn pop(&self) -> Option<HostMessage> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(msg) = state.0.pop_front() {
                return Some(msg);
            }
            if state.1 {
                return None;
            }
            state = self.ready.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_look_like_the_docs() {
        let light = HostMessage::Light(Light { zones: vec![[255, 140, 60]], brightness: 0.5 });
        assert_eq!(serde_json::to_string(&light).unwrap(), r#"{"type":"light","zones":[[255,140,60]],"brightness":0.5}"#);
        let status: PluginMessage = serde_json::from_str(r#"{"type":"status","ok":true,"text":"hi"}"#).unwrap();
        assert_eq!(status, PluginMessage::Status { ok: true, text: "hi".into() });
    }

    #[test]
    fn playback_details_are_optional() {
        // As older versions of Flex sent it.
        let old: HostMessage = serde_json::from_str(r#"{"type":"playback","state":"paused","title":"T","kind":"video"}"#).unwrap();
        let HostMessage::Playback(p) = old else { panic!("not playback") };
        assert_eq!((p.state, p.position, p.media), (PlaybackState::Paused, None, None));
        let full = HostMessage::Playback(Playback {
            state: PlaybackState::Stopped,
            title: "Frieren · S1E3".into(),
            kind: "video".into(),
            position: Some(1300.0),
            duration: Some(1420.0),
            media: Some(Media { kind: "episode".into(), title: "Frieren".into(), year: Some(2023), anilist: Some(154587), season: Some(1), episode: Some(3), ..Media::default() }),
        });
        let json = serde_json::to_string(&full).unwrap();
        assert!(json.contains(r#""media":{"kind":"episode","title":"Frieren""#), "{json}");
        assert_eq!(serde_json::from_str::<HostMessage>(&json).unwrap(), full);
    }

    #[test]
    fn unknown_messages_and_fields_are_ignored() {
        let msg: HostMessage = serde_json::from_str(r#"{"type":"something_new","x":1}"#).unwrap();
        assert_eq!(msg, HostMessage::Unknown);
        let msg: HostMessage = serde_json::from_str(r#"{"type":"action","id":"scan","extra":true}"#).unwrap();
        assert_eq!(msg, HostMessage::Action { id: "scan".into() });
        let msg: PluginMessage = serde_json::from_str(r#"{"type":"later"}"#).unwrap();
        assert_eq!(msg, PluginMessage::Unknown);
    }

    #[test]
    fn queued_light_updates_merge() {
        let inbox = Inbox::default();
        let light = |b| HostMessage::Light(Light { zones: vec![[255, 0, 0]], brightness: b });
        inbox.push(light(0.1));
        inbox.push(light(0.2));
        inbox.push(HostMessage::Action { id: "a".into() });
        inbox.push(light(0.3));
        inbox.push(light(0.4));
        inbox.close();
        assert_eq!(inbox.pop(), Some(light(0.2)));
        assert_eq!(inbox.pop(), Some(HostMessage::Action { id: "a".into() }));
        assert_eq!(inbox.pop(), Some(light(0.4)));
        assert_eq!(inbox.pop(), None);
    }

    #[derive(Default)]
    struct Recorder(Vec<String>);

    impl Plugin for Recorder {
        fn hello(&mut self, host: &Host, settings: &Settings) {
            self.0.push(format!("hello {}", settings.len()));
            host.status(true, "ready");
        }
        fn action(&mut self, _host: &Host, id: &str) {
            self.0.push(format!("action {id}"));
        }
        fn shutdown(&mut self, _host: &Host) {
            self.0.push("shutdown".into());
        }
    }

    #[test]
    fn run_dispatches_until_shutdown() {
        let input = concat!(
            r#"{"type":"hello","api":1,"flex_version":"0","settings":{"a":1}}"#,
            "\nnot json\n",
            r#"{"type":"action","id":"scan"}"#,
            "\n",
            r#"{"type":"shutdown"}"#,
            "\n",
            r#"{"type":"action","id":"too late"}"#,
            "\n",
        );
        let out = Arc::new(Mutex::new(Vec::new()));
        struct Shared(Arc<Mutex<Vec<u8>>>);
        impl Write for Shared {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().write(buf)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut plugin = Recorder::default();
        run_with(&mut plugin, std::io::Cursor::new(input), &Host::new(Shared(Arc::clone(&out))));
        assert_eq!(plugin.0, ["hello 1", "action scan", "shutdown"]);
        let out = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        assert_eq!(out, "{\"type\":\"status\",\"ok\":true,\"text\":\"ready\"}\n");
    }
}
