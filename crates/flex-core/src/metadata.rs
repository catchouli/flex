//! Online metadata: TMDB for titles, years, overviews and posters (TV and films), with
//! AniList as a helper that turns romaji anime titles into ones TMDB knows.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::parse::normalize;

const TMDB_API: &str = "https://api.themoviedb.org/3";
const ANILIST_API: &str = "https://graphql.anilist.co";
/// Size used for cached posters.
pub const POSTER_BASE: &str = "https://image.tmdb.org/t/p/w342";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Tv,
    Movie,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Tv => "tv",
            Kind::Movie => "movie",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub kind: Kind,
    pub id: u64,
    pub title: String,
    pub original_title: String,
    pub year: Option<i32>,
    pub overview: String,
    pub poster_path: Option<String>,
    pub popularity: f64,
    pub genre_ids: Vec<u32>,
    pub original_language: String,
}

/// TMDB's genre id for animation.
const ANIMATION: u32 = 16;

impl Candidate {
    /// Plausibly anime: animated, or Japanese / Chinese / Korean.
    pub fn looks_like_anime(&self) -> bool {
        self.genre_ids.contains(&ANIMATION) || matches!(self.original_language.as_str(), "ja" | "zh" | "ko")
    }

    /// Stored as the show's provider id, e.g. "tv:12345".
    pub fn provider_id(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.id)
    }
}

#[derive(Deserialize)]
struct SearchPage {
    results: Vec<SearchResult>,
}

#[derive(Deserialize)]
struct SearchResult {
    id: u64,
    // TV uses name / original_name / first_air_date; films title / original_title / release_date.
    name: Option<String>,
    original_name: Option<String>,
    first_air_date: Option<String>,
    title: Option<String>,
    original_title: Option<String>,
    release_date: Option<String>,
    #[serde(default)]
    overview: String,
    poster_path: Option<String>,
    #[serde(default)]
    popularity: f64,
    #[serde(default)]
    genre_ids: Vec<u32>,
    #[serde(default)]
    original_language: String,
}

fn year_of(date: Option<&str>) -> Option<i32> {
    date.and_then(|d| d.get(..4)).and_then(|y| y.parse().ok())
}

pub struct Tmdb {
    token: String,
    agent: ureq::Agent,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).build().into()
}

impl Tmdb {
    /// `token` is the "API Read Access Token" from TMDB's settings.
    pub fn new(token: &str) -> Self {
        Self { token: token.trim().to_owned(), agent: agent() }
    }

    pub fn search(&self, kind: Kind, query: &str, year: Option<i32>) -> Result<Vec<Candidate>> {
        let mut req = self
            .agent
            .get(format!("{TMDB_API}/search/{}", kind.as_str()))
            .header("Authorization", format!("Bearer {}", self.token))
            .query("query", query)
            .query("include_adult", "false");
        if let Some(y) = year {
            let key = if kind == Kind::Tv { "first_air_date_year" } else { "year" };
            req = req.query(key, y.to_string());
        }
        let page: SearchPage = req
            .call()
            .with_context(|| format!("TMDB search for {query:?}"))?
            .body_mut()
            .read_json()
            .context("reading TMDB search results")?;
        Ok(page
            .results
            .into_iter()
            .map(|r| {
                let title = r.name.or(r.title).unwrap_or_default();
                let original_title = r.original_name.or(r.original_title).unwrap_or_else(|| title.clone());
                Candidate {
                    kind,
                    id: r.id,
                    year: year_of(r.first_air_date.as_deref().or(r.release_date.as_deref())),
                    title,
                    original_title,
                    overview: r.overview,
                    poster_path: r.poster_path,
                    popularity: r.popularity,
                    genre_ids: r.genre_ids,
                    original_language: r.original_language,
                }
            })
            .collect())
    }

    /// Checks the token works.
    pub fn verify(&self) -> Result<()> {
        self.agent
            .get(format!("{TMDB_API}/authentication"))
            .header("Authorization", format!("Bearer {}", self.token))
            .call()
            .context("TMDB rejected the token")?;
        Ok(())
    }
}

/// AniList allows roughly 30 requests a minute; space them out.
const ANILIST_SPACING: Duration = Duration::from_millis(2100);

fn anilist_wait() {
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = *last {
        std::thread::sleep(ANILIST_SPACING.saturating_sub(t.elapsed()));
    }
    *last = Some(std::time::Instant::now());
}

/// Alternative titles for an anime from AniList (English, romaji, native), to retry TMDB with.
pub fn anilist_titles(query: &str) -> Result<Vec<String>> {
    let body = serde_json::json!({
        "query": "query($s:String){Page(perPage:3){media(search:$s,type:ANIME){title{romaji english native}}}}",
        "variables": { "s": query },
    });
    anilist_wait();
    let mut response = agent().post(ANILIST_API).send_json(&body);
    if matches!(response, Err(ureq::Error::StatusCode(429))) {
        std::thread::sleep(Duration::from_secs(30)); // rate limited: back off once
        anilist_wait();
        response = agent().post(ANILIST_API).send_json(&body);
    }
    let value: serde_json::Value = response.context("AniList search")?.body_mut().read_json()?;
    let mut titles = Vec::new();
    for media in value["data"]["Page"]["media"].as_array().into_iter().flatten() {
        for key in ["english", "romaji", "native"] {
            if let Some(t) = media["title"][key].as_str().filter(|t| !t.is_empty())
                && !titles.iter().any(|x: &String| x == t)
            {
                titles.push(t.to_owned());
            }
        }
    }
    Ok(titles)
}

/// How well a candidate fits the title (and year) parsed from file names: 0–1.
pub fn score(query: &str, year: Option<i32>, c: &Candidate) -> f64 {
    let q = normalize(query);
    let sim = [&c.title, &c.original_title]
        .iter()
        .map(|t| {
            let t = normalize(t);
            if t == q { 1.0 } else { strsim::jaro_winkler(&q, &t) }
        })
        .fold(0.0, f64::max);
    let year_adj = match (year, c.year) {
        (Some(a), Some(b)) if (a - b).abs() <= 1 => 0.05,
        (Some(_), Some(_)) => -0.15,
        _ => 0.0,
    };
    (sim + year_adj).clamp(0.0, 1.0)
}

/// Candidates must score at least this to be accepted automatically.
pub const ACCEPT_SCORE: f64 = 0.9;

/// The best candidate and its score, preferring popular entries on ties.
pub fn best<'a>(query: &str, year: Option<i32>, candidates: &'a [Candidate]) -> Option<(&'a Candidate, f64)> {
    candidates
        .iter()
        .map(|c| (c, score(query, year, c)))
        .max_by(|(a, sa), (b, sb)| sa.total_cmp(sb).then(a.popularity.total_cmp(&b.popularity)))
}

/// Picks the result to accept automatically, or `None` to leave the show unmatched.
/// `candidates` must be in TMDB's own relevance order: TMDB already searches alternative
/// and translated titles, so its top hit is trusted when the year or the wording agrees,
/// even if the title differs ("Rasen" → "The Spiral").
pub fn choose<'a>(query: &str, year: Option<i32>, candidates: &'a [Candidate]) -> Option<&'a Candidate> {
    let first = candidates.first()?;
    if let Some((c, s)) = best(query, year, candidates)
        && s >= ACCEPT_SCORE
    {
        return Some(c);
    }
    let q = normalize(query);
    if q.chars().count() < 3 {
        return None; // too little to go on
    }
    let year_ok = matches!((year, first.year), (Some(a), Some(b)) if (a - b).abs() <= 1);
    let year_conflict = matches!((year, first.year), (Some(a), Some(b)) if (a - b).abs() > 1);
    // Whole words only: "mai" must not match "Ganga Mai Ki Betiyan".
    let padded = |s: &str| format!(" {s} ");
    let contains = [&first.title, &first.original_title].iter().any(|t| {
        let t = normalize(t);
        let (pt, pq) = (padded(&t), padded(&q));
        !t.is_empty() && q.contains(' ') && (pt.contains(&pq) || pq.contains(&pt))
    });
    let only = candidates.len() == 1;
    let resemblance = score(query, None, first) >= 0.4;
    let accept = (year_ok && (only || contains || resemblance)) || (!year_conflict && (contains || (only && resemblance)));
    accept.then_some(first)
}

/// What to look up for one show.
#[derive(Clone, Debug)]
pub struct Lookup {
    pub show_id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub kind: Kind,
    /// Retry via AniList's alternative titles if TMDB finds nothing.
    pub anime: bool,
}

/// Finds the TMDB entry for a show, or `None` if nothing fits well enough.
///
/// Tries the expected kind (TV/film) then the other one (anime films often come with
/// extras that make them look like a series), and for anime retries with AniList's
/// alternative titles. In anime libraries only animated / East Asian results count.
pub fn lookup(tmdb: &Tmdb, item: &Lookup) -> Result<Option<Candidate>> {
    let other = if item.kind == Kind::Tv { Kind::Movie } else { Kind::Tv };
    let attempt = |kind: Kind, title: &str| -> Result<Option<Candidate>> {
        let keep = |mut results: Vec<Candidate>| {
            if item.anime {
                results.retain(Candidate::looks_like_anime);
            }
            results
        };
        let results = keep(tmdb.search(kind, title, item.year)?);
        if let Some(c) = choose(title, item.year, &results) {
            return Ok(Some(c.clone()));
        }
        // The parsed year may be off (e.g. the end of "1998-2000"): without it, accept
        // only an exact title within a few years.
        if let Some(year) = item.year {
            let q = normalize(title);
            let close = |c: &Candidate| c.year.is_none_or(|y| (y - year).abs() <= 3);
            let exact = |c: &Candidate| normalize(&c.title) == q || normalize(&c.original_title) == q;
            return Ok(keep(tmdb.search(kind, title, None)?).into_iter().find(|c| exact(c) && close(c)));
        }
        Ok(None)
    };
    // "Mai-HiME" needs its hyphenated part, "BtX-VFS" doesn't (VFS is a fansub group):
    // try the full title first, then without the last "-part".
    let mut titles = vec![item.title.clone()];
    if let Some((before, _)) = item.title.rsplit_once('-') {
        let shorter = crate::parse::clean_title(before);
        if !shorter.is_empty() && shorter != item.title {
            titles.push(shorter);
        }
    }
    for title in &titles {
        for kind in [item.kind, other] {
            if let Some(found) = attempt(kind, title)? {
                return Ok(Some(found));
            }
        }
    }
    if item.anime {
        for alt in anilist_titles(&item.title)?.iter().take(3) {
            for kind in [item.kind, other] {
                if let Some(found) = attempt(kind, alt)? {
                    return Ok(Some(found));
                }
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(title: &str, original: &str, year: Option<i32>, popularity: f64) -> Candidate {
        Candidate {
            kind: Kind::Tv,
            id: 1,
            title: title.into(),
            original_title: original.into(),
            year,
            overview: String::new(),
            poster_path: None,
            popularity,
            genre_ids: vec![16],
            original_language: "ja".into(),
        }
    }

    #[test]
    fn scoring() {
        let exact = cand("Dorohedoro", "ドロヘドロ", Some(2020), 10.0);
        assert!(score("Dorohedoro", None, &exact) >= ACCEPT_SCORE);
        // Matches on the original (romaji/native) title too.
        let aot = cand("Attack on Titan", "進撃の巨人", Some(2013), 50.0);
        assert!(score("Attack On Titan", Some(2013), &aot) >= ACCEPT_SCORE);
        assert!(score("Lost Universe", None, &aot) < ACCEPT_SCORE);
        // A wrong year counts against.
        let remake = cand("Rasen", "らせん", Some(1998), 5.0);
        assert!(score("Rasen", Some(1998), &remake) > score("Rasen", Some(2010), &remake));
    }

    #[test]
    fn trusts_tmdb_order_when_year_or_wording_agrees() {
        let movie = |title: &str, original: &str, year| Candidate { kind: Kind::Movie, ..cand(title, original, year, 1.0) };
        let lampoon = [movie("National Lampoon's Christmas Vacation", "", Some(1989))];
        assert!(choose("Christmas Vacation", Some(1989), &lampoon).is_some());
        let spiral = [movie("The Spiral", "らせん", Some(1998))];
        assert!(choose("Rasen", Some(1998), &spiral).is_some(), "only result, right year");
        assert!(choose("Rasen", Some(2012), &spiral).is_none(), "wrong year");
        let escaflowne = [cand("The Vision of Escaflowne", "天空のエスカフローネ", Some(1996), 1.0)];
        assert!(choose("Tenkuu no Escaflowne", None, &escaflowne).is_some(), "only result, and it resembles the romaji title");
        let unrelated = [cand("Friends", "", Some(1994), 99.0), cand("Frasier", "", Some(1993), 50.0)];
        assert!(choose("Abd", None, &unrelated).is_none());
        let mai = [cand("Ganga Mai Ki Betiyan", "", Some(2025), 3.0), cand("Other", "", None, 1.0)];
        assert!(choose("Mai", None, &mai).is_none(), "single short word isn't enough");
    }

    #[test]
    fn best_prefers_exact_then_popular() {
        let cs = [cand("Lost Universe", "", Some(1998), 1.0), cand("Lost", "", Some(2004), 90.0)];
        assert_eq!(best("Lost Universe", None, &cs).unwrap().0.title, "Lost Universe");
    }
}
