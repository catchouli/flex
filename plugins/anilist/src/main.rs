//! Flex plugin for AniList (anilist.co): updates your list as you watch anime. Once an
//! episode is 80% through, the show's progress is set to that episode (never backwards),
//! and it's marked completed after the last one. Only files in Anime libraries count.
//!
//! Flex matches shows on TMDB, so the plugin finds each one on AniList itself: a search by
//! title and year for its first season, then AniList's "sequel" links for later seasons.
//! An episode number past an entry's length carries on into the next one, for series that
//! AniList splits into parts but releases number straight through.
//!
//! Signing in uses AniList's "pin" page: "Sign in" opens AniList in a browser on this PC,
//! and the token it shows is pasted into the Token setting. Tokens last a year and can't
//! be renewed, so the plugin says when one is about to run out.
//!
//! `FLEX_ANILIST_API` points it at another server (tests).

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flex_plugin::serde_json::{Value, json};
use flex_plugin::{Host, LogLevel, Media, Playback, PlaybackState, Plugin, Settings};

const API: &str = "https://graphql.anilist.co";
const AUTHORIZE: &str = "https://anilist.co/api/v2/oauth/authorize";
/// Flex's app on AniList (anilist.co/settings/developer), set up with the "pin" redirect
/// (https://anilist.co/api/v2/oauth/pin). Sign-in links carry it, so it isn't a secret.
const CLIENT_ID: &str = "54256";
const USER_AGENT: &str = concat!("flex/", env!("CARGO_PKG_VERSION"));
/// How far through an episode counts as watching it (as Trakt does).
const WATCHED: f64 = 0.8;
/// Say the sign-in is running out this long before it does.
const WARN_BEFORE_SECS: u64 = 14 * 86_400;
/// The formats a series' seasons come in.
const SERIES: [&str; 3] = ["TV", "TV_SHORT", "ONA"];
/// Longest wait for AniList's rate limit before giving up on an update.
const MAX_WAIT_SECS: u64 = 65;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Debug, PartialEq)]
enum Error {
    /// The token was refused (expired or revoked).
    SignedOut,
    Other(String),
}

/// An AniList entry: enough to follow a series along and update progress.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    id: i64,
    episodes: Option<i64>,
    format: String,
    year: Option<i32>,
    titles: Vec<String>,
    /// Sequels: (id, format, year).
    sequels: Vec<(i64, String, Option<i32>)>,
}

const ENTRY: &str = "fragment entry on Media {
  id episodes format startDate { year } title { romaji english native } synonyms
  relations { edges { relationType node { id type format startDate { year } } } }
}";

impl Entry {
    fn from_json(v: &Value) -> Option<Self> {
        let titles = ["romaji", "english", "native"]
            .iter()
            .filter_map(|k| v["title"][*k].as_str())
            .chain(v["synonyms"].as_array().into_iter().flatten().filter_map(Value::as_str))
            .map(str::to_owned)
            .collect();
        let sequels = v["relations"]["edges"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["relationType"] == "SEQUEL" && e["node"]["type"] == "ANIME")
            .filter_map(|e| {
                let n = &e["node"];
                Some((n["id"].as_i64()?, n["format"].as_str().unwrap_or_default().to_owned(), year(n)))
            })
            .collect();
        Some(Self {
            id: v["id"].as_i64()?,
            episodes: v["episodes"].as_i64().filter(|n| *n > 0),
            format: v["format"].as_str().unwrap_or_default().to_owned(),
            year: year(v),
            titles,
            sequels,
        })
    }

    /// A later part of the same season ("Part 2", "2nd Cour"), not a season of its own.
    fn is_part(&self) -> bool {
        self.titles.iter().any(|t| {
            let t = t.to_lowercase();
            let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
            words.contains(&"cour") || words.windows(2).any(|w| w[0] == "part" && !matches!(w[1], "1" | "i" | "one"))
        })
    }

    /// The next entry along: the earliest sequel in a series format.
    fn next(&self) -> Option<i64> {
        let mut sequels: Vec<_> = self.sequels.iter().filter(|(_, f, _)| SERIES.contains(&f.as_str())).collect();
        sequels.sort_by_key(|(_, _, y)| y.unwrap_or(i32::MAX));
        sequels.first().map(|(id, _, _)| *id)
    }
}

fn year(v: &Value) -> Option<i32> {
    v["startDate"]["year"].as_i64().map(|y| y as i32)
}

/// Lowercase letters and digits only, so "Journey's" and "Journey’s" compare equal.
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// When a token runs out (Unix time): the `exp` in its JWT payload.
fn expires_at(token: &str) -> Option<u64> {
    let payload = token.split('.').nth(1)?;
    let json: Value = flex_plugin::serde_json::from_slice(&base64url(payload)?).ok()?;
    json["exp"].as_u64()
}

fn base64url(s: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'-' | b'+' => Some(62),
        b'_' | b'/' => Some(63),
        _ => None,
    };
    let (mut out, mut bits, mut acc) = (Vec::new(), 0, 0u32);
    for c in s.bytes().filter(|c| *c != b'=') {
        acc = (acc << 6) | u32::from(value(c)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Opens a web page in this PC's browser.
fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.spawn().map(|_| ())
}

struct AniList {
    api: String,
    agent: ureq::Agent,
    settings: Settings,
    /// The token last checked, and whose it is.
    checked: Option<(String, Result<String, String>)>,
    /// Show (TMDB id, or title and year) to its first season's entry; None when AniList
    /// doesn't have it.
    shows: HashMap<String, Option<i64>>,
    entries: HashMap<i64, Entry>,
    /// Already said the sign-in is running out.
    warned: bool,
}

impl AniList {
    fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            api: std::env::var("FLEX_ANILIST_API").unwrap_or_else(|_| API.to_owned()),
            agent,
            settings: Settings::new(),
            checked: None,
            shows: HashMap::new(),
            entries: HashMap::new(),
            warned: false,
        }
    }

    fn text(&self, key: &str) -> String {
        self.settings.get(key).and_then(Value::as_str).unwrap_or_default().trim().to_owned()
    }

    fn token(&self) -> Option<String> {
        Some(self.text("token")).filter(|t| !t.is_empty())
    }

    fn client_id(&self) -> String {
        Some(self.text("client_id")).filter(|c| !c.is_empty()).unwrap_or_else(|| CLIENT_ID.to_owned())
    }

    /// A GraphQL request. Waits once for AniList's rate limit if it's hit.
    fn query(&self, query: &str, variables: Value, token: Option<&str>) -> Result<Value, Error> {
        let body = json!({ "query": query, "variables": variables });
        for attempt in 0..2 {
            let mut req = self.agent.post(&self.api).header("User-Agent", USER_AGENT).header("Accept", "application/json");
            if let Some(token) = token {
                req = req.header("Authorization", format!("Bearer {token}"));
            }
            let mut res = req.send_json(&body).map_err(|e| Error::Other(format!("AniList: {e}")))?;
            let status = res.status().as_u16();
            if status == 429 && attempt == 0 {
                let wait = res.headers().get("Retry-After").and_then(|v| v.to_str().ok()?.parse().ok()).unwrap_or(60);
                std::thread::sleep(Duration::from_secs(u64::min(wait, MAX_WAIT_SECS)));
                continue;
            }
            let v: Value = res.body_mut().read_json().unwrap_or(Value::Null);
            let refused = status == 401 || v["errors"].as_array().into_iter().flatten().any(|e| e["status"] == 401);
            if refused && token.is_some() {
                return Err(Error::SignedOut);
            }
            if status != 200 || v["errors"].is_array() {
                let message = v["errors"][0]["message"].as_str().unwrap_or("no answer");
                return Err(Error::Other(format!("AniList said {status}: {message}")));
            }
            return Ok(v["data"].clone());
        }
        Err(Error::Other("AniList is busy (rate limit)".into()))
    }

    fn entry(&mut self, id: i64) -> Result<Entry, Error> {
        if let Some(e) = self.entries.get(&id) {
            return Ok(e.clone());
        }
        let data = self.query(&format!("query($id: Int) {{ Media(id: $id, type: ANIME) {{ ...entry }} }} {ENTRY}"), json!({ "id": id }), None)?;
        let entry = Entry::from_json(&data["Media"]).ok_or_else(|| Error::Other(format!("AniList has no entry {id}")))?;
        self.entries.insert(id, entry.clone());
        Ok(entry)
    }

    /// The film, or a series' first season, on AniList: a search by title, kept when the
    /// year agrees (within one), preferring an exact title.
    fn first_entry(&mut self, media: &Media) -> Result<Option<i64>, Error> {
        let key = media.tmdb.map_or_else(|| format!("{}|{:?}", media.title, media.year), |id| format!("{}:{id}", media.kind));
        if let Some(found) = self.shows.get(&key) {
            return Ok(*found);
        }
        let formats: &[&str] = if media.kind == "movie" { &["MOVIE"] } else { &SERIES };
        let query = format!(
            "query($q: String, $f: [MediaFormat]) {{ Page(perPage: 8) {{ media(search: $q, type: ANIME, format_in: $f, sort: SEARCH_MATCH) {{ ...entry }} }} }} {ENTRY}"
        );
        let data = self.query(&query, json!({ "q": media.title, "f": formats }), None)?;
        let found: Vec<Entry> = data["Page"]["media"].as_array().into_iter().flatten().filter_map(Entry::from_json).collect();
        let year_ok = |e: &Entry| match (media.year, e.year) {
            (Some(a), Some(b)) => (a - b).abs() <= 1,
            _ => true,
        };
        let title = norm(&media.title);
        let exact = found.iter().find(|e| year_ok(e) && e.titles.iter().any(|t| norm(t) == title));
        // Otherwise only AniList's best match, and only if the year agrees.
        let pick = exact.or(found.first().filter(|e| year_ok(e)));
        for e in &found {
            self.entries.insert(e.id, e.clone());
        }
        let id = pick.map(|e| e.id);
        self.shows.insert(key, id);
        Ok(id)
    }

    /// The AniList entry and its episode number for what was watched, and the entry's
    /// length. None when it isn't on AniList (or is a special).
    fn locate(&mut self, media: &Media) -> Result<Option<(i64, i64, Option<i64>)>, Error> {
        let Some(first) = self.first_entry(media)? else { return Ok(None) };
        if media.kind == "movie" {
            let entry = self.entry(first)?;
            return Ok(Some((entry.id, 1, entry.episodes.or(Some(1)))));
        }
        let (Some(season), Some(episode)) = (media.season, media.episode) else { return Ok(None) };
        if season < 1 || episode < 1 {
            return Ok(None);
        }
        let mut entry = self.entry(first)?;
        for _ in 1..season {
            // The next season, past any further parts of this one.
            loop {
                let Some(next) = entry.next() else { return Ok(None) };
                entry = self.entry(next)?;
                if !entry.is_part() {
                    break;
                }
            }
        }
        let mut episode = i64::from(episode);
        while let Some(total) = entry.episodes.filter(|t| episode > *t) {
            let Some(next) = entry.next() else { break };
            episode -= total;
            entry = self.entry(next)?;
        }
        Ok(Some((entry.id, episode, entry.episodes)))
    }

    /// Sets the progress for a watched episode, unless the list is already past it.
    /// Returns what changed, if anything.
    fn update(&mut self, media: &Media, token: &str) -> Result<Option<String>, Error> {
        let Some((id, episode, total)) = self.locate(media)? else { return Ok(None) };
        let query = "query($id: Int) { Media(id: $id) { title { romaji } mediaListEntry { status progress } } }";
        let data = self.query(query, json!({ "id": id }), Some(token))?;
        let list = &data["Media"]["mediaListEntry"];
        let (status, progress) = (list["status"].as_str().unwrap_or_default(), list["progress"].as_i64().unwrap_or(0));
        // Finished already, or further along: leave it (a rewatch, or watched elsewhere).
        if status == "COMPLETED" || progress >= episode {
            return Ok(None);
        }
        let done = total == Some(episode);
        let mutation = "mutation($id: Int, $p: Int, $s: MediaListStatus) { SaveMediaListEntry(mediaId: $id, progress: $p, status: $s) { progress status } }";
        let status = if done { "COMPLETED" } else { "CURRENT" };
        self.query(mutation, json!({ "id": id, "p": episode, "s": status }), Some(token))?;
        let name = data["Media"]["title"]["romaji"].as_str().unwrap_or(&media.title).to_owned();
        Ok(Some(if done { format!("{name}: completed") } else { format!("{name}: episode {episode}") }))
    }

    /// Checks the token (once per token) and says who's signed in and for how long.
    fn show_status(&mut self, host: &Host) {
        let Some(token) = self.token() else {
            self.checked = None;
            host.status(true, "Not signed in: press Sign in");
            return;
        };
        if self.checked.as_ref().is_none_or(|(t, _)| *t != token) {
            let who = match self.query("query { Viewer { name } }", json!({}), Some(&token)) {
                Ok(data) => Ok(data["Viewer"]["name"].as_str().unwrap_or("?").to_owned()),
                Err(Error::SignedOut) => Err("That token doesn't work (expired?): sign in again".to_owned()),
                Err(Error::Other(e)) => Err(e),
            };
            self.checked = Some((token.clone(), who));
        }
        let left = expires_at(&token).map(|exp| exp.saturating_sub(now()));
        match &self.checked {
            Some((_, Ok(name))) => match left {
                Some(0) => host.status(false, "Your AniList sign-in has run out: sign in again"),
                Some(secs) if secs < WARN_BEFORE_SECS => {
                    let text = format!("Signed in as {name} · runs out in {} days: sign in again soon", secs.div_ceil(86_400));
                    if !std::mem::replace(&mut self.warned, true) {
                        host.toast(format!("AniList: {text}"));
                    }
                    host.status(false, text);
                }
                _ => host.status(true, format!("Signed in as {name}")),
            },
            Some((_, Err(e))) => host.status(false, e.clone()),
            None => {}
        }
    }

    fn sign_in(&self, host: &Host) {
        let url = format!("{AUTHORIZE}?client_id={}&response_type=token", self.client_id());
        match open_browser(&url) {
            Ok(()) => host.status(true, "Approve Flex on AniList, then paste the token it shows into Token"),
            Err(e) => {
                host.log(LogLevel::Warn, format!("opening the browser: {e}"));
                host.status(false, format!("Open {url} in a browser, then paste the token into Token"));
            }
        }
    }
}

impl Plugin for AniList {
    fn hello(&mut self, host: &Host, settings: &Settings) {
        self.settings = settings.clone();
        self.show_status(host);
    }

    fn settings(&mut self, host: &Host, settings: &Settings) {
        self.settings = settings.clone();
        self.show_status(host);
    }

    fn action(&mut self, host: &Host, id: &str) {
        match id {
            "sign_in" => self.sign_in(host),
            "sign_out" => {
                let mut s = Settings::new();
                s.insert("token".into(), json!(""));
                host.set(s);
                self.settings.insert("token".into(), json!(""));
                self.show_status(host);
                host.toast("Signed out of AniList");
            }
            _ => {}
        }
    }

    fn playback(&mut self, host: &Host, playback: &Playback) {
        let enabled = self.settings.get("sync").and_then(Value::as_bool).unwrap_or(true);
        let watched = match (playback.position, playback.duration) {
            (Some(pos), Some(dur)) if dur > 0.0 => pos / dur >= WATCHED,
            _ => false,
        };
        if !enabled || playback.state != PlaybackState::Stopped || playback.kind != "video" || !watched {
            return;
        }
        let Some(media) = playback.media.as_ref().filter(|m| m.anime) else { return };
        let Some(token) = self.token() else { return };
        match self.update(media, &token) {
            Ok(Some(change)) => {
                host.log(LogLevel::Info, format!("updated {change}"));
                host.status(true, format!("Updated {change}"));
            }
            Ok(None) => host.log(LogLevel::Debug, format!("nothing to update for {:?}", media)),
            Err(Error::SignedOut) => {
                host.status(false, "Your AniList sign-in has run out: sign in again");
                host.toast("AniList: your sign-in has run out. Sign in again in Settings → Plugins → AniList");
            }
            Err(Error::Other(e)) => host.log(LogLevel::Warn, e),
        }
    }
}

fn main() {
    flex_plugin::run(&mut AniList::new());
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::{Arc, Mutex};

    use super::*;

    /// A token whose payload says it runs out at `exp`.
    fn token(exp: u64) -> String {
        let payload = format!(r#"{{"aud":"54256","exp":{exp}}}"#);
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let bytes = payload.as_bytes();
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
            for i in 0..=chunk.len() {
                out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
            }
        }
        format!("eyJ0eXAiOiJKV1QifQ.{out}.sig")
    }

    /// Real AniList: `cargo test -p flex-anilist anilist_online -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn anilist_online() {
        let mut a = AniList { api: API.to_owned(), ..AniList::new() };
        let cases = [
            ("Frieren: Beyond Journey's End", 2023, 1, 5),
            ("Mob Psycho 100", 2016, 2, 3),
            ("Attack on Titan", 2013, 4, 20),
            ("Spy x Family", 2022, 1, 20),
            ("Breaking Bad", 2008, 1, 1),
        ];
        for (title, year, season, episode) in cases {
            let found = a.locate(&media(title, year, season, episode)).unwrap();
            let name = found.and_then(|(id, _, _)| a.entries.get(&id).map(|e| e.titles[0].clone()));
            eprintln!("{title} S{season}E{episode} -> {found:?} {name:?}");
        }
        let movie = Media { kind: "movie".into(), title: "Your Name.".into(), year: Some(2016), anime: true, ..Media::default() };
        eprintln!("Your Name. -> {:?}", a.locate(&movie).unwrap());
    }

    #[test]
    fn reads_token_expiry() {
        assert_eq!(expires_at(&token(1_800_000_000)), Some(1_800_000_000));
        assert_eq!(expires_at("not a token"), None);
        assert_eq!(norm("Frieren: Beyond Journey’s End"), norm("frieren beyond journey's end"));
        let titled = |t: &str| Entry { id: 1, episodes: None, format: "TV".into(), year: None, titles: vec![t.into()], sequels: vec![] };
        assert!(titled("Shingeki no Kyojin Season 3 Part 2").is_part());
        assert!(titled("Spy x Family Part 2").is_part());
        assert!(titled("Jujutsu Kaisen: 2nd Cour").is_part());
        assert!(!titled("Mob Psycho 100 II").is_part());
        assert!(!titled("Kingdom Part 1").is_part());
        assert!(!titled("Party Kindergarten").is_part());
    }

    fn media(title: &str, year: i32, season: i32, episode: i32) -> Media {
        Media { kind: "episode".into(), title: title.into(), year: Some(year), tmdb: None, season: Some(season), episode: Some(episode), anime: true, ..Media::default() }
    }

    /// Requests the fake AniList got: (Authorization header, body).
    type Requests = Arc<Mutex<Vec<(String, Value)>>>;

    /// A pretend AniList: a show with a 12-episode season 1, a 2-part season 2 (12 + 11)
    /// and a film sequel; "Wrong" is a 2010 show; the list has the show at episode 4.
    /// "Bearer old" is refused.
    fn fake_anilist() -> (String, Requests) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let log: Requests = Arc::default();
        let seen = Arc::clone(&log);
        let node = |id: i64, format: &str, year: i32| json!({ "relationType": "SEQUEL", "node": { "id": id, "type": "ANIME", "format": format, "startDate": { "year": year } } });
        let entry = move |id: i64, eps: i64, year: i32, title: &str, sequels: Vec<Value>| {
            let english = (id == 1).then_some("Frieren: Beyond Journey’s End");
            json!({ "id": id, "episodes": eps, "format": "TV", "startDate": { "year": year }, "title": { "romaji": title, "english": english, "native": null }, "synonyms": [], "relations": { "edges": sequels } })
        };
        let entries: HashMap<i64, Value> = [
            (1, entry(1, 12, 2023, "Sousou no Frieren", vec![node(2, "TV", 2026), node(9, "MOVIE", 2025)])),
            (2, entry(2, 12, 2026, "Sousou no Frieren 2", vec![node(3, "TV", 2026)])),
            (3, entry(3, 11, 2026, "Sousou no Frieren 2 Part 2", vec![node(4, "TV", 2027)])),
            (4, entry(4, 12, 2027, "Sousou no Frieren 3", vec![])),
            (8, entry(8, 24, 2010, "Wrong", vec![])),
        ]
        .into_iter()
        .collect();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let (mut length, mut auth) = (0, String::new());
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap_or_default();
                    match k.trim().to_ascii_lowercase().as_str() {
                        "content-length" => length = v.trim().parse().unwrap_or(0),
                        "authorization" => auth = v.trim().to_owned(),
                        _ => {}
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let body: Value = flex_plugin::serde_json::from_slice(&body).unwrap();
                seen.lock().unwrap().push((auth.clone(), body.clone()));
                let q = body["query"].as_str().unwrap_or_default();
                let vars = &body["variables"];
                let (status, answer) = if auth == "Bearer old" {
                    (401, json!({ "errors": [{ "message": "Invalid token", "status": 401 }] }))
                } else if q.contains("Viewer") {
                    (200, json!({ "data": { "Viewer": { "name": "cat" } } }))
                } else if q.contains("Page(") {
                    let found: Vec<Value> = match vars["q"].as_str() {
                        Some("Frieren: Beyond Journey's End") => vec![entries[&8].clone(), entries[&1].clone()],
                        Some("Wrong") => vec![entries[&8].clone()],
                        _ => vec![],
                    };
                    (200, json!({ "data": { "Page": { "media": found } } }))
                } else if q.contains("SaveMediaListEntry") {
                    (200, json!({ "data": { "SaveMediaListEntry": { "progress": vars["p"], "status": vars["s"] } } }))
                } else if q.contains("mediaListEntry") {
                    let list = if vars["id"] == 1 { json!({ "status": "CURRENT", "progress": 4 }) } else { Value::Null };
                    (200, json!({ "data": { "Media": { "title": { "romaji": "Sousou no Frieren" }, "mediaListEntry": list } } }))
                } else {
                    let id = vars["id"].as_i64().unwrap_or(0);
                    (200, json!({ "data": { "Media": entries.get(&id).cloned().unwrap_or(Value::Null) } }))
                };
                let text = answer.to_string();
                let mut out = conn;
                let _ = write!(out, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
            }
        });
        (base, log)
    }

    fn plugin(base: String) -> AniList {
        AniList { api: base, ..AniList::new() }
    }

    #[test]
    fn follows_sequels_and_parts() {
        let (base, _) = fake_anilist();
        let mut a = plugin(base);
        let show = "Frieren: Beyond Journey's End";
        // "Wrong" comes first in the search but is from 2010; the exact title wins anyway.
        assert_eq!(a.locate(&media(show, 2023, 1, 3)).unwrap(), Some((1, 3, Some(12))));
        // Season 2 is the TV sequel (not the film), and episode 15 is part 2's third.
        assert_eq!(a.locate(&media(show, 2023, 2, 5)).unwrap(), Some((2, 5, Some(12))));
        assert_eq!(a.locate(&media(show, 2023, 2, 15)).unwrap(), Some((3, 3, Some(11))));
        // Season 3 comes after season 2's part 2.
        assert_eq!(a.locate(&media(show, 2023, 3, 2)).unwrap(), Some((4, 2, Some(12))));
        // Specials, missing seasons and shows AniList doesn't have.
        assert_eq!(a.locate(&media(show, 2023, 0, 1)).unwrap(), None);
        assert_eq!(a.locate(&media(show, 2023, 5, 1)).unwrap(), None);
        assert_eq!(a.locate(&media("Wrong", 2023, 1, 1)).unwrap(), None, "the year doesn't agree");
        assert_eq!(a.locate(&media("Nothing", 2023, 1, 1)).unwrap(), None);
    }

    #[test]
    fn updates_progress_without_going_backwards() {
        let (base, log) = fake_anilist();
        let host = Host::new(std::io::sink());
        let mut a = plugin(base);
        let mut settings = Settings::new();
        settings.insert("token".into(), json!(token(now() + 300 * 86_400)));
        a.hello(&host, &settings);
        let stopped = |m: Media, pos: f64| Playback { state: PlaybackState::Stopped, kind: "video".into(), position: Some(pos), duration: Some(1440.0), media: Some(m), ..Playback::default() };
        let show = "Frieren: Beyond Journey's End";

        a.playback(&host, &stopped(media(show, 2023, 1, 3), 1400.0)); // the list is at 4 already
        a.playback(&host, &stopped(media(show, 2023, 1, 6), 600.0)); // not 80% through
        a.playback(&host, &stopped(Media { anime: false, ..media(show, 2023, 1, 6) }, 1400.0)); // not an Anime library
        a.playback(&host, &stopped(media(show, 2023, 1, 6), 1400.0));
        a.playback(&host, &stopped(media(show, 2023, 2, 23), 1440.0)); // part 2's last episode
        let saves: Vec<Value> = log.lock().unwrap().iter().filter(|(_, b)| b["query"].as_str().unwrap().contains("SaveMediaListEntry")).map(|(_, b)| b["variables"].clone()).collect();
        assert_eq!(saves, [json!({ "id": 1, "p": 6, "s": "CURRENT" }), json!({ "id": 3, "p": 11, "s": "COMPLETED" })]);
        assert!(log.lock().unwrap().iter().filter(|(_, b)| b["query"].as_str().unwrap().contains("mediaListEntry")).all(|(auth, _)| auth.starts_with("Bearer ")));
    }

    #[test]
    fn says_when_the_sign_in_runs_out() {
        let (base, _) = fake_anilist();
        let out = Arc::new(Mutex::new(Vec::new()));
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().write(buf)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let host = Host::new(Sink(Arc::clone(&out)));
        let mut a = plugin(base);
        let mut settings = Settings::new();
        settings.insert("token".into(), json!(token(now() + 5 * 86_400)));
        a.hello(&host, &settings);
        settings.insert("token".into(), json!("old"));
        a.settings(&host, &settings);
        let text = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        assert!(text.contains("Signed in as cat · runs out in 5 days"), "{text}");
        assert!(text.contains(r#""type":"toast""#), "warned on screen: {text}");
        assert!(text.contains("doesn't work (expired?)"), "{text}");
    }
}
