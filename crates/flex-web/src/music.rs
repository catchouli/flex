//! Music in the web UI: a library's albums, an album's tracks, and covers. Tracks play
//! straight from `/media/{library}/{file}` (browsers play mp3, flac, opus and m4a).

use std::sync::Arc;

use axum::extract::{Path, Query, Request, State as AxState};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Redirect, Response};
use flex_core::music::Album;
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

use crate::{LibrarySnapshot, State};

#[derive(Deserialize)]
pub struct AlbumKey {
    key: String,
}

fn library(state: &State, id: i64) -> Option<LibrarySnapshot> {
    state.snapshot().libraries.iter().find(|l| l.lib.id == id).cloned()
}

fn cover_url(l: &LibrarySnapshot, album: &Album) -> Option<String> {
    l.covers.contains_key(&album.key).then(|| format!("/cover/{}?key={}", l.lib.id, urlencode(&album.key)))
}

/// Percent-encodes everything but unreserved characters (for a query value).
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(Serialize)]
struct AlbumSummary {
    key: String,
    title: String,
    artist: String,
    year: Option<i32>,
    added: i64,
    tracks: usize,
    cover: Option<String>,
}

#[derive(Serialize)]
pub struct Albums {
    id: i64,
    name: String,
    /// Tags are still being read.
    loading: bool,
    albums: Vec<AlbumSummary>,
}

/// `/api/music/{id}`: a music library's albums (loose tracks with no album tag aren't
/// listed, as in the app).
pub async fn albums(AxState(state): AxState<Arc<State>>, Path(id): Path<i64>) -> Response {
    let Some(l) = library(&state, id) else { return StatusCode::NOT_FOUND.into_response() };
    let albums = l
        .music
        .iter()
        .flat_map(|m| m.albums.iter())
        .filter(|a| !a.singles)
        .map(|a| AlbumSummary {
            key: a.key.clone(),
            title: a.title.clone(),
            artist: a.artist.clone(),
            year: a.year,
            added: a.added_at,
            tracks: a.tracks.len(),
            cover: cover_url(&l, a),
        })
        .collect();
    Json(Albums { id, name: l.lib.name.clone(), loading: l.music.is_none(), albums }).into_response()
}

#[derive(Serialize)]
struct TrackInfo {
    file: i64,
    title: String,
    /// The track's own artists when they differ from the album's.
    artists: Option<String>,
    number: Option<u32>,
    disc: Option<u32>,
    duration: f64,
}

#[derive(Serialize)]
pub struct AlbumDetails {
    library: i64,
    key: String,
    title: String,
    artist: String,
    year: Option<i32>,
    cover: Option<String>,
    duration: f64,
    tracks: Vec<TrackInfo>,
}

/// `/api/album/{id}?key=…`: an album's tracks, in play order.
pub async fn album(AxState(state): AxState<Arc<State>>, Path(id): Path<i64>, Query(q): Query<AlbumKey>) -> Response {
    let Some(l) = library(&state, id) else { return StatusCode::NOT_FOUND.into_response() };
    let Some((_, album)) = l.music.as_ref().and_then(|m| m.album(&q.key)) else { return StatusCode::NOT_FOUND.into_response() };
    let tracks = album
        .tracks
        .iter()
        .map(|t| {
            let own = t.artists.join(", ");
            TrackInfo {
                file: t.file_id,
                title: t.title.clone(),
                artists: (!own.is_empty() && own != album.artist).then_some(own),
                number: t.number,
                disc: t.disc,
                duration: t.duration,
            }
        })
        .collect();
    Json(AlbumDetails {
        library: id,
        key: album.key.clone(),
        title: album.title.clone(),
        artist: album.artist.clone(),
        year: album.year,
        cover: cover_url(&l, album),
        duration: album.duration(),
        tracks,
    })
    .into_response()
}

/// `/cover/{id}?key=…`: art next to the tracks, or a redirect to its Cover Art Archive
/// image.
pub async fn cover(AxState(state): AxState<Arc<State>>, Path(id): Path<i64>, Query(q): Query<AlbumKey>, req: Request) -> Response {
    let Some(cover) = library(&state, id).and_then(|l| l.covers.get(&q.key).cloned()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match cover.strip_prefix("file:") {
        Some(path) => match tower_http::services::ServeFile::new(path).oneshot(req).await {
            Ok(res) => res.into_response(),
            Err(e) => {
                log::warn!("web: serving a cover: {e}");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        },
        None if cover.starts_with("https://") => Redirect::temporary(&cover).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use flex_core::music::{Album, Artist, MusicCatalog, Track};
    use flex_core::{Library, LibraryKind, MediaFile, Tree};

    use super::urlencode;
    use crate::{Config, LibrarySnapshot, Server, Snapshot};

    /// A music library with one album (and a loose track), served for real on a free port.
    #[test]
    fn serves_albums_tracks_and_covers() {
        let dir = std::env::temp_dir().join(format!("flex-web-music-{}", std::process::id()));
        let root = dir.join("music");
        std::fs::create_dir_all(root.join("Lantern Quarry/Harbour Lights")).unwrap();
        std::fs::write(root.join("Lantern Quarry/Harbour Lights/01 Morning Tide.mp3"), b"ID3 not really audio").unwrap();
        let art = root.join("Lantern Quarry/Harbour Lights/cover.jpg");
        std::fs::write(&art, b"\xff\xd8 not really a jpeg").unwrap();

        let lib = Library { id: 3, name: "Music".into(), root: root.clone(), kind: LibraryKind::Music };
        let file = MediaFile { id: 11, library_id: 3, rel_path: "Lantern Quarry/Harbour Lights/01 Morning Tide.mp3".into(), ..Default::default() };
        let track = |file_id, title: &str| Track { file_id, title: title.into(), artists: Vec::new(), number: Some(1), disc: None, duration: 245.0 };
        let album = |key: &str, title: &str, singles| Album {
            key: key.into(),
            title: title.into(),
            artist: "Lantern Quarry".into(),
            artists: vec!["Lantern Quarry".into()],
            year: Some(2019),
            tracks: vec![track(11, "Morning Tide")],
            cover: None,
            added_at: 5,
            singles,
        };
        let music = MusicCatalog {
            albums: vec![album("lq/Harbour Lights", "Harbour Lights", false), album("lq/loose", "Singles", true)],
            artists: vec![Artist { name: "Lantern Quarry".into(), albums: vec![0], appears_on: Vec::new() }],
            tracks: 2,
        };
        let covers: HashMap<String, String> = [
            ("lq/Harbour Lights".to_owned(), format!("file:{}", art.display())),
            ("lq/loose".to_owned(), "https://coverartarchive.org/release-group/x/front-500".to_owned()),
        ]
        .into();
        let snapshot = Snapshot {
            libraries: vec![LibrarySnapshot {
                lib,
                tree: Arc::new(Tree::new(vec![file])),
                catalog: None,
                music: Some(Arc::new(music)),
                covers: Arc::new(covers),
            }],
            series: Arc::default(),
        };
        let config = Config {
            port: 0,
            db_path: dir.join("flex.db"),
            images_dir: dir.join("images"),
            shots_dir: dir.join("shots"),
            cache_dir: dir.join("cache"),
        };
        let server = Server::start(config, snapshot, |_| {}).unwrap();
        let base = format!("http://127.0.0.1:{}", server.port());
        let client: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).max_redirects(0).build().into();
        let json = |path: &str| -> serde_json::Value { client.get(&format!("{base}{path}")).call().unwrap().body_mut().read_json().unwrap() };

        let home = json("/api/home");
        assert_eq!(home["libraries"][0]["kind"], "music");
        assert_eq!(home["libraries"][0]["albums"], 1, "Singles isn't counted");
        assert_eq!(home["continue"].as_array().unwrap().len(), 0);

        let albums = json("/api/music/3");
        assert_eq!(albums["albums"].as_array().unwrap().len(), 1);
        assert_eq!(albums["albums"][0]["title"], "Harbour Lights");
        let cover = albums["albums"][0]["cover"].as_str().unwrap().to_owned();
        assert_eq!(cover, "/cover/3?key=lq%2FHarbour%20Lights");

        let album = json("/api/album/3?key=lq%2FHarbour%20Lights");
        assert_eq!(album["tracks"][0]["file"], 11);
        assert_eq!(album["tracks"][0]["artists"], serde_json::Value::Null, "same as the album's artist");

        let mut local = client.get(&format!("{base}{cover}")).call().unwrap();
        assert_eq!(local.status().as_u16(), 200);
        assert_eq!(local.body_mut().read_to_vec().unwrap(), std::fs::read(&art).unwrap());
        let remote = client.get(&format!("{base}/cover/3?key=lq%2Floose")).call().unwrap();
        assert_eq!(remote.status().as_u16(), 307);
        assert!(remote.headers()["location"].to_str().unwrap().starts_with("https://coverartarchive.org/"));
        assert_eq!(client.get(&format!("{base}/api/album/3?key=nope")).call().unwrap().status().as_u16(), 404);

        let mut audio = client.get(&format!("{base}/media/3/11")).call().unwrap();
        assert_eq!(audio.status().as_u16(), 200);
        assert_eq!(audio.headers()["content-type"].to_str().unwrap(), "audio/mpeg");
        assert_eq!(audio.body_mut().read_to_vec().unwrap(), b"ID3 not really audio");
        let js = client.get(&format!("{base}/music.js")).call().unwrap();
        assert_eq!(js.status().as_u16(), 200);

        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn encodes_album_keys() {
        assert_eq!(urlencode("Artist/Album (2019)\u{1f}Album"), "Artist%2FAlbum%20%282019%29%1FAlbum");
        assert_eq!(urlencode("plain-key_1.0~"), "plain-key_1.0~");
    }
}
