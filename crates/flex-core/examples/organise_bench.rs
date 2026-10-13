//! Times parsing (uncached and cached) + grouping a library. Writes the parse cache. Usage: organise_bench <db> <library id> <anime|movies>
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut db = flex_core::Db::open(std::path::Path::new(&args[1]))?;
    let id = args[2].parse()?;
    let tree = flex_core::Tree::new(db.files(id)?);
    let kind = flex_core::LibraryKind::parse(&args[3]).unwrap();
    let t = std::time::Instant::now();
    let parsed = flex_core::parse::parse_library(kind, tree.files());
    println!("parse: {:?}", t.elapsed());
    // Twice: the first run fills the cache (unless it's already warm), the second uses it.
    for _ in 0..2 {
        let t = std::time::Instant::now();
        let cached = flex_core::parse::parse_library_cached(&mut db, id, kind, tree.files())?;
        println!("parse (cached): {:?}, same as uncached: {}", t.elapsed(), cached == parsed);
    }
    let t = std::time::Instant::now();
    let groups = flex_core::catalog::derive_groups(kind, &tree, &parsed);
    let shows: std::collections::HashSet<&str> = groups.iter().map(|g| g.norm.as_str()).collect();
    println!("groups: {} groups, {} distinct titles in {:?}", groups.len(), shows.len(), t.elapsed());
    let mut by_norm: std::collections::HashMap<&str, Vec<&str>> = Default::default();
    for g in &groups {
        by_norm.entry(&g.norm).or_default().push(&g.key);
    }
    let mut merged: Vec<_> = by_norm.into_iter().filter(|(_, v)| v.len() > 1).collect();
    merged.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (norm, keys) in merged.iter().take(12) {
        let sample: Vec<String> = keys.iter().take(3).map(|k| k.chars().take(70).collect()).collect();
        println!("{:>4} x {norm:?}: {}", keys.len(), sample.join(" | "));
    }
    Ok(())
}
