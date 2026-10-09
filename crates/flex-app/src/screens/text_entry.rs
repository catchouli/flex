//! Typing a value (needs a keyboard): naming a library, or pasting the TMDB token.
//! Enter saves; Esc or Back leaves it unchanged.

use eframe::egui::{self, Key, Rect, TextStyle, Ui, vec2};

use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::widgets;

pub enum Purpose {
    RenameLibrary(i64),
    TmdbToken,
}

pub struct TextEntry {
    purpose: Purpose,
    title: &'static str,
    hint: &'static str,
    text: String,
    /// Focus the text box on the first frame.
    focus_pending: bool,
}

impl TextEntry {
    /// `title` is e.g. "Name this library" (after adding) or "Rename library".
    pub fn rename_library(sh: &Shared, library_id: i64, title: &'static str) -> Self {
        let text = sh.lib(library_id).map_or_else(String::new, |l| l.lib.name.clone());
        Self {
            purpose: Purpose::RenameLibrary(library_id),
            title,
            hint: "Type a name, then Enter. Esc keeps the current name.",
            text,
            focus_pending: true,
        }
    }

    pub fn tmdb_token() -> Self {
        Self {
            purpose: Purpose::TmdbToken,
            title: "TMDB token",
            hint: "Paste the API Read Access Token from themoviedb.org → Settings → API, then Enter.",
            text: String::new(),
            focus_pending: true,
        }
    }

    fn id() -> egui::Id {
        egui::Id::new("flex-text-entry")
    }

    fn save(&self, sh: &mut Shared) -> anyhow::Result<()> {
        match self.purpose {
            Purpose::RenameLibrary(id) => sh.rename_library(id, &self.text),
            Purpose::TmdbToken => {
                anyhow::ensure!(!self.text.trim().is_empty(), "Paste the token first");
                sh.set_tmdb_token(&self.text);
                sh.toast("Token saved, matching libraries…");
                Ok(())
            }
        }
    }
}

impl Screen for TextEntry {
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
        let body = widgets::header(ui.painter(), rect, self.title, self.hint, ui.style());
        let field = Rect::from_min_size(body.min, vec2(body.width().min(900.0), 64.0));
        let edit = egui::TextEdit::singleline(&mut self.text)
            .id(Self::id())
            .password(matches!(self.purpose, Purpose::TmdbToken))
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
                return match self.save(sh) {
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
