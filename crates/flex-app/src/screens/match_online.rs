//! "Match online": search TMDB for a show or film and pick the right entry by hand.
//! The search box needs a keyboard; the results can be picked with the remote.

use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui::{self, Key, Rect, TextStyle, Ui, vec2};
use flex_core::metadata::{Candidate, Kind, Tmdb};

use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::widgets::{self, Icon, ListInput, ListState, Row};

pub struct MatchOnline {
    library_id: i64,
    show_id: i64,
    query: String,
    kind: Kind,
    results: Vec<Candidate>,
    pending: Option<Receiver<Result<Vec<Candidate>, String>>>,
    status: String,
    list: ListState,
}

impl MatchOnline {
    pub fn new(sh: &Shared, library_id: i64, show_id: i64) -> Self {
        let entry = sh.lib(library_id).and_then(|l| l.catalog()?.show(show_id));
        let query = entry.map_or_else(String::new, |e| e.show.title.clone());
        let kind = if entry.is_some_and(|e| e.is_series()) { Kind::Tv } else { Kind::Movie };
        let mut screen = Self {
            library_id,
            show_id,
            query,
            kind,
            results: Vec::new(),
            pending: None,
            status: String::new(),
            list: ListState::default(),
        };
        screen.search(sh);
        screen
    }

    fn search(&mut self, sh: &Shared) {
        let Some(token) = sh.tmdb_token() else {
            self.status = "Set up TMDB in Settings first".into();
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let (query, kind, ctx) = (self.query.clone(), self.kind, sh.ctx.clone());
        std::thread::spawn(move || {
            let result = Tmdb::new(&token).search(kind, &query, None).map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.pending = Some(rx);
        self.status = "Searching…".into();
    }

    fn poll(&mut self) {
        let Some(rx) = &self.pending else { return };
        match rx.try_recv() {
            Ok(Ok(results)) => {
                self.status = if results.is_empty() { "Nothing found".into() } else { String::new() };
                self.results = results;
                self.list.select(0);
            }
            Ok(Err(e)) => self.status = e,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {}
        }
        self.pending = None;
    }

    fn query_id() -> egui::Id {
        egui::Id::new("flex-match-online-query")
    }
}

impl Screen for MatchOnline {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        // Row 0 switches between searching TV and films; then the results.
        match self.list.handle(press, self.results.len() + 1) {
            ListInput::AtTop => {
                sh.ctx.memory_mut(|m| m.request_focus(Self::query_id()));
                Nav::None
            }
            ListInput::Activate(0) => {
                self.kind = if self.kind == Kind::Tv { Kind::Movie } else { Kind::Tv };
                self.search(sh);
                Nav::None
            }
            ListInput::Activate(i) => {
                let chosen = self.results[i - 1].clone();
                sh.apply_match(self.library_id, self.show_id, &chosen);
                Nav::Pop
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.poll();
        let body = widgets::header(ui.painter(), rect, "Match online", &self.status, ui.style());
        let field = Rect::from_min_size(body.min, vec2(body.width().min(900.0), 56.0));
        let edit = egui::TextEdit::singleline(&mut self.query)
            .id(Self::query_id())
            .hint_text("Title to search for (keyboard)")
            .font(TextStyle::Body)
            .margin(vec2(16.0, 10.0))
            .desired_width(field.width());
        let response = ui.put(field, edit);
        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            self.search(sh);
        }
        if response.has_focus() && ui.input(|i| i.key_pressed(Key::ArrowDown)) {
            response.surrender_focus();
        }
        let list_rect = Rect::from_min_max(egui::pos2(body.left(), field.bottom() + 16.0), body.max);
        let kind = if self.kind == Kind::Tv { "Searching TV series" } else { "Searching films" };
        let mut rows = vec![Row::new(Icon::Refresh, kind).detail("OK to switch")];
        rows.extend(self.results.iter().map(|c| {
            let title = match c.year {
                Some(y) => format!("{} ({y})", c.title),
                None => c.title.clone(),
            };
            let detail = if c.original_title != c.title { c.original_title.clone() } else { String::new() };
            Row::new(Icon::Video, title).detail(detail)
        }));
        widgets::list(ui, list_rect, &rows, &mut self.list, focused);
        Nav::None
    }
}
