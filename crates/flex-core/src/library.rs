use std::path::{Path, PathBuf};

/// What a library contains; decides how its files are matched to metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryKind {
    Movies,
    Tv,
    Anime,
}

impl LibraryKind {
    pub const ALL: [Self; 3] = [Self::Movies, Self::Tv, Self::Anime];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Movies => "movies",
            Self::Tv => "tv",
            Self::Anime => "anime",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Movies => "Movies",
            Self::Tv => "TV",
            Self::Anime => "Anime",
        }
    }

    /// Whether files play in sequence (offering "next episode").
    pub fn is_episodic(self) -> bool {
        self != Self::Movies
    }
}

#[derive(Clone, Debug)]
pub struct Library {
    pub id: i64,
    pub name: String,
    pub root: PathBuf,
    pub kind: LibraryKind,
}

#[derive(Clone, Debug, Default)]
pub struct MediaFile {
    pub id: i64,
    pub library_id: i64,
    /// Path relative to the library root, always '/'-separated so it's the same on every OS.
    pub rel_path: String,
    pub size: u64,
    pub mtime: i64,
    /// When Flex first saw the file (Unix seconds).
    pub added_at: i64,
    pub progress: crate::progress::Progress,
}

impl MediaFile {
    pub fn file_name(&self) -> &str {
        self.rel_path.rsplit('/').next().unwrap_or(&self.rel_path)
    }

    pub fn abs_path(&self, root: &Path) -> PathBuf {
        join_rel(root, &self.rel_path)
    }
}

/// Joins a '/'-separated relative path onto `root` using the OS separator.
pub fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    p.extend(rel.split('/').filter(|c| !c.is_empty()));
    p
}

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "wmv", "webm", "ts", "m2ts", "mts", "mpg", "mpeg", "vob", "ogv",
    "flv", "3gp", "rmvb", "divx", "iso",
];

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.iter().any(|v| v.eq_ignore_ascii_case(e)))
}

/// Turns scene-style names ("2.Fast.2.Furious.2003.mkv") into something readable
/// ("2 Fast 2 Furious 2003"). Names that already contain spaces are left alone.
pub fn display_name(name: &str, strip_extension: bool) -> String {
    let stem = if strip_extension {
        Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name)
    } else {
        name
    };
    if stem.contains(' ') {
        stem.to_owned()
    } else {
        stem.replace(['.', '_'], " ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names() {
        assert_eq!(display_name("2.Fast.2.Furious.2003.mkv", true), "2 Fast 2 Furious 2003");
        assert_eq!(display_name("(CBB) Dorohedoro - 01.mkv", true), "(CBB) Dorohedoro - 01");
        assert_eq!(display_name("Some_Folder", false), "Some Folder");
    }

    #[test]
    fn join_rel_uses_components() {
        let p = join_rel(Path::new("root"), "a/b/c.mkv");
        assert_eq!(p, Path::new("root").join("a").join("b").join("c.mkv"));
    }
}
