//! MusicBrainz: matching albums (release groups), artists' discographies, and covers
//! (Cover Art Archive) and photos (via Wikidata and Wikimedia Commons).
//!
//! No key is needed, but MusicBrainz asks for at most one request a second and a
//! User-Agent saying who's asking.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const API: &str = "https://musicbrainz.org/ws/2";
pub const USER_AGENT: &str = concat!("Flex/", env!("CARGO_PKG_VERSION"), " ( https://github.com/catchouli/flex )");
/// MusicBrainz allows one request a second (per address).
const SPACING: Duration = Duration::from_millis(1100);
/// A match is only taken when the names are at least this similar (0–1).
const MIN_TITLE: f64 = 0.85;
const MIN_ARTIST: f64 = 0.75;

/// A release group's front cover, from the Cover Art Archive (500px).
pub fn cover_url(release_group: &str) -> String {
    format!("https://coverartarchive.org/release-group/{release_group}/front-500")
}

/// An album as MusicBrainz knows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReleaseGroup {
    pub id: String,
    pub title: String,
    /// "Album", "EP", "Single", ...
    pub kind: Option<String>,
    /// "Live", "Compilation", ...
    #[serde(default)]
    pub secondary: Vec<String>,
    pub year: Option<i32>,
    /// (name, artist id) as credited.
    #[serde(default)]
    pub artists: Vec<(String, String)>,
}

impl ReleaseGroup {
    /// "Album · Live", "EP".
    pub fn kind_label(&self) -> String {
        let mut parts: Vec<&str> = self.kind.iter().map(String::as_str).collect();
        parts.extend(self.secondary.iter().map(String::as_str));
        parts.join(" · ")
    }
}

/// What's fetched about an artist.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ArtistInfo {
    pub name: String,
    /// A photo (Wikimedia Commons), if Wikidata has one.
    pub image: Option<String>,
    /// Their studio albums, EPs and soundtracks, oldest first.
    pub discography: Vec<ReleaseGroup>,
}

#[derive(Deserialize)]
struct RgJson {
    id: String,
    title: String,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(rename = "secondary-types", default)]
    secondary_types: Vec<String>,
    #[serde(rename = "first-release-date", default)]
    first_release_date: Option<String>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<CreditJson>,
    /// Search relevance, 0–100.
    #[serde(default)]
    score: Option<u32>,
}

#[derive(Deserialize)]
struct CreditJson {
    name: String,
    artist: ArtistRefJson,
}

#[derive(Deserialize)]
struct ArtistRefJson {
    id: String,
}

impl From<RgJson> for ReleaseGroup {
    fn from(r: RgJson) -> Self {
        let year = r.first_release_date.as_deref().and_then(|d| d.get(..4)).and_then(|y| y.parse().ok());
        Self {
            id: r.id,
            title: r.title,
            kind: r.primary_type,
            secondary: r.secondary_types,
            year,
            artists: r.artist_credit.into_iter().map(|c| (c.name, c.artist.id)).collect(),
        }
    }
}

/// Talks to MusicBrainz (and Wikidata), one request a second.
pub struct MusicBrainz {
    agent: ureq::Agent,
}

impl Default for MusicBrainz {
    fn default() -> Self {
        Self::new()
    }
}

impl MusicBrainz {
    pub fn new() -> Self {
        Self { agent: ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).build().into() }
    }

    /// GETs `url` as JSON, spacing MusicBrainz requests out and retrying when it's busy.
    fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        static LAST: Mutex<Option<Instant>> = Mutex::new(None);
        for attempt in 0..5 {
            {
                // One request at a time across all threads, a second apart.
                let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(t) = *last {
                    std::thread::sleep(SPACING.saturating_sub(t.elapsed()));
                }
                *last = Some(Instant::now());
            }
            match self.agent.get(url).header("User-Agent", USER_AGENT).header("Accept", "application/json").call() {
                Ok(mut response) => return response.body_mut().with_config().limit(10_000_000).read_json().context("reading MusicBrainz's answer"),
                // "Busy": over the rate limit. Wait a little longer each time.
                Err(ureq::Error::StatusCode(503)) => std::thread::sleep(Duration::from_secs(1 << attempt)),
                Err(e) => return Err(e).with_context(|| format!("asking MusicBrainz ({url})")),
            }
        }
        bail!("MusicBrainz stayed busy")
    }

    /// Release groups matching an album title (and artist, unless it's a compilation).
    pub fn search(&self, title: &str, artist: Option<&str>) -> Result<Vec<(ReleaseGroup, u32)>> {
        let mut query = format!("releasegroup:\"{}\"", lucene_escape(title));
        if let Some(artist) = artist {
            query += &format!(" AND artist:\"{}\"", lucene_escape(artist));
        }
        #[derive(Deserialize)]
        struct Found {
            #[serde(rename = "release-groups", default)]
            release_groups: Vec<RgJson>,
        }
        let url = format!("{API}/release-group?query={}&fmt=json&limit=15", encode(&query));
        let found: Found = self.get(&url)?;
        Ok(found.release_groups.into_iter().map(|r| (r.score.unwrap_or(0), r)).map(|(s, r)| (ReleaseGroup::from(r), s)).collect())
    }

    /// The best match for a local album, if one is close enough.
    pub fn find_album(&self, title: &str, artist: Option<&str>, tracks: usize, year: Option<i32>) -> Result<Option<ReleaseGroup>> {
        let found = self.search(title, artist)?;
        Ok(best_match(found, title, artist, tracks, year))
    }

    /// An artist's albums and EPs, and a photo if Wikidata has one.
    pub fn artist(&self, id: &str) -> Result<ArtistInfo> {
        #[derive(Deserialize)]
        struct Artist {
            name: String,
            #[serde(default)]
            relations: Vec<Relation>,
        }
        #[derive(Deserialize)]
        struct Relation {
            #[serde(rename = "type")]
            kind: String,
            url: Option<Resource>,
        }
        #[derive(Deserialize)]
        struct Resource {
            resource: String,
        }
        #[derive(Deserialize)]
        struct Browse {
            #[serde(rename = "release-groups", default)]
            release_groups: Vec<RgJson>,
        }
        let artist: Artist = self.get(&format!("{API}/artist/{id}?inc=url-rels&fmt=json"))?;
        let wikidata = artist.relations.iter().filter(|r| r.kind == "wikidata").find_map(|r| r.url.as_ref()).and_then(|u| u.resource.rsplit('/').next().map(str::to_owned));
        let image = match wikidata {
            Some(q) => self.wikidata_image(&q).unwrap_or_else(|e| {
                log::debug!("wikidata {q}: {e:#}");
                None
            }),
            None => None,
        };
        let browse: Browse = self.get(&format!("{API}/release-group?artist={id}&type=album|ep&limit=100&fmt=json"))?;
        // Studio albums, EPs and soundtracks: not every live recording, compilation,
        // mixtape and bootleg (Eminem has hundreds).
        let mut discography: Vec<ReleaseGroup> =
            browse.release_groups.into_iter().map(ReleaseGroup::from).filter(|rg| rg.secondary.iter().all(|s| s == "Soundtrack")).collect();
        discography.sort_by(|a, b| a.year.is_none().cmp(&b.year.is_none()).then_with(|| a.year.cmp(&b.year)));
        Ok(ArtistInfo { name: artist.name, image, discography })
    }

    /// The Commons image (P18) of a Wikidata item, as a 500px-wide URL.
    fn wikidata_image(&self, item: &str) -> Result<Option<String>> {
        let url = format!("https://www.wikidata.org/wiki/Special:EntityData/{item}.json");
        let data: serde_json::Value = self.agent.get(&url).header("User-Agent", USER_AGENT).call()?.body_mut().with_config().limit(20_000_000).read_json()?;
        let file = data["entities"][item]["claims"]["P18"][0]["mainsnak"]["datavalue"]["value"].as_str();
        Ok(file.map(|f| format!("https://commons.wikimedia.org/wiki/Special:FilePath/{}?width=500", encode(&f.replace(' ', "_")))))
    }
}

/// Lowercase, without punctuation and spacing, for comparing names.
fn normalise(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn similarity(a: &str, b: &str) -> f64 {
    let (a, b) = (normalise(a), normalise(b));
    if a.is_empty() || b.is_empty() {
        return if a == b { 1.0 } else { 0.0 };
    }
    strsim::normalized_levenshtein(&a, &b)
}

/// Picks the release group that best fits a local album, or none if nothing is close.
fn best_match(found: Vec<(ReleaseGroup, u32)>, title: &str, artist: Option<&str>, tracks: usize, year: Option<i32>) -> Option<ReleaseGroup> {
    // Which kind of release this many tracks suggests, best first.
    let kinds: &[&str] = match tracks {
        0..=3 => &["Single", "EP", "Album"],
        4..=6 => &["EP", "Album", "Single"],
        _ => &["Album", "EP", "Single"],
    };
    found
        .into_iter()
        .filter_map(|(rg, score)| {
            let title_sim = similarity(&rg.title, title);
            let artist_sim = match artist {
                Some(a) => rg.artists.iter().map(|(name, _)| similarity(name, a)).fold(0.0, f64::max).max(similarity(&credit(&rg), a)),
                None => 1.0,
            };
            if title_sim < MIN_TITLE || artist_sim < MIN_ARTIST {
                return None;
            }
            let kind_rank = rg.kind.as_deref().and_then(|k| kinds.iter().position(|x| *x == k)).unwrap_or(kinds.len());
            let year_off = match (year, rg.year) {
                (Some(a), Some(b)) => (a - b).unsigned_abs(),
                _ => 5,
            };
            // Closest names, then the right kind, then fewer "Live"/"Compilation" tags,
            // then the closest year, then MusicBrainz's own relevance.
            let key = (
                std::cmp::Reverse((title_sim * 100.0) as u32 + (artist_sim * 50.0) as u32),
                kind_rank,
                rg.secondary.len(),
                year_off,
                std::cmp::Reverse(score),
            );
            Some((key, rg))
        })
        .min_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, rg)| rg)
}

/// The whole artist credit as one name ("A & B").
fn credit(rg: &ReleaseGroup) -> String {
    rg.artists.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(" & ")
}

/// Escapes Lucene query syntax inside a quoted phrase.
fn lucene_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Percent-encodes a URL query or path value.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rg(id: &str, title: &str, kind: &str, year: i32, artist: &str) -> (ReleaseGroup, u32) {
        let rg = ReleaseGroup {
            id: id.into(),
            title: title.into(),
            kind: Some(kind.into()),
            secondary: Vec::new(),
            year: Some(year),
            artists: vec![(artist.into(), "a".into())],
        };
        (rg, 100)
    }

    #[test]
    fn prefers_the_album_over_a_single_of_the_same_name() {
        let found = vec![rg("single", "Hypnotize", "Single", 2005, "System of a Down"), rg("album", "Hypnotize", "Album", 2005, "System of a Down")];
        let best = best_match(found.clone(), "Hypnotize", Some("System Of A Down"), 12, Some(2005)).unwrap();
        assert_eq!(best.id, "album");
        let best = best_match(found, "Hypnotize", Some("System Of A Down"), 2, Some(2005)).unwrap();
        assert_eq!(best.id, "single");
    }

    #[test]
    fn rejects_different_names() {
        let found = vec![rg("x", "Mezmerize", "Album", 2005, "System of a Down")];
        assert!(best_match(found, "Hypnotize", Some("System of a Down"), 12, None).is_none());
        let found = vec![rg("x", "Hypnotize", "Album", 2005, "Someone Else")];
        assert!(best_match(found, "Hypnotize", Some("System of a Down"), 12, None).is_none());
    }

    #[test]
    fn ignores_punctuation_and_case() {
        let found = vec![rg("x", "momo-i quality ~Best of Momoi~", "Album", 2006, "桃井はるこ")];
        assert!(best_match(found, "momo-i quality -Best of Momoi-", None, 13, Some(2006)).is_some());
    }

    #[test]
    fn encodes_queries() {
        assert_eq!(encode("a b\"c"), "a%20b%22c");
        assert_eq!(lucene_escape("say \"hi\""), "say \\\"hi\\\"");
    }

    /// Asks MusicBrainz for real: `cargo test -p flex-core musicbrainz -- --ignored`.
    #[test]
    #[ignore]
    fn finds_an_album_and_its_artist() {
        let mb = MusicBrainz::new();
        let album = mb.find_album("Hypnotize", Some("System Of A Down"), 12, Some(2005)).unwrap().unwrap();
        assert_eq!(album.kind.as_deref(), Some("Album"));
        let artist = mb.artist(&album.artists[0].1).unwrap();
        assert!(artist.discography.iter().any(|r| r.id == album.id));
        assert!(artist.image.is_some(), "no photo for {}", artist.name);
    }
}
