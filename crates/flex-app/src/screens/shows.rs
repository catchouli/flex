//! A library organised into shows: Shows → Show (seasons) → Season (episodes).
//! Movies libraries list movies, which play directly.

use eframe::egui::{Rect, Ui};
use flex_core::catalog::ShowEntry;
use flex_core::library::LibraryKind;

use super::inspector::MatchInspector;
use super::library::{LibraryView, file_row};
use super::options::{OptionsMenu, Target};
use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::{LibState, Shared, thousands};
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

/// "12 episodes · 3 unwatched" / "All watched".
fn episode_summary(state: &LibState, ids: impl Iterator<Item = i64>) -> (String, bool) {
    let (mut total, mut unwatched) = (0, 0);
    for id in ids {
        total += 1;
        unwatched += usize::from(!state.tree.file(id).is_some_and(|f| f.progress.watched));
    }
    let count = if total == 1 { "1 episode".to_owned() } else { format!("{} episodes", thousands(total)) };
    match unwatched {
        0 => ("All watched".to_owned(), true),
        u if u == total => (count, false),
        u => (format!("{count} · {} unwatched", thousands(u)), false),
    }
}

/// Opens a show: its only season directly, a season list otherwise; movies play.
fn open_show(sh: &Shared, library_id: i64, entry: &ShowEntry) -> Nav {
    if !entry.is_series() {
        let first = entry.episodes.first().map(|e| e.file_id);
        return Nav::play(first.and_then(|id| sh.play_request(library_id, id)));
    }
    match entry.seasons().as_slice() {
        [only] => Nav::push(SeasonView::new(library_id, entry.show.id, *only)),
        _ => Nav::push(ShowView::new(library_id, entry.show.id)),
    }
}

pub struct ShowsView {
    library_id: i64,
    list: ListState,
}

impl ShowsView {
    pub fn new(library_id: i64) -> Self {
        Self { library_id, list: ListState::default() }
    }
}

impl Screen for ShowsView {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let Some(catalog) = sh.lib(self.library_id).and_then(|l| l.catalog()) else {
            return match self.list.handle(press, 1) {
                ListInput::AtTop => Nav::FocusTopBar,
                ListInput::Activate(_) => Nav::push(LibraryView::new(self.library_id, String::new())),
                ListInput::None => Nav::None,
            };
        };
        // Row 0 is "Browse folders".
        let entry = self.list.selected.checked_sub(1).and_then(|i| catalog.shows.get(i)).cloned();
        if press.is(Action::Menu) {
            return entry.map_or(Nav::None, |e| Nav::push(ShowOptions::new(self.library_id, e.show.id)));
        }
        match self.list.handle(press, catalog.shows.len() + 1) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(0) => Nav::push(LibraryView::new(self.library_id, String::new())),
            ListInput::Activate(_) => entry.map_or(Nav::None, |e| open_show(sh, self.library_id, &e)),
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(state) = sh.lib(self.library_id) else { return Nav::Pop };
        let movies = state.lib.kind == LibraryKind::Movies;
        let Some(catalog) = state.catalog() else {
            let subtitle = if state.organising || state.scanning.is_some() { "Organising…" } else { "" };
            let body = widgets::header(ui.painter(), rect, &state.lib.name, subtitle, ui.style());
            let rows = [Row::new(Icon::Folder, "Browse folders")];
            widgets::list(ui, body, &rows, &mut self.list, focused);
            return Nav::None;
        };
        let what = if movies { "movies" } else { "shows" };
        let subtitle = format!("{} {what} · {}", thousands(catalog.shows.len()), state.status());
        let body = widgets::header(ui.painter(), rect, &state.lib.name, &subtitle, ui.style());
        let mut rows = vec![Row::new(Icon::Folder, "Browse folders")];
        for entry in &catalog.shows {
            let row = if !entry.is_series() {
                let file = entry.episodes.first().and_then(|e| state.tree.file(e.file_id));
                let mut row = file.map_or_else(|| Row::new(Icon::Video, ""), file_row);
                row.title = title_with_year(entry);
                row
            } else {
                let (summary, done) = episode_summary(state, entry.episodes.iter().map(|e| e.file_id));
                let seasons = entry.seasons().iter().filter(|s| **s != 0).count();
                let detail = if seasons > 1 { format!("{seasons} seasons · {summary}") } else { summary };
                Row::new(Icon::Folder, title_with_year(entry)).detail(detail).dim(done)
            };
            rows.push(row);
        }
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

pub struct ShowView {
    library_id: i64,
    show_id: i64,
    list: ListState,
}

impl ShowView {
    pub fn new(library_id: i64, show_id: i64) -> Self {
        Self { library_id, show_id, list: ListState::default() }
    }
}

impl Screen for ShowView {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let Some(entry) = sh.lib(self.library_id).and_then(|l| l.catalog()?.show(self.show_id)) else {
            return Nav::Pop;
        };
        let seasons = entry.seasons();
        let season = seasons.get(self.list.selected).copied();
        if press.is(Action::Menu) {
            return match season {
                Some(s) => {
                    let ids = entry.season_episodes(s).map(|e| e.file_id).collect();
                    let label = format!("{} · {}", entry.show.title, season_name(s));
                    Nav::push(OptionsMenu::new(Target::Files { library_id: self.library_id, ids, label }))
                }
                None => Nav::None,
            };
        }
        // Last row: the match inspector.
        match self.list.handle(press, seasons.len() + 1) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) if i == seasons.len() => {
                Nav::push(MatchInspector::new(self.library_id, self.show_id))
            }
            ListInput::Activate(i) => Nav::push(SeasonView::new(self.library_id, self.show_id, seasons[i])),
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(state) = sh.lib(self.library_id) else { return Nav::Pop };
        let Some(entry) = state.catalog().and_then(|c| c.show(self.show_id)) else { return Nav::Pop };
        let subtitle = entry.show.overview.clone().unwrap_or_else(|| state.lib.name.clone());
        let body = widgets::header(ui.painter(), rect, &title_with_year(entry), &subtitle, ui.style());
        let mut rows: Vec<Row> = entry
            .seasons()
            .into_iter()
            .map(|s| {
                let (summary, done) = episode_summary(state, entry.season_episodes(s).map(|e| e.file_id));
                Row::new(Icon::Folder, season_name(s)).detail(summary).dim(done)
            })
            .collect();
        rows.push(Row::new(Icon::None, "Folders & seasons…").detail("Check or fix how folders are matched"));
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
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
        match self.list.handle(press, ids.len()) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) => Nav::play(sh.play_request(library_id, ids[i])),
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
                let mut row = file_row(file);
                if let Some(n) = e.number {
                    row.title = format!("{n:>2}. {}", row.title);
                }
                Some(row)
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
        match self.list.handle(press, 3) {
            ListInput::Activate(i @ (0 | 1)) => {
                sh.set_watched(self.library_id, &ids, i == 0);
                Nav::Pop
            }
            ListInput::Activate(_) => Nav::replace(MatchInspector::new(self.library_id, self.show_id)),
            _ => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(entry) = sh.lib(self.library_id).and_then(|l| l.catalog()?.show(self.show_id)) else {
            return Nav::Pop;
        };
        let body = widgets::header(ui.painter(), rect, &title_with_year(entry), "Options", ui.style());
        let rows = [
            Row::new(Icon::Add, "Mark all as watched"),
            Row::new(Icon::Remove, "Mark all as unwatched"),
            Row::new(Icon::Folder, "Folders & seasons…"),
        ];
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
