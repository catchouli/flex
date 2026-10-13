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
    /// Set when the file can't be played in the browser.
    error: Option<String>,
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
    let req = StreamRequest { path: &path, start: q.start.max(0.0), audio: q.audio, video: q.video, audio_mode: q.audio_mode, info: &info };
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
