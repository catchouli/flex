//! Search results for the query typed in the top bar.

use eframe::egui::{Rect, Ui};
use flex_core::search::{Item, Target};

use super::options::{self, OptionsMenu};
use super::{Nav, Screen, library::open_folder};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::widgets::{self, Icon, ListInput, ListState, Row};

const MAX_RESULTS: usize = 100;

#[derive(Default)]
pub struct SearchView {
    query: Option<String>,
    results: Vec<Item>,
    list: ListState,
}

impl SearchView {
    fn refresh(&mut self, sh: &mut Shared) {
        if self.query.as_deref() != Some(sh.search_query.as_str()) {
            let query = sh.search_query.clone();
            self.results = sh.search(&query, MAX_RESULTS);
            self.query = Some(query);
            self.list.select(0);
        }
    }

    fn open(&self, sh: &mut Shared, i: usize) -> Nav {
        match &self.results[i].target {
            Target::Folder { library_id, rel } => open_folder(sh, *library_id, rel),
            Target::File { library_id, file_id } => Nav::play(sh.play_request(*library_id, *file_id)),
        }
    }
}

impl Screen for SearchView {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        self.refresh(sh);
        if press.is(Action::Back) {
            sh.search_query.clear();
            return Nav::Pop;
        }
        if press.is(Action::Menu) {
            let Some(item) = self.results.get(self.list.selected) else { return Nav::None };
            let target = match &item.target {
                Target::Folder { library_id, rel } => options::Target::Folder { library_id: *library_id, rel: rel.clone() },
                Target::File { library_id, file_id } => {
                    options::Target::File { library_id: *library_id, file_id: *file_id }
                }
            };
            return Nav::push(OptionsMenu::new(target));
        }
        match self.list.handle(press, self.results.len()) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) => self.open(sh, i),
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let subtitle = match self.results.len() {
            0 => String::new(),
            MAX_RESULTS => format!("Top {MAX_RESULTS} matches"),
            n => format!("{n} matches"),
        };
        let title = format!("“{}”", sh.search_query.trim());
        let body = widgets::header(ui.painter(), rect, &title, &subtitle, ui.style());
        if self.results.is_empty() {
            widgets::message(ui.painter(), body, "Nothing found", ui.style());
            return Nav::None;
        }
        let rows: Vec<Row> = self
            .results
            .iter()
            .map(|item| {
                let icon = if matches!(item.target, Target::Folder { .. }) { Icon::Folder } else { Icon::Video };
                Row::new(icon, item.title.as_str()).detail(item.context.as_str())
            })
            .collect();
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }

    fn is_search(&self) -> bool {
        true
    }

    fn activate_first(&mut self, sh: &mut Shared) -> Nav {
        self.refresh(sh);
        if self.results.is_empty() { Nav::None } else { self.open(sh, 0) }
    }
}
