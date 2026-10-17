//! YouTube in the app: everything slow (yt-dlp, feeds, SponsorBlock, tool updates) runs on
//! background threads and comes back through [`YouTube::poll`]. Off until switched on in
//! Settings → YouTube.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui;
use flex_core::youtube::feeds::{self, FeedVideo, Listed};
use flex_core::youtube::sponsorblock::{self, Segment};
use flex_core::youtube::tools::{self, Tool};
use flex_core::youtube::{Tools, Video};

pub const ENABLED_SETTING: &str = "youtube";
pub const QUALITY_SETTING: &str = "youtube_quality";
pub const SHORTS_SETTING: &str = "youtube_shorts";
pub const SPONSORBLOCK_SETTING: &str = "youtube_sponsorblock";
/// Highest resolutions offered in Settings; the default is 1080p.
pub const QUALITIES: [u32; 4] = [720, 1080, 1440, 2160];
pub const DEFAULT_QUALITY: u32 = 1080;

/// Stream URLs stop working after about six hours; don't reuse them for long.
const RESOLVED_FOR: Duration = Duration::from_secs(2 * 3600);
/// How often subscriptions' feeds are read again.
const REFRESH_EVERY: Duration = Duration::from_secs(30 * 60);
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
    /// "Add a channel": its id and name.
    Channel(Result<(String, String), String>),
}

/// A search: the query, and its results once in.
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
    pub refreshing: bool,
    last_refresh: Option<Instant>,
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
            refreshing: false,
            last_refresh: None,
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
    pub fn prepare(&mut self, id: &str, max_height: u32, categories: Vec<String>) {
        let fresh = self.resolved.get(id).is_some_and(|(at, r)| r.is_ok() && at.elapsed() < RESOLVED_FOR);
        if fresh || self.resolving.contains(id) {
            return;
        }
        let Some(tools) = self.tools.clone() else { return };
        self.resolved.remove(id);
        self.resolving.insert(id.to_owned());
        let video_id = id.to_owned();
        self.spawn(move |tx| {
            let mut result = tools.resolve(&video_id, max_height);
            if let Err(e) = &result {
                // YouTube may have changed something that a newer yt-dlp handles.
                log::warn!("youtube: {video_id}: {e:#}; checking for a newer yt-dlp");
                if let Ok(report) = tools::update(true, crate::shared::now_unix())
                    && report.updated.iter().any(|(t, _)| *t == Tool::YtDlp)
                    && let Some(tools) = Tools::find()
                {
                    result = tools.resolve(&video_id, max_height);
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

    pub fn start_search(&mut self, query: &str) {
        let query = query.trim().to_owned();
        let Some(tools) = self.tools.clone().filter(|_| !query.is_empty()) else { return };
        self.search = Some((query.clone(), None));
        self.spawn(move |tx| {
            let result = tools.search(&query, 30).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Search(query, result));
        });
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
                Msg::Tools(result) => {
                    self.tools_busy = false;
                    self.tools = Tools::find();
                    out.push(Msg::Tools(result));
                }
                Msg::FeedsDone => {
                    self.refreshing = false;
                    out.push(Msg::FeedsDone);
                }
                other => out.push(other),
            }
        }
        out
    }
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
