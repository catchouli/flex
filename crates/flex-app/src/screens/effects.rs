//! Settings → Picture → Video effects: pick an effect (or none) and change its settings.

use eframe::egui::{Rect, Ui};
use flex_core::effects::Effect;
use flex_core::plugins::SettingSpec;

use super::plugins::{describe, next_value};
use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::widgets::{self, Icon, ListInput, ListState, Row};

#[derive(Default)]
pub struct VideoEffects {
    list: ListState,
}

/// The chosen effect and its settings (the rows under "Effect").
fn chosen(sh: &Shared) -> Option<(Effect, Vec<SettingSpec>)> {
    let e = sh.effects.chosen()?.clone();
    let specs = e.settings().cloned().collect();
    Some((e, specs))
}

impl Screen for VideoEffects {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let chosen = chosen(sh);
        let settings = chosen.as_ref().map_or(0, |(_, s)| s.len());
        match self.list.handle(press, 3 + settings) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(0) => {
                let (effects, db) = sh.effects_mut();
                effects.cycle(db);
                Nav::None
            }
            ListInput::Activate(i) if i <= settings => {
                let Some((effect, specs)) = chosen else { return Nav::None };
                let spec = &specs[i - 1];
                let current = sh.effects.value(&effect.id, &spec.key).cloned();
                if let Some(value) = next_value(&spec.options, spec, current.as_ref()) {
                    let (effects, db) = sh.effects_mut();
                    effects.set_setting(&effect.id, &spec.key, value, db);
                }
                Nav::None
            }
            ListInput::Activate(i) if i == settings + 1 => {
                let before = sh.effects.list().len();
                sh.refresh_effects();
                let found = sh.effects.list().len();
                sh.toast(match found {
                    0 => "No effects found".to_owned(),
                    n if n > before => format!("Found {} new", n - before),
                    n => format!("{n} effects, nothing new"),
                });
                Nav::None
            }
            ListInput::Activate(_) => {
                let dir = flex_core::effects::user_effect_dir();
                let _ = std::fs::create_dir_all(&dir);
                crate::logging::open_folder(&dir);
                Nav::None
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let subtitle = "Shaders drawn over the video. Hold OK while watching to switch.";
        let body = widgets::header(ui.painter(), rect, "Video effects", subtitle, ui.style());
        let chosen = chosen(sh);
        let effect_row = match &chosen {
            None if sh.effects.list().is_empty() => Row::new(Icon::None, "Effect: none found").detail("Put .frag files in the effects folder"),
            None => Row::new(Icon::None, "Effect: off").detail(format!("{} to choose from · OK to change", sh.effects.list().len())),
            Some((e, _)) => {
                let detail = match (&sh.effects.status, &e.problem) {
                    (Some((false, error)), _) => format!("⚠ {error}"),
                    (_, Some(problem)) => format!("⚠ {problem}"),
                    _ if !e.manifest.description.is_empty() => e.manifest.description.clone(),
                    _ => "OK to change".to_owned(),
                };
                Row::new(Icon::None, format!("Effect: {}", e.manifest.name)).detail(detail)
            }
        };
        let mut rows = vec![effect_row];
        if let Some((e, specs)) = &chosen {
            for spec in specs {
                let detail = if spec.detail.is_empty() { "OK to change".to_owned() } else { spec.detail.clone() };
                rows.push(Row::new(Icon::None, describe(&spec.options, spec, sh.effects.value(&e.id, &spec.key))).detail(detail));
            }
        }
        rows.push(Row::new(Icon::Refresh, "Look for new effects"));
        let dir = flex_core::effects::user_effect_dir();
        rows.push(Row::new(Icon::Folder, "Open effects folder").detail(dir.display().to_string()));
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
