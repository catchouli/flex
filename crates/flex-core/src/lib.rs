//! UI-independent parts of Flex: library storage, scanning, folder trees and search.

pub mod ambient;
pub mod audio;
pub mod catalog;
pub mod db;
pub mod disc;
pub mod effects;
pub mod language;
pub mod library;
pub mod metadata;
pub mod music;
pub mod musicbrainz;
pub mod natural;
pub mod parse;
pub mod plex;
pub mod plugins;
pub mod progress;
pub mod scan;
pub mod search;
pub mod skip;
pub mod sync;
pub mod titles;
pub mod tree;
pub mod update;
pub mod youtube;

pub use db::Db;
pub use library::{Library, LibraryKind, MediaFile};
pub use tree::Tree;

/// Takes the single-instance lock. Returns the lock file (keep it open for the life of the
/// app), or `None` if another Flex already holds it.
pub fn single_instance_lock() -> std::io::Result<Option<std::fs::File>> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("flex.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// Where Flex keeps its database: `%APPDATA%\Flex\data` on Windows,
/// `~/Library/Application Support/Flex` on macOS.
pub fn data_dir() -> std::path::PathBuf {
    // For testing against a copy of the data.
    if let Some(dir) = std::env::var_os("FLEX_DATA_DIR") {
        return std::path::PathBuf::from(dir);
    }
    directories::ProjectDirs::from("", "", "Flex")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("flex-data"))
}

/// A program such as ffmpeg: next to Flex's program, else on the PATH (plus Homebrew's
/// folders, since apps started from Finder don't get the shell's PATH).
pub fn find_tool(name: &str) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(&exe)));
    let path_dirs = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    let extra = [PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")];
    beside.into_iter().chain(path_dirs.into_iter().map(|d| d.join(&exe))).chain(extra.iter().map(|d| d.join(&exe))).find(|p| p.is_file())
}
