//! A library organised into shows: Shows → Show (seasons) → Season (episodes).
//! Movies libraries list movies, which play directly.

use eframe::egui::{self, Align2, CornerRadius, Rect, TextStyle, Ui};
use flex_core::catalog::ShowEntry;

use super::inspector::MatchInspector;
use super::match_online::MatchOnline;
use super::library::file_row_titled;
use super::options::{OptionsMenu, Target};
use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::images::{Size, paint_cover};
use crate::shots::{self, Shot};
use crate::theme;
use crate::widgets::{self, GridSpec, GridState, Icon, ListInput, ListState, Row};

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

const SEASON_LAYOUT_SETTING: &str = "season_layout";
/// Episode tile: a 16:9 still plus two lines of text.
const TILE_W: f32 = 400.0;
const STILL_H: f32 = TILE_W * 9.0 / 16.0;
const TILES: GridSpec = GridSpec { card: egui::vec2(TILE_W, STILL_H + 80.0), gap: 26.0 };
const TOOLBAR_H: f32 = 64.0;

/// One episode as shown in a season.
struct EpisodeItem {
    file_id: i64,
    /// "5. Title".
    title: String,
    /// TMDB still, if any.
    still: Option<String>,
    progress: flex_core::progress::Progress,
    air_date: Option<String>,
}

/// A season's episodes, as tiles with stills (screenshots for episodes TMDB has none for)
/// or as a list. Up from the top reaches the View button.
pub struct SeasonView {
    library_id: i64,
    show_id: i64,
    season: i32,
    tiles: bool,
    /// The View button has focus.
    toolbar: bool,
    list: ListState,
    grid: GridState,
}

impl SeasonView {
    pub fn new(sh: &Shared, library_id: i64, show_id: i64, season: i32) -> Self {
        let tiles = sh.setting(SEASON_LAYOUT_SETTING).is_none_or(|v| v != "list");
        Self { library_id, show_id, season, tiles, toolbar: false, list: ListState::default(), grid: GridState::default() }
    }

    fn selected(&self) -> usize {
        if self.tiles { self.grid.selected } else { self.list.selected }
    }

    fn items(&self, sh: &Shared) -> Vec<EpisodeItem> {
        let Some(state) = sh.lib(self.library_id) else { return Vec::new() };
        let Some(entry) = state.catalog().and_then(|c| c.show(self.show_id)) else { return Vec::new() };
        entry
            .season_episodes(self.season)
            .filter_map(|e| {
                let file = state.tree.file(e.file_id)?;
                let name = sh.titles(self.library_id, e.file_id).map_or_else(String::new, |t| t.name);
                let title = match e.number {
                    Some(n) if self.season > 0 => format!("{n}. {name}"),
                    _ => name,
                };
                let meta = sh.episode_meta(self.library_id, e.file_id);
                Some(EpisodeItem {
                    file_id: e.file_id,
                    title,
                    still: meta.and_then(|m| m.still.clone()),
                    progress: file.progress,
                    air_date: meta.and_then(|m| m.air_date.clone()),
                })
            })
            .collect()
    }

    fn toggle_layout(&mut self, sh: &mut Shared) {
        let i = self.selected();
        self.tiles = !self.tiles;
        sh.set_setting(SEASON_LAYOUT_SETTING, if self.tiles { "tiles" } else { "list" });
        self.grid.select(i);
        self.list.select(i);
    }
}

fn paint_tile(painter: &egui::Painter, rect: Rect, item: &EpisodeItem, still: Option<&egui::TextureHandle>, focused: bool, style: &egui::Style) {
    let still_rect = Rect::from_min_size(rect.min, egui::vec2(rect.width(), STILL_H));
    match still {
        Some(t) => paint_cover(painter, still_rect, t),
        None => {
            painter.rect_filled(still_rect, CornerRadius::same(8), theme::panel());
        }
    }
    if item.progress.watched {
        widgets::paint_tick(painter, still_rect.right_top() + egui::vec2(-26.0, 26.0));
    }
    if let Some(fraction) = item.progress.fraction() {
        widgets::paint_progress(painter, still_rect, fraction as f32, theme::progress());
    }
    theme::paint_picture_frame(painter, still_rect, focused);
    let text = Rect::from_min_max(egui::pos2(rect.left(), still_rect.bottom() + 8.0), rect.max);
    let clip = painter.with_clip_rect(text);
    let color = if focused { theme::highlight() } else if item.progress.watched { theme::dim() } else { theme::text() };
    clip.text(text.left_top(), Align2::LEFT_TOP, &item.title, TextStyle::Small.resolve(style), color);
    let p = item.progress;
    let sub = match p.remaining_secs() {
        Some(left) => theme::format_remaining(left),
        None if p.watched => "Watched".to_owned(),
        None => item.air_date.clone().unwrap_or_default(),
    };
    clip.text(text.left_top() + egui::vec2(0.0, 32.0), Align2::LEFT_TOP, sub, TextStyle::Small.resolve(style), theme::dim());
}

impl Screen for SeasonView {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if self.toolbar {
            match press.action {
                Action::Down => self.toolbar = false,
                Action::Up if !press.repeat => return Nav::FocusTopBar,
                Action::Back if !press.repeat => return Nav::Pop,
                _ if press.is_activate() => self.toggle_layout(sh),
                _ => {}
            }
            return Nav::None;
        }
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let Some(entry) = sh.lib(self.library_id).and_then(|l| l.catalog()?.show(self.show_id)) else {
            return Nav::Pop;
        };
        let ids: Vec<i64> = entry.season_episodes(self.season).map(|e| e.file_id).collect();
        let library_id = self.library_id;
        if press.is(Action::Menu) {
            return ids.get(self.selected()).map_or(Nav::None, |&file_id| {
                Nav::push(OptionsMenu::new(Target::File { library_id, file_id }))
            });
        }
        // Play/Pause plays straight away; OK opens the episode's page.
        if press.is(Action::PlayPause) {
            return ids.get(self.selected()).map_or(Nav::None, |&id| Nav::play(sh.play_request(library_id, id)));
        }
        let input = if self.tiles { self.grid.handle(press, ids.len()) } else { self.list.handle(press, ids.len()) };
        match input {
            ListInput::AtTop => {
                self.toolbar = true;
                Nav::None
            }
            ListInput::Activate(i) => Nav::push(super::details::EpisodePage::new(library_id, ids[i])),
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(state) = sh.lib(self.library_id) else { return Nav::Pop };
        let Some(entry) = state.catalog().and_then(|c| c.show(self.show_id)) else { return Nav::Pop };
        let style = ui.style().clone();
        let body = widgets::header(ui.painter(), rect, &season_name(self.season), &title_with_year(entry), &style);
        let items = self.items(sh);

        let label = if self.tiles { "View: Tiles" } else { "View: List" };
        let font = TextStyle::Small.resolve(&style);
        let width = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), egui::Color32::WHITE).size().x + 44.0;
        let button = Rect::from_min_size(body.min, egui::vec2(width, TOOLBAR_H));
        if widgets::hit(ui, button).any() {
            self.toolbar = true;
        }
        let color = widgets::paint_focusable(ui.painter(), button, focused && self.toolbar, false);
        ui.painter().text(button.center(), Align2::CENTER_CENTER, label, font, color);
        let content = Rect::from_min_max(egui::pos2(body.left(), button.bottom() + 24.0), body.max);
        let content_focused = focused && !self.toolbar;

        if items.is_empty() {
            widgets::message(ui.painter(), content, "No episodes", &style);
            return Nav::None;
        }
        let hit = if self.tiles {
            let library_id = self.library_id;
            widgets::grid(ui, content, items.len(), TILES, &mut self.grid, content_focused, |painter, i, r, f| {
                let item = &items[i];
                // TMDB's still, else a screenshot from the episode itself.
                let still = match &item.still {
                    Some(path) => sh.images.get(path, Size::Still),
                    None => match sh.shot(library_id, item.file_id, shots::THUMB) {
                        Shot::Ready(path) => sh.images.get(&path, Size::Shot),
                        _ => None,
                    },
                };
                paint_tile(painter, r, item, still.as_ref(), f, &style);
            })
        } else {
            let rows: Vec<Row> = items
                .iter()
                .filter_map(|it| Some(file_row_titled(state_file(sh, self.library_id, it.file_id)?, it.title.clone())))
                .collect();
            widgets::list(ui, content, &rows, &mut self.list, content_focused)
        };
        if hit.any() {
            self.toolbar = false;
        }
        Nav::None
    }
}

fn state_file(sh: &Shared, library_id: i64, file_id: i64) -> Option<&flex_core::MediaFile> {
    sh.lib(library_id)?.tree.file(file_id)
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
