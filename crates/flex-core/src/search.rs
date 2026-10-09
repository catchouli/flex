//! Fuzzy search over shows, films, episodes and folder/file names, using nucleo-matcher
//! (the matcher from the Helix editor). Results can be limited to a kind or a library.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32String};

use crate::library::{Library, display_name};
use crate::tree::Tree;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A show or film (opens its page).
    Show { library_id: i64, show_id: i64 },
    /// An episode (opens its page).
    Episode { library_id: i64, file_id: i64 },
    Folder { library_id: i64, rel: String },
    File { library_id: i64, file_id: i64 },
}

impl Target {
    pub fn library_id(&self) -> i64 {
        match self {
            Target::Show { library_id, .. }
            | Target::Episode { library_id, .. }
            | Target::Folder { library_id, .. }
            | Target::File { library_id, .. } => *library_id,
        }
    }
}

/// Which results to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Shows and films in every library.
    Titles,
    /// Shows and films in one library.
    Library(i64),
    Episodes,
    /// Folder and file names.
    Paths,
}

impl Scope {
    fn includes(self, target: &Target) -> bool {
        match (self, target) {
            (Scope::Titles, Target::Show { .. }) => true,
            (Scope::Library(id), Target::Show { library_id, .. }) => *library_id == id,
            (Scope::Episodes, Target::Episode { .. }) => true,
            (Scope::Paths, Target::Folder { .. } | Target::File { .. }) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub title: String,
    /// Secondary text, e.g. "Anime · 2020" or "Anime › Show/Season 1".
    pub context: String,
    pub target: Target,
    /// Ranked above other matches of similar quality (e.g. top-level folders).
    pub boost: u32,
    /// What's matched: the title plus any alternative names.
    haystack: Utf32String,
}

impl Item {
    /// `also` are extra names to match on (e.g. original or folder titles).
    pub fn new(title: String, context: String, target: Target, boost: u32, also: &[String]) -> Self {
        let mut haystack = title.clone();
        for a in also {
            if !a.is_empty() && !haystack.to_lowercase().contains(&a.to_lowercase()) {
                haystack.push_str(" | ");
                haystack.push_str(a);
            }
        }
        Self { title, context, target, boost, haystack: haystack.as_str().into() }
    }
}

pub struct SearchIndex {
    items: Vec<Item>,
    matcher: Matcher,
}

/// Score bonus for top-level folders, so "dorohedoro" lists the show's folder before its files.
const TOP_LEVEL_BONUS: u32 = 40;
const FOLDER_BONUS: u32 = 10;
const MIN_SCORE_PER_CHAR: u32 = 22;

/// Folder and file items for a library's tree.
pub fn path_items(lib: &Library, tree: &Tree) -> Vec<Item> {
    let context_for = |parent: &str| if parent.is_empty() { lib.name.clone() } else { format!("{} › {parent}", lib.name) };
    let mut items = Vec::new();
    for rel in tree.dirs() {
        let (parent, name) = rel.rsplit_once('/').unwrap_or(("", &rel));
        let boost = if parent.is_empty() { TOP_LEVEL_BONUS } else { FOLDER_BONUS };
        let target = Target::Folder { library_id: lib.id, rel: rel.clone() };
        items.push(Item::new(display_name(name, false), context_for(parent), target, boost, &[]));
    }
    for f in tree.files() {
        let parent = f.rel_path.rsplit_once('/').map_or("", |(d, _)| d);
        let target = Target::File { library_id: lib.id, file_id: f.id };
        items.push(Item::new(display_name(f.file_name(), true), context_for(parent), target, 0, &[]));
    }
    items
}

impl SearchIndex {
    pub fn new(items: Vec<Item>) -> Self {
        Self { items, matcher: Matcher::new(Config::DEFAULT) }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Best matches in `scope` first. Each whitespace-separated word is matched fuzzily and
    /// all must match.
    pub fn search(&mut self, query: &str, scope: Scope, limit: usize) -> Vec<Item> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        // Letters matched one by one, scattered through a long title, score about 16 each;
        // real matches (whole words, word starts) about 27. Drop the scattered ones.
        let min_score = MIN_SCORE_PER_CHAR * query.chars().filter(|c| !c.is_whitespace()).count() as u32;
        let mut scored: Vec<(u32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| scope.includes(&item.target))
            .filter_map(|(i, item)| {
                let score = pattern.score(item.haystack.slice(..), &mut self.matcher)?;
                (score >= min_score).then_some((score + item.boost, i))
            })
            .collect();
        scored.sort_by(|(sa, ia), (sb, ib)| {
            sb.cmp(sa).then_with(|| self.items[*ia].title.len().cmp(&self.items[*ib].title.len()))
        });
        scored.into_iter().take(limit).map(|(_, i)| self.items[i].clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{LibraryKind, MediaFile};

    #[test]
    fn scopes_and_alternative_titles() {
        let lib = Library { id: 1, name: "Anime".into(), root: "/a".into(), kind: LibraryKind::Anime };
        let paths = [
            "(CBB) Dorohedoro (BD 1080p)/(CBB) Dorohedoro - 01.mkv",
            "Shingeki no Kyojin S1/Shingeki no Kyojin - 01.mkv",
        ];
        let tree = Tree::new(
            paths
                .iter()
                .enumerate()
                .map(|(i, p)| MediaFile { id: i as i64, library_id: 1, rel_path: (*p).into(), ..Default::default() })
                .collect(),
        );
        let mut items = path_items(&lib, &tree);
        let show = |id, title: &str, also: &[String]| {
            Item::new(title.into(), "Anime".into(), Target::Show { library_id: 1, show_id: id }, 0, also)
        };
        items.push(show(10, "Dorohedoro", &[]));
        items.push(show(11, "Attack on Titan", &["Shingeki no Kyojin".to_owned()]));
        items.push(Item::new("Dorohedoro · S1E1 · Episode 1".into(), String::new(), Target::Episode { library_id: 1, file_id: 0 }, 0, &[]));
        let mut index = SearchIndex::new(items);

        let titles = index.search("dorohed", Scope::Titles, 10);
        assert_eq!(titles.len(), 1, "only the show, not its folder or files");
        assert_eq!(index.search("shingeki", Scope::Titles, 10)[0].title, "Attack on Titan");
        assert_eq!(index.search("doro", Scope::Episodes, 10).len(), 1);
        assert!(index.search("doro", Scope::Library(2), 10).is_empty());
        assert!(index.search("dtn", Scope::Titles, 10).is_empty(), "scattered letters don't count");
        let paths = index.search("dorohed", Scope::Paths, 10);
        assert_eq!(paths.len(), 2);
        assert!(matches!(paths[0].target, Target::Folder { .. }), "folder ranks above its file");
    }
}
