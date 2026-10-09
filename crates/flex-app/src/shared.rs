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
use flex_core::parse::Parsed;
use flex_core::progress::{self, ContinueItem, Progress};
use flex_core::search::{Item, SearchIndex};
use flex_core::{Db, Library, LibraryKind, Tree};

use crate::mpv::GetProcAddress;

pub struct LibState {
    pub lib: Library,
    pub tree: Tree,
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
}

/// Display names for a file, from online metadata where available.
#[derive(Clone, Debug)]
pub struct Titles {
    /// Show or film title (None if the file isn't organised yet).
    pub show: Option<String>,
    /// For episodes: "S1E5 · Title", "S1E5", or the file name.
    pub episode: Option<String>,
    /// The best single name for the file itself: episode title, film title, else file name.
    pub name: String,
}

impl Titles {
    /// "Show · S1E5 · Title" for episodes, the title for films.
    pub fn full(&self) -> String {
        match (&self.show, &self.episode) {
            (Some(show), Some(ep)) => format!("{show} · {ep}"),
            (Some(show), None) => show.clone(),
            (None, _) => self.name.clone(),
        }
    }
}

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
            tree,
            scanning: None,
            error: None,
            version: 0,
            organised: None,
            organising: false,
            matching: None,
            fetching: None,
        }
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
        } else if let Some(e) = &self.error {
            e.clone()
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

const WATCHED_THRESHOLD_SETTING: &str = "watched_threshold";
/// Choices offered in Settings for "mark watched at".
pub const WATCHED_THRESHOLDS: [f64; 4] = [0.8, 0.85, 0.9, 0.95];

pub fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

enum ScanMsg {
    Progress(i64, usize),
    Done(i64, Result<String, String>),
    Organised(i64, Result<Organised, String>),
    /// Online matching progress (done, total); the catalog is reloaded on these.
    Matching(i64, usize, usize),
    MatchDone(i64, Result<String, String>),
    /// Details fetching progress (done, total), and completion.
    Fetching(i64, usize, usize),
    /// TMDB details are in (the slower AniList voice-cast pass may still be running).
    DetailsReady(i64),
    FetchDone(i64, Result<usize, String>),
}

/// Re-fetch details this often, so new episodes of running series get their titles.
const DETAILS_MAX_AGE_SECS: i64 = 30 * 24 * 3600;
const VOICE_MAX_AGE_SECS: i64 = 180 * 24 * 3600;

const TMDB_TOKEN_SETTING: &str = "tmdb_token";
/// Bump when grouping/matching logic improves: automatic results are redone on the next
/// start (manual choices are kept).
const MATCHER_VERSION: &str = "4";
const MATCHER_VERSION_SETTING: &str = "matcher_version";
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
    /// Episode titles etc. for matched series, by provider id ("tv:123").
    pub series: HashMap<String, flex_core::db::SeriesMeta>,
    /// The window's monitor refresh rate, for smooth video timing.
    pub display_fps: Option<f64>,
    search: Option<SearchIndex>,
    scan_tx: Sender<ScanMsg>,
    scan_rx: Receiver<ScanMsg>,
    /// Cancel flags for running scans, by library id.
    scan_cancel: HashMap<i64, Arc<AtomicBool>>,
    toasts: Vec<String>,
}

impl Shared {
    pub fn new(ctx: &egui::Context, proc_address: Option<GetProcAddress>) -> Result<Self> {
        let db_path = flex_core::data_dir().join("flex.db");
        log::info!("database: {}", db_path.display());
        let db = Db::open(&db_path)?;
        let (scan_tx, scan_rx) = channel();
        let mut shared = Self {
            ctx: ctx.clone(),
            proc_address,
            db_path,
            db,
            libs: Vec::new(),
            search_query: String::new(),
            series: HashMap::new(),
            images: crate::images::Images::new(ctx, flex_core::data_dir().join("images")),
            display_fps: None,
            search: None,
            scan_tx,
            scan_rx,
            scan_cancel: HashMap::new(),
            toasts: Vec::new(),
        };
        shared.series = shared.db.series_meta()?;
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
                shared.start_scan(id);
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

    pub fn watched_threshold(&self) -> f64 {
        self.db
            .setting(WATCHED_THRESHOLD_SETTING)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(progress::DEFAULT_WATCHED_THRESHOLD)
    }

    pub fn set_watched_threshold(&mut self, threshold: f64) {
        if let Err(e) = self.db.set_setting(WATCHED_THRESHOLD_SETTING, &threshold.to_string()) {
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
            && let Some(file) = state.tree.file_mut(req.file_id)
        {
            file.progress = new;
            state.version += 1;
        }
        Some(new)
    }

    /// Adds a viewing session to the history.
    pub fn add_history(&mut self, req: &PlayRequest, played_at: i64, position: Option<f64>, completed: bool) {
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
                    l.tree = Tree::new(files);
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
        if let Some(state) = self.lib_mut(library_id) {
            for id in ids {
                if let Some(file) = state.tree.file_mut(*id) {
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
        let organised = self.libs.iter().filter_map(|l| l.catalog().map(|c| (&l.lib, &l.tree, c)));
        let mut items = progress::continue_watching_shows(organised, limit);
        let pending = self.libs.iter().filter(|l| l.organised.is_none()).map(|l| (&l.lib, &l.tree));
        items.extend(progress::continue_watching(pending, limit));
        items.truncate(limit);
        items
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

    /// Sorts a library into shows on a background thread (parsing ~10k names takes a second).
    pub fn organise(&mut self, id: i64) {
        let Some(state) = self.lib_mut(id) else { return };
        state.organising = true;
        let kind = state.lib.kind;
        let files = state.tree.files().to_vec();
        let (tx, ctx, db_path) = (self.scan_tx.clone(), self.ctx.clone(), self.db_path.clone());
        std::thread::spawn(move || {
            let result = (|| -> Result<Organised> {
                let started = std::time::Instant::now();
                let tree = Tree::new(files);
                let parsed = flex_core::parse::parse_library(kind, tree.files());
                let groups = catalog::derive_groups(kind, &tree, &parsed);
                let mut db = Db::open(&db_path)?;
                db.sync_groups(id, &groups)?;
                let catalog = Catalog::build(&tree, &parsed, &groups, db.shows(id)?, &db.group_matches(id)?);
                log::info!(
                    "organised library {id}: {} groups into {} shows in {:?}",
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
                    if done % 10 == 0 || done == total {
                        let _ = tx.send(ScanMsg::Fetching(id, done, total));
                        ctx.request_repaint();
                    }
                };
                let tmdb = flex_core::metadata::Tmdb::new(&token);
                let mut db = Db::open(&db_path)?;
                let mut fetched = 0;
                for (i, (show_id, provider_id, title)) in todo.iter().enumerate() {
                    let outcome = (|| -> Result<()> {
                        let details = tmdb.details(provider_id)?;
                        db.save_details(*show_id, provider_id, &details)?;
                        if let Some(tv_id) = provider_id.strip_prefix("tv:") {
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
                    match flex_core::metadata::anilist_voice_cast(title) {
                        Ok(cast) => db.save_voice_cast(*show_id, provider_id, &cast)?,
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
            Ok(series) => self.series = series,
            Err(e) => log::warn!("loading episode titles: {e:#}"),
        }
        self.rebuild_catalog(id);
        self.search = None;
    }

    /// Display names for a file: show and episode titles from online metadata where
    /// available, the file name otherwise.
    pub fn titles(&self, library_id: i64, file_id: i64) -> Option<Titles> {
        let state = self.lib(library_id)?;
        let file = state.tree.file(file_id)?;
        let file_name = flex_core::library::display_name(file.file_name(), true);
        let Some(entry) = state.catalog().and_then(|c| c.show_of_file(file_id)) else {
            return Some(Titles { show: None, episode: None, name: file_name });
        };
        let show = entry.show.title.clone();
        if !entry.is_series() {
            return Some(Titles { name: show.clone(), show: Some(show), episode: None });
        }
        let ep = entry.episodes.iter().find(|e| e.file_id == file_id)?;
        let meta = match (ep.season, ep.number, entry.show.provider_id.as_ref()) {
            // Specials' numbering rarely lines up with TMDB's, so they keep file names.
            (s, Some(n), Some(pid)) if s > 0 => self.series.get(pid).and_then(|m| m.episode(s, n)),
            _ => None,
        };
        let title = meta.map(|m| m.title.clone()).filter(|t| !t.is_empty());
        let code = match (ep.season, ep.number) {
            (0, _) => None,
            (s, Some(n)) => Some(format!("S{s}E{n}")),
            _ => None,
        };
        let episode = match (code, &title) {
            (Some(c), Some(t)) => format!("{c} · {t}"),
            (Some(c), None) => c,
            (None, Some(t)) => t.clone(),
            (None, None) => file_name.clone(),
        };
        Some(Titles { show: Some(show), episode: Some(episode), name: title.unwrap_or(file_name) })
    }

    /// TMDB metadata (description, still) for an episode, if known.
    pub fn episode_meta(&self, library_id: i64, file_id: i64) -> Option<&flex_core::db::EpisodeMeta> {
        let entry = self.lib(library_id)?.catalog()?.show_of_file(file_id)?;
        let ep = entry.episodes.iter().find(|e| e.file_id == file_id)?;
        let pid = entry.show.provider_id.as_ref()?;
        (ep.season > 0).then_some(())?;
        self.series.get(pid)?.episode(ep.season, ep.number?)
    }

    pub fn credits(&self, provider_id: &str, kind: &str) -> Vec<flex_core::metadata::CastMember> {
        self.db.credits(provider_id, kind).unwrap_or_default()
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
        self.toasts.push(msg.into());
    }

    pub fn take_toasts(&mut self) -> Vec<String> {
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
        self.start_scan(id);
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
        if let Some(l) = self.lib_mut(id) {
            l.lib.kind = kind;
            l.version += 1;
        }
        Ok(())
    }

    pub fn start_scan(&mut self, id: i64) {
        let Some(state) = self.lib_mut(id) else { return };
        if state.scanning.is_some() {
            return;
        }
        state.scanning = Some(0);
        state.error = None;
        let root = state.lib.root.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.scan_cancel.insert(id, cancel.clone());
        let (tx, ctx, db_path) = (self.scan_tx.clone(), self.ctx.clone(), self.db_path.clone());
        std::thread::spawn(move || {
            let result = (|| -> Result<String> {
                let scan = flex_core::scan::scan(&root, &cancel, |n| {
                    let _ = tx.send(ScanMsg::Progress(id, n));
                    ctx.request_repaint();
                })?;
                for e in &scan.errors {
                    log::warn!("scan: {e}");
                }
                // A separate connection: SQLite (WAL) handles the concurrent reader on the UI thread.
                let summary = Db::open(&db_path)?.apply_scan(id, &scan.files)?;
                let mut msg = format!(
                    "{} videos ({} new, {} removed)",
                    thousands(summary.total),
                    summary.added,
                    summary.removed
                );
                if !scan.errors.is_empty() {
                    msg += &format!(", {} unreadable", scan.errors.len());
                }
                Ok(msg)
            })();
            let _ = tx.send(ScanMsg::Done(id, result.map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
    }

    pub fn scan_all(&mut self) {
        let ids: Vec<i64> = self.libs.iter().map(|l| l.lib.id).collect();
        for id in ids {
            self.start_scan(id);
        }
    }

    /// Applies finished/progressing scans. Call once per frame.
    pub fn poll_scans(&mut self) {
        self.images.poll();
        let mut reorganise = Vec::new();
        let mut rematch = Vec::new();
        let mut fetch = Vec::new();
        while let Ok(msg) = self.scan_rx.try_recv() {
            match msg {
                ScanMsg::Progress(id, n) => {
                    if let Some(l) = self.lib_mut(id) {
                        l.scanning = Some(n);
                    }
                }
                ScanMsg::Done(id, result) => {
                    self.scan_cancel.remove(&id);
                    let outcome = result.and_then(|summary| {
                        self.db.files(id).map(|files| (summary, files)).map_err(|e| format!("{e:#}"))
                    });
                    let Some(l) = self.lib_mut(id) else { continue };
                    l.scanning = None;
                    let toast = match outcome {
                        Ok((summary, files)) => {
                            l.tree = Tree::new(files);
                            l.version += 1;
                            reorganise.push(id);
                            format!("{}: {summary}", l.lib.name)
                        }
                        Err(e) => {
                            l.error = Some(e.clone());
                            format!("{}: scan failed: {e}", l.lib.name)
                        }
                    };
                    self.search = None;
                    self.toast(toast);
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

    pub fn search(&mut self, query: &str, limit: usize) -> Vec<Item> {
        let libs = &self.libs;
        self.search
            .get_or_insert_with(|| SearchIndex::build(libs.iter().map(|l| (&l.lib, &l.tree))))
            .search(query, limit)
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
