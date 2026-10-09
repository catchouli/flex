//! Live check of the TMDB/AniList client. Usage: tmdb_try <token file>
use flex_core::metadata::{self, Kind, Tmdb};

fn main() -> anyhow::Result<()> {
    let token = std::fs::read_to_string(std::env::args().nth(1).unwrap())?;
    let tmdb = Tmdb::new(&token);
    tmdb.verify()?;
    for (kind, q, year) in [
        (Kind::Tv, "Dorohedoro", None),
        (Kind::Tv, "Tenkuu no Escaflowne", None),
        (Kind::Movie, "Christmas Vacation", Some(1989)),
        (Kind::Movie, "Rasen", Some(1998)),
    ] {
        let results = tmdb.search(kind, q, year)?;
        let best = metadata::best(q, year, &results).map(|(c, s)| format!("{} ({:?}) {}, score {s:.2}", c.title, c.year, c.provider_id()));
        println!("{q:?}: {} results, best: {best:?}", results.len());
    }
    println!("AniList for 'Shingeki no Kyojin': {:?}", metadata::anilist_titles("Shingeki no Kyojin")?);
    Ok(())
}
