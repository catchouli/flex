//! A library's viewing history, newest first: what was watched and when (including history
//! imported from Plex). OK plays it again.

use chrono::{Local, TimeZone};
use eframe::egui::{Rect, Ui};
use flex_core::db::HistoryEntry;

use super::options::{OptionsMenu, Target};
use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::theme;
use crate::widgets::{self, Icon, ListInput, ListState, Row};

const MAX_ENTRIES: usize = 500;

pub struct HistoryView {
    library_id: i64,
    entries: Vec<HistoryEntry>,
    version: Option<u64>,
    list: ListState,
}

impl HistoryView {
    pub fn new(library_id: i64) -> Self {
        Self { library_id, entries: Vec::new(), version: None, list: ListState::default() }
    }

    fn refresh(&mut self, sh: &Shared) {
        let version = sh.lib(self.library_id).map(|l| l.version);
        if self.version != version {
            self.entries = sh.history(self.library_id, MAX_ENTRIES);
            self.version = version;
        }
    }
}

/// "Dorohedoro · S1E5 · Title", a film's title, or the file name if not organised.
fn label(sh: &Shared, library_id: i64, file_id: i64) -> String {
    sh.titles(library_id, file_id).map_or_else(String::new, |t| t.full())
}

fn when(unix: i64) -> String {
    Local.timestamp_opt(unix, 0).single().map_or_else(String::new, |t| t.format("%a %-d %b %Y, %H:%M").to_string())
}

impl Screen for HistoryView {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        self.refresh(sh);
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let library_id = self.library_id;
        if press.is(Action::Menu) {
            return self.entries.get(self.list.selected).map_or(Nav::None, |e| {
                Nav::push(OptionsMenu::new(Target::File { library_id, file_id: e.file_id }))
            });
        }
        match self.list.handle(press, self.entries.len()) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) => super::details::open_file(sh, library_id, self.entries[i].file_id),
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let Some(state) = sh.lib(self.library_id) else { return Nav::Pop };
        let body = widgets::header(ui.painter(), rect, "History", &state.lib.name, ui.style());
        if self.entries.is_empty() {
            widgets::message(ui.painter(), body, "Nothing watched yet", ui.style());
            return Nav::None;
        }
        let rows: Vec<Row> = self
            .entries
            .iter()
            .map(|e| {
                let how = match (e.completed, e.position, e.source.as_str()) {
                    (true, _, "plex") => "watched (Plex)".to_owned(),
                    (true, _, _) => "watched".to_owned(),
                    (false, Some(p), _) => format!("stopped at {}", theme::format_time(p)),
                    (false, None, _) => "started".to_owned(),
                };
                Row::new(Icon::Video, label(sh, self.library_id, e.file_id)).detail(format!("{} · {how}", when(e.played_at)))
            })
            .collect();
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
