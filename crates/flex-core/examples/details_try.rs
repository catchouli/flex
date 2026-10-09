//! Live check of details/season/voice-cast fetching. Usage: details_try <token file>
fn main() -> anyhow::Result<()> {
    let token = std::fs::read_to_string(std::env::args().nth(1).unwrap())?;
    let tmdb = flex_core::metadata::Tmdb::new(&token);
    let d = tmdb.details("tv:94404")?;
    println!("{} ({:?}) {:?} min, genres {:?}", d.title, d.release_date, d.runtime, d.genres);
    println!("seasons {:?}", d.seasons.iter().map(|s| (s.number, s.episode_count)).collect::<Vec<_>>());
    println!("cast: {:?}", d.cast.iter().take(3).map(|c| format!("{} as {}", c.name, c.role)).collect::<Vec<_>>());
    let eps = tmdb.season("94404", 1)?;
    println!("S1: {} episodes, e.g. {:?}", eps.len(), eps.iter().take(2).map(|e| &e.title).collect::<Vec<_>>());
    let m = tmdb.details("movie:5825")?;
    println!("{} {:?} {:?} min, cast[0] {:?}", m.title, m.release_date, m.runtime, m.cast.first().map(|c| (&c.name, &c.role)));
    let info = flex_core::metadata::anilist_info("Attack on Titan")?;
    println!("titles: {:?}", info.titles);
    println!("VAs: {:?}", info.cast.iter().take(3).map(|c| format!("{} as {}", c.name, c.role)).collect::<Vec<_>>());
    Ok(())
}
