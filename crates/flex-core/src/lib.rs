//! UI-independent parts of Flex: library storage, scanning, folder trees and search.

pub mod db;
pub mod library;
pub mod natural;
pub mod progress;
pub mod scan;
pub mod search;
pub mod tree;

pub use db::Db;
pub use library::{Library, LibraryKind, MediaFile};
pub use tree::Tree;

/// Where Flex keeps its database: `%APPDATA%\Flex\data` on Windows,
/// `~/Library/Application Support/Flex` on macOS.
pub fn data_dir() -> std::path::PathBuf {
    directories::ProjectDirs::from("", "", "Flex")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("flex-data"))
}
