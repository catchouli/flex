//! Flex plugin for Lotus Lamp X (ELK-BLEDOM / MELK) Bluetooth LED strips, via the
//! `lotus-lamp` crate. The strip shows one colour at a time, so it asks for one zone.
//!
//! Keeps one connection open (connecting takes about a second; the strip's own CLI is
//! slow because it reconnects for every command), reconnects when it drops, and sends at
//! most ~25 colours a second, always the newest: faster than ~30 a second the Bluetooth
//! queue backs up.
//!
//! `FLEX_LOTUS_LAMP_TRACE=1` (e.g. under `env` in plugin.toml) logs every colour sent.

use std::time::Duration;

use flex_plugin::serde_json::{Value, json};
use flex_plugin::{Choice, Host, Light, Plugin, Settings};
use lotus_lamp::LedStrip;
use tokio::sync::watch;

const MIN_GAP: Duration = Duration::from_millis(40);
const SCAN: Duration = Duration::from_secs(10);
const FIND: Duration = Duration::from_secs(5);

#[derive(Clone, PartialEq)]
struct Config {
    /// Platform id of the strip; empty = the first one found.
    device: String,
    /// The strip's own brightness, 10–100.
    max_brightness: u8,
    off_on_exit: bool,
}

impl Config {
    fn from(settings: &Settings) -> Self {
        Self {
            device: settings.get("device").and_then(Value::as_str).unwrap_or_default().to_owned(),
            max_brightness: settings.get("max_brightness").and_then(Value::as_u64).map_or(100, |b| b.clamp(1, 100) as u8),
            off_on_exit: settings.get("off_on_exit").and_then(Value::as_bool).unwrap_or(true),
        }
    }
}

struct Lamp {
    rt: tokio::runtime::Runtime,
    config: watch::Sender<Option<Config>>,
    light: watch::Sender<Option<Light>>,
    stop: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Plugin for Lamp {
    fn hello(&mut self, host: &Host, settings: &Settings) {
        let config = Config::from(settings);
        let mut options = vec![Choice { id: String::new(), label: "The first one found".into() }];
        if !config.device.is_empty() {
            options.push(Choice { id: config.device.clone(), label: "The strip used last time".into() });
        }
        host.choices("device", options);
        self.config.send_replace(Some(config));
        let (host, config, light, stop) = (host.clone(), self.config.subscribe(), self.light.subscribe(), self.stop.subscribe());
        self.task = Some(self.rt.spawn(drive(host, config, light, stop)));
    }

    fn settings(&mut self, _host: &Host, settings: &Settings) {
        self.config.send_replace(Some(Config::from(settings)));
    }

    fn light(&mut self, _host: &Host, light: &Light) {
        self.light.send_replace(Some(light.clone()));
    }

    fn action(&mut self, host: &Host, id: &str) {
        if id != "find" {
            return;
        }
        let (host, current) = (host.clone(), self.config.borrow().as_ref().map(|c| c.device.clone()).unwrap_or_default());
        self.rt.spawn(async move {
            host.toast("Looking for LED strips…");
            match LedStrip::discover(FIND).await {
                Ok(found) => {
                    let mut options = vec![Choice { id: String::new(), label: "The first one found".into() }];
                    for s in &found {
                        let rssi = s.rssi.map_or(String::new(), |r| format!(", signal {r} dBm"));
                        options.push(Choice { id: s.id.clone(), label: format!("{} ({}{rssi})", s.name, short(&s.id)) });
                    }
                    // A connected strip stops advertising, so it may not be in the list.
                    if !current.is_empty() && !found.iter().any(|s| s.matches(&current)) {
                        options.push(Choice { id: current.clone(), label: format!("Connected strip ({})", short(&current)) });
                    }
                    host.toast(match found.len() {
                        0 => "No other strips found (a connected strip doesn't show up)".to_owned(),
                        1 => "Found 1 strip".to_owned(),
                        n => format!("Found {n} strips"),
                    });
                    host.choices("device", options);
                }
                Err(e) => host.toast(format!("Couldn't look for strips: {e}")),
            }
        });
    }

    fn shutdown(&mut self, _host: &Host) {
        self.stop.send_replace(true);
        if let Some(task) = self.task.take() {
            let _ = self.rt.block_on(async { tokio::time::timeout(Duration::from_millis(1500), task).await });
        }
    }
}

/// The last few characters of a device id, to tell strips apart.
fn short(id: &str) -> &str {
    let start = id.char_indices().rev().nth(5).map_or(0, |(i, _)| i);
    &id[start..]
}

/// Connects, sends colours, reconnects; until told to stop.
async fn drive(
    host: Host,
    mut config: watch::Receiver<Option<Config>>,
    mut light: watch::Receiver<Option<Light>>,
    mut stop: watch::Receiver<bool>,
) {
    let mut retry = Duration::from_secs(2);
    while !*stop.borrow() {
        let Some(cfg) = config.borrow_and_update().clone() else { return };
        host.status(true, "Looking for the strip…");
        let strip = tokio::select! {
            r = connect(&cfg.device) => r,
            _ = stop.changed() => return,
            _ = config.changed() => continue,
        };
        let (strip, id) = match strip {
            Ok(found) => found,
            Err(e) => {
                host.status(false, format!("{e}; trying again in {}s", retry.as_secs()));
                tokio::select! {
                    _ = tokio::time::sleep(retry) => {}
                    _ = stop.changed() => return,
                    _ = config.changed() => {}
                }
                retry = (retry * 2).min(Duration::from_secs(30));
                continue;
            }
        };
        retry = Duration::from_secs(2);
        host.status(true, format!("Connected ({})", short(&id)));
        if cfg.device.is_empty() {
            // Reconnect straight to this one next time.
            let mut s = Settings::new();
            s.insert("device".into(), json!(id));
            host.set(s);
        }
        match run_connected(&strip, &id, &cfg, &mut config, &mut light, &mut stop).await {
            Ok(Exit::Stop) => {
                if cfg.off_on_exit {
                    let _ = strip.power(false).await;
                }
                let _ = strip.disconnect().await;
                return;
            }
            Ok(Exit::Reconfigure) => {
                let _ = strip.disconnect().await;
            }
            Err(e) => {
                host.status(false, format!("Lost the connection ({e}); reconnecting"));
                let _ = strip.disconnect().await;
            }
        }
    }
}

enum Exit {
    Stop,
    /// A different strip was chosen.
    Reconfigure,
}

/// Connects to strip `device` (a platform id), or the strongest one found if empty.
/// Returns it with its id.
async fn connect(device: &str) -> lotus_lamp::Result<(LedStrip, String)> {
    if !device.is_empty() {
        return Ok((LedStrip::connect_id(device, SCAN).await?, device.to_owned()));
    }
    let deadline = tokio::time::Instant::now() + SCAN;
    loop {
        let found = LedStrip::discover(Duration::from_secs(2)).await?;
        if let Some(strip) = found.into_iter().max_by_key(|s| s.rssi.unwrap_or(i16::MIN)) {
            let id = strip.id.clone();
            return Ok((strip.connect().await?, id));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(lotus_lamp::Error::NotFound);
        }
    }
}

async fn run_connected(
    strip: &LedStrip,
    id: &str,
    cfg: &Config,
    config: &mut watch::Receiver<Option<Config>>,
    light: &mut watch::Receiver<Option<Light>>,
    stop: &mut watch::Receiver<bool>,
) -> lotus_lamp::Result<Exit> {
    let trace = std::env::var_os("FLEX_LOTUS_LAMP_TRACE").is_some();
    strip.power(true).await?;
    let mut brightness = cfg.max_brightness;
    strip.set_brightness(brightness).await?;
    light.mark_changed();
    loop {
        tokio::select! {
            r = light.changed() => if r.is_err() { return Ok(Exit::Stop) },
            _ = stop.changed() => return Ok(Exit::Stop),
            r = config.changed() => {
                if r.is_err() {
                    return Ok(Exit::Stop);
                }
                let Some(new) = config.borrow_and_update().clone() else { continue };
                if new.device != cfg.device && !(cfg.device.is_empty() && new.device == id) {
                    return Ok(Exit::Reconfigure);
                }
                if new.max_brightness != brightness {
                    brightness = new.max_brightness;
                    strip.set_brightness(brightness).await?;
                }
                continue;
            }
        }
        let Some(l) = light.borrow_and_update().clone() else { continue };
        let [r, g, b] = l.zones.first().copied().unwrap_or_default().map(|c| (f32::from(c) * l.brightness.clamp(0.0, 1.0)).round() as u8);
        strip.set_color(r, g, b).await?;
        if trace {
            eprintln!("colour {r:02x}{g:02x}{b:02x}");
        }
        tokio::time::sleep(MIN_GAP).await;
    }
}

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
    let mut lamp = Lamp {
        rt,
        config: watch::Sender::new(None),
        light: watch::Sender::new(None),
        stop: watch::Sender::new(false),
        task: None,
    };
    flex_plugin::run(&mut lamp);
}
