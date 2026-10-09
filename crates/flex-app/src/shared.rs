//! State shared by all screens: the database, libraries with their folder trees,
//! background scans and the search index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::Result;
use eframe::egui;
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
}

impl LibState {
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
}

enum ScanMsg {
    Progress(i64, usize),
    Done(i64, Result<String, String>),
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
            shared.libs.push(LibState { lib, tree, scanning: None, error: None, version: 0 });
            if never_scanned {
                shared.start_scan(id);
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

    pub fn add_library(&mut self, root: &Path, kind: LibraryKind) -> Result<()> {
        let name = root
            .file_name()
            .map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
        let id = self.db.add_library(&name, root, kind)?;
        let lib = Library { id, name, root: root.to_path_buf(), kind };
        self.libs.push(LibState { lib, tree: Tree::new(Vec::new()), scanning: None, error: None, version: 0 });
        self.libs.sort_by_key(|l| l.lib.name.to_lowercase());
        self.start_scan(id);
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
            }
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
        Some(PlayRequest {
            path: file.abs_path(&state.lib.root),
            title: flex_core::library::display_name(file.file_name(), true),
            library_id,
            file_id,
        })
    }

    /// The next episode after `req`: the next file in the same folder, for episodic libraries.
    pub fn next_after(&self, req: &PlayRequest) -> Option<PlayRequest> {
        let state = self.lib(req.library_id)?;
        if !state.lib.kind.is_episodic() {
            return None;
        }
        let next = state.tree.next_in_folder(req.file_id)?;
        self.play_request(req.library_id, next.id)
    }
}
