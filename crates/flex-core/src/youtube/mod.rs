//! YouTube through yt-dlp: Flex never parses YouTube itself. yt-dlp (kept up to date in
//! the data folder's `tools`) turns a video into stream URLs, which mpv plays like any
//! other file. When YouTube changes something, updating yt-dlp is normally the fix.
//!
//! The streams come straight from YouTube's video servers, so there are no ads in them.

pub mod feeds;
pub mod sponsorblock;
pub mod tools;

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The programs YouTube playback needs. Deno is optional, but yt-dlp needs a JavaScript
/// runtime for some of YouTube's checks and may lose formats without one.
#[derive(Clone, Debug)]
pub struct Tools {
    pub yt_dlp: PathBuf,
    pub deno: Option<PathBuf>,
}

/// Where Flex keeps yt-dlp and Deno.
pub fn tools_dir() -> PathBuf {
    crate::data_dir().join("tools")
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() }
}

/// `name` in `dir`, else on the PATH.
fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    let file = exe(name);
    let local = dir.join(&file);
    if local.is_file() {
        return Some(local);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(&file)).find(|p| p.is_file())
}

impl Tools {
    /// yt-dlp and Deno from the tools folder, else the PATH.
    pub fn find() -> Option<Self> {
        let dir = tools_dir();
        Some(Self { yt_dlp: find(&dir, "yt-dlp")?, deno: find(&dir, "deno") })
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.yt_dlp);
        cmd.args(["--no-config", "--no-playlist", "--no-warnings"]);
        if let Some(deno) = &self.deno {
            cmd.arg("--js-runtimes").arg(format!("deno:{}", deno.display()));
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd
    }

    pub fn yt_dlp_version(&self) -> Option<String> {
        let out = self.command().arg("--version").output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    /// Finds the streams for a video (a URL or an 11-character id), at most `max_height`
    /// pixels tall. Takes a few seconds (it asks YouTube).
    pub fn resolve(&self, video: &str, max_height: u32) -> Result<Video> {
        let url = match video_id(video) {
            Some(id) => format!("https://www.youtube.com/watch?v={id}"),
            None => video.to_owned(),
        };
        let out = self
            .command()
            .args(["--dump-single-json", "--format", &format_choice(max_height), &url])
            .output()
            .with_context(|| format!("running {}", self.yt_dlp.display()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let line = err.lines().rev().find(|l| l.starts_with("ERROR")).unwrap_or(err.trim());
            bail!("yt-dlp: {}", line.trim_start_matches("ERROR: "));
        }
        parse(&out.stdout)
    }
}

/// yt-dlp's format choice: the best video up to `max_height` without AV1 (which many
/// GPUs can't decode, e.g. the TV PC's GTX 1070 Ti), plus the best audio; else the best
/// single file.
fn format_choice(max_height: u32) -> String {
    format!("bv*[vcodec!^=av01][height<={max_height}]+ba/b[height<={max_height}]/b")
}

/// A video ready to play.
#[derive(Clone, Debug, PartialEq)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: String,
    /// Seconds; `None` for live streams.
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    /// The video stream, or a file with both.
    pub video_url: String,
    /// A separate audio stream, when there is one (usual on YouTube).
    pub audio_url: Option<String>,
    pub height: Option<u32>,
    /// (start in seconds, title)
    pub chapters: Vec<(f64, String)>,
}

impl Video {
    /// What to hand mpv: the stream, or both streams joined with mpv's EDL (each URL is
    /// length-prefixed, so it needs no escaping).
    pub fn mpv_url(&self) -> String {
        match &self.audio_url {
            None => self.video_url.clone(),
            Some(audio) => {
                let part = |url: &str| format!("!no_clip;!no_chapters;%{}%{url}", url.len());
                format!("edl://{};!new_stream;{}", part(&self.video_url), part(audio))
            }
        }
    }
}

#[derive(Deserialize)]
struct Info {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    uploader: Option<String>,
    duration: Option<f64>,
    thumbnail: Option<String>,
    url: Option<String>,
    height: Option<u32>,
    #[serde(default)]
    requested_formats: Vec<Format>,
    #[serde(default)]
    chapters: Option<Vec<Chapter>>,
}

#[derive(Deserialize)]
struct Format {
    url: String,
    #[serde(default)]
    vcodec: Option<String>,
    height: Option<u32>,
}

#[derive(Deserialize)]
struct Chapter {
    start_time: f64,
    #[serde(default)]
    title: String,
}

fn parse(json: &[u8]) -> Result<Video> {
    let info: Info = serde_json::from_slice(json).context("reading yt-dlp's output")?;
    let has_video = |f: &&Format| f.vcodec.as_deref().is_some_and(|c| c != "none");
    let (video_url, audio_url, height) = match &info.requested_formats[..] {
        [] => (info.url.context("yt-dlp gave no stream")?, None, info.height),
        formats => {
            let video = formats.iter().find(has_video).unwrap_or(&formats[0]);
            let audio = formats.iter().find(|f| !std::ptr::eq(*f, video)).map(|f| f.url.clone());
            (video.url.clone(), audio, video.height)
        }
    };
    Ok(Video {
        id: info.id,
        title: info.title,
        channel: info.channel.or(info.uploader).unwrap_or_default(),
        duration: info.duration,
        thumbnail: info.thumbnail,
        video_url,
        audio_url,
        height,
        chapters: info.chapters.unwrap_or_default().into_iter().map(|c| (c.start_time, c.title)).collect(),
    })
}

/// The 11-character video id in a YouTube URL (watch, youtu.be, shorts, embed, live), or
/// the string itself if it is one.
pub fn video_id(s: &str) -> Option<String> {
    let valid = |id: &str| id.len() == 11 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let s = s.trim();
    if valid(s) {
        return Some(s.to_owned());
    }
    let rest = s.split_once("://").map_or(s, |(_, r)| r);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.trim_start_matches("www.").trim_start_matches("m.").trim_start_matches("music.");
    let candidate = match host {
        "youtu.be" => path.split(['?', '&', '#', '/']).next(),
        "youtube.com" | "youtube-nocookie.com" => {
            if let Some(query) = path.strip_prefix("watch").and_then(|p| p.split_once('?')).map(|(_, q)| q) {
                query.split(['&', '#']).find_map(|kv| kv.strip_prefix("v="))
            } else {
                ["shorts/", "embed/", "live/", "v/"].iter().find_map(|p| path.strip_prefix(p)).and_then(|r| r.split(['?', '&', '#', '/']).next())
            }
        }
        _ => None,
    };
    candidate.filter(|id| valid(id)).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_video_ids() {
        let id = "aqz-KE-bpKQ";
        for url in [
            "aqz-KE-bpKQ",
            "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
            "https://youtube.com/watch?feature=share&v=aqz-KE-bpKQ&t=30",
            "https://m.youtube.com/watch?v=aqz-KE-bpKQ#t=1",
            "https://youtu.be/aqz-KE-bpKQ?si=abc",
            "https://www.youtube.com/shorts/aqz-KE-bpKQ",
            "https://www.youtube.com/embed/aqz-KE-bpKQ?start=4",
            "https://www.youtube.com/live/aqz-KE-bpKQ",
            "youtube.com/watch?v=aqz-KE-bpKQ",
        ] {
            assert_eq!(video_id(url).as_deref(), Some(id), "{url}");
        }
        for not in ["https://example.com/watch?v=aqz-KE-bpKQ", "https://youtube.com/watch?v=short", "hello", "https://youtube.com/@channel"] {
            assert_eq!(video_id(not), None, "{not}");
        }
    }

    #[test]
    fn parses_separate_streams_and_joins_them_for_mpv() {
        let json = br#"{"id":"abc","title":"T","channel":"C","duration":12.5,"thumbnail":"https://i.ytimg.com/x.jpg",
            "requested_formats":[{"url":"https://v/1?a=b,c;d","vcodec":"vp9","height":1080},{"url":"https://a/2","vcodec":"none"}],
            "chapters":[{"start_time":0.0,"title":"Intro"},{"start_time":5.0,"title":"Main"}]}"#;
        let v = parse(json).unwrap();
        assert_eq!((v.video_url.as_str(), v.audio_url.as_deref(), v.height), ("https://v/1?a=b,c;d", Some("https://a/2"), Some(1080)));
        assert_eq!(v.chapters, vec![(0.0, "Intro".to_owned()), (5.0, "Main".to_owned())]);
        // Commas and semicolons in URLs are safe thanks to the length prefixes.
        assert_eq!(
            v.mpv_url(),
            "edl://!no_clip;!no_chapters;%19%https://v/1?a=b,c;d;!new_stream;!no_clip;!no_chapters;%11%https://a/2"
        );
    }

    #[test]
    fn parses_a_single_file() {
        let v = parse(br#"{"id":"abc","title":"T","uploader":"U","url":"https://f","height":360}"#).unwrap();
        assert_eq!((v.video_url.as_str(), v.audio_url.as_deref(), v.channel.as_str()), ("https://f", None, "U"));
        assert_eq!(v.mpv_url(), "https://f");
    }

    /// Asks YouTube for real (needs yt-dlp): `cargo test -p flex-core youtube -- --ignored`.
    #[test]
    #[ignore]
    fn resolves_a_real_video() {
        let tools = Tools::find().expect("yt-dlp in the tools folder or on the PATH");
        let v = tools.resolve("https://www.youtube.com/watch?v=aqz-KE-bpKQ", 1080).unwrap();
        assert_eq!(v.id, "aqz-KE-bpKQ");
        assert!(v.title.contains("Big Buck Bunny"), "{}", v.title);
        assert!(v.audio_url.is_some() && v.height.is_some_and(|h| h <= 1080));
    }
}
