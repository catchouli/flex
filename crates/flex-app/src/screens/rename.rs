//! Naming a library (needs a keyboard). Enter saves, Esc or Back keeps the current name.

use eframe::egui::{self, Key, Rect, TextStyle, Ui, vec2};

use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::widgets;

pub struct RenameLibrary {
    library_id: i64,
    title: &'static str,
    name: String,
    /// Focus the text box on the first frame.
    focus_pending: bool,
}

impl RenameLibrary {
    pub fn new(sh: &Shared, library_id: i64, title: &'static str) -> Self {
        let name = sh.lib(library_id).map_or_else(String::new, |l| l.lib.name.clone());
        Self { library_id, title, name, focus_pending: true }
    }

    fn id() -> egui::Id {
        egui::Id::new("flex-rename-library")
    }
}

impl Screen for RenameLibrary {
    /// Only reached while the text box isn't focused.
    fn handle(&mut self, _sh: &mut Shared, press: Press) -> Nav {
        match press.action {
            Action::Back | Action::Stop if !press.repeat => Nav::Pop,
            _ if press.is_activate() => {
                self.focus_pending = true;
                Nav::None
            }
            _ => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, _focused: bool) -> Nav {
        let subtitle = "Type a name, then Enter. Esc keeps the current name.";
        let body = widgets::header(ui.painter(), rect, self.title, subtitle, ui.style());
        let field = Rect::from_min_size(body.min, vec2(body.width().min(900.0), 64.0));
        let edit = egui::TextEdit::singleline(&mut self.name)
            .id(Self::id())
            .font(TextStyle::Body)
            .margin(vec2(18.0, 12.0))
            .desired_width(field.width());
        let response = ui.put(field, edit);
        if std::mem::take(&mut self.focus_pending) {
            response.request_focus();
        }
        if response.lost_focus() {
            let (enter, escape) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if escape {
                return Nav::Pop;
            }
            if enter {
                return match sh.rename_library(self.library_id, &self.name) {
                    Ok(()) => Nav::Pop,
                    Err(e) => {
                        sh.toast(format!("{e:#}"));
                        self.focus_pending = true;
                        Nav::None
                    }
                };
            }
        }
        Nav::None
    }

    fn shows_top_bar(&self) -> bool {
        false
    }
}
