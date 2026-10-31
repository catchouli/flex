//! Posters, stills and screenshots, served from the app's caches. Images the app hasn't
//! downloaded yet are fetched (from TMDB or AniList only) and cached the same way.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as UrlPath, Query, State as AxState};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::State;

/// TMDB sizes the web UI asks for (the same renditions the app caches).
const SIZES: [&str; 4] = ["w185", "w342", "w780", "w1280"];
const TMDB_IMAGES: &str = "https://image.tmdb.org/t/p";
/// Screenshot used as an episode's picture when TMDB has none (the app's `shots::THUMB`).
const THUMB_SHOT: &str = "04.jpg";

/// The web UI's URL for a TMDB image path ("/abc.jpg") or an AniList image URL.
pub fn url(path: &str, size: &str) -> String {
    if path.starts_with("http") {
        format!("/img/web?u={}", encode(path))
    } else {
        format!("/img/{size}{path}")
    }
}

/// Percent-encodes a URL for use as a query value.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The app's cache file for a full image URL (see `flex-app/src/images.rs`). The hash
/// only has to match within one build, which is the case: both run in the same binary.
fn web_cache_file(dir: &Path, url: &str) -> PathBuf {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    url.hash(&mut h);
    let ext = url.rsplit('.').next().filter(|e| e.len() <= 4).unwrap_or("img");
    dir.join("web").join(format!("{:016x}.{ext}", h.finish()))
}

fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    }
}

/// Reads a cached image, downloading it first if needed.
async fn cached(state: Arc<State>, file: PathBuf, url: Option<String>) -> Response {
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(Vec<u8>, &'static str)> {
        let kind = content_type(&file);
        if let Ok(bytes) = std::fs::read(&file) {
            return Ok((bytes, kind));
        }
        let url = url.ok_or_else(|| anyhow::anyhow!("not cached"))?;
        let mut response = state.agent.get(&url).call()?;
        let image = response.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("image/"));
        anyhow::ensure!(image, "{url} isn't an image");
        let bytes = response.body_mut().with_config().limit(20_000_000).read_to_vec()?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Write then rename so a half-written file is never mistaken for a cached one.
        let tmp = file.with_extension("part");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &file)?;
        Ok((bytes, kind))
    })
    .await;
    match result {
        Ok(Ok((bytes, kind))) => {
            ([(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, "public, max-age=604800")], bytes).into_response()
        }
        Ok(Err(e)) => {
            log::debug!("web image: {e:#}");
            StatusCode::NOT_FOUND.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `/img/{size}/{name}`: a TMDB image.
pub async fn tmdb(AxState(state): AxState<Arc<State>>, UrlPath((size, name)): UrlPath<(String, String)>) -> Response {
    let safe = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
    if !SIZES.contains(&size.as_str()) || !safe {
        return StatusCode::NOT_FOUND.into_response();
    }
    let file = state.images_dir.join(&size).join(&name);
    let url = format!("{TMDB_IMAGES}/{size}/{name}");
    cached(state, file, Some(url)).await
}

#[derive(serde::Deserialize)]
pub struct WebImage {
    u: String,
}

/// `/img/web?u=…`: an image by URL, only from the hosts metadata comes from (so the
/// server can't be used to fetch anything else).
pub async fn web(AxState(state): AxState<Arc<State>>, Query(q): Query<WebImage>) -> Response {
    if !allowed_web_image(&q.u) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let file = web_cache_file(&state.images_dir, &q.u);
    cached(state, file, Some(q.u)).await
}

/// Whether `url` is a plain https URL on one of the hosts metadata images come from. The
/// host has to be nothing but a hostname: "https://evil.example?.tmdb.org" or
/// "https://x.tmdb.org@evil.example" would otherwise pass a suffix check while fetching
/// somewhere else. No query or fragment either (image URLs don't have them), so the cache
/// can't be filled with endless variants of one URL.
fn allowed_web_image(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let hostname = !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    let plain_path = !path.contains(['?', '#', '\\']);
    hostname && plain_path && ["anilist.co", "tmdb.org"].iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// `/shot/{library}/{file}`: a screenshot the app has already taken, if any.
pub async fn shot(AxState(state): AxState<Arc<State>>, UrlPath((library_id, file_id)): UrlPath<(i64, i64)>) -> Response {
    let snapshot = state.snapshot();
    let Some(file) = snapshot.libraries.iter().find(|l| l.lib.id == library_id).and_then(|l| l.tree.file(file_id)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let path = state.shots_dir.join(format!("{}-{}", file.id, file.size)).join(THUMB_SHOT);
    cached(state.clone(), path, None).await
}

/// Whether the app has a screenshot for a file (so pages only link ones that exist).
pub fn has_shot(state: &State, file: &flex_core::MediaFile) -> bool {
    state.shots_dir.join(format!("{}-{}", file.id, file.size)).join(THUMB_SHOT).exists()
}

#[cfg(test)]
mod tests {
    use super::allowed_web_image;

    #[test]
    fn web_images_only_come_from_metadata_hosts() {
        assert!(allowed_web_image("https://s4.anilist.co/file/anilistcdn/media/anime/cover/large/bx1-abc.png"));
        assert!(allowed_web_image("https://image.tmdb.org/t/p/w500/abc.jpg"));
        for bad in [
            "https://evil.example?.tmdb.org",
            "https://evil.example#.tmdb.org",
            "https://x.tmdb.org@evil.example/a.png",
            "https://evil.example\\.tmdb.org/a.png",
            "https://image.tmdb.org:8080/a.png",
            "https://eviltmdb.org/a.png",
            "http://image.tmdb.org/a.png",
            "https://image.tmdb.org/a.png?v=1",
            "https://image.tmdb.org/a.png#x",
            "https:///a.png",
        ] {
            assert!(!allowed_web_image(bad), "{bad}");
        }
    }
}
