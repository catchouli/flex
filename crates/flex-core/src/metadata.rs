//! Online metadata: TMDB for titles, years, overviews and posters (TV and films), with
//! AniList as a helper that turns romaji anime titles into ones TMDB knows.

use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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

/// One credited person: an actor and their character, or a voice actor.
#[derive(Clone, Debug, PartialEq)]
pub struct CastMember {
    pub name: String,
    /// Character played.
    pub role: String,
    /// TMDB profile image path, or a full image URL (AniList).
    pub image: Option<String>,
    /// "tmdb:123" or "anilist:456", for their page.
    pub person: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeasonInfo {
    pub number: i32,
    pub episode_count: i32,
}

/// Everything shown on a show or film's page.
#[derive(Clone, Debug, Default)]
pub struct Details {
    pub title: String,
    pub overview: String,
    pub release_date: Option<String>,
    /// Minutes (per episode for TV).
    pub runtime: Option<i32>,
    pub genres: Vec<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub cast: Vec<CastMember>,
    /// TV only.
    pub seasons: Vec<SeasonInfo>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EpisodeInfo {
    pub season: i32,
    pub episode: i32,
    pub title: String,
    pub overview: String,
    pub still_path: Option<String>,
    pub air_date: Option<String>,
    pub runtime: Option<i32>,
}

/// How many cast members are kept.
const MAX_CAST: usize = 20;

fn str_of(v: &serde_json::Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

fn opt_str(v: &serde_json::Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

impl Tmdb {
    fn get_json(&self, path: &str, query: &[(&str, &str)]) -> Result<serde_json::Value> {
        let mut req =
            self.agent.get(format!("{TMDB_API}{path}")).header("Authorization", format!("Bearer {}", self.token));
        for (k, v) in query {
            req = req.query(*k, *v);
        }
        Ok(req.call().with_context(|| format!("TMDB {path}"))?.body_mut().read_json()?)
    }

    /// Details and cast for a matched entry ("tv:123" / "movie:456").
    pub fn details(&self, provider_id: &str) -> Result<Details> {
        let (kind, id) = provider_id.split_once(':').context("bad provider id")?;
        let tv = kind == "tv";
        let credits = if tv { "aggregate_credits" } else { "credits" };
        let v = self.get_json(&format!("/{kind}/{id}"), &[("append_to_response", credits)])?;
        let cast = v[credits]["cast"]
            .as_array()
            .into_iter()
            .flatten()
            .take(MAX_CAST)
            .map(|c| CastMember {
                name: str_of(&c["name"]),
                // TV credits list roles; films a single character.
                role: c["roles"][0]["character"].as_str().map_or_else(|| str_of(&c["character"]), str::to_owned),
                image: opt_str(&c["profile_path"]),
                person: c["id"].as_u64().map(|id| format!("tmdb:{id}")),
            })
            .collect();
        let runtime = if tv { v["episode_run_time"][0].as_i64() } else { v["runtime"].as_i64() };
        Ok(Details {
            title: str_of(if tv { &v["name"] } else { &v["title"] }),
            overview: str_of(&v["overview"]),
            release_date: opt_str(if tv { &v["first_air_date"] } else { &v["release_date"] }),
            runtime: runtime.filter(|r| *r > 0).map(|r| r as i32),
            genres: v["genres"].as_array().into_iter().flatten().map(|g| str_of(&g["name"])).collect(),
            poster_path: opt_str(&v["poster_path"]),
            backdrop_path: opt_str(&v["backdrop_path"]),
            cast,
            seasons: v["seasons"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| {
                    Some(SeasonInfo {
                        number: s["season_number"].as_i64()? as i32,
                        episode_count: s["episode_count"].as_i64().unwrap_or(0) as i32,
                    })
                })
                .collect(),
        })
    }

    /// Episode titles, descriptions and stills for one season of a series.
    pub fn season(&self, tv_id: &str, season: i32) -> Result<Vec<EpisodeInfo>> {
        let v = self.get_json(&format!("/tv/{tv_id}/season/{season}"), &[])?;
        Ok(v["episodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                Some(EpisodeInfo {
                    season,
                    episode: e["episode_number"].as_i64()? as i32,
                    title: str_of(&e["name"]),
                    overview: str_of(&e["overview"]),
                    still_path: opt_str(&e["still_path"]),
                    air_date: opt_str(&e["air_date"]),
                    runtime: e["runtime"].as_i64().map(|r| r as i32),
                })
            })
            .collect())
    }
}

/// Where a locally numbered episode sits in TMDB's seasons. Uses (season, episode) when
/// TMDB has it; otherwise treats the number as absolute and counts through the seasons
/// (anime numbered 1–150 straight through, which TMDB splits into seasons).
pub fn map_episode(season: i32, episode: i32, seasons: &[SeasonInfo]) -> Option<(i32, i32)> {
    let fits = |s: &SeasonInfo| s.number == season && episode >= 1 && episode <= s.episode_count;
    if seasons.iter().any(fits) {
        return Some((season, episode));
    }
    // Absolute numbering only makes sense counted from the first regular season.
    let first_regular = seasons.iter().filter(|s| s.number > 0).map(|s| s.number).min()?;
    if season > first_regular || season == 0 {
        return None;
    }
    let mut regular: Vec<&SeasonInfo> = seasons.iter().filter(|s| s.number > 0).collect();
    regular.sort_by_key(|s| s.number);
    let mut remaining = episode;
    for s in regular {
        if remaining <= s.episode_count {
            return Some((s.number, remaining));
        }
        remaining -= s.episode_count;
    }
    None
}

/// What AniList knows about an anime: its other titles and the Japanese voice cast.
#[derive(Clone, Debug, Default)]
pub struct AniListInfo {
    /// Romaji, English, native titles and synonyms.
    pub titles: Vec<String>,
    pub cast: Vec<CastMember>,
}

/// Looks an anime up on AniList (one request).
pub fn anilist_info(title: &str) -> Result<AniListInfo> {
    let body = serde_json::json!({
        "query": "query($s:String){Media(search:$s,type:ANIME){title{romaji english native} synonyms characters(sort:[ROLE,RELEVANCE],perPage:16){edges{node{name{full}}voiceActors(language:JAPANESE){id name{full}image{medium}}}}}}",
        "variables": { "s": title },
    });
    anilist_wait();
    let value: serde_json::Value = match agent().post(ANILIST_API).send_json(&body) {
        Ok(mut response) => response.body_mut().read_json()?,
        // AniList answers 404 when nothing matches: that's an answer, not an error.
        Err(ureq::Error::StatusCode(404)) => return Ok(AniListInfo::default()),
        Err(e) => return Err(e).context("AniList lookup"),
    };
    let media = &value["data"]["Media"];
    let mut titles: Vec<String> = Vec::new();
    let names = ["romaji", "english", "native"].iter().map(|k| &media["title"][*k]);
    for t in names.chain(media["synonyms"].as_array().into_iter().flatten()) {
        if let Some(t) = t.as_str().filter(|t| !t.is_empty())
            && !titles.iter().any(|x| x == t)
        {
            titles.push(t.to_owned());
        }
    }
    let mut cast = Vec::new();
    for edge in media["characters"]["edges"].as_array().into_iter().flatten() {
        let character = str_of(&edge["node"]["name"]["full"]);
        if let Some(va) = edge["voiceActors"].as_array().and_then(|v| v.first()) {
            cast.push(CastMember {
                name: str_of(&va["name"]["full"]),
                role: character,
                image: opt_str(&va["image"]["medium"]),
                person: va["id"].as_u64().map(|id| format!("anilist:{id}")),
            });
        }
    }
    Ok(AniListInfo { titles, cast })
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

/// What a person's page shows from online: about them, and everything they've been in.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    pub biography: String,
    /// "1980-04-12".
    pub born: Option<String>,
    pub birthplace: Option<String>,
    pub image: Option<String>,
    /// Most popular first.
    pub credits: Vec<PersonCredit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CreditKind {
    Movie,
    Tv,
    Anime,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersonCredit {
    pub kind: CreditKind,
    /// TMDB entry ("movie:123"), comparable with shows' provider ids.
    pub provider_id: Option<String>,
    pub title: String,
    /// Other titles (AniList: romaji, English, native), for spotting ones in the library.
    pub other_titles: Vec<String>,
    pub year: Option<i32>,
    /// Characters played, comma separated.
    pub role: String,
    /// TMDB poster path or image URL.
    pub poster: Option<String>,
}

/// TMDB genres that are people appearing as themselves on talk shows and the news.
const TALK_AND_NEWS: [u64; 2] = [10767, 10763];

impl Tmdb {
    /// A person's details and acting credits (films and TV), most popular first.
    pub fn person(&self, id: &str) -> Result<Person> {
        let v = self.get_json(&format!("/person/{id}"), &[("append_to_response", "combined_credits")])?;
        let mut credits: Vec<(PersonCredit, f64)> = Vec::new();
        for c in v["combined_credits"]["cast"].as_array().into_iter().flatten() {
            let genres: Vec<u64> = c["genre_ids"].as_array().into_iter().flatten().filter_map(|g| g.as_u64()).collect();
            if genres.iter().any(|g| TALK_AND_NEWS.contains(g)) {
                continue;
            }
            let (kind, title, date) = match c["media_type"].as_str() {
                Some("movie") => (CreditKind::Movie, &c["title"], &c["release_date"]),
                Some("tv") => (CreditKind::Tv, &c["name"], &c["first_air_date"]),
                _ => continue,
            };
            let Some(tmdb_id) = c["id"].as_u64() else { continue };
            let provider_id = format!("{}:{tmdb_id}", if kind == CreditKind::Movie { "movie" } else { "tv" });
            let role = str_of(&c["character"]);
            // Several roles in one entry are listed once.
            if let Some((existing, _)) = credits.iter_mut().find(|(e, _)| e.provider_id.as_deref() == Some(provider_id.as_str())) {
                if !role.is_empty() && !existing.role.split(", ").any(|r| r == role) {
                    existing.role = if existing.role.is_empty() { role } else { format!("{}, {role}", existing.role) };
                }
                continue;
            }
            let credit = PersonCredit {
                kind,
                provider_id: Some(provider_id),
                title: str_of(title),
                other_titles: Vec::new(),
                year: year_of(date.as_str()),
                role,
                poster: opt_str(&c["poster_path"]),
            };
            // Best known first: by votes, and for TV shows with real parts (10+ episodes)
            // before guest spots.
            let votes = c["vote_count"].as_f64().unwrap_or(0.0);
            let rank = match kind {
                CreditKind::Movie => votes,
                _ => c["episode_count"].as_f64().unwrap_or(1.0).min(10.0) * 1e7 + votes,
            };
            // Appearing as themselves (award shows, documentaries) goes last.
            let first_role = credit.role.split([',', '(', '-']).next().unwrap_or_default().trim().to_lowercase();
            let as_self = matches!(first_role.as_str(), "self" | "himself" | "herself" | "themselves" | "themself");
            credits.push((credit, if as_self { rank - 1e9 } else { rank }));
        }
        credits.sort_by(|a, b| b.1.total_cmp(&a.1));
        Ok(Person {
            name: str_of(&v["name"]),
            // TMDB copies Wikipedia, credit line and all.
            biography: str_of(&v["biography"]).split("Description above from the Wikipedia").next().unwrap_or_default().trim().to_owned(),
            born: opt_str(&v["birthday"]),
            birthplace: opt_str(&v["place_of_birth"]),
            image: opt_str(&v["profile_path"]),
            credits: credits.into_iter().map(|(c, _)| c).collect(),
        })
    }
}

/// Removes AniList's markdown, collapsed-block markers ("~!...!~", used for long lists of
/// roles) and lines of nothing but links from a description.
fn plain_text(s: &str) -> String {
    static LINK: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap());
    let s = s.replace("~!", "\n").replace("!~", "\n").replace("<br>", "\n");
    let lines: Vec<String> = s
        .lines()
        .filter(|l| !LINK.replace_all(l, "").trim_matches(|c: char| c == '|' || c.is_whitespace()).is_empty())
        .map(|l| LINK.replace_all(l, "$1").replace("__", "").replace("**", "").trim().to_owned())
        .collect();
    lines.join("\n")
}

/// A voice actor's details and anime roles from AniList, most popular first.
pub fn anilist_person(id: &str) -> Result<Person> {
    let body = serde_json::json!({
        "query": "query($id:Int){Staff(id:$id){name{full} image{large} description dateOfBirth{year month day} homeTown characterMedia(sort:POPULARITY_DESC,perPage:50){edges{node{type title{romaji english native} startDate{year} coverImage{large}} characters{name{full}}}}}}",
        "variables": { "id": id.parse::<i64>().context("bad AniList id")? },
    });
    anilist_wait();
    let value: serde_json::Value = agent().post(ANILIST_API).send_json(&body).context("AniList lookup")?.body_mut().read_json()?;
    let staff = &value["data"]["Staff"];
    let born = {
        let d = &staff["dateOfBirth"];
        match (d["year"].as_i64(), d["month"].as_i64(), d["day"].as_i64()) {
            (Some(y), Some(m), Some(day)) => Some(format!("{y:04}-{m:02}-{day:02}")),
            (Some(y), _, _) => Some(y.to_string()),
            _ => None,
        }
    };
    let mut credits = Vec::new();
    for edge in staff["characterMedia"]["edges"].as_array().into_iter().flatten() {
        let node = &edge["node"];
        if node["type"].as_str() != Some("ANIME") {
            continue;
        }
        let names = ["english", "romaji", "native"].map(|k| opt_str(&node["title"][k]));
        let mut titles = names.into_iter().flatten();
        let Some(title) = titles.next() else { continue };
        let characters: Vec<String> =
            edge["characters"].as_array().into_iter().flatten().filter_map(|c| opt_str(&c["name"]["full"])).collect();
        credits.push(PersonCredit {
            kind: CreditKind::Anime,
            provider_id: None,
            title,
            other_titles: titles.collect(),
            year: node["startDate"]["year"].as_i64().map(|y| y as i32),
            role: characters.join(", "),
            poster: opt_str(&node["coverImage"]["large"]),
        });
    }
    Ok(Person {
        name: str_of(&staff["name"]["full"]),
        biography: plain_text(staff["description"].as_str().unwrap_or_default()),
        born,
        birthplace: opt_str(&staff["homeTown"]),
        image: opt_str(&staff["image"]["large"]),
        credits,
    })
}

/// Looks a person up by their id ("tmdb:123" needs `tmdb`; "anilist:456" doesn't).
pub fn person(tmdb: Option<&Tmdb>, person: &str) -> Result<Person> {
    match person.split_once(':') {
        Some(("tmdb", id)) => tmdb.context("TMDB isn't set up")?.person(id),
        Some(("anilist", id)) => anilist_person(id),
        _ => anyhow::bail!("unknown person {person:?}"),
    }
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
    fn maps_absolute_episode_numbers_across_seasons() {
        let seasons = [
            SeasonInfo { number: 0, episode_count: 5 },
            SeasonInfo { number: 1, episode_count: 12 },
            SeasonInfo { number: 2, episode_count: 13 },
        ];
        assert_eq!(map_episode(1, 5, &seasons), Some((1, 5)));
        assert_eq!(map_episode(2, 3, &seasons), Some((2, 3)));
        assert_eq!(map_episode(1, 14, &seasons), Some((2, 2)), "absolute numbering");
        assert_eq!(map_episode(1, 40, &seasons), None);
        assert_eq!(map_episode(3, 1, &seasons), None);
        assert_eq!(map_episode(0, 2, &seasons), Some((0, 2)));
    }

    #[test]
    fn anilist_descriptions_become_plain_text() {
        let d = "[Profile](https://a) | [Twitter](https://b)\n__Height:__ 156 cm\n\nShe voiced [Mai](https://anilist.co/character/1).\n**Roles:**~!\n• A\n!~";
        assert_eq!(plain_text(d), "Height: 156 cm\nShe voiced Mai.\nRoles:\n• A");
    }

    #[test]
    fn best_prefers_exact_then_popular() {
        let cs = [cand("Lost Universe", "", Some(1998), 1.0), cand("Lost", "", Some(2004), 90.0)];
        assert_eq!(best("Lost Universe", None, &cs).unwrap().0.title, "Lost Universe");
    }
}
