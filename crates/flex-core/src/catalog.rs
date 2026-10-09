//! Organises a library's files into shows (or movies), seasons and episodes.
//!
//! Files are first split into *groups*: for TV/anime, the folder that directly holds the
//! videos; for movies, a single-movie folder or a single file. Each group is assigned to a
//! show, and that assignment is stored (see [`crate::db::GroupMatch`]) so later scans keep
//! it and manual fixes always win. Groups whose titles normalise the same share a show, so
//! "Dorohedoro" season 1 and season 2 from different release groups end up together.

use std::collections::{HashMap, HashSet};

use crate::library::LibraryKind;
use crate::natural::natural_cmp;
use crate::parse::{self, Parsed};
use crate::tree::Tree;

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// Stable identity: a folder path, a file path (movies), or `root:<title>` for loose
    /// episodes at the library root.
    pub key: String,
    pub title: String,
    pub norm: String,
    pub year: Option<i32>,
    /// Season named by the files or folder, if any.
    pub season: Option<i32>,
    /// For a DVD/Blu-ray folder, the files are disc chunks: only the first (largest) is
    /// listed, and it plays the whole disc.
    pub disc: bool,
    pub file_ids: Vec<i64>,
}

fn parent_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(d, _)| d)
}

fn last_component(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// Most common value, ties broken by the smallest.
fn mode<T: Copy + Ord + std::hash::Hash>(values: impl IntoIterator<Item = T>) -> Option<T> {
    let mut counts: HashMap<T, usize> = HashMap::new();
    for v in values {
        *counts.entry(v).or_default() += 1;
    }
    counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(v, _)| v)
}

fn title_from_dir(dir: &str) -> Option<(String, Option<i32>)> {
    let top = dir.split('/').next()?;
    let r = hunch::hunch(top);
    let mut title = r.title()?.to_owned();
    // "Mai-HiME" isn't the film "Mai" by release group "HiME": a scene-style "-GROUP"
    // suffix with nothing else around it is part of a folder's title.
    if let Some(group) = r.release_group()
        && parse::normalize(top) == parse::normalize(&format!("{title} {group}"))
    {
        title = format!("{title}-{group}");
    }
    let title = parse::clean_title(&title);
    (!title.is_empty()).then_some((title, r.year()))
}

/// Splits a library into groups. `parsed` comes from [`parse::parse_episodes`] or
/// [`parse::parse_movies`].
pub fn derive_groups(kind: LibraryKind, tree: &Tree, parsed: &HashMap<i64, Parsed>) -> Vec<Group> {
    // DVD/Blu-ray folder structures are one item each; everything else goes by folder.
    let mut discs: HashMap<&str, Vec<i64>> = HashMap::new();
    let mut by_dir: HashMap<&str, Vec<i64>> = HashMap::new();
    for f in tree.files() {
        match parse::disc_folder(&f.rel_path) {
            Some(disc) => discs.entry(disc).or_default().push(f.id),
            None => by_dir.entry(parent_of(&f.rel_path)).or_default().push(f.id),
        }
    }
    let file_title = |id: &i64| parsed.get(id).and_then(|p| p.title.clone());
    let mut groups = Vec::new();
    let mut push = |key: String, title: String, year: Option<i32>, season: Option<i32>, disc: bool, file_ids: Vec<i64>| {
        // Films are told apart by year too (remakes share titles); seasons of a show aren't.
        let norm = match (season, year) {
            (None, Some(y)) if !kind.is_episodic() => format!("{} {y}", parse::normalize(&title)),
            _ => parse::normalize(&title),
        };
        groups.push(Group { key, title, norm, year, season, disc, file_ids });
    };

    for (disc, mut ids) in discs {
        // Largest chunk first: it's the one listed (and the disc plays from the folder).
        ids.sort_by_key(|id| std::cmp::Reverse(tree.file(*id).map_or(0, |f| f.size)));
        let movie_dir = parent_of(disc);
        let named = if movie_dir.is_empty() { disc } else { movie_dir };
        let (title, year) = title_from_dir(named).unwrap_or_else(|| (last_component(named).to_owned(), None));
        push(disc.to_owned(), title, year, None, true, ids);
    }

    for (dir, ids) in by_dir {
        // Series can turn up in a movies library too (e.g. a complete-series folder), and
        // an extras folder inside one belongs to that show as specials.
        let extras = parse::is_specials_dir(last_component(dir));
        if kind.is_episodic() || extras || parse::looks_episodic(&ids, parsed) {
            let seasons = ids.iter().filter_map(|id| parsed.get(id).and_then(|p| p.season));
            // A group of only specials is season 0; otherwise ignore stray extras.
            let season = if extras { Some(0) } else { mode(seasons.clone().filter(|s| *s != 0)).or_else(|| mode(seasons)) };
            if dir.is_empty() {
                // Loose episodes at the root: one group per title.
                let mut by_title: HashMap<String, (String, Vec<i64>)> = HashMap::new();
                for id in ids {
                    let title = file_title(&id).unwrap_or_else(|| "Unknown".into());
                    by_title.entry(parse::normalize(&title)).or_insert_with(|| (title, Vec::new())).1.push(id);
                }
                for (norm, (title, ids)) in by_title {
                    push(format!("root:{norm}"), title, None, season, false, ids);
                }
            } else {
                let (title, year) = title_from_dir(dir)
                    .or_else(|| mode(ids.iter().filter_map(file_title).collect::<Vec<_>>().iter()).map(|t| (t.clone(), None)))
                    .unwrap_or_else(|| (last_component(dir).to_owned(), None));
                push(dir.to_owned(), title, year, season, false, ids);
            }
        } else {
            // Movies: a folder with a few files of the same film is one movie, otherwise each
            // file stands alone (e.g. a collection folder).
            let titles: HashSet<String> = ids.iter().filter_map(|id| file_title(id).map(|t| parse::normalize(&t))).collect();
            let one_movie = !dir.is_empty() && ids.len() <= 3 && titles.len() <= 1;
            if one_movie {
                let year = ids.iter().find_map(|id| parsed.get(id).and_then(|p| p.year));
                let (title, dir_year) = title_from_dir(last_component(dir))
                    .or_else(|| ids.first().and_then(file_title).map(|t| (t, None)))
                    .unwrap_or_else(|| (last_component(dir).to_owned(), None));
                push(dir.to_owned(), title, year.or(dir_year), None, false, ids);
            } else {
                for id in ids {
                    let Some(file) = tree.file(id) else { continue };
                    let p = parsed.get(&id).cloned().unwrap_or_default();
                    let title = p.title.unwrap_or_else(|| crate::library::display_name(file.file_name(), true));
                    push(file.rel_path.clone(), title, p.year, None, false, vec![id]);
                }
            }
        }
    }
    groups.sort_by(|a, b| natural_cmp(&a.key, &b.key));
    groups
}

/// Where each group's files go: show and season.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub show_id: i64,
    pub season: i32,
}

/// Seasons for one show's groups: a manual or named season wins; the rest are numbered in
/// folder order using the lowest numbers still free (so two un-numbered sagas become
/// seasons 1 and 2 instead of being merged).
pub fn assign_seasons(groups: &[(&Group, Option<i32>)]) -> Vec<i32> {
    let mut taken: HashSet<i32> = groups.iter().filter_map(|(g, manual)| manual.or(g.season)).collect();
    let mut next = 1;
    groups
        .iter()
        .map(|(g, manual)| {
            manual.or(g.season).unwrap_or_else(|| {
                while taken.contains(&next) {
                    next += 1;
                }
                taken.insert(next);
                next
            })
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct Episode {
    pub file_id: i64,
    pub season: i32,
    pub number: Option<i32>,
}

/// Episodes of a set of groups, by season then episode number then name.
pub fn episodes(tree: &Tree, parsed: &HashMap<i64, Parsed>, groups: &[(&Group, i32)]) -> Vec<Episode> {
    let mut out: Vec<(Episode, &str)> = Vec::new();
    for (group, season) in groups {
        let ids = if group.disc { &group.file_ids[..1.min(group.file_ids.len())] } else { &group.file_ids[..] };
        for id in ids {
            let Some(file) = tree.file(*id) else { continue };
            let p = parsed.get(id);
            // Extras inside a regular season folder stay specials.
            let season = if p.and_then(|p| p.season) == Some(0) { 0 } else { *season };
            out.push((Episode { file_id: *id, season, number: p.and_then(|p| p.episode) }, file.file_name()));
        }
    }
    out.sort_by(|(a, an), (b, bn)| {
        let key = |e: &Episode| (if e.season == 0 { i32::MAX } else { e.season }, e.number.unwrap_or(i32::MAX));
        key(a).cmp(&key(b)).then_with(|| natural_cmp(an, bn))
    });
    out.into_iter().map(|(e, _)| e).collect()
}

/// One folder (group) as placed in a show.
#[derive(Clone, Debug)]
pub struct PlacedGroup {
    pub key: String,
    /// Title parsed from the folder/file names.
    pub title: String,
    pub season: i32,
    pub manual: bool,
    pub files: usize,
}

#[derive(Clone, Debug)]
pub struct ShowEntry {
    pub show: crate::db::Show,
    pub groups: Vec<PlacedGroup>,
    /// In watching order: seasons ascending (specials last), then episode number.
    pub episodes: Vec<Episode>,
}

impl ShowEntry {
    /// A series (with numbered episodes) rather than a film, even in a movies library.
    pub fn is_series(&self) -> bool {
        self.episodes.len() > 1 && self.episodes.iter().any(|e| e.number.is_some())
    }

    /// Season numbers present, in order (specials, 0, last).
    pub fn seasons(&self) -> Vec<i32> {
        let mut seasons: Vec<i32> = self.episodes.iter().map(|e| e.season).collect();
        seasons.dedup();
        seasons
    }

    pub fn season_episodes(&self, season: i32) -> impl Iterator<Item = &Episode> {
        self.episodes.iter().filter(move |e| e.season == season)
    }
}

/// A library organised into shows (or movies).
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub shows: Vec<ShowEntry>,
    /// file id -> index into `shows`
    file_show: HashMap<i64, usize>,
}

impl Catalog {
    pub fn build(
        tree: &Tree,
        parsed: &HashMap<i64, Parsed>,
        groups: &[Group],
        shows: Vec<crate::db::Show>,
        matches: &[crate::db::GroupMatch],
    ) -> Self {
        let by_key: HashMap<&str, &crate::db::GroupMatch> = matches.iter().map(|m| (m.group_key.as_str(), m)).collect();
        let mut per_show: HashMap<i64, Vec<(&Group, &crate::db::GroupMatch)>> = HashMap::new();
        for g in groups {
            if let Some(m) = by_key.get(g.key.as_str()) {
                per_show.entry(m.show_id).or_default().push((g, m));
            }
        }
        let mut entries = Vec::new();
        for show in shows {
            let Some(placed) = per_show.remove(&show.id) else { continue };
            let seasons = assign_seasons(&placed.iter().map(|(g, m)| (*g, m.season)).collect::<Vec<_>>());
            let with_seasons: Vec<(&Group, i32)> = placed.iter().map(|(g, _)| *g).zip(seasons.iter().copied()).collect();
            let episodes = episodes(tree, parsed, &with_seasons);
            let groups = placed
                .iter()
                .zip(&seasons)
                .map(|((g, m), season)| PlacedGroup {
                    key: g.key.clone(),
                    title: g.title.clone(),
                    season: *season,
                    manual: m.manual,
                    files: if g.disc { 1 } else { g.file_ids.len() },
                })
                .collect();
            entries.push(ShowEntry { show, groups, episodes });
        }
        entries.sort_by(|a, b| natural_cmp(&a.show.title, &b.show.title));
        let file_show = entries
            .iter()
            .enumerate()
            .flat_map(|(i, e)| e.episodes.iter().map(move |ep| (ep.file_id, i)))
            .collect();
        Self { shows: entries, file_show }
    }

    pub fn show(&self, show_id: i64) -> Option<&ShowEntry> {
        self.shows.iter().find(|s| s.show.id == show_id)
    }

    pub fn show_of_file(&self, file_id: i64) -> Option<&ShowEntry> {
        self.file_show.get(&file_id).map(|i| &self.shows[*i])
    }

    /// The episode `delta` places from `file_id` within the same season (for next/previous
    /// and "Up next", which stop at the end of a season).
    pub fn neighbour_in_season(&self, file_id: i64, delta: isize) -> Option<i64> {
        let show = self.show_of_file(file_id)?;
        let season = show.episodes.iter().find(|e| e.file_id == file_id)?.season;
        let eps: Vec<&Episode> = show.season_episodes(season).collect();
        let pos = eps.iter().position(|e| e.file_id == file_id)?;
        let target = pos.checked_add_signed(delta)?;
        eps.get(target).map(|e| e.file_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::MediaFile;

    fn tree(paths: &[&str]) -> Tree {
        Tree::new(
            paths
                .iter()
                .enumerate()
                .map(|(i, p)| MediaFile { id: i as i64, rel_path: (*p).into(), ..Default::default() })
                .collect(),
        )
    }

    #[test]
    fn two_release_groups_become_one_show_with_two_seasons() {
        let t = tree(&[
            "(CBB) Dorohedoro (BD 1080p)(Dual-Audio)(HEVC-10bit-x265)/(CBB) Dorohedoro - 01 (BD 1080p)(Dual-Audio).mkv",
            "(CBB) Dorohedoro (BD 1080p)(Dual-Audio)(HEVC-10bit-x265)/(CBB) Dorohedoro - 02 (BD 1080p)(Dual-Audio).mkv",
            "[Ironclad] Dorohedoro - S02 [WEB.1080p.AV1]/[Ironclad] Dorohedoro 2 - S02E01 [WEB.1080p.AV1].mkv",
            "[Ironclad] Dorohedoro - S02 [WEB.1080p.AV1]/[Ironclad] Dorohedoro 2 - S02E02 [WEB.1080p.AV1].mkv",
        ]);
        let parsed = parse::parse_episodes(t.files());
        let groups = derive_groups(LibraryKind::Anime, &t, &parsed);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.norm == "dorohedoro"), "{groups:?}");
        let seasons = assign_seasons(&groups.iter().map(|g| (g, None)).collect::<Vec<_>>());
        assert_eq!(seasons, [1, 2]);
    }

    #[test]
    fn hyphenated_folder_titles_survive() {
        assert_eq!(title_from_dir("Mai-HiME").map(|t| t.0).as_deref(), Some("Mai-HiME"));
        assert_eq!(title_from_dir("Rasen.1998.1080p.Bluray-GROUP").map(|t| t.0).as_deref(), Some("Rasen"));
    }

    #[test]
    fn unnumbered_sagas_get_separate_seasons() {
        let t = tree(&[
            "Grappler Baki/1. Grappler Baki - Kid Saga [2001]/Grappler Baki - Episode 01.mkv",
            "Grappler Baki/2. Grappler Baki - Maximum Tournament [2001]/Grappler Baki - Episode 01.mkv",
        ]);
        let parsed = parse::parse_episodes(t.files());
        let groups = derive_groups(LibraryKind::Anime, &t, &parsed);
        assert_eq!(groups.iter().map(|g| g.title.as_str()).collect::<Vec<_>>(), ["Grappler Baki", "Grappler Baki"]);
        let seasons = assign_seasons(&groups.iter().map(|g| (g, None)).collect::<Vec<_>>());
        assert_eq!(seasons, [1, 2]);
        // A manual season is respected and the others fill around it.
        let seasons = assign_seasons(&[(&groups[0], Some(2)), (&groups[1], None)]);
        assert_eq!(seasons, [2, 1]);
    }

    #[test]
    fn movies_folder_vs_collection() {
        let t = tree(&[
            "Rasen.1998.1080p.Bluray.JPN.AVC/Rasen.1998.1080p.mkv",
            "Ghibli Collection/Spirited Away (2001).mkv",
            "Ghibli Collection/My Neighbor Totoro (1988).mkv",
            "Ghibli Collection/Porco Rosso (1992).mkv",
            "Ghibli Collection/Kiki's Delivery Service (1989).mkv",
            "Christmas.Vacation.1989.1080p.mkv",
        ]);
        let parsed = parse::parse_movies(t.files());
        let groups = derive_groups(LibraryKind::Movies, &t, &parsed);
        let summary: Vec<_> = groups.iter().map(|g| (g.title.as_str(), g.year)).collect();
        assert!(summary.contains(&("Rasen", Some(1998))));
        assert!(summary.contains(&("Spirited Away", Some(2001))));
        assert!(summary.contains(&("Christmas Vacation", Some(1989))));
        assert_eq!(groups.len(), 6);
    }
}
