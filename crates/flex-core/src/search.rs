//! Fuzzy search over folder and file names across all libraries, using nucleo-matcher
//! (the matcher from the Helix editor).

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32String};

use crate::library::{Library, display_name};
use crate::tree::Tree;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Folder { library_id: i64, rel: String },
    File { library_id: i64, file_id: i64 },
}

#[derive(Clone, Debug)]
pub struct Item {
    pub title: String,
    /// Where it lives, e.g. "Anime › Show/Season 1".
    pub context: String,
    pub target: Target,
    /// Folders directly inside a library root: usually a whole show or movie.
    pub top_level: bool,
    haystack: Utf32String,
}

pub struct SearchIndex {
    items: Vec<Item>,
    matcher: Matcher,
}

/// Score bonus for top-level folders, so "dorohedoro" lists the show before its episodes.
const TOP_LEVEL_BONUS: u32 = 40;
const FOLDER_BONUS: u32 = 10;

impl SearchIndex {
    pub fn build<'a>(libraries: impl IntoIterator<Item = (&'a Library, &'a Tree)>) -> Self {
        let mut items = Vec::new();
        for (lib, tree) in libraries {
            let context_for = |parent: &str| {
                if parent.is_empty() { lib.name.clone() } else { format!("{} › {parent}", lib.name) }
            };
            for rel in tree.dirs() {
                let (parent, name) = rel.rsplit_once('/').unwrap_or(("", &rel));
                let title = display_name(name, false);
                items.push(Item {
                    haystack: title.as_str().into(),
                    title,
                    context: context_for(parent),
                    top_level: parent.is_empty(),
                    target: Target::Folder { library_id: lib.id, rel: rel.clone() },
                });
            }
            for f in tree.files() {
                let parent = f.rel_path.rsplit_once('/').map_or("", |(d, _)| d);
                let title = display_name(f.file_name(), true);
                items.push(Item {
                    haystack: title.as_str().into(),
                    title,
                    context: context_for(parent),
                    top_level: false,
                    target: Target::File { library_id: lib.id, file_id: f.id },
                });
            }
        }
        Self { items, matcher: Matcher::new(Config::DEFAULT) }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Best matches first. Each whitespace-separated word is matched fuzzily and all must match.
    pub fn search(&mut self, query: &str, limit: usize) -> Vec<Item> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let mut scored: Vec<(u32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                let score = pattern.score(item.haystack.slice(..), &mut self.matcher)?;
                let bonus = match (&item.target, item.top_level) {
                    (_, true) => TOP_LEVEL_BONUS,
                    (Target::Folder { .. }, false) => FOLDER_BONUS,
                    _ => 0,
                };
                Some((score + bonus, i))
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
    fn ranks_shows_above_episodes() {
        let lib = Library { id: 1, name: "Anime".into(), root: "/a".into(), kind: LibraryKind::Anime };
        let paths = [
            "(CBB) Dorohedoro (BD 1080p)/(CBB) Dorohedoro - 01.mkv",
            "(CBB) Dorohedoro (BD 1080p)/(CBB) Dorohedoro - 02.mkv",
            "Vinland.Saga.S01/Vinland.Saga.S01E01.mkv",
        ];
        let tree = Tree::new(
            paths
                .iter()
                .enumerate()
                .map(|(i, p)| MediaFile { id: i as i64, library_id: 1, rel_path: (*p).into(), ..Default::default() })
                .collect(),
        );
        let mut index = SearchIndex::build([(&lib, &tree)]);

        let hits = index.search("dorohed", 10);
        assert_eq!(hits.len(), 3);
        assert!(hits[0].top_level, "show folder first, got {:?}", hits[0].title);

        let hits = index.search("vinland saga", 10);
        assert_eq!(hits[0].title, "Vinland Saga S01");

        assert!(index.search("zzzz", 10).is_empty());
    }
}
