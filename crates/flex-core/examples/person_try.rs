//! Live check of person lookups (actor pages). Usage: person_try <token file>
fn main() -> anyhow::Result<()> {
    let token = std::fs::read_to_string(std::env::args().nth(1).unwrap())?;
    let tmdb = flex_core::metadata::Tmdb::new(&token);
    let m = tmdb.details("movie:5825")?;
    let actor = m.cast.first().and_then(|c| c.person.clone()).expect("cast has ids");
    let p = flex_core::metadata::person(Some(&tmdb), &actor)?;
    println!("{actor}: {} born {:?} in {:?}, {} credits", p.name, p.born, p.birthplace, p.credits.len());
    for c in p.credits.iter().take(5) {
        println!("  {:?} {} ({:?}) as {} [{:?}]", c.kind, c.title, c.year, c.role, c.provider_id);
    }
    let voice = flex_core::metadata::anilist_info("Attack on Titan")?;
    let va = voice.cast.first().and_then(|c| c.person.clone()).expect("voice cast has ids");
    let p = flex_core::metadata::person(None, &va)?;
    println!("{va}: {} born {:?}, {} credits\n  bio: {:.200}", p.name, p.born, p.credits.len(), p.biography);
    for c in p.credits.iter().take(5) {
        println!("  {:?} {} ({:?}) as {} also {:?}", c.kind, c.title, c.year, c.role, c.other_titles);
    }
    Ok(())
}
