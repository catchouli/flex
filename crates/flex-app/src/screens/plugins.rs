//! Settings → Plugins: switch plugins on and off, change their settings, and (with a
//! light plugin on) Settings → Ambient light.

use eframe::egui::{Rect, Ui};
use flex_core::plugins::{Plugin, SettingKind, SettingSpec};
use flex_plugin::serde_json::{Value, json};

use super::{Nav, Screen, text_entry::TextEntry};
use crate::ambient::CHOICES;
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::widgets::{self, Icon, ListInput, ListState, Row};

/// Whether a switched-on plugin wants screen colours (so Ambient light is worth showing).
fn light_plugin_on(sh: &Shared) -> bool {
    sh.plugins.plugins().iter().any(|p| p.enabled && p.subscribes("light"))
}

fn status_line(p: &Plugin) -> String {
    match (&p.status, p.enabled) {
        (_, false) => "Off".into(),
        (Some((true, text)), _) => format!("On · {text}"),
        (Some((false, text)), _) => format!("On · ⚠ {text}"),
        (None, _) if p.running() => "On · starting…".into(),
        (None, _) => "On".into(),
    }
}

#[derive(Default)]
pub struct Plugins {
    list: ListState,
}

impl Plugins {
    /// Rows after the plugins: Ambient light (when there's a light plugin), then look for
    /// new plugins and open the folder.
    fn extra_rows(sh: &Shared) -> usize {
        2 + usize::from(light_plugin_on(sh))
    }
}

impl Screen for Plugins {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let n = sh.plugins.plugins().len();
        let ambient = light_plugin_on(sh);
        match self.list.handle(press, n + Self::extra_rows(sh)) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) if i < n => Nav::push(PluginOptions::new(i)),
            ListInput::Activate(i) if ambient && i == n => Nav::push(AmbientLight::default()),
            ListInput::Activate(i) if i == n + Self::extra_rows(sh) - 2 => {
                let before = sh.plugins.plugins().len();
                sh.refresh_plugins();
                let found = sh.plugins.plugins().len().saturating_sub(before);
                sh.toast(match found {
                    0 => "No new plugins".to_owned(),
                    1 => "Found a new plugin".to_owned(),
                    n => format!("Found {n} new plugins"),
                });
                Nav::None
            }
            ListInput::Activate(_) => {
                let dir = flex_core::plugins::user_plugin_dir();
                let _ = std::fs::create_dir_all(&dir);
                crate::logging::open_folder(&dir);
                Nav::None
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let subtitle = if sh.plugins.plugins().is_empty() {
            "None installed: put a plugin's folder in the plugins folder, then look for new plugins."
        } else {
            "Add-ons that run alongside Flex. They only run while switched on."
        };
        let body = widgets::header(ui.painter(), rect, "Plugins", subtitle, ui.style());
        let mut rows: Vec<Row> = sh
            .plugins
            .plugins()
            .iter()
            .map(|p| Row::new(Icon::None, &p.installed.manifest.name).detail(status_line(p)).dim(!p.enabled))
            .collect();
        if light_plugin_on(sh) {
            rows.push(Row::new(Icon::None, "Ambient light…").detail("How screen colours are turned into light"));
        }
        rows.push(Row::new(Icon::Refresh, "Look for new plugins"));
        let dir = flex_core::plugins::user_plugin_dir();
        rows.push(Row::new(Icon::Folder, "Open plugins folder").detail(dir.display().to_string()));
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

/// One plugin: on/off, its status, and its own settings.
pub struct PluginOptions {
    index: usize,
    list: ListState,
}

impl PluginOptions {
    fn new(index: usize) -> Self {
        Self { index, list: ListState::default() }
    }
}

/// The value after `current` when OK is pressed on a setting.
fn next_value(p: &Plugin, spec: &SettingSpec, current: Option<&Value>) -> Option<Value> {
    match spec.kind {
        SettingKind::Bool => Some(json!(!current.and_then(Value::as_bool).unwrap_or(false))),
        SettingKind::Choice => {
            let options = p.options(spec);
            if options.is_empty() {
                return None;
            }
            let at = current.and_then(Value::as_str).and_then(|c| options.iter().position(|o| o.id == c));
            let next = at.map_or(0, |i| (i + 1) % options.len());
            Some(json!(options[next].id))
        }
        SettingKind::Number => {
            let (min, max) = (spec.min.unwrap_or(0.0), spec.max.unwrap_or(100.0));
            let step = spec.step.unwrap_or(((max - min) / 10.0).max(1.0));
            let value = current.and_then(Value::as_f64).unwrap_or(min) + step;
            let value = if value > max + step * 1e-6 { min } else { value.min(max) };
            Some(if value.fract() == 0.0 { json!(value as i64) } else { json!(value) })
        }
        SettingKind::Text | SettingKind::Button => None,
    }
}

fn describe(p: &Plugin, spec: &SettingSpec, value: Option<&Value>) -> String {
    let shown = match spec.kind {
        SettingKind::Bool => (if value.and_then(Value::as_bool).unwrap_or(false) { "on" } else { "off" }).to_owned(),
        SettingKind::Choice => {
            let id = value.and_then(Value::as_str).unwrap_or("");
            match p.options(spec).iter().find(|o| o.id == id) {
                Some(o) => o.label.clone(),
                None if id.is_empty() => "none".to_owned(),
                None => id.to_owned(),
            }
        }
        SettingKind::Number => {
            let n = value.and_then(Value::as_f64).unwrap_or(spec.min.unwrap_or(0.0));
            format!("{n}{}", spec.unit)
        }
        SettingKind::Text => match value.and_then(Value::as_str).filter(|s| !s.is_empty()) {
            Some(_) if spec.secret => "set".to_owned(),
            Some(text) => text.to_owned(),
            None => "not set".to_owned(),
        },
        SettingKind::Button => return spec.label.clone(),
    };
    format!("{}: {shown}", spec.label)
}

impl Screen for PluginOptions {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let Some(p) = sh.plugins.plugins().get(self.index) else { return Nav::Pop };
        let specs = p.installed.manifest.settings.clone();
        let enabled = p.enabled;
        match self.list.handle(press, 1 + specs.len()) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(0) => {
                let (plugins, db) = sh.plugins_mut();
                plugins.set_enabled(self.index, !enabled, db);
                Nav::None
            }
            ListInput::Activate(i) => {
                let spec = &specs[i - 1];
                let p = &sh.plugins.plugins()[self.index];
                let current = p.settings.get(&spec.key).cloned();
                match spec.kind {
                    SettingKind::Button if !enabled => {
                        sh.toast("Switch the plugin on first");
                        Nav::None
                    }
                    SettingKind::Button => {
                        sh.plugins.action(self.index, &spec.key);
                        Nav::None
                    }
                    SettingKind::Text => {
                        // A secret starts empty rather than showing what's saved.
                        let text = if spec.secret { String::new() } else { current.as_ref().and_then(Value::as_str).unwrap_or("").to_owned() };
                        Nav::push(TextEntry::plugin_setting(self.index, &spec.key, &spec.label, text, spec.secret))
                    }
                    _ => {
                        match next_value(p, spec, current.as_ref()) {
                            Some(value) => {
                                let (plugins, db) = sh.plugins_mut();
                                plugins.set_setting(self.index, &spec.key, value, db);
                            }
                            None => sh.toast("Nothing to choose from yet"),
                        }
                        Nav::None
                    }
                }
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(p) = sh.plugins.plugins().get(self.index) else { return Nav::Pop };
        let m = &p.installed.manifest;
        let subtitle = match (m.description.is_empty(), m.version.is_empty()) {
            (false, false) => format!("{} · version {}", m.description, m.version),
            (false, true) => m.description.clone(),
            (true, false) => format!("Version {}", m.version),
            (true, true) => String::new(),
        };
        let body = widgets::header(ui.painter(), rect, &m.name, &subtitle, ui.style());
        let mut rows = vec![Row::new(Icon::None, if p.enabled { "Switched on" } else { "Switched off" }).detail(status_line(p))];
        for spec in &m.settings {
            let icon = if spec.kind == SettingKind::Button { Icon::Refresh } else { Icon::None };
            let detail = match spec.kind {
                _ if !spec.detail.is_empty() => spec.detail.clone(),
                SettingKind::Text => "OK to type".to_owned(),
                SettingKind::Button => String::new(),
                _ => "OK to change".to_owned(),
            };
            rows.push(Row::new(icon, describe(p, spec, p.settings.get(&spec.key))).detail(detail).dim(!p.enabled));
        }
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

/// Settings → Ambient light: how colours are taken from the screen.
#[derive(Default)]
pub struct AmbientLight {
    list: ListState,
}

impl Screen for AmbientLight {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        match self.list.handle(press, CHOICES.len()) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) => {
                let next = (sh.ambient.values[i] + 1) % CHOICES[i].options.len();
                sh.set_ambient(i, next);
                Nav::None
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let subtitle = "For lights behind the TV. Press F2 while a video plays to see the colour.";
        let body = widgets::header(ui.painter(), rect, "Ambient light", subtitle, ui.style());
        let rows: Vec<Row> = CHOICES
            .iter()
            .zip(sh.ambient.values)
            .map(|(c, v)| Row::new(Icon::None, format!("{}: {}", c.label, c.options[v])).detail(c.detail))
            .collect();
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
