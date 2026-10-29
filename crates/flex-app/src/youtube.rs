//! YouTube in the app: everything slow (yt-dlp, feeds, SponsorBlock, tool updates) runs on
//! background threads and comes back through [`YouTube::poll`]. Off until switched on in
//! Settings → YouTube.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui;
use flex_core::youtube::feeds::{self, FeedVideo, Listed};
use flex_core::youtube::listing::Entry;
use flex_core::youtube::{recommend, related};
use flex_core::youtube::sponsorblock::{self, Segment};
use flex_core::youtube::tools::{self, Tool};
use flex_core::youtube::{Tools, Video};

pub const ENABLED_SETTING: &str = "youtube";
pub const QUALITY_SETTING: &str = "youtube_quality";
/// Whether to prefer H.264, which slow machines decode more easily than VP9.
pub const H264_SETTING: &str = "youtube_h264";
pub const SHORTS_SETTING: &str = "youtube_shorts";
pub const SPONSORBLOCK_SETTING: &str = "youtube_sponsorblock";
pub const CONTINUE_SETTING: &str = "youtube_continue";
/// Whether YouTube's page shows only focused channels' videos.
pub const FOCUSED_SETTING: &str = "youtube_focused_only";
/// Whether YouTube's page shows suggestions ("for you") instead of subscriptions' videos.
pub const FOR_YOU_SETTING: &str = "youtube_for_you";
/// How many part-watched videos Continue Watching keeps, as offered in Settings.
pub const CONTINUE_LIMITS: [usize; 5] = [0, 3, 5, 10, 20];
pub const DEFAULT_CONTINUE: usize = 10;
/// Highest resolutions offered in Settings; the default is 1080p.
pub const QUALITIES: [u32; 4] = [720, 1080, 1440, 2160];
pub const DEFAULT_QUALITY: u32 = 1080;

/// Stream URLs stop working after about six hours; don't reuse them for long.
const RESOLVED_FOR: Duration = Duration::from_secs(2 * 3600);
/// How often subscriptions' feeds are read again.
const REFRESH_EVERY: Duration = Duration::from_secs(30 * 60);
/// How long suggestions last before they're found again.
const FOR_YOU_EVERY: Duration = Duration::from_secs(3600);
/// A pause between feeds, to be gentle with YouTube.
const FEED_GAP: Duration = Duration::from_millis(300);

/// Results from background work, handled by [`crate::shared::Shared::poll_youtube`].
pub enum Msg {
    Resolved(String, Result<Video, String>),
    Segments(String, Vec<Segment>),
    /// A channel's feed: id, then its title and videos.
    Feed(String, Result<(String, Vec<FeedVideo>), String>),
    FeedsDone,
    Tools(Result<tools::Report, String>),
    Search(String, Result<Vec<Listed>, String>),
    /// A video's related videos.
    Related(String, Result<Vec<Listed>, String>),
    /// "Add a channel": its id and name.
    Channel(Result<(String, String), String>),
    /// Suggestions for YouTube's page.
    ForYou(Result<Vec<Listed>, String>),
    /// Lengths of subscriptions' videos: the videos asked about, then (channel id, video
    /// id, seconds) for those found.
    Durations(Vec<(String, Vec<String>)>, Result<Vec<(String, String, f64)>, String>),
}

/// A search (or a video's related videos): the query (or video), and its results once in.
pub type Search = (String, Option<Result<Vec<Listed>, String>>);

pub struct YouTube {
    ctx: egui::Context,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// yt-dlp (and maybe Deno), once installed.
    pub tools: Option<Tools>,
    pub tools_busy: bool,
    resolved: HashMap<String, (Instant, Result<Video, String>)>,
    resolving: HashSet<String>,
    segments: HashMap<String, Vec<Segment>>,
    /// The current search and its results, once in.
    pub search: Option<Search>,
    /// The video whose related videos were last asked for, and them once in.
    pub related: Option<Search>,
    pub refreshing: bool,
    last_refresh: Option<Instant>,
    /// Suggestions once found (or why not), and whether they're being found now.
    pub for_you: Option<Result<Vec<Listed>, String>>,
    pub for_you_busy: bool,
    for_you_at: Option<Instant>,
    durations_busy: bool,
    /// Bumped when subscriptions or their videos change, so screens reload.
    pub version: u64,
}

impl YouTube {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        Self {
            ctx: ctx.clone(),
            tx,
            rx,
            tools: Tools::find(),
            tools_busy: false,
            resolved: HashMap::new(),
            resolving: HashSet::new(),
            segments: HashMap::new(),
            search: None,
            related: None,
            refreshing: false,
            last_refresh: None,
            for_you: None,
            for_you_busy: false,
            for_you_at: None,
            durations_busy: false,
            version: 0,
        }
    }

    fn spawn(&self, work: impl FnOnce(&Sender<Msg>) + Send + 'static) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            work(&tx);
            ctx.request_repaint();
        });
    }

    /// Installs yt-dlp and Deno if they're missing, and updates them (daily, or now with
    /// `force`).
    pub fn update_tools(&mut self, force: bool) {
        if self.tools_busy {
            return;
        }
        self.tools_busy = true;
        self.spawn(move |tx| {
            let result = tools::update(force, crate::shared::now_unix()).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Tools(result));
        });
    }

    /// Starts finding a video's streams and SponsorBlock segments, unless that's done or
    /// under way. Called when a video is highlighted, so it's usually ready by OK.
    pub fn prepare(&mut self, id: &str, max_height: u32, h264: bool, categories: Vec<String>) {
        let fresh = self.resolved.get(id).is_some_and(|(at, r)| r.is_ok() && at.elapsed() < RESOLVED_FOR);
        if fresh || self.resolving.contains(id) {
            return;
        }
        let Some(tools) = self.tools.clone() else { return };
        self.resolved.remove(id);
        self.resolving.insert(id.to_owned());
        let video_id = id.to_owned();
        self.spawn(move |tx| {
            let mut result = tools.resolve(&video_id, max_height, h264);
            if let Err(e) = &result {
                // YouTube may have changed something that a newer yt-dlp handles.
                log::warn!("youtube: {video_id}: {e:#}; checking for a newer yt-dlp");
                if let Ok(report) = tools::update(true, crate::shared::now_unix())
                    && report.updated.iter().any(|(t, _)| *t == Tool::YtDlp)
                    && let Some(tools) = Tools::find()
                {
                    result = tools.resolve(&video_id, max_height, h264);
                }
            }
            let _ = tx.send(Msg::Resolved(video_id, result.map_err(|e| format!("{e:#}"))));
        });
        if !self.segments.contains_key(id) {
            let video_id = id.to_owned();
            self.spawn(move |tx| {
                let segments = sponsorblock::segments(&video_id, &categories).unwrap_or_else(|e| {
                    log::info!("youtube: no SponsorBlock for {video_id}: {e:#}");
                    Vec::new()
                });
                let _ = tx.send(Msg::Segments(video_id, segments));
            });
        }
    }

    /// The video's streams once found (or why not).
    pub fn resolved(&self, id: &str) -> Option<&Result<Video, String>> {
        self.resolved.get(id).map(|(_, r)| r)
    }

    /// Forgets every video's streams, after the quality or codec setting changes.
    pub fn forget_streams(&mut self) {
        self.resolved.clear();
    }

    /// Forgets a failed attempt so playing again tries again.
    pub fn forget(&mut self, id: &str) {
        if self.resolved.get(id).is_some_and(|(_, r)| r.is_err()) {
            self.resolved.remove(id);
        }
    }

    pub fn segments(&self, id: &str) -> Option<&[Segment]> {
        self.segments.get(id).map(Vec::as_slice)
    }

    /// Reads every subscription's feed again (in the background).
    pub fn refresh(&mut self, channels: Vec<String>) {
        if self.refreshing || channels.is_empty() {
            return;
        }
        self.refreshing = true;
        self.last_refresh = Some(Instant::now());
        self.spawn(move |tx| {
            for id in channels {
                let result = feeds::fetch_feed(&id).map_err(|e| format!("{e:#}"));
                let _ = tx.send(Msg::Feed(id, result));
                std::thread::sleep(FEED_GAP);
            }
            let _ = tx.send(Msg::FeedsDone);
        });
    }

    /// Whether feeds are due for their regular refresh.
    pub fn refresh_due(&self) -> bool {
        !self.refreshing && self.last_refresh.is_none_or(|t| t.elapsed() >= REFRESH_EVERY)
    }

    /// Searches in the background, unless the last search was the same and didn't fail.
    pub fn start_search(&mut self, query: &str) {
        let query = query.trim().to_owned();
        if reusable(&self.search, &query) {
            return;
        }
        let Some(tools) = self.tools.clone().filter(|_| !query.is_empty()) else { return };
        self.search = Some((query.clone(), None));
        self.spawn(move |tx| {
            let result = tools.search(&query, 30).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Search(query, result));
        });
    }

    /// Finds a video's related videos in the background (unless they're already in).
    pub fn start_related(&mut self, id: &str) {
        if reusable(&self.related, id) {
            return;
        }
        self.related = Some((id.to_owned(), None));
        let id = id.to_owned();
        self.spawn(move |tx| {
            let result = related::related(&id).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Related(id, result));
        });
    }

    /// Whether suggestions should be found (again).
    pub fn for_you_due(&self) -> bool {
        !self.for_you_busy && self.for_you_at.is_none_or(|t| t.elapsed() >= FOR_YOU_EVERY)
    }

    /// Finds suggestions in the background from each seed's related videos (see
    /// [`recommend`]). Fails only if every seed's related videos did.
    pub fn start_for_you(&mut self, seeds: Vec<(String, f32)>, exclude: HashSet<String>, subscribed: HashSet<String>, shorts: bool) {
        if self.for_you_busy {
            return;
        }
        self.for_you_busy = true;
        self.for_you_at = Some(Instant::now());
        self.spawn(move |tx| {
            let results: Vec<(f32, anyhow::Result<Vec<Listed>>)> = std::thread::scope(|s| {
                let handles: Vec<_> = seeds.iter().map(|(id, weight)| (*weight, s.spawn(move || related::related(id)))).collect();
                handles.into_iter().map(|(w, h)| (w, h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("panicked"))))).collect()
            });
            let mut lists = Vec::new();
            let mut error = None;
            for (weight, result) in results {
                match result {
                    Ok(list) => lists.push((weight, list)),
                    Err(e) => error = Some(format!("{e:#}")),
                }
            }
            let result = match error {
                Some(e) if lists.is_empty() => Err(e),
                _ => {
                    let mut exclude = exclude;
                    exclude.extend(seeds.into_iter().map(|(id, _)| id));
                    Ok(recommend::rank(&lists, &exclude, &subscribed, shorts))
                }
            };
            let _ = tx.send(Msg::ForYou(result));
        });
    }

    /// Looks up how long subscriptions' videos are, since feeds don't say: `missing` is
    /// (channel id, video ids) from `Db::yt_missing_durations`.
    pub fn find_durations(&mut self, missing: Vec<(String, Vec<String>)>) {
        if self.durations_busy || self.tools_busy || missing.is_empty() {
            return;
        }
        let Some(tools) = self.tools.clone() else { return };
        self.durations_busy = true;
        self.spawn(move |tx| {
            let channels: Vec<String> = missing.iter().map(|(c, _)| c.clone()).collect();
            let result = tools.latest_durations(&channels).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Durations(missing, result));
        });
    }

    /// Starts listing a channel tab or playlist (see [`Lazy`]); `None` until yt-dlp is
    /// installed, and while it's being updated (a yt-dlp running while its file is
    /// replaced fails: it reads itself as it goes).
    pub fn lazy(&self, url: &str) -> Option<Lazy> {
        if self.tools_busy {
            return None;
        }
        Some(Lazy::start(&self.ctx, self.tools.clone()?, url.to_owned()))
    }

    /// Looks up a channel from a link or @handle (then subscribes, see `Shared`).
    pub fn find_channel(&self, input: &str) {
        let Some(tools) = self.tools.clone() else { return };
        let input = input.to_owned();
        self.spawn(move |tx| {
            let result = match feeds::channel_id(&input).filter(|_| !input.contains('@')) {
                // A /channel/UC... link: its feed gives the name without yt-dlp.
                Some(id) => feeds::fetch_feed(&id).map(|(title, _)| (id, title)),
                None => tools.channel(&input),
            };
            let _ = tx.send(Msg::Channel(result.map_err(|e| format!("{e:#}"))));
        });
    }

    /// Takes finished work. Results that only the service needs are kept here; the rest
    /// are returned for `Shared` (which has the database).
    pub fn poll(&mut self) -> Vec<Msg> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Resolved(id, result) => {
                    self.resolving.remove(&id);
                    self.resolved.insert(id, (Instant::now(), result));
                }
                Msg::Segments(id, segments) => {
                    self.segments.insert(id, segments);
                }
                Msg::Search(query, result) => {
                    if self.search.as_ref().is_some_and(|(q, _)| *q == query) {
                        self.search = Some((query, Some(result)));
                    }
                }
                Msg::Related(id, result) => {
                    if self.related.as_ref().is_some_and(|(v, _)| *v == id) {
                        self.related = Some((id, Some(result)));
                    }
                }
                Msg::Tools(result) => {
                    self.tools_busy = false;
                    self.tools = Tools::find();
                    out.push(Msg::Tools(result));
                }
                Msg::FeedsDone => {
                    self.refreshing = false;
                    out.push(Msg::FeedsDone);
                }
                Msg::Durations(missing, result) => {
                    self.durations_busy = false;
                    out.push(Msg::Durations(missing, result));
                }
                Msg::ForYou(result) => {
                    if let Err(e) = &result {
                        log::warn!("youtube: no suggestions: {e}");
                    }
                    self.for_you_busy = false;
                    self.for_you = Some(result);
                    self.version += 1;
                }
                other => out.push(other),
            }
        }
        out
    }
}

/// Entries listed before anyone scrolls (a few screens' worth).
const LAZY_FIRST: usize = 60;
/// How far past the last entry on screen to keep listed.
pub const LAZY_AHEAD: usize = 60;

/// A channel's videos or playlists, or a playlist's videos, coming in from yt-dlp on a
/// background thread. It reads only as far as [`Lazy::want`] asks, and yt-dlp waits in the
/// meantime; dropping this stops it.
pub struct Lazy {
    pub entries: Vec<Entry>,
    /// Everything's in (or it failed).
    pub done: bool,
    pub error: Option<String>,
    rx: Receiver<Result<Entry, String>>,
    want_tx: Sender<usize>,
    wanted: usize,
}

impl Lazy {
    fn start(ctx: &egui::Context, tools: Tools, url: String) -> Self {
        let (tx, rx) = channel();
        let (want_tx, want_rx) = channel::<usize>();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let listing = match tools.listing(&url) {
                Ok(listing) => listing,
                Err(e) => {
                    let _ = tx.send(Err(format!("{e:#}")));
                    ctx.request_repaint();
                    return;
                }
            };
            let (mut listing, mut sent, mut wanted) = (listing, 0, LAZY_FIRST);
            loop {
                while let Ok(n) = want_rx.try_recv() {
                    wanted = wanted.max(n);
                }
                if sent >= wanted {
                    match want_rx.recv() {
                        Ok(n) => wanted = wanted.max(n),
                        Err(_) => return, // the page closed
                    }
                    continue;
                }
                let Some(entry) = listing.next() else { break };
                if tx.send(Ok(entry)).is_err() {
                    return;
                }
                sent += 1;
                ctx.request_repaint();
            }
            if let Err(e) = listing.finish() {
                log::warn!("youtube: listing {url}: {e:#}");
                let _ = tx.send(Err(format!("{e:#}")));
            }
            ctx.request_repaint();
        });
        Self { entries: Vec::new(), done: false, error: None, rx, want_tx, wanted: LAZY_FIRST }
    }

    /// Takes entries that came in; returns whether any did.
    pub fn poll(&mut self) -> bool {
        let before = self.entries.len();
        loop {
            match self.rx.try_recv() {
                Ok(Ok(entry)) => self.entries.push(entry),
                Ok(Err(e)) => self.error = Some(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.done = true;
                    break;
                }
            }
        }
        self.entries.len() > before
    }

    /// Asks for at least `n` entries.
    pub fn want(&mut self, n: usize) {
        if n > self.wanted {
            self.wanted = n;
            let _ = self.want_tx.send(n);
        }
    }

    /// Still listing (or about to be).
    pub fn loading(&self) -> bool {
        !self.done && self.entries.len() < self.wanted
    }
}

/// Opens a web page in the default browser.
pub fn open_in_browser(url: &str) {
    let mut cmd = if cfg!(windows) {
        let mut cmd = std::process::Command::new("rundll32");
        cmd.arg("url.dll,FileProtocolHandler");
        cmd
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else {
        std::process::Command::new("xdg-open")
    };
    if let Err(e) = cmd.arg(url).spawn() {
        log::warn!("opening {url}: {e}");
    }
}

/// Whether `current` is for `key` and hasn't failed (so needn't run again).
fn reusable(current: &Option<Search>, key: &str) -> bool {
    current.as_ref().is_some_and(|(k, result)| k == key && !matches!(result, Some(Err(_))))
}

/// "3 hours ago", "2 days ago", "5 months ago".
pub fn ago(published: i64, now: i64) -> String {
    let secs = (now - published).max(0);
    let (n, unit) = match secs {
        s if s < 3600 => ((s / 60).max(1), "minute"),
        s if s < 86_400 => (s / 3600, "hour"),
        s if s < 7 * 86_400 => (s / 86_400, "day"),
        s if s < 30 * 86_400 => (s / (7 * 86_400), "week"),
        s if s < 365 * 86_400 => (s / (30 * 86_400), "month"),
        s => (s / (365 * 86_400), "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn says_how_long_ago() {
        assert_eq!(ago(0, 30), "1 minute ago");
        assert_eq!(ago(0, 3 * 3600), "3 hours ago");
        assert_eq!(ago(0, 86_400), "1 day ago");
        assert_eq!(ago(0, 20 * 86_400), "2 weeks ago");
        assert_eq!(ago(0, 400 * 86_400), "1 year ago");
        assert_eq!(ago(100, 0), "1 minute ago", "clock skew");
    }
}
