//! Prints how hunch parses a sample of a library's folders and files.
//! Usage: cargo run --example parse_eval -- <db path> <library id> [every nth folder]

use std::collections::BTreeMap;

use flex_core::Db;

fn show(label: &str, r: &hunch::HunchResult) {
    println!(
        "  {label:<6} title={:?} year={:?} S={:?} E={:?} type={:?}",
        r.title(),
        r.year(),
        r.season(),
        r.episode(),
        r.media_type()
    );
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let db = Db::open(std::path::Path::new(&args[1]))?;
    let lib: i64 = args[2].parse()?;
    let every: usize = args.get(3).map_or(Ok(25), |s| s.parse())?;

    let mut by_top: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in db.files(lib)? {
        let top = f.rel_path.split('/').next().unwrap_or_default().to_owned();
        by_top.entry(top).or_default().push(f.rel_path);
    }
    for (top, mut files) in by_top.into_iter().step_by(every) {
        files.sort();
        println!("== {top}  ({} files)", files.len());
        show("folder", &hunch::hunch(&top));
        let names: Vec<&str> = files.iter().map(|p| p.rsplit('/').next().unwrap_or(p)).collect();
        for path in files.iter().take(2) {
            let name = path.rsplit('/').next().unwrap_or(path);
            println!("  file   {path}");
            show("name", &hunch::hunch_with_context(name, &names));
            show("path", &hunch::hunch(path));
        }
    }
    Ok(())
}
