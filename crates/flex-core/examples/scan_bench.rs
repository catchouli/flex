//! Times a full scan of a library folder, then a rescan that skips unchanged folders:
//! `cargo run --release -p flex-core --example scan_bench -- <folder> [music]`.

use std::sync::atomic::AtomicBool;
use std::time::Instant;

use flex_core::LibraryKind;
use flex_core::scan::{Previous, scan};

fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).expect("library folder"));
    let kind = if std::env::args().nth(2).as_deref() == Some("music") { LibraryKind::Music } else { LibraryKind::Movies };
    let t = Instant::now();
    let full = scan(&root, kind, &Previous::default(), &AtomicBool::new(false), |_| {}).unwrap();
    println!("full scan: {} files, {} folders, {} errors, {:?}", full.files.len(), full.dirs_read, full.errors.len(), t.elapsed());
    for e in &full.errors {
        println!("  {e}");
    }
    let previous = Previous { files: full.files.clone(), dirs: full.dirs.iter().cloned().collect() };
    let t = Instant::now();
    let again = scan(&root, kind, &previous, &AtomicBool::new(false), |_| {}).unwrap();
    println!("rescan: {} files, {} folders read, {:?}", again.files.len(), again.dirs_read, t.elapsed());
    assert_eq!(again.files, full.files);
}
