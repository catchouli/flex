//! Importing watch state and history from a Plex Media Server database.
//!
//! Plex paths (e.g. `/data/Media/Anime/Show/ep01.mkv`) are matched to Flex files by their
//! path *relative to the Plex library folder*, so it doesn't matter where the files are
//! mounted now (`U:\` on Windows, `~/media/anime` on a Mac). Each Plex library is paired
//! with the Flex library that contains most of its files.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use unicode_normalization::UnicodeNormalization;

use crate::progress::Progress;
use crate::tree::Tree;

#[derive(Clone, Debug)]
pub struct PlexItem {
    pub file: String,
    pub watched: bool,
    pub offset_secs: Option<f64>,
    pub duration_secs: Option<f64>,
    pub last_viewed: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct PlexView {
    pub file: String,
    pub viewed_at: i64,
}

#[derive(Debug, Default)]
pub struct PlexData {
    pub account: String,
    /// Plex library folders, e.g. "/data/Media/Anime".
    pub roots: Vec<String>,
    pub items: Vec<PlexItem>,
    pub views: Vec<PlexView>,
    /// History entries for media Plex no longer has (can't be tied to a file).
    pub orphan_views: usize,
    /// When Plex added each file (Unix seconds), for every file it knows.
    pub added: Vec<(String, i64)>,
}

/// Reads the account with the most history (the household's main account).
pub fn read(path: &Path) -> Result<PlexData> {
    // immutable: never touch the copy (no -wal/-shm files), even on a read-only share.
    let uri = format!("file:{}?mode=ro&immutable=1", path.to_string_lossy().replace('?', "%3f").replace('#', "%23"));
    let conn = Connection::open_with_flags(&uri, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)
        .with_context(|| format!("opening {}", path.display()))?;
    let (account_id, account): (i64, String) = conn
        .query_row(
            "SELECT v.account_id, COALESCE(a.name, '') FROM metadata_item_views v
             LEFT JOIN accounts a ON a.id = v.account_id
             GROUP BY v.account_id ORDER BY count(*) DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .context("this doesn't look like a Plex library database")?;

    let roots = {
        let mut stmt = conn.prepare("SELECT root_path FROM section_locations")?;
        stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?
    };
    // Plex stores times as text like "2026-10-08 21:07:31" (UTC).
    let items = {
        let mut stmt = conn.prepare(
            "SELECT p.file, s.view_count, s.view_offset, CAST(strftime('%s', s.last_viewed_at) AS INTEGER), mi.duration
             FROM metadata_item_settings s
             JOIN metadata_items m ON m.guid = s.guid
             JOIN media_items mi ON mi.metadata_item_id = m.id
             JOIN media_parts p ON p.media_item_id = mi.id
             WHERE s.account_id = ?1 AND (s.view_count > 0 OR s.view_offset > 0)",
        )?;
        let rows = stmt.query_map([account_id], |r| {
            let count: Option<i64> = r.get(1)?;
            let offset_ms: Option<i64> = r.get(2)?;
            let duration_ms: Option<i64> = r.get(4)?;
            Ok(PlexItem {
                file: r.get(0)?,
                watched: count.unwrap_or(0) > 0,
                offset_secs: offset_ms.filter(|o| *o > 0).map(|o| o as f64 / 1000.0),
                duration_secs: duration_ms.filter(|d| *d > 0).map(|d| d as f64 / 1000.0),
                last_viewed: r.get(3)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let views = {
        let mut stmt = conn.prepare(
            "SELECT p.file, CAST(strftime('%s', v.viewed_at) AS INTEGER)
             FROM metadata_item_views v
             JOIN metadata_items m ON m.guid = v.guid
             JOIN media_items mi ON mi.metadata_item_id = m.id
             JOIN media_parts p ON p.media_item_id = mi.id
             WHERE v.account_id = ?1 AND v.viewed_at IS NOT NULL",
        )?;
        let rows = stmt.query_map([account_id], |r| Ok(PlexView { file: r.get(0)?, viewed_at: r.get(1)? }))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let added = {
        let mut stmt = conn.prepare(
            "SELECT p.file, CAST(strftime('%s', m.added_at) AS INTEGER)
             FROM media_parts p
             JOIN media_items mi ON mi.id = p.media_item_id
             JOIN metadata_items m ON m.id = mi.metadata_item_id
             WHERE m.added_at IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let orphan_views: usize = conn.query_row(
        "SELECT count(*) FROM metadata_item_views WHERE account_id = ?1
         AND guid NOT IN (SELECT guid FROM metadata_items WHERE guid IS NOT NULL)",
        [account_id],
        |r| r.get(0),
    )?;
    Ok(PlexData { account, roots, items, views, orphan_views, added })
}

/// Comparable form of a relative path: '/'-separated, Unicode NFC, lower case.
fn path_key(rel: &str) -> String {
    rel.replace('\\', "/").trim_matches('/').nfc().collect::<String>().to_lowercase()
}

fn file_key(rel: &str) -> String {
    path_key(rel.rsplit(['/', '\\']).next().unwrap_or(rel))
}

/// How one Plex library folder lines up with Flex.
#[derive(Clone, Debug)]
pub struct SectionMapping {
    pub root: String,
    pub library_id: Option<i64>,
    pub matched: usize,
    pub total: usize,
}

#[derive(Debug, Default)]
pub struct ImportPlan {
    pub sections: Vec<SectionMapping>,
    /// Progress to merge into each matched file.
    pub progress: Vec<(i64, PlexItem)>,
    /// (file id, viewed at) history entries.
    pub history: Vec<(i64, i64)>,
    /// (file id, added at) from Plex's library.
    pub added: Vec<(i64, i64)>,
    pub unmatched_items: usize,
    pub unmatched_views: usize,
}

struct LibIndex {
    by_path: HashMap<String, i64>,
    /// File name -> id, only for names that are unique in the library.
    by_name: HashMap<String, Option<i64>>,
}

/// Works out which Flex file each Plex entry refers to.
pub fn plan(data: &PlexData, libraries: &[(i64, &Tree)]) -> ImportPlan {
    let indexes: Vec<(i64, LibIndex)> = libraries
        .iter()
        .map(|(id, tree)| {
            let mut by_name: HashMap<String, Option<i64>> = HashMap::new();
            for f in tree.files() {
                by_name.entry(file_key(&f.rel_path)).and_modify(|e| *e = None).or_insert(Some(f.id));
            }
            let by_path = tree.files().iter().map(|f| (path_key(&f.rel_path), f.id)).collect();
            (*id, LibIndex { by_path, by_name })
        })
        .collect();

    // Longest Plex root first, so nested roots resolve to the most specific one.
    let mut roots = data.roots.clone();
    roots.sort_by_key(|r| std::cmp::Reverse(r.len()));
    let split = |file: &str| -> Option<(usize, String)> {
        let i = roots.iter().position(|r| file.starts_with(r.as_str()))?;
        Some((i, path_key(&file[roots[i].len()..])))
    };

    // Pair each Plex root with the Flex library holding most of its files.
    let mut counts: HashMap<(usize, i64), usize> = HashMap::new();
    let mut totals: HashMap<usize, usize> = HashMap::new();
    for item in &data.items {
        let Some((root, rel)) = split(&item.file) else { continue };
        *totals.entry(root).or_default() += 1;
        for (lib, index) in &indexes {
            if index.by_path.contains_key(&rel) {
                *counts.entry((root, *lib)).or_default() += 1;
            }
        }
    }
    // A pairing needs a fair share of the Plex library's files, so a few shared files
    // (say, a series kept in the movies folder too) can't pull a whole library across.
    let library_for = |root: usize| {
        let total = totals.get(&root).copied().unwrap_or(0);
        indexes
            .iter()
            .map(|(lib, _)| (*lib, counts.get(&(root, *lib)).copied().unwrap_or(0)))
            .max_by_key(|(_, n)| *n)
            .filter(|(_, n)| *n > 0 && *n * 5 >= total)
    };

    let find = |file: &str| -> Option<i64> {
        let (root, rel) = split(file)?;
        let (lib, _) = library_for(root)?;
        let index = &indexes.iter().find(|(id, _)| *id == lib)?.1;
        index.by_path.get(&rel).copied().or_else(|| index.by_name.get(&file_key(&rel)).copied().flatten())
    };

    let mut plan = ImportPlan::default();
    let mut matched_per_root: HashMap<usize, usize> = HashMap::new();
    for item in &data.items {
        match find(&item.file) {
            Some(id) => {
                if let Some((root, _)) = split(&item.file) {
                    *matched_per_root.entry(root).or_default() += 1;
                }
                plan.progress.push((id, item.clone()));
            }
            None => plan.unmatched_items += 1,
        }
    }
    for view in &data.views {
        match find(&view.file) {
            Some(id) => plan.history.push((id, view.viewed_at)),
            None => plan.unmatched_views += 1,
        }
    }
    plan.unmatched_views += data.orphan_views;
    plan.added = data.added.iter().filter_map(|(file, at)| Some((find(file)?, *at))).collect();
    plan.sections = roots
        .iter()
        .enumerate()
        .filter(|(i, _)| totals.contains_key(i))
        .map(|(i, root)| SectionMapping {
            root: root.clone(),
            library_id: library_for(i).map(|(lib, _)| lib),
            matched: matched_per_root.get(&i).copied().unwrap_or(0),
            total: totals[&i],
        })
        .collect();
    plan
}

/// Merges Plex progress into Flex's: watched stays watched; the resume position and last
/// played time come from whichever was played more recently.
pub fn merge(existing: Progress, plex: &PlexItem) -> Progress {
    let plex_newer = match (plex.last_viewed, existing.last_played) {
        (Some(p), Some(e)) => p > e,
        (Some(_), None) => true,
        _ => false,
    };
    let mut merged = existing;
    merged.watched |= plex.watched;
    if plex_newer {
        merged.last_played = plex.last_viewed;
        merged.position = plex.offset_secs;
        merged.duration = plex.duration_secs.or(existing.duration);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::MediaFile;

    fn tree(paths: &[(i64, &str)]) -> Tree {
        Tree::new(
            paths
                .iter()
                .map(|(id, p)| MediaFile { id: *id, rel_path: (*p).into(), ..Default::default() })
                .collect(),
        )
    }

    fn item(file: &str) -> PlexItem {
        PlexItem { file: file.into(), watched: true, offset_secs: None, duration_secs: None, last_viewed: Some(100) }
    }

    #[test]
    fn maps_plex_roots_to_flex_libraries_by_relative_path() {
        let anime = tree(&[(1, "Dorohedoro/ep01.mkv"), (2, "Dorohedoro/ep02.mkv"), (3, "Café/ep01.mkv")]);
        let tv = tree(&[(10, "South Park/S19E06.mkv"), (11, "Dorohedoro/ep01.mkv")]);
        let data = PlexData {
            roots: vec!["/data/Media/Anime".into(), "/data/Media/TV".into()],
            items: vec![
                item("/data/Media/Anime/Dorohedoro/ep01.mkv"),
                item("/data/Media/Anime/dorohedoro/EP02.mkv"), // case differs
                item("/data/Media/Anime/Cafe\u{301}/ep01.mkv"),  // decomposed é (NFD)
                item("/data/Media/TV/South Park/S19E06.mkv"),
                item("/data/Media/TV/Gone/x.mkv"),
            ],
            views: vec![PlexView { file: "/data/Media/TV/South Park/S19E06.mkv".into(), viewed_at: 5 }],
            ..Default::default()
        };
        let plan = plan(&data, &[(100, &anime), (200, &tv)]);
        let ids: Vec<i64> = plan.progress.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [1, 2, 3, 10]);
        assert_eq!(plan.unmatched_items, 1);
        assert_eq!(plan.history, [(10, 5)]);
        let anime_section = plan.sections.iter().find(|s| s.root.ends_with("Anime")).unwrap();
        assert_eq!((anime_section.library_id, anime_section.matched, anime_section.total), (Some(100), 3, 3));
    }

    #[test]
    fn merging_keeps_the_newer_position() {
        let flex = Progress { position: Some(30.0), duration: Some(100.0), watched: false, last_played: Some(200) };
        let older = PlexItem { file: String::new(), watched: false, offset_secs: Some(80.0), duration_secs: Some(100.0), last_viewed: Some(100) };
        assert_eq!(merge(flex, &older), flex);
        let newer = PlexItem { last_viewed: Some(300), ..older };
        assert_eq!(merge(flex, &newer).position, Some(80.0));
        let watched = PlexItem { watched: true, offset_secs: None, ..newer };
        let m = merge(flex, &watched);
        assert!(m.watched && m.position.is_none());
    }
}
