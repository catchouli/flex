//! Playing files in the browser: what's in a file (ffprobe), and ffmpeg commands that
//! turn it into something the browser can play (MP4 fragments streamed as they're made).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// ffmpeg and ffprobe: next to flex.exe (as bundled in releases), else on the PATH.
pub struct Tools {
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    probes: Mutex<HashMap<(i64, u64, i64), Arc<MediaInfo>>>,
    encoder: OnceLock<Option<Encoder>>,
}

fn find_tool(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(&exe)));
    let path_dirs = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    // Apps started from Finder don't get the shell's PATH.
    let extra = [PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")];
    beside.into_iter().chain(path_dirs.into_iter().map(|d| d.join(&exe))).chain(extra.iter().map(|d| d.join(&exe))).find(|p| p.is_file())
}

/// A command that doesn't flash a console window on Windows.
fn command(program: &Path) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

impl Tools {
    pub fn find() -> Self {
        let tools = Self {
            ffmpeg: find_tool("ffmpeg"),
            ffprobe: find_tool("ffprobe"),
            probes: Mutex::default(),
            encoder: OnceLock::new(),
        };
        match (&tools.ffmpeg, &tools.ffprobe) {
            (Some(f), Some(p)) => log::info!("web playback: {} and {}", f.display(), p.display()),
            _ => log::warn!("web playback needs ffmpeg and ffprobe next to Flex or on the PATH"),
        }
        tools
    }

    /// What's in a file (cached per file id, size and mtime).
    pub async fn probe(&self, key: (i64, u64, i64), path: &Path) -> Result<Arc<MediaInfo>> {
        if let Some(info) = self.probes.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Ok(info.clone());
        }
        let ffprobe = self.ffprobe.as_ref().context("ffprobe isn't installed")?;
        let out = command(ffprobe)
            .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
            .arg(path)
            .stdin(Stdio::null())
            .output()
            .await
            .context("running ffprobe")?;
        if !out.status.success() {
            bail!("ffprobe: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let raw: Probe = serde_json::from_slice(&out.stdout).context("reading ffprobe's output")?;
        let info = Arc::new(MediaInfo::from(raw));
        self.probes.lock().unwrap_or_else(|e| e.into_inner()).insert(key, info.clone());
        Ok(info)
    }

    /// The H.264 encoder to transcode with: a GPU one if it works here, else x264.
    pub async fn encoder(&self) -> Option<Encoder> {
        if let Some(e) = self.encoder.get() {
            return *e;
        }
        let found = self.detect_encoder().await;
        log::info!("web transcoding encoder: {found:?}");
        *self.encoder.get_or_init(|| found)
    }

    async fn detect_encoder(&self) -> Option<Encoder> {
        let ffmpeg = self.ffmpeg.as_ref()?;
        for encoder in [Encoder::Nvenc, Encoder::Qsv, Encoder::Amf, Encoder::VideoToolbox, Encoder::X264] {
            // Listed isn't enough (an NVENC build on a PC without NVIDIA): try a few frames.
            let ok = command(ffmpeg)
                .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=320x240:r=24:d=0.5"])
                .args(["-c:v", encoder.name(), "-pix_fmt", "yuv420p", "-f", "null", "-"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .is_ok_and(|s| s.success());
            if ok {
                return Some(encoder);
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoder {
    Nvenc,
    Qsv,
    Amf,
    VideoToolbox,
    X264,
}

impl Encoder {
    fn name(self) -> &'static str {
        match self {
            Encoder::Nvenc => "h264_nvenc",
            Encoder::Qsv => "h264_qsv",
            Encoder::Amf => "h264_amf",
            Encoder::VideoToolbox => "h264_videotoolbox",
            Encoder::X264 => "libx264",
        }
    }

    /// Good-looking 1080p at a size a home network handles easily.
    fn quality_args(self) -> &'static [&'static str] {
        match self {
            Encoder::Nvenc => &["-preset", "p4", "-rc", "vbr", "-cq", "23", "-b:v", "0", "-maxrate", "20M", "-bufsize", "40M"],
            Encoder::Qsv => &["-preset", "medium", "-global_quality", "23"],
            Encoder::Amf => &["-quality", "balanced", "-rc", "qvbr", "-qvbr_quality_level", "23"],
            Encoder::VideoToolbox => &["-b:v", "8M", "-maxrate", "16M"],
            Encoder::X264 => &["-preset", "veryfast", "-crf", "21", "-maxrate", "20M", "-bufsize", "40M"],
        }
    }
}

// ---------------------------------------------------------------------------------------
// ffprobe output

#[derive(Deserialize)]
struct Probe {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    format_name: Option<String>,
    duration: Option<String>,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    profile: Option<String>,
    pix_fmt: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    channels: Option<u32>,
    #[serde(default)]
    tags: HashMap<String, String>,
    #[serde(default)]
    disposition: HashMap<String, i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct VideoInfo {
    pub codec: String,
    pub profile: Option<String>,
    /// 8 or 10 (from the pixel format).
    pub bits: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct AudioInfo {
    /// Among the file's audio streams (ffmpeg `0:a:N`).
    pub index: usize,
    pub codec: String,
    pub channels: Option<u32>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub default: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SubtitleInfo {
    /// Among the file's subtitle streams (ffmpeg `0:s:N`).
    pub index: usize,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub default: bool,
    pub forced: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct MediaInfo {
    /// ffmpeg's container names, e.g. "matroska,webm" or "mov,mp4,m4a,3gp,3g2,mj2".
    pub container: String,
    pub duration: Option<f64>,
    pub video: Option<VideoInfo>,
    pub audio: Vec<AudioInfo>,
    pub subtitles: Vec<SubtitleInfo>,
}

impl From<Probe> for MediaInfo {
    fn from(p: Probe) -> Self {
        let tag = |s: &ProbeStream, k: &str| s.tags.get(k).or_else(|| s.tags.get(&k.to_uppercase())).cloned().filter(|v| !v.is_empty());
        let flag = |s: &ProbeStream, k: &str| s.disposition.get(k).is_some_and(|v| *v != 0);
        let kind = |s: &ProbeStream, k: &str| s.codec_type.as_deref() == Some(k);
        let video = p
            .streams
            .iter()
            .find(|s| kind(s, "video") && !flag(s, "attached_pic"))
            .map(|s| VideoInfo {
                codec: s.codec_name.clone().unwrap_or_default(),
                profile: s.profile.clone(),
                bits: if s.pix_fmt.as_deref().is_some_and(|f| f.contains("10") || f.contains("12")) { 10 } else { 8 },
                width: s.width.unwrap_or_default(),
                height: s.height.unwrap_or_default(),
            });
        let audio = p
            .streams
            .iter()
            .filter(|s| kind(s, "audio"))
            .enumerate()
            .map(|(index, s)| AudioInfo {
                index,
                codec: s.codec_name.clone().unwrap_or_default(),
                channels: s.channels,
                language: tag(s, "language"),
                title: tag(s, "title"),
                default: flag(s, "default"),
            })
            .collect();
        let subtitles = p
            .streams
            .iter()
            .filter(|s| kind(s, "subtitle"))
            .enumerate()
            .map(|(index, s)| SubtitleInfo {
                index,
                codec: s.codec_name.clone().unwrap_or_default(),
                language: tag(s, "language"),
                title: tag(s, "title"),
                default: flag(s, "default"),
                forced: flag(s, "forced"),
            })
            .collect();
        let format = p.format;
        Self {
            container: format.as_ref().and_then(|f| f.format_name.clone()).unwrap_or_default(),
            duration: format.and_then(|f| f.duration?.parse().ok()),
            video,
            audio,
            subtitles,
        }
    }
}

// ---------------------------------------------------------------------------------------
// Streaming

/// How a stream is made: what happens to the video and the audio.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VideoMode {
    /// Passed through untouched (the browser can decode it).
    Copy,
    /// Re-encoded as 8-bit H.264.
    H264,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AudioMode {
    Copy,
    Aac,
}

pub struct StreamRequest<'a> {
    pub path: &'a Path,
    pub start: f64,
    pub audio: Option<usize>,
    pub video: VideoMode,
    pub audio_mode: AudioMode,
    pub info: &'a MediaInfo,
}

/// Starts ffmpeg writing fragmented MP4 to its stdout. The process is killed when the
/// returned child is dropped (e.g. the browser seeks or leaves).
pub async fn start_stream(tools: &Tools, req: StreamRequest<'_>) -> Result<tokio::process::Child> {
    let ffmpeg = tools.ffmpeg.as_ref().context("ffmpeg isn't installed")?;
    let mut cmd = command(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
    if req.start > 0.0 {
        cmd.args(["-ss", &format!("{:.3}", req.start)]);
    }
    cmd.arg("-i").arg(req.path);
    // The main picture (not cover art), and the chosen audio track.
    cmd.args(["-map", "0:V:0?"]);
    if let Some(a) = req.audio.filter(|a| *a < req.info.audio.len()) {
        cmd.args(["-map", &format!("0:a:{a}")]);
    } else if !req.info.audio.is_empty() {
        cmd.args(["-map", "0:a:0"]);
    }
    cmd.args(["-sn", "-dn", "-map_chapters", "-1"]);
    match req.video {
        VideoMode::Copy => {
            cmd.args(["-c:v", "copy"]);
            if req.info.video.as_ref().is_some_and(|v| v.codec == "hevc") {
                cmd.args(["-tag:v", "hvc1"]); // what browsers expect for HEVC in MP4
            }
        }
        VideoMode::H264 => {
            let encoder = tools.encoder().await.context("no H.264 encoder works with this ffmpeg")?;
            cmd.args(["-vf", "format=yuv420p", "-c:v", encoder.name()]);
            cmd.args(encoder.quality_args());
            // A keyframe every 2s: playback starts quickly and fragments stay small.
            cmd.args(["-g", "48", "-force_key_frames", "expr:gte(t,n_forced*2)", "-profile:v", "high"]);
        }
    }
    match req.audio_mode {
        AudioMode::Copy => cmd.args(["-c:a", "copy"]),
        AudioMode::Aac => cmd.args(["-c:a", "aac", "-ac", "2", "-b:a", "192k"]),
    };
    cmd.args([
        "-avoid_negative_ts",
        "make_zero",
        "-movflags",
        "frag_keyframe+empty_moov+default_base_moof",
        "-frag_duration",
        "2000000",
        "-f",
        "mp4",
        "pipe:1",
    ]);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().context("starting ffmpeg")?;
    // Log what ffmpeg complains about (it's quiet unless something's wrong).
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                log::warn!("ffmpeg: {line}");
            }
        });
    }
    Ok(child)
}
