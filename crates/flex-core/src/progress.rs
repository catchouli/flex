//! Watch progress: resume positions, the "watched" rule and Continue Watching.

use std::collections::HashSet;

use crate::library::Library;
use crate::tree::Tree;

/// Below this many seconds, playback isn't recorded (opened by mistake, or just sampling).
pub const MIN_RECORDED_SECS: f64 = 10.0;
/// Default fraction of a file that counts as watched (Plex uses the same default).
pub const DEFAULT_WATCHED_THRESHOLD: f64 = 0.9;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Progress {
    /// Resume position in seconds; `None` = start from the beginning.
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub watched: bool,
    /// Unix seconds when last played.
    pub last_played: Option<i64>,
}

impl Progress {
    /// Fraction watched (0–1) for a resumable file.
    pub fn fraction(&self) -> Option<f64> {
        match (self.position, self.duration) {
            (Some(p), Some(d)) if d > 0.0 => Some((p / d).clamp(0.0, 1.0)),
            _ => None,
        }
    }

    pub fn remaining_secs(&self) -> Option<f64> {
        Some((self.duration? - self.position?).max(0.0))
    }

    /// The new progress after playing up to `position` of `duration`, or `None` if this
    /// playback is too short to record. Passing the threshold marks the file watched and
    /// clears the resume position.
    pub fn after_playing(&self, position: f64, duration: f64, threshold: f64, now: i64) -> Option<Self> {
        let finished = duration > 0.0 && position / duration >= threshold;
        if !finished && position < MIN_RECORDED_SECS {
            return None;
        }
        Some(Self {
            position: (!finished).then_some(position),
            duration: (duration > 0.0).then_some(duration).or(self.duration),
            watched: self.watched || finished,
            last_played: Some(now),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContinueKind {
    /// Partway through this file.
    Resume,
    /// The next unwatched episode after one that was finished.
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContinueItem {
    pub library_id: i64,
    pub file_id: i64,
    pub kind: ContinueKind,
}

fn parent_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(d, _)| d)
}

/// What to offer in Continue Watching, most recent first: one entry per folder (season),
/// either the file you're partway through or the next unwatched episode.
pub fn continue_watching<'a>(
    libraries: impl IntoIterator<Item = (&'a Library, &'a Tree)>,
    limit: usize,
) -> Vec<ContinueItem> {
    let libraries: Vec<_> = libraries.into_iter().collect();
    let mut played: Vec<_> = libraries
        .iter()
        .flat_map(|(lib, tree)| {
            tree.files().iter().filter_map(move |f| f.progress.last_played.map(|t| (t, *lib, *tree, f)))
        })
        .collect();
    played.sort_by_key(|p| std::cmp::Reverse(p.0));

    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for (_, lib, tree, file) in played {
        // Episodes: one entry per folder (season). Movies stand alone, even side by side.
        let group = if lib.kind.is_episodic() { parent_of(&file.rel_path) } else { &file.rel_path };
        if !seen.insert((lib.id, group)) {
            continue; // an older entry from a folder already represented
        }
        let item = if file.progress.position.is_some() {
            Some(ContinueItem { library_id: lib.id, file_id: file.id, kind: ContinueKind::Resume })
        } else if lib.kind.is_episodic() {
            tree.next_unwatched_in_folder(file.id)
                .map(|next| ContinueItem { library_id: lib.id, file_id: next.id, kind: ContinueKind::Next })
        } else {
            None
        };
        items.extend(item);
        if items.len() == limit {
            break;
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{LibraryKind, MediaFile};

    #[test]
    fn watched_rule() {
        let p = Progress::default();
        assert_eq!(p.after_playing(5.0, 1400.0, 0.9, 1), None, "too short to record");
        let mid = p.after_playing(600.0, 1400.0, 0.9, 1).unwrap();
        assert_eq!((mid.position, mid.watched), (Some(600.0), false));
        let done = mid.after_playing(1300.0, 1400.0, 0.9, 2).unwrap();
        assert_eq!((done.position, done.watched, done.last_played), (None, true, Some(2)));
        // Rewatching part of a watched file keeps it watched but remembers the position.
        let again = done.after_playing(300.0, 1400.0, 0.9, 3).unwrap();
        assert_eq!((again.position, again.watched), (Some(300.0), true));
    }

    fn file(id: i64, lib: i64, path: &str, progress: Progress) -> MediaFile {
        MediaFile { id, library_id: lib, rel_path: path.into(), progress, ..Default::default() }
    }

    fn played(at: i64, position: Option<f64>, watched: bool) -> Progress {
        Progress { position, duration: Some(100.0), watched, last_played: Some(at) }
    }

    #[test]
    fn continue_watching_picks_resume_and_next() {
        let anime = Library { id: 1, name: "Anime".into(), root: "/a".into(), kind: LibraryKind::Anime };
        let movies = Library { id: 2, name: "Movies".into(), root: "/m".into(), kind: LibraryKind::Movies };
        let anime_tree = Tree::new(vec![
            file(1, 1, "A/ep1.mkv", played(10, None, true)),
            file(2, 1, "A/ep2.mkv", played(20, None, true)), // finished ep2 -> skip watched ep3 -> offer ep4
            file(3, 1, "A/ep3.mkv", Progress { watched: true, ..Default::default() }),
            file(8, 1, "A/ep4.mkv", Progress::default()),
            file(4, 1, "B/ep1.mkv", played(30, Some(40.0), false)), // partway
            file(5, 1, "C/ep1.mkv", played(5, None, true)), // last in folder: nothing
        ]);
        let movie_tree = Tree::new(vec![
            file(6, 2, "Film.mkv", played(40, Some(10.0), false)),
            file(7, 2, "Other.mkv", played(50, None, true)), // finished movie: nothing
        ]);
        let items = continue_watching([(&anime, &anime_tree), (&movies, &movie_tree)], 10);
        let summary: Vec<_> = items.iter().map(|i| (i.file_id, i.kind)).collect();
        assert_eq!(summary, [(6, ContinueKind::Resume), (4, ContinueKind::Resume), (8, ContinueKind::Next)]);
    }
}
