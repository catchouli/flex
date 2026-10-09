//! Settings: manage libraries (add, rescan, change type, remove).

use eframe::egui::{Rect, Ui};
use flex_core::LibraryKind;

use super::picker::{FolderPicker, Purpose};
use super::{Nav, Screen, text_entry::TextEntry};
use crate::input::{Action, Press};
use crate::shared::{LibraryView, Shared, WATCHED_THRESHOLDS};
use crate::widgets::{self, Icon, ListInput, ListState, Row};

#[derive(Default)]
pub struct Settings {
    list: ListState,
}

impl Screen for Settings {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let libs = sh.libs.len();
        match self.list.handle(press, libs + 6) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) if i < libs => Nav::push(LibraryOptions::new(sh.libs[i].lib.id)),
            ListInput::Activate(i) if i == libs => Nav::push(FolderPicker::new(sh)),
            ListInput::Activate(i) if i == libs + 1 => {
                let current = sh.watched_threshold();
                let i = WATCHED_THRESHOLDS.iter().position(|t| (t - current).abs() < 0.001).unwrap_or(2);
                sh.set_watched_threshold(WATCHED_THRESHOLDS[(i + 1) % WATCHED_THRESHOLDS.len()]);
                Nav::None
            }
            ListInput::Activate(i) if i == libs + 2 => Nav::push(TextEntry::tmdb_token()),
            ListInput::Activate(i) if i == libs + 3 => Nav::push(FolderPicker::with_purpose(sh, Purpose::PlexDatabase)),
            ListInput::Activate(i) if i == libs + 5 => {
                crate::logging::open_logs_folder();
                Nav::None
            }
            ListInput::Activate(_) => {
                sh.scan_all();
                sh.toast("Rescanning all libraries…");
                Nav::None
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let body = widgets::header(ui.painter(), rect, "Settings", "Libraries", ui.style());
        let mut rows: Vec<Row> = sh
            .libs
            .iter()
            .map(|l| {
                Row::new(Icon::Folder, &l.lib.name)
                    .detail(format!("{} · {} · {}", l.lib.kind.label(), l.status(), l.lib.root.display()))
            })
            .collect();
        rows.push(Row::new(Icon::Add, "Add library…"));
        let threshold = (sh.watched_threshold() * 100.0).round();
        rows.push(Row::new(Icon::None, format!("Mark as watched after {threshold}%")).detail("OK to change"));
        let tmdb = if sh.tmdb_token().is_some() { "connected" } else { "not set up" };
        rows.push(Row::new(Icon::None, format!("TMDB (titles & posters): {tmdb}")).detail("OK to enter a token"));
        rows.push(Row::new(Icon::Add, "Import from Plex…").detail("Watched state, resume points and history"));
        rows.push(Row::new(Icon::Refresh, "Rescan all libraries"));
        rows.push(Row::new(Icon::Folder, "Open logs folder").detail(crate::logging::logs_dir().display().to_string()));
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

pub struct LibraryOptions {
    library_id: i64,
    confirm_remove: bool,
    list: ListState,
}

impl LibraryOptions {
    fn new(library_id: i64) -> Self {
        Self { library_id, confirm_remove: false, list: ListState::default() }
    }
}

impl Screen for LibraryOptions {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let Some(state) = sh.lib(self.library_id) else { return Nav::Pop };
        let kind = state.lib.kind;
        let input = self.list.handle(press, 6);
        // Moving off "Remove" cancels a pending confirmation.
        self.confirm_remove &= self.list.selected == 5;
        match input {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(0) => {
                sh.start_scan(self.library_id);
                Nav::None
            }
            ListInput::Activate(1) => Nav::push(TextEntry::rename_library(sh, self.library_id, "Rename library")),
            ListInput::Activate(2) => {
                let i = LibraryKind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
                let next = LibraryKind::ALL[(i + 1) % LibraryKind::ALL.len()];
                if let Err(e) = sh.set_kind(self.library_id, next) {
                    sh.toast(format!("{e:#}"));
                }
                Nav::None
            }
            ListInput::Activate(3) => {
                let view = match sh.library_view(self.library_id) {
                    LibraryView::Shows => LibraryView::Folders,
                    LibraryView::Folders => LibraryView::Shows,
                };
                sh.set_library_view(self.library_id, view);
                Nav::None
            }
            ListInput::Activate(4) => {
                sh.rematch(self.library_id);
                sh.toast("Re-matching… shows you fixed by hand are kept");
                Nav::None
            }
            ListInput::Activate(_) if !self.confirm_remove => {
                self.confirm_remove = true;
                Nav::None
            }
            ListInput::Activate(_) => {
                if let Err(e) = sh.remove_library(self.library_id) {
                    sh.toast(format!("{e:#}"));
                    return Nav::None;
                }
                sh.toast("Library removed");
                Nav::Pop
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(state) = sh.lib(self.library_id) else { return Nav::Pop };
        let subtitle = format!("{} · {}", state.lib.root.display(), state.status());
        let body = widgets::header(ui.painter(), rect, &state.lib.name, &subtitle, ui.style());
        let remove = if self.confirm_remove {
            "Press OK again to remove (your files are not touched)"
        } else {
            "Remove library"
        };
        let rows = [
            Row::new(Icon::Refresh, "Rescan now"),
            Row::new(Icon::None, format!("Name: {}", state.lib.name)).detail("OK to rename"),
            Row::new(Icon::None, format!("Type: {}", state.lib.kind.label())).detail("OK to change"),
            Row::new(Icon::None, match sh.library_view(self.library_id) {
                LibraryView::Shows => "Opens in: Shows",
                LibraryView::Folders => "Opens in: Folders",
            })
            .detail("OK to change"),
            Row::new(Icon::Refresh, "Re-match online").detail("Redo automatic matches; keeps your fixes"),
            Row::new(Icon::Remove, remove),
        ];
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
