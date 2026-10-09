//! SQLite storage for libraries and their files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

use crate::library::{Library, LibraryKind, MediaFile};
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
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        match version {
            0 => {
                conn.execute_batch(SCHEMA)?;
                conn.pragma_update(None, "user_version", 1)?;
            }
            1 => {}
            v => bail!("database is from a newer version of Flex (schema {v})"),
        }
        Ok(Self { conn })
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
            "SELECT id, library_id, rel_path, size, mtime FROM media_files WHERE library_id = ?1 AND missing = 0",
        )?;
        let rows = stmt.query_map([library_id], |r| {
            Ok(MediaFile {
                id: r.get(0)?,
                library_id: r.get(1)?,
                rel_path: r.get(2)?,
                size: r.get::<_, i64>(3)? as u64,
                mtime: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
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

        db.remove_library(lib).unwrap();
        assert!(db.files(lib).unwrap().is_empty());
    }
}
