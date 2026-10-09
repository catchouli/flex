//! A library organised into shows: Shows → Show (seasons) → Season (episodes).
//! Movies libraries list movies, which play directly.

use eframe::egui::{Rect, Ui};


use flex_core::catalog::ShowEntry;

use super::inspector::MatchInspector;
use super::match_online::MatchOnline;
use super::library::file_row_titled;
use super::options::{OptionsMenu, Target};
use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::widgets::{self, Icon, ListInput, ListState, Row};

fn season_name(season: i32) -> String {
    if season == 0 { "Specials".to_owned() } else { format!("Season {season}") }
}


fn title_with_year(entry: &ShowEntry) -> String {
    match entry.show.year {
        Some(y) => format!("{} ({y})", entry.show.title),
        None => entry.show.title.clone(),
    }
}


/// Opens a show or film's page.
pub fn open_show(_sh: &Shared, library_id: i64, entry: &ShowEntry) -> Nav {
    Nav::push(super::details::TitlePage::new(library_id, entry.show.id))
}

pub struct SeasonView {
    library_id: i64,
    show_id: i64,
    season: i32,
    list: ListState,
}

impl SeasonView {
    pub fn new(library_id: i64, show_id: i64, season: i32) -> Self {
        Self { library_id, show_id, season, list: ListState::default() }
    }
}

impl Screen for SeasonView {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let Some(entry) = sh.lib(self.library_id).and_then(|l| l.catalog()?.show(self.show_id)) else {
            return Nav::Pop;
        };
        let ids: Vec<i64> = entry.season_episodes(self.season).map(|e| e.file_id).collect();
        let library_id = self.library_id;
        if press.is(Action::Menu) {
            return ids.get(self.list.selected).map_or(Nav::None, |&file_id| {
                Nav::push(OptionsMenu::new(Target::File { library_id, file_id }))
            });
        }
        // Play/Pause plays straight away; OK opens the episode's page.
        if press.is(Action::PlayPause) {
            return ids.get(self.list.selected).map_or(Nav::None, |&id| Nav::play(sh.play_request(library_id, id)));
        }
        match self.list.handle(press, ids.len()) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) => Nav::push(super::details::EpisodePage::new(library_id, ids[i])),
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(state) = sh.lib(self.library_id) else { return Nav::Pop };
        let Some(entry) = state.catalog().and_then(|c| c.show(self.show_id)) else { return Nav::Pop };
        let body = widgets::header(ui.painter(), rect, &season_name(self.season), &title_with_year(entry), ui.style());
        let rows: Vec<Row> = entry
            .season_episodes(self.season)
            .filter_map(|e| {
                let file = state.tree.file(e.file_id)?;
                let name = sh.titles(self.library_id, e.file_id).map_or_else(String::new, |t| t.name);
                let title = match e.number {
                    Some(n) if self.season > 0 => format!("{n:>2}. {name}"),
                    _ => name,
                };
                Some(file_row_titled(file, title))
            })
            .collect();
        if rows.is_empty() {
            widgets::message(ui.painter(), body, "No episodes", ui.style());
        }
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

/// Hold-OK options for a whole show or movie.
pub struct ShowOptions {
    library_id: i64,
    show_id: i64,
    list: ListState,
}

impl ShowOptions {
    pub fn new(library_id: i64, show_id: i64) -> Self {
        Self { library_id, show_id, list: ListState::default() }
    }
}

impl Screen for ShowOptions {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) || press.is(Action::Menu) {
            return Nav::Pop;
        }
        let Some(entry) = sh.lib(self.library_id).and_then(|l| l.catalog()?.show(self.show_id)).cloned() else {
            return Nav::Pop;
        };
        let ids: Vec<i64> = entry.episodes.iter().map(|e| e.file_id).collect();
        match self.list.handle(press, 5) {
            ListInput::Activate(0) => {
                let on = !sh.on_watchlist(self.library_id, self.show_id);
                sh.set_watchlist(self.library_id, self.show_id, on);
                Nav::Pop
            }
            ListInput::Activate(i @ (1 | 2)) => {
                sh.set_watched(self.library_id, &ids, i == 1);
                Nav::Pop
            }
            ListInput::Activate(3) => Nav::replace(MatchOnline::new(sh, self.library_id, self.show_id)),
            ListInput::Activate(_) => Nav::replace(MatchInspector::new(self.library_id, self.show_id)),
            _ => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(entry) = sh.lib(self.library_id).and_then(|l| l.catalog()?.show(self.show_id)) else {
            return Nav::Pop;
        };
        let body = widgets::header(ui.painter(), rect, &title_with_year(entry), "Options", ui.style());
        let watchlist = if sh.on_watchlist(self.library_id, self.show_id) {
            Row::new(Icon::Remove, "Remove from watchlist")
        } else {
            Row::new(Icon::Add, "Add to watchlist")
        };
        let rows = [
            watchlist,
            Row::new(Icon::Add, "Mark all as watched"),
            Row::new(Icon::Remove, "Mark all as unwatched"),
            Row::new(Icon::Refresh, "Match online…").detail("Pick the right title on TMDB"),
            Row::new(Icon::Folder, "Folders & seasons…"),
        ];
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
