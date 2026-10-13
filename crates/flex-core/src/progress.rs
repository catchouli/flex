//! Watch progress: resume positions, the "watched" rule and Continue Watching.

use std::collections::HashSet;

use crate::library::Library;
use crate::tree::Tree;

/// Below this many seconds, playback isn't recorded (opened by mistake, or just sampling).
pub const MIN_RECORDED_SECS: f64 = 10.0;
/// Default fraction of a file that counts as watched (Plex uses the same default).
pub const DEFAULT_WATCHED_THRESHOLD: f64 = 0.9;
/// Setting holding the fraction that counts as watched.
pub const WATCHED_THRESHOLD_SETTING: &str = "watched_threshold";

/// The fraction of a file that counts as watched (Settings), as a number in 0–1.
pub fn watched_threshold(db: &crate::Db) -> f64 {
    db.setting(WATCHED_THRESHOLD_SETTING).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_WATCHED_THRESHOLD)
}

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

/// Continue Watching from organised libraries: one entry per show or movie. For a show,
/// the episode in progress, else the next unwatched episode after the last one played
/// (carrying on into later seasons); a finished show doesn't appear.
pub fn continue_watching_shows<'a>(
    libraries: impl IntoIterator<Item = (&'a Library, &'a Tree, &'a crate::catalog::Catalog)>,
    limit: usize,
) -> Vec<ContinueItem> {
    let mut candidates: Vec<(i64, ContinueItem)> = Vec::new();
    for (lib, tree, catalog) in libraries {
        for show in &catalog.shows {
            let progress = |id: i64| tree.file(id).map(|f| f.progress).unwrap_or_default();
            let Some((last_played, latest)) = show
                .episodes
                .iter()
                .filter_map(|e| progress(e.file_id).last_played.map(|t| (t, e)))
                .max_by_key(|(t, _)| *t)
            else {
                continue;
            };
            let item = |file_id, kind| ContinueItem { library_id: lib.id, file_id, kind };
            // Anything partway through, most recently played first.
            let in_progress = show
                .episodes
                .iter()
                .filter(|e| progress(e.file_id).position.is_some())
                .max_by_key(|e| progress(e.file_id).last_played);
            let chosen = match in_progress {
                Some(e) => Some(item(e.file_id, ContinueKind::Resume)),
                None if show.is_series() => {
                    let pos = show.episodes.iter().position(|e| e.file_id == latest.file_id).unwrap_or(0);
                    show.episodes[pos + 1..]
                        .iter()
                        .filter(|e| e.season != 0)
                        .find(|e| !progress(e.file_id).watched)
                        .map(|e| item(e.file_id, ContinueKind::Next))
                }
                None => None,
            };
            if let Some(c) = chosen {
                candidates.push((last_played, c));
            }
        }
    }
    candidates.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    candidates.into_iter().take(limit).map(|(_, c)| c).collect()
}

/// What a show or film page's main button should play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextUp {
    /// An episode (or the film) partway through.
    Resume(i64),
    /// The next unwatched episode.
    Next(i64),
    /// Everything or nothing watched: the first episode, from the start.
    Start(i64),
}

impl NextUp {
    pub fn file_id(self) -> i64 {
        match self {
            NextUp::Resume(id) | NextUp::Next(id) | NextUp::Start(id) => id,
        }
    }
}

/// Picks what to play next for a show or film: the most recently played episode in
/// progress, else the first unwatched episode after the last one played (or the first
/// unwatched overall), else the first episode. Specials are skipped unless that's all
/// there is.
pub fn next_up(show: &crate::catalog::ShowEntry, tree: &Tree) -> Option<NextUp> {
    let progress = |id: i64| tree.file(id).map(|f| f.progress).unwrap_or_default();
    let regular: Vec<_> = show.episodes.iter().filter(|e| e.season != 0).collect();
    let list: Vec<_> = if regular.is_empty() { show.episodes.iter().collect() } else { regular };
    if let Some(e) = list
        .iter()
        .filter(|e| progress(e.file_id).position.is_some())
        .max_by_key(|e| progress(e.file_id).last_played)
    {
        return Some(NextUp::Resume(e.file_id));
    }
    let unwatched = |e: &&&crate::catalog::Episode| !progress(e.file_id).watched;
    if let Some(latest) = list.iter().enumerate().filter(|(_, e)| progress(e.file_id).last_played.is_some()).max_by_key(|(_, e)| progress(e.file_id).last_played)
        && let Some(next) = list[latest.0 + 1..].iter().find(unwatched)
    {
        return Some(NextUp::Next(next.file_id));
    }
    let any_watched = list.iter().any(|e| progress(e.file_id).watched);
    match list.iter().find(unwatched) {
        Some(next) if any_watched => Some(NextUp::Next(next.file_id)),
        _ => list.first().map(|e| NextUp::Start(e.file_id)),
    }
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
    fn continue_watching_per_show() {
        use crate::catalog::{self, Catalog};
        use crate::db::{GroupMatch, Show};
        let lib = Library { id: 1, name: "Anime".into(), root: "/a".into(), kind: LibraryKind::Anime };
        let tree = Tree::new(vec![
            file(1, 1, "Show S1/Show - 01.mkv", played(10, None, true)),
            file(2, 1, "Show S1/Show - 02.mkv", played(20, None, true)), // end of season 1...
            file(3, 1, "Show S2/Show S2 - 01.mkv", Progress::default()), // ...so S2E1 is next
            file(4, 1, "Done/Done - 01.mkv", played(30, None, true)), // complete show: absent
        ]);
        let parsed = crate::parse::parse_episodes(tree.files());
        let groups = catalog::derive_groups(LibraryKind::Anime, &tree, &parsed);
        let show = |id, title: &str| Show { id, library_id: 1, title: title.into(), state: "local".into(), ..Default::default() };
        let m = |key: &str, show_id| GroupMatch { group_key: key.into(), show_id, season: None, manual: false };
        let matches = [m("Show S1", 1), m("Show S2", 1), m("Done", 2)];
        let cat = Catalog::build(&tree, &parsed, &groups, vec![show(1, "Show"), show(2, "Done")], &matches);
        let items = continue_watching_shows([(&lib, &tree, &cat)], 10);
        assert_eq!(items, [ContinueItem { library_id: 1, file_id: 3, kind: ContinueKind::Next }]);
        // Up next / next-episode stops at the end of a season.
        assert_eq!(cat.neighbour_in_season(1, 1), Some(2));
        assert_eq!(cat.neighbour_in_season(2, 1), None);
    }

    #[test]
    fn next_up_rules() {
        use crate::catalog::{self, Catalog};
        use crate::db::{GroupMatch, Show};
        let build = |progress: [Progress; 3]| {
            let tree = Tree::new(vec![
                file(1, 1, "S/S - 01.mkv", progress[0]),
                file(2, 1, "S/S - 02.mkv", progress[1]),
                file(3, 1, "S/S - 03.mkv", progress[2]),
            ]);
            let parsed = crate::parse::parse_episodes(tree.files());
            let groups = catalog::derive_groups(LibraryKind::Anime, &tree, &parsed);
            let show = Show { id: 1, library_id: 1, title: "S".into(), ..Default::default() };
            let m = GroupMatch { group_key: "S".into(), show_id: 1, season: None, manual: false };
            let cat = Catalog::build(&tree, &parsed, &groups, vec![show], &[m]);
            next_up(&cat.shows[0], &tree)
        };
        let none = Progress::default();
        let done = |t| played(t, None, true);
        assert_eq!(build([none, none, none]), Some(NextUp::Start(1)));
        assert_eq!(build([done(1), none, none]), Some(NextUp::Next(2)));
        assert_eq!(build([done(1), played(2, Some(30.0), false), none]), Some(NextUp::Resume(2)));
        assert_eq!(build([done(1), done(2), done(3)]), Some(NextUp::Start(1)));
        // Rewatched episode 3 last: the gap at 2 is offered next.
        assert_eq!(build([done(1), none, done(5)]), Some(NextUp::Next(2)));
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
