//! UI-independent parts of Flex: library storage, scanning, folder trees and search.

pub mod catalog;
pub mod db;
pub mod disc;
pub mod library;
pub mod metadata;
pub mod music;
pub mod musicbrainz;
pub mod natural;
pub mod parse;
pub mod plex;
pub mod progress;
pub mod scan;
pub mod search;
pub mod titles;
pub mod tree;
pub mod update;

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
