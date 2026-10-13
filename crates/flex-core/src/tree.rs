//! In-memory folder view of a library's files, built from the database.

use std::collections::BTreeMap;

use crate::library::MediaFile;
use crate::natural::natural_cmp;

#[derive(Clone)]
pub struct Tree {
    files: Vec<MediaFile>,
}

#[derive(Clone, Debug)]
pub enum Node {
    Dir {
        name: String,
        /// Relative path of the folder, '/'-separated.
        rel: String,
        /// Number of videos anywhere below it.
        videos: usize,
        /// How many of those aren't watched yet.
        unwatched: usize,
    },
    File(MediaFile),
}

fn parent_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(dir, _)| dir)
}

impl Tree {
    pub fn new(mut files: Vec<MediaFile>) -> Self {
        files.sort_by(|a, b| natural_cmp(&a.rel_path, &b.rel_path));
        Self { files }
    }

    pub fn files(&self) -> &[MediaFile] {
        &self.files
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn file(&self, id: i64) -> Option<&MediaFile> {
        self.files.iter().find(|f| f.id == id)
    }

    pub fn file_mut(&mut self, id: i64) -> Option<&mut MediaFile> {
        self.files.iter_mut().find(|f| f.id == id)
    }

    /// Ids of every file below folder `dir` ("" = everything).
    pub fn ids_under(&self, dir: &str) -> Vec<i64> {
        let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        self.files.iter().filter(|f| f.rel_path.starts_with(&prefix)).map(|f| f.id).collect()
    }

    /// Immediate children of folder `dir` ("" for the root): folders first, then files.
    pub fn children(&self, dir: &str) -> Vec<Node> {
        let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        let mut dirs: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        let mut files = Vec::new();
        for f in &self.files {
            let Some(rest) = f.rel_path.strip_prefix(&prefix) else { continue };
            match rest.split_once('/') {
                Some((sub, _)) => {
                    let counts = dirs.entry(sub).or_default();
                    counts.0 += 1;
                    counts.1 += usize::from(!f.progress.watched);
                }
                None => files.push(Node::File(f.clone())),
            }
        }
        let mut out: Vec<Node> = dirs
            .into_iter()
            .map(|(name, (videos, unwatched))| Node::Dir {
                name: name.to_owned(),
                rel: format!("{prefix}{name}"),
                videos,
                unwatched,
            })
            .collect();
        out.sort_by(|a, b| match (a, b) {
            (Node::Dir { name: x, .. }, Node::Dir { name: y, .. }) => natural_cmp(x, y),
            _ => std::cmp::Ordering::Equal,
        });
        out.extend(files); // already in natural order
        out
    }

    /// The only video below `dir`, if there is exactly one (e.g. a movie in its own folder).
    pub fn single_video_in(&self, dir: &str) -> Option<&MediaFile> {
        let prefix = format!("{dir}/");
        let mut it = self.files.iter().filter(|f| f.rel_path.starts_with(&prefix));
        let first = it.next()?;
        it.next().is_none().then_some(first)
    }

    /// The file after `id` in the same folder, in natural order.
    pub fn next_in_folder(&self, id: i64) -> Option<&MediaFile> {
        let pos = self.files.iter().position(|f| f.id == id)?;
        let dir = parent_of(&self.files[pos].rel_path);
        // Files sort by full path, so the next file in the same folder is the next entry
        // with the same parent (subfolders of `dir` may sit in between).
        self.files[pos + 1..].iter().find(|f| parent_of(&f.rel_path) == dir)
    }

    /// The first unwatched file after `id` in the same folder.
    pub fn next_unwatched_in_folder(&self, id: i64) -> Option<&MediaFile> {
        let pos = self.files.iter().position(|f| f.id == id)?;
        let dir = parent_of(&self.files[pos].rel_path);
        self.files[pos + 1..].iter().find(|f| parent_of(&f.rel_path) == dir && !f.progress.watched)
    }

    /// The file before `id` in the same folder, in natural order.
    pub fn prev_in_folder(&self, id: i64) -> Option<&MediaFile> {
        let pos = self.files.iter().position(|f| f.id == id)?;
        let dir = parent_of(&self.files[pos].rel_path);
        self.files[..pos].iter().rev().find(|f| parent_of(&f.rel_path) == dir)
    }

    /// Every distinct folder path, for search.
    pub fn dirs(&self) -> Vec<String> {
        let mut dirs: Vec<String> = Vec::new();
        for f in &self.files {
            let mut dir = parent_of(&f.rel_path);
            while !dir.is_empty() {
                dirs.push(dir.to_owned());
                dir = parent_of(dir);
            }
        }
        dirs.sort();
        dirs.dedup();
        dirs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(paths: &[&str]) -> Tree {
        Tree::new(
            paths
                .iter()
                .enumerate()
                .map(|(i, p)| MediaFile { id: i as i64, library_id: 1, rel_path: (*p).into(), ..Default::default() })
                .collect(),
        )
    }

    #[test]
    fn children_and_next() {
        let t = tree(&["Show/S1/ep10.mkv", "Show/S1/ep2.mkv", "Show/S1/Extras/x.mkv", "Show/S2/ep1.mkv", "movie.mkv"]);
        let root: Vec<_> = t
            .children("")
            .into_iter()
            .map(|n| match n {
                Node::Dir { name, videos, unwatched, .. } => format!("{name}/{videos}/{unwatched}"),
                Node::File(f) => f.rel_path,
            })
            .collect();
        assert_eq!(root, ["Show/4/4", "movie.mkv"]);
        assert_eq!(t.ids_under("Show/S1").len(), 3);

        let ep2 = t.files().iter().find(|f| f.rel_path.ends_with("ep2.mkv")).unwrap().id;
        assert_eq!(t.next_in_folder(ep2).unwrap().rel_path, "Show/S1/ep10.mkv");
        let ep10 = t.next_in_folder(ep2).unwrap().id;
        assert!(t.next_in_folder(ep10).is_none(), "end of season must not roll into S2");
        assert_eq!(t.prev_in_folder(ep10).unwrap().id, ep2);
        assert!(t.prev_in_folder(ep2).is_none());

        assert!(t.single_video_in("Show/S2").is_some());
        assert!(t.single_video_in("Show/S1").is_none());
        assert_eq!(t.dirs(), ["Show", "Show/S1", "Show/S1/Extras", "Show/S2"]);
    }
}
