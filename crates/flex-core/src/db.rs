//! SQLite storage for libraries and their files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

use crate::library::{Library, LibraryKind, MediaFile};
use crate::progress::Progress;
use crate::scan::{Previous, ScanResult, ScannedFile};

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
    // 4: viewing history (Flex sessions and imports)
    "CREATE TABLE play_history (
        id         INTEGER PRIMARY KEY,
        file_id    INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
        played_at  INTEGER NOT NULL,
        position   REAL,
        completed  INTEGER NOT NULL DEFAULT 0,
        source     TEXT NOT NULL DEFAULT 'flex',
        UNIQUE (file_id, played_at)
    );
    CREATE INDEX play_history_by_time ON play_history(played_at);",
    // 5: richer metadata (details, cast, episode titles) and when files were added
    "ALTER TABLE media_files ADD COLUMN added_at INTEGER;
    UPDATE media_files SET added_at = mtime;
    ALTER TABLE shows ADD COLUMN release_date TEXT;
    ALTER TABLE shows ADD COLUMN runtime INTEGER;
    ALTER TABLE shows ADD COLUMN genres TEXT;
    ALTER TABLE shows ADD COLUMN backdrop TEXT;
    ALTER TABLE shows ADD COLUMN details_at INTEGER;
    -- keyed by provider id so they survive shows being regrouped
    CREATE TABLE credits (
        provider_id TEXT NOT NULL,
        kind        TEXT NOT NULL, -- 'cast' (TMDB) or 'voice' (AniList, Japanese)
        ord         INTEGER NOT NULL,
        name        TEXT NOT NULL,
        role        TEXT,
        image       TEXT,
        PRIMARY KEY (provider_id, kind, ord)
    );
    CREATE TABLE tv_episodes (
        provider_id TEXT NOT NULL,
        season      INTEGER NOT NULL,
        episode     INTEGER NOT NULL,
        title       TEXT,
        overview    TEXT,
        still       TEXT,
        air_date    TEXT,
        runtime     INTEGER,
        PRIMARY KEY (provider_id, season, episode)
    );
    CREATE TABLE tv_seasons (
        provider_id   TEXT NOT NULL,
        season        INTEGER NOT NULL,
        episode_count INTEGER NOT NULL,
        PRIMARY KEY (provider_id, season)
    );",
    // 6: when Japanese voice actors were last looked up (anime)
    "ALTER TABLE shows ADD COLUMN voice_at INTEGER;",
    // 7: other titles (romaji etc., searched too) and the watchlist. The AniList pass is
    // redone once to collect titles.
    "CREATE TABLE alt_titles (
        provider_id TEXT NOT NULL,
        title       TEXT NOT NULL,
        PRIMARY KEY (provider_id, title)
    );
    UPDATE shows SET voice_at = NULL;
    -- keyed by provider id ('tv:1'), or 'local:<show id>' for unmatched shows
    CREATE TABLE watchlist (
        key        TEXT PRIMARY KEY,
        library_id INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
        added_at   INTEGER NOT NULL
    );",
    // 8: who's who in credits (TMDB / AniList person ids) for actor pages, and their
    // online filmographies. Cast is fetched again (cheaply: no episodes) to get the ids.
    "ALTER TABLE credits ADD COLUMN person TEXT;
    CREATE INDEX credits_by_person ON credits(person);
    CREATE INDEX credits_by_name ON credits(name COLLATE NOCASE);
    -- set once the cast has been saved with person ids
    ALTER TABLE shows ADD COLUMN cast_ids INTEGER;
    UPDATE shows SET voice_at = NULL;
    CREATE TABLE people (
        person     TEXT PRIMARY KEY,
        fetched_at INTEGER NOT NULL,
        data       TEXT NOT NULL -- JSON metadata::Person
    );",
    // 9: parsed file names per folder, so organising at startup needn't parse them again
    "CREATE TABLE parse_cache (
        library_id  INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
        dir         TEXT NOT NULL,
        fingerprint TEXT NOT NULL, -- parse::folder_fingerprint
        data        TEXT NOT NULL, -- JSON [[file id, parse::Parsed], ...]
        PRIMARY KEY (library_id, dir)
    );",
    // 10: music albums matched on MusicBrainz, and what's known about their artists
    "CREATE TABLE music_matches (
        library_id INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
        album_key  TEXT NOT NULL,   -- music::Album::key
        data       TEXT,            -- JSON musicbrainz::ReleaseGroup; NULL = not found
        manual     INTEGER NOT NULL DEFAULT 0,
        checked_at INTEGER NOT NULL,
        PRIMARY KEY (library_id, album_key)
    );
    CREATE TABLE music_artists (
        artist     TEXT PRIMARY KEY, -- MusicBrainz id
        fetched_at INTEGER NOT NULL,
        data       TEXT NOT NULL     -- JSON musicbrainz::ArtistInfo
    );",
    // 11: folders' modified times from the last scan, so rescans skip unchanged folders
    "CREATE TABLE scan_dirs (
        library_id INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
        rel_path   TEXT NOT NULL,
        mtime      INTEGER NOT NULL, -- nanoseconds; 0 = read it again
        PRIMARY KEY (library_id, rel_path)
    ) WITHOUT ROWID;",
    // 12: YouTube: subscribed channels, their recent videos (from RSS) and watch progress
    "CREATE TABLE yt_channels (
        id         TEXT PRIMARY KEY, -- UC...
        title      TEXT NOT NULL,
        added_at   INTEGER NOT NULL,
        checked_at INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE yt_videos (
        id         TEXT PRIMARY KEY,
        channel_id TEXT NOT NULL,
        title      TEXT NOT NULL,
        channel    TEXT NOT NULL,
        published  INTEGER NOT NULL,
        short      INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX yt_videos_channel ON yt_videos (channel_id, published);
    CREATE INDEX yt_videos_published ON yt_videos (published);
    CREATE TABLE yt_progress (
        id         TEXT PRIMARY KEY,
        title      TEXT NOT NULL,
        channel    TEXT NOT NULL,
        position   REAL,              -- NULL: not started, or finished
        duration   REAL,
        watched    INTEGER NOT NULL DEFAULT 0,
        updated_at INTEGER NOT NULL
    );",
    // 13: entries removed from Continue Watching (until played again)
    "CREATE TABLE continue_hidden (
        key       TEXT PRIMARY KEY, -- '<library id>:<file id>', or 'yt:<video id>'
        hidden_at INTEGER NOT NULL
    );"
];

/// What MusicBrainz said about one album.
#[derive(Clone, Debug, PartialEq)]
pub struct MusicMatch {
    /// `None` = looked up, nothing close enough (or unmatched by hand).
    pub release_group: Option<crate::musicbrainz::ReleaseGroup>,
    /// Chosen by hand: never redone automatically.
    pub manual: bool,
    /// Unix seconds.
    pub checked_at: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub file_id: i64,
    /// Unix seconds.
    pub played_at: i64,
    pub position: Option<f64>,
    pub completed: bool,
    /// "flex" or "plex".
    pub source: String,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ImportSummary {
    pub files_updated: usize,
    pub history_added: usize,
    pub dates_added: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
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
    pub release_date: Option<String>,
    /// Minutes (per episode for TV).
    pub runtime: Option<i32>,
    /// Comma separated.
    pub genres: Option<String>,
    pub backdrop: Option<String>,
    /// When details were last fetched (Unix seconds).
    pub details_at: Option<i64>,
}

/// TMDB's title etc. for one episode.
#[derive(Clone, Debug, PartialEq)]
pub struct EpisodeMeta {
    pub title: String,
    pub overview: String,
    pub still: Option<String>,
    pub air_date: Option<String>,
    pub runtime: Option<i32>,
}

/// Episode metadata for one series: per (season, episode), plus TMDB's season sizes.
#[derive(Clone, Debug, Default)]
pub struct SeriesMeta {
    pub episodes: HashMap<(i32, i32), EpisodeMeta>,
    pub seasons: Vec<crate::metadata::SeasonInfo>,
}

impl SeriesMeta {
    /// Metadata for a locally numbered episode (see [`crate::metadata::map_episode`]).
    pub fn episode(&self, season: i32, number: i32) -> Option<&EpisodeMeta> {
        let key = crate::metadata::map_episode(season, number, &self.seasons)?;
        self.episodes.get(&key)
    }
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
    pub(crate) conn: Connection,
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
        // Scans, matching and the UI write from separate connections. Write transactions
        // start IMMEDIATE so they wait for each other instead of failing to upgrade.
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
            "SELECT id, library_id, title, year, provider, provider_id, overview, poster, state,
                    release_date, runtime, genres, backdrop, details_at
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
                release_date: r.get(9)?,
                runtime: r.get(10)?,
                genres: r.get(11)?,
                backdrop: r.get(12)?,
                details_at: r.get(13)?,
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
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        {
            // group key -> (show id, manual)
            let existing: HashMap<String, (i64, bool)> = {
                let mut stmt =
                    tx.prepare("SELECT group_key, show_id, manual FROM group_matches WHERE library_id = ?1")?;
                let rows = stmt.query_map([library_id], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            // show id -> (norm, matched online)
            let show_info: HashMap<i64, (String, bool)> = {
                let mut stmt = tx.prepare("SELECT id, norm, state != 'local' FROM shows WHERE library_id = ?1")?;
                let rows = stmt.query_map([library_id], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
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
                // Keep manual choices, automatic ones whose show still has this title, and
                // ones in a show already matched online (where differently named folders
                // were merged). Re-place the rest: their parsed title has changed.
                match existing.get(&g.key) {
                    Some((_, true)) => continue,
                    Some((show, false))
                        if show_info.get(show).is_some_and(|(norm, matched)| *matched || *norm == g.norm) =>
                    {
                        continue
                    }
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

    /// Stores what was found online for a show. `state` is "matched", or "manual" when
    /// chosen by hand. If another show in the library already has this entry, this show's
    /// folders join it instead (e.g. a romaji-named folder and an English-named one).
    pub fn set_show_metadata(&mut self, show_id: i64, found: &crate::metadata::Candidate, state: &str) -> Result<()> {
        let provider_id = found.provider_id();
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let library_id: i64 = tx.query_row("SELECT library_id FROM shows WHERE id = ?1", [show_id], |r| r.get(0))?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM shows WHERE library_id = ?1 AND provider = 'tmdb' AND provider_id = ?2 AND id != ?3",
                params![library_id, provider_id, show_id],
                |r| r.get(0),
            )
            .optional()?;
        match existing {
            Some(into) => {
                tx.execute("UPDATE group_matches SET show_id = ?1 WHERE show_id = ?2", params![into, show_id])?;
                tx.execute("DELETE FROM shows WHERE id = ?1", [show_id])?;
            }
            None => {
                tx.execute(
                    "UPDATE shows SET provider = 'tmdb', provider_id = ?2, title = ?3, year = ?4, overview = ?5,
                     poster = ?6, state = ?7, details_at = NULL WHERE id = ?1",
                    params![show_id, provider_id, found.title, found.year, found.overview, found.poster_path, state],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Forgets automatic grouping and online matches for a library (manual choices stay),
    /// so the next organise + match starts afresh.
    pub fn reset_matching(&mut self, library_id: i64) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM group_matches WHERE library_id = ?1 AND manual = 0", [library_id])?;
        tx.execute("UPDATE shows SET state = 'local' WHERE library_id = ?1 AND state = 'unmatched'", [library_id])?;
        Self::delete_empty_shows(&tx, library_id)?;
        tx.commit()?;
        Ok(())
    }

    /// Matched shows whose details are missing or older than `max_age` seconds, or whose
    /// cast was saved without person ids: (show id, provider id, title, whole details
    /// needed). When only the cast is needed, episodes needn't be fetched again.
    pub fn shows_needing_details(&self, library_id: i64, max_age: i64) -> Result<Vec<(i64, String, String, bool)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, provider_id, title, details_at IS NULL OR details_at < strftime('%s', 'now') - ?2 AS stale
             FROM shows
             WHERE library_id = ?1 AND provider_id IS NOT NULL AND (stale OR cast_ids IS NULL)",
        )?;
        let rows = stmt.query_map(params![library_id, max_age], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Stores fetched details (and TMDB cast) for a show.
    pub fn save_details(&mut self, show_id: i64, provider_id: &str, d: &crate::metadata::Details) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let genres = (!d.genres.is_empty()).then(|| d.genres.join(", "));
        tx.execute(
            "UPDATE shows SET overview = ?2, release_date = ?3, runtime = ?4, genres = ?5, backdrop = ?6,
             poster = COALESCE(?7, poster), details_at = strftime('%s', 'now'), cast_ids = 1 WHERE id = ?1",
            params![show_id, d.overview, d.release_date, d.runtime, genres, d.backdrop_path, d.poster_path],
        )?;
        Self::replace_credits(&tx, provider_id, "cast", &d.cast)?;
        tx.execute("DELETE FROM tv_seasons WHERE provider_id = ?1", [provider_id])?;
        for s in &d.seasons {
            tx.execute(
                "INSERT INTO tv_seasons (provider_id, season, episode_count) VALUES (?1, ?2, ?3)",
                params![provider_id, s.number, s.episode_count],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn replace_credits(conn: &Connection, provider_id: &str, kind: &str, cast: &[crate::metadata::CastMember]) -> Result<()> {
        conn.execute("DELETE FROM credits WHERE provider_id = ?1 AND kind = ?2", params![provider_id, kind])?;
        for (i, c) in cast.iter().enumerate() {
            conn.execute(
                "INSERT INTO credits (provider_id, kind, ord, name, role, image, person) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![provider_id, kind, i as i64, c.name, c.role, c.image, c.person],
            )?;
        }
        Ok(())
    }

    /// Matched shows whose voice cast hasn't been looked up (or not for `max_age` seconds):
    /// (show id, provider id, title).
    pub fn shows_needing_voice_cast(&self, library_id: i64, max_age: i64) -> Result<Vec<(i64, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, provider_id, title FROM shows
             WHERE library_id = ?1 AND provider_id IS NOT NULL
             AND (voice_at IS NULL OR voice_at < strftime('%s', 'now') - ?2)",
        )?;
        let rows = stmt.query_map(params![library_id, max_age], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Stores what AniList had for a show (voice cast, other titles) and notes that it was
    /// looked up.
    pub fn save_anilist(&self, show_id: i64, provider_id: &str, info: &crate::metadata::AniListInfo) -> Result<()> {
        if !info.cast.is_empty() {
            Self::replace_credits(&self.conn, provider_id, "voice", &info.cast)?;
        }
        for t in &info.titles {
            self.conn.execute("INSERT OR IGNORE INTO alt_titles (provider_id, title) VALUES (?1, ?2)", params![provider_id, t])?;
        }
        self.conn.execute("UPDATE shows SET voice_at = strftime('%s', 'now') WHERE id = ?1", [show_id])?;
        Ok(())
    }

    /// Other titles by provider id.
    pub fn alt_titles(&self) -> Result<HashMap<String, Vec<String>>> {
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        let mut stmt = self.conn.prepare("SELECT provider_id, title FROM alt_titles")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (pid, title) = row?;
            out.entry(pid).or_default().push(title);
        }
        Ok(out)
    }

    /// Watchlist entries: (key, library id, added at), most recent first.
    /// Removes an entry from Continue Watching until it's played again.
    pub fn hide_continue(&self, key: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO continue_hidden (key, hidden_at) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET hidden_at = ?2",
            params![key, now],
        )?;
        Ok(())
    }

    /// Entries removed from Continue Watching: key → when.
    pub fn continue_hidden(&self) -> Result<HashMap<String, i64>> {
        let mut stmt = self.conn.prepare("SELECT key, hidden_at FROM continue_hidden")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn watchlist(&self) -> Result<Vec<(String, i64, i64)>> {
        let mut stmt = self.conn.prepare("SELECT key, library_id, added_at FROM watchlist ORDER BY added_at DESC")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_watchlist(&self, key: &str, library_id: i64, on: bool) -> Result<()> {
        if on {
            self.conn.execute(
                "INSERT OR IGNORE INTO watchlist (key, library_id, added_at) VALUES (?1, ?2, strftime('%s', 'now'))",
                params![key, library_id],
            )?;
        } else {
            self.conn.execute("DELETE FROM watchlist WHERE key = ?1", [key])?;
        }
        Ok(())
    }

    /// Credits of one kind ("cast" or "voice"), in billing order.
    pub fn credits(&self, provider_id: &str, kind: &str) -> Result<Vec<crate::metadata::CastMember>> {
        let mut stmt =
            self.conn.prepare("SELECT name, role, image, person FROM credits WHERE provider_id = ?1 AND kind = ?2 ORDER BY ord")?;
        let rows = stmt.query_map(params![provider_id, kind], |r| {
            Ok(crate::metadata::CastMember {
                name: r.get(0)?,
                role: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                image: r.get(2)?,
                person: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Everything a person is credited in: (provider id, role), found by their id, or by
    /// name (the same person under TMDB and AniList, or credits saved before ids were).
    pub fn appearances(&self, person: Option<&str>, name: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT provider_id, COALESCE(role, '') FROM credits
             WHERE person = ?1 OR name = ?2 COLLATE NOCASE ORDER BY provider_id, kind, ord",
        )?;
        let rows = stmt.query_map(params![person, name], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out: Vec<(String, String)> = Vec::new();
        for row in rows {
            let (pid, role) = row?;
            // Credited twice (cast and voice cast): keep the first role.
            if !out.iter().any(|(p, _)| *p == pid) {
                out.push((pid, role));
            }
        }
        Ok(out)
    }

    /// A person's cached online details, if fetched within `max_age` seconds.
    pub fn person(&self, person: &str, max_age: i64) -> Result<Option<crate::metadata::Person>> {
        let data: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM people WHERE person = ?1 AND fetched_at >= strftime('%s', 'now') - ?2",
                params![person, max_age],
                |r| r.get(0),
            )
            .optional()?;
        // Unreadable (e.g. from an older version): fetch again.
        Ok(data.and_then(|d| serde_json::from_str(&d).ok()))
    }

    pub fn save_person(&self, person: &str, info: &crate::metadata::Person) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO people (person, fetched_at, data) VALUES (?1, strftime('%s', 'now'), ?2)",
            params![person, serde_json::to_string(info)?],
        )?;
        Ok(())
    }

    pub fn save_episodes(&mut self, provider_id: &str, episodes: &[crate::metadata::EpisodeInfo]) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for e in episodes {
            tx.execute(
                "INSERT OR REPLACE INTO tv_episodes (provider_id, season, episode, title, overview, still, air_date, runtime)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![provider_id, e.season, e.episode, e.title, e.overview, e.still_path, e.air_date, e.runtime],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// All series metadata, by provider id.
    pub fn series_meta(&self) -> Result<HashMap<String, SeriesMeta>> {
        let mut out: HashMap<String, SeriesMeta> = HashMap::new();
        let mut stmt = self.conn.prepare("SELECT provider_id, season, episode_count FROM tv_seasons")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))?;
        for row in rows {
            let (pid, number, episode_count) = row?;
            out.entry(pid).or_default().seasons.push(crate::metadata::SeasonInfo { number, episode_count });
        }
        let mut stmt =
            self.conn.prepare("SELECT provider_id, season, episode, title, overview, still, air_date, runtime FROM tv_episodes")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get(1)?, r.get(2)?),
                EpisodeMeta {
                    title: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    overview: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    still: r.get(5)?,
                    air_date: r.get(6)?,
                    runtime: r.get(7)?,
                },
            ))
        })?;
        for row in rows {
            let (pid, key, meta) = row?;
            out.entry(pid).or_default().episodes.insert(key, meta);
        }
        Ok(out)
    }

    pub fn set_show_state(&self, show_id: i64, state: &str) -> Result<()> {
        self.conn.execute("UPDATE shows SET state = ?2 WHERE id = ?1", params![show_id, state])?;
        Ok(())
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

    /// Cached folder parses for a library: dir -> (fingerprint, JSON data).
    pub fn parse_cache(&self, library_id: i64) -> Result<HashMap<String, (String, String)>> {
        let mut stmt = self.conn.prepare("SELECT dir, fingerprint, data FROM parse_cache WHERE library_id = ?1")?;
        let rows = stmt.query_map([library_id], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Stores new folder parses (dir, fingerprint, JSON data) and forgets folders that
    /// no longer exist.
    /// Every album of a library that's been looked up, by album key.
    pub fn music_matches(&self, library_id: i64) -> Result<HashMap<String, MusicMatch>> {
        let mut stmt = self.conn.prepare("SELECT album_key, data, manual, checked_at FROM music_matches WHERE library_id = ?1")?;
        let rows = stmt.query_map([library_id], |r| {
            let data: Option<String> = r.get(1)?;
            Ok((r.get::<_, String>(0)?, data, r.get::<_, bool>(2)?, r.get::<_, i64>(3)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (key, data, manual, checked_at) = row?;
            let release_group = data.and_then(|d| serde_json::from_str(&d).ok());
            out.insert(key, MusicMatch { release_group, manual, checked_at });
        }
        Ok(out)
    }

    pub fn set_music_match(&self, library_id: i64, album_key: &str, release_group: Option<&crate::musicbrainz::ReleaseGroup>, manual: bool) -> Result<()> {
        let data = release_group.map(serde_json::to_string).transpose()?;
        self.conn.execute(
            "INSERT OR REPLACE INTO music_matches (library_id, album_key, data, manual, checked_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%s', 'now'))",
            params![library_id, album_key, data, manual],
        )?;
        Ok(())
    }

    /// Artist info fetched within `max_age_secs`, by MusicBrainz id.
    pub fn music_artists(&self, max_age_secs: i64) -> Result<HashMap<String, crate::musicbrainz::ArtistInfo>> {
        let mut stmt = self.conn.prepare("SELECT artist, data FROM music_artists WHERE fetched_at >= strftime('%s', 'now') - ?1")?;
        let rows = stmt.query_map([max_age_secs], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, data) = row?;
            if let Ok(info) = serde_json::from_str(&data) {
                out.insert(id, info);
            }
        }
        Ok(out)
    }

    pub fn set_music_artist(&self, artist: &str, info: &crate::musicbrainz::ArtistInfo) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO music_artists (artist, fetched_at, data) VALUES (?1, strftime('%s', 'now'), ?2)",
            params![artist, serde_json::to_string(info)?],
        )?;
        Ok(())
    }

    pub fn save_parse_cache(&mut self, library_id: i64, fresh: &[(String, String, String)], gone: &[String]) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        {
            let mut upsert = tx.prepare(
                "INSERT INTO parse_cache (library_id, dir, fingerprint, data) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(library_id, dir) DO UPDATE SET fingerprint = ?3, data = ?4",
            )?;
            for (dir, fingerprint, data) in fresh {
                upsert.execute(params![library_id, dir, fingerprint, data])?;
            }
            let mut delete = tx.prepare("DELETE FROM parse_cache WHERE library_id = ?1 AND dir = ?2")?;
            for dir in gone {
                delete.execute(params![library_id, dir])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Files currently present in a library (not marked missing).
    pub fn files(&self, library_id: i64) -> Result<Vec<MediaFile>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, library_id, rel_path, size, mtime, position, duration, watched, last_played, added_at
             FROM media_files WHERE library_id = ?1 AND missing = 0",
        )?;
        let rows = stmt.query_map([library_id], |r| {
            Ok(MediaFile {
                id: r.get(0)?,
                library_id: r.get(1)?,
                rel_path: r.get(2)?,
                size: r.get::<_, i64>(3)? as u64,
                mtime: r.get(4)?,
                added_at: r.get::<_, Option<i64>>(9)?.unwrap_or_default(),
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

    /// Records a viewing session. A session already recorded at that time is left alone.
    pub fn add_history(&self, file_id: i64, played_at: i64, position: Option<f64>, completed: bool) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO play_history (file_id, played_at, position, completed) VALUES (?1, ?2, ?3, ?4)",
            params![file_id, played_at, position, completed],
        )?;
        Ok(())
    }

    /// Most recent first.
    pub fn history(&self, library_id: i64, limit: usize) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT h.file_id, h.played_at, h.position, h.completed, h.source
             FROM play_history h JOIN media_files f ON f.id = h.file_id
             WHERE f.library_id = ?1 ORDER BY h.played_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![library_id, limit as i64], |r| {
            Ok(HistoryEntry { file_id: r.get(0)?, played_at: r.get(1)?, position: r.get(2)?, completed: r.get(3)?, source: r.get(4)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Applies a Plex import: merges watch state into files and adds history (importing
    /// twice doesn't duplicate anything).
    pub fn import_plex(&mut self, plan: &crate::plex::ImportPlan) -> Result<ImportSummary> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut summary = ImportSummary::default();
        {
            let mut read = tx.prepare("SELECT position, duration, watched, last_played FROM media_files WHERE id = ?1")?;
            let mut write = tx.prepare(
                "UPDATE media_files SET position = ?2, duration = ?3, watched = ?4, last_played = ?5 WHERE id = ?1",
            )?;
            for (id, item) in &plan.progress {
                let existing = read.query_row([id], |r| {
                    Ok(Progress { position: r.get(0)?, duration: r.get(1)?, watched: r.get(2)?, last_played: r.get(3)? })
                })?;
                let merged = crate::plex::merge(existing, item);
                if merged != existing {
                    write.execute(params![id, merged.position, merged.duration, merged.watched, merged.last_played])?;
                    summary.files_updated += 1;
                }
            }
            let mut add = tx.prepare(
                "INSERT OR IGNORE INTO play_history (file_id, played_at, completed, source) VALUES (?1, ?2, 1, 'plex')",
            )?;
            for (id, at) in &plan.history {
                summary.history_added += add.execute(params![id, at])?;
            }
            // Plex's "added" date is kept unless Flex already has an earlier one.
            let mut added = tx.prepare(
                "UPDATE media_files SET added_at = ?2 WHERE id = ?1 AND (added_at IS NULL OR added_at > ?2)",
            )?;
            for (id, at) in &plan.added {
                summary.dates_added += added.execute(params![id, at])?;
            }
        }
        tx.commit()?;
        Ok(summary)
    }

    /// Marks files watched or unwatched, clearing any resume position.
    pub fn set_watched(&mut self, file_ids: &[i64], watched: bool) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
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
    /// What the last scan found, for the next one. `full` leaves out the folder times, so
    /// every folder is read again.
    pub fn previous_scan(&self, library_id: i64, full: bool) -> Result<Previous> {
        let files = self
            .files(library_id)?
            .into_iter()
            .map(|f| ScannedFile { rel_path: f.rel_path, size: f.size, mtime: f.mtime })
            .collect();
        let dirs = if full {
            HashMap::new()
        } else {
            let mut stmt = self.conn.prepare("SELECT rel_path, mtime FROM scan_dirs WHERE library_id = ?1")?;
            let rows = stmt.query_map([library_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        Ok(Previous { files, dirs })
    }

    pub fn apply_scan(&mut self, library_id: i64, scan: &ScanResult) -> Result<ScanSummary> {
        let files = &scan.files;
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM scan_dirs WHERE library_id = ?1", [library_id])?;
        {
            let mut insert = tx.prepare("INSERT INTO scan_dirs (library_id, rel_path, mtime) VALUES (?1, ?2, ?3)")?;
            for (rel, mtime) in &scan.dirs {
                insert.execute(params![library_id, rel, mtime])?;
            }
        }
        let mut existing: HashMap<String, (i64, bool)> = {
            let mut stmt = tx.prepare("SELECT id, rel_path, missing FROM media_files WHERE library_id = ?1")?;
            let rows = stmt.query_map([library_id], |r| Ok((r.get::<_, String>(1)?, (r.get(0)?, r.get(2)?))))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut summary = ScanSummary { total: files.len(), ..Default::default() };
        {
            let mut insert = tx.prepare(
                "INSERT INTO media_files (library_id, rel_path, size, mtime, added_at)
                 VALUES (?1, ?2, ?3, ?4, strftime('%s', 'now'))",
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
mod music_tests {
    use super::*;
    use crate::musicbrainz::{ArtistInfo, ReleaseGroup};

    #[test]
    fn stores_music_matches_and_artists() {
        let db = Db::open_in_memory().unwrap();
        let lib = db.add_library("Music", Path::new("/m"), LibraryKind::Music).unwrap();
        let rg = ReleaseGroup { id: "rg".into(), title: "T".into(), kind: Some("Album".into()), secondary: vec![], year: Some(2005), artists: vec![("A".into(), "a1".into())] };
        db.set_music_match(lib, "dir\u{1f}t", Some(&rg), false).unwrap();
        db.set_music_match(lib, "other\u{1f}", None, false).unwrap();
        let matches = db.music_matches(lib).unwrap();
        assert_eq!(matches["dir\u{1f}t"].release_group.as_ref(), Some(&rg));
        assert_eq!(matches["other\u{1f}"].release_group, None);
        let info = ArtistInfo { name: "A".into(), image: None, discography: vec![rg] };
        db.set_music_artist("a1", &info).unwrap();
        assert_eq!(db.music_artists(3600).unwrap()["a1"], info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(p: &str) -> ScannedFile {
        ScannedFile { rel_path: p.into(), size: 1, mtime: 0 }
    }

    fn found(files: &[ScannedFile]) -> ScanResult {
        ScanResult { files: files.to_vec(), dirs: vec![("".into(), 5), ("a".into(), 7)], ..Default::default() }
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

        // Matching online: a second show that turns out to be the same entry merges in.
        let found = crate::metadata::Candidate {
            kind: crate::metadata::Kind::Tv,
            id: 1,
            title: "Show A (TMDB)".into(),
            original_title: String::new(),
            year: Some(2020),
            overview: String::new(),
            poster_path: None,
            popularity: 0.0,
            genre_ids: vec![],
            original_language: String::new(),
        };
        let shows = db.shows(lib).unwrap();
        let a = shows[0].id;
        db.set_show_metadata(a, &found, "matched").unwrap();
        db.sync_groups(lib, &[group("A1", "Show A"), group("A2", "Show A"), group("B", "Show B"), group("C", "Other")])
            .unwrap();
        let other = db.shows(lib).unwrap().into_iter().find(|s| s.title == "Other").unwrap().id;
        db.set_show_metadata(other, &found, "matched").unwrap();
        assert!(db.shows(lib).unwrap().iter().all(|s| s.id != other));
        let c = db.group_matches(lib).unwrap().into_iter().find(|m| m.group_key == "C").unwrap();
        assert_eq!(c.show_id, a);
        // ...and stays merged on the next sync instead of being split off and re-matched.
        db.sync_groups(lib, &[group("A1", "Show A"), group("A2", "Show A"), group("B", "Show B"), group("C", "Other")])
            .unwrap();
        let c = db.group_matches(lib).unwrap().into_iter().find(|m| m.group_key == "C").unwrap();
        assert_eq!(c.show_id, a);
        assert!(db.shows(lib).unwrap().iter().all(|s| s.title != "Other"));

        // A vanished automatic group is dropped.
        db.sync_groups(lib, &[group("A1", "Show A"), group("B", "Show B")]).unwrap();
        assert_eq!(db.group_matches(lib).unwrap().len(), 2, "A2 and C dropped");
    }

    #[test]
    fn plex_import_is_idempotent() {
        use crate::plex::{ImportPlan, PlexItem};
        let mut db = Db::open_in_memory().unwrap();
        let lib = db.add_library("Anime", Path::new("/a"), LibraryKind::Anime).unwrap();
        db.apply_scan(lib, &found(&[file("a/1.mkv")])).unwrap();
        let id = db.files(lib).unwrap()[0].id;
        let item = PlexItem { file: String::new(), watched: true, offset_secs: None, duration_secs: Some(1400.0), last_viewed: Some(500) };
        let plan = ImportPlan {
            progress: vec![(id, item)],
            history: vec![(id, 400), (id, 500)],
            added: vec![(id, 300)],
            ..Default::default()
        };
        let first = db.import_plex(&plan).unwrap();
        assert_eq!((first.files_updated, first.history_added, first.dates_added), (1, 2, 1));
        let again = db.import_plex(&plan).unwrap();
        assert_eq!((again.files_updated, again.history_added, again.dates_added), (0, 0, 0));
        assert_eq!(db.files(lib).unwrap()[0].added_at, 300);
        assert!(db.files(lib).unwrap()[0].progress.watched);
        db.add_history(id, 600, Some(42.0), false).unwrap();
        let h = db.history(lib, 10).unwrap();
        assert_eq!(h.iter().map(|e| e.played_at).collect::<Vec<_>>(), [600, 500, 400]);
        assert_eq!(h[1].source, "plex");
    }

    #[test]
    fn details_and_episodes_roundtrip() {
        use crate::metadata::{CastMember, Details, EpisodeInfo, SeasonInfo};
        let mut db = Db::open_in_memory().unwrap();
        let lib = db.add_library("Anime", Path::new("/a"), LibraryKind::Anime).unwrap();
        let show = db.create_show(lib, "Show", None).unwrap();
        let details = Details {
            overview: "About".into(),
            runtime: Some(24),
            genres: vec!["Animation".into(), "Comedy".into()],
            cast: vec![CastMember { name: "A".into(), role: "B".into(), image: None, person: Some("tmdb:7".into()) }],
            seasons: vec![SeasonInfo { number: 1, episode_count: 12 }, SeasonInfo { number: 2, episode_count: 12 }],
            ..Default::default()
        };
        db.save_details(show, "tv:1", &details).unwrap();
        let s = db.shows(lib).unwrap().into_iter().find(|s| s.id == show).unwrap();
        assert_eq!((s.genres.as_deref(), s.runtime), (Some("Animation, Comedy"), Some(24)));
        assert!(s.details_at.is_some());
        assert_eq!(db.credits("tv:1", "cast").unwrap()[0].person.as_deref(), Some("tmdb:7"));
        assert_eq!(db.shows_needing_details(lib, 3600).unwrap().len(), 0);
        // Found by id, or by name (case aside).
        assert_eq!(db.appearances(Some("tmdb:7"), "x").unwrap(), [("tv:1".to_owned(), "B".to_owned())]);
        assert_eq!(db.appearances(None, "a").unwrap().len(), 1);
        let info = crate::metadata::Person { name: "A".into(), ..Default::default() };
        db.save_person("tmdb:7", &info).unwrap();
        assert_eq!(db.person("tmdb:7", 60).unwrap().unwrap().name, "A");
        let ep = |season, episode, title: &str| EpisodeInfo {
            season,
            episode,
            title: title.into(),
            overview: String::new(),
            still_path: None,
            air_date: None,
            runtime: None,
        };
        db.save_episodes("tv:1", &[ep(2, 1, "Second season start")]).unwrap();
        let meta = db.series_meta().unwrap();
        // Absolute episode 13 is season 2 episode 1.
        assert_eq!(meta["tv:1"].episode(1, 13).unwrap().title, "Second season start");
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

        let s = db.apply_scan(lib, &found(&[file("a/1.mkv"), file("a/2.mkv")])).unwrap();
        assert_eq!((s.added, s.removed, s.total), (2, 0, 2));

        let s = db.apply_scan(lib, &found(&[file("a/2.mkv"), file("a/3.mkv")])).unwrap();
        assert_eq!((s.added, s.removed), (1, 1));
        let previous = db.previous_scan(lib, false).unwrap();
        let mut files: Vec<_> = previous.files.iter().map(|f| f.rel_path.as_str()).collect();
        files.sort();
        assert_eq!(files, ["a/2.mkv", "a/3.mkv"], "missing files aren't carried over");
        assert_eq!(previous.dirs, HashMap::from([("".into(), 5), ("a".into(), 7)]));
        assert!(db.previous_scan(lib, true).unwrap().dirs.is_empty());
        let mut names: Vec<_> = db.files(lib).unwrap().into_iter().map(|f| f.rel_path).collect();
        names.sort();
        assert_eq!(names, ["a/2.mkv", "a/3.mkv"]);

        // A file coming back keeps its row (and so its watch history).
        let s = db.apply_scan(lib, &found(&[file("a/1.mkv"), file("a/2.mkv"), file("a/3.mkv")])).unwrap();
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
