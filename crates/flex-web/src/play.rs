//! Playing in the browser: what a file holds and how to play it, the file itself (when
//! the browser can play it as is), an ffmpeg stream (when it can't), and progress.

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State as AxState};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use flex_core::progress::Progress;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

use crate::media::{AudioMode, MediaInfo, StreamRequest, VideoMode};
use crate::{Change, LibrarySnapshot, State};

/// A library file: its library, the file, and its full path.
fn locate(state: &State, library_id: i64, file_id: i64) -> Option<(LibrarySnapshot, flex_core::MediaFile, std::path::PathBuf)> {
    let snapshot = state.snapshot();
    let l = snapshot.libraries.iter().find(|l| l.lib.id == library_id)?.clone();
    let file = l.tree.file(file_id)?.clone();
    let path = file.abs_path(&l.lib.root);
    Some((l, file, path))
}

/// Why a file can't be played in the browser, if it can't.
fn unsupported(file: &flex_core::MediaFile) -> Option<&'static str> {
    let iso = file.rel_path.rsplit('.').next().is_some_and(|e| e.eq_ignore_ascii_case("iso"));
    (iso || flex_core::parse::disc_folder(&file.rel_path).is_some()).then_some("Discs (DVD and Blu-ray) only play in the Flex app for now.")
}

#[derive(Serialize)]
pub struct NextUp {
    file: i64,
    title: String,
}

#[derive(Serialize)]
pub struct PlayInfo {
    library: i64,
    file: i64,
    /// The show or film's page, once organised.
    show: Option<i64>,
    title: String,
    episode: Option<String>,
    /// Saved resume position (seconds).
    position: Option<f64>,
    duration: Option<f64>,
    next: Option<NextUp>,
    media: Option<Arc<MediaInfo>>,
    /// Fonts attached to the file, for its ASS subtitles.
    fonts: Vec<String>,
    /// A font with CJK characters, for subtitles whose fonts lack them: its family name
    /// (its URL is `/fonts/fallback`).
    cjk_font: Option<&'static str>,
    /// Set when the file can't be played in the browser.
    error: Option<String>,
}

/// The subtitle track to show at first: a forced or default one, as mpv would.
fn default_subtitles(media: &MediaInfo) -> Option<usize> {
    let tracks = &media.subtitles;
    tracks.iter().find(|s| s.forced).or_else(|| tracks.iter().find(|s| s.default)).map(|s| s.index)
}

/// The next episode in the same season (the app's rule: none for films, none past the
/// end of a season).
fn next_up(snapshot: &crate::Snapshot, l: &LibrarySnapshot, file_id: i64) -> Option<NextUp> {
    let id = match l.catalog.as_ref().filter(|c| c.show_of_file(file_id).is_some()) {
        Some(c) if c.show_of_file(file_id)?.is_series() => c.neighbour_in_season(file_id, 1)?,
        Some(_) => return None,
        None if !l.lib.kind.is_episodic() => return None,
        None => l.tree.next_in_folder(file_id)?.id,
    };
    let titles = flex_core::titles::titles(&l.tree, l.catalog.as_deref(), &snapshot.series, id)?;
    Some(NextUp { file: id, title: titles.episode.unwrap_or(titles.name) })
}

pub async fn info(AxState(state): AxState<Arc<State>>, Path((library_id, file_id)): Path<(i64, i64)>) -> Result<Json<PlayInfo>, StatusCode> {
    let snapshot = state.snapshot();
    let (l, file, path) = locate(&state, library_id, file_id).ok_or(StatusCode::NOT_FOUND)?;
    let titles = flex_core::titles::titles(&l.tree, l.catalog.as_deref(), &snapshot.series, file_id).ok_or(StatusCode::NOT_FOUND)?;
    let (media, error) = match unsupported(&file) {
        Some(why) => (None, Some(why.to_owned())),
        None => match state.tools.probe((file.id, file.size, file.mtime), &path).await {
            Ok(info) => (Some(info), None),
            Err(e) => {
                log::warn!("web: probing {}: {e:#}", path.display());
                (None, Some(format!("Couldn't read this file: {e:#}")))
            }
        },
    };
    let duration = media.as_ref().and_then(|m| m.duration).or(file.progress.duration);
    // Start extracting the subtitles likely to be shown, so they're ready when playback starts.
    if let Some(m) = &media
        && let Some(index) = default_subtitles(m).filter(|i| !m.subtitles[*i].image)
        && let Err(e) = state.tools.subtitles(&state.cache_dir, (file.id, file.size), &path, index)
    {
        log::warn!("web: extracting subtitles: {e:#}");
    }
    let fonts = media
        .as_ref()
        .map(|m| m.fonts.iter().map(|(i, _)| format!("/fonts/{library_id}/{file_id}/{i}")).collect())
        .unwrap_or_default();
    Ok(Json(PlayInfo {
        library: library_id,
        file: file_id,
        show: l.catalog.as_ref().and_then(|c| c.show_of_file(file_id)).map(|e| e.show.id),
        title: titles.show.clone().unwrap_or_else(|| titles.name.clone()),
        episode: titles.show.as_ref().and(titles.episode.clone()),
        position: file.progress.position,
        duration,
        next: next_up(&snapshot, &l, file_id),
        media,
        fonts,
        cjk_font: crate::media::cjk_fallback_font().map(|(_, family)| family),
        error,
    }))
}

/// `/media/{library}/{file}`: the file as is, with seeking (HTTP ranges).
pub async fn file(AxState(state): AxState<Arc<State>>, Path((library_id, file_id)): Path<(i64, i64)>, req: Request) -> Response {
    let Some((_, file, path)) = locate(&state, library_id, file_id) else { return StatusCode::NOT_FOUND.into_response() };
    if unsupported(&file).is_some() {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tower_http::services::ServeFile::new(path).oneshot(req).await {
        Ok(res) => res.into_response(),
        Err(e) => {
            log::warn!("web: serving a file: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct StreamQuery {
    #[serde(default)]
    start: f64,
    audio: Option<usize>,
    video: VideoMode,
    #[serde(rename = "audiomode")]
    audio_mode: AudioMode,
    /// A picture subtitle track to burn in.
    burn: Option<usize>,
}

/// `/stream/{library}/{file}?start=…&audio=…&video=copy|h264&audiomode=copy|aac`: the file
/// as fragmented MP4, made by ffmpeg as it's sent. Seeking requests a new stream.
pub async fn stream(
    AxState(state): AxState<Arc<State>>,
    Path((library_id, file_id)): Path<(i64, i64)>,
    Query(q): Query<StreamQuery>,
) -> Response {
    let Some((_, file, path)) = locate(&state, library_id, file_id) else { return StatusCode::NOT_FOUND.into_response() };
    if unsupported(&file).is_some() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let info = match state.tools.probe((file.id, file.size, file.mtime), &path).await {
        Ok(info) => info,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let req = StreamRequest {
        path: &path,
        start: q.start.max(0.0),
        audio: q.audio,
        video: q.video,
        audio_mode: q.audio_mode,
        burn: q.burn,
        info: &info,
    };
    let mut child = match crate::media::start_stream(&state.tools, req).await {
        Ok(child) => child,
        Err(e) => {
            log::warn!("web: streaming {}: {e:#}", path.display());
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
        }
    };
    log::info!("web: streaming {} from {:.0}s (video {:?}, audio {:?})", file.rel_path, q.start, q.video, q.audio_mode);
    let Some(stdout) = child.stdout.take() else { return StatusCode::INTERNAL_SERVER_ERROR.into_response() };
    // The stream owns ffmpeg: when the browser stops reading, both are dropped and
    // ffmpeg is killed.
    let body = tokio_util::io::ReaderStream::with_capacity(stdout, 256 * 1024).map(move |chunk| {
        let _keep_alive = &child;
        chunk
    });
    (
        [(header::CONTENT_TYPE, "video/mp4"), (header::CACHE_CONTROL, "no-store"), (header::ACCEPT_RANGES, "none")],
        Body::from_stream(body),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct ProgressReport {
    library: i64,
    file: i64,
    position: f64,
    duration: f64,
    /// The end of a viewing session (leaving the player): logged in the history.
    #[serde(default)]
    end: bool,
    /// When the session started (Unix seconds), for the history.
    #[serde(default)]
    session_start: i64,
    /// Whether anything was recorded earlier in this session.
    #[serde(default)]
    recorded: bool,
    /// Whether the file became watched earlier in this session.
    #[serde(default)]
    completed: bool,
}

#[derive(Serialize)]
pub struct ProgressReply {
    recorded: bool,
    watched: bool,
}

/// Saves how far the browser has got, with the app's rules (the watched threshold, and
/// nothing for a few seconds of sampling).
pub async fn progress(AxState(state): AxState<Arc<State>>, Json(r): Json<ProgressReport>) -> Result<Json<ProgressReply>, StatusCode> {
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<ProgressReply>> {
        let Some((_, file, _)) = locate(&state, r.library, r.file) else { return Ok(None) };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let threshold = flex_core::progress::watched_threshold(&db);
        let old = file.progress;
        let new = old.after_playing(r.position, r.duration, threshold, now);
        if let Some(p) = new.filter(|p| *p != old) {
            db.set_progress(r.file, &p)?;
            state.patch_files(r.library, &[r.file], |f| f.progress = p);
            (state.on_change)(Change::Progress { library_id: r.library, file_id: r.file, progress: p });
        }
        let recorded = new.is_some();
        let became_watched = new.is_some_and(|p: Progress| p.watched && p.position.is_none());
        if r.end && (recorded || r.recorded) {
            let position = if became_watched || r.completed { r.duration } else { r.position };
            db.add_history(r.file, if r.session_start > 0 { r.session_start } else { now }, Some(position), became_watched || r.completed)?;
        }
        Ok(Some(ProgressReply { recorded, watched: became_watched }))
    })
    .await;
    match result {
        Ok(Ok(Some(reply))) => Ok(Json(reply)),
        Ok(Ok(None)) => Err(StatusCode::NOT_FOUND),
        Ok(Err(e)) => {
            log::warn!("web: saving progress: {e:#}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
pub struct StartQuery {
    t: f64,
}

#[derive(Serialize)]
pub struct Start {
    start: f64,
}

/// `/api/start/{library}/{file}?t=…`: where a copied stream asked to start at `t` really
/// starts (the keyframe before it), so the player can line up its clock and subtitles.
pub async fn start(AxState(state): AxState<Arc<State>>, Path((library_id, file_id)): Path<(i64, i64)>, Query(q): Query<StartQuery>) -> Result<Json<Start>, StatusCode> {
    let (_, _, path) = locate(&state, library_id, file_id).ok_or(StatusCode::NOT_FOUND)?;
    let t = q.t.max(0.0);
    let start = if t > 0.0 { state.tools.keyframe_before(&path, t).await.unwrap_or(t) } else { 0.0 };
    Ok(Json(Start { start }))
}

#[derive(Deserialize)]
pub struct SubtitlesQuery {
    #[serde(default)]
    from: f64,
}

/// `/subs/{library}/{file}/{index}?from=…`: a text subtitle track as ASS, sent as it's
/// extracted. The whole track once it's been extracted (cached); otherwise, from the
/// start, the full extraction as it goes, or from `from` on, a quick extraction from
/// there (the full one carries on in the background for next time).
pub async fn subtitles(
    AxState(state): AxState<Arc<State>>,
    Path((library_id, file_id, index)): Path<(i64, i64, usize)>,
    Query(q): Query<SubtitlesQuery>,
) -> Response {
    let Some((_, file, path)) = locate(&state, library_id, file_id) else { return StatusCode::NOT_FOUND.into_response() };
    let headers = [(header::CONTENT_TYPE, "text/plain; charset=utf-8"), (header::CACHE_CONTROL, "no-store")];
    match state.tools.subtitles(&state.cache_dir, (file.id, file.size), &path, index) {
        Ok(growing) if growing.is_complete() || q.from < 1.0 => (headers, Body::from_stream(growing.stream())).into_response(),
        Ok(_) => match state.tools.subtitles_from(&path, index, q.from) {
            Ok(mut child) => {
                let Some(stdout) = child.stdout.take() else { return StatusCode::INTERNAL_SERVER_ERROR.into_response() };
                let body = tokio_util::io::ReaderStream::new(stdout).map(move |chunk| {
                    let _keep_alive = &child;
                    chunk
                });
                (headers, Body::from_stream(body)).into_response()
            }
            Err(e) => {
                log::warn!("web: subtitles for {}: {e:#}", path.display());
                StatusCode::NOT_FOUND.into_response()
            }
        },
        Err(e) => {
            log::warn!("web: subtitles for {}: {e:#}", path.display());
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

/// `/fonts/{library}/{file}/{index}`: a font attached to the file.
pub async fn font(AxState(state): AxState<Arc<State>>, Path((library_id, file_id, index)): Path<(i64, i64, usize)>) -> Response {
    let Some((_, file, path)) = locate(&state, library_id, file_id) else { return StatusCode::NOT_FOUND.into_response() };
    let Ok(info) = state.tools.probe((file.id, file.size, file.mtime), &path).await else { return StatusCode::NOT_FOUND.into_response() };
    let Some((_, name)) = info.fonts.iter().find(|(i, _)| *i == index) else { return StatusCode::NOT_FOUND.into_response() };
    match state.tools.font(&state.cache_dir, (file.id, file.size), &path, index, name).await {
        Ok(out) => match tokio::fs::read(&out).await {
            Ok(bytes) => ([(header::CONTENT_TYPE, "application/octet-stream"), (header::CACHE_CONTROL, "private, max-age=604800")], bytes).into_response(),
            Err(_) => StatusCode::NOT_FOUND.into_response(),
        },
        Err(e) => {
            log::warn!("web: font {name}: {e:#}");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

/// `/fonts/fallback`: a system font with CJK characters.
pub async fn fallback_font() -> Response {
    let Some((path, _)) = crate::media::cjk_fallback_font() else { return StatusCode::NOT_FOUND.into_response() };
    match tokio::fs::read(&path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/octet-stream"), (header::CACHE_CONTROL, "public, max-age=2592000")], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `/vendor/jassub/{name}`: the subtitle renderer (see scripts/vendor-jassub.sh).
pub async fn jassub(Path(name): Path<String>) -> Response {
    let (body, kind): (&'static [u8], &str) = match name.as_str() {
        "jassub.js" => (include_bytes!("../web/vendor/jassub/jassub.js"), "text/javascript; charset=utf-8"),
        "worker.js" => (include_bytes!("../web/vendor/jassub/worker.js"), "text/javascript; charset=utf-8"),
        "jassub-worker.wasm" => (include_bytes!("../web/vendor/jassub/jassub-worker.wasm"), "application/wasm"),
        "jassub-worker-modern.wasm" => (include_bytes!("../web/vendor/jassub/jassub-worker-modern.wasm"), "application/wasm"),
        "default.woff2" => (include_bytes!("../web/vendor/jassub/default.woff2"), "font/woff2"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, "public, max-age=86400")], body).into_response()
}
