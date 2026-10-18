//! The phone remote: buttons that press keys in the app, "Play on TV", search (the app's
//! own search, plus YouTube's when it's on) and what's playing. Commands reach the app as
//! [`Change::Remote`]; searches are answered by the app through a reply channel.

use std::sync::Arc;
use std::sync::mpsc::sync_channel;
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State as AxState};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::{Change, State};

/// A button on the remote, pressed in the app as if on the TV remote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
    #[serde(rename = "playpause")]
    PlayPause,
    /// Holding OK: a thing's options.
    Menu,
}

/// What the remote asks the app to do.
#[derive(Clone, Debug)]
pub enum Remote {
    Key(Key),
    Play { library_id: i64, file_id: i64 },
    YouTube { id: String, title: String },
    /// Turns subtitles off (holding Up on the TV remote).
    SubtitlesOff,
}

/// A search result from the app, for the remote.
#[derive(Clone, Debug, Serialize)]
pub struct Hit {
    pub title: String,
    pub context: String,
    pub library: i64,
    /// A show or film's page (opened on the phone).
    pub show: Option<i64>,
    /// An episode or file (played on the TV).
    pub file: Option<i64>,
}

/// What's playing in the app, for the remote.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NowPlaying {
    pub title: String,
    pub paused: bool,
    pub position: Option<f64>,
    pub duration: Option<f64>,
}

/// What the app tells the remote, kept up to date by [`crate::Server::set_status`].
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Status {
    pub playing: Option<NowPlaying>,
    /// YouTube is on (and set up), so it can be searched.
    pub youtube: bool,
}

pub(crate) async fn status(AxState(state): AxState<Arc<State>>) -> Json<Status> {
    Json(state.status.read().unwrap_or_else(|e| e.into_inner()).clone())
}

#[derive(Deserialize)]
pub(crate) struct KeyBody {
    key: Option<Key>,
    /// "subs-off".
    action: Option<String>,
}

pub(crate) async fn key(AxState(state): AxState<Arc<State>>, Json(body): Json<KeyBody>) -> StatusCode {
    let command = match (body.key, body.action.as_deref()) {
        (Some(key), _) => Remote::Key(key),
        (None, Some("subs-off")) => Remote::SubtitlesOff,
        _ => return StatusCode::BAD_REQUEST,
    };
    (state.on_change)(Change::Remote(command));
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
pub(crate) struct PlayBody {
    library: Option<i64>,
    file: Option<i64>,
    youtube: Option<String>,
    title: Option<String>,
}

pub(crate) async fn play(AxState(state): AxState<Arc<State>>, Json(body): Json<PlayBody>) -> StatusCode {
    let command = match body {
        PlayBody { library: Some(library_id), file: Some(file_id), .. } => Remote::Play { library_id, file_id },
        PlayBody { youtube: Some(id), title, .. } if flex_core::youtube::video_id(&id).is_some() => {
            Remote::YouTube { id, title: title.unwrap_or_default() }
        }
        _ => return StatusCode::BAD_REQUEST,
    };
    (state.on_change)(Change::Remote(command));
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
pub(crate) struct SearchQuery {
    q: String,
}

/// The app's search: shows and films, then episodes.
pub(crate) async fn search(AxState(state): AxState<Arc<State>>, Query(query): Query<SearchQuery>) -> Result<Json<Vec<Hit>>, StatusCode> {
    let q = query.q.trim().to_owned();
    if q.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let hits = tokio::task::spawn_blocking(move || {
        let (reply, answer) = sync_channel(1);
        (state.on_change)(Change::Search { query: q, reply });
        // The app answers on its next frame; it may be busy for a moment.
        answer.recv_timeout(Duration::from_secs(5)).ok()
    })
    .await
    .ok()
    .flatten();
    hits.map(Json).ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

#[derive(Serialize)]
pub(crate) struct Video {
    id: String,
    title: String,
    channel: String,
    duration: Option<f64>,
    thumbnail: String,
}

/// YouTube search through yt-dlp (only when YouTube is on in the app).
pub(crate) async fn youtube(AxState(state): AxState<Arc<State>>, Query(query): Query<SearchQuery>) -> Result<Json<Vec<Video>>, (StatusCode, String)> {
    if !state.status.read().unwrap_or_else(|e| e.into_inner()).youtube {
        return Err((StatusCode::FORBIDDEN, "YouTube is off in Flex".to_owned()));
    }
    let q = query.q.trim().to_owned();
    let found = tokio::task::spawn_blocking(move || {
        let tools = flex_core::youtube::Tools::find().ok_or_else(|| "yt-dlp isn't installed yet".to_owned())?;
        tools.search(&q, 20).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(
        found
            .into_iter()
            .map(|l| Video { thumbnail: flex_core::youtube::feeds::thumbnail(&l.id), id: l.id, title: l.title, channel: l.channel, duration: l.duration })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::{Config, Server, Snapshot};

    /// Starts the real server; the "app" records commands and answers searches.
    #[test]
    fn remote_reaches_the_app() {
        let dir = std::env::temp_dir().join(format!("flex-remote-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = Config {
            port: 0,
            db_path: dir.join("flex.db"),
            images_dir: dir.join("images"),
            shots_dir: dir.join("shots"),
            cache_dir: dir.join("cache"),
        };
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = seen.clone();
        let server = Server::start(config, Snapshot::default(), move |change| match change {
            Change::Search { query, reply } => {
                let hit = Hit { title: format!("Found {query}"), context: "Anime".into(), library: 1, show: None, file: Some(7) };
                reply.send(vec![hit]).unwrap();
            }
            Change::Remote(command) => log.lock().unwrap().push(format!("{command:?}")),
            _ => {}
        })
        .unwrap();
        let base = format!("http://127.0.0.1:{}", server.port());
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        let post = |path: &str, body: serde_json::Value| agent.post(&format!("{base}{path}")).send_json(body).unwrap().status().as_u16();

        assert_eq!(post("/api/remote/key", serde_json::json!({ "key": "playpause" })), 204);
        assert_eq!(post("/api/remote/key", serde_json::json!({ "action": "subs-off" })), 204);
        assert_eq!(post("/api/remote/key", serde_json::json!({ "key": "jump" })), 422);
        assert_eq!(post("/api/remote/play", serde_json::json!({ "library": 1, "file": 7 })), 204);
        assert_eq!(post("/api/remote/play", serde_json::json!({ "youtube": "aqz-KE-bpKQ", "title": "Bunny" })), 204);
        assert_eq!(post("/api/remote/play", serde_json::json!({ "youtube": "not a video" })), 400);
        assert_eq!(
            *seen.lock().unwrap(),
            [
                "Key(PlayPause)",
                "SubtitlesOff",
                "Play { library_id: 1, file_id: 7 }",
                "YouTube { id: \"aqz-KE-bpKQ\", title: \"Bunny\" }",
            ]
        );

        let get = |path: &str| {
            let mut res = agent.get(&format!("{base}{path}")).call().unwrap();
            (res.status().as_u16(), res.body_mut().read_to_string().unwrap())
        };
        let (status, body) = get("/api/search?q=frieren");
        assert_eq!(status, 200);
        assert!(body.contains("Found frieren"), "{body}");

        server.set_status(Status { playing: Some(NowPlaying { title: "Frieren · S1E3".into(), paused: true, position: Some(60.0), duration: Some(1440.0) }), youtube: false });
        let (_, body) = get("/api/remote");
        assert!(body.contains("Frieren · S1E3") && body.contains("\"paused\":true"), "{body}");
        assert_eq!(get("/api/youtube/search?q=x").0, 403, "YouTube is off");

        for path in ["/", "/remote.js", "/manifest.webmanifest", "/sw.js", "/icon-192.png", "/icon-512.png"] {
            assert_eq!(agent.get(&format!("{base}{path}")).call().unwrap().status().as_u16(), 200, "{path}");
        }
        let (_, manifest) = get("/manifest.webmanifest");
        let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(manifest["display"], "standalone");
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
