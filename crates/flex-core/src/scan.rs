//! Walks a library folder and collects its video (or, for music, audio) files.
//!
//! Rescans are quick: adding, removing or renaming a file changes its folder's modified
//! time, so folders whose time matches the previous scan aren't read again. Only those
//! folders are looked at (a few thousand, against tens of thousands of files), several at
//! a time, since over a network share the time goes on waiting for replies. In a changed
//! folder only new files are looked at. A full scan (no previous folder times) looks at
//! everything again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};

use crate::LibraryKind;

#[derive(Clone, Debug, PartialEq)]
pub struct ScannedFile {
    pub rel_path: String,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub files: Vec<ScannedFile>,
    /// Entries that couldn't be read (permissions, unreadable names, vanished files).
    pub errors: Vec<String>,
    /// Every folder seen ("" is the root) with its modified time in nanoseconds, for the
    /// next scan. 0 means "read it again next time".
    pub dirs: Vec<(String, i64)>,
    /// How many folders were read (the rest were unchanged since the previous scan).
    pub dirs_read: usize,
}

/// What the previous scan found.
#[derive(Debug, Default)]
pub struct Previous {
    pub files: Vec<ScannedFile>,
    /// Folder -> modified time in nanoseconds, as in [`ScanResult::dirs`]. Empty for a
    /// full scan.
    pub dirs: HashMap<String, i64>,
}

/// Folders read at once.
const THREADS: usize = 8;
/// Guards against symlink loops.
const MAX_DEPTH: usize = 64;
/// A folder changed this recently might change again within the same clock tick, so its
/// time isn't trusted for skipping it next time.
const SETTLE: Duration = Duration::from_secs(5);

/// Folders NAS software litters around that never contain real media.
fn is_ignored(name: &str) -> bool {
    name.starts_with('.')
        || name.starts_with('$')
        || name.starts_with('@') // Synology @eaDir etc.
        || name.eq_ignore_ascii_case("#recycle")
        || name.eq_ignore_ascii_case("System Volume Information")
}

fn nanos(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as i64)
}

fn join(root: &Path, rel: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(rel.split('/').filter(|c| !c.is_empty()));
    path
}

fn child(rel: &str, name: &str) -> String {
    if rel.is_empty() { name.to_owned() } else { format!("{rel}/{name}") }
}

fn parent(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(p, _)| p)
}

/// The previous scan's files and subfolders, per folder.
struct Index<'a> {
    files: HashMap<&'a str, Vec<&'a ScannedFile>>,
    /// Files by path, to skip looking at known files again in a changed folder; empty
    /// for a full scan.
    known: HashMap<&'a str, &'a ScannedFile>,
    subdirs: HashMap<&'a str, Vec<&'a str>>,
}

impl<'a> Index<'a> {
    fn new(previous: &'a Previous) -> Self {
        let mut files: HashMap<&str, Vec<&ScannedFile>> = HashMap::new();
        for f in &previous.files {
            files.entry(parent(&f.rel_path)).or_default().push(f);
        }
        let mut subdirs: HashMap<&str, Vec<&str>> = HashMap::new();
        for d in previous.dirs.keys().filter(|d| !d.is_empty()) {
            subdirs.entry(parent(d)).or_default().push(d);
        }
        let known = if previous.dirs.is_empty() {
            HashMap::new()
        } else {
            previous.files.iter().map(|f| (f.rel_path.as_str(), f)).collect()
        };
        Self { files, subdirs, known }
    }
}

#[derive(Default)]
struct Found {
    files: Vec<ScannedFile>,
    errors: Vec<String>,
    dirs: Vec<(String, i64)>,
    dirs_read: usize,
}

/// Folders waiting to be looked at, and how many are being looked at now.
struct Queue {
    todo: Mutex<(Vec<(String, usize)>, usize)>,
    wake: Condvar,
}

impl Queue {
    /// The next folder, or None once there's nothing left (or the scan was cancelled).
    fn next(&self, cancel: &AtomicBool) -> Option<(String, usize)> {
        let mut todo = self.todo.lock().unwrap();
        loop {
            if cancel.load(Ordering::Relaxed) {
                self.wake.notify_all();
                return None;
            }
            if let Some(dir) = todo.0.pop() {
                todo.1 += 1;
                return Some(dir);
            }
            if todo.1 == 0 {
                return None;
            }
            todo = self.wake.wait(todo).unwrap();
        }
    }

    /// Adds a folder's subfolders and marks it done.
    fn done(&self, subdirs: Vec<(String, usize)>) {
        let mut todo = self.todo.lock().unwrap();
        todo.0.extend(subdirs);
        todo.1 -= 1;
        self.wake.notify_all();
    }
}

/// Looks at one folder: reuses the previous scan's findings if it hasn't changed, else
/// reads it. Returns its subfolders.
fn visit(root: &Path, kind: LibraryKind, rel: &str, previous: &Previous, index: &Index, settled: i64, out: &mut Found) -> Vec<String> {
    let path = join(root, rel);
    let mtime = match std::fs::metadata(&path).and_then(|m| m.modified()) {
        Ok(t) => nanos(t),
        Err(e) => {
            out.errors.push(format!("{}: {e}", path.display()));
            return Vec::new();
        }
    };
    if mtime != 0 && previous.dirs.get(rel) == Some(&mtime) {
        out.dirs.push((rel.to_owned(), mtime));
        out.files.extend(index.files.get(rel).into_iter().flatten().map(|f| (*f).clone()));
        return index.subdirs.get(rel).into_iter().flatten().map(|d| (*d).to_owned()).collect();
    }
    out.dirs_read += 1;
    let entries = match std::fs::read_dir(&path) {
        Ok(e) => e,
        Err(e) => {
            out.errors.push(format!("{}: {e}", path.display()));
            return Vec::new();
        }
    };
    let errors_before = out.errors.len();
    let mut subdirs = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                out.errors.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_ignored(&name) {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(e) => {
                out.errors.push(format!("{}: {e}", entry.path().display()));
                continue;
            }
        };
        let is_link = file_type.is_symlink();
        if file_type.is_dir() || (is_link && entry.path().is_dir()) {
            subdirs.push(child(rel, &name));
            continue;
        }
        if !kind.wants(&entry.path()) {
            continue;
        }
        let rel_path = child(rel, &name);
        if let Some(known) = index.known.get(rel_path.as_str()) {
            out.files.push((*known).clone());
            continue;
        }
        // Follows symlinks to files.
        match std::fs::metadata(entry.path()) {
            Ok(meta) if meta.is_file() => out.files.push(ScannedFile {
                rel_path,
                size: meta.len(),
                mtime: meta.modified().map_or(0, |t| nanos(t) / 1_000_000_000),
            }),
            Ok(_) => {}
            Err(e) => out.errors.push(format!("{}: {e}", entry.path().display())),
        }
    }
    // Read it again next time if it changed just now or something in it couldn't be read.
    let trusted = mtime < settled && out.errors.len() == errors_before;
    out.dirs.push((rel.to_owned(), if trusted { mtime } else { 0 }));
    subdirs
}

/// Scans `root` recursively for the files `kind` wants, reading only the folders that
/// changed since `previous`. `progress` is called periodically with the number found.
/// Setting `cancel` stops the scan with an error.
///
/// Fails if `root` isn't an accessible folder, or turns up nothing when the previous scan
/// found files, so an unmounted share is never mistaken for an empty library.
pub fn scan(
    root: &Path,
    kind: LibraryKind,
    previous: &Previous,
    cancel: &AtomicBool,
    progress: impl FnMut(usize),
) -> Result<ScanResult> {
    scan_settled(root, kind, previous, cancel, progress, SETTLE)
}

fn scan_settled(
    root: &Path,
    kind: LibraryKind,
    previous: &Previous,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize),
    settle: Duration,
) -> Result<ScanResult> {
    if !root.is_dir() {
        bail!("{} isn't available. Is the drive or share connected?", root.display());
    }
    let settled = nanos(SystemTime::now() - settle);
    let index = Index::new(previous);
    let queue = Queue { todo: Mutex::new((vec![(String::new(), 0)], 0)), wake: Condvar::new() };
    let found = AtomicUsize::new(0);
    let parts: Vec<Found> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..THREADS)
            .map(|_| {
                s.spawn(|| {
                    let mut out = Found::default();
                    while let Some((rel, depth)) = queue.next(cancel) {
                        let before = out.files.len();
                        let subdirs = visit(root, kind, &rel, previous, &index, settled, &mut out);
                        found.fetch_add(out.files.len() - before, Ordering::Relaxed);
                        let deeper = if depth < MAX_DEPTH { subdirs } else { Vec::new() };
                        queue.done(deeper.into_iter().map(|d| (d, depth + 1)).collect());
                    }
                    out
                })
            })
            .collect();
        while !workers.iter().all(|w| w.is_finished()) {
            progress(found.load(Ordering::Relaxed));
            std::thread::sleep(Duration::from_millis(100));
        }
        workers.into_iter().map(|w| w.join().expect("scan thread panicked")).collect()
    });
    if cancel.load(Ordering::Relaxed) {
        bail!("scan cancelled");
    }
    let mut result = ScanResult::default();
    for part in parts {
        result.files.extend(part.files);
        result.errors.extend(part.errors);
        result.dirs.extend(part.dirs);
        result.dirs_read += part.dirs_read;
    }
    result.files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    progress(result.files.len());
    if result.files.is_empty() && !previous.files.is_empty() {
        bail!("{} is empty. Is the drive or share connected?", root.display());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flex-scan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn touch(dir: &Path, rel: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    fn paths(result: &ScanResult) -> Vec<&str> {
        result.files.iter().map(|f| f.rel_path.as_str()).collect()
    }

    /// A scan that trusts folder times straight away.
    fn rescan(dir: &Path, previous: &ScanResult) -> Result<ScanResult> {
        let previous = Previous { files: previous.files.clone(), dirs: previous.dirs.iter().cloned().collect() };
        scan_settled(dir, LibraryKind::Tv, &previous, &AtomicBool::new(false), |_| {}, Duration::ZERO)
    }

    #[test]
    fn finds_videos_and_skips_junk() {
        let dir = temp_dir("junk");
        for p in ["Show/Season 1/ep1.mkv", "Show/Season 1/ep1.nfo", ".hidden/x.mkv", "@eaDir/y.mp4", "movie.MP4"] {
            touch(&dir, p);
        }
        let none = Previous::default();
        let found = scan(&dir, LibraryKind::Tv, &none, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(paths(&found), ["Show/Season 1/ep1.mkv", "movie.MP4"]);
        touch(&dir, "song.opus");
        let music = scan(&dir, LibraryKind::Music, &none, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(paths(&music), ["song.opus"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rescans_read_only_changed_folders() {
        let dir = temp_dir("incremental");
        for p in ["A/S1/1.mkv", "A/S1/2.mkv", "B/1.mkv", "Extras/Empty/readme.txt"] {
            touch(&dir, p);
        }
        let first = rescan(&dir, &ScanResult::default()).unwrap();
        assert_eq!(paths(&first), ["A/S1/1.mkv", "A/S1/2.mkv", "B/1.mkv"]);
        assert_eq!(first.dirs_read, 6, "root, A, A/S1, B, Extras, Extras/Empty");

        let same = rescan(&dir, &first).unwrap();
        assert_eq!((same.files.clone(), same.dirs_read), (first.files.clone(), 0));

        // A new episode deep down: only its folder is read again.
        std::thread::sleep(Duration::from_millis(20));
        touch(&dir, "A/S1/3.mkv");
        let added = rescan(&dir, &same).unwrap();
        assert_eq!(paths(&added), ["A/S1/1.mkv", "A/S1/2.mkv", "A/S1/3.mkv", "B/1.mkv"]);
        assert_eq!(added.dirs_read, 1);

        // A new folder in a folder with no videos, then a show removed.
        std::thread::sleep(Duration::from_millis(20));
        touch(&dir, "Extras/Empty/New/1.mkv");
        std::fs::remove_dir_all(dir.join("B")).unwrap();
        let changed = rescan(&dir, &added).unwrap();
        assert_eq!(paths(&changed), ["A/S1/1.mkv", "A/S1/2.mkv", "A/S1/3.mkv", "Extras/Empty/New/1.mkv"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unchanged_folders_are_taken_from_the_previous_scan() {
        let dir = temp_dir("trust");
        touch(&dir, "A/1.mkv");
        let mut first = rescan(&dir, &ScanResult::default()).unwrap();
        // Pretend the previous scan saw another file: with the folder unchanged it's kept.
        first.files.push(ScannedFile { rel_path: "A/2.mkv".into(), size: 1, mtime: 0 });
        assert_eq!(paths(&rescan(&dir, &first).unwrap()), ["A/1.mkv", "A/2.mkv"]);
        // In a changed folder, files seen before aren't looked at again.
        std::thread::sleep(Duration::from_millis(20));
        touch(&dir, "A/3.mkv");
        first.files[0].size = 99;
        let changed = rescan(&dir, &first).unwrap();
        assert_eq!(paths(&changed), ["A/1.mkv", "A/3.mkv"]);
        assert_eq!(changed.files[0].size, 99);
        // A full scan (no folder times) reads everything.
        first.dirs.clear();
        let full = rescan(&dir, &first).unwrap();
        assert_eq!(paths(&full), ["A/1.mkv", "A/3.mkv"]);
        assert_eq!(full.files[0].size, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recently_changed_folders_are_read_again() {
        let dir = temp_dir("settle");
        touch(&dir, "A/1.mkv");
        let first = scan(&dir, LibraryKind::Tv, &Previous::default(), &AtomicBool::new(false), |_| {}).unwrap();
        assert!(first.dirs.iter().all(|(_, mtime)| *mtime == 0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_empty_folder_that_had_files_is_an_error() {
        let dir = temp_dir("unmounted");
        std::fs::create_dir_all(&dir).unwrap();
        let previous = ScanResult { files: vec![ScannedFile { rel_path: "a.mkv".into(), size: 1, mtime: 0 }], ..Default::default() };
        let err = rescan(&dir, &previous).unwrap_err().to_string();
        assert!(err.contains("is empty"), "{err}");
        assert!(rescan(&dir, &ScanResult::default()).unwrap().files.is_empty(), "a new library may be empty");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancel_stops_the_scan() {
        let dir = std::env::temp_dir();
        assert!(scan(&dir, LibraryKind::Tv, &Previous::default(), &AtomicBool::new(true), |_| {}).is_err());
    }

    #[test]
    fn missing_root_is_an_error() {
        let none = Previous::default();
        assert!(scan(Path::new("/definitely/not/here"), LibraryKind::Tv, &none, &AtomicBool::new(false), |_| {}).is_err());
    }
}
