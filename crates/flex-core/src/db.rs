//! SQLite storage for libraries and their files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

use crate::library::{Library, LibraryKind, MediaFile};
use crate::progress::Progress;
use crate::scan::ScannedFile;

const SCHEMA: &str = "
CREATE TABLE libraries (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL,
    root  TEXT NOT NULL UNIQUE,
    kind  TEXT NOT NULL
);
CREATE TABLE media_files (
    id           INTEGER PRIMARY KEY,
    library_id   INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
    rel_path     TEXT NOT NULL,
    size         INTEGER NOT NULL,
    mtime        INTEGER NOT NULL,
    -- Kept (not deleted) when a file disappears, so watch history survives a share being offline.
    missing      INTEGER NOT NULL DEFAULT 0,
    position     REAL,
    duration     REAL,
    watched      INTEGER NOT NULL DEFAULT 0,
    last_played  INTEGER,
    UNIQUE (library_id, rel_path)
);
";

/// Schema changes after version 1, applied in order.
const MIGRATIONS: &[&str] = &[
    // 2: app settings
    "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    // 3: shows/movies and which folders (groups) belong to them
    "CREATE TABLE shows (
        id           INTEGER PRIMARY KEY,
        library_id   INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
        title        TEXT NOT NULL,
        norm         TEXT NOT NULL,
        year         INTEGER,
        provider     TEXT,
        provider_id  TEXT,
        overview     TEXT,
        poster       TEXT,
        -- local: from file names only; matched: found online; unmatched: lookup failed
        state        TEXT NOT NULL DEFAULT 'local'
    );
    CREATE INDEX shows_by_library ON shows(library_id, norm);
    CREATE TABLE group_matches (
        library_id  INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
        group_key   TEXT NOT NULL,
        show_id     INTEGER NOT NULL REFERENCES shows(id) ON DELETE CASCADE,
        -- set only when chosen by hand; otherwise derived from names each time
        season      INTEGER,
        manual      INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (library_id, group_key)
    );",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Show {
    pub id: i64,
    pub library_id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub provider: Option<String>,
    pub provider_id: Option<String>,
    pub overview: Option<String>,
    pub poster: Option<String>,
    pub state: String,
}

/// Which show a group (folder) belongs to.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupMatch {
    pub group_key: String,
    pub show_id: i64,
    /// Season chosen by hand, if any.
    pub season: Option<i32>,
    /// Set by hand: never changed automatically.
    pub manual: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ScanSummary {
    pub added: usize,
    pub removed: usize,
    pub total: usize,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        let mut version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let latest = 1 + MIGRATIONS.len() as i64;
        if version > latest {
            bail!("database is from a newer version of Flex (schema {version})");
        }
        if version == 0 {
            conn.execute_batch(SCHEMA)?;
            version = 1;
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize - 1) {
            conn.execute_batch(sql)?;
            version = i as i64 + 2;
        }
        conn.pragma_update(None, "user_version", version)?;
        Ok(Self { conn })
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn shows(&self, library_id: i64) -> Result<Vec<Show>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, library_id, title, year, provider, provider_id, overview, poster, state
             FROM shows WHERE library_id = ?1",
        )?;
        let rows = stmt.query_map([library_id], |r| {
            Ok(Show {
                id: r.get(0)?,
                library_id: r.get(1)?,
                title: r.get(2)?,
                year: r.get(3)?,
                provider: r.get(4)?,
                provider_id: r.get(5)?,
                overview: r.get(6)?,
                poster: r.get(7)?,
                state: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn group_matches(&self, library_id: i64) -> Result<Vec<GroupMatch>> {
        let mut stmt =
            self.conn.prepare("SELECT group_key, show_id, season, manual FROM group_matches WHERE library_id = ?1")?;
        let rows = stmt.query_map([library_id], |r| {
            Ok(GroupMatch { group_key: r.get(0)?, show_id: r.get(1)?, season: r.get(2)?, manual: r.get(3)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Brings group assignments up to date with the library's current groups: new groups
    /// join the show with the same normalised title (creating it if needed), assignments
    /// for vanished groups are dropped unless set by hand, and shows left with no groups
    /// are removed.
    pub fn sync_groups(&mut self, library_id: i64, groups: &[crate::catalog::Group]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            // group key -> (show id, manual)
            let existing: HashMap<String, (i64, bool)> = {
                let mut stmt =
                    tx.prepare("SELECT group_key, show_id, manual FROM group_matches WHERE library_id = ?1")?;
                let rows = stmt.query_map([library_id], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let show_norm: HashMap<i64, String> = {
                let mut stmt = tx.prepare("SELECT id, norm FROM shows WHERE library_id = ?1")?;
                let rows = stmt.query_map([library_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let mut by_norm: HashMap<String, i64> = {
                let mut stmt = tx.prepare("SELECT norm, id FROM shows WHERE library_id = ?1 ORDER BY id")?;
                let rows = stmt.query_map([library_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let mut insert_show =
                tx.prepare("INSERT INTO shows (library_id, title, norm, year) VALUES (?1, ?2, ?3, ?4)")?;
            let mut insert_match = tx.prepare(
                "INSERT INTO group_matches (library_id, group_key, show_id) VALUES (?1, ?2, ?3)
                 ON CONFLICT(library_id, group_key) DO UPDATE SET show_id = ?3",
            )?;
            let current: std::collections::HashSet<&str> = groups.iter().map(|g| g.key.as_str()).collect();
            for g in groups {
                // Keep manual choices, and automatic ones whose show still has this title;
                // re-place automatic ones whose parsed title has changed.
                match existing.get(&g.key) {
                    Some((_, true)) => continue,
                    Some((show, false)) if show_norm.get(show) == Some(&g.norm) => continue,
                    _ => {}
                }
                let show_id = match by_norm.get(&g.norm) {
                    Some(id) => *id,
                    None => {
                        insert_show.execute(params![library_id, g.title, g.norm, g.year])?;
                        let id = tx.last_insert_rowid();
                        by_norm.insert(g.norm.clone(), id);
                        id
                    }
                };
                insert_match.execute(params![library_id, g.key, show_id])?;
            }
            let mut delete_match = tx.prepare("DELETE FROM group_matches WHERE library_id = ?1 AND group_key = ?2")?;
            for (key, (_, manual)) in &existing {
                if !manual && !current.contains(key.as_str()) {
                    delete_match.execute(params![library_id, key])?;
                }
            }
        }
        Self::delete_empty_shows(&tx, library_id)?;
        tx.commit()?;
        Ok(())
    }

    fn delete_empty_shows(conn: &Connection, library_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM shows WHERE library_id = ?1
             AND id NOT IN (SELECT show_id FROM group_matches WHERE library_id = ?1)",
            [library_id],
        )?;
        Ok(())
    }

    /// Assigns a group by hand (to `show_id`, optionally with a season). Never undone by scans.
    pub fn set_group_match(&self, library_id: i64, group_key: &str, show_id: i64, season: Option<i32>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO group_matches (library_id, group_key, show_id, season, manual) VALUES (?1, ?2, ?3, ?4, 1)
             ON CONFLICT(library_id, group_key) DO UPDATE SET show_id = ?3, season = ?4, manual = 1",
            params![library_id, group_key, show_id, season],
        )?;
        Self::delete_empty_shows(&self.conn, library_id)
    }

    /// A new show for a group split off by hand.
    pub fn create_show(&self, library_id: i64, title: &str, year: Option<i32>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO shows (library_id, title, norm, year) VALUES (?1, ?2, ?3, ?4)",
            params![library_id, title, crate::parse::normalize(title), year],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn libraries(&self) -> Result<Vec<Library>> {
        let mut stmt = self.conn.prepare("SELECT id, name, root, kind FROM libraries ORDER BY name COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?;
        let mut libs = Vec::new();
        for row in rows {
            let (id, name, root, kind) = row?;
            let kind = LibraryKind::parse(&kind).unwrap_or(LibraryKind::Tv);
            libs.push(Library { id, name, root: PathBuf::from(root), kind });
        }
        Ok(libs)
    }

    pub fn add_library(&self, name: &str, root: &Path, kind: LibraryKind) -> Result<i64> {
        let root = root.to_string_lossy();
        let exists: Option<i64> =
            self.conn.query_row("SELECT id FROM libraries WHERE root = ?1", [&root], |r| r.get(0)).optional()?;
        if exists.is_some() {
            bail!("{root} is already a library");
        }
        self.conn.execute(
            "INSERT INTO libraries (name, root, kind) VALUES (?1, ?2, ?3)",
            params![name, root, kind.as_str()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_library(&self, id: i64, name: &str) -> Result<()> {
        self.conn.execute("UPDATE libraries SET name = ?1 WHERE id = ?2", params![name, id])?;
        Ok(())
    }

    pub fn set_library_kind(&self, id: i64, kind: LibraryKind) -> Result<()> {
        self.conn.execute("UPDATE libraries SET kind = ?1 WHERE id = ?2", params![kind.as_str(), id])?;
        Ok(())
    }

    pub fn remove_library(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM libraries WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Files currently present in a library (not marked missing).
    pub fn files(&self, library_id: i64) -> Result<Vec<MediaFile>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, library_id, rel_path, size, mtime, position, duration, watched, last_played
             FROM media_files WHERE library_id = ?1 AND missing = 0",
        )?;
        let rows = stmt.query_map([library_id], |r| {
            Ok(MediaFile {
                id: r.get(0)?,
                library_id: r.get(1)?,
                rel_path: r.get(2)?,
                size: r.get::<_, i64>(3)? as u64,
                mtime: r.get(4)?,
                progress: Progress {
                    position: r.get(5)?,
                    duration: r.get(6)?,
                    watched: r.get(7)?,
                    last_played: r.get(8)?,
                },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_progress(&self, file_id: i64, p: &Progress) -> Result<()> {
        self.conn.execute(
            "UPDATE media_files SET position = ?2, duration = ?3, watched = ?4, last_played = ?5 WHERE id = ?1",
            params![file_id, p.position, p.duration, p.watched, p.last_played],
        )?;
        Ok(())
    }

    /// Marks files watched or unwatched, clearing any resume position.
    pub fn set_watched(&mut self, file_ids: &[i64], watched: bool) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare("UPDATE media_files SET watched = ?2, position = NULL WHERE id = ?1")?;
            for id in file_ids {
                stmt.execute(params![id, watched])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Records a scan: inserts new files, updates changed ones and marks vanished ones missing.
    pub fn apply_scan(&mut self, library_id: i64, files: &[ScannedFile]) -> Result<ScanSummary> {
        let tx = self.conn.transaction()?;
        let mut existing: HashMap<String, (i64, bool)> = {
            let mut stmt = tx.prepare("SELECT id, rel_path, missing FROM media_files WHERE library_id = ?1")?;
            let rows = stmt.query_map([library_id], |r| Ok((r.get::<_, String>(1)?, (r.get(0)?, r.get(2)?))))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut summary = ScanSummary { total: files.len(), ..Default::default() };
        {
            let mut insert = tx.prepare(
                "INSERT INTO media_files (library_id, rel_path, size, mtime) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut update =
                tx.prepare("UPDATE media_files SET size = ?2, mtime = ?3, missing = 0 WHERE id = ?1")?;
            for f in files {
                match existing.remove(&f.rel_path) {
                    Some((id, was_missing)) => {
                        update.execute(params![id, f.size as i64, f.mtime])?;
                        summary.added += usize::from(was_missing);
                    }
                    None => {
                        insert.execute(params![library_id, f.rel_path, f.size as i64, f.mtime])?;
                        summary.added += 1;
                    }
                }
            }
            let mut mark_missing = tx.prepare("UPDATE media_files SET missing = 1 WHERE id = ?1")?;
            for (id, was_missing) in existing.into_values() {
                if !was_missing {
                    mark_missing.execute([id])?;
                    summary.removed += 1;
                }
            }
        }
        tx.commit()?;
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(p: &str) -> ScannedFile {
        ScannedFile { rel_path: p.into(), size: 1, mtime: 0 }
    }

    #[test]
    fn group_sync_keeps_manual_choices() {
        use crate::catalog::Group;
        let group = |key: &str, title: &str| Group {
            key: key.into(),
            title: title.into(),
            norm: crate::parse::normalize(title),
            year: None,
            season: None,
            disc: false,
            file_ids: vec![],
        };
        let mut db = Db::open_in_memory().unwrap();
        let lib = db.add_library("Anime", Path::new("/a"), LibraryKind::Anime).unwrap();
        db.sync_groups(lib, &[group("A1", "Show A"), group("A2", "Show A"), group("B", "Show B")]).unwrap();
        let shows = db.shows(lib).unwrap();
        assert_eq!(shows.len(), 2, "same title shares a show");

        // Move B's folder into Show A by hand, as season 3: Show B disappears.
        let a = shows.iter().find(|s| s.title == "Show A").unwrap().id;
        db.set_group_match(lib, "B", a, Some(3)).unwrap();
        assert_eq!(db.shows(lib).unwrap().len(), 1);

        // A rescan neither undoes the manual move nor recreates Show B.
        db.sync_groups(lib, &[group("A1", "Show A"), group("A2", "Show A"), group("B", "Show B")]).unwrap();
        let m = db.group_matches(lib).unwrap().into_iter().find(|m| m.group_key == "B").unwrap();
        assert_eq!((m.show_id, m.season, m.manual), (a, Some(3), true));
        assert_eq!(db.shows(lib).unwrap().len(), 1);

        // A vanished automatic group is dropped.
        db.sync_groups(lib, &[group("A1", "Show A"), group("B", "Show B")]).unwrap();
        assert_eq!(db.group_matches(lib).unwrap().len(), 2);
    }

    #[test]
    fn settings_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.setting("fullscreen").unwrap(), None);
        db.set_setting("fullscreen", "true").unwrap();
        db.set_setting("fullscreen", "false").unwrap();
        assert_eq!(db.setting("fullscreen").unwrap().as_deref(), Some("false"));
    }

    #[test]
    fn scan_diffing() {
        let mut db = Db::open_in_memory().unwrap();
        let lib = db.add_library("Anime", Path::new("/media/anime"), LibraryKind::Anime).unwrap();
        assert!(db.add_library("Dup", Path::new("/media/anime"), LibraryKind::Tv).is_err());

        let s = db.apply_scan(lib, &[file("a/1.mkv"), file("a/2.mkv")]).unwrap();
        assert_eq!((s.added, s.removed, s.total), (2, 0, 2));

        let s = db.apply_scan(lib, &[file("a/2.mkv"), file("a/3.mkv")]).unwrap();
        assert_eq!((s.added, s.removed), (1, 1));
        let mut names: Vec<_> = db.files(lib).unwrap().into_iter().map(|f| f.rel_path).collect();
        names.sort();
        assert_eq!(names, ["a/2.mkv", "a/3.mkv"]);

        // A file coming back keeps its row (and so its watch history).
        let s = db.apply_scan(lib, &[file("a/1.mkv"), file("a/2.mkv"), file("a/3.mkv")]).unwrap();
        assert_eq!((s.added, s.removed), (1, 0));

        let id = db.files(lib).unwrap()[0].id;
        let p = Progress { position: Some(42.0), duration: Some(100.0), watched: false, last_played: Some(7) };
        db.set_progress(id, &p).unwrap();
        let stored = db.files(lib).unwrap().into_iter().find(|f| f.id == id).unwrap().progress;
        assert_eq!(stored, p);
        db.set_watched(&[id], true).unwrap();
        let stored = db.files(lib).unwrap().into_iter().find(|f| f.id == id).unwrap().progress;
        assert_eq!((stored.watched, stored.position), (true, None));

        db.remove_library(lib).unwrap();
        assert!(db.files(lib).unwrap().is_empty());
    }
}
