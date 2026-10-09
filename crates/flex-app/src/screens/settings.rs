//! Settings: manage libraries (add, rescan, change type, remove).

use eframe::egui::{Rect, Ui};
use flex_core::LibraryKind;

use super::{Nav, Screen, picker::FolderPicker};
use crate::input::{Action, Press};
use crate::shared::Shared;
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
        match self.list.handle(press, libs + 2) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) if i < libs => Nav::push(LibraryOptions::new(sh.libs[i].lib.id)),
            ListInput::Activate(i) if i == libs => Nav::push(FolderPicker::new(sh)),
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
        rows.push(Row::new(Icon::Refresh, "Rescan all libraries"));
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
        let input = self.list.handle(press, 3);
        // Moving off "Remove" cancels a pending confirmation.
        self.confirm_remove &= self.list.selected == 2;
        match input {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(0) => {
                sh.start_scan(self.library_id);
                Nav::None
            }
            ListInput::Activate(1) => {
                let i = LibraryKind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
                let next = LibraryKind::ALL[(i + 1) % LibraryKind::ALL.len()];
                if let Err(e) = sh.set_kind(self.library_id, next) {
                    sh.toast(format!("{e:#}"));
                }
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
            Row::new(Icon::None, format!("Type: {}", state.lib.kind.label())).detail("OK to change"),
            Row::new(Icon::Remove, remove),
        ];
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
