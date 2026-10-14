//! A music library: a grid of albums, each album's page (tracks and artists), the list of
//! artists, and each artist's page (their albums and the ones they're on).

use eframe::egui::{self, Align2, Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind, TextStyle, Ui, pos2, vec2};
use flex_core::music::{Album, MusicCatalog};
use flex_core::natural::natural_cmp;

use super::library::LibraryView;
use super::{Nav, Screen};
use crate::images::{Size, paint_cover};
use crate::input::{Action, Press};
use crate::music_player::MusicPlayer;
use crate::shared::{Shared, thousands};
use crate::theme;
use crate::widgets::{self, GridSpec, GridState, Icon, ListInput, ListState, Row};

const CARD_W: f32 = 250.0;
const GRID: GridSpec = GridSpec { card: egui::vec2(CARD_W, CARD_W + 76.0), gap: 26.0 };
const TOOLBAR_H: f32 = 64.0;
const PAGE_COVER: f32 = 440.0;

/// Plays an album from track `start`, or shuffled.
pub fn play_album(sh: &mut Shared, library_id: i64, key: &str, start: usize, shuffle: bool) -> Nav {
    match MusicPlayer::open(sh, library_id, key, start, shuffle) {
        Ok(player) => Nav::push(player),
        Err(e) => {
            sh.toast(format!("Couldn't play: {e:#}"));
            Nav::None
        }
    }
}

/// Plays a track picked in the folder view: its album, from that track.
pub fn play_file(sh: &mut Shared, library_id: i64, file_id: i64) -> Nav {
    let found = sh.lib(library_id).and_then(|l| l.music.as_ref()).and_then(|m| m.track_of(file_id)).map(|(a, i)| (a.key.clone(), i));
    match found {
        Some((key, i)) => play_album(sh, library_id, &key, i, false),
        None => {
            sh.toast("Still reading the music; try again in a moment");
            Nav::None
        }
    }
}

fn catalog(sh: &Shared, library_id: i64) -> Option<std::sync::Arc<MusicCatalog>> {
    sh.lib(library_id)?.music.clone()
}

fn cover_path(album: &Album) -> Option<String> {
    album.cover.as_ref().map(|c| format!("file:{c}"))
}

fn duration_text(secs: f64) -> String {
    let mins = (secs / 60.0).round() as u64;
    if mins >= 60 { format!("{} h {} min", mins / 60, mins % 60) } else { format!("{mins} min") }
}

/// An album card: square cover, then the title and artist · year.
fn paint_album(painter: &Painter, sh: &mut Shared, rect: Rect, album: &Album, sub: &str, focused: bool, style: &egui::Style) {
    let cover = Rect::from_min_size(rect.min, vec2(rect.width(), rect.width()));
    match cover_path(album).and_then(|c| sh.images.get(&c, Size::Poster)) {
        Some(t) => paint_cover(painter, cover, &t),
        None => {
            painter.rect_filled(cover, CornerRadius::same(8), theme::ROW);
            let galley = painter.layout(album.title.clone(), TextStyle::Body.resolve(style), theme::DIM, cover.width() - 30.0);
            painter.galley(cover.center() - galley.size() / 2.0, galley, theme::DIM);
        }
    }
    if focused {
        painter.rect_stroke(cover, CornerRadius::same(8), Stroke::new(5.0, theme::FOCUS), StrokeKind::Inside);
    }
    let text = Rect::from_min_max(pos2(rect.left(), cover.bottom() + 8.0), rect.max);
    let clip = painter.with_clip_rect(text);
    let color = if focused { theme::FOCUS } else { theme::TEXT };
    clip.text(text.left_top(), Align2::LEFT_TOP, &album.title, TextStyle::Small.resolve(style), color);
    clip.text(text.left_top() + vec2(0.0, 32.0), Align2::LEFT_TOP, sub, TextStyle::Small.resolve(style), theme::DIM);
}

fn artist_year(album: &Album) -> String {
    match album.year {
        Some(y) => format!("{} · {y}", album.artist),
        None => album.artist.clone(),
    }
}

/// Draws a grid of albums (by index into the catalog); returns what the mouse did.
#[allow(clippy::too_many_arguments)]
fn album_grid(
    ui: &mut Ui,
    sh: &mut Shared,
    rect: Rect,
    music: &MusicCatalog,
    albums: &[usize],
    grid: &mut GridState,
    focused: bool,
    sub: impl Fn(usize, &Album) -> String,
) -> widgets::Hit {
    let style = ui.style().clone();
    widgets::grid(ui, rect, albums.len(), GRID, grid, focused, |painter, i, r, f| {
        let album = &music.albums[albums[i]];
        paint_album(painter, sh, r, album, &sub(i, album), f, &style);
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Added,
    Title,
    Artist,
    Year,
}

const SORTS: [(Sort, &str, &str); 4] =
    [(Sort::Added, "Recently added", "added"), (Sort::Artist, "Artist", "artist"), (Sort::Title, "A–Z", "title"), (Sort::Year, "Year", "year")];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Sort,
    /// Albums only, or singles too.
    Show,
    Artists,
    Folders,
}

const TOOLS: [Tool; 4] = [Tool::Sort, Tool::Show, Tool::Artists, Tool::Folders];

/// A music library's albums.
pub struct AlbumsView {
    library_id: i64,
    sort: Sort,
    /// Loose singles too, not just albums.
    singles: bool,
    /// `Some(i)` = toolbar button i has focus.
    toolbar: Option<usize>,
    grid: GridState,
    albums: Vec<usize>,
    built_for: Option<(u64, Sort, bool)>,
}

impl AlbumsView {
    pub fn new(sh: &Shared, library_id: i64) -> Self {
        let saved = sh.setting(&Self::sort_key(library_id)).unwrap_or_default();
        let sort = SORTS.iter().find(|s| s.2 == saved).map_or(Sort::Added, |s| s.0);
        let singles = sh.setting_bool(&Self::singles_key(library_id));
        Self { library_id, sort, singles, toolbar: None, grid: GridState::default(), albums: Vec::new(), built_for: None }
    }

    fn sort_key(library_id: i64) -> String {
        format!("music_sort:{library_id}")
    }

    fn singles_key(library_id: i64) -> String {
        format!("music_singles:{library_id}")
    }

    fn refresh(&mut self, sh: &Shared) {
        let version = sh.lib(self.library_id).map_or(0, |l| l.version);
        if self.built_for == Some((version, self.sort, self.singles)) {
            return;
        }
        let Some(music) = catalog(sh, self.library_id) else { return };
        let selected = self.albums.get(self.grid.selected).map(|&i| music.albums.get(i).map(|a| a.key.clone()));
        let a = &music.albums;
        let mut order: Vec<usize> = (0..a.len()).filter(|&i| self.singles || !a[i].singles).collect();
        match self.sort {
            Sort::Added => order.sort_by(|&x, &y| a[y].added_at.cmp(&a[x].added_at).then_with(|| natural_cmp(&a[x].title, &a[y].title))),
            Sort::Title => order.sort_by(|&x, &y| natural_cmp(&a[x].title, &a[y].title)),
            Sort::Artist => {} // the catalog's own order: artist, year, title
            Sort::Year => order.sort_by(|&x, &y| a[y].year.cmp(&a[x].year).then_with(|| natural_cmp(&a[x].artist, &a[y].artist))),
        }
        self.albums = order;
        let at = selected.flatten().and_then(|key| self.albums.iter().position(|&i| a[i].key == key)).unwrap_or(0);
        self.grid.select(at);
        self.built_for = Some((version, self.sort, self.singles));
    }

    fn tool_label(&self, tool: Tool) -> String {
        match tool {
            Tool::Sort => format!("Sort: {}", SORTS.iter().find(|s| s.0 == self.sort).map_or("", |s| s.1)),
            Tool::Show => if self.singles { "Show: Albums & singles" } else { "Show: Albums" }.to_owned(),
            Tool::Artists => "Artists".to_owned(),
            Tool::Folders => "Browse folders".to_owned(),
        }
    }

    fn activate_tool(&mut self, sh: &mut Shared, tool: Tool) -> Nav {
        match tool {
            Tool::Sort => {
                let i = SORTS.iter().position(|s| s.0 == self.sort).unwrap_or(0);
                let next = SORTS[(i + 1) % SORTS.len()];
                self.sort = next.0;
                sh.set_setting(&Self::sort_key(self.library_id), next.2);
                Nav::None
            }
            Tool::Show => {
                self.singles = !self.singles;
                sh.set_setting_bool(&Self::singles_key(self.library_id), self.singles);
                Nav::None
            }
            Tool::Artists => Nav::push(ArtistsView::new(self.library_id)),
            Tool::Folders => Nav::push(LibraryView::new(self.library_id, String::new())),
        }
    }
}

impl Screen for AlbumsView {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        self.refresh(sh);
        if let Some(at) = self.toolbar {
            match press.action {
                Action::Left => self.toolbar = Some(at.saturating_sub(1)),
                Action::Right => self.toolbar = Some((at + 1).min(TOOLS.len() - 1)),
                Action::Down => self.toolbar = None,
                Action::Up if !press.repeat => return Nav::FocusTopBar,
                Action::Back if !press.repeat => return Nav::Pop,
                _ if press.is_activate() => return self.activate_tool(sh, TOOLS[at]),
                _ => {}
            }
            return Nav::None;
        }
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        match self.grid.handle(press, self.albums.len()) {
            ListInput::AtTop => {
                self.toolbar = Some(0);
                Nav::None
            }
            ListInput::Activate(i) => match catalog(sh, self.library_id).and_then(|m| m.albums.get(self.albums[i]).map(|a| a.key.clone())) {
                Some(key) => Nav::push(AlbumPage::new(self.library_id, key)),
                None => Nav::None,
            },
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let Some(state) = sh.lib(self.library_id) else { return Nav::Pop };
        let (name, status) = (state.lib.name.clone(), state.status());
        let style = ui.style().clone();
        let body = widgets::header(ui.painter(), rect, &name, &status, &style);
        let toolbar = Rect::from_min_size(body.min, vec2(body.width(), TOOLBAR_H));
        let mut x = toolbar.left();
        let font = TextStyle::Small.resolve(&style);
        for (i, tool) in TOOLS.iter().enumerate() {
            let label = self.tool_label(*tool);
            let width = ui.painter().layout_no_wrap(label.clone(), font.clone(), Color32::WHITE).size().x + 44.0;
            let r = Rect::from_min_size(pos2(x, toolbar.top()), vec2(width, toolbar.height()));
            if widgets::hit(ui, r).any() {
                self.toolbar = Some(i);
            }
            let color = widgets::paint_focusable(ui.painter(), r, focused && self.toolbar == Some(i), false);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, label, font.clone(), color);
            x += width + 14.0;
        }
        let content = Rect::from_min_max(pos2(body.left(), toolbar.bottom() + 24.0), body.max);
        let Some(music) = catalog(sh, self.library_id) else {
            widgets::message(ui.painter(), content, "Reading the music… albums will appear shortly", &style);
            return Nav::None;
        };
        if self.albums.is_empty() {
            widgets::message(ui.painter(), content, "No music here", &style);
            return Nav::None;
        }
        let albums = self.albums.clone();
        if album_grid(ui, sh, content, &music, &albums, &mut self.grid, focused && self.toolbar.is_none(), |_, a| artist_year(a)).any() {
            self.toolbar = None;
        }
        Nav::None
    }
}

/// One album: cover and actions on the left, tracks on the right.
pub struct AlbumPage {
    library_id: i64,
    key: String,
    /// Left column (Play, Shuffle, artists) or right (tracks).
    on_tracks: bool,
    actions: ListState,
    tracks: ListState,
}

impl AlbumPage {
    pub fn new(library_id: i64, key: String) -> Self {
        Self { library_id, key, on_tracks: false, actions: ListState::default(), tracks: ListState::default() }
    }
}

impl Screen for AlbumPage {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let Some(music) = catalog(sh, self.library_id) else { return Nav::None };
        let Some((_, album)) = music.album(&self.key) else { return Nav::Pop };
        // Left/Right switch columns (lists use them for paging otherwise).
        match press.action {
            Action::Right if !self.on_tracks => {
                self.on_tracks = true;
                return Nav::None;
            }
            Action::Left if self.on_tracks => {
                self.on_tracks = false;
                return Nav::None;
            }
            _ => {}
        }
        if self.on_tracks {
            return match self.tracks.handle(press, album.tracks.len()) {
                ListInput::Activate(i) => play_album(sh, self.library_id, &self.key, i, false),
                ListInput::AtTop => Nav::FocusTopBar,
                ListInput::None => Nav::None,
            };
        }
        match self.actions.handle(press, 2 + album.artists.len()) {
            ListInput::Activate(0) => play_album(sh, self.library_id, &self.key, 0, false),
            ListInput::Activate(1) => play_album(sh, self.library_id, &self.key, 0, true),
            ListInput::Activate(i) => Nav::push(ArtistPage::new(self.library_id, album.artists[i - 2].clone())),
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(music) = catalog(sh, self.library_id) else { return Nav::None };
        let Some((_, album)) = music.album(&self.key) else { return Nav::Pop };
        let style = ui.style().clone();
        let left = Rect::from_min_size(rect.min, vec2(PAGE_COVER, rect.height()));
        let cover = Rect::from_min_size(left.min, vec2(PAGE_COVER, PAGE_COVER));
        match cover_path(album).and_then(|c| sh.images.get(&c, Size::Poster)) {
            Some(t) => paint_cover(ui.painter(), cover, &t),
            None => {
                ui.painter().rect_filled(cover, CornerRadius::same(8), theme::ROW);
                let galley = ui.painter().layout(album.title.clone(), TextStyle::Body.resolve(&style), theme::DIM, PAGE_COVER - 60.0);
                ui.painter().galley(cover.center() - galley.size() / 2.0, galley, theme::DIM);
            }
        }
        let painter = ui.painter().with_clip_rect(left);
        let mut y = cover.bottom() + 20.0;
        let title = painter.layout(album.title.clone(), TextStyle::Body.resolve(&style), theme::TEXT, PAGE_COVER);
        let title_h = title.size().y.min(80.0);
        painter.with_clip_rect(Rect::from_min_size(pos2(left.left(), y), vec2(PAGE_COVER, title_h))).galley(pos2(left.left(), y), title, theme::TEXT);
        y += title_h + 6.0;
        let mut facts = vec![album.artist.clone()];
        facts.extend(album.year.map(|y| y.to_string()));
        facts.push(format!("{} tracks · {}", album.tracks.len(), duration_text(album.duration())));
        painter.text(pos2(left.left(), y), Align2::LEFT_TOP, facts.join(" · "), TextStyle::Small.resolve(&style), theme::DIM);
        y += 44.0;

        let mut rows = vec![Row::new(Icon::Video, "Play"), Row::new(Icon::Refresh, "Shuffle")];
        rows.extend(album.artists.iter().map(|a| Row::new(Icon::None, a).detail("Artist")));
        let actions = Rect::from_min_max(pos2(left.left(), y), left.max);
        // Two lists on one page: each needs its own scroll ID.
        let hit = ui.push_id("actions", |ui| widgets::list(ui, actions, &rows, &mut self.actions, focused && !self.on_tracks)).inner;
        if hit.any() {
            self.on_tracks = false;
        }

        let discs = album.tracks.iter().filter_map(|t| t.disc).max().unwrap_or(1) > 1;
        let rows: Vec<Row> = album
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let number = match (discs, t.disc) {
                    (true, Some(d)) => format!("{d}-{:02}", t.number.unwrap_or(i as u32 + 1)),
                    _ => format!("{}", t.number.unwrap_or(i as u32 + 1)),
                };
                let others = !(t.artists.len() == 1 && t.artists[0].eq_ignore_ascii_case(&album.artist)) && !t.artists.is_empty();
                let time = theme::format_time(t.duration);
                let detail = if others { format!("{} · {time}", t.artists.join(", ")) } else { time };
                Row::new(Icon::None, format!("{number}. {}", t.title)).detail(detail)
            })
            .collect();
        let right = Rect::from_min_max(pos2(left.right() + 60.0, rect.top()), rect.max);
        let hit = ui.push_id("tracks", |ui| widgets::list(ui, right, &rows, &mut self.tracks, focused && self.on_tracks)).inner;
        if hit.any() {
            self.on_tracks = true;
        }
        Nav::None
    }
}

/// "3 albums · singles · on 2 more".
fn artist_summary(music: &MusicCatalog, artist: &flex_core::music::Artist) -> String {
    let albums = artist.albums.iter().filter(|&&i| !music.albums[i].singles).count();
    let mut parts = Vec::new();
    match albums {
        0 => {}
        1 => parts.push("1 album".to_owned()),
        n => parts.push(format!("{n} albums")),
    }
    if albums < artist.albums.len() {
        parts.push(if parts.is_empty() { "Singles" } else { "singles" }.to_owned());
    }
    if !artist.appears_on.is_empty() {
        parts.push(format!("on {} more", artist.appears_on.len()));
    }
    parts.join(" · ")
}

/// Everyone in a music library.
pub struct ArtistsView {
    library_id: i64,
    list: ListState,
}

impl ArtistsView {
    pub fn new(library_id: i64) -> Self {
        Self { library_id, list: ListState::default() }
    }
}

impl Screen for ArtistsView {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let Some(music) = catalog(sh, self.library_id) else { return Nav::None };
        match self.list.handle(press, music.artists.len()) {
            ListInput::Activate(i) => Nav::push(ArtistPage::new(self.library_id, music.artists[i].name.clone())),
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(music) = catalog(sh, self.library_id) else { return Nav::None };
        let subtitle = format!("{} artists", thousands(music.artists.len()));
        let body = widgets::header(ui.painter(), rect, "Artists", &subtitle, ui.style());
        let rows: Vec<Row> = music
            .artists
            .iter()
            .map(|a| Row::new(Icon::None, &a.name).detail(artist_summary(&music, a)))
            .collect();
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

/// An artist: their albums, then albums by others that they're on.
pub struct ArtistPage {
    library_id: i64,
    name: String,
    grid: GridState,
}

impl ArtistPage {
    pub fn new(library_id: i64, name: String) -> Self {
        Self { library_id, name, grid: GridState::default() }
    }

    fn albums(&self, music: &MusicCatalog) -> (Vec<usize>, usize) {
        let Some(artist) = music.artist(&self.name) else { return (Vec::new(), 0) };
        let own = artist.albums.len();
        (artist.albums.iter().chain(&artist.appears_on).copied().collect(), own)
    }
}

impl Screen for ArtistPage {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let Some(music) = catalog(sh, self.library_id) else { return Nav::None };
        let (albums, _) = self.albums(&music);
        match self.grid.handle(press, albums.len()) {
            ListInput::Activate(i) => Nav::push(AlbumPage::new(self.library_id, music.albums[albums[i]].key.clone())),
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(music) = catalog(sh, self.library_id) else { return Nav::None };
        let (albums, own) = self.albums(&music);
        let subtitle = music.artist(&self.name).map(|a| artist_summary(&music, a)).unwrap_or_default();
        let style = ui.style().clone();
        let name = music.artist(&self.name).map_or(self.name.clone(), |a| a.name.clone());
        let body = widgets::header(ui.painter(), rect, &name, &subtitle, &style);
        album_grid(ui, sh, body, &music, &albums, &mut self.grid, focused, |i, a| {
            if i < own { a.year.map(|y| y.to_string()).unwrap_or_default() } else { format!("On: {}", artist_year(a)) }
        });
        Nav::None
    }
}
