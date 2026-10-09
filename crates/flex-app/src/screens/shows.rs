//! A library organised into shows: Shows → Show (seasons) → Season (episodes).
//! Movies libraries list movies, which play directly.

use eframe::egui::{self, Rect, Ui};

use crate::images::Size;

/// Width of the poster on a show's page.
const POSTER_W: f32 = 340.0;
use flex_core::catalog::ShowEntry;

use super::inspector::MatchInspector;
use super::match_online::MatchOnline;
use super::library::file_row_titled;
use super::options::{OptionsMenu, Target};
use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::{LibState, Shared, thousands};
use crate::widgets::{self, Icon, ListInput, ListState, Row};

fn season_name(season: i32) -> String {
    if season == 0 { "Specials".to_owned() } else { format!("Season {season}") }
}

/// Appended to details of shows not found online (they keep their file-name title).
fn unmatched_note(entry: &ShowEntry) -> &'static str {
    if entry.show.state == "unmatched" { " · not matched online" } else { "" }
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
pub fn open_show(sh: &Shared, library_id: i64, entry: &ShowEntry) -> Nav {
    if !entry.is_series() {
        let first = entry.episodes.first().map(|e| e.file_id);
        return Nav::play(first.and_then(|id| sh.play_request(library_id, id)));
    }
    match entry.seasons().as_slice() {
        [only] => Nav::push(SeasonView::new(library_id, entry.show.id, *only)),
        _ => Nav::push(ShowView::new(library_id, entry.show.id)),
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
        let subtitle = format!("{}{}", state.lib.name, unmatched_note(entry));
        // Poster in a column on the right; everything else to its left.
        let poster_path = entry.show.poster.clone();
        let (rect, poster_rect) = match &poster_path {
            Some(_) => {
                let poster = egui::Rect::from_min_size(egui::pos2(rect.right() - POSTER_W, rect.top()), egui::vec2(POSTER_W, POSTER_W * 1.5));
                (egui::Rect::from_min_max(rect.min, egui::pos2(poster.left() - 40.0, rect.bottom())), Some(poster))
            }
            None => (rect, None),
        };
        let mut body = widgets::header(ui.painter(), rect, &title_with_year(entry), &subtitle, ui.style());
        if let Some(overview) = entry.show.overview.as_deref().filter(|o| !o.is_empty()) {
            body.min.y += widgets::paragraph(ui.painter(), body, overview, 3, ui.style()) + 16.0;
        }
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
        if let (Some(path), Some(poster_rect)) = (poster_path, poster_rect) {
            match sh.images.get(&path, Size::Poster) {
                Some(texture) => crate::images::paint_cover(ui.painter(), poster_rect, &texture),
                None => {
                    ui.painter().rect_filled(poster_rect, 8, crate::theme::ROW);
                }
            }
        }
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
        match self.list.handle(press, 4) {
            ListInput::Activate(i @ (0 | 1)) => {
                sh.set_watched(self.library_id, &ids, i == 0);
                Nav::Pop
            }
            ListInput::Activate(2) => Nav::replace(MatchOnline::new(sh, self.library_id, self.show_id)),
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
            Row::new(Icon::Refresh, "Match online…").detail("Pick the right title on TMDB"),
            Row::new(Icon::Folder, "Folders & seasons…"),
        ];
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
