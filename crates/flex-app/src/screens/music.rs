//! A music library: a grid of albums (or artists), each album's page (tracks and
//! artists), and each artist's page (their albums and the ones they're on).

use eframe::egui::{self, Align2, Color32, CornerRadius, Painter, Rect, TextStyle, Ui, pos2, vec2};
use flex_core::music::{Album, Artist, MusicCatalog};
use flex_core::musicbrainz::ReleaseGroup;
use flex_core::natural::natural_cmp;

use super::library::LibraryView;
use super::{Nav, Screen};
use crate::images::{Size, paint_cover};
use crate::input::{Action, Press};
use crate::music_player::MusicPlayer;
use crate::shared::{LibState, Shared};
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

/// An album's cover: its own art, else MusicBrainz's.
fn cover_path(sh: &Shared, library_id: i64, album: &Album) -> Option<String> {
    sh.lib(library_id)?.album_cover(album)
}

fn duration_text(secs: f64) -> String {
    let mins = (secs / 60.0).round() as u64;
    if mins >= 60 { format!("{} h {} min", mins / 60, mins % 60) } else { format!("{mins} min") }
}

/// An album card: square cover (or the title on a tile), then the title and a second line.
#[allow(clippy::too_many_arguments)]
fn paint_card(painter: &Painter, sh: &mut Shared, rect: Rect, title: &str, cover: Option<&str>, sub: &str, focused: bool, style: &egui::Style) {
    let tile = Rect::from_min_size(rect.min, vec2(rect.width(), rect.width()));
    match cover.and_then(|c| sh.images.get(c, Size::Poster)) {
        Some(t) => paint_cover(painter, tile, &t),
        None => {
            painter.rect_filled(tile, CornerRadius::same(8), theme::panel());
            let galley = painter.layout(title.to_owned(), TextStyle::Body.resolve(style), theme::dim(), tile.width() - 30.0);
            painter.galley(tile.center() - galley.size() / 2.0, galley, theme::dim());
        }
    }
    theme::paint_picture_frame(painter, tile, focused);
    let text = Rect::from_min_max(pos2(rect.left(), tile.bottom() + 8.0), rect.max);
    let clip = painter.with_clip_rect(text);
    let color = if focused { theme::highlight() } else { theme::text() };
    clip.text(text.left_top(), Align2::LEFT_TOP, title, TextStyle::Small.resolve(style), color);
    clip.text(text.left_top() + vec2(0.0, 32.0), Align2::LEFT_TOP, sub, TextStyle::Small.resolve(style), theme::dim());
}

/// When any of the album's tracks was last played (0 = never).
fn album_played(state: &LibState, album: &Album) -> i64 {
    album.tracks.iter().filter_map(|t| state.tree.file(t.file_id)?.progress.last_played).max().unwrap_or(0)
}

/// The newest of an artist's albums (their own first) that has a cover.
fn artist_cover(sh: &Shared, library_id: i64, music: &MusicCatalog, artist: &Artist) -> Option<String> {
    artist.albums.iter().chain(&artist.appears_on).find_map(|&i| cover_path(sh, library_id, &music.albums[i]))
}

/// An artist's photo (from Wikimedia), else one of their covers.
fn artist_image(sh: &Shared, library_id: i64, music: &MusicCatalog, artist: &Artist) -> Option<String> {
    sh.artist_info(library_id, &artist.name).and_then(|i| i.image.clone()).or_else(|| artist_cover(sh, library_id, music, artist))
}

/// An artist card: their photo or one of their covers, then name and summary.
#[allow(clippy::too_many_arguments)]
fn paint_artist(painter: &Painter, sh: &mut Shared, library_id: i64, rect: Rect, music: &MusicCatalog, artist: &Artist, focused: bool, style: &egui::Style) {
    let image = artist_image(sh, library_id, music, artist);
    paint_card(painter, sh, rect, &artist.name, image.as_deref(), &artist_summary(music, artist), focused, style);
}

/// A row of toolbar buttons; hovering one focuses it (`toolbar`). Returns whether the
/// mouse is on one.
fn draw_toolbar(ui: &Ui, rect: Rect, labels: &[String], toolbar: &mut Option<usize>, focused: bool) -> bool {
    let font = TextStyle::Small.resolve(ui.style());
    let mut x = rect.left();
    let mut hovered = false;
    for (i, label) in labels.iter().enumerate() {
        let width = ui.painter().layout_no_wrap(label.clone(), font.clone(), Color32::WHITE).size().x + 44.0;
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(width, rect.height()));
        if widgets::hit(ui, r).any() {
            *toolbar = Some(i);
            hovered = true;
        }
        let color = widgets::paint_focusable(ui.painter(), r, focused && *toolbar == Some(i), false);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, font.clone(), color);
        x += width + 14.0;
    }
    hovered
}

/// Moves between toolbar buttons; `None` = not a toolbar press (or it left the toolbar).
fn toolbar_press(press: Press, at: usize, count: usize, toolbar: &mut Option<usize>) -> Option<Nav> {
    match press.action {
        Action::Left => *toolbar = Some(at.saturating_sub(1)),
        Action::Right => *toolbar = Some((at + 1).min(count - 1)),
        Action::Down => *toolbar = None,
        Action::Up if !press.repeat => return Some(Nav::FocusTopBar),
        Action::Back if !press.repeat => return Some(Nav::Pop),
        _ => return None,
    }
    Some(Nav::None)
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
    library_id: i64,
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
        let cover = cover_path(sh, library_id, album);
        paint_card(painter, sh, r, &album.title, cover.as_deref(), &sub(i, album), f, &style);
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Added,
    Played,
    Artist,
    Title,
    Year,
}

const SORTS: [(Sort, &str, &str); 5] = [
    (Sort::Added, "Recently added", "added"),
    (Sort::Played, "Recently played", "played"),
    (Sort::Artist, "Artist", "artist"),
    (Sort::Title, "A–Z", "title"),
    (Sort::Year, "Year", "year"),
];

/// What the library grid shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Show {
    Albums,
    /// Albums and folders of loose singles.
    Singles,
    Artists,
}

const SHOWS: [(Show, &str, &str); 3] =
    [(Show::Albums, "Albums", "albums"), (Show::Singles, "Albums & singles", "singles"), (Show::Artists, "Artists", "artists")];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Sort,
    Show,
    Folders,
}

const TOOLS: [Tool; 3] = [Tool::Sort, Tool::Show, Tool::Folders];

/// A music library: albums, or artists.
pub struct AlbumsView {
    library_id: i64,
    sort: Sort,
    show: Show,
    /// `Some(i)` = toolbar button i has focus.
    toolbar: Option<usize>,
    grid: GridState,
    /// Indexes of albums, or of artists when showing artists.
    items: Vec<usize>,
    built_for: Option<(u64, Sort, Show)>,
}

impl AlbumsView {
    pub fn new(sh: &Shared, library_id: i64) -> Self {
        let pick = |key: String| sh.setting(&key).unwrap_or_default();
        let sort = SORTS.iter().find(|s| s.2 == pick(format!("music_sort:{library_id}"))).map_or(Sort::Added, |s| s.0);
        let show = SHOWS.iter().find(|s| s.2 == pick(format!("music_show:{library_id}"))).map_or(Show::Albums, |s| s.0);
        Self { library_id, sort, show, toolbar: None, grid: GridState::default(), items: Vec::new(), built_for: None }
    }

    fn refresh(&mut self, sh: &Shared) {
        let Some(state) = sh.lib(self.library_id) else { return };
        if self.built_for == Some((state.version, self.sort, self.show)) {
            return;
        }
        let Some(music) = state.music.clone() else { return };
        // Keep the same thing selected across rebuilds.
        let key = |show: Show, i: usize| match show {
            Show::Artists => music.artists.get(i).map(|a| a.name.clone()),
            _ => music.albums.get(i).map(|a| a.key.clone()),
        };
        let previous = self.built_for.map(|(_, _, show)| show);
        let selected = previous.and_then(|show| self.items.get(self.grid.selected).and_then(|&i| key(show, i)));
        let a = &music.albums;
        let played: Vec<i64> = a.iter().map(|album| album_played(state, album)).collect();
        self.items = match self.show {
            Show::Artists => {
                let artists = &music.artists;
                let newest = |list: &[usize], f: &dyn Fn(usize) -> i64| list.iter().map(|&i| f(i)).max().unwrap_or(0);
                let by = |x: &Artist, f: &dyn Fn(usize) -> i64| newest(&x.albums, f).max(newest(&x.appears_on, f));
                let mut order: Vec<usize> = (0..artists.len()).collect();
                match self.sort {
                    Sort::Added => order.sort_by_key(|&i| std::cmp::Reverse(by(&artists[i], &|j| a[j].added_at))),
                    Sort::Played => order.sort_by_key(|&i| std::cmp::Reverse(by(&artists[i], &|j| played[j]))),
                    Sort::Year => order.sort_by_key(|&i| std::cmp::Reverse(by(&artists[i], &|j| i64::from(a[j].year.unwrap_or(0))))),
                    Sort::Artist | Sort::Title => {} // the catalog's own order: by name
                }
                order
            }
            show => {
                let mut order: Vec<usize> = (0..a.len()).filter(|&i| show == Show::Singles || !a[i].singles).collect();
                match self.sort {
                    Sort::Added => order.sort_by(|&x, &y| a[y].added_at.cmp(&a[x].added_at).then_with(|| natural_cmp(&a[x].title, &a[y].title))),
                    Sort::Played => order.sort_by(|&x, &y| played[y].cmp(&played[x]).then_with(|| natural_cmp(&a[x].title, &a[y].title))),
                    Sort::Title => order.sort_by(|&x, &y| natural_cmp(&a[x].title, &a[y].title)),
                    Sort::Artist => {} // the catalog's own order: artist, year, title
                    Sort::Year => order.sort_by(|&x, &y| a[y].year.cmp(&a[x].year).then_with(|| natural_cmp(&a[x].artist, &a[y].artist))),
                }
                order
            }
        };
        let at = selected.and_then(|k| self.items.iter().position(|&i| key(self.show, i).as_ref() == Some(&k)));
        self.grid.select(at.unwrap_or(0));
        self.built_for = Some((state.version, self.sort, self.show));
    }

    fn tool_label(&self, tool: Tool) -> String {
        match tool {
            Tool::Sort => {
                // Sorting artists "A–Z" and by "Artist" is the same thing.
                let label = SORTS.iter().find(|s| s.0 == self.sort).map_or("", |s| s.1);
                let label = if self.show == Show::Artists && self.sort == Sort::Artist { "A–Z" } else { label };
                format!("Sort: {label}")
            }
            Tool::Show => format!("Show: {}", SHOWS.iter().find(|s| s.0 == self.show).map_or("", |s| s.1)),
            Tool::Folders => "Browse folders".to_owned(),
        }
    }

    fn activate_tool(&mut self, sh: &mut Shared, tool: Tool) -> Nav {
        match tool {
            Tool::Sort => {
                let i = SORTS.iter().position(|s| s.0 == self.sort).unwrap_or(0);
                let mut next = SORTS[(i + 1) % SORTS.len()];
                if self.show == Show::Artists && next.0 == Sort::Title {
                    next = SORTS[(i + 2) % SORTS.len()]; // same as Artist there
                }
                self.sort = next.0;
                sh.set_setting(&format!("music_sort:{}", self.library_id), next.2);
                Nav::None
            }
            Tool::Show => {
                let i = SHOWS.iter().position(|s| s.0 == self.show).unwrap_or(0);
                let next = SHOWS[(i + 1) % SHOWS.len()];
                self.show = next.0;
                sh.set_setting(&format!("music_show:{}", self.library_id), next.2);
                Nav::None
            }
            Tool::Folders => Nav::push(LibraryView::new(self.library_id, String::new())),
        }
    }
}

impl Screen for AlbumsView {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        self.refresh(sh);
        if let Some(at) = self.toolbar {
            if press.is_activate() {
                return self.activate_tool(sh, TOOLS[at]);
            }
            return toolbar_press(press, at, TOOLS.len(), &mut self.toolbar).unwrap_or(Nav::None);
        }
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        match self.grid.handle(press, self.items.len()) {
            ListInput::AtTop => {
                self.toolbar = Some(0);
                Nav::None
            }
            ListInput::Activate(i) => {
                let Some(music) = catalog(sh, self.library_id) else { return Nav::None };
                let item = self.items[i];
                match self.show {
                    Show::Artists => music.artists.get(item).map_or(Nav::None, |a| Nav::push(ArtistPage::new(sh, self.library_id, a.name.clone()))),
                    _ => music.albums.get(item).map_or(Nav::None, |a| Nav::push(AlbumPage::new(self.library_id, a.key.clone()))),
                }
            }
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
        let labels: Vec<String> = TOOLS.iter().map(|t| self.tool_label(*t)).collect();
        draw_toolbar(ui, toolbar, &labels, &mut self.toolbar, focused);
        let content = Rect::from_min_max(pos2(body.left(), toolbar.bottom() + 24.0), body.max);
        let Some(music) = catalog(sh, self.library_id) else {
            widgets::message(ui.painter(), content, "Reading the music… albums will appear shortly", &style);
            return Nav::None;
        };
        if self.items.is_empty() {
            widgets::message(ui.painter(), content, "No music here", &style);
            return Nav::None;
        }
        let items = self.items.clone();
        let grid_focused = focused && self.toolbar.is_none();
        let hit = match self.show {
            Show::Artists => widgets::grid(ui, content, items.len(), GRID, &mut self.grid, grid_focused, |painter, i, r, f| {
                paint_artist(painter, sh, self.library_id, r, &music, &music.artists[items[i]], f, &style);
            }),
            _ => album_grid(ui, sh, self.library_id, content, &music, &items, &mut self.grid, grid_focused, |_, a| artist_year(a)),
        };
        if hit.any() {
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
        let artists = album.artists.len();
        match self.actions.handle(press, 3 + artists) {
            ListInput::Activate(0) => play_album(sh, self.library_id, &self.key, 0, false),
            ListInput::Activate(1) => play_album(sh, self.library_id, &self.key, 0, true),
            ListInput::Activate(i) if i < 2 + artists => Nav::push(ArtistPage::new(sh, self.library_id, album.artists[i - 2].clone())),
            ListInput::Activate(_) => Nav::push(MatchAlbum::new(sh, self.library_id, album)),
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
        match cover_path(sh, self.library_id, album).and_then(|c| sh.images.get(&c, Size::Poster)) {
            Some(t) => paint_cover(ui.painter(), cover, &t),
            None => {
                ui.painter().rect_filled(cover, CornerRadius::same(8), theme::panel());
                let galley = ui.painter().layout(album.title.clone(), TextStyle::Body.resolve(&style), theme::dim(), PAGE_COVER - 60.0);
                ui.painter().galley(cover.center() - galley.size() / 2.0, galley, theme::dim());
            }
        }
        let painter = ui.painter().with_clip_rect(left);
        let mut y = cover.bottom() + 20.0;
        let title = painter.layout(album.title.clone(), TextStyle::Body.resolve(&style), theme::text(), PAGE_COVER);
        let title_h = title.size().y.min(80.0);
        painter.with_clip_rect(Rect::from_min_size(pos2(left.left(), y), vec2(PAGE_COVER, title_h))).galley(pos2(left.left(), y), title, theme::text());
        y += title_h + 6.0;
        let mut facts = vec![album.artist.clone()];
        facts.extend(album.year.map(|y| y.to_string()));
        facts.push(format!("{} tracks · {}", album.tracks.len(), duration_text(album.duration())));
        painter.text(pos2(left.left(), y), Align2::LEFT_TOP, facts.join(" · "), TextStyle::Small.resolve(&style), theme::dim());
        y += 44.0;

        let mut rows = vec![Row::new(Icon::Video, "Play"), Row::new(Icon::Refresh, "Shuffle")];
        rows.extend(album.artists.iter().map(|a| Row::new(Icon::None, a).detail("Artist")));
        let matched = sh.lib(self.library_id).and_then(|l| l.music_matches.get(&self.key).cloned());
        let status = match matched {
            Some(m) => match m.release_group {
                Some(rg) => format!("Matched: {}", [rg.kind_label(), rg.year.map(|y| y.to_string()).unwrap_or_default()].join(" ").trim()),
                None => "Not found".to_owned(),
            },
            None => String::new(),
        };
        rows.push(Row::new(Icon::Refresh, "Match online…").detail(status));
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArtistSort {
    Newest,
    Oldest,
    Title,
    Played,
}

const ARTIST_SORTS: [(ArtistSort, &str, &str); 4] = [
    (ArtistSort::Newest, "Newest first", "newest"),
    (ArtistSort::Oldest, "Oldest first", "oldest"),
    (ArtistSort::Title, "A–Z", "title"),
    (ArtistSort::Played, "Recently played", "played"),
];
const ARTIST_SORT_SETTING: &str = "artist_sort";
const PHOTO: f32 = 220.0;

/// A card on an artist's page.
enum Tile {
    /// One of the library's albums (index into the catalog).
    Local(usize),
    /// One of their albums that isn't in the library (from MusicBrainz).
    Remote(ReleaseGroup),
}

/// An artist: their albums, the ones they're on, then (from MusicBrainz) the rest of
/// their albums.
pub struct ArtistPage {
    library_id: i64,
    name: String,
    sort: ArtistSort,
    /// The Sort button has focus.
    on_toolbar: bool,
    grid: GridState,
}

impl ArtistPage {
    pub fn new(sh: &Shared, library_id: i64, name: String) -> Self {
        let saved = sh.setting(ARTIST_SORT_SETTING).unwrap_or_default();
        let sort = ARTIST_SORTS.iter().find(|s| s.2 == saved).map_or(ArtistSort::Newest, |s| s.0);
        Self { library_id, name, sort, on_toolbar: false, grid: GridState::default() }
    }

    /// Their albums, the ones they're on, then the rest of their discography, each in the
    /// chosen order; and how many of the first two kinds there are.
    fn tiles(&self, sh: &Shared, music: &MusicCatalog) -> (Vec<Tile>, usize, usize) {
        let (Some(artist), Some(state)) = (music.artist(&self.name), sh.lib(self.library_id)) else { return (Vec::new(), 0, 0) };
        let a = &music.albums;
        let sorted = |list: &[usize]| {
            let mut list = list.to_vec();
            match self.sort {
                ArtistSort::Newest => list.sort_by(|&x, &y| a[y].year.cmp(&a[x].year).then_with(|| natural_cmp(&a[x].title, &a[y].title))),
                ArtistSort::Oldest => list.sort_by(|&x, &y| {
                    // Undated last.
                    a[x].year.is_none().cmp(&a[y].year.is_none()).then_with(|| a[x].year.cmp(&a[y].year)).then_with(|| natural_cmp(&a[x].title, &a[y].title))
                }),
                ArtistSort::Title => list.sort_by(|&x, &y| natural_cmp(&a[x].title, &a[y].title)),
                ArtistSort::Played => list.sort_by_key(|&i| std::cmp::Reverse(album_played(state, &a[i]))),
            }
            list
        };
        let own = sorted(&artist.albums);
        let on = sorted(&artist.appears_on);
        let (own_count, local_count) = (own.len(), own.len() + on.len());
        let mut tiles: Vec<Tile> = own.into_iter().chain(on).map(Tile::Local).collect();
        // The rest of their albums, from MusicBrainz.
        if let Some(info) = sh.artist_info(self.library_id, &artist.name) {
            let have: std::collections::HashSet<&str> =
                state.music_matches.values().filter_map(|m| m.release_group.as_ref()).map(|rg| rg.id.as_str()).collect();
            let mut rest: Vec<&ReleaseGroup> = info.discography.iter().filter(|rg| !have.contains(rg.id.as_str())).collect();
            match self.sort {
                ArtistSort::Oldest => {} // already oldest first
                ArtistSort::Title => rest.sort_by(|x, y| natural_cmp(&x.title, &y.title)),
                ArtistSort::Newest | ArtistSort::Played => rest.reverse(),
            }
            tiles.extend(rest.into_iter().cloned().map(Tile::Remote));
        }
        (tiles, own_count, local_count)
    }

    fn sort_label(&self) -> String {
        format!("Sort: {}", ARTIST_SORTS.iter().find(|s| s.0 == self.sort).map_or("", |s| s.1))
    }
}

impl Screen for ArtistPage {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if self.on_toolbar {
            if press.is_activate() {
                let i = ARTIST_SORTS.iter().position(|s| s.0 == self.sort).unwrap_or(0);
                let next = ARTIST_SORTS[(i + 1) % ARTIST_SORTS.len()];
                self.sort = next.0;
                self.grid.select(0);
                sh.set_setting(ARTIST_SORT_SETTING, next.2);
                return Nav::None;
            }
            let mut toolbar = Some(0);
            let nav = toolbar_press(press, 0, 1, &mut toolbar).unwrap_or(Nav::None);
            self.on_toolbar = toolbar.is_some();
            return nav;
        }
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let Some(music) = catalog(sh, self.library_id) else { return Nav::None };
        let (tiles, _, _) = self.tiles(sh, &music);
        match self.grid.handle(press, tiles.len()) {
            ListInput::Activate(i) => match &tiles[i] {
                Tile::Local(album) => Nav::push(AlbumPage::new(self.library_id, music.albums[*album].key.clone())),
                Tile::Remote(rg) => {
                    sh.toast(format!("{} isn't in your library", rg.title));
                    Nav::None
                }
            },
            ListInput::AtTop => {
                self.on_toolbar = true;
                Nav::None
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(music) = catalog(sh, self.library_id) else { return Nav::None };
        let (tiles, own, local) = self.tiles(sh, &music);
        let mut subtitle = music.artist(&self.name).map(|a| artist_summary(&music, a)).unwrap_or_default();
        if tiles.len() > local {
            subtitle.push_str(&format!(" · {} more not in your library", tiles.len() - local));
        }
        let style = ui.style().clone();
        let name = music.artist(&self.name).map_or(self.name.clone(), |a| a.name.clone());
        // Their photo, if Wikimedia has one, beside the name.
        let photo = sh.artist_info(self.library_id, &self.name).and_then(|i| i.image.clone());
        let texture = photo.and_then(|p| sh.images.get(&p, Size::Profile));
        let text_left = if let Some(t) = &texture {
            paint_cover(ui.painter(), Rect::from_min_size(rect.min, vec2(PHOTO, PHOTO)), t);
            rect.left() + PHOTO + 36.0
        } else {
            rect.left()
        };
        let body = widgets::header(ui.painter(), Rect::from_min_max(pos2(text_left, rect.top()), rect.max), &name, &subtitle, &style);
        let toolbar = Rect::from_min_size(body.min, vec2(body.width(), TOOLBAR_H));
        let mut at = self.on_toolbar.then_some(0);
        draw_toolbar(ui, toolbar, &[self.sort_label()], &mut at, focused);
        self.on_toolbar = at.is_some();
        let top = if texture.is_some() { toolbar.bottom().max(rect.top() + PHOTO) } else { toolbar.bottom() };
        let content = Rect::from_min_max(pos2(rect.left(), top + 24.0), rect.max);
        let library_id = self.library_id;
        let hit = widgets::grid(ui, content, tiles.len(), GRID, &mut self.grid, focused && !self.on_toolbar, |painter, i, r, f| match &tiles[i] {
            Tile::Local(index) => {
                let album = &music.albums[*index];
                let cover = cover_path(sh, library_id, album);
                let sub = if i < own { album.year.map(|y| y.to_string()).unwrap_or_default() } else { format!("On: {}", artist_year(album)) };
                paint_card(painter, sh, r, &album.title, cover.as_deref(), &sub, f, &style);
            }
            Tile::Remote(rg) => {
                let cover = flex_core::musicbrainz::cover_url(&rg.id);
                let year = rg.year.map(|y| format!(" · {y}")).unwrap_or_default();
                let sub = format!("Not in your library{year}");
                paint_card(painter, sh, r, &rg.title, Some(&cover), &sub, f, &style);
            }
        });
        if hit.any() {
            self.on_toolbar = false;
        }
        Nav::None
    }
}

/// What a background search found (or why it failed), once it's done.
type SearchResults = std::sync::Arc<std::sync::Mutex<Option<Result<Vec<ReleaseGroup>, String>>>>;

/// Searching MusicBrainz for an album, to pick its match by hand.
pub struct MatchAlbum {
    library_id: i64,
    key: String,
    title: String,
    artist: String,
    /// Filled in by the search thread.
    results: SearchResults,
    list: ListState,
}

impl MatchAlbum {
    pub fn new(sh: &Shared, library_id: i64, album: &Album) -> Self {
        let results = SearchResults::default();
        let (out, ctx) = (std::sync::Arc::clone(&results), sh.ctx.clone());
        let (title, artist) = (album.title.clone(), album.artist.clone());
        let (t, a) = (title.clone(), (artist != "Various Artists").then(|| artist.clone()));
        std::thread::spawn(move || {
            let mb = flex_core::musicbrainz::MusicBrainz::new();
            let found = (|| -> anyhow::Result<Vec<ReleaseGroup>> {
                let mut found: Vec<ReleaseGroup> = mb.search(&t, a.as_deref())?.into_iter().map(|(rg, _)| rg).collect();
                // Few hits: the artist may be spelled differently there, so try the title alone.
                if found.len() < 5 {
                    for (rg, _) in mb.search(&t, None)? {
                        if !found.iter().any(|f| f.id == rg.id) {
                            found.push(rg);
                        }
                    }
                }
                Ok(found)
            })();
            if let Ok(mut r) = out.lock() {
                *r = Some(found.map_err(|e| format!("{e:#}")));
            }
            ctx.request_repaint();
        });
        let mut list = ListState::default();
        list.select(1); // the first result, not "Not on MusicBrainz"
        Self { library_id, key: album.key.clone(), title, artist, results, list }
    }

    fn found(&self) -> Vec<ReleaseGroup> {
        match self.results.lock().ok().as_deref() {
            Some(Some(Ok(found))) => found.clone(),
            _ => Vec::new(),
        }
    }
}

impl Screen for MatchAlbum {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        // Nothing to choose from until the search is back (the list isn't even shown).
        if !matches!(self.results.lock().ok().as_deref(), Some(Some(Ok(_)))) {
            return Nav::None;
        }
        let found = self.found();
        match self.list.handle(press, 1 + found.len()) {
            ListInput::Activate(0) => {
                sh.set_album_match(self.library_id, &self.key, None);
                sh.toast(format!("{}: not matched", self.title));
                Nav::Pop
            }
            ListInput::Activate(i) => {
                sh.set_album_match(self.library_id, &self.key, Some(&found[i - 1]));
                Nav::Pop
            }
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let subtitle = format!("{} · {}", self.title, self.artist);
        let body = widgets::header(ui.painter(), rect, "Match on MusicBrainz", &subtitle, ui.style());
        let results = self.results.lock().ok().and_then(|r| r.clone());
        let found = match results {
            Some(Ok(found)) => found,
            waiting_or_failed => {
                let message = match waiting_or_failed {
                    Some(Err(e)) => format!("Couldn't search: {e}"),
                    _ => "Searching…".to_owned(),
                };
                widgets::message(ui.painter(), body, &message, ui.style());
                return Nav::None;
            }
        };
        let current = sh.lib(self.library_id).and_then(|l| l.music_matches.get(&self.key)?.release_group.as_ref().map(|rg| rg.id.clone()));
        let mut rows = vec![Row::new(Icon::Remove, "Not on MusicBrainz").detail("Don't match")];
        rows.extend(found.iter().map(|rg| {
            let artists: Vec<&str> = rg.artists.iter().map(|(n, _)| n.as_str()).collect();
            let mut detail = [rg.kind_label(), rg.year.map(|y| y.to_string()).unwrap_or_default()].join(" ").trim().to_owned();
            if current.as_deref() == Some(rg.id.as_str()) {
                detail = format!("current · {detail}");
            }
            Row::new(Icon::None, format!("{} — {}", rg.title, artists.join(", "))).detail(detail)
        }));
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
