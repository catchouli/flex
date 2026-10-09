//! Times hunch over every file in a library. Usage: parse_bench <db> <library id>
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let db = flex_core::Db::open(std::path::Path::new(&args[1]))?;
    let files = db.files(args[2].parse()?)?;
    let start = std::time::Instant::now();
    let mut episodes = 0;
    for f in &files {
        episodes += usize::from(hunch::hunch(f.file_name()).episode().is_some());
    }
    println!("{} files in {:?}; {} with an episode number", files.len(), start.elapsed(), episodes);
    Ok(())
}
