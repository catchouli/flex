//! YouTube: subscriptions' latest videos, channels and their playlists, search, a video's
//! options, and Settings → YouTube. Videos play in Flex's player through yt-dlp (see `crate::youtube`).

use std::collections::HashMap;

use eframe::egui::{self, Align2, Color32, CornerRadius, Painter, Rect, TextStyle, Ui, pos2, vec2};
use flex_core::youtube::feeds::{Listed, thumbnail};
use flex_core::youtube::listing::{self, Entry, Playlist};
use flex_core::youtube::sponsorblock::CATEGORIES;
use flex_core::youtube::store::{Video, WatchProgress};
use flex_core::youtube::tools::{Installed, Tool};

use super::picker::{FolderPicker, Purpose};
use super::{Nav, Screen, text_entry::TextEntry};
use crate::images::{Size, paint_cover};
use crate::input::{Action, Press};
use crate::shared::{PlayRequest, Shared, now_unix};
use crate::widgets::{self, GridSpec, GridState, Icon, ListInput, ListState, Row};
use crate::youtube::{CONTINUE_LIMITS, LAZY_AHEAD, Lazy, QUALITIES, ago, open_in_browser};
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

/// "950 views", "12K views", "1.2M views".
fn views(n: u64) -> String {
    let short = |n: u64, unit: u64, suffix: &str| {
        let tenths = n * 10 / unit;
        if tenths < 100 && !tenths.is_multiple_of(10) { format!("{}.{}{suffix}", tenths / 10, tenths % 10) } else { format!("{}{suffix}", n / unit) }
    };
    let n = match n {
        1 => return "1 view".to_owned(),
        n if n < 1_000 => n.to_string(),
        n if n < 1_000_000 => short(n, 1_000, "K"),
        n if n < 1_000_000_000 => short(n, 1_000_000, "M"),
        n => short(n, 1_000_000_000, "B"),
    };
    format!("{n} views")
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
    /// Seconds, when known: shown on the thumbnail.
    duration: Option<f64>,
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
        item.duration = item.duration.or(p.duration);
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
            let item = VideoItem {
                title: v.title,
                channel: v.channel,
                channel_id: v.channel_id,
                detail,
                duration: v.duration,
                fraction: None,
                watched: false,
                id: v.id,
            };
            with_progress(item, saved)
        })
        .collect()
}

fn listed_item(l: &Listed, detail: String, progress: &HashMap<String, WatchProgress>) -> VideoItem {
    let item = VideoItem {
        id: l.id.clone(),
        title: l.title.clone(),
        channel: l.channel.clone(),
        channel_id: l.channel_id.clone(),
        detail,
        duration: l.duration,
        fraction: None,
        watched: false,
    };
    with_progress(item, progress.get(&l.id))
}

fn from_listed(listed: &[Listed], progress: &HashMap<String, WatchProgress>) -> Vec<VideoItem> {
    listed.iter().map(|l| listed_item(l, l.channel.clone(), progress)).collect()
}

/// `detail` ("Blender · 12K views · 3 days ago") without as many of its last parts as it
/// takes to fit in `width` (keeping the first, which is clipped if need be).
fn fit_detail(detail: &str, width: f32, measure: impl Fn(&str) -> f32) -> &str {
    let mut fitted = detail;
    while measure(fitted) > width {
        match fitted.rsplit_once(" · ") {
            Some((rest, _)) => fitted = rest,
            None => break,
        }
    }
    fitted
}

/// A label in a thumbnail's bottom-right corner (a video's length), above the progress bar
/// if there is one.
fn paint_badge(painter: &Painter, thumb: Rect, text: &str, above_progress: bool, style: &egui::Style) {
    let galley = painter.layout_no_wrap(text.to_owned(), TextStyle::Small.resolve(style), Color32::WHITE);
    let pad = vec2(7.0, 2.0);
    let max = pos2(thumb.right() - 8.0, thumb.bottom() - if above_progress { 18.0 } else { 8.0 });
    let rect = Rect::from_min_max(max - galley.size() - pad * 2.0, max);
    painter.rect_filled(rect, CornerRadius::same(4), Color32::from_black_alpha(200));
    painter.galley(rect.min + pad, galley, Color32::WHITE);
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
    if let Some(d) = item.duration.filter(|d| *d > 0.0) {
        paint_badge(painter, thumb_rect, &theme::format_time(d), item.fraction.is_some() && !item.watched, style);
    }
    theme::paint_picture_frame(painter, thumb_rect, focused);
    let color = if focused { theme::highlight() } else { theme::text() };
    let text = Rect::from_min_max(pos2(rect.left(), thumb_rect.bottom() + 8.0), rect.max);
    widgets::text_block(painter, Rect::from_min_size(text.min, vec2(text.width(), 44.0)), &item.title, TextStyle::Small.resolve(style), color);
    let font = TextStyle::Small.resolve(style);
    let detail = fit_detail(&item.detail, text.width(), |s| painter.layout_no_wrap(s.to_owned(), font.clone(), theme::dim()).size().x);
    painter.with_clip_rect(text).text(pos2(text.left(), text.bottom() - 4.0), Align2::LEFT_BOTTOM, detail, font, theme::dim());
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

/// YouTube's main page: part-watched videos, then the latest from subscriptions (or
/// suggestions, "for you").
#[derive(Default)]
pub struct YouTubeHome {
    view: VideoGrid,
    items: Vec<VideoItem>,
    loaded: Option<u64>,
    /// Suggestions are on but there's nothing played or subscribed to suggest from.
    no_seeds: bool,
}

/// Search, Channels, History, Refresh, and which channels' videos to show.
fn home_buttons(sh: &Shared) -> [&'static str; 5] {
    [
        "Search…",
        "Channels",
        "History",
        if sh.youtube.refreshing { "Refreshing…" } else { "Refresh" },
        match (sh.youtube_for_you(), sh.youtube_focused_only()) {
            (true, _) => "Showing: for you",
            (false, true) => "Showing: focused",
            (false, false) => "Showing: all channels",
        },
    ]
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
                let item = VideoItem {
                    id,
                    title: p.title.clone(),
                    channel: p.channel.clone(),
                    channel_id: String::new(),
                    detail,
                    duration: None,
                    fraction: None,
                    watched: false,
                };
                with_progress(item, Some(&p))
            })
            .collect();
        let continuing: Vec<String> = items.iter().map(|v| v.id.clone()).collect();
        let rest = if sh.youtube_for_you() {
            let suggested = sh.youtube.for_you.as_ref().and_then(|r| r.as_ref().ok());
            from_listed(suggested.map_or(&[], Vec::as_slice), &progress).into_iter().filter(|v| !v.watched).collect()
        } else {
            from_store(sh.yt_latest(LATEST), &progress)
        };
        items.extend(rest.into_iter().filter(|v| !continuing.contains(&v.id)));
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
            GridInput::Button(3) => {
                sh.refresh_youtube_feeds();
                if sh.youtube_for_you() {
                    self.no_seeds = !sh.find_youtube_for_you(true);
                }
                Nav::None
            }
            GridInput::Button(_) => {
                // All channels → focused → for you → all channels.
                if sh.youtube_for_you() {
                    sh.set_youtube_for_you(false);
                    sh.set_youtube_focused_only(false);
                } else if sh.youtube_focused_only() {
                    sh.set_youtube_for_you(true);
                } else {
                    sh.set_youtube_focused_only(true);
                }
                Nav::None
            }
            input => video_input(input, &self.items).unwrap_or(Nav::None),
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let for_you = sh.youtube_for_you();
        // With nothing to suggest from, try again only once something changes.
        if for_you && sh.youtube.tools.is_some() && (!self.no_seeds || self.loaded != Some(sh.youtube.version)) {
            self.no_seeds = !sh.find_youtube_for_you(false);
        }
        self.refresh(sh);
        let channels = sh.yt_channels().len();
        let subtitle = match (sh.youtube.tools.is_some(), channels, sh.youtube.refreshing) {
            (false, _, _) if sh.youtube.tools_busy => "Setting up (downloading yt-dlp)…".to_owned(),
            (false, _, _) => "Not set up: switch YouTube on in Settings → YouTube".to_owned(),
            (true, _, _) if for_you && sh.youtube.for_you_busy => "Finding videos for you…".to_owned(),
            (true, 0, _) => "Search, or subscribe to channels in Channels".to_owned(),
            (true, n, true) => format!("{} · checking for new videos…", count(n, "channel")),
            (true, n, false) => format!("{} · no ads, sponsors skipped", count(n, "channel")),
        };
        let body = widgets::header(ui.painter(), rect, "YouTube", &subtitle, ui.style());
        let items = std::mem::take(&mut self.items);
        let empty = match (channels, sh.youtube_focused_only()) {
            _ if for_you && sh.youtube.for_you_busy => "Finding videos for you…",
            _ if for_you && self.no_seeds => "Play a few videos or subscribe to channels, and suggestions will show here",
            _ if for_you && matches!(sh.youtube.for_you, Some(Err(_))) => "Couldn't get suggestions from YouTube. Try Refresh",
            _ if for_you => "No suggestions yet. Try Refresh",
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
                let item = VideoItem {
                    id,
                    title: p.title.clone(),
                    channel: p.channel.clone(),
                    channel_id: String::new(),
                    detail,
                    duration: None,
                    fraction: None,
                    watched: false,
                };
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

/// Videos yt-dlp lists as far as they're scrolled to: a channel's uploads, or a playlist's
/// videos.
struct LazyVideos {
    url: String,
    lazy: Option<Lazy>,
}

impl LazyVideos {
    fn new(url: String) -> Self {
        Self { url, lazy: None }
    }

    /// Starts listing once YouTube's set up, asks for more as `grid` scrolls, and takes
    /// videos that came in (remembering their lengths). Returns whether any came in.
    fn poll(&mut self, sh: &mut Shared, grid: &GridState) -> bool {
        if self.lazy.is_none() {
            self.lazy = sh.youtube.lazy(&self.url);
        }
        let Some(lazy) = &mut self.lazy else { return false };
        lazy.want(grid.seen() + LAZY_AHEAD);
        let before = lazy.entries.len();
        if !lazy.poll() {
            return false;
        }
        let durations: Vec<(String, f64)> = videos(&lazy.entries[before..]).filter_map(|v| Some((v.id.clone(), v.duration?))).collect();
        sh.save_yt_durations(&durations);
        true
    }

    fn videos(&self) -> impl Iterator<Item = &Listed> {
        videos(self.lazy.as_ref().map_or(&[], |l| l.entries.as_slice()))
    }

    fn any(&self) -> bool {
        self.videos().next().is_some()
    }

    /// For the subtitle: "120 videos", plus "loading…" while more are coming.
    fn status(&self) -> String {
        let Some(lazy) = &self.lazy else { return String::new() };
        match (self.videos().count(), lazy.loading()) {
            (0, _) => String::new(),
            (n, true) => format!("{} · loading…", count(n, "video")),
            (n, false) if lazy.done => count(n, "video"),
            (n, false) => format!("{}+", count(n, "video")),
        }
    }

    /// What to show while there are no videos.
    fn empty(&self, sh: &Shared) -> String {
        match &self.lazy {
            None if sh.youtube.tools.is_none() => "YouTube isn't set up yet (Settings → YouTube)".to_owned(),
            Some(Lazy { error: Some(e), .. }) => format!("Couldn't list the videos: {e}"),
            Some(lazy) if lazy.done => "No videos".to_owned(),
            _ => "Loading videos…".to_owned(),
        }
    }
}

fn videos(entries: &[Entry]) -> impl Iterator<Item = &Listed> {
    entries.iter().filter_map(|e| match e {
        Entry::Video(v) => Some(v),
        Entry::Playlist(_) => None,
    })
}

/// "12K views · 3 days ago", from whichever is known.
fn video_detail(view_count: Option<u64>, published: Option<i64>, now: i64) -> String {
    let parts: Vec<String> = [view_count.map(views), published.map(|p| ago(p, now))].into_iter().flatten().collect();
    parts.join(" · ")
}

#[derive(Clone, Copy)]
enum ChannelButton {
    Subscribe(bool),
    Focus(bool),
    Playlists,
}

fn channel_buttons(sh: &Shared, id: &str) -> Vec<(&'static str, ChannelButton)> {
    let mut out = Vec::new();
    if sh.is_subscribed(id) {
        out.push(("Unsubscribe", ChannelButton::Subscribe(false)));
        let on = sh.is_channel_focused(id);
        out.push((if on { "Unfocus" } else { "Focus" }, ChannelButton::Focus(!on)));
    } else {
        out.push(("Subscribe", ChannelButton::Subscribe(true)));
    }
    out.push(("Playlists", ChannelButton::Playlists));
    out
}

/// What a channel's button (or its options in Channels) does.
fn channel_action(sh: &mut Shared, id: &str, title: &str, button: ChannelButton) -> Nav {
    match button {
        ChannelButton::Subscribe(true) => sh.subscribe(id, title),
        ChannelButton::Subscribe(false) => {
            sh.unsubscribe(id);
            sh.toast(format!("Unsubscribed from {title}"));
        }
        ChannelButton::Focus(on) => sh.set_channel_focused(id, title, on),
        ChannelButton::Playlists => return Nav::push(ChannelPlaylists::new(id, title)),
    }
    Nav::None
}

/// One channel's videos, all the way back: they load as they're scrolled to. Until the
/// first ones are in, a subscription shows the videos its feed listed.
pub struct ChannelPage {
    id: String,
    title: String,
    view: VideoGrid,
    items: Vec<VideoItem>,
    loaded: Option<u64>,
    videos: LazyVideos,
}

impl ChannelPage {
    pub fn new(id: &str, title: &str) -> Self {
        Self {
            id: id.to_owned(),
            title: title.to_owned(),
            view: VideoGrid::default(),
            items: Vec::new(),
            loaded: None,
            videos: LazyVideos::new(listing::channel_videos_url(id)),
        }
    }

    fn refresh(&mut self, sh: &mut Shared) {
        let new = self.videos.poll(sh, &self.view.grid);
        if !new && self.loaded == Some(sh.youtube.version) {
            return;
        }
        self.loaded = Some(sh.youtube.version);
        let progress = sh.yt_progress_all();
        let stored = sh.yt_channel_videos(&self.id, LATEST);
        if !self.videos.any() {
            self.items = from_store(stored, &progress);
            return;
        }
        // The listing's dates are approximate; the feed's are exact, for recent ones.
        let published: HashMap<&str, i64> = stored.iter().map(|v| (v.id.as_str(), v.published)).collect();
        let now = now_unix();
        self.items = self
            .videos
            .videos()
            .map(|l| {
                let mut item = listed_item(l, video_detail(l.views, published.get(l.id.as_str()).copied().or(l.published), now), &progress);
                if item.channel.is_empty() {
                    item.channel = self.title.clone();
                }
                item
            })
            .collect();
    }
}

impl Screen for ChannelPage {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        self.refresh(sh);
        let buttons = channel_buttons(sh, &self.id);
        match self.view.handle(press, buttons.len(), self.items.len()) {
            GridInput::Button(b) => match buttons.get(b) {
                Some((_, button)) => channel_action(sh, &self.id.clone(), &self.title.clone(), *button),
                None => Nav::None,
            },
            input => video_input(input, &self.items).unwrap_or(Nav::None),
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let subscribed = match (sh.is_subscribed(&self.id), sh.is_channel_focused(&self.id)) {
            (true, true) => "Subscribed · focused",
            (true, false) => "Subscribed",
            _ => "",
        };
        let subtitle = [subscribed.to_owned(), self.videos.status()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        let body = widgets::header(ui.painter(), rect, &self.title, &subtitle, ui.style());
        let buttons: Vec<&str> = channel_buttons(sh, &self.id).into_iter().map(|b| b.0).collect();
        let empty = self.videos.empty(sh);
        let items = std::mem::take(&mut self.items);
        self.view.ui(ui, sh, body, &buttons, &items, &empty, focused);
        self.items = items;
        Nav::None
    }
}

/// Hold OK on a channel in Channels: focus it, its playlists, or unsubscribe.
pub struct ChannelOptions {
    id: String,
    title: String,
    list: ListState,
}

impl ChannelOptions {
    fn new(id: &str, title: &str) -> Self {
        Self { id: id.to_owned(), title: title.to_owned(), list: ListState::default() }
    }

    fn choices(&self, sh: &Shared) -> Vec<(Row, ChannelButton)> {
        let mut out = Vec::new();
        if sh.is_subscribed(&self.id) {
            let on = sh.is_channel_focused(&self.id);
            let row = Row::new(Icon::Star, if on { "Unfocus" } else { "Focus" }).detail("Channels you watch regularly");
            out.push((row, ChannelButton::Focus(!on)));
        }
        out.push((Row::new(Icon::Folder, "Playlists"), ChannelButton::Playlists));
        if sh.is_subscribed(&self.id) {
            out.push((Row::new(Icon::Remove, "Unsubscribe"), ChannelButton::Subscribe(false)));
        } else {
            out.push((Row::new(Icon::Add, "Subscribe"), ChannelButton::Subscribe(true)));
        }
        out
    }
}

impl Screen for ChannelOptions {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let choices = self.choices(sh);
        let ListInput::Activate(i) = self.list.handle(press, choices.len()) else { return Nav::None };
        match choices.get(i).map(|c| c.1) {
            Some(ChannelButton::Playlists) => Nav::replace(ChannelPlaylists::new(&self.id, &self.title)),
            Some(button) => {
                channel_action(sh, &self.id.clone(), &self.title.clone(), button);
                Nav::Pop
            }
            None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let body = widgets::header(ui.painter(), rect, &self.title, "Channel", ui.style());
        let rows: Vec<Row> = self.choices(sh).into_iter().map(|c| c.0).collect();
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

/// A channel's playlists; OK opens one.
pub struct ChannelPlaylists {
    channel: String,
    grid: GridState,
    lazy: Option<Lazy>,
    url: String,
}

impl ChannelPlaylists {
    fn new(id: &str, title: &str) -> Self {
        Self { channel: title.to_owned(), grid: GridState::default(), lazy: None, url: listing::channel_playlists_url(id) }
    }

    fn poll(&mut self, sh: &Shared) {
        if self.lazy.is_none() {
            self.lazy = sh.youtube.lazy(&self.url);
        }
        if let Some(lazy) = &mut self.lazy {
            lazy.want(self.grid.seen() + LAZY_AHEAD);
            lazy.poll();
        }
    }

    fn playlists(&self) -> Vec<&Playlist> {
        let entries = self.lazy.as_ref().map_or(&[][..], |l| l.entries.as_slice());
        entries
            .iter()
            .filter_map(|e| match e {
                Entry::Playlist(p) => Some(p),
                Entry::Video(_) => None,
            })
            .collect()
    }
}

impl Screen for ChannelPlaylists {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        self.poll(sh);
        let len = self.playlists().len();
        match self.grid.handle(press, len) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) => match self.playlists().get(i) {
                Some(p) => Nav::push(PlaylistPage::new(p, &self.channel)),
                None => Nav::None,
            },
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.poll(sh);
        let playlists: Vec<Playlist> = self.playlists().into_iter().cloned().collect();
        let loading = self.lazy.as_ref().is_some_and(Lazy::loading);
        let subtitle = match (playlists.len(), loading) {
            (0, _) => "Playlists".to_owned(),
            (n, true) => format!("{} · loading…", count(n, "playlist")),
            (n, false) => count(n, "playlist"),
        };
        let body = widgets::header(ui.painter(), rect, &self.channel, &subtitle, ui.style());
        if playlists.is_empty() {
            let empty = match &self.lazy {
                None if sh.youtube.tools.is_none() => "YouTube isn't set up yet (Settings → YouTube)".to_owned(),
                Some(Lazy { error: Some(e), .. }) => format!("Couldn't list the playlists: {e}"),
                Some(lazy) if lazy.done => "No playlists".to_owned(),
                _ => "Loading playlists…".to_owned(),
            };
            widgets::message(ui.painter(), body, &empty, ui.style());
            return Nav::None;
        }
        let style = ui.style().clone();
        let images = &mut sh.images;
        widgets::grid(ui, body, playlists.len(), GRID, &mut self.grid, focused, |painter, i, r, f| {
            let p = &playlists[i];
            let thumb = p.thumbnail.as_ref().and_then(|url| images.get(url, Size::Still));
            let item = VideoItem {
                id: p.id.clone(),
                title: p.title.clone(),
                channel: self.channel.clone(),
                channel_id: String::new(),
                detail: "Playlist".to_owned(),
                duration: None,
                fraction: None,
                watched: false,
            };
            paint_video(painter, r, &item, thumb.as_ref(), f, &style);
            paint_badge(painter, Rect::from_min_size(r.min, THUMB), "Playlist", false, &style);
        });
        Nav::None
    }
}

/// A playlist's videos, loading as they're scrolled to.
pub struct PlaylistPage {
    title: String,
    channel: String,
    view: VideoGrid,
    items: Vec<VideoItem>,
    loaded: Option<u64>,
    videos: LazyVideos,
}

impl PlaylistPage {
    fn new(playlist: &Playlist, channel: &str) -> Self {
        Self {
            title: playlist.title.clone(),
            channel: channel.to_owned(),
            view: VideoGrid::default(),
            items: Vec::new(),
            loaded: None,
            videos: LazyVideos::new(listing::playlist_url(&playlist.id)),
        }
    }

    fn refresh(&mut self, sh: &mut Shared) {
        let new = self.videos.poll(sh, &self.view.grid);
        if !new && self.loaded == Some(sh.youtube.version) {
            return;
        }
        self.loaded = Some(sh.youtube.version);
        let progress = sh.yt_progress_all();
        let now = now_unix();
        self.items = self
            .videos
            .videos()
            .map(|l| {
                // Playlists can hold other channels' videos.
                let detail = [l.channel.clone(), video_detail(l.views, l.published, now)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                listed_item(l, detail, &progress)
            })
            .collect();
    }
}

impl Screen for PlaylistPage {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        self.refresh(sh);
        video_input(self.view.handle(press, 0, self.items.len()), &self.items).unwrap_or(Nav::None)
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let subtitle = [format!("Playlist · {}", self.channel), self.videos.status()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        let body = widgets::header(ui.painter(), rect, &self.title, &subtitle, ui.style());
        let empty = self.videos.empty(sh);
        let items = std::mem::take(&mut self.items);
        self.view.ui(ui, sh, body, &[], &items, &empty, focused);
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

/// The videos YouTube suggests after one (from its options, or when it finishes playing).
pub struct RelatedVideos {
    title: String,
    results: VideoResults,
    /// Shown after the video finished: a video picked here replaces this page, so Back
    /// still returns to wherever playback was started from.
    finished: bool,
}

impl RelatedVideos {
    fn new(sh: &mut Shared, item: &VideoItem) -> Self {
        Self { title: item.title.clone(), results: VideoResults::related(sh, &item.id), finished: false }
    }

    /// For the video that just finished playing.
    pub fn after(sh: &mut Shared, id: &str, title: &str) -> Self {
        Self { title: title.to_owned(), results: VideoResults::related(sh, id), finished: true }
    }
}

impl Screen for RelatedVideos {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) || (self.finished && press.is(Action::Stop)) {
            return Nav::Pop;
        }
        let nav = self.results.handle(sh, press);
        match nav {
            Nav::Play { req, .. } if self.finished => Nav::Play { req, replace: true },
            nav => nav,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let subtitle = if self.finished { "Finished · related videos" } else { "Related videos" };
        let body = widgets::header(ui.painter(), rect, &self.title, subtitle, ui.style());
        self.results.ui(ui, sh, body, &[], focused);
        Nav::None
    }
}

/// Hold OK on a video: play from the start, mark watched, related videos, its channel, (when
/// part-watched) remove it from Continue Watching, and open it in the browser.
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
    Browser,
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
            duration: None,
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
        out.push((Row::new(Icon::None, "Open in browser").detail("On YouTube's website"), VideoChoice::Browser));
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
            Some(VideoChoice::Browser) => {
                open_in_browser(&format!("https://www.youtube.com/watch?v={}", item.id));
                sh.toast("Opening in your browser…");
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
/// channel. Focused channels come first; holding OK on one gives its options (focus,
/// playlists, unsubscribe).
#[derive(Default)]
pub struct ChannelList {
    list: ListState,
    /// The channel whose options were opened: still selected after it moves (focused ones
    /// come first).
    reselect: Option<String>,
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
            return match self.list.selected.checked_sub(CHANNEL_LIST_TOP).and_then(|i| channels.get(i)) {
                Some(c) => {
                    self.reselect = Some(c.id.clone());
                    Nav::push(ChannelOptions::new(&c.id, &c.title))
                }
                None => Nav::None,
            };
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
        // Back from a channel's options: it may have moved (focused ones come first).
        if let Some(id) = self.reselect.take()
            && let Some(i) = listed_channels(sh).iter().position(|c| c.id == id)
        {
            self.list.select(CHANNEL_LIST_TOP + i);
        }
        let all = sh.yt_channels();
        let focused_count = all.iter().filter(|c| c.focused).count();
        let subtitle = match all.len() {
            0 => "No subscriptions yet".to_owned(),
            n => format!("{} · {focused_count} focused · hold OK on a channel for more", count(n, "subscription")),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_views_briefly() {
        assert_eq!(views(1), "1 view");
        assert_eq!(views(950), "950 views");
        assert_eq!(views(1_000), "1K views");
        assert_eq!(views(1_250), "1.2K views");
        assert_eq!(views(657_000), "657K views");
        assert_eq!(views(11_000_000), "11M views");
        assert_eq!(views(1_900_000_000), "1.9B views");
    }

    #[test]
    fn details_drop_their_last_parts_to_fit() {
        let chars = |s: &str| s.chars().count() as f32;
        assert_eq!(fit_detail("Chan · 12K views · 3 days ago", 30.0, chars), "Chan · 12K views · 3 days ago");
        assert_eq!(fit_detail("Chan · 12K views · 3 days ago", 20.0, chars), "Chan · 12K views");
        assert_eq!(fit_detail("A very long channel name", 5.0, chars), "A very long channel name");
    }

    #[test]
    fn details_show_what_is_known() {
        assert_eq!(video_detail(Some(12_000), Some(0), 3 * 86_400), "12K views · 3 days ago");
        assert_eq!(video_detail(Some(5), None, 0), "5 views");
        assert_eq!(video_detail(None, None, 0), "");
    }
}
