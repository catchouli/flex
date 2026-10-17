//! State shared by all screens: the database, libraries with their folder trees,
//! background scans and the search index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::Result;
use eframe::egui;
use flex_core::catalog::{self, Catalog, Group};
use flex_core::db::MusicMatch;
use flex_core::music::{Album, MusicCatalog};
use flex_core::musicbrainz::{ArtistInfo, ReleaseGroup};
use flex_core::parse::Parsed;
use flex_core::progress::{self, ContinueItem, Progress};
use flex_core::search::{Item, SearchIndex};
use flex_core::{Db, Library, LibraryKind, Tree};
use flex_plugin::{Playback, PlaybackState};

use crate::mpv::GetProcAddress;

pub struct LibState {
    pub lib: Library,
    /// Shared with the web server's snapshots; edits copy it only while one holds it.
    pub tree: Arc<Tree>,
    /// Videos found so far while a scan is running.
    pub scanning: Option<usize>,
    pub error: Option<String>,
    /// Bumped whenever `tree` changes, so screens know to refresh.
    pub version: u64,
    /// Shows/seasons, once organised (in the background after loading or scanning).
    pub organised: Option<Organised>,
    pub organising: bool,
    /// Looked up / to look up, while matching online.
    pub matching: Option<(usize, usize)>,
    /// Done / to do, while fetching details (cast, episode titles) for matched shows.
    pub fetching: Option<(usize, usize)>,
    /// Music libraries: albums and artists, once the tags are read.
    pub music: Option<Arc<MusicCatalog>>,
    /// Folders read / to read, while reading tags.
    pub reading_tags: Option<(usize, usize)>,
    /// Music: albums looked up on MusicBrainz, by album key.
    pub music_matches: Arc<HashMap<String, MusicMatch>>,
    /// Music: MusicBrainz artist ids by lowercased name, from the matches.
    pub music_artist_ids: Arc<HashMap<String, String>>,
}

pub use flex_core::titles::Titles;

/// A library sorted into shows. `parsed` and `groups` are kept so the catalog can be
/// rebuilt instantly after a manual fix.
pub struct Organised {
    pub parsed: Arc<HashMap<i64, Parsed>>,
    pub groups: Arc<Vec<Group>>,
    pub catalog: Arc<Catalog>,
}

/// How a library opens from Home.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LibraryView {
    Shows,
    Folders,
}

impl LibState {
    fn new(lib: Library, tree: Tree) -> Self {
        Self {
            lib,
            tree: Arc::new(tree),
            scanning: None,
            error: None,
            version: 0,
            organised: None,
            organising: false,
            matching: None,
            fetching: None,
            music: None,
            reading_tags: None,
            music_matches: Arc::default(),
            music_artist_ids: Arc::default(),
        }
    }

    /// An album's cover: art next to its tracks, else MusicBrainz's (Cover Art Archive).
    pub fn album_cover(&self, album: &Album) -> Option<String> {
        if let Some(local) = &album.cover {
            return Some(format!("file:{local}"));
        }
        let rg = self.music_matches.get(&album.key)?.release_group.as_ref()?;
        Some(flex_core::musicbrainz::cover_url(&rg.id))
    }

    fn set_music_matches(&mut self, matches: HashMap<String, MusicMatch>) {
        let mut ids = HashMap::new();
        for rg in matches.values().filter_map(|m| m.release_group.as_ref()) {
            for (name, id) in &rg.artists {
                ids.entry(name.to_lowercase()).or_insert_with(|| id.clone());
            }
        }
        self.music_matches = Arc::new(matches);
        self.music_artist_ids = Arc::new(ids);
        self.version += 1;
    }

    pub fn is_music(&self) -> bool {
        self.lib.kind == LibraryKind::Music
    }

    pub fn catalog(&self) -> Option<&Catalog> {
        self.organised.as_ref().map(|o| o.catalog.as_ref())
    }

    /// One-line status, e.g. "1,234 videos" or "Scanning… 120 found".
    pub fn status(&self) -> String {
        if let Some(n) = self.scanning {
            format!("Scanning… {} found", thousands(n))
        } else if let Some((done, total)) = self.matching {
            format!("Matching… {done}/{total}")
        } else if let Some((done, total)) = self.fetching {
            format!("Fetching details… {done}/{total}")
        } else if let Some((done, total)) = self.reading_tags {
            format!("Reading tags… {}/{} folders", thousands(done), thousands(total))
        } else if let Some(e) = &self.error {
            e.clone()
        } else if let Some(music) = &self.music {
            let albums = music.albums.iter().filter(|a| !a.singles).count();
            format!("{} albums · {} tracks", thousands(albums), thousands(music.tracks))
        } else if self.is_music() {
            format!("{} tracks", thousands(self.tree.len()))
        } else {
            format!("{} videos", thousands(self.tree.len()))
        }
    }
}

pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Everything needed to start playing a file.
#[derive(Clone)]
pub struct PlayRequest {
    pub path: PathBuf,
    pub title: String,
    pub library_id: i64,
    pub file_id: i64,
    /// Where to start, in seconds. `None` = ask to resume if there's a saved position.
    pub start: Option<f64>,
}

/// The "library" YouTube plays belong to: their path is `youtube:<video id>`.
pub const YOUTUBE_LIBRARY: i64 = -2;
const YOUTUBE_PREFIX: &str = "youtube:";

impl PlayRequest {
    pub fn youtube(id: &str, title: &str, start: Option<f64>) -> Self {
        Self { path: PathBuf::from(format!("{YOUTUBE_PREFIX}{id}")), title: title.to_owned(), library_id: YOUTUBE_LIBRARY, file_id: -1, start }
    }

    /// The video id, for a YouTube play.
    pub fn youtube_id(&self) -> Option<&str> {
        if self.library_id != YOUTUBE_LIBRARY {
            return None;
        }
        self.path.to_str()?.strip_prefix(YOUTUBE_PREFIX)
    }
}

const SKIP_SETTING: &str = "skip_intros";

/// Skipping openings and endings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipMode {
    /// Show a "Skip opening" button (OK presses it).
    Ask,
    Auto,
    Off,
}

impl SkipMode {
    pub fn label(self) -> &'static str {
        match self {
            SkipMode::Ask => "Ask",
            SkipMode::Auto => "Automatically",
            SkipMode::Off => "Off",
        }
    }

    pub fn next(self) -> Self {
        match self {
            SkipMode::Ask => SkipMode::Auto,
            SkipMode::Auto => SkipMode::Off,
            SkipMode::Off => SkipMode::Ask,
        }
    }
}

/// Choices offered in Settings for "mark watched at".
pub const WATCHED_THRESHOLDS: [f64; 4] = [0.8, 0.85, 0.9, 0.95];

pub fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

enum ScanMsg {
    Progress(i64, usize),
    /// A scan finished: library id, whether it was an automatic one, and the outcome.
    Done(i64, bool, Result<ScanDone, String>),
    Organised(i64, Result<Organised, String>),
    /// Online matching progress (done, total); the catalog is reloaded on these.
    Matching(i64, usize, usize),
    MatchDone(i64, Result<String, String>),
    /// Details fetching progress (done, total), and completion.
    Fetching(i64, usize, usize),
    /// TMDB details are in (the slower AniList voice-cast pass may still be running).
    DetailsReady(i64),
    FetchDone(i64, Result<usize, String>),
    /// Music: tags read in (done, total) folders, and the finished albums.
    ReadingTags(i64, usize, usize),
    Music(i64, Result<MusicCatalog, String>),
    /// MusicBrainz: albums looked up (done, total), and the end of that pass.
    MusicMatching(i64, usize, usize),
    MusicMatchDone(i64, Result<String, String>),
    /// Artist details fetched (done, total), and the end of that pass.
    MusicArtists(i64, usize, usize),
    MusicArtistsDone(i64, Result<usize, String>),
    /// An artist fetched on its own (after a match by hand).
    MusicArtistReady(i64),
}

/// Look again for albums MusicBrainz didn't have after this long.
/// How often libraries are scanned for changes automatically (and at startup). Unchanged
/// folders aren't read again, so this is cheap.
const AUTO_SCAN_EVERY: std::time::Duration = std::time::Duration::from_secs(30 * 60);

struct ScanDone {
    /// What was found, for a toast.
    msg: String,
    /// Whether files came or went.
    changed: bool,
}

/// Look again for albums MusicBrainz didn't have after this long.
const MUSIC_RETRY_SECS: i64 = 30 * 24 * 3600;
/// Re-fetch artists' details (new albums, photos) after this long.
const MUSIC_ARTIST_MAX_AGE_SECS: i64 = 90 * 24 * 3600;
/// Give up on a matching pass after this many failures in a row (offline, say).
const MUSIC_MAX_FAILURES: usize = 5;

/// Re-fetch details this often, so new episodes of running series get their titles.
const DETAILS_MAX_AGE_SECS: i64 = 30 * 24 * 3600;
const VOICE_MAX_AGE_SECS: i64 = 180 * 24 * 3600;
/// Actors' filmographies are looked up again after this long.
const PERSON_MAX_AGE_SECS: i64 = 30 * 24 * 3600;

const TMDB_TOKEN_SETTING: &str = "tmdb_token";
/// Bump when grouping/matching logic improves: automatic results are redone on the next
/// start (manual choices are kept).
const MATCHER_VERSION: &str = "4";
const MATCHER_VERSION_SETTING: &str = "matcher_version";
const CLOSE_TO_TRAY_SETTING: &str = "close_to_tray";
const WEB_SETTING: &str = "web_ui";
const WEB_PORT_SETTING: &str = "web_port";
const DEFAULT_WEB_PORT: u16 = 8484;
/// Pause between TMDB requests (TMDB allows ~50/s; no need to hurry).
const MATCH_PAUSE: std::time::Duration = std::time::Duration::from_millis(60);

pub struct Shared {
    pub ctx: egui::Context,
    pub proc_address: Option<GetProcAddress>,
    db_path: PathBuf,
    db: Db,
    pub libs: Vec<LibState>,
    pub search_query: String,
    pub images: crate::images::Images,
    pub shots: crate::shots::Shots,
    /// Other titles (romaji etc.) by provider id, searched alongside the main title.
    alt_titles: HashMap<String, Vec<String>>,
    /// (key, library id, added at), most recent first.
    watchlist: Vec<(String, i64, i64)>,
    /// Bumped whenever the watchlist changes, so screens showing it refresh.
    pub watchlist_version: u64,
    /// Episode titles etc. for matched series, by provider id ("tv:123").
    pub series: Arc<HashMap<String, flex_core::db::SeriesMeta>>,
    /// The window's monitor refresh rate, for smooth video timing.
    pub display_fps: Option<f64>,
    search: Option<SearchIndex>,
    scan_tx: Sender<ScanMsg>,
    scan_rx: Receiver<ScanMsg>,
    /// Cancel flags for running scans, by library id.
    scan_cancel: HashMap<i64, Arc<AtomicBool>>,
    /// When libraries were last scanned for changes automatically.
    last_auto_scan: Option<std::time::Instant>,
    toasts: Vec<flex_core::plugins::Toast>,
    /// Music artists' details from MusicBrainz (photo, discography), by MusicBrainz id.
    pub music_artists: Arc<HashMap<String, ArtistInfo>>,
    /// Closing the window hides Flex in the system tray instead of quitting.
    pub close_to_tray: bool,
    pub updater: crate::update::Updater,
    /// The web UI, while switched on.
    web: Option<flex_web::Server>,
    /// What the web UI was last given: (library id, version, name) per library.
    web_published: Vec<(i64, u64, String)>,
    web_tx: Sender<flex_web::Change>,
    web_rx: Receiver<flex_web::Change>,
    /// Plugins found on disk (only switched-on ones run).
    pub plugins: flex_core::plugins::Host,
    /// Screen colours for plugins subscribed to `light`.
    pub ambient: crate::ambient::Ambient,
    /// The last playback state sent to plugins.
    playback_sent: Option<Playback>,
    /// The latest playback seen (with its library file), for where a stop happened.
    playback_last: Option<(Playback, Option<(i64, i64)>)>,
    pub youtube: crate::youtube::YouTube,
    /// Bumped when something is removed from Continue Watching, so Home refreshes.
    pub continue_version: u64,
}

impl Shared {
    pub fn new(ctx: &egui::Context, proc_address: Option<GetProcAddress>) -> Result<Self> {
        let db_path = flex_core::data_dir().join("flex.db");
        log::info!("database: {}", db_path.display());
        let db = Db::open(&db_path)?;
        let (scan_tx, scan_rx) = channel();
        let (web_tx, web_rx) = channel();
        let waker = ctx.clone();
        let plugins = flex_core::plugins::Host::new(&flex_core::plugins::plugin_dirs(), &db, move || waker.request_repaint());
        let ambient = crate::ambient::Ambient::new(|key| db.setting(key).ok().flatten());
        let mut shared = Self {
            ctx: ctx.clone(),
            proc_address,
            db_path,
            db,
            libs: Vec::new(),
            search_query: String::new(),
            series: Arc::default(),
            alt_titles: HashMap::new(),
            watchlist: Vec::new(),
            watchlist_version: 0,
            images: crate::images::Images::new(ctx, flex_core::data_dir().join("images")),
            shots: crate::shots::Shots::new(ctx, flex_core::data_dir().join("shots")),
            display_fps: None,
            search: None,
            scan_tx,
            scan_rx,
            scan_cancel: HashMap::new(),
            last_auto_scan: None,
            toasts: Vec::new(),
            music_artists: Arc::default(),
            close_to_tray: false,
            updater: crate::update::Updater::default(),
            web: None,
            web_published: Vec::new(),
            web_tx,
            web_rx,
            plugins,
            ambient,
            playback_sent: None,
            playback_last: None,
            youtube: crate::youtube::YouTube::new(ctx),
            continue_version: 0,
        };
        shared.close_to_tray = shared.setting_bool(CLOSE_TO_TRAY_SETTING);
        if shared.youtube_enabled() {
            // Installs yt-dlp if it's missing; otherwise only checks once a day.
            shared.youtube.update_tools(false);
        }
        // Videos piled up before there was a limit (or since it was lowered).
        shared.trim_yt_continue();
        shared.music_artists = Arc::new(shared.db.music_artists(i64::MAX).unwrap_or_default());
        if shared.auto_update() {
            shared.updater.set_enabled(true, ctx);
        }
        shared.series = Arc::new(shared.db.series_meta()?);
        shared.alt_titles = shared.db.alt_titles()?;
        shared.watchlist = shared.db.watchlist()?;
        if shared.db.setting(MATCHER_VERSION_SETTING)?.as_deref() != Some(MATCHER_VERSION) {
            for lib in shared.db.libraries()? {
                shared.db.reset_matching(lib.id)?;
            }
            shared.db.set_setting(MATCHER_VERSION_SETTING, MATCHER_VERSION)?;
        }
        for lib in shared.db.libraries()? {
            let tree = Tree::new(shared.db.files(lib.id)?);
            let never_scanned = tree.is_empty();
            let id = lib.id;
            shared.libs.push(LibState::new(lib, tree));
            if never_scanned {
                shared.start_scan(id, true);
            } else {
                shared.organise(id);
            }
        }
        Ok(shared)
    }

    pub fn setting(&self, key: &str) -> Option<String> {
        self.db.setting(key).ok().flatten()
    }

    pub fn set_setting(&mut self, key: &str, value: &str) {
        if let Err(e) = self.db.set_setting(key, value) {
            log::warn!("saving setting {key}: {e:#}");
        }
    }

    pub fn setting_bool(&self, key: &str) -> bool {
        self.db.setting(key).ok().flatten().is_some_and(|v| v == "true")
    }

    pub fn set_setting_bool(&mut self, key: &str, value: bool) {
        if let Err(e) = self.db.set_setting(key, if value { "true" } else { "false" }) {
            log::warn!("saving setting {key}: {e:#}");
        }
    }

    /// Starts the web UI at startup if it was left on.
    pub fn start_web_if_enabled(&mut self) {
        if self.setting_bool(WEB_SETTING) {
            self.set_web_enabled(true);
        }
    }

    pub fn web_enabled(&self) -> bool {
        self.web.is_some()
    }

    /// The address other devices can open, while the web UI is on.
    pub fn web_url(&self) -> Option<String> {
        let port = self.web.as_ref()?.port();
        let host = flex_web::lan_address().map_or_else(|| "localhost".to_owned(), |ip| ip.to_string());
        Some(format!("http://{host}:{port}"))
    }

    pub fn set_web_enabled(&mut self, on: bool) {
        self.set_setting_bool(WEB_SETTING, on);
        if !on {
            self.web = None;
            return;
        }
        if self.web.is_some() {
            return;
        }
        let port = self.setting(WEB_PORT_SETTING).and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_WEB_PORT);
        let config = flex_web::Config {
            port,
            db_path: self.db_path.clone(),
            images_dir: flex_core::data_dir().join("images"),
            shots_dir: flex_core::data_dir().join("shots"),
            cache_dir: flex_core::data_dir().join("web-cache"),
        };
        let (tx, ctx) = (self.web_tx.clone(), self.ctx.clone());
        let on_change = move |change| {
            let _ = tx.send(change);
            ctx.request_repaint();
        };
        match flex_web::Server::start(config, self.web_snapshot(), on_change) {
            Ok(server) => {
                self.web = Some(server);
                self.web_published = self.web_key();
            }
            Err(e) => {
                log::error!("starting the web UI: {e:#}");
                self.toast(format!("Couldn't start the web UI: {e:#}"));
            }
        }
    }

    fn web_key(&self) -> Vec<(i64, u64, String)> {
        self.libs.iter().map(|l| (l.lib.id, l.version, l.lib.name.clone())).collect()
    }

    fn web_snapshot(&self) -> flex_web::Snapshot {
        flex_web::Snapshot {
            libraries: self
                .libs
                .iter()
                .map(|l| flex_web::LibrarySnapshot {
                    lib: l.lib.clone(),
                    tree: l.tree.clone(),
                    catalog: l.organised.as_ref().map(|o| o.catalog.clone()),
                    music: l.music.clone(),
                    covers: Arc::new(
                        l.music
                            .iter()
                            .flat_map(|m| m.albums.iter())
                            .filter_map(|a| Some((a.key.clone(), l.album_cover(a)?)))
                            .collect(),
                    ),
                })
                .collect(),
            series: self.series.clone(),
        }
    }

    /// Applies changes made from the web and keeps the web UI up to date with ours.
    fn poll_web(&mut self) {
        while let Ok(change) = self.web_rx.try_recv() {
            match change {
                flex_web::Change::Watched { library_id, file_ids, watched } => {
                    log::info!("web: marked {} file(s) {}", file_ids.len(), if watched { "watched" } else { "unwatched" });
                    self.apply_watched(library_id, &file_ids, watched);
                }
                flex_web::Change::Progress { library_id, file_id, progress } => {
                    if let Some(state) = self.lib_mut(library_id)
                        && let Some(file) = Arc::make_mut(&mut state.tree).file_mut(file_id)
                    {
                        file.progress = progress;
                        state.version += 1;
                    }
                }
            }
        }
        if self.web.is_some() {
            let key = self.web_key();
            if key != self.web_published {
                if let Some(web) = &self.web {
                    web.publish(self.web_snapshot());
                }
                self.web_published = key;
            }
        }
    }

    /// Whether "Update automatically" is on (it's only offered in installed copies).
    pub fn auto_update(&self) -> bool {
        crate::update::installed() && self.setting_bool(crate::update::SETTING)
    }

    pub fn set_auto_update(&mut self, on: bool) {
        self.set_setting_bool(crate::update::SETTING, on);
        self.updater.set_enabled(on, &self.ctx);
    }

    pub fn set_close_to_tray(&mut self, on: bool) {
        self.close_to_tray = on;
        self.set_setting_bool(CLOSE_TO_TRAY_SETTING, on);
    }

    pub fn watched_threshold(&self) -> f64 {
        progress::watched_threshold(&self.db)
    }

    pub fn set_watched_threshold(&mut self, threshold: f64) {
        if let Err(e) = self.db.set_setting(progress::WATCHED_THRESHOLD_SETTING, &threshold.to_string()) {
            log::warn!("saving watched threshold: {e:#}");
        }
    }

    pub fn progress(&self, library_id: i64, file_id: i64) -> Option<Progress> {
        Some(self.lib(library_id)?.tree.file(file_id)?.progress)
    }

    /// Records having played `req` up to `position` (seconds) of `duration`. Returns the
    /// new progress, or `None` if nothing was recorded (too short, or not a library file).
    pub fn record_progress(&mut self, req: &PlayRequest, position: f64, duration: f64) -> Option<Progress> {
        let threshold = self.watched_threshold();
        if let Some(id) = req.youtube_id() {
            let channel = match self.youtube.resolved(id) {
                Some(Ok(video)) => video.channel.clone(),
                _ => String::new(),
            };
            if let Err(e) = self.db.record_yt_progress(id, &req.title, &channel, position, duration, threshold, now_unix()) {
                log::warn!("saving YouTube progress: {e:#}");
            }
            self.youtube.version += 1;
            self.trim_yt_continue();
            return None;
        }
        let old = self.progress(req.library_id, req.file_id)?;
        let new = old.after_playing(position, duration, threshold, now_unix())?;
        if new == old {
            return Some(new);
        }
        if let Err(e) = self.db.set_progress(req.file_id, &new) {
            log::warn!("saving progress: {e:#}");
            return None;
        }
        if let Some(state) = self.lib_mut(req.library_id)
            && let Some(file) = Arc::make_mut(&mut state.tree).file_mut(req.file_id)
        {
            file.progress = new;
            state.version += 1;
        }
        Some(new)
    }

    /// Adds a viewing session to the history.
    pub fn add_history(&mut self, req: &PlayRequest, played_at: i64, position: Option<f64>, completed: bool) {
        if req.youtube_id().is_some() {
            return; // the history is per library
        }
        if let Err(e) = self.db.add_history(req.file_id, played_at, position, completed) {
            log::warn!("saving history: {e:#}");
        }
    }

    pub fn history(&self, library_id: i64, limit: usize) -> Vec<flex_core::db::HistoryEntry> {
        self.db.history(library_id, limit).unwrap_or_else(|e| {
            log::warn!("reading history: {e:#}");
            Vec::new()
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Reloads files (and their watch state) from the database, e.g. after an import.
    pub fn reload_files(&mut self) {
        for i in 0..self.libs.len() {
            let id = self.libs[i].lib.id;
            match self.db.files(id) {
                Ok(files) => {
                    let l = &mut self.libs[i];
                    l.tree = Arc::new(Tree::new(files));
                    l.version += 1;
                }
                Err(e) => log::warn!("reloading library {id}: {e:#}"),
            }
        }
        self.search = None;
    }

    /// Marks files (e.g. a whole folder) watched or unwatched.
    pub fn set_watched(&mut self, library_id: i64, ids: &[i64], watched: bool) {
        if let Err(e) = self.db.set_watched(ids, watched) {
            self.toast(format!("Couldn't update: {e:#}"));
            return;
        }
        self.apply_watched(library_id, ids, watched);
    }

    /// Updates watched state in memory (the database is already up to date).
    /// Notes that a file started playing now (music: for "Recently played").
    pub fn mark_played(&mut self, library_id: i64, file_id: i64) {
        let Some(state) = self.lib_mut(library_id) else { return };
        let Some(file) = Arc::make_mut(&mut state.tree).file_mut(file_id) else { return };
        file.progress.last_played = Some(now_unix());
        let progress = file.progress;
        state.version += 1;
        if let Err(e) = self.db.set_progress(file_id, &progress) {
            log::warn!("saving play time: {e:#}");
        }
    }

    fn apply_watched(&mut self, library_id: i64, ids: &[i64], watched: bool) {
        if let Some(state) = self.lib_mut(library_id) {
            for id in ids {
                if let Some(file) = Arc::make_mut(&mut state.tree).file_mut(*id) {
                    file.progress.watched = watched;
                    file.progress.position = None;
                }
            }
            state.version += 1;
        }
    }

    /// Continue Watching entries, most recent first.
    /// Continue Watching: per show for organised libraries, per folder for the rest
    /// (while they're still being organised).
    pub fn continue_watching(&self, limit: usize) -> Vec<ContinueItem> {
        let organised = self.libs.iter().filter_map(|l| l.catalog().map(|c| (&l.lib, &*l.tree, c)));
        let mut items = progress::continue_watching_shows(organised, limit);
        let pending = self.libs.iter().filter(|l| l.organised.is_none() && !l.is_music()).map(|l| (&l.lib, &*l.tree));
        items.extend(progress::continue_watching(pending, limit));
        let mut items = progress::without_hidden(items, &self.db.continue_hidden().unwrap_or_default());
        items.truncate(limit);
        items
    }

    /// Removes an entry from Continue Watching until it's played again (`key` from
    /// `ContinueItem::hide_key`, or `yt:<video id>`).
    pub fn hide_continue(&mut self, key: &str) {
        if let Err(e) = self.db.hide_continue(key, now_unix()) {
            log::warn!("hiding from Continue Watching: {e:#}");
        }
        self.continue_version += 1;
        self.youtube.version += 1;
    }

    pub fn library_view(&self, id: i64) -> LibraryView {
        match self.db.setting(&format!("view:{id}")).ok().flatten().as_deref() {
            Some("folders") => LibraryView::Folders,
            _ => LibraryView::Shows,
        }
    }

    pub fn set_library_view(&mut self, id: i64, view: LibraryView) {
        let value = if view == LibraryView::Folders { "folders" } else { "shows" };
        if let Err(e) = self.db.set_setting(&format!("view:{id}"), value) {
            log::warn!("saving library view: {e:#}");
        }
    }

    /// Sorts a library into shows on a background thread. Parsed names are cached per
    /// folder, so only new or changed folders are parsed (13k anime names took ~8s).
    pub fn organise(&mut self, id: i64) {
        if self.lib(id).is_some_and(LibState::is_music) {
            self.organise_music(id);
            return;
        }
        let Some(state) = self.lib_mut(id) else { return };
        state.organising = true;
        let kind = state.lib.kind;
        let files = state.tree.files().to_vec();
        let (tx, ctx, db_path) = (self.scan_tx.clone(), self.ctx.clone(), self.db_path.clone());
        std::thread::spawn(move || {
            let result = (|| -> Result<Organised> {
                let started = std::time::Instant::now();
                let tree = Tree::new(files);
                let mut db = Db::open(&db_path)?;
                let parsed = flex_core::parse::parse_library_cached(&mut db, id, kind, tree.files())?;
                let parse_time = started.elapsed();
                let groups = catalog::derive_groups(kind, &tree, &parsed);
                db.sync_groups(id, &groups)?;
                let catalog = Catalog::build(&tree, &parsed, &groups, db.shows(id)?, &db.group_matches(id)?);
                log::info!(
                    "organised library {id}: {} groups into {} shows in {:?} (parsing {parse_time:?})",
                    groups.len(),
                    catalog.shows.len(),
                    started.elapsed()
                );
                Ok(Organised { parsed: Arc::new(parsed), groups: Arc::new(groups), catalog: Arc::new(catalog) })
            })();
            let _ = tx.send(ScanMsg::Organised(id, result.map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
    }

    /// Reads a music library's tags (new or changed folders only) and sorts its tracks
    /// into albums, on a background thread.
    fn organise_music(&mut self, id: i64) {
        let Some(state) = self.lib_mut(id) else { return };
        state.organising = true;
        let (root, files) = (state.lib.root.clone(), state.tree.files().to_vec());
        let (tx, ctx, db_path) = (self.scan_tx.clone(), self.ctx.clone(), self.db_path.clone());
        let covers = flex_core::data_dir().join("covers");
        std::thread::spawn(move || {
            let result = (|| -> Result<MusicCatalog> {
                let started = std::time::Instant::now();
                let mut db = Db::open(&db_path)?;
                let folders = flex_core::music::read_library_cached(&mut db, id, &root, &files, &covers, |done, total| {
                    let _ = tx.send(ScanMsg::ReadingTags(id, done, total));
                    ctx.request_repaint();
                })?;
                let music = flex_core::music::build(&files, &folders);
                log::info!("organised music library {id}: {} albums, {} artists in {:?}", music.albums.len(), music.artists.len(), started.elapsed());
                Ok(music)
            })();
            let _ = tx.send(ScanMsg::Music(id, result.map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
    }

    /// Looks up albums on MusicBrainz that haven't been (or weren't found a while ago),
    /// in the background, one a second.
    pub fn start_music_matching(&mut self, id: i64) {
        let Some(state) = self.lib(id) else { return };
        let Some(music) = state.music.clone() else { return };
        if state.matching.is_some() {
            return;
        }
        let now = now_unix();
        // (album key, title, artist, tracks, year)
        type Lookup = (String, String, Option<String>, usize, Option<i32>);
        let todo: Vec<Lookup> = music
            .albums
            .iter()
            .filter(|a| !a.singles)
            .filter(|a| match state.music_matches.get(&a.key) {
                None => true,
                Some(m) => !m.manual && m.release_group.is_none() && now - m.checked_at > MUSIC_RETRY_SECS,
            })
            .map(|a| {
                let artist = (a.artist != "Various Artists" && a.artist != "Unknown artist").then(|| a.artist.clone());
                (a.key.clone(), a.title.clone(), artist, a.tracks.len(), a.year)
            })
            .collect();
        if todo.is_empty() {
            return;
        }
        let total = todo.len();
        if let Some(l) = self.lib_mut(id) {
            l.matching = Some((0, total));
        }
        let (tx, ctx, db_path) = (self.scan_tx.clone(), self.ctx.clone(), self.db_path.clone());
        std::thread::spawn(move || {
            let result = (|| -> Result<String> {
                let mb = flex_core::musicbrainz::MusicBrainz::new();
                let db = Db::open(&db_path)?;
                let (mut matched, mut missed, mut failures) = (0, 0, 0);
                for (i, (key, title, artist, tracks, year)) in todo.iter().enumerate() {
                    match mb.find_album(title, artist.as_deref(), *tracks, *year) {
                        Ok(found) => {
                            failures = 0;
                            if found.is_some() { matched += 1 } else { missed += 1 }
                            db.set_music_match(id, key, found.as_ref(), false)?;
                        }
                        Err(e) => {
                            log::warn!("MusicBrainz {title:?}: {e:#}");
                            failures += 1;
                            if failures >= MUSIC_MAX_FAILURES {
                                anyhow::bail!("stopped after {failures} failures in a row ({e:#})");
                            }
                        }
                    }
                    if (i + 1) % 10 == 0 || i + 1 == total {
                        let _ = tx.send(ScanMsg::MusicMatching(id, i + 1, total));
                        ctx.request_repaint();
                    }
                }
                Ok(format!("{matched} albums found on MusicBrainz, {missed} not found"))
            })();
            let _ = tx.send(ScanMsg::MusicMatchDone(id, result.map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
    }

    /// Fetches details (photo, discography) for the artists of matched albums, in the
    /// background.
    pub fn start_music_artists(&mut self, id: i64) {
        let Some(state) = self.lib(id) else { return };
        if state.fetching.is_some() {
            return;
        }
        let fresh = self.db.music_artists(MUSIC_ARTIST_MAX_AGE_SECS).unwrap_or_default();
        let mut todo: Vec<String> = state.music_artist_ids.values().filter(|a| !fresh.contains_key(*a)).cloned().collect();
        todo.sort();
        todo.dedup();
        if todo.is_empty() {
            return;
        }
        let total = todo.len();
        if let Some(l) = self.lib_mut(id) {
            l.fetching = Some((0, total));
        }
        let (tx, ctx, db_path) = (self.scan_tx.clone(), self.ctx.clone(), self.db_path.clone());
        std::thread::spawn(move || {
            let result = (|| -> Result<usize> {
                let mb = flex_core::musicbrainz::MusicBrainz::new();
                let db = Db::open(&db_path)?;
                let (mut done, mut failures) = (0, 0);
                for (i, artist) in todo.iter().enumerate() {
                    match mb.artist(artist) {
                        Ok(info) => {
                            failures = 0;
                            done += 1;
                            db.set_music_artist(artist, &info)?;
                        }
                        Err(e) => {
                            log::warn!("MusicBrainz artist {artist}: {e:#}");
                            failures += 1;
                            if failures >= MUSIC_MAX_FAILURES {
                                anyhow::bail!("stopped after {failures} failures in a row ({e:#})");
                            }
                        }
                    }
                    if (i + 1) % 10 == 0 || i + 1 == total {
                        let _ = tx.send(ScanMsg::MusicArtists(id, i + 1, total));
                        ctx.request_repaint();
                    }
                }
                Ok(done)
            })();
            let _ = tx.send(ScanMsg::MusicArtistsDone(id, result.map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
    }

    /// Sets (or clears) an album's MusicBrainz match by hand.
    pub fn set_album_match(&mut self, library_id: i64, album_key: &str, release_group: Option<&ReleaseGroup>) {
        if let Err(e) = self.db.set_music_match(library_id, album_key, release_group, true) {
            self.toast(format!("Couldn't save the match: {e:#}"));
            return;
        }
        let matches = self.db.music_matches(library_id).unwrap_or_default();
        if let Some(l) = self.lib_mut(library_id) {
            l.set_music_matches(matches);
        }
        // Its artists straight away, rather than after everyone else's.
        let todo: Vec<String> = release_group.map(|rg| rg.artists.iter().map(|(_, id)| id.clone()).collect()).unwrap_or_default();
        let todo: Vec<String> = todo.into_iter().filter(|a| !self.music_artists.contains_key(a)).collect();
        if todo.is_empty() {
            return;
        }
        let (tx, ctx, db_path) = (self.scan_tx.clone(), self.ctx.clone(), self.db_path.clone());
        std::thread::spawn(move || {
            let mb = flex_core::musicbrainz::MusicBrainz::new();
            let Ok(db) = Db::open(&db_path) else { return };
            for artist in todo {
                match mb.artist(&artist).and_then(|info| db.set_music_artist(&artist, &info)) {
                    Ok(()) => {
                        let _ = tx.send(ScanMsg::MusicArtistReady(library_id));
                        ctx.request_repaint();
                    }
                    Err(e) => log::warn!("MusicBrainz artist {artist}: {e:#}"),
                }
            }
        });
    }

    /// What MusicBrainz knows about an artist (by name, as tagged), once fetched.
    pub fn artist_info(&self, library_id: i64, name: &str) -> Option<&ArtistInfo> {
        let id = self.lib(library_id)?.music_artist_ids.get(&name.to_lowercase())?;
        self.music_artists.get(id)
    }

    /// The TMDB "API Read Access Token": from Settings, or a `tmdb_token` file in the data
    /// folder (handy for setting it without typing).
    pub fn tmdb_token(&self) -> Option<String> {
        let from_db = self.db.setting(TMDB_TOKEN_SETTING).ok().flatten();
        let from_file = || std::fs::read_to_string(flex_core::data_dir().join("tmdb_token")).ok();
        from_db.or_else(from_file).map(|t| t.trim().to_owned()).filter(|t| !t.is_empty())
    }

    pub fn set_tmdb_token(&mut self, token: &str) {
        if let Err(e) = self.db.set_setting(TMDB_TOKEN_SETTING, token.trim()) {
            self.toast(format!("Couldn't save the token: {e:#}"));
            return;
        }
        let ids: Vec<i64> = self.libs.iter().map(|l| l.lib.id).collect();
        for id in ids {
            self.start_matching(id);
        }
    }

    /// Redoes a library's automatic grouping and online matching (manual choices stay).
    pub fn rematch(&mut self, id: i64) {
        if self.lib(id).is_some_and(|l| l.matching.is_some() || l.organising) {
            self.toast("Still matching; try again when it's done");
            return;
        }
        if let Err(e) = self.db.reset_matching(id) {
            self.toast(format!("Couldn't reset: {e:#}"));
            return;
        }
        if let Some(l) = self.lib_mut(id) {
            l.organised = None;
            l.version += 1;
        }
        self.organise(id);
    }

    /// Looks up shows that haven't been matched online yet, in the background.
    pub fn start_matching(&mut self, id: i64) {
        let Some(token) = self.tmdb_token() else { return };
        let Some(state) = self.lib(id) else { return };
        if state.matching.is_some() {
            return;
        }
        let Some(catalog) = state.catalog() else { return };
        let anime = state.lib.kind == LibraryKind::Anime;
        let todo: Vec<flex_core::metadata::Lookup> = catalog
            .shows
            .iter()
            .filter(|s| s.show.state == "local")
            .map(|s| flex_core::metadata::Lookup {
                show_id: s.show.id,
                title: s.show.title.clone(),
                year: s.show.year,
                kind: if s.is_series() { flex_core::metadata::Kind::Tv } else { flex_core::metadata::Kind::Movie },
                anime,
            })
            .collect();
        if todo.is_empty() {
            return;
        }
        let total = todo.len();
        if let Some(l) = self.lib_mut(id) {
            l.matching = Some((0, total));
        }
        let (tx, ctx, db_path) = (self.scan_tx.clone(), self.ctx.clone(), self.db_path.clone());
        std::thread::spawn(move || {
            let result = (|| -> Result<String> {
                let tmdb = flex_core::metadata::Tmdb::new(&token);
                tmdb.verify()?;
                let mut db = Db::open(&db_path)?;
                let (mut matched, mut unmatched) = (0, 0);
                for (i, item) in todo.iter().enumerate() {
                    match flex_core::metadata::lookup(&tmdb, item) {
                        Ok(Some(c)) => {
                            db.set_show_metadata(item.show_id, &c, "matched")?;
                            matched += 1;
                        }
                        Ok(None) => {
                            db.set_show_state(item.show_id, "unmatched")?;
                            unmatched += 1;
                        }
                        // Network hiccup: leave it for next time.
                        Err(e) => log::warn!("matching {:?}: {e:#}", item.title),
                    }
                    if (i + 1) % 25 == 0 || i + 1 == total {
                        let _ = tx.send(ScanMsg::Matching(id, i + 1, total));
                        ctx.request_repaint();
                    }
                    std::thread::sleep(MATCH_PAUSE);
                }
                Ok(format!("matched {matched} online, {unmatched} not found"))
            })();
            let _ = tx.send(ScanMsg::MatchDone(id, result.map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
    }

    /// Fetches details for matched shows in the background: TMDB overview, genres, cast and
    /// every season's episode titles; then, for anime, Japanese voice actors from AniList
    /// (slower: AniList allows ~30 requests a minute).
    pub fn start_details(&mut self, id: i64) {
        let Some(token) = self.tmdb_token() else { return };
        let Some(state) = self.lib(id) else { return };
        if state.fetching.is_some() {
            return;
        }
        let anime = state.lib.kind == LibraryKind::Anime;
        let todo = self.db.shows_needing_details(id, DETAILS_MAX_AGE_SECS).unwrap_or_else(|e| {
            log::warn!("listing shows needing details: {e:#}");
            Vec::new()
        });
        let voice_todo = if anime {
            self.db.shows_needing_voice_cast(id, VOICE_MAX_AGE_SECS).unwrap_or_default()
        } else {
            Vec::new()
        };
        if todo.is_empty() && voice_todo.is_empty() {
            return;
        }
        let total = todo.len() + voice_todo.len();
        if let Some(l) = self.lib_mut(id) {
            l.fetching = Some((0, total));
        }
        let (tx, ctx, db_path) = (self.scan_tx.clone(), self.ctx.clone(), self.db_path.clone());
        std::thread::spawn(move || {
            let result = (|| -> Result<usize> {
                let progress = |done: usize| {
                    if done.is_multiple_of(10) || done == total {
                        let _ = tx.send(ScanMsg::Fetching(id, done, total));
                        ctx.request_repaint();
                    }
                };
                let tmdb = flex_core::metadata::Tmdb::new(&token);
                let mut db = Db::open(&db_path)?;
                let mut fetched = 0;
                for (i, (show_id, provider_id, title, full)) in todo.iter().enumerate() {
                    let outcome = (|| -> Result<()> {
                        let details = tmdb.details(provider_id)?;
                        db.save_details(*show_id, provider_id, &details)?;
                        // Only the cast was missing: the episodes are already known.
                        if let Some(tv_id) = provider_id.strip_prefix("tv:").filter(|_| *full) {
                            for season in &details.seasons {
                                let episodes = tmdb.season(tv_id, season.number)?;
                                db.save_episodes(provider_id, &episodes)?;
                                std::thread::sleep(MATCH_PAUSE);
                            }
                        }
                        Ok(())
                    })();
                    match outcome {
                        Ok(()) => fetched += 1,
                        Err(e) => log::warn!("details for {title:?} ({provider_id}): {e:#}"),
                    }
                    progress(i + 1);
                    std::thread::sleep(MATCH_PAUSE);
                }
                let _ = tx.send(ScanMsg::DetailsReady(id));
                ctx.request_repaint();
                for (i, (show_id, provider_id, title)) in voice_todo.iter().enumerate() {
                    progress(todo.len() + i + 1);
                    match flex_core::metadata::anilist_info(title) {
                        Ok(info) => db.save_anilist(*show_id, provider_id, &info)?,
                        // Network trouble: try again next time.
                        Err(e) => log::warn!("voice cast for {title:?}: {e:#}"),
                    }
                }
                Ok(fetched)
            })();
            let _ = tx.send(ScanMsg::FetchDone(id, result.map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
    }

    /// Picks up newly fetched details: episode titles and the shows' overviews/posters.
    fn reload_details(&mut self, id: i64) {
        match self.db.series_meta() {
            Ok(series) => self.series = Arc::new(series),
            Err(e) => log::warn!("loading episode titles: {e:#}"),
        }
        match self.db.alt_titles() {
            Ok(alt) => self.alt_titles = alt,
            Err(e) => log::warn!("loading other titles: {e:#}"),
        }
        self.rebuild_catalog(id);
        self.search = None;
    }

    /// The watchlist key for a show: its TMDB entry, or its local id if unmatched.
    fn watchlist_key(&self, library_id: i64, show_id: i64) -> Option<String> {
        let entry = self.lib(library_id)?.catalog()?.show(show_id)?;
        Some(entry.show.provider_id.clone().unwrap_or_else(|| format!("local:{show_id}")))
    }

    pub fn on_watchlist(&self, library_id: i64, show_id: i64) -> bool {
        self.watchlist_key(library_id, show_id).is_some_and(|k| self.watchlist.iter().any(|(w, _, _)| *w == k))
    }

    pub fn set_watchlist(&mut self, library_id: i64, show_id: i64, on: bool) {
        let Some(key) = self.watchlist_key(library_id, show_id) else { return };
        match self.db.set_watchlist(&key, library_id, on).and_then(|()| self.db.watchlist()) {
            Ok(list) => {
                self.watchlist = list;
                self.watchlist_version += 1;
                self.toast(if on { "Added to watchlist" } else { "Removed from watchlist" });
            }
            Err(e) => self.toast(format!("Couldn't update the watchlist: {e:#}")),
        }
    }

    /// Watchlist shows that are still in a library: (library id, show id, added at).
    pub fn watchlist_shows(&self) -> Vec<(i64, i64, i64)> {
        self.watchlist
            .iter()
            .filter_map(|(key, library_id, added)| {
                let catalog = self.lib(*library_id)?.catalog()?;
                let show = match key.strip_prefix("local:") {
                    Some(id) => catalog.show(id.parse().ok()?)?,
                    None => catalog.shows.iter().find(|s| s.show.provider_id.as_deref() == Some(key))?,
                };
                Some((*library_id, show.show.id, *added))
            })
            .collect()
    }

    /// Display names for a file: show and episode titles from online metadata where
    /// available, the file name otherwise.
    pub fn titles(&self, library_id: i64, file_id: i64) -> Option<Titles> {
        let state = self.lib(library_id)?;
        flex_core::titles::titles(&state.tree, state.catalog(), &self.series, file_id)
    }

    /// TMDB metadata (description, still) for an episode, if known.
    pub fn episode_meta(&self, library_id: i64, file_id: i64) -> Option<&flex_core::db::EpisodeMeta> {
        flex_core::titles::episode_meta(self.lib(library_id)?.catalog()?, &self.series, file_id)
    }

    pub fn credits(&self, provider_id: &str, kind: &str) -> Vec<flex_core::metadata::CastMember> {
        self.db.credits(provider_id, kind).unwrap_or_default()
    }

    /// Shows and films in the libraries that a person is credited in: (library id, show
    /// id, role).
    pub fn appearances(&self, person: Option<&str>, name: &str) -> Vec<(i64, i64, String)> {
        let credits = self.db.appearances(person, name).unwrap_or_else(|e| {
            log::warn!("reading credits for {name}: {e:#}");
            Vec::new()
        });
        let roles: HashMap<&str, &str> = credits.iter().map(|(p, r)| (p.as_str(), r.as_str())).collect();
        let mut out = Vec::new();
        for l in &self.libs {
            for entry in l.catalog().map_or(&[][..], |c| &c.shows) {
                if let Some(role) = entry.show.provider_id.as_deref().and_then(|p| roles.get(p)) {
                    out.push((l.lib.id, entry.show.id, (*role).to_owned()));
                }
            }
        }
        out
    }

    /// A person's online details if fetched recently.
    pub fn cached_person(&self, person: &str) -> Option<flex_core::metadata::Person> {
        self.db.person(person, PERSON_MAX_AGE_SECS).unwrap_or_else(|e| {
            log::warn!("reading {person}: {e:#}");
            None
        })
    }

    /// Whether a TMDB entry ("movie:123") is in a library.
    pub fn in_library(&self, provider_id: &str) -> bool {
        self.libs.iter().filter_map(|l| l.catalog()).any(|c| c.shows.iter().any(|s| s.show.provider_id.as_deref() == Some(provider_id)))
    }

    /// Every title (and other title) of the shows in anime libraries, normalised: anime from
    /// AniList are recognised by name.
    pub fn anime_titles(&self) -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        for l in self.libs.iter().filter(|l| l.lib.kind == LibraryKind::Anime) {
            for entry in l.catalog().map_or(&[][..], |c| &c.shows) {
                out.insert(flex_core::parse::normalize(&entry.show.title));
                if let Some(alt) = entry.show.provider_id.as_ref().and_then(|p| self.alt_titles.get(p)) {
                    out.extend(alt.iter().map(|t| flex_core::parse::normalize(t)));
                }
            }
        }
        out
    }

    /// Rebuilds a library's catalog from the database (after a manual fix), reusing the
    /// already-parsed names.
    fn rebuild_catalog(&mut self, id: i64) {
        let (shows, matches) = match (self.db.shows(id), self.db.group_matches(id)) {
            (Ok(s), Ok(m)) => (s, m),
            (Err(e), _) | (_, Err(e)) => {
                self.toast(format!("Couldn't reload shows: {e:#}"));
                return;
            }
        };
        let Some(state) = self.lib_mut(id) else { return };
        let Some(org) = &mut state.organised else { return };
        org.catalog = Arc::new(Catalog::build(&state.tree, &org.parsed, &org.groups, shows, &matches));
        state.version += 1;
    }

    /// Applies a TMDB entry chosen by hand to a show.
    pub fn apply_match(&mut self, library_id: i64, show_id: i64, c: &flex_core::metadata::Candidate) {
        match self.db.set_show_metadata(show_id, c, "manual") {
            Ok(()) => {
                self.rebuild_catalog(library_id);
                self.search = None;
                self.toast(format!("Matched to {}", c.title));
            }
            Err(e) => self.toast(format!("Couldn't save the match: {e:#}")),
        }
    }

    /// Puts a folder (group) into `show_id`, optionally with a season. Kept on rescans.
    pub fn place_group(&mut self, library_id: i64, key: &str, show_id: i64, season: Option<i32>) {
        if let Err(e) = self.db.set_group_match(library_id, key, show_id, season) {
            self.toast(format!("Couldn't save: {e:#}"));
            return;
        }
        self.rebuild_catalog(library_id);
    }

    /// Moves a folder (group) into a new show of its own.
    pub fn split_group(&mut self, library_id: i64, key: &str) {
        let Some(group) = self.lib(library_id).and_then(|l| l.organised.as_ref()?.groups.iter().find(|g| g.key == key).cloned())
        else {
            return;
        };
        match self.db.create_show(library_id, &group.title, group.year) {
            Ok(show_id) => self.place_group(library_id, key, show_id, None),
            Err(e) => self.toast(format!("Couldn't create show: {e:#}")),
        }
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toasts.push(flex_core::plugins::Toast::new(msg));
    }

    pub fn take_toasts(&mut self) -> Vec<flex_core::plugins::Toast> {
        std::mem::take(&mut self.toasts)
    }

    pub fn lib(&self, id: i64) -> Option<&LibState> {
        self.libs.iter().find(|l| l.lib.id == id)
    }

    fn lib_mut(&mut self, id: i64) -> Option<&mut LibState> {
        self.libs.iter_mut().find(|l| l.lib.id == id)
    }

    /// Adds a library (named after its folder) and starts scanning it; returns its id.
    pub fn add_library(&mut self, root: &Path, kind: LibraryKind) -> Result<i64> {
        let name = root
            .file_name()
            .map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
        let id = self.db.add_library(&name, root, kind)?;
        let lib = Library { id, name, root: root.to_path_buf(), kind };
        self.libs.push(LibState::new(lib, Tree::new(Vec::new())));
        self.libs.sort_by_key(|l| l.lib.name.to_lowercase());
        self.start_scan(id, true);
        Ok(id)
    }

    pub fn rename_library(&mut self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("A library needs a name");
        }
        self.db.rename_library(id, name)?;
        if let Some(l) = self.lib_mut(id) {
            l.lib.name = name.to_owned();
            l.version += 1;
        }
        self.libs.sort_by_key(|l| l.lib.name.to_lowercase());
        self.search = None; // results show library names
        Ok(())
    }

    pub fn remove_library(&mut self, id: i64) -> Result<()> {
        if let Some(cancel) = self.scan_cancel.remove(&id) {
            cancel.store(true, Ordering::Relaxed);
        }
        self.db.remove_library(id)?;
        self.libs.retain(|l| l.lib.id != id);
        self.search = None;
        Ok(())
    }

    pub fn set_kind(&mut self, id: i64, kind: LibraryKind) -> Result<()> {
        self.db.set_library_kind(id, kind)?;
        let mut rescan = false;
        if let Some(l) = self.lib_mut(id) {
            // Music and video libraries look for different files.
            rescan = (l.lib.kind == LibraryKind::Music) != (kind == LibraryKind::Music);
            l.lib.kind = kind;
            l.version += 1;
            if rescan {
                (l.organised, l.music) = (None, None);
            }
        }
        if rescan {
            self.start_scan(id, true);
        }
        Ok(())
    }

    /// Scans a library for changes, or with `full`, looks at every file again.
    pub fn start_scan(&mut self, id: i64, full: bool) {
        self.scan(id, full, false);
    }

    /// `quiet` scans (the automatic ones) only say something if files came or went.
    fn scan(&mut self, id: i64, full: bool, quiet: bool) {
        let Some(state) = self.lib_mut(id) else { return };
        if state.scanning.is_some() {
            return;
        }
        state.scanning = Some(0);
        state.error = None;
        let (root, kind) = (state.lib.root.clone(), state.lib.kind);
        let cancel = Arc::new(AtomicBool::new(false));
        self.scan_cancel.insert(id, cancel.clone());
        let (tx, ctx, db_path) = (self.scan_tx.clone(), self.ctx.clone(), self.db_path.clone());
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let result = (|| -> Result<ScanDone> {
                // A separate connection: SQLite (WAL) handles the concurrent reader on the UI thread.
                let mut db = Db::open(&db_path)?;
                let previous = db.previous_scan(id, full)?;
                let scan = flex_core::scan::scan(&root, kind, &previous, &cancel, |n| {
                    let _ = tx.send(ScanMsg::Progress(id, n));
                    ctx.request_repaint();
                })?;
                for e in &scan.errors {
                    log::warn!("scan: {e}");
                }
                let summary = db.apply_scan(id, &scan)?;
                log::info!(
                    "scan of {}: {} files, {} of {} folders read, {:?}",
                    root.display(),
                    summary.total,
                    scan.dirs_read,
                    scan.dirs.len(),
                    started.elapsed()
                );
                let what = if kind == LibraryKind::Music { "tracks" } else { "videos" };
                let mut msg = format!(
                    "{} {what} ({} new, {} removed)",
                    thousands(summary.total),
                    summary.added,
                    summary.removed
                );
                if !scan.errors.is_empty() {
                    msg += &format!(", {} unreadable", scan.errors.len());
                }
                Ok(ScanDone { msg, changed: summary.added + summary.removed > 0 })
            })();
            let _ = tx.send(ScanMsg::Done(id, quiet, result.map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
    }

    /// Quietly scans every library for changes now and then, so new episodes turn up by
    /// themselves.
    fn auto_scan(&mut self) {
        if self.last_auto_scan.is_some_and(|t| t.elapsed() < AUTO_SCAN_EVERY) {
            return;
        }
        self.last_auto_scan = Some(std::time::Instant::now());
        let ids: Vec<i64> = self.libs.iter().map(|l| l.lib.id).collect();
        for id in ids {
            self.scan(id, false, true);
        }
        self.ctx.request_repaint_after(AUTO_SCAN_EVERY);
    }

    pub fn scan_all(&mut self, full: bool) {
        let ids: Vec<i64> = self.libs.iter().map(|l| l.lib.id).collect();
        for id in ids {
            self.start_scan(id, full);
        }
    }

    /// Applies finished/progressing scans. Call once per frame.
    /// What to do at a file's opening and ending chapters (see `flex_core::skip`).
    pub fn skip_mode(&self) -> SkipMode {
        match self.setting(SKIP_SETTING).as_deref() {
            Some("auto") => SkipMode::Auto,
            Some("off") => SkipMode::Off,
            _ => SkipMode::Ask,
        }
    }

    pub fn set_skip_mode(&mut self, mode: SkipMode) {
        let value = match mode {
            SkipMode::Ask => "ask",
            SkipMode::Auto => "auto",
            SkipMode::Off => "off",
        };
        self.set_setting(SKIP_SETTING, value);
    }

    pub fn youtube_enabled(&self) -> bool {
        self.setting_bool(crate::youtube::ENABLED_SETTING)
    }

    pub fn set_youtube_enabled(&mut self, on: bool) {
        self.set_setting_bool(crate::youtube::ENABLED_SETTING, on);
        if on {
            self.youtube.update_tools(false);
        }
    }

    /// The tallest video to play, in pixels.
    pub fn youtube_quality(&self) -> u32 {
        self.setting(crate::youtube::QUALITY_SETTING).and_then(|q| q.parse().ok()).unwrap_or(crate::youtube::DEFAULT_QUALITY)
    }

    pub fn set_youtube_quality(&mut self, height: u32) {
        self.set_setting(crate::youtube::QUALITY_SETTING, &height.to_string());
    }

    pub fn youtube_shorts(&self) -> bool {
        self.setting_bool(crate::youtube::SHORTS_SETTING)
    }

    pub fn set_youtube_shorts(&mut self, on: bool) {
        self.set_setting_bool(crate::youtube::SHORTS_SETTING, on);
        self.youtube.version += 1;
    }

    /// SponsorBlock categories to skip.
    pub fn sponsorblock_categories(&self) -> Vec<String> {
        match self.setting(crate::youtube::SPONSORBLOCK_SETTING) {
            Some(list) => list.split(',').filter(|c| !c.is_empty()).map(str::to_owned).collect(),
            None => flex_core::youtube::sponsorblock::default_categories(),
        }
    }

    pub fn set_sponsorblock_category(&mut self, category: &str, on: bool) {
        let mut list = self.sponsorblock_categories();
        list.retain(|c| c != category);
        if on {
            list.push(category.to_owned());
        }
        self.set_setting(crate::youtube::SPONSORBLOCK_SETTING, &list.join(","));
    }

    /// Starts finding a video's streams (and what to skip) ahead of playing it.
    pub fn prepare_youtube(&mut self, id: &str) {
        let (quality, categories) = (self.youtube_quality(), self.sponsorblock_categories());
        self.youtube.prepare(id, quality, categories);
    }

    pub fn yt_channels(&self) -> Vec<flex_core::youtube::store::Channel> {
        self.db.yt_channels().unwrap_or_default()
    }

    pub fn yt_latest(&self, limit: usize) -> Vec<flex_core::youtube::store::Video> {
        self.db.yt_latest(limit, self.youtube_shorts()).unwrap_or_default()
    }

    pub fn yt_channel_videos(&self, channel_id: &str, limit: usize) -> Vec<flex_core::youtube::store::Video> {
        self.db.yt_channel_videos(channel_id, limit, self.youtube_shorts()).unwrap_or_default()
    }

    /// Part-watched YouTube videos, minus any removed from Continue Watching (until played
    /// again).
    pub fn yt_continue(&self, limit: usize) -> Vec<(String, flex_core::youtube::store::WatchProgress)> {
        self.db.yt_continue(limit.min(self.youtube_continue_limit())).unwrap_or_default()
    }

    /// How many part-watched YouTube videos Continue Watching keeps (older ones drop off).
    pub fn youtube_continue_limit(&self) -> usize {
        self.setting(crate::youtube::CONTINUE_SETTING).and_then(|v| v.parse().ok()).unwrap_or(crate::youtube::DEFAULT_CONTINUE)
    }

    pub fn set_youtube_continue_limit(&mut self, keep: usize) {
        self.set_setting(crate::youtube::CONTINUE_SETTING, &keep.to_string());
        self.trim_yt_continue();
    }

    /// Drops the oldest YouTube videos from Continue Watching beyond the limit.
    fn trim_yt_continue(&mut self) {
        match self.db.trim_yt_continue(self.youtube_continue_limit(), now_unix()) {
            Ok(0) => {}
            Ok(n) => {
                log::info!("youtube: {n} older videos dropped from Continue Watching");
                self.continue_version += 1;
                self.youtube.version += 1;
            }
            Err(e) => log::warn!("trimming Continue Watching: {e:#}"),
        }
    }

    pub fn yt_progress(&self, id: &str) -> Option<flex_core::youtube::store::WatchProgress> {
        self.db.yt_progress(id).ok().flatten()
    }

    pub fn yt_progress_all(&self) -> HashMap<String, flex_core::youtube::store::WatchProgress> {
        self.db.yt_progress_all().unwrap_or_default()
    }

    pub fn set_yt_watched(&mut self, id: &str, title: &str, channel: &str, watched: bool) {
        if let Err(e) = self.db.set_yt_watched(id, title, channel, watched, now_unix()) {
            log::warn!("saving YouTube progress: {e:#}");
        }
        self.youtube.version += 1;
    }

    pub fn subscribe(&mut self, id: &str, title: &str) {
        match self.db.add_yt_channel(id, title, now_unix()) {
            Ok(true) => {
                self.toast(format!("Subscribed to {title}"));
                self.youtube.version += 1;
                // Its videos show up after the next refresh; start one if none is running.
                self.youtube.refresh(vec![id.to_owned()]);
            }
            Ok(false) => self.toast(format!("Already subscribed to {title}")),
            Err(e) => self.toast(format!("Couldn't subscribe: {e:#}")),
        }
    }

    pub fn unsubscribe(&mut self, id: &str) {
        if let Err(e) = self.db.remove_yt_channel(id) {
            self.toast(format!("Couldn't unsubscribe: {e:#}"));
        }
        self.youtube.version += 1;
    }

    pub fn is_subscribed(&self, channel_id: &str) -> bool {
        self.yt_channels().iter().any(|c| c.id == channel_id)
    }

    /// Imports a subscription list (Takeout CSV, FreeTube, NewPipe or OPML).
    pub fn import_subscriptions(&mut self, path: &Path) -> anyhow::Result<usize> {
        let text = std::fs::read_to_string(path)?;
        let found = flex_core::youtube::feeds::parse_subscriptions(&text);
        anyhow::ensure!(!found.is_empty(), "no YouTube channels in that file");
        let now = now_unix();
        let mut added = 0;
        for (id, title) in &found {
            added += usize::from(self.db.add_yt_channel(id, title, now)?);
        }
        self.youtube.version += 1;
        let ids = self.yt_channels().into_iter().map(|c| c.id).collect();
        self.youtube.refresh(ids);
        Ok(added)
    }

    /// Takes YouTube's background results: saves feeds, reports tool updates and new
    /// subscriptions, and refreshes feeds every so often.
    pub fn poll_youtube(&mut self) {
        use crate::youtube::Msg;
        for msg in self.youtube.poll() {
            match msg {
                Msg::Feed(id, Ok((title, videos))) => {
                    if let Err(e) = self.db.save_yt_feed(&id, &title, &videos, now_unix()) {
                        log::warn!("saving the feed for {id}: {e:#}");
                    }
                }
                Msg::Feed(id, Err(e)) => log::warn!("youtube: feed for {id}: {e}"),
                Msg::FeedsDone => self.youtube.version += 1,
                Msg::Tools(Ok(report)) => {
                    for (tool, version) in report.updated {
                        log::info!("youtube: {} {version} installed", tool.name());
                    }
                }
                Msg::Tools(Err(e)) => self.toast(format!("Couldn't set up YouTube: {e}")),
                Msg::Channel(Ok((id, title))) => self.subscribe(&id, &title),
                Msg::Channel(Err(e)) => self.toast(format!("Couldn't find that channel: {e}")),
                Msg::Resolved(..) | Msg::Segments(..) | Msg::Search(..) => {}
            }
        }
        if self.youtube_enabled() && self.youtube.tools.is_some() && self.youtube.refresh_due() {
            let ids = self.yt_channels().into_iter().map(|c| c.id).collect();
            self.youtube.refresh(ids);
        }
    }

    /// Handles plugin messages, tells plugins about playback and updates lights.
    /// `playing`: what the top screen is playing, if anything, and the library file.
    pub fn poll_plugins(&mut self, playing: Option<(Playback, Option<(i64, i64)>)>) {
        let mut wait = self.plugins.poll(&self.db);
        let plugin_toasts = self.plugins.take_toasts();
        self.toasts.extend(plugin_toasts);
        // Plugins hear about starts, pauses, resumes and stops (not every frame); a stop
        // carries where playback got to. Something new starting stops the old one first.
        let same = |a: &Playback, b: &Playback| a.state == b.state && a.title == b.title && a.kind == b.kind;
        let previous = self.playback_last.take();
        match (&playing, self.playback_sent.take()) {
            (Some((p, _)), Some(sent)) if same(p, &sent) => self.playback_sent = Some(sent),
            (Some((p, file)), sent) => {
                if sent.is_some_and(|s| s.title != p.title) {
                    self.send_stopped(previous);
                }
                let p = self.with_media(p.clone(), *file);
                self.plugins.send_playback(&p);
                self.playback_sent = Some(p);
            }
            (None, Some(_)) => self.send_stopped(previous),
            (None, None) => {}
        }
        self.playback_last = playing;
        let video = self.playback_sent.as_ref().is_some_and(|p| p.kind == "video");
        if let Some(lights) = self.ambient.tick(&self.plugins, video) {
            wait = Some(wait.map_or(lights, |w| w.min(lights)));
        }
        if let Some(wait) = wait {
            self.ctx.request_repaint_after(wait);
        }
    }

    fn send_stopped(&self, last: Option<(Playback, Option<(i64, i64)>)>) {
        let (last, file) = last.unwrap_or_default();
        let stopped = self.with_media(Playback { state: PlaybackState::Stopped, ..last }, file);
        self.plugins.send_playback(&stopped);
    }

    /// Adds what a library file is (a matched film or episode) to a playback event.
    fn with_media(&self, mut playback: Playback, file: Option<(i64, i64)>) -> Playback {
        if playback.media.is_none() && self.plugins.plugins().iter().any(|p| p.running() && p.subscribes("playback")) {
            playback.media = file.and_then(|(library_id, file_id)| self.media_info(library_id, file_id));
        }
        playback
    }

    /// A file as plugins see it: the film or episode it's matched to online.
    pub fn media_info(&self, library_id: i64, file_id: i64) -> Option<flex_plugin::Media> {
        let entry = self.lib(library_id)?.catalog()?.show_of_file(file_id)?;
        let (source, id) = entry.show.provider_id.as_deref()?.split_once(':')?;
        let id: u64 = id.parse().ok()?;
        let (tmdb, anilist) = match source {
            "tv" | "movie" => (Some(id), None),
            "anilist" => (None, Some(id)),
            _ => return None,
        };
        let media = flex_plugin::Media { title: entry.show.title.clone(), year: entry.show.year, tmdb, anilist, ..Default::default() };
        if entry.is_series() {
            let ep = entry.episodes.iter().find(|e| e.file_id == file_id)?;
            Some(flex_plugin::Media { kind: "episode".into(), season: Some(ep.season), episode: ep.number, ..media })
        } else {
            Some(flex_plugin::Media { kind: "movie".into(), ..media })
        }
    }

    /// The plugin host with the database it saves settings in.
    pub fn plugins_mut(&mut self) -> (&mut flex_core::plugins::Host, &Db) {
        (&mut self.plugins, &self.db)
    }

    /// Looks for plugins installed since Flex started.
    pub fn refresh_plugins(&mut self) {
        self.plugins.refresh(&flex_core::plugins::plugin_dirs(), &self.db);
    }

    /// Sets Settings → Ambient light option `choice` to its `value`th option.
    pub fn set_ambient(&mut self, choice: usize, value: usize) {
        self.ambient.values[choice] = value;
        self.set_setting(crate::ambient::CHOICES[choice].key, &value.to_string());
        self.ambient.changed();
    }

    pub fn poll_scans(&mut self) {
        self.auto_scan();
        self.poll_web();
        self.images.poll();
        self.shots.poll();
        let mut reorganise = Vec::new();
        let mut rematch = Vec::new();
        let mut fetch = Vec::new();
        let mut match_music = Vec::new();
        let mut music_artists = Vec::new();
        while let Ok(msg) = self.scan_rx.try_recv() {
            match msg {
                ScanMsg::Progress(id, n) => {
                    if let Some(l) = self.lib_mut(id) {
                        l.scanning = Some(n);
                    }
                }
                ScanMsg::Done(id, quiet, result) => {
                    self.scan_cancel.remove(&id);
                    // Nothing to redo if no files came or went (tree, search, organising).
                    let outcome = result.and_then(|done| match done.changed {
                        true => self.db.files(id).map(|files| (done, Some(files))).map_err(|e| format!("{e:#}")),
                        false => Ok((done, None)),
                    });
                    let Some(l) = self.lib_mut(id) else { continue };
                    l.scanning = None;
                    let toast = match outcome {
                        Ok((done, files)) => {
                            if let Some(files) = files {
                                l.tree = Arc::new(Tree::new(files));
                                l.version += 1;
                                reorganise.push(id);
                            }
                            (!quiet || done.changed).then(|| format!("{}: {}", l.lib.name, done.msg))
                        }
                        Err(e) => {
                            log::warn!("scan of {} failed: {e}", l.lib.name);
                            l.error = Some(e.clone());
                            (!quiet).then(|| format!("{}: scan failed: {e}", l.lib.name))
                        }
                    };
                    if !reorganise.is_empty() {
                        self.search = None;
                    }
                    if let Some(toast) = toast {
                        self.toast(toast);
                    }
                }
                ScanMsg::Matching(id, done, total) => {
                    if let Some(l) = self.lib_mut(id) {
                        l.matching = Some((done, total));
                    }
                    self.rebuild_catalog(id);
                }
                ScanMsg::Fetching(id, done, total) => {
                    if let Some(l) = self.lib_mut(id) {
                        l.fetching = Some((done, total));
                    }
                }
                ScanMsg::DetailsReady(id) => self.reload_details(id),
                ScanMsg::MusicMatching(id, done, total) => {
                    let matches = self.db.music_matches(id).unwrap_or_default();
                    if let Some(l) = self.lib_mut(id) {
                        l.matching = Some((done, total));
                        l.set_music_matches(matches);
                    }
                }
                ScanMsg::MusicMatchDone(id, result) => {
                    let matches = self.db.music_matches(id).unwrap_or_default();
                    let Some(l) = self.lib_mut(id) else { continue };
                    l.matching = None;
                    l.set_music_matches(matches);
                    match result {
                        Ok(summary) => {
                            let msg = format!("{}: {summary}", l.lib.name);
                            self.toast(msg);
                        }
                        Err(e) => {
                            let msg = format!("{}: MusicBrainz: {e}", l.lib.name);
                            self.toast(msg);
                        }
                    }
                    music_artists.push(id);
                }
                ScanMsg::MusicArtists(id, done, total) => {
                    self.music_artists = Arc::new(self.db.music_artists(i64::MAX).unwrap_or_default());
                    if let Some(l) = self.lib_mut(id) {
                        l.fetching = Some((done, total));
                        l.version += 1;
                    }
                }
                ScanMsg::MusicArtistReady(id) => {
                    self.music_artists = Arc::new(self.db.music_artists(i64::MAX).unwrap_or_default());
                    if let Some(l) = self.lib_mut(id) {
                        l.version += 1;
                    }
                }
                ScanMsg::MusicArtistsDone(id, result) => {
                    self.music_artists = Arc::new(self.db.music_artists(i64::MAX).unwrap_or_default());
                    let Some(l) = self.lib_mut(id) else { continue };
                    l.fetching = None;
                    l.version += 1;
                    if let Err(e) = result {
                        let msg = format!("{}: couldn't fetch artists: {e}", l.lib.name);
                        self.toast(msg);
                    }
                }
                ScanMsg::ReadingTags(id, done, total) => {
                    if let Some(l) = self.lib_mut(id) {
                        l.reading_tags = (done < total).then_some((done, total));
                    }
                }
                ScanMsg::Music(id, result) => {
                    let matches = self.db.music_matches(id).unwrap_or_default();
                    let Some(l) = self.lib_mut(id) else { continue };
                    (l.organising, l.reading_tags) = (false, None);
                    match result {
                        Ok(music) => {
                            l.music = Some(Arc::new(music));
                            l.set_music_matches(matches);
                            match_music.push(id);
                            self.search = None;
                        }
                        Err(e) => {
                            let msg = format!("{}: couldn't read the music: {e}", l.lib.name);
                            self.toast(msg);
                        }
                    }
                }
                ScanMsg::FetchDone(id, result) => {
                    let Some(l) = self.lib_mut(id) else { continue };
                    l.fetching = None;
                    if let Err(e) = result {
                        let msg = format!("{}: couldn't fetch details: {e}", l.lib.name);
                        self.toast(msg);
                    }
                    self.reload_details(id);
                }
                ScanMsg::MatchDone(id, result) => {
                    fetch.push(id);
                    let Some(l) = self.lib_mut(id) else { continue };
                    l.matching = None;
                    let msg = match result {
                        Ok(summary) => format!("{}: {summary}", l.lib.name),
                        Err(e) => format!("{}: matching failed: {e}", l.lib.name),
                    };
                    self.rebuild_catalog(id);
                    self.search = None;
                    self.toast(msg);
                }
                ScanMsg::Organised(id, result) => {
                    let Some(l) = self.lib_mut(id) else { continue };
                    l.organising = false;
                    match result {
                        Ok(org) => {
                            l.organised = Some(org);
                            l.version += 1;
                            rematch.push(id);
                        }
                        Err(e) => {
                            let msg = format!("{}: couldn't organise shows: {e}", l.lib.name);
                            self.toast(msg);
                        }
                    }
                }
            }
        }
        for id in reorganise {
            self.organise(id);
        }
        for id in match_music {
            self.start_music_matching(id);
            // Nothing to look up: go straight on to the artists.
            if self.lib(id).is_some_and(|l| l.matching.is_none()) {
                music_artists.push(id);
            }
        }
        for id in music_artists {
            self.start_music_artists(id);
        }
        for id in rematch {
            self.start_matching(id);
            // Nothing to match: go straight on to details.
            if self.lib(id).is_some_and(|l| l.matching.is_none()) {
                fetch.push(id);
            }
        }
        for id in fetch {
            self.start_details(id);
        }
    }

    pub fn search(&mut self, query: &str, scope: flex_core::search::Scope, limit: usize) -> Vec<Item> {
        if self.search.is_none() {
            let started = std::time::Instant::now();
            let index = SearchIndex::new(self.search_items());
            log::info!("search index: {} items in {:?}", index.len(), started.elapsed());
            self.search = Some(index);
        }
        self.search.as_mut().map_or_else(Vec::new, |i| i.search(query, scope, limit))
    }

    /// Everything searchable: shows/films (matched title plus folder titles as alternative
    /// names), episodes by clean name, every folder and file, and music libraries' artists,
    /// albums and tracks.
    fn search_items(&self) -> Vec<Item> {
        use flex_core::search::{Target, music_items, path_items};
        let mut items = Vec::new();
        for l in self.libs.iter().filter(|l| l.is_music()) {
            if let Some(music) = &l.music {
                items.extend(music_items(&l.lib, music));
            }
        }
        for l in self.libs.iter().filter(|l| !l.is_music()) {
            if let Some(catalog) = l.catalog() {
                for entry in &catalog.shows {
                    let mut also: Vec<String> = entry.groups.iter().map(|g| g.title.clone()).collect();
                    if let Some(alt) = entry.show.provider_id.as_ref().and_then(|p| self.alt_titles.get(p)) {
                        also.extend(alt.iter().cloned());
                    }
                    let context = match entry.show.year {
                        Some(y) => format!("{} · {y}", l.lib.name),
                        None => l.lib.name.clone(),
                    };
                    let target = Target::Show { library_id: l.lib.id, show_id: entry.show.id };
                    items.push(Item::new(entry.show.title.clone(), context, target, 0, &also));
                    if entry.is_series() {
                        for e in &entry.episodes {
                            let Some(titles) = self.titles(l.lib.id, e.file_id) else { continue };
                            let target = Target::Episode { library_id: l.lib.id, file_id: e.file_id };
                            items.push(Item::new(titles.full(), l.lib.name.clone(), target, 0, &[]));
                        }
                    }
                }
            }
            items.extend(path_items(&l.lib, &l.tree));
        }
        items
    }

    pub fn play_request(&self, library_id: i64, file_id: i64) -> Option<PlayRequest> {
        let state = self.lib(library_id)?;
        let file = state.tree.file(file_id)?;
        let titles = self.titles(library_id, file_id).filter(|t| t.show.is_some());
        // DVD/Blu-ray chunks play as the whole disc, titled after its folder.
        let (path, title) = match flex_core::parse::disc_folder(&file.rel_path) {
            Some(disc) => {
                let named = disc.rsplit_once('/').map_or(disc, |(parent, _)| parent);
                let title = named.rsplit('/').next().unwrap_or(named).to_owned();
                (flex_core::library::join_rel(&state.lib.root, disc), titles.map_or(title, |t| t.full()))
            }
            None => {
                let fallback = || flex_core::library::display_name(file.file_name(), true);
                (file.abs_path(&state.lib.root), titles.map_or_else(fallback, |t| t.full()))
            }
        };
        Some(PlayRequest { path, title, library_id, file_id, start: None })
    }

    /// Screenshot `index` of a file (see [`crate::shots`]), taken in the background if needed.
    pub fn shot(&mut self, library_id: i64, file_id: i64, index: usize) -> crate::shots::Shot {
        let Some(state) = self.libs.iter().find(|l| l.lib.id == library_id) else {
            return crate::shots::Shot::Failed("no such library".into());
        };
        let Some(file) = state.tree.file(file_id) else { return crate::shots::Shot::Failed("no such file".into()) };
        let path = match flex_core::parse::disc_folder(&file.rel_path) {
            Some(disc) => flex_core::library::join_rel(&state.lib.root, disc),
            None => file.abs_path(&state.lib.root),
        };
        self.shots.get(file_id, file.size, &path, index)
    }

    /// The next episode after `req` in the same season, for episodic libraries.
    pub fn next_after(&self, req: &PlayRequest) -> Option<PlayRequest> {
        self.neighbour(req, 1)
    }

    /// The episode before `req` in the same season, for episodic libraries.
    pub fn prev_before(&self, req: &PlayRequest) -> Option<PlayRequest> {
        self.neighbour(req, -1)
    }

    /// The episode before/after in the same season (show order once organised, folder
    /// order until then). Movies have no neighbours.
    fn neighbour(&self, req: &PlayRequest, delta: isize) -> Option<PlayRequest> {
        let state = self.lib(req.library_id)?;
        let id = match state.catalog().filter(|c| c.show_of_file(req.file_id).is_some()) {
            Some(catalog) if catalog.show_of_file(req.file_id)?.is_series() => {
                catalog.neighbour_in_season(req.file_id, delta)?
            }
            Some(_) => return None, // a film
            None if !state.lib.kind.is_episodic() => return None,
            None if delta > 0 => state.tree.next_in_folder(req.file_id)?.id,
            None => state.tree.prev_in_folder(req.file_id)?.id,
        };
        self.play_request(req.library_id, id)
    }
}
