//! Syncing what you've watched between devices through a shared folder.
//!
//! Every Flex stays standalone (its own database, library folders and scans). Each one
//! writes a snapshot of its watching state to `<sync folder>/<device id>.json` and reads
//! everyone else's. Only that device ever writes its file, and it's replaced in one go
//! (written aside, then renamed), so devices never fight over a file.
//!
//! A snapshot is the whole state, not a log of changes: every entry says when it last
//! changed, and for each entry the newest one wins. Applying writes those times as they
//! are, so applying never counts as a new change (no ping-pong), reading a snapshot twice
//! changes nothing, and a device that was off for a week just catches up.
//!
//! Files are named portably: by library name (each device has its own folder for it) and
//! path inside the library. Shows by their TMDB id, or title and year if not matched.
//!
//! Synced: watched/position/last played, history, the watchlist, removals from Continue
//! Watching, folders moved and shows matched by hand, and YouTube subscriptions and
//! progress. Not synced: libraries' folders, settings, caches and automatic matches.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Db;

/// Snapshot format version. Snapshots from newer formats are left alone.
pub const FORMAT: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub format: u32,
    /// This device's random id (its file name).
    pub device: String,
    /// What the device is called, e.g. "TV PC".
    pub name: String,
    pub written_at: i64,
    #[serde(default)]
    pub files: Vec<FileState>,
    #[serde(default)]
    pub history: Vec<Play>,
    #[serde(default)]
    pub watchlist: Vec<Watch>,
    #[serde(default)]
    pub hidden: Vec<Hidden>,
    #[serde(default)]
    pub groups: Vec<GroupFix>,
    #[serde(default)]
    pub shows: Vec<ShowFix>,
    #[serde(default)]
    pub yt_channels: Vec<Channel>,
    #[serde(default)]
    pub yt_videos: Vec<Video>,
}

/// A file's watched state and resume point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileState {
    pub library: String,
    pub path: String,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub watched: bool,
    pub last_played: Option<i64>,
    /// When this last changed.
    pub at: i64,
}

/// One viewing session (history only ever grows).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Play {
    pub library: String,
    pub path: String,
    pub played_at: i64,
    pub position: Option<f64>,
    pub completed: bool,
    pub source: String,
}

/// A show or film, portably: by TMDB id when matched, else title and year.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShowRef {
    pub provider_id: Option<String>,
    pub title: String,
    pub year: Option<i32>,
}

impl ShowRef {
    fn key(&self) -> String {
        match &self.provider_id {
            Some(p) => p.clone(),
            None => format!("{}|{}", crate::parse::normalize(&self.title), self.year.unwrap_or(0)),
        }
    }
}

/// On the watchlist, or taken off it (`removed`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Watch {
    pub library: String,
    pub show: ShowRef,
    pub removed: bool,
    pub at: i64,
}

/// Removed from Continue Watching: `file:<library>/<path>` or `yt:<video id>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hidden {
    pub key: String,
    pub at: i64,
}

/// A folder moved to a show by hand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupFix {
    pub library: String,
    /// The folder (group key).
    pub group: String,
    pub show: ShowRef,
    pub season: Option<i32>,
    pub at: i64,
}

/// A show matched online by hand: the show containing `group` is this TMDB entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShowFix {
    pub library: String,
    pub group: String,
    pub provider_id: String,
    pub title: String,
    pub year: Option<i32>,
    pub overview: String,
    pub poster: Option<String>,
    pub at: i64,
}

/// A YouTube subscription, or an unsubscription (`removed`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub title: String,
    pub removed: bool,
    pub at: i64,
    /// Marked as focused, and when that last changed (0 = never), which syncs on its own.
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub focused_at: i64,
}

/// A YouTube video's progress.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub watched: bool,
    pub at: i64,
}

/// When a file's state last changed. Watched (or part-watched) with no time at all, as
/// some imported history is, counts as 1: it reaches devices that know nothing about the
/// file, but never overrides a real change.
const FILE_AT: &str = "COALESCE(state_at, last_played, CASE WHEN watched = 1 OR position IS NOT NULL THEN 1 ELSE 0 END)";

/// What applying other devices' snapshots changed, so the app can reload it.
#[derive(Debug, Default, PartialEq)]
pub struct Applied {
    /// Libraries whose files' watch state changed (reload their files).
    pub files: HashSet<i64>,
    /// Libraries whose matches changed (organise them again).
    pub matches: HashSet<i64>,
    pub watchlist: bool,
    pub continue_hidden: bool,
    pub youtube: bool,
    pub history: usize,
}

impl Applied {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A new random device id (12 hex characters).
pub fn new_device_id() -> String {
    let mut hasher = Sha256::new();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(crate::data_dir().to_string_lossy().as_bytes());
    hasher.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect()
}

fn file_of(dir: &Path, device: &str) -> PathBuf {
    dir.join(format!("{device}.json"))
}

/// Writes this device's snapshot: aside first, then renamed into place, so others never
/// read half a file.
pub fn write(dir: &Path, snapshot: &Snapshot) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let dest = file_of(dir, &snapshot.device);
    let temp = dir.join(format!(".{}.json.tmp", snapshot.device));
    std::fs::write(&temp, serde_json::to_vec(snapshot)?).with_context(|| format!("writing {}", temp.display()))?;
    // Windows can't replace a file another device has open for a moment: try again.
    let mut last = None;
    for attempt in 0..5 {
        match std::fs::rename(&temp, &dest) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(200 * (attempt + 1)));
    }
    Err(last.expect("tried")).with_context(|| format!("replacing {}", dest.display()))
}

/// Everyone else's snapshots in `dir` (unreadable ones and newer formats are skipped,
/// and tried again next time).
pub fn read_others(dir: &Path, device: &str) -> Vec<Snapshot> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if !name.ends_with(".json") || name.starts_with('.') || name == format!("{device}.json") {
            continue;
        }
        match std::fs::read(&path).map_err(anyhow::Error::from).and_then(|b| Ok(serde_json::from_slice::<Snapshot>(&b)?)) {
            Ok(s) if s.format > FORMAT => log::warn!("sync: {name} is from a newer Flex; skipped"),
            Ok(s) => out.push(s),
            Err(e) => log::warn!("sync: reading {name}: {e:#}"),
        }
    }
    out
}

/// This device's watching state as a snapshot.
pub fn export(db: &Db, device: &str, name: &str, now: i64) -> Result<Snapshot> {
    let conn = &db.conn;
    let libraries: HashMap<i64, String> = db.libraries()?.into_iter().map(|l| (l.id, l.name)).collect();
    let lib = |id: i64| libraries.get(&id).cloned().unwrap_or_default();
    let mut snap = Snapshot { format: FORMAT, device: device.to_owned(), name: name.to_owned(), written_at: now, ..Snapshot::default() };

    let mut stmt = conn.prepare(&format!(
        "SELECT library_id, rel_path, position, duration, watched, last_played, {FILE_AT}
         FROM media_files WHERE state_at IS NOT NULL OR last_played IS NOT NULL OR watched = 1 OR position IS NOT NULL"
    ))?;
    snap.files = stmt
        .query_map([], |r| {
            Ok(FileState {
                library: lib(r.get(0)?),
                path: r.get(1)?,
                position: r.get(2)?,
                duration: r.get(3)?,
                watched: r.get(4)?,
                last_played: r.get(5)?,
                at: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut stmt = conn.prepare(
        "SELECT f.library_id, f.rel_path, h.played_at, h.position, h.completed, h.source
         FROM play_history h JOIN media_files f ON f.id = h.file_id",
    )?;
    snap.history = stmt
        .query_map([], |r| {
            Ok(Play { library: lib(r.get(0)?), path: r.get(1)?, played_at: r.get(2)?, position: r.get(3)?, completed: r.get(4)?, source: r.get(5)? })
        })?
        .collect::<rusqlite::Result<_>>()?;

    // Watchlist keys are provider ids, or "local:<show id>" for unmatched shows.
    let show_ref = |key: &str| -> Result<Option<ShowRef>> {
        let row = match key.strip_prefix("local:").and_then(|id| id.parse::<i64>().ok()) {
            Some(id) => conn.query_row("SELECT title, year FROM shows WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?,
            None => conn
                .query_row("SELECT title, year FROM shows WHERE provider_id = ?1 LIMIT 1", [key], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?,
        };
        let provider_id = (!key.starts_with("local:")).then(|| key.to_owned());
        Ok(match (row, provider_id) {
            (Some((title, year)), provider_id) => Some(ShowRef { provider_id, title, year }),
            (None, Some(p)) => Some(ShowRef { provider_id: Some(p), title: String::new(), year: None }),
            (None, None) => None,
        })
    };
    for (key, library_id, added_at) in db.watchlist()? {
        if let Some(show) = show_ref(&key)? {
            snap.watchlist.push(Watch { library: lib(library_id), show, removed: false, at: added_at });
        }
    }
    let mut stmt = conn.prepare("SELECT key, at FROM sync_removed WHERE kind = 'watchlist'")?;
    let removed: Vec<(String, i64)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    for (key, at) in removed {
        // The library comes from the show (unmatched ones), or any library with it.
        let library = match key.strip_prefix("local:").and_then(|id| id.parse::<i64>().ok()) {
            Some(id) => conn.query_row("SELECT library_id FROM shows WHERE id = ?1", [id], |r| r.get(0)).optional()?,
            None => conn.query_row("SELECT library_id FROM shows WHERE provider_id = ?1 LIMIT 1", [&key], |r| r.get(0)).optional()?,
        };
        if let (Some(show), Some(library)) = (show_ref(&key)?, library) {
            snap.watchlist.push(Watch { library: lib(library), show, removed: true, at });
        }
    }

    for (key, at) in db.continue_hidden()? {
        let portable = match key.strip_prefix("yt:") {
            Some(_) => Some(key.clone()),
            None => key.split_once(':').and_then(|(l, f)| Some((l.parse::<i64>().ok()?, f.parse::<i64>().ok()?))).and_then(|(l, f)| {
                let path: Option<String> = conn.query_row("SELECT rel_path FROM media_files WHERE id = ?1", [f], |r| r.get(0)).optional().ok()??;
                Some(format!("file:{}/{}", lib(l), path?))
            }),
        };
        if let Some(key) = portable {
            snap.hidden.push(Hidden { key, at });
        }
    }

    let mut stmt = conn.prepare(
        "SELECT g.library_id, g.group_key, s.provider_id, s.title, s.year, g.season, COALESCE(g.changed_at, 0)
         FROM group_matches g JOIN shows s ON s.id = g.show_id WHERE g.manual = 1",
    )?;
    snap.groups = stmt
        .query_map([], |r| {
            Ok(GroupFix {
                library: lib(r.get(0)?),
                group: r.get(1)?,
                show: ShowRef { provider_id: r.get(2)?, title: r.get(3)?, year: r.get(4)? },
                season: r.get(5)?,
                at: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut stmt = conn.prepare(
        "SELECT s.library_id, (SELECT group_key FROM group_matches WHERE show_id = s.id ORDER BY group_key LIMIT 1),
                s.provider_id, s.title, s.year, COALESCE(s.overview, ''), s.poster, COALESCE(s.manual_at, 0)
         FROM shows s WHERE s.state = 'manual' AND s.provider_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))
    })?;
    for row in rows {
        let (library, group, provider_id, title, year, overview, poster, at) = row?;
        if let Some(group) = group {
            snap.shows.push(ShowFix { library: lib(library), group, provider_id, title, year, overview, poster, at });
        }
    }

    for c in db.yt_channels()? {
        snap.yt_channels.push(Channel { id: c.id, title: c.title, removed: false, at: c.added_at, focused: c.focused, focused_at: c.focused_at });
    }
    let mut stmt = conn.prepare("SELECT key, at FROM sync_removed WHERE kind = 'yt_channel'")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (id, at) = row?;
        snap.yt_channels.push(Channel { id, title: String::new(), removed: true, at, focused: false, focused_at: 0 });
    }
    for (id, p) in db.yt_progress_all()? {
        snap.yt_videos.push(Video { id, title: p.title, channel: p.channel, position: p.position, duration: p.duration, watched: p.watched, at: p.updated_at });
    }
    Ok(snap)
}

/// The newest of each entry across snapshots, by `key`.
fn newest<'a, T, K: std::hash::Hash + Eq>(items: impl Iterator<Item = &'a T>, key: impl Fn(&T) -> K, at: impl Fn(&T) -> i64) -> Vec<&'a T>
where
    T: 'a,
{
    let mut best: HashMap<K, &T> = HashMap::new();
    for item in items {
        let k = key(item);
        if best.get(&k).is_none_or(|b| at(item) > at(b)) {
            best.insert(k, item);
        }
    }
    best.into_values().collect()
}

/// Merges other devices' snapshots into this database: for each entry, the newest wins.
pub fn apply(db: &mut Db, snapshots: &[Snapshot]) -> Result<Applied> {
    let mut applied = Applied::default();
    // Libraries are paired by name (each device has its own folder for them).
    let libraries: HashMap<String, i64> = db.libraries()?.into_iter().map(|l| (l.name, l.id)).collect();
    // Take the write lock up front: a transaction that reads and then writes can't wait for
    // another connection's write in between (SQLite fails it at once, as "locked"), and the
    // app and this thread both write.
    let tx = db.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let file_id = |library: &str, path: &str| -> Result<Option<(i64, i64)>> {
        let Some(&lib) = libraries.get(library) else { return Ok(None) };
        Ok(tx
            .query_row("SELECT id FROM media_files WHERE library_id = ?1 AND rel_path = ?2", params![lib, path], |r| r.get(0))
            .optional()?
            .map(|id| (lib, id)))
    };

    for f in newest(snapshots.iter().flat_map(|s| &s.files), |f| (f.library.clone(), f.path.clone()), |f| f.at) {
        let Some((lib, id)) = file_id(&f.library, &f.path)? else { continue };
        let local: i64 = tx.query_row(&format!("SELECT {FILE_AT} FROM media_files WHERE id = ?1"), [id], |r| r.get(0))?;
        if f.at > local {
            tx.execute(
                "UPDATE media_files SET position = ?2, duration = ?3, watched = ?4, last_played = ?5, state_at = ?6 WHERE id = ?1",
                params![id, f.position, f.duration, f.watched, f.last_played, f.at],
            )?;
            applied.files.insert(lib);
        }
    }

    for p in snapshots.iter().flat_map(|s| &s.history) {
        let Some((_, id)) = file_id(&p.library, &p.path)? else { continue };
        applied.history += tx.execute(
            "INSERT OR IGNORE INTO play_history (file_id, played_at, position, completed, source) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, p.played_at, p.position, p.completed, p.source],
        )?;
    }

    // A show here for a portable reference (made if needed, for folders moved by hand).
    let find_show = |lib: i64, show: &ShowRef, create: bool| -> Result<Option<i64>> {
        if let Some(p) = &show.provider_id
            && let Some(id) = tx.query_row("SELECT id FROM shows WHERE library_id = ?1 AND provider_id = ?2", params![lib, p], |r| r.get(0)).optional()?
        {
            return Ok(Some(id));
        }
        let norm = crate::parse::normalize(&show.title);
        let by_title = tx
            .query_row(
                "SELECT id FROM shows WHERE library_id = ?1 AND norm = ?2 AND (year IS ?3 OR ?3 IS NULL) ORDER BY id LIMIT 1",
                params![lib, norm, show.year],
                |r| r.get(0),
            )
            .optional()?;
        if by_title.is_some() || !create || show.title.is_empty() {
            return Ok(by_title);
        }
        tx.execute("INSERT INTO shows (library_id, title, norm, year) VALUES (?1, ?2, ?3, ?4)", params![lib, show.title, norm, show.year])?;
        Ok(Some(tx.last_insert_rowid()))
    };

    for w in newest(snapshots.iter().flat_map(|s| &s.watchlist), |w| (w.library.clone(), w.show.key()), |w| w.at) {
        let Some(&lib) = libraries.get(&w.library) else { continue };
        let Some(show) = find_show(lib, &w.show, false)? else { continue };
        let key = match &w.show.provider_id {
            Some(p) => p.clone(),
            None => format!("local:{show}"),
        };
        let added: Option<i64> = tx.query_row("SELECT added_at FROM watchlist WHERE key = ?1", [&key], |r| r.get(0)).optional()?;
        let gone: Option<i64> = tx.query_row("SELECT at FROM sync_removed WHERE kind = 'watchlist' AND key = ?1", [&key], |r| r.get(0)).optional()?;
        if w.at <= added.unwrap_or(0).max(gone.unwrap_or(0)) {
            continue;
        }
        if w.removed {
            if added.is_some() {
                tx.execute("DELETE FROM watchlist WHERE key = ?1", [&key])?;
                applied.watchlist = true;
            }
            tx.execute("INSERT OR REPLACE INTO sync_removed (kind, key, at) VALUES ('watchlist', ?1, ?2)", params![key, w.at])?;
        } else {
            tx.execute("INSERT OR REPLACE INTO watchlist (key, library_id, added_at) VALUES (?1, ?2, ?3)", params![key, lib, w.at])?;
            tx.execute("DELETE FROM sync_removed WHERE kind = 'watchlist' AND key = ?1", [&key])?;
            applied.watchlist = true;
        }
    }

    for h in newest(snapshots.iter().flat_map(|s| &s.hidden), |h| h.key.clone(), |h| h.at) {
        let key = match h.key.strip_prefix("file:") {
            None => h.key.clone(),
            Some(rest) => {
                let Some((library, path)) = rest.split_once('/') else { continue };
                let Some((lib, id)) = file_id(library, path)? else { continue };
                format!("{lib}:{id}")
            }
        };
        let local: Option<i64> = tx.query_row("SELECT hidden_at FROM continue_hidden WHERE key = ?1", [&key], |r| r.get(0)).optional()?;
        if h.at > local.unwrap_or(0) {
            tx.execute("INSERT OR REPLACE INTO continue_hidden (key, hidden_at) VALUES (?1, ?2)", params![key, h.at])?;
            applied.continue_hidden = true;
        }
    }

    for g in newest(snapshots.iter().flat_map(|s| &s.groups), |g| (g.library.clone(), g.group.clone()), |g| g.at) {
        let Some(&lib) = libraries.get(&g.library) else { continue };
        let local: Option<(bool, i64)> = tx
            .query_row(
                "SELECT manual, COALESCE(changed_at, 0) FROM group_matches WHERE library_id = ?1 AND group_key = ?2",
                params![lib, g.group],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if local.is_some_and(|(manual, at)| manual && at >= g.at) {
            continue;
        }
        let Some(show) = find_show(lib, &g.show, true)? else { continue };
        tx.execute(
            "INSERT INTO group_matches (library_id, group_key, show_id, season, manual, changed_at) VALUES (?1, ?2, ?3, ?4, 1, ?5)
             ON CONFLICT(library_id, group_key) DO UPDATE SET show_id = ?3, season = ?4, manual = 1, changed_at = ?5",
            params![lib, g.group, show, g.season, g.at],
        )?;
        applied.matches.insert(lib);
    }

    for s in newest(snapshots.iter().flat_map(|s| &s.shows), |s| (s.library.clone(), s.group.clone()), |s| s.at) {
        let Some(&lib) = libraries.get(&s.library) else { continue };
        let show: Option<(i64, String, i64)> = tx
            .query_row(
                "SELECT sh.id, sh.state, COALESCE(sh.manual_at, 0) FROM group_matches g JOIN shows sh ON sh.id = g.show_id
                 WHERE g.library_id = ?1 AND g.group_key = ?2",
                params![lib, s.group],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((id, state, at)) = show else { continue };
        if state == "manual" && at >= s.at {
            continue;
        }
        tx.execute(
            "UPDATE shows SET provider = 'tmdb', provider_id = ?2, title = ?3, norm = ?4, year = ?5, overview = ?6, poster = ?7,
             state = 'manual', details_at = NULL, manual_at = ?8 WHERE id = ?1",
            params![id, s.provider_id, s.title, crate::parse::normalize(&s.title), s.year, s.overview, s.poster, s.at],
        )?;
        applied.matches.insert(lib);
    }

    for c in newest(snapshots.iter().flat_map(|s| &s.yt_channels), |c| c.id.clone(), |c| c.at) {
        let added: Option<i64> = tx.query_row("SELECT added_at FROM yt_channels WHERE id = ?1", [&c.id], |r| r.get(0)).optional()?;
        let gone: Option<i64> = tx.query_row("SELECT at FROM sync_removed WHERE kind = 'yt_channel' AND key = ?1", [&c.id], |r| r.get(0)).optional()?;
        if c.at <= added.unwrap_or(0).max(gone.unwrap_or(0)) {
            continue;
        }
        if c.removed {
            tx.execute("DELETE FROM yt_videos WHERE channel_id = ?1", [&c.id])?;
            tx.execute("DELETE FROM yt_channels WHERE id = ?1", [&c.id])?;
            tx.execute("INSERT OR REPLACE INTO sync_removed (kind, key, at) VALUES ('yt_channel', ?1, ?2)", params![c.id, c.at])?;
        } else {
            tx.execute(
                "INSERT INTO yt_channels (id, title, added_at) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET added_at = ?3",
                params![c.id, c.title, c.at],
            )?;
            tx.execute("DELETE FROM sync_removed WHERE kind = 'yt_channel' AND key = ?1", [&c.id])?;
        }
        applied.youtube = true;
    }
    // Focus changes on their own: the newest wins, for channels subscribed here.
    let focus = snapshots.iter().flat_map(|s| &s.yt_channels).filter(|c| !c.removed && c.focused_at > 0);
    for c in newest(focus, |c| c.id.clone(), |c| c.focused_at) {
        let changed = tx.execute(
            "UPDATE yt_channels SET focused = ?2, focused_at = ?3 WHERE id = ?1 AND focused_at < ?3",
            params![c.id, c.focused, c.focused_at],
        )?;
        applied.youtube |= changed > 0;
    }

    for v in newest(snapshots.iter().flat_map(|s| &s.yt_videos), |v| v.id.clone(), |v| v.at) {
        let local: Option<i64> = tx.query_row("SELECT updated_at FROM yt_progress WHERE id = ?1", [&v.id], |r| r.get(0)).optional()?;
        if v.at > local.unwrap_or(0) {
            tx.execute(
                "INSERT OR REPLACE INTO yt_progress (id, title, channel, position, duration, watched, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![v.id, v.title, v.channel, v.position, v.duration, v.watched, v.at],
            )?;
            applied.youtube = true;
        }
    }
    tx.commit()?;
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LibraryKind;
    use crate::scan::{ScanResult, ScannedFile};

    /// A device with an "Anime" library at its own path, files added in `order`.
    fn device(root: &str, order: &[&str]) -> (Db, i64) {
        let mut db = Db::open_in_memory().unwrap();
        let lib = db.add_library("Anime", Path::new(root), LibraryKind::Anime).unwrap();
        let files = order.iter().map(|p| ScannedFile { rel_path: (*p).into(), size: 1, mtime: 1 }).collect();
        db.apply_scan(lib, &ScanResult { files, ..ScanResult::default() }).unwrap();
        (db, lib)
    }

    fn id(db: &Db, lib: i64, path: &str) -> i64 {
        db.files(lib).unwrap().into_iter().find(|f| f.rel_path == path).unwrap().id
    }

    fn progress(db: &Db, lib: i64, path: &str) -> crate::progress::Progress {
        db.files(lib).unwrap().into_iter().find(|f| f.rel_path == path).unwrap().progress
    }

    /// Sets a file's state as if changed at `at`.
    fn set(db: &Db, lib: i64, path: &str, position: Option<f64>, watched: bool, at: i64) {
        db.conn
            .execute(
                "UPDATE media_files SET position = ?2, duration = 1400, watched = ?3, last_played = ?4, state_at = ?4 WHERE id = ?1",
                params![id(db, lib, path), position, watched, at],
            )
            .unwrap();
    }

    const PATHS: [&str; 3] = ["Show/01.mkv", "Show/02.mkv", "Show/03.mkv"];

    #[test]
    fn watch_state_crosses_devices_newest_wins() {
        // Different folders and file ids on each device; same library name and paths.
        let (mut tv, tv_lib) = device("D:/media/anime", &PATHS);
        let (mut mac, mac_lib) = device("/Users/cat/media/anime", &["Show/03.mkv", "Show/02.mkv", "Show/01.mkv"]);
        set(&tv, tv_lib, "Show/01.mkv", None, true, 100);
        set(&tv, tv_lib, "Show/02.mkv", Some(300.0), false, 110);

        let from_tv = export(&tv, "tv", "TV PC", 120).unwrap();
        let applied = apply(&mut mac, std::slice::from_ref(&from_tv)).unwrap();
        assert!(applied.files.contains(&mac_lib));
        assert!(progress(&mac, mac_lib, "Show/01.mkv").watched);
        assert_eq!(progress(&mac, mac_lib, "Show/02.mkv").position, Some(300.0));
        // Applying again changes nothing.
        assert!(apply(&mut mac, &[from_tv]).unwrap().is_empty());

        // The Mac watches further later; the TV takes that, but not an older change back.
        set(&mac, mac_lib, "Show/02.mkv", None, true, 200);
        set(&tv, tv_lib, "Show/03.mkv", Some(50.0), false, 150);
        let from_mac = export(&mac, "mac", "Mac", 210).unwrap();
        apply(&mut tv, &[from_mac]).unwrap();
        assert!(progress(&tv, tv_lib, "Show/02.mkv").watched);
        assert_eq!(progress(&tv, tv_lib, "Show/03.mkv").position, Some(50.0), "the TV's own newer change stays");
        // Marking unwatched travels too.
        tv.set_watched(&[id(&tv, tv_lib, "Show/01.mkv")], false).unwrap();
        tv.conn.execute("UPDATE media_files SET state_at = 300 WHERE id = ?1", [id(&tv, tv_lib, "Show/01.mkv")]).unwrap();
        apply(&mut mac, &[export(&tv, "tv", "TV PC", 310).unwrap()]).unwrap();
        assert!(!progress(&mac, mac_lib, "Show/01.mkv").watched);
    }

    #[test]
    fn untimed_watched_state_spreads_but_never_overrides() {
        let (tv, tv_lib) = device("/tv", &PATHS);
        let (mut mac, mac_lib) = device("/mac", &PATHS);
        // Imported: watched, no time.
        tv.conn.execute("UPDATE media_files SET watched = 1 WHERE id = ?1", [id(&tv, tv_lib, "Show/01.mkv")]).unwrap();
        tv.conn.execute("UPDATE media_files SET watched = 1 WHERE id = ?1", [id(&tv, tv_lib, "Show/02.mkv")]).unwrap();
        // The Mac unwatched 02 on purpose.
        set(&mac, mac_lib, "Show/02.mkv", None, false, 500);
        apply(&mut mac, &[export(&tv, "tv", "TV", 600).unwrap()]).unwrap();
        assert!(progress(&mac, mac_lib, "Show/01.mkv").watched, "reaches a device with nothing");
        assert!(!progress(&mac, mac_lib, "Show/02.mkv").watched, "doesn't override a real change");
    }

    #[test]
    fn history_merges_and_unknown_files_wait() {
        let (tv, tv_lib) = device("/tv", &PATHS);
        let (mut mac, mac_lib) = device("/mac", &PATHS[..2]);
        tv.add_history(id(&tv, tv_lib, "Show/01.mkv"), 1000, Some(1400.0), true).unwrap();
        tv.add_history(id(&tv, tv_lib, "Show/03.mkv"), 2000, None, false).unwrap();
        set(&tv, tv_lib, "Show/03.mkv", Some(10.0), false, 2000);
        let snap = export(&tv, "tv", "TV", 3000).unwrap();
        assert_eq!(apply(&mut mac, std::slice::from_ref(&snap)).unwrap().history, 1, "03 isn't on the Mac yet");
        assert_eq!(apply(&mut mac, std::slice::from_ref(&snap)).unwrap().history, 0, "no duplicates");
        // Once the Mac has the file (a later scan), the snapshot catches it up.
        let files = PATHS.iter().map(|p| ScannedFile { rel_path: (*p).into(), size: 1, mtime: 1 }).collect();
        mac.apply_scan(mac_lib, &ScanResult { files, ..ScanResult::default() }).unwrap();
        let applied = apply(&mut mac, &[snap]).unwrap();
        assert_eq!(applied.history, 1);
        assert_eq!(progress(&mac, mac_lib, "Show/03.mkv").position, Some(10.0));
    }

    #[test]
    fn watchlist_and_removals_travel() {
        let (tv, tv_lib) = device("/tv", &PATHS);
        let (mut mac, mac_lib) = device("/mac", &PATHS);
        for (db, lib) in [(&tv, tv_lib), (&mac, mac_lib)] {
            let show = db.create_show(lib, "Frieren", Some(2023)).unwrap();
            db.conn.execute("UPDATE shows SET provider_id = 'tv:209867' WHERE id = ?1", [show]).unwrap();
            db.create_show(lib, "Home Movies", None).unwrap();
        }
        tv.set_watchlist("tv:209867", tv_lib, true).unwrap();
        let local = tv.shows(tv_lib).unwrap().into_iter().find(|s| s.title == "Home Movies").unwrap().id;
        tv.set_watchlist(&format!("local:{local}"), tv_lib, true).unwrap();
        apply(&mut mac, &[export(&tv, "tv", "TV", 10).unwrap()]).unwrap();
        let keys: HashSet<String> = mac.watchlist().unwrap().into_iter().map(|w| w.0).collect();
        let mac_local = mac.shows(mac_lib).unwrap().into_iter().find(|s| s.title == "Home Movies").unwrap().id;
        assert_eq!(keys, HashSet::from(["tv:209867".to_owned(), format!("local:{mac_local}")]), "unmatched shows map by title");

        // Taken off on the Mac (later): gone from the TV too.
        mac.set_watchlist("tv:209867", mac_lib, false).unwrap();
        mac.conn.execute("UPDATE sync_removed SET at = strftime('%s','now') + 100", []).unwrap();
        let mut tv = tv;
        let applied = apply(&mut tv, &[export(&mac, "mac", "Mac", 20).unwrap()]).unwrap();
        assert!(applied.watchlist);
        assert!(!tv.watchlist().unwrap().iter().any(|w| w.0 == "tv:209867"));
    }

    #[test]
    fn folder_moves_and_manual_matches_travel() {
        let (tv, tv_lib) = device("/tv", &PATHS);
        let (mut mac, mac_lib) = device("/mac", &PATHS);
        // On the TV, "Show S2" was moved by hand to Frieren season 2, and Frieren matched by hand.
        let frieren = tv.create_show(tv_lib, "Frieren", Some(2023)).unwrap();
        tv.set_group_match(tv_lib, "Show S2", frieren, Some(2)).unwrap();
        tv.conn.execute("UPDATE group_matches SET changed_at = 50", []).unwrap();
        tv.conn
            .execute("UPDATE shows SET provider_id = 'tv:209867', state = 'manual', manual_at = 60, overview = 'An elf.' WHERE id = ?1", [frieren])
            .unwrap();
        let snap = export(&tv, "tv", "TV", 70).unwrap();
        assert_eq!(snap.groups.len(), 1);
        assert_eq!(snap.shows.len(), 1);

        let applied = apply(&mut mac, &[snap]).unwrap();
        assert!(applied.matches.contains(&mac_lib));
        let m = &mac.group_matches(mac_lib).unwrap()[0];
        assert_eq!((m.group_key.as_str(), m.season, m.manual), ("Show S2", Some(2), true));
        let show = mac.shows(mac_lib).unwrap().into_iter().find(|s| s.id == m.show_id).unwrap();
        assert_eq!((show.title.as_str(), show.provider_id.as_deref(), show.state.as_str()), ("Frieren", Some("tv:209867"), "manual"));
    }

    #[test]
    fn youtube_and_continue_watching_travel() {
        let (mut tv, tv_lib) = device("/tv", &PATHS);
        let (mut mac, mac_lib) = device("/mac", &PATHS);
        tv.add_yt_channel("UCa", "Chan A", 10).unwrap();
        tv.add_yt_channel("UCb", "Chan B", 11).unwrap();
        tv.record_yt_progress("vid12345678", "A video", "Chan A", 100.0, 600.0, 0.9, 20).unwrap();
        tv.hide_continue(&format!("{tv_lib}:{}", id(&tv, tv_lib, "Show/02.mkv")), 30).unwrap();
        tv.hide_continue("yt:vid12345678", 31).unwrap();
        apply(&mut mac, &[export(&tv, "tv", "TV", 40).unwrap()]).unwrap();
        assert_eq!(mac.yt_channels().unwrap().len(), 2);
        assert_eq!(mac.yt_progress("vid12345678").unwrap().unwrap().position, Some(100.0));
        let hidden = mac.continue_hidden().unwrap();
        assert_eq!(hidden.get(&format!("{mac_lib}:{}", id(&mac, mac_lib, "Show/02.mkv"))), Some(&30));
        assert_eq!(hidden.get("yt:vid12345678"), Some(&31));

        // Unsubscribed on the Mac later: unsubscribed on the TV.
        mac.remove_yt_channel("UCb").unwrap();
        mac.conn.execute("UPDATE sync_removed SET at = 100 WHERE key = 'UCb'", []).unwrap();
        apply(&mut tv, &[export(&mac, "mac", "Mac", 110).unwrap()]).unwrap();
        let ids: Vec<String> = tv.yt_channels().unwrap().into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["UCa"]);
        // Focused on the TV, then unfocused on the Mac later: each change travels.
        let focused = |db: &Db, id: &str| db.yt_channels().unwrap().into_iter().find(|c| c.id == id).unwrap().focused;
        tv.set_yt_channel_focused("UCa", true, 120).unwrap();
        let focused_on_tv = export(&tv, "tv", "TV", 130).unwrap();
        apply(&mut mac, std::slice::from_ref(&focused_on_tv)).unwrap();
        assert!(focused(&mac, "UCa"));
        mac.set_yt_channel_focused("UCa", false, 140).unwrap();
        apply(&mut tv, &[export(&mac, "mac", "Mac", 150).unwrap()]).unwrap();
        assert!(!focused(&tv, "UCa"));
        // The older change, read again, doesn't undo it.
        apply(&mut tv, &[focused_on_tv]).unwrap();
        assert!(!focused(&tv, "UCa"));
    }

    #[test]
    fn files_in_the_shared_folder() {
        let dir = std::env::temp_dir().join(format!("flex-sync-{}", std::process::id()));
        let snap = |device: &str| Snapshot { format: FORMAT, device: device.into(), name: device.into(), written_at: 1, ..Snapshot::default() };
        write(&dir, &snap("aaa")).unwrap();
        write(&dir, &snap("bbb")).unwrap();
        write(&dir, &snap("bbb")).unwrap(); // replaced in place
        std::fs::write(dir.join("broken.json"), "{ half a file").unwrap();
        std::fs::write(dir.join("ccc.json"), serde_json::to_vec(&Snapshot { format: FORMAT + 1, ..snap("ccc") }).unwrap()).unwrap();
        let others = read_others(&dir, "aaa");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(others.iter().map(|s| s.device.as_str()).collect::<Vec<_>>(), ["bbb"], "not our own, broken or newer files");
        assert_eq!(new_device_id().len(), 12);
    }
}
