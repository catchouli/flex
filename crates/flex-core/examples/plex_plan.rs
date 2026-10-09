//! Dry run of a Plex import. Usage: plex_plan <flex db> <plex db>
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let db = flex_core::Db::open(std::path::Path::new(&args[1]))?;
    let t = std::time::Instant::now();
    let data = flex_core::plex::read(std::path::Path::new(&args[2]))?;
    println!("account {:?}: {} items, {} views ({} orphaned) read in {:?}", data.account, data.items.len(), data.views.len(), data.orphan_views, t.elapsed());
    let libs: Vec<(i64, flex_core::Tree)> =
        db.libraries()?.into_iter().map(|l| Ok((l.id, flex_core::Tree::new(db.files(l.id)?)))).collect::<anyhow::Result<_>>()?;
    let refs: Vec<(i64, &flex_core::Tree)> = libs.iter().map(|(id, t)| (*id, t)).collect();
    let plan = flex_core::plex::plan(&data, &refs);
    for s in &plan.sections {
        println!("  {:<32} -> library {:?}: {}/{} matched", s.root, s.library_id, s.matched, s.total);
    }
    println!("progress {} (unmatched {}), history {} (unmatched {})", plan.progress.len(), plan.unmatched_items, plan.history.len(), plan.unmatched_views);
    Ok(())
}
