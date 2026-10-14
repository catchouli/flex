//! Reads a music folder into albums with a scratch database: `cargo run -p flex-core --release --example music_try -- <folder> <scratch dir>`.
use std::sync::atomic::AtomicBool;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (root, scratch) = (std::path::Path::new(&args[1]), std::path::Path::new(&args[2]));
    let started = std::time::Instant::now();
    let scan = flex_core::scan::scan(root, flex_core::LibraryKind::Music, &AtomicBool::new(false), |_| {})?;
    println!("scan: {} files in {:?}", scan.files.len(), started.elapsed());
    let mut db = flex_core::Db::open(&scratch.join("music.db"))?;
    let id = match db.libraries()?.first() {
        Some(l) => l.id,
        None => db.add_library("music", root, flex_core::LibraryKind::Music)?,
    };
    db.apply_scan(id, &scan.files)?;
    let files = db.files(id)?;
    let t = std::time::Instant::now();
    let folders = flex_core::music::read_library_cached(&mut db, id, root, &files, &scratch.join("covers"), |_, _| {})?;
    println!("tags: {:?}", t.elapsed());
    let catalog = flex_core::music::build(&files, &folders);
    let covers = catalog.albums.iter().filter(|a| a.cover.is_some()).count();
    println!("{} albums ({} with covers), {} artists, {} tracks", catalog.albums.len(), covers, catalog.artists.len(), catalog.tracks);
    for a in catalog.albums.iter().step_by((catalog.albums.len() / 25).max(1)) {
        println!("  {} — {} ({:?}) {} tracks, artists {:?}", a.artist, a.title, a.year, a.tracks.len(), &a.artists[..a.artists.len().min(4)]);
    }
    Ok(())
}
