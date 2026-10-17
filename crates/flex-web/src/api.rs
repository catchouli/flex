//! The web UI's pages (one static page, routed in the browser) and its JSON API.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State as AxState};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use flex_core::catalog::ShowEntry;
use flex_core::progress::{self, ContinueKind};
use serde::{Deserialize, Serialize};

use crate::images::{self, has_shot};
use crate::{Change, LibrarySnapshot, Snapshot, State};

const MAX_CONTINUE: usize = 20;

pub fn router(state: Arc<State>) -> axum::Router {
    axum::Router::new()
        .route("/", get(index))
        .route("/app.js", get(|| asset(include_str!("../web/app.js"), "text/javascript; charset=utf-8")))
        .route("/app.css", get(|| asset(include_str!("../web/app.css"), "text/css; charset=utf-8")))
        .route("/player.js", get(|| asset(include_str!("../web/player.js"), "text/javascript; charset=utf-8")))
        .route("/subtitles.js", get(|| asset(include_str!("../web/subtitles.js"), "text/javascript; charset=utf-8")))
        .route("/icon.png", get(icon))
        .route("/api/home", get(home))
        .route("/api/music/{id}", get(crate::music::albums))
        .route("/api/album/{id}", get(crate::music::album))
        .route("/cover/{id}", get(crate::music::cover))
        .route("/music.js", get(|| asset(include_str!("../web/music.js"), "text/javascript; charset=utf-8")))
        .route("/api/library/{id}", get(library))
        .route("/api/show/{library}/{show}", get(show))
        .route("/api/watched", post(set_watched))
        .route("/api/play/{library}/{file}", get(crate::play::info))
        .route("/api/progress", post(crate::play::progress))
        .route("/media/{library}/{file}", get(crate::play::file))
        .route("/stream/{library}/{file}", get(crate::play::stream))
        .route("/api/start/{library}/{file}", get(crate::play::start))
        .route("/api/subs-ready/{library}/{file}/{index}", get(crate::play::subs_ready))
        .route("/subs/{library}/{file}/{index}", get(crate::play::subtitles))
        .route("/fonts/fallback", get(crate::play::fallback_font))
        .route("/fonts/{library}/{file}/{index}", get(crate::play::font))
        .route("/vendor/jassub/{name}", get(crate::play::jassub))
        .route("/img/web", get(images::web))
        .route("/img/{size}/{name}", get(images::tmdb))
        .route("/shot/{library}/{file}", get(images::shot))
        .with_state(state)
}

async fn index() -> Response {
    asset(include_str!("../web/index.html"), "text/html; charset=utf-8").await
}

async fn asset(body: &'static str, kind: &'static str) -> Response {
    // Small files: always check for a newer version (after updating Flex).
    ([(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, "no-cache")], body).into_response()
}

async fn icon() -> Response {
    let png: &'static [u8] = include_bytes!("../../../assets/icon-256.png");
    ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=86400")], png).into_response()
}

fn lib(snapshot: &Snapshot, id: i64) -> Option<&LibrarySnapshot> {
    snapshot.libraries.iter().find(|l| l.lib.id == id)
}

// ---------------------------------------------------------------------------------------
// Home

#[derive(Serialize)]
struct Card {
    library: i64,
    file: i64,
    /// The show or film's page, once organised.
    show: Option<i64>,
    title: String,
    episode: String,
    /// "Up next" rather than partway through.
    next: bool,
    remaining: Option<f64>,
    fraction: Option<f64>,
    image: Option<String>,
}

#[derive(Serialize)]
struct LibrarySummary {
    id: i64,
    name: String,
    kind: &'static str,
    videos: usize,
    organised: bool,
    /// Music libraries: albums (not counting loose tracks), once the tags are read.
    albums: Option<usize>,
}

#[derive(Serialize)]
struct HomePage {
    #[serde(rename = "continue")]
    cont: Vec<Card>,
    libraries: Vec<LibrarySummary>,
}

/// The picture for a file's card: its episode's still or the film's backdrop, else a
/// screenshot the app has taken.
fn card_image(state: &State, snapshot: &Snapshot, l: &LibrarySnapshot, file_id: i64) -> Option<String> {
    let entry = l.catalog.as_ref().and_then(|c| c.show_of_file(file_id));
    let tmdb = match entry {
        Some(e) if e.is_series() => l
            .catalog
            .as_ref()
            .and_then(|c| flex_core::titles::episode_meta(c, &snapshot.series, file_id))
            .and_then(|m| m.still.as_deref())
            .map(|p| images::url(p, "w780")),
        Some(e) => e.show.backdrop.as_deref().map(|p| images::url(p, "w1280")),
        None => None,
    };
    tmdb.or_else(|| {
        let file = l.tree.file(file_id)?;
        has_shot(state, file).then(|| format!("/shot/{}/{}", l.lib.id, file_id))
    })
}

async fn home(AxState(state): AxState<Arc<State>>) -> Json<HomePage> {
    let snapshot = state.snapshot();
    // Like the app: per show for organised libraries, per folder for the rest.
    let organised = snapshot.libraries.iter().filter_map(|l| l.catalog.as_deref().map(|c| (&l.lib, &*l.tree, c)));
    let mut items = progress::continue_watching_shows(organised, MAX_CONTINUE);
    let pending = snapshot
        .libraries
        .iter()
        .filter(|l| l.catalog.is_none() && l.lib.kind != flex_core::LibraryKind::Music)
        .map(|l| (&l.lib, &*l.tree));
    items.extend(progress::continue_watching(pending, MAX_CONTINUE));
    // Removed on the TV (hold OK → Remove from Continue Watching): hidden here too.
    let hidden = state.db.lock().unwrap_or_else(|e| e.into_inner()).continue_hidden().unwrap_or_default();
    let mut items = progress::without_hidden(items, &hidden);
    items.truncate(MAX_CONTINUE);

    let cont = items
        .iter()
        .filter_map(|item| {
            let l = lib(&snapshot, item.library_id)?;
            let file = l.tree.file(item.file_id)?;
            let titles = flex_core::titles::titles(&l.tree, l.catalog.as_deref(), &snapshot.series, item.file_id)?;
            // The show's (matched) title once organised; the top folder name until then.
            let title = match (&titles.show, file.rel_path.split_once('/')) {
                (Some(s), _) => s.clone(),
                (None, Some((top, _))) => flex_core::library::display_name(top, false),
                (None, None) => titles.name.clone(),
            };
            let episode = titles.episode.clone().unwrap_or_else(|| if titles.show.is_none() { titles.name.clone() } else { String::new() });
            let resume = item.kind == ContinueKind::Resume;
            Some(Card {
                library: l.lib.id,
                file: item.file_id,
                show: l.catalog.as_ref().and_then(|c| c.show_of_file(item.file_id)).map(|e| e.show.id),
                title,
                episode,
                next: !resume,
                remaining: if resume { file.progress.remaining_secs() } else { None },
                fraction: if resume { file.progress.fraction() } else { None },
                image: card_image(&state, &snapshot, l, item.file_id),
            })
        })
        .collect();
    let libraries = snapshot
        .libraries
        .iter()
        .map(|l| LibrarySummary {
            id: l.lib.id,
            name: l.lib.name.clone(),
            kind: l.lib.kind.as_str(),
            videos: l.tree.len(),
            organised: l.catalog.is_some() || l.music.is_some(),
            albums: l.music.as_ref().map(|m| m.albums.iter().filter(|a| !a.singles).count()),
        })
        .collect();
    Json(HomePage { cont, libraries })
}

// ---------------------------------------------------------------------------------------
// A library's shows

#[derive(Serialize)]
struct ShowSummary {
    id: i64,
    title: String,
    year: Option<i32>,
    poster: Option<String>,
    series: bool,
    episodes: usize,
    unwatched: usize,
    /// When the newest file arrived (Unix seconds), for "recently added".
    added: i64,
}

#[derive(Serialize)]
struct LibraryPage {
    id: i64,
    name: String,
    kind: &'static str,
    /// False while the app is still sorting the library into shows (try again shortly).
    organised: bool,
    shows: Vec<ShowSummary>,
}

fn regular_episodes(entry: &ShowEntry) -> impl Iterator<Item = &flex_core::catalog::Episode> {
    let has_regular = entry.episodes.iter().any(|e| e.season != 0);
    entry.episodes.iter().filter(move |e| !has_regular || e.season != 0)
}

async fn library(AxState(state): AxState<Arc<State>>, Path(id): Path<i64>) -> Result<Json<LibraryPage>, StatusCode> {
    let snapshot = state.snapshot();
    let l = lib(&snapshot, id).ok_or(StatusCode::NOT_FOUND)?;
    let shows = l
        .catalog
        .as_ref()
        .map(|c| {
            c.shows
                .iter()
                .map(|e| {
                    let files: Vec<_> = regular_episodes(e).filter_map(|ep| l.tree.file(ep.file_id)).collect();
                    ShowSummary {
                        id: e.show.id,
                        title: e.show.title.clone(),
                        year: e.show.year,
                        poster: e.show.poster.as_deref().map(|p| images::url(p, "w342")),
                        series: e.is_series(),
                        episodes: files.len(),
                        unwatched: files.iter().filter(|f| !f.progress.watched).count(),
                        added: files.iter().map(|f| f.added_at).max().unwrap_or_default(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Json(LibraryPage {
        id,
        name: l.lib.name.clone(),
        kind: l.lib.kind.as_str(),
        organised: l.catalog.is_some(),
        shows,
    }))
}

// ---------------------------------------------------------------------------------------
// A show or film

#[derive(Serialize)]
struct EpisodeInfo {
    file: i64,
    number: Option<i32>,
    title: String,
    overview: Option<String>,
    image: Option<String>,
    watched: bool,
    fraction: Option<f64>,
    remaining: Option<f64>,
    duration: Option<f64>,
}

#[derive(Serialize)]
struct Season {
    season: i32,
    label: String,
    episodes: Vec<EpisodeInfo>,
}

#[derive(Serialize)]
struct ShowPage {
    library: i64,
    id: i64,
    title: String,
    year: Option<i32>,
    overview: Option<String>,
    genres: Option<String>,
    /// Minutes (per episode for series).
    runtime: Option<i32>,
    poster: Option<String>,
    backdrop: Option<String>,
    series: bool,
    /// What to watch next: partway through, the next unwatched, or the first.
    next: Option<i64>,
    seasons: Vec<Season>,
}

async fn show(AxState(state): AxState<Arc<State>>, Path((library_id, show_id)): Path<(i64, i64)>) -> Result<Json<ShowPage>, StatusCode> {
    let snapshot = state.snapshot();
    let l = lib(&snapshot, library_id).ok_or(StatusCode::NOT_FOUND)?;
    let catalog = l.catalog.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let entry = catalog.show(show_id).ok_or(StatusCode::NOT_FOUND)?;
    let series = entry.is_series();
    let seasons = entry
        .seasons()
        .into_iter()
        .map(|season| {
            let episodes = entry
                .season_episodes(season)
                .filter_map(|ep| {
                    let file = l.tree.file(ep.file_id)?;
                    let titles = flex_core::titles::titles(&l.tree, Some(catalog), &snapshot.series, ep.file_id)?;
                    let meta = flex_core::titles::episode_meta(catalog, &snapshot.series, ep.file_id);
                    let p = file.progress;
                    Some(EpisodeInfo {
                        file: ep.file_id,
                        number: ep.number,
                        // For films, the file name tells versions/parts apart.
                        title: if series { titles.name } else { flex_core::library::display_name(file.file_name(), true) },
                        overview: meta.map(|m| m.overview.clone()).filter(|o| !o.is_empty()),
                        image: meta.and_then(|m| m.still.as_deref()).map(|s| images::url(s, "w780")).or_else(|| {
                            has_shot(&state, file).then(|| format!("/shot/{library_id}/{}", ep.file_id))
                        }),
                        watched: p.watched,
                        fraction: p.fraction(),
                        remaining: p.position.and(p.remaining_secs()),
                        duration: p.duration,
                    })
                })
                .collect();
            let label = match season {
                0 => "Specials".to_owned(),
                _ if !series => "Files".to_owned(),
                s => format!("Season {s}"),
            };
            Season { season, label, episodes }
        })
        .collect();
    let s = &entry.show;
    Ok(Json(ShowPage {
        library: library_id,
        id: s.id,
        title: s.title.clone(),
        year: s.year,
        overview: s.overview.clone(),
        genres: s.genres.clone(),
        runtime: s.runtime,
        poster: s.poster.as_deref().map(|p| images::url(p, "w342")),
        backdrop: s.backdrop.as_deref().map(|p| images::url(p, "w1280")),
        series,
        next: progress::next_up(entry, &l.tree).map(|n| n.file_id()),
        seasons,
    }))
}

// ---------------------------------------------------------------------------------------
// Changes

#[derive(Deserialize)]
struct WatchedRequest {
    library: i64,
    files: Vec<i64>,
    watched: bool,
}

async fn set_watched(AxState(state): AxState<Arc<State>>, Json(req): Json<WatchedRequest>) -> StatusCode {
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
        let snapshot = state.snapshot();
        let Some(l) = lib(&snapshot, req.library) else { return Ok(false) };
        // Only files of that library.
        let ids: Vec<i64> = req.files.iter().copied().filter(|id| l.tree.file(*id).is_some()).collect();
        if ids.is_empty() {
            return Ok(false);
        }
        state.db.lock().unwrap_or_else(|e| e.into_inner()).set_watched(&ids, req.watched)?;
        state.patch_files(req.library, &ids, |f| {
            f.progress.watched = req.watched;
            f.progress.position = None;
        });
        (state.on_change)(Change::Watched { library_id: req.library, file_ids: ids, watched: req.watched });
        Ok(true)
    })
    .await;
    match result {
        Ok(Ok(true)) => StatusCode::NO_CONTENT,
        Ok(Ok(false)) => StatusCode::NOT_FOUND,
        Ok(Err(e)) => {
            log::warn!("web: marking watched: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
