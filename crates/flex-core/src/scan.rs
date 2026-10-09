//! Walks a library folder and collects its video files.

use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::{Result, bail};
use walkdir::{DirEntry, WalkDir};

use crate::library::is_video;

#[derive(Clone, Debug)]
pub struct ScannedFile {
    pub rel_path: String,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub files: Vec<ScannedFile>,
    /// Entries that couldn't be read (permissions, unreadable names, vanished files).
    pub errors: Vec<String>,
}

/// Folders NAS software litters around that never contain real media.
fn is_ignored(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.')
        || name.starts_with('$')
        || name.starts_with('@') // Synology @eaDir etc.
        || name.eq_ignore_ascii_case("#recycle")
        || name.eq_ignore_ascii_case("System Volume Information")
}

/// Scans `root` recursively. `progress` is called periodically with the number of videos found.
///
/// Fails if `root` isn't an accessible folder, so an unmounted share is never mistaken for
/// an empty library.
pub fn scan(root: &Path, mut progress: impl FnMut(usize)) -> Result<ScanResult> {
    if !root.is_dir() {
        bail!("{} isn't available. Is the drive or share connected?", root.display());
    }
    let mut result = ScanResult::default();
    let walker = WalkDir::new(root).follow_links(true).into_iter().filter_entry(|e| e.depth() == 0 || !is_ignored(e));
    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                result.errors.push(e.to_string());
                continue;
            }
        };
        if !entry.file_type().is_file() || !is_video(entry.path()) {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                result.errors.push(format!("{}: {e}", entry.path().display()));
                continue;
            }
        };
        let Ok(rel) = entry.path().strip_prefix(root) else { continue };
        let rel_path = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        result.files.push(ScannedFile { rel_path, size: meta.len(), mtime });
        if result.files.len() % 50 == 0 {
            progress(result.files.len());
        }
    }
    progress(result.files.len());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_videos_and_skips_junk() {
        let dir = std::env::temp_dir().join(format!("flex-scan-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for p in ["Show/Season 1/ep1.mkv", "Show/Season 1/ep1.nfo", ".hidden/x.mkv", "@eaDir/y.mp4", "movie.MP4"] {
            let path = dir.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        let mut found: Vec<_> = scan(&dir, |_| {}).unwrap().files.into_iter().map(|f| f.rel_path).collect();
        found.sort();
        assert_eq!(found, ["Show/Season 1/ep1.mkv", "movie.MP4"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_root_is_an_error() {
        assert!(scan(Path::new("/definitely/not/here"), |_| {}).is_err());
    }
}
