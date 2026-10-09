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
        Self { lib, tree, scanning: None, error: None, version: 0, organised: None, organising: false }
    }

    pub fn catalog(&self) -> Option<&Catalog> {
        self.organised.as_ref().map(|o| o.catalog.as_ref())
    }

    /// One-line status, e.g. "1,234 videos" or "Scanning… 120 found".
    pub fn status(&self) -> String {
        if let Some(n) = self.scanning {
            format!("Scanning… {} found", thousands(n))
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

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

enum ScanMsg {
    Progress(i64, usize),
    Done(i64, Result<String, String>),
    Organised(i64, Result<Organised, String>),
}

pub struct Shared {
    pub ctx: egui::Context,
    pub proc_address: Option<GetProcAddress>,
    db_path: PathBuf,
    db: Db,
    pub libs: Vec<LibState>,
    pub search_query: String,
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
            search: None,
            scan_tx,
            scan_rx,
            scan_cancel: HashMap::new(),
            toasts: Vec::new(),
        };
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

    /// Records having played `req` up to `position` (seconds) of `duration`.
    pub fn record_progress(&mut self, req: &PlayRequest, position: f64, duration: f64) {
        let threshold = self.watched_threshold();
        let Some(old) = self.progress(req.library_id, req.file_id) else { return };
        let Some(new) = old.after_playing(position, duration, threshold, now_unix()) else { return };
        if new == old {
            return;
        }
        if let Err(e) = self.db.set_progress(req.file_id, &new) {
            log::warn!("saving progress: {e:#}");
            return;
        }
        if let Some(state) = self.lib_mut(req.library_id)
            && let Some(file) = state.tree.file_mut(req.file_id)
        {
            file.progress = new;
            state.version += 1;
        }
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
        let mut reorganise = Vec::new();
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
                ScanMsg::Organised(id, result) => {
                    let Some(l) = self.lib_mut(id) else { continue };
                    l.organising = false;
                    match result {
                        Ok(org) => {
                            l.organised = Some(org);
                            l.version += 1;
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
        // DVD/Blu-ray chunks play as the whole disc, titled after its folder.
        let (path, title) = match flex_core::parse::disc_folder(&file.rel_path) {
            Some(disc) => {
                let named = disc.rsplit_once('/').map_or(disc, |(parent, _)| parent);
                let title = named.rsplit('/').next().unwrap_or(named).to_owned();
                (flex_core::library::join_rel(&state.lib.root, disc), title)
            }
            None => (file.abs_path(&state.lib.root), flex_core::library::display_name(file.file_name(), true)),
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
