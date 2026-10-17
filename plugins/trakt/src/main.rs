//! Flex plugin for Trakt (trakt.tv): "scrobbles" what you watch, so your Trakt history and
//! progress follow Flex. Only films and episodes Flex has matched online are sent (TMDB
//! ids, or title and year for anime matched on AniList).
//!
//! Connecting uses Trakt's device sign-in: the plugin shows a code to enter at
//! trakt.tv/activate on a phone, so nothing has to be typed on the TV.
//!
//! Trakt needs a registered app (an ID and secret, from trakt.tv/oauth/applications).
//! Release builds bring their own (`FLEX_TRAKT_CLIENT_ID`/`_SECRET` at build time); the
//! plugin's settings can give others. `FLEX_TRAKT_API` points it at another server (tests).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flex_plugin::serde_json::{Value, json};
use flex_plugin::{Host, LogLevel, Media, Playback, PlaybackState, Plugin, Settings};

const API: &str = "https://api.trakt.tv";
/// Trakt's "no redirect" value, for apps without a web page to return to.
const REDIRECT: &str = "urn:ietf:wg:oauth:2.0:oob";
const USER_AGENT: &str = concat!("flex/", env!("CARGO_PKG_VERSION"));
/// Refresh the sign-in when it has less than this left.
const REFRESH_BEFORE_SECS: u64 = 24 * 3600;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A signed-in Trakt account.
#[derive(Clone, Debug, Default, PartialEq)]
struct Auth {
    access: String,
    refresh: String,
    /// Unix time.
    expires_at: u64,
    username: String,
}

impl Auth {
    /// From a token answer (`/oauth/device/token` or `/oauth/token`).
    fn from_token(v: &Value) -> Option<Self> {
        let created = v["created_at"].as_u64().unwrap_or_else(now);
        Some(Self {
            access: v["access_token"].as_str()?.to_owned(),
            refresh: v["refresh_token"].as_str().unwrap_or_default().to_owned(),
            expires_at: created + v["expires_in"].as_u64().unwrap_or(0),
            username: String::new(),
        })
    }

    fn from_settings(s: &Settings) -> Option<Self> {
        let text = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let access = text("access_token");
        (!access.is_empty()).then(|| Self {
            access,
            refresh: text("refresh_token"),
            expires_at: s.get("expires_at").and_then(Value::as_u64).unwrap_or(0),
            username: text("username"),
        })
    }

    /// For saving in Flex's settings (empty = signed out).
    fn to_settings(auth: Option<&Self>) -> Settings {
        let a = auth.cloned().unwrap_or_default();
        let mut s = Settings::new();
        s.insert("access_token".into(), json!(a.access));
        s.insert("refresh_token".into(), json!(a.refresh));
        s.insert("expires_at".into(), json!(a.expires_at));
        s.insert("username".into(), json!(a.username));
        s
    }
}

/// Talks to Trakt's API as one registered app.
#[derive(Clone)]
struct Client {
    base: String,
    id: String,
    secret: String,
}

struct DeviceCode {
    device_code: String,
    user_code: String,
    url: String,
    interval: u64,
    expires_in: u64,
}

enum Poll {
    Done(Auth),
    Pending,
    SlowDown,
}

impl Client {
    fn agent() -> ureq::Agent {
        ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).http_status_as_error(false).build().into()
    }

    /// Sends a request; returns the status and the JSON answer (Null if none).
    fn call(&self, method: &str, path: &str, body: Option<&Value>, token: Option<&str>) -> Result<(u16, Value), String> {
        let url = format!("{}{path}", self.base);
        let agent = Self::agent();
        let auth = token.map(|t| format!("Bearer {t}"));
        let result = if method == "GET" {
            let mut req = agent.get(&url).header("trakt-api-version", "2").header("trakt-api-key", &self.id).header("User-Agent", USER_AGENT);
            if let Some(a) = &auth {
                req = req.header("Authorization", a);
            }
            req.call()
        } else {
            let mut req = agent
                .post(&url)
                .header("Content-Type", "application/json")
                .header("trakt-api-version", "2")
                .header("trakt-api-key", &self.id)
                .header("User-Agent", USER_AGENT);
            if let Some(a) = &auth {
                req = req.header("Authorization", a);
            }
            req.send(body.map(Value::to_string).unwrap_or_default())
        };
        let mut response = result.map_err(|e| format!("couldn't reach Trakt: {e}"))?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        Ok((status, flex_plugin::serde_json::from_str(&text).unwrap_or(Value::Null)))
    }

    fn device_code(&self) -> Result<DeviceCode, String> {
        let (status, v) = self.call("POST", "/oauth/device/code", Some(&json!({ "client_id": self.id })), None)?;
        if status != 200 {
            return Err(format!("Trakt said {status} (is the app ID right?)"));
        }
        Ok(DeviceCode {
            device_code: v["device_code"].as_str().unwrap_or_default().to_owned(),
            user_code: v["user_code"].as_str().unwrap_or_default().to_owned(),
            url: v["verification_url"].as_str().unwrap_or("https://trakt.tv/activate").to_owned(),
            interval: v["interval"].as_u64().unwrap_or(5),
            expires_in: v["expires_in"].as_u64().unwrap_or(600),
        })
    }

    fn poll_token(&self, device_code: &str) -> Result<Poll, String> {
        let body = json!({ "code": device_code, "client_id": self.id, "client_secret": self.secret });
        let (status, v) = self.call("POST", "/oauth/device/token", Some(&body), None)?;
        match status {
            200 => Auth::from_token(&v).map(Poll::Done).ok_or_else(|| "Trakt sent no token".to_owned()),
            400 => Ok(Poll::Pending),
            429 => Ok(Poll::SlowDown),
            404 => Err("Trakt didn't recognise the code".into()),
            409 => Err("That code was already used".into()),
            410 => Err("The code expired: press Connect again".into()),
            418 => Err("Connecting was declined on Trakt".into()),
            other => Err(format!("Trakt said {other} (is the app secret right?)")),
        }
    }

    fn refresh(&self, auth: &Auth) -> Result<Auth, String> {
        let body = json!({
            "refresh_token": auth.refresh, "client_id": self.id, "client_secret": self.secret,
            "redirect_uri": REDIRECT, "grant_type": "refresh_token",
        });
        match self.call("POST", "/oauth/token", Some(&body), None)? {
            (200, v) => {
                let fresh = Auth::from_token(&v).ok_or("Trakt sent no token")?;
                Ok(Auth { username: auth.username.clone(), ..fresh })
            }
            (status, _) => Err(format!("Trakt wouldn't renew the sign-in ({status}): press Connect again")),
        }
    }

    fn username(&self, token: &str) -> Option<String> {
        let (status, v) = self.call("GET", "/users/settings", None, Some(token)).ok()?;
        (status == 200).then(|| v["user"]["username"].as_str().unwrap_or_default().to_owned())
    }

    fn revoke(&self, auth: &Auth) {
        let body = json!({ "token": auth.access, "client_id": self.id, "client_secret": self.secret });
        let _ = self.call("POST", "/oauth/revoke", Some(&body), None);
    }
}

/// What to send Trakt for a film or episode `progress` percent through.
fn scrobble_body(media: &Media, progress: f64) -> Option<Value> {
    let mut ids = json!({});
    if let Some(tmdb) = media.tmdb {
        ids["tmdb"] = json!(tmdb);
    }
    let item = json!({ "title": media.title, "year": media.year, "ids": ids });
    match media.kind.as_str() {
        "movie" => Some(json!({ "movie": item, "progress": progress })),
        "episode" => Some(json!({
            "show": item,
            "episode": { "season": media.season?, "number": media.episode? },
            "progress": progress,
        })),
        _ => None,
    }
}

/// Trakt's endpoint for a playback state.
fn endpoint(state: PlaybackState) -> &'static str {
    match state {
        PlaybackState::Playing => "/scrobble/start",
        PlaybackState::Paused => "/scrobble/pause",
        PlaybackState::Stopped => "/scrobble/stop",
    }
}

/// How far through, in percent.
fn progress(p: &Playback) -> f64 {
    match (p.position, p.duration) {
        (Some(pos), Some(dur)) if dur > 0.0 => ((pos / dur * 100.0).clamp(0.0, 100.0) * 100.0).round() / 100.0,
        _ => 0.0,
    }
}

struct Trakt {
    settings: Settings,
    auth: Arc<Mutex<Option<Auth>>>,
    connecting: Arc<AtomicBool>,
    base: String,
}

impl Trakt {
    fn new() -> Self {
        let base = std::env::var("FLEX_TRAKT_API").unwrap_or_else(|_| API.to_owned());
        Self { settings: Settings::new(), auth: Arc::default(), connecting: Arc::default(), base }
    }

    /// The app's ID and secret: from the settings, else built in.
    fn client(&self) -> Option<Client> {
        let setting = |k: &str| self.settings.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned);
        let built_in = |v: Option<&'static str>| v.filter(|v| !v.is_empty()).map(str::to_owned);
        let id = setting("client_id").or_else(|| built_in(option_env!("FLEX_TRAKT_CLIENT_ID")))?;
        let secret = setting("client_secret").or_else(|| built_in(option_env!("FLEX_TRAKT_CLIENT_SECRET")))?;
        Some(Client { base: self.base.clone(), id, secret })
    }

    fn auth(&self) -> Option<Auth> {
        self.auth.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_auth(&self, host: &Host, auth: Option<Auth>) {
        host.set(Auth::to_settings(auth.as_ref()));
        *self.auth.lock().unwrap_or_else(|e| e.into_inner()) = auth;
    }

    fn show_status(&self, host: &Host) {
        match (self.auth(), self.client()) {
            (Some(a), _) if !a.username.is_empty() => host.status(true, format!("Connected as {}", a.username)),
            (Some(_), _) => host.status(true, "Connected"),
            (None, None) => host.status(false, "Needs a Trakt app ID and secret (below)"),
            (None, Some(_)) => host.status(true, "Not connected: press Connect to Trakt"),
        }
    }

    /// The sign-in, renewed first if it's about to expire.
    fn fresh_auth(&self, host: &Host, client: &Client) -> Option<Auth> {
        let auth = self.auth()?;
        if auth.expires_at > now() + REFRESH_BEFORE_SECS {
            return Some(auth);
        }
        self.renew(host, client, &auth)
    }

    fn renew(&self, host: &Host, client: &Client, auth: &Auth) -> Option<Auth> {
        match client.refresh(auth) {
            Ok(fresh) => {
                self.set_auth(host, Some(fresh.clone()));
                Some(fresh)
            }
            Err(e) => {
                host.status(false, e);
                None
            }
        }
    }

    fn connect(&self, host: &Host) {
        let Some(client) = self.client() else {
            host.toast("Enter a Trakt app ID and secret first");
            return;
        };
        if self.connecting.swap(true, Ordering::SeqCst) {
            return;
        }
        let (host, auth, connecting) = (host.clone(), Arc::clone(&self.auth), Arc::clone(&self.connecting));
        std::thread::spawn(move || {
            if let Err(e) = device_flow(&client, &host, &auth) {
                host.status(false, e);
            }
            connecting.store(false, Ordering::SeqCst);
        });
    }
}

/// Shows a code to enter on trakt.tv, then waits for it to be approved.
fn device_flow(client: &Client, host: &Host, auth: &Mutex<Option<Auth>>) -> Result<(), String> {
    let code = client.device_code()?;
    let prompt = format!("Go to {} and enter {}", code.url.trim_start_matches("https://"), code.user_code);
    host.status(true, &prompt);
    host.toast(&prompt);
    let deadline = now() + code.expires_in;
    let mut interval = code.interval.max(1);
    while now() < deadline {
        std::thread::sleep(Duration::from_secs(interval));
        match client.poll_token(&code.device_code)? {
            Poll::Done(mut fresh) => {
                fresh.username = client.username(&fresh.access).unwrap_or_default();
                host.set(Auth::to_settings(Some(&fresh)));
                let name = fresh.username.clone();
                *auth.lock().unwrap_or_else(|e| e.into_inner()) = Some(fresh);
                host.status(true, format!("Connected as {name}"));
                host.toast(format!("Connected to Trakt as {name}"));
                return Ok(());
            }
            Poll::Pending => {}
            Poll::SlowDown => interval += 1,
        }
    }
    Err("The code expired: press Connect again".into())
}

impl Plugin for Trakt {
    fn hello(&mut self, host: &Host, settings: &Settings) {
        self.settings = settings.clone();
        *self.auth.lock().unwrap_or_else(|e| e.into_inner()) = Auth::from_settings(settings);
        self.show_status(host);
    }

    fn settings(&mut self, host: &Host, settings: &Settings) {
        self.settings = settings.clone();
        self.show_status(host);
    }

    fn action(&mut self, host: &Host, id: &str) {
        match id {
            "connect" => self.connect(host),
            "disconnect" => {
                if let (Some(client), Some(auth)) = (self.client(), self.auth()) {
                    client.revoke(&auth);
                }
                self.set_auth(host, None);
                self.show_status(host);
                host.toast("Disconnected from Trakt");
            }
            _ => {}
        }
    }

    fn playback(&mut self, host: &Host, playback: &Playback) {
        let enabled = self.settings.get("scrobble").and_then(Value::as_bool).unwrap_or(true);
        let Some(media) = playback.media.as_ref().filter(|_| enabled && playback.kind == "video") else { return };
        let (Some(client), Some(body)) = (self.client(), scrobble_body(media, progress(playback))) else { return };
        let Some(mut auth) = self.fresh_auth(host, &client) else { return };
        let path = endpoint(playback.state);
        for attempt in 0..2 {
            match client.call("POST", path, Some(&body), Some(&auth.access)) {
                Ok((201, _)) => {
                    host.log(LogLevel::Info, format!("{path}: {} at {:.0}%", media.title, progress(playback)));
                    return;
                }
                // Already sent recently: fine.
                Ok((409, _)) => return,
                Ok((401, _)) if attempt == 0 => match self.renew(host, &client, &auth) {
                    Some(fresh) => auth = fresh,
                    None => return,
                },
                Ok((404, _)) => {
                    host.log(LogLevel::Warn, format!("Trakt doesn't know {} ({:?})", media.title, media));
                    return;
                }
                Ok((status, v)) => {
                    host.log(LogLevel::Warn, format!("{path}: Trakt said {status}: {v}"));
                    return;
                }
                Err(e) => {
                    host.log(LogLevel::Warn, e);
                    return;
                }
            }
        }
    }
}

fn main() {
    flex_plugin::run(&mut Trakt::new());
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{Ipv4Addr, TcpListener};

    use super::*;

    fn episode() -> Media {
        Media { kind: "episode".into(), title: "Frieren".into(), year: Some(2023), anilist: Some(154587), season: Some(1), episode: Some(3), ..Media::default() }
    }

    #[test]
    fn builds_scrobbles() {
        let film = Media { kind: "movie".into(), title: "Paprika".into(), year: Some(2006), tmdb: Some(4977), ..Media::default() };
        assert_eq!(
            scrobble_body(&film, 12.5).unwrap(),
            json!({ "movie": { "title": "Paprika", "year": 2006, "ids": { "tmdb": 4977 } }, "progress": 12.5 })
        );
        // Anime matched on AniList: Trakt goes by title and year.
        assert_eq!(
            scrobble_body(&episode(), 90.0).unwrap(),
            json!({ "show": { "title": "Frieren", "year": 2023, "ids": {} }, "episode": { "season": 1, "number": 3 }, "progress": 90.0 })
        );
        assert!(scrobble_body(&Media { episode: None, ..episode() }, 1.0).is_none(), "an episode needs its number");
        let p = Playback { position: Some(300.0), duration: Some(1200.0), ..Playback::default() };
        assert_eq!(progress(&p), 25.0);
        assert_eq!(progress(&Playback::default()), 0.0);
        assert_eq!(endpoint(PlaybackState::Stopped), "/scrobble/stop");
    }

    /// A pretend Trakt: answers by path, the device token on the second try, and refuses
    /// the first scrobble with an expired token. Records (path, Authorization, body).
    /// Requests the fake Trakt got: (path, Authorization header, body).
    type Requests = Arc<Mutex<Vec<(String, String, Value)>>>;

    fn fake_trakt() -> (String, Requests) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let log: Requests = Arc::default();
        let seen = Arc::clone(&log);
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or_default().to_owned();
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
                let body: Value = flex_plugin::serde_json::from_slice(&body).unwrap_or(Value::Null);
                let tries = seen.lock().unwrap().iter().filter(|(p, _, _)| *p == path).count();
                seen.lock().unwrap().push((path.clone(), auth.clone(), body));
                let (status, answer) = match path.as_str() {
                    "/oauth/device/code" => (200, json!({ "device_code": "dev1", "user_code": "ABCD1234", "verification_url": "https://trakt.tv/activate", "expires_in": 600, "interval": 1 })),
                    "/oauth/device/token" if tries == 0 => (400, Value::Null),
                    "/oauth/device/token" => (200, json!({ "access_token": "a1", "refresh_token": "r1", "expires_in": 7_776_000, "created_at": now() })),
                    "/oauth/token" => (200, json!({ "access_token": "a2", "refresh_token": "r2", "expires_in": 7_776_000, "created_at": now() })),
                    "/users/settings" => (200, json!({ "user": { "username": "cat" } })),
                    "/scrobble/start" if auth == "Bearer a1" => (401, Value::Null),
                    p if p.starts_with("/scrobble/") => (201, json!({})),
                    _ => (404, Value::Null),
                };
                let text = if answer.is_null() { String::new() } else { answer.to_string() };
                let reason = if status < 300 { "OK" } else { "Nope" };
                let mut out = conn;
                let _ = write!(out, "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
            }
        });
        (base, log)
    }

    #[test]
    fn connects_and_scrobbles() {
        let (base, log) = fake_trakt();
        let messages = Arc::new(Mutex::new(Vec::new()));
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().write(buf)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let host = Host::new(Sink(Arc::clone(&messages)));
        let mut trakt = Trakt { base, ..Trakt::new() };
        let mut settings = Settings::new();
        settings.insert("client_id".into(), json!("id1"));
        settings.insert("client_secret".into(), json!("secret1"));
        trakt.hello(&host, &settings);

        // Connecting: the code is shown, the token comes on the second poll.
        let client = trakt.client().unwrap();
        device_flow(&client, &host, &trakt.auth).unwrap();
        assert_eq!(trakt.auth().unwrap().access, "a1");
        assert_eq!(trakt.auth().unwrap().username, "cat");
        let out = String::from_utf8(messages.lock().unwrap().clone()).unwrap();
        assert!(out.contains("Go to trakt.tv/activate and enter ABCD1234"), "{out}");
        assert!(out.contains(r#""access_token":"a1""#), "saved in Flex's settings: {out}");

        // Playing: a1 is refused, so the sign-in is renewed and the scrobble retried.
        let playing = Playback { state: PlaybackState::Playing, kind: "video".into(), position: Some(60.0), duration: Some(1440.0), media: Some(episode()), ..Playback::default() };
        trakt.playback(&host, &playing);
        assert_eq!(trakt.auth().unwrap().access, "a2");
        let starts: Vec<_> = log.lock().unwrap().iter().filter(|(p, _, _)| p == "/scrobble/start").map(|(_, a, b)| (a.clone(), b.clone())).collect();
        assert_eq!(starts.len(), 2);
        assert_eq!(starts[1].0, "Bearer a2");
        assert_eq!(starts[1].1["episode"], json!({ "season": 1, "number": 3 }));
        assert_eq!(starts[1].1["progress"], json!(4.17));

        // Music and unmatched files aren't sent; switching scrobbling off stops it.
        trakt.playback(&host, &Playback { kind: "music".into(), media: Some(episode()), ..playing.clone() });
        trakt.playback(&host, &Playback { media: None, ..playing.clone() });
        settings.insert("scrobble".into(), json!(false));
        trakt.settings(&host, &settings);
        trakt.playback(&host, &Playback { state: PlaybackState::Stopped, ..playing.clone() });
        let scrobbles = log.lock().unwrap().iter().filter(|(p, _, _)| p.starts_with("/scrobble/")).count();
        assert_eq!(scrobbles, 2, "nothing more sent");
    }
}
