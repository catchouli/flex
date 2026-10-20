//! YouTube: subscriptions' latest videos, channels, search, a video's options, and
//! Settings → YouTube. Videos play in Flex's player through yt-dlp (see `crate::youtube`).

use std::collections::HashMap;

use eframe::egui::{self, Align2, Color32, CornerRadius, Painter, Rect, TextStyle, Ui, pos2, vec2};
use flex_core::youtube::feeds::{Listed, thumbnail};
use flex_core::youtube::sponsorblock::CATEGORIES;
use flex_core::youtube::store::{Video, WatchProgress};
use flex_core::youtube::tools::{Installed, Tool};

use super::picker::{FolderPicker, Purpose};
use super::{Nav, Screen, text_entry::TextEntry};
use crate::images::{Size, paint_cover};
use crate::input::{Action, Press};
use crate::shared::{PlayRequest, Shared, now_unix};
use crate::widgets::{self, GridSpec, GridState, Icon, ListInput, ListState, Row};
use crate::youtube::{CONTINUE_LIMITS, QUALITIES, ago};
use crate::theme;

const THUMB: egui::Vec2 = egui::vec2(400.0, 225.0);
const GRID: GridSpec = GridSpec { card: egui::vec2(400.0, 225.0 + 84.0), gap: 26.0 };
const BUTTON: egui::Vec2 = egui::vec2(260.0, 64.0);
const BUTTONS_H: f32 = 96.0;
const LATEST: usize = 300;

/// "1 channel", "3 channels".
fn count(n: usize, thing: &str) -> String {
    format!("{n} {thing}{}", if n == 1 { "" } else { "s" })
}

/// A video as shown in a grid.
#[derive(Clone)]
pub struct VideoItem {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub channel_id: String,
    /// Under the title, e.g. "Blender · 3 days ago".
    detail: String,
    /// How far through a part-watched video is.
    fraction: Option<f32>,
    watched: bool,
}

impl VideoItem {
    fn play(&self) -> Nav {
        Nav::Play { req: PlayRequest::youtube(&self.id, &self.title, None), replace: false }
    }
}

fn with_progress(mut item: VideoItem, progress: Option<&WatchProgress>) -> VideoItem {
    if let Some(p) = progress {
        item.watched = p.watched;
        item.fraction = match (p.position, p.duration) {
            (Some(pos), Some(dur)) if dur > 0.0 => Some((pos / dur) as f32),
            _ => None,
        };
    }
    item
}

fn from_store(videos: Vec<Video>, progress: &HashMap<String, WatchProgress>) -> Vec<VideoItem> {
    let now = now_unix();
    videos
        .into_iter()
        .map(|v| {
            let detail = format!("{} · {}", v.channel, ago(v.published, now));
            let saved = progress.get(&v.id);
            let item = VideoItem { title: v.title, channel: v.channel, channel_id: v.channel_id, detail, fraction: None, watched: false, id: v.id };
            with_progress(item, saved)
        })
        .collect()
}

fn from_listed(listed: &[Listed], progress: &HashMap<String, WatchProgress>) -> Vec<VideoItem> {
    listed
        .iter()
        .map(|l| {
            let detail = match l.duration {
                Some(d) => format!("{} · {}", l.channel, theme::format_time(d)),
                None => l.channel.clone(),
            };
            let item = VideoItem { id: l.id.clone(), title: l.title.clone(), channel: l.channel.clone(), channel_id: l.channel_id.clone(), detail, fraction: None, watched: false };
            with_progress(item, progress.get(&l.id))
        })
        .collect()
}

fn paint_video(painter: &Painter, rect: Rect, item: &VideoItem, thumb: Option<&egui::TextureHandle>, focused: bool, style: &egui::Style) {
    let thumb_rect = Rect::from_min_size(rect.min, THUMB);
    match thumb {
        Some(texture) => paint_cover(painter, thumb_rect, texture),
        None => {
            painter.rect_filled(thumb_rect, CornerRadius::same(8), theme::panel());
        }
    }
    if item.watched {
        painter.rect_filled(thumb_rect, CornerRadius::same(8), Color32::from_black_alpha(110));
        widgets::paint_tick(painter, thumb_rect.right_top() + vec2(-26.0, 26.0));
    } else if let Some(fraction) = item.fraction {
        widgets::paint_progress(painter, thumb_rect, fraction, theme::progress());
    }
    theme::paint_picture_frame(painter, thumb_rect, focused);
    let color = if focused { theme::highlight() } else { theme::text() };
    let text = Rect::from_min_max(pos2(rect.left(), thumb_rect.bottom() + 8.0), rect.max);
    widgets::text_block(painter, Rect::from_min_size(text.min, vec2(text.width(), 44.0)), &item.title, TextStyle::Small.resolve(style), color);
    painter.with_clip_rect(text).text(pos2(text.left(), text.bottom() - 4.0), Align2::LEFT_BOTTOM, &item.detail, TextStyle::Small.resolve(style), theme::dim());
}

/// A row of buttons above a grid of videos, shared by the YouTube screens.
#[derive(Default)]
struct VideoGrid {
    grid: GridState,
    /// Focus is on the buttons (which one), not the grid.
    button: Option<usize>,
    /// The video last asked to be prepared.
    prepared: Option<String>,
    /// The first few videos have been prepared (once, when the grid first shows them).
    warmed: bool,
}

/// Videos prepared as soon as a grid appears, since the first ones are the likeliest picks.
const WARM: usize = 4;

enum GridInput {
    None,
    Nav(Nav),
    Button(usize),
    Play(usize),
    Options(usize),
}

impl VideoGrid {
    fn handle(&mut self, press: Press, buttons: usize, len: usize) -> GridInput {
        if let Some(b) = self.button {
            return match press.action {
                Action::Left => {
                    self.button = Some(b.saturating_sub(1));
                    GridInput::None
                }
                Action::Right => {
                    self.button = Some((b + 1).min(buttons.saturating_sub(1)));
                    GridInput::None
                }
                Action::Up if !press.repeat => GridInput::Nav(Nav::FocusTopBar),
                Action::Down if len > 0 => {
                    self.button = None;
                    GridInput::None
                }
                _ if press.is_activate() => GridInput::Button(b),
                _ => GridInput::None,
            };
        }
        if press.is(Action::Menu) && len > 0 {
            return GridInput::Options(self.grid.selected.min(len - 1));
        }
        match self.grid.handle(press, len) {
            ListInput::AtTop if buttons > 0 => {
                self.button = Some(0);
                GridInput::None
            }
            ListInput::AtTop => GridInput::Nav(Nav::FocusTopBar),
            ListInput::Activate(i) => GridInput::Play(i),
            ListInput::None => GridInput::None,
        }
    }

    /// Draws the buttons and the grid; starts preparing the selected video. Returns whether
    /// the grid was clicked.
    #[allow(clippy::too_many_arguments)]
    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, body: Rect, buttons: &[&str], items: &[VideoItem], empty: &str, focused: bool) -> bool {
        let style = ui.style().clone();
        if items.is_empty() && self.button.is_none() && !buttons.is_empty() {
            self.button = Some(0);
        }
        let mut top = body.top();
        if !buttons.is_empty() {
            let font = TextStyle::Body.resolve(&style);
            let mut x = body.left();
            for (i, label) in buttons.iter().enumerate() {
                // Wide enough for the label (e.g. "Showing: all channels").
                let text_w = ui.painter().layout_no_wrap((*label).to_owned(), font.clone(), theme::text()).size().x;
                let r = Rect::from_min_size(pos2(x, top), vec2(BUTTON.x.max(text_w + 56.0), BUTTON.y));
                x = r.right() + 20.0;
                if widgets::hit(ui, r).any() {
                    self.button = Some(i);
                }
                widgets::button(ui.painter(), r, label, focused && self.button == Some(i), &style);
            }
            top += BUTTONS_H;
        }
        let area = Rect::from_min_max(pos2(body.left(), top), body.max);
        if items.is_empty() {
            widgets::message(ui.painter(), area, empty, &style);
            return false;
        }
        if !std::mem::replace(&mut self.warmed, true) {
            for item in items.iter().take(WARM) {
                sh.prepare_youtube(&item.id);
            }
        }
        let images = &mut sh.images;
        let grid_focused = focused && self.button.is_none();
        let hit = widgets::grid(ui, area, items.len(), GRID, &mut self.grid, grid_focused, |painter, i, r, f| {
            let thumb = images.get(&thumbnail(&items[i].id), Size::Still);
            paint_video(painter, r, &items[i], thumb.as_ref(), f, &style);
        });
        if hit.any() {
            self.button = None;
        }
        if grid_focused && let Some(item) = items.get(self.grid.selected) && self.prepared.as_deref() != Some(&item.id) {
            self.prepared = Some(item.id.clone());
            sh.prepare_youtube(&item.id);
        }
        hit.any()
    }
}

/// What a video grid's OK and hold-OK do.
fn video_input(input: GridInput, items: &[VideoItem]) -> Option<Nav> {
    Some(match input {
        GridInput::Nav(nav) => nav,
        GridInput::Play(i) => items.get(i)?.play(),
        GridInput::Options(i) => Nav::push(VideoOptions::new(items.get(i)?.clone())),
        GridInput::None | GridInput::Button(_) => return None,
    })
}

/// YouTube's main page: part-watched videos, then the latest from subscriptions.
#[derive(Default)]
pub struct YouTubeHome {
    view: VideoGrid,
    items: Vec<VideoItem>,
    loaded: Option<u64>,
}

/// Search, Channels, History, and which channels' videos to show.
fn home_buttons(sh: &Shared) -> [&'static str; 4] {
    ["Search…", "Channels", "History", if sh.youtube_focused_only() { "Showing: focused" } else { "Showing: all channels" }]
}

impl YouTubeHome {
    fn refresh(&mut self, sh: &Shared) {
        if self.loaded == Some(sh.youtube.version) {
            return;
        }
        self.loaded = Some(sh.youtube.version);
        let progress = sh.yt_progress_all();
        let mut items: Vec<VideoItem> = sh
            .yt_continue(20)
            .into_iter()
            .map(|(id, p)| {
                let left = match (p.position, p.duration) {
                    (Some(pos), Some(dur)) => format!(" · {} left", theme::format_time(dur - pos)),
                    _ => String::new(),
                };
                let detail = format!("{}{left}", p.channel);
                let item = VideoItem { id, title: p.title.clone(), channel: p.channel.clone(), channel_id: String::new(), detail, fraction: None, watched: false };
                with_progress(item, Some(&p))
            })
            .collect();
        let continuing: Vec<String> = items.iter().map(|v| v.id.clone()).collect();
        items.extend(from_store(sh.yt_latest(LATEST), &progress).into_iter().filter(|v| !continuing.contains(&v.id)));
        self.items = items;
    }
}

impl Screen for YouTubeHome {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        self.refresh(sh);
        match self.view.handle(press, home_buttons(sh).len(), self.items.len()) {
            GridInput::Button(0) => Nav::push(TextEntry::youtube_search()),
            GridInput::Button(1) => Nav::push(ChannelList::default()),
            GridInput::Button(2) => Nav::push(YouTubeHistory::default()),
            GridInput::Button(_) => {
                let on = !sh.youtube_focused_only();
                sh.set_youtube_focused_only(on);
                Nav::None
            }
            input => video_input(input, &self.items).unwrap_or(Nav::None),
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let channels = sh.yt_channels().len();
        let subtitle = match (sh.youtube.tools.is_some(), channels, sh.youtube.refreshing) {
            (false, _, _) if sh.youtube.tools_busy => "Setting up (downloading yt-dlp)…".to_owned(),
            (false, _, _) => "Not set up: switch YouTube on in Settings → YouTube".to_owned(),
            (true, 0, _) => "Search, or subscribe to channels in Channels".to_owned(),
            (true, n, true) => format!("{} · checking for new videos…", count(n, "channel")),
            (true, n, false) => format!("{} · no ads, sponsors skipped", count(n, "channel")),
        };
        let body = widgets::header(ui.painter(), rect, "YouTube", &subtitle, ui.style());
        let items = std::mem::take(&mut self.items);
        let empty = match (channels, sh.youtube_focused_only()) {
            (0, _) => "Nothing here yet",
            (_, true) if !sh.yt_channels().iter().any(|c| c.focused) => "No focused channels yet: hold OK on one in Channels",
            _ => "No new videos",
        };
        self.view.ui(ui, sh, body, &home_buttons(sh), &items, empty, focused);
        self.items = items;
        Nav::None
    }
}

const HISTORY: usize = 500;

/// Videos played, most recently first (synced like the rest of YouTube's progress).
#[derive(Default)]
pub struct YouTubeHistory {
    view: VideoGrid,
    items: Vec<VideoItem>,
    loaded: Option<u64>,
}

impl YouTubeHistory {
    fn refresh(&mut self, sh: &Shared) {
        if self.loaded == Some(sh.youtube.version) {
            return;
        }
        self.loaded = Some(sh.youtube.version);
        let now = now_unix();
        self.items = sh
            .yt_history(HISTORY)
            .into_iter()
            .map(|(id, p)| {
                let how = match (p.watched, p.position, p.duration) {
                    (true, _, _) => "watched".to_owned(),
                    (false, Some(pos), Some(dur)) => format!("{} left", theme::format_time(dur - pos)),
                    _ => "started".to_owned(),
                };
                let detail = format!("{} · {} · {how}", p.channel, ago(p.updated_at, now));
                let item = VideoItem { id, title: p.title.clone(), channel: p.channel.clone(), channel_id: String::new(), detail, fraction: None, watched: false };
                with_progress(item, Some(&p))
            })
            .collect();
    }
}

impl Screen for YouTubeHistory {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        self.refresh(sh);
        video_input(self.view.handle(press, 0, self.items.len()), &self.items).unwrap_or(Nav::None)
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let body = widgets::header(ui.painter(), rect, "History", "YouTube", ui.style());
        let items = std::mem::take(&mut self.items);
        self.view.ui(ui, sh, body, &[], &items, "Nothing watched yet", focused);
        self.items = items;
        Nav::None
    }
}

/// One channel's videos.
pub struct ChannelPage {
    id: String,
    title: String,
    view: VideoGrid,
    items: Vec<VideoItem>,
    loaded: Option<u64>,
}

impl ChannelPage {
    pub fn new(id: &str, title: &str) -> Self {
        Self { id: id.to_owned(), title: title.to_owned(), view: VideoGrid::default(), items: Vec::new(), loaded: None }
    }

    fn refresh(&mut self, sh: &Shared) {
        if self.loaded != Some(sh.youtube.version) {
            self.loaded = Some(sh.youtube.version);
            self.items = from_store(sh.yt_channel_videos(&self.id, LATEST), &sh.yt_progress_all());
        }
    }
}

impl Screen for ChannelPage {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        self.refresh(sh);
        let buttons = if sh.is_subscribed(&self.id) { 2 } else { 1 };
        match self.view.handle(press, buttons, self.items.len()) {
            GridInput::Button(1) => {
                let on = !sh.is_channel_focused(&self.id);
                sh.set_channel_focused(&self.id, &self.title, on);
                Nav::None
            }
            GridInput::Button(_) => {
                if sh.is_subscribed(&self.id) {
                    sh.unsubscribe(&self.id);
                    sh.toast(format!("Unsubscribed from {}", self.title));
                } else {
                    let (id, title) = (self.id.clone(), self.title.clone());
                    sh.subscribe(&id, &title);
                }
                Nav::None
            }
            input => video_input(input, &self.items).unwrap_or(Nav::None),
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let subscribed = sh.is_subscribed(&self.id);
        let channel_focused = sh.is_channel_focused(&self.id);
        let subtitle = match (subscribed, channel_focused) {
            (true, true) => "Subscribed · focused",
            (true, false) => "Subscribed",
            _ => "",
        };
        let body = widgets::header(ui.painter(), rect, &self.title, subtitle, ui.style());
        let buttons: &[&str] = match (subscribed, channel_focused) {
            (true, true) => &["Unsubscribe", "Unfocus"],
            (true, false) => &["Unsubscribe", "Focus"],
            _ => &["Subscribe"],
        };
        let empty = if subscribed { "No videos yet (they appear after the next check)" } else { "Subscribe to see their latest videos" };
        let items = std::mem::take(&mut self.items);
        self.view.ui(ui, sh, body, buttons, &items, empty, focused);
        self.items = items;
        Nav::None
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Lookup {
    Search,
    Related,
}

/// Videos from a lookup that runs in the background: a search, or a video's related
/// videos. Also shown in the main search's YouTube scope.
pub struct VideoResults {
    lookup: Lookup,
    /// The query, or the video whose related videos these are.
    key: String,
    view: VideoGrid,
    items: Vec<VideoItem>,
    done: bool,
}

impl VideoResults {
    pub fn search(sh: &mut Shared, query: &str) -> Self {
        let query = query.trim();
        sh.youtube.start_search(query);
        Self::new(Lookup::Search, query)
    }

    fn related(sh: &mut Shared, id: &str) -> Self {
        sh.youtube.start_related(id);
        Self::new(Lookup::Related, id)
    }

    fn new(lookup: Lookup, key: &str) -> Self {
        Self { lookup, key: key.to_owned(), view: VideoGrid::default(), items: Vec::new(), done: false }
    }

    /// What these are results for (the query, for a search).
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The videos, once in.
    pub fn len(&self) -> Option<usize> {
        self.done.then_some(self.items.len())
    }

    fn refresh(&mut self, sh: &mut Shared) {
        if self.done {
            return;
        }
        let current = match self.lookup {
            Lookup::Search => &sh.youtube.search,
            Lookup::Related => &sh.youtube.related,
        };
        let Some((key, Some(result))) = current else { return };
        if *key != self.key {
            return;
        }
        self.done = true;
        match result {
            Ok(listed) => self.items = from_listed(listed, &sh.yt_progress_all()),
            Err(e) => {
                let what = if self.lookup == Lookup::Search { "Search failed" } else { "Couldn't find related videos" };
                let e = e.clone();
                sh.toast(format!("{what}: {e}"));
            }
        }
    }

    fn handle_grid(&mut self, sh: &mut Shared, press: Press, buttons: usize) -> GridInput {
        self.refresh(sh);
        self.view.handle(press, buttons, self.items.len())
    }

    /// Up from the top row gives `Nav::FocusTopBar`.
    pub fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        let input = self.handle_grid(sh, press, 0);
        video_input(input, &self.items).unwrap_or(Nav::None)
    }

    /// Plays the first video, if they're in.
    pub fn play_first(&self) -> Nav {
        self.items.first().map_or(Nav::None, VideoItem::play)
    }

    /// Draws the videos in `body`; returns whether they were clicked.
    pub fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, body: Rect, buttons: &[&str], focused: bool) -> bool {
        self.refresh(sh);
        let empty = match (self.done, self.lookup) {
            (true, _) => "Nothing found",
            (false, _) if sh.youtube.tools.is_none() => "YouTube isn't set up yet (Settings → YouTube)",
            (false, Lookup::Search) => "Searching…",
            (false, Lookup::Related) => "Finding related videos…",
        };
        if !self.done {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }
        let items = std::mem::take(&mut self.items);
        let hit = self.view.ui(ui, sh, body, buttons, &items, empty, focused);
        self.items = items;
        hit
    }
}

/// YouTube search results (the search runs in the background).
pub struct SearchResults(VideoResults);

impl SearchResults {
    pub fn new(sh: &mut Shared, query: &str) -> Self {
        Self(VideoResults::search(sh, query))
    }
}

impl Screen for SearchResults {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        match self.0.handle_grid(sh, press, 1) {
            GridInput::Button(_) => Nav::replace(TextEntry::youtube_search()),
            input => video_input(input, &self.0.items).unwrap_or(Nav::None),
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let body = widgets::header(ui.painter(), rect, &format!("“{}”", self.0.key), "YouTube search", ui.style());
        self.0.ui(ui, sh, body, &["New search…"], focused);
        Nav::None
    }
}

/// The videos YouTube suggests after one (from its options).
pub struct RelatedVideos {
    title: String,
    results: VideoResults,
}

impl RelatedVideos {
    fn new(sh: &mut Shared, item: &VideoItem) -> Self {
        Self { title: item.title.clone(), results: VideoResults::related(sh, &item.id) }
    }
}

impl Screen for RelatedVideos {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        self.results.handle(sh, press)
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let body = widgets::header(ui.painter(), rect, &self.title, "Related videos", ui.style());
        self.results.ui(ui, sh, body, &[], focused);
        Nav::None
    }
}

/// Hold OK on a video: play from the start, mark watched, related videos, its channel, and (when
/// part-watched) remove it from Continue Watching.
pub struct VideoOptions {
    item: VideoItem,
    list: ListState,
}

enum VideoChoice {
    FromStart,
    Watched(bool),
    Channel,
    Related,
    Subscribe(bool),
    Focus(bool),
    RemoveFromContinue,
}

impl VideoOptions {
    pub fn new(item: VideoItem) -> Self {
        Self { item, list: ListState::default() }
    }

    /// For a part-watched video on Home's Continue Watching row.
    pub fn continuing(id: &str, title: &str, channel: &str) -> Self {
        let item = VideoItem {
            id: id.to_owned(),
            title: title.to_owned(),
            channel: channel.to_owned(),
            channel_id: String::new(),
            detail: String::new(),
            fraction: None,
            watched: false,
        };
        Self::new(item)
    }

    fn choices(&self, sh: &Shared) -> Vec<(Row, VideoChoice)> {
        let progress = sh.yt_progress(&self.item.id);
        let mut out = Vec::new();
        let continuing = progress.as_ref().is_some_and(|p| p.position.is_some());
        if continuing {
            out.push((Row::new(Icon::Video, "Play from beginning"), VideoChoice::FromStart));
        }
        if progress.as_ref().is_some_and(|p| p.watched) {
            out.push((Row::new(Icon::Remove, "Mark as unwatched"), VideoChoice::Watched(false)));
        } else {
            out.push((Row::new(Icon::Add, "Mark as watched"), VideoChoice::Watched(true)));
        }
        out.push((Row::new(Icon::Video, "See related videos"), VideoChoice::Related));
        if !self.item.channel_id.is_empty() {
            out.push((Row::new(Icon::Folder, format!("Go to {}", self.item.channel)), VideoChoice::Channel));
            let subscribed = sh.is_subscribed(&self.item.channel_id);
            let label = if subscribed { format!("Unsubscribe from {}", self.item.channel) } else { format!("Subscribe to {}", self.item.channel) };
            out.push((Row::new(if subscribed { Icon::Remove } else { Icon::Add }, label), VideoChoice::Subscribe(!subscribed)));
            if subscribed {
                let on = sh.is_channel_focused(&self.item.channel_id);
                let label = if on { format!("Unfocus {}", self.item.channel) } else { format!("Focus {}", self.item.channel) };
                out.push((Row::new(Icon::Star, label).detail("Channels you watch regularly"), VideoChoice::Focus(!on)));
            }
        }
        if continuing && sh.yt_continue(usize::MAX).iter().any(|(id, _)| *id == self.item.id) {
            out.push((Row::new(Icon::Remove, "Remove from Continue Watching").detail("Until you watch it again"), VideoChoice::RemoveFromContinue));
        }
        out
    }
}

impl Screen for VideoOptions {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let choices = self.choices(sh);
        let ListInput::Activate(i) = self.list.handle(press, choices.len()) else { return Nav::None };
        let item = &self.item;
        match choices.into_iter().nth(i).map(|c| c.1) {
            Some(VideoChoice::FromStart) => Nav::Play { req: PlayRequest::youtube(&item.id, &item.title, Some(0.0)), replace: true },
            Some(VideoChoice::Watched(on)) => {
                sh.set_yt_watched(&item.id, &item.title, &item.channel, on);
                Nav::Pop
            }
            Some(VideoChoice::Channel) => Nav::replace(ChannelPage::new(&item.channel_id, &item.channel)),
            Some(VideoChoice::Related) => {
                let page = RelatedVideos::new(sh, item);
                Nav::replace(page)
            }
            Some(VideoChoice::Subscribe(true)) => {
                sh.subscribe(&item.channel_id, &item.channel);
                Nav::Pop
            }
            Some(VideoChoice::Subscribe(false)) => {
                sh.unsubscribe(&item.channel_id);
                sh.toast(format!("Unsubscribed from {}", item.channel));
                Nav::Pop
            }
            Some(VideoChoice::Focus(on)) => {
                sh.set_channel_focused(&item.channel_id, &item.channel, on);
                Nav::Pop
            }
            Some(VideoChoice::RemoveFromContinue) => {
                sh.hide_continue(&format!("yt:{}", item.id));
                Nav::Pop
            }
            None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let body = widgets::header(ui.painter(), rect, &self.item.title, &self.item.channel, ui.style());
        let rows: Vec<Row> = self.choices(sh).into_iter().map(|c| c.0).collect();
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

/// Subscriptions: add one, import a list, show all or only focused ones, or open a
/// channel. Focused channels come first; holding OK on one focuses or unfocuses it.
#[derive(Default)]
pub struct ChannelList {
    list: ListState,
}

/// Rows above the channels.
const CHANNEL_LIST_TOP: usize = 3;

/// The channels the list shows: focused ones first, then (unless only focused ones are
/// shown) the rest, each by name.
fn listed_channels(sh: &Shared) -> Vec<flex_core::youtube::store::Channel> {
    let (mut focused, rest): (Vec<_>, Vec<_>) = sh.yt_channels().into_iter().partition(|c| c.focused);
    if !sh.youtube_focused_only() {
        focused.extend(rest);
    }
    focused
}

impl Screen for ChannelList {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let channels = listed_channels(sh);
        if press.is(Action::Menu) {
            if let Some(c) = self.list.selected.checked_sub(CHANNEL_LIST_TOP).and_then(|i| channels.get(i)) {
                sh.set_channel_focused(&c.id, &c.title, !c.focused);
                // It moves (focused ones come first, or it's no longer shown): keep it
                // selected if it's still there.
                if let Some(i) = listed_channels(sh).iter().position(|n| n.id == c.id) {
                    self.list.select(CHANNEL_LIST_TOP + i);
                }
            }
            return Nav::None;
        }
        match self.list.handle(press, CHANNEL_LIST_TOP + channels.len()) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(0) => Nav::push(TextEntry::youtube_channel()),
            ListInput::Activate(1) => Nav::push(FolderPicker::with_purpose(sh, Purpose::YouTubeImport)),
            ListInput::Activate(2) => {
                let on = !sh.youtube_focused_only();
                sh.set_youtube_focused_only(on);
                Nav::None
            }
            ListInput::Activate(i) => match channels.get(i - CHANNEL_LIST_TOP) {
                Some(c) => Nav::push(ChannelPage::new(&c.id, &c.title)),
                None => Nav::None,
            },
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let all = sh.yt_channels();
        let focused_count = all.iter().filter(|c| c.focused).count();
        let subtitle = match all.len() {
            0 => "No subscriptions yet".to_owned(),
            n => format!("{} · {focused_count} focused · hold OK on a channel to focus it", count(n, "subscription")),
        };
        let body = widgets::header(ui.painter(), rect, "Channels", &subtitle, ui.style());
        let show = if sh.youtube_focused_only() { "Show: focused channels" } else { "Show: all channels" };
        let mut rows = vec![
            Row::new(Icon::Add, "Add a channel…").detail("Paste a channel link or @handle"),
            Row::new(Icon::Add, "Import subscriptions…").detail("Google Takeout, FreeTube, NewPipe or OPML"),
            Row::new(Icon::None, show).detail("OK to change (YouTube's page too)"),
        ];
        let now = now_unix();
        rows.extend(listed_channels(sh).iter().map(|c| {
            let checked = if c.checked_at == 0 { "Not checked yet".to_owned() } else { format!("Checked {}", ago(c.checked_at, now)) };
            let detail = if c.focused { format!("Focused · {checked}") } else { checked };
            Row::new(if c.focused { Icon::Star } else { Icon::Folder }, &c.title).detail(detail)
        }));
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

/// Settings → YouTube.
#[derive(Default)]
pub struct YouTubeSettings {
    list: ListState,
}

impl Screen for YouTubeSettings {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        match self.list.handle(press, 8) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(0) => {
                let on = !sh.youtube_enabled();
                sh.set_youtube_enabled(on);
                if on {
                    sh.toast("YouTube on: getting yt-dlp ready…");
                }
                Nav::None
            }
            ListInput::Activate(1) => {
                sh.youtube.update_tools(true);
                sh.toast("Checking for new versions of yt-dlp and Deno…");
                Nav::None
            }
            ListInput::Activate(2) => {
                let current = sh.youtube_quality();
                let i = QUALITIES.iter().position(|q| *q == current).unwrap_or(1);
                sh.set_youtube_quality(QUALITIES[(i + 1) % QUALITIES.len()]);
                Nav::None
            }
            ListInput::Activate(3) => {
                let on = !sh.youtube_h264();
                sh.set_youtube_h264(on);
                Nav::None
            }
            ListInput::Activate(4) => {
                let on = !sh.youtube_shorts();
                sh.set_youtube_shorts(on);
                Nav::None
            }
            ListInput::Activate(5) => {
                let current = sh.youtube_continue_limit();
                let i = CONTINUE_LIMITS.iter().position(|n| *n == current).unwrap_or(3);
                sh.set_youtube_continue_limit(CONTINUE_LIMITS[(i + 1) % CONTINUE_LIMITS.len()]);
                Nav::None
            }
            ListInput::Activate(6) => Nav::push(SponsorBlockSettings::default()),
            ListInput::Activate(_) => Nav::push(ChannelList::default()),
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let body = widgets::header(ui.painter(), rect, "YouTube", "Played in Flex's player through yt-dlp: no ads", ui.style());
        let on = sh.youtube_enabled();
        let installed = Installed::load();
        let version = |tool: Tool| installed.version(tool).unwrap_or("not installed").to_owned();
        let tools = if sh.youtube.tools_busy {
            "yt-dlp and Deno: updating…".to_owned()
        } else {
            format!("yt-dlp {} · Deno {}", version(Tool::YtDlp), version(Tool::Deno))
        };
        let skipping = sh.sponsorblock_categories().len();
        let channels = sh.yt_channels().len();
        let rows = [
            Row::new(Icon::None, format!("YouTube: {}", if on { "on" } else { "off" })).detail("Adds YouTube to Home"),
            Row::new(Icon::Refresh, tools).detail("OK to update now (they also update daily)"),
            Row::new(Icon::None, format!("Quality: up to {}p", sh.youtube_quality())).detail("OK to change"),
            Row::new(Icon::None, format!("Codec: {}", if sh.youtube_h264() { "H.264 when available" } else { "any (VP9 or H.264)" }))
                .detail("H.264 is easier on slow machines · OK to change"),
            Row::new(Icon::None, format!("Shorts: {}", if sh.youtube_shorts() { "shown" } else { "hidden" })).detail("OK to change"),
            Row::new(Icon::None, match sh.youtube_continue_limit() {
                0 => "Continue Watching: no YouTube videos".to_owned(),
                n => format!("Continue Watching: up to {}", count(n, "video")),
            })
            .detail("Older ones drop off · OK to change"),
            Row::new(Icon::None, "SponsorBlock…").detail(format!("Skipping {skipping} kinds of segment")),
            Row::new(Icon::Folder, "Channels…").detail(count(channels, "subscription")),
        ];
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

/// Which SponsorBlock categories to skip.
#[derive(Default)]
pub struct SponsorBlockSettings {
    list: ListState,
}

impl Screen for SponsorBlockSettings {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        match self.list.handle(press, CATEGORIES.len()) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) => {
                let id = CATEGORIES[i].0;
                let on = !sh.sponsorblock_categories().iter().any(|c| c == id);
                sh.set_sponsorblock_category(id, on);
                Nav::None
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let body = widgets::header(ui.painter(), rect, "SponsorBlock", "Skips parts of videos marked by SponsorBlock's community", ui.style());
        let skipping = sh.sponsorblock_categories();
        let rows: Vec<Row> = CATEGORIES
            .iter()
            .map(|(id, name, _)| {
                let on = skipping.iter().any(|c| c == id);
                Row::new(Icon::None, format!("{name}: {}", if on { "skip" } else { "don't skip" })).detail("OK to change")
            })
            .collect();
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
