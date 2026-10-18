//! Settings → Themes: pick how Flex looks. Built-in themes, plus custom ones from the
//! themes folder (see THEMES.md).

use eframe::egui::{Rect, Ui};

use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::theme::{self, Theme};
use crate::widgets::{self, Icon, ListInput, ListState, Row};

pub const THEME_SETTING: &str = "theme";

pub struct Themes {
    themes: Vec<Theme>,
    list: ListState,
}

impl Default for Themes {
    fn default() -> Self {
        let themes = theme::all();
        let current = theme::current().id.clone();
        let mut list = ListState::default();
        list.select(themes.iter().position(|t| t.id == current).unwrap_or(0));
        Self { themes, list }
    }
}

impl Screen for Themes {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let n = self.themes.len();
        match self.list.handle(press, n + 2) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) if i < n => {
                let chosen = self.themes[i].clone();
                sh.set_setting(THEME_SETTING, &chosen.id);
                theme::apply(&sh.ctx, chosen);
                Nav::None
            }
            ListInput::Activate(i) if i == n => {
                let before = self.themes.len();
                self.themes = theme::all();
                let found = self.themes.len().saturating_sub(before);
                sh.toast(match found {
                    0 => "No new themes".to_owned(),
                    1 => "Found a new theme".to_owned(),
                    n => format!("Found {n} new themes"),
                });
                Nav::None
            }
            ListInput::Activate(_) => {
                let dir = theme::themes_dir();
                let _ = std::fs::create_dir_all(&dir);
                crate::logging::open_folder(&dir);
                Nav::None
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, _sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let body = widgets::header(ui.painter(), rect, "Themes", "How Flex looks. Add your own in the themes folder.", ui.style());
        let current = theme::current().id.clone();
        let mut rows: Vec<Row> = self
            .themes
            .iter()
            .map(|t| {
                let name = if t.id == current { format!("{}  ✓", t.name) } else { t.name.clone() };
                let detail = if t.dir.is_some() { format!("{} · custom", t.description) } else { t.description.clone() };
                Row::new(Icon::None, name).detail(detail)
            })
            .collect();
        rows.push(Row::new(Icon::Refresh, "Look for new themes"));
        rows.push(Row::new(Icon::Folder, "Open themes folder").detail(theme::themes_dir().display().to_string()));
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
