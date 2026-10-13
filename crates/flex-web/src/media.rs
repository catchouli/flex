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
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    /// Subtitle extractions under way, by cache file.
    extracting: Mutex<HashMap<PathBuf, Finished>>,
    /// Whether ffmpeg has libass (the `subtitles` filter), to burn in text subtitles.
    text_burn: OnceLock<bool>,
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
            locks: Mutex::default(),
            extracting: Mutex::default(),
            text_burn: OnceLock::new(),
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

    /// Whether this ffmpeg can burn in text subtitles (it has libass; Homebrew's
    /// doesn't, the bundled Windows build does).
    pub async fn can_burn_text(&self) -> bool {
        if let Some(can) = self.text_burn.get() {
            return *can;
        }
        let can = match &self.ffmpeg {
            Some(ffmpeg) => command(ffmpeg)
                .args(["-hide_banner", "-filters"])
                .stdin(Stdio::null())
                .output()
                .await
                .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.split_whitespace().nth(1) == Some("subtitles"))),
            None => false,
        };
        *self.text_burn.get_or_init(|| can)
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
    /// Text subtitles are drawn by the browser; pictures (Blu-ray/DVD) are burned in.
    pub image: bool,
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
    /// Fonts attached to the file (MKV), for its ASS subtitles: (index among the
    /// attachments, file name).
    #[serde(skip)]
    pub fonts: Vec<(usize, String)>,
}

/// Subtitle codecs that are pictures rather than text.
const IMAGE_SUBTITLES: [&str; 4] = ["hdmv_pgs_subtitle", "dvd_subtitle", "dvb_subtitle", "xsub"];

fn is_font(name: &str, mime: Option<&str>) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    ["ttf", "otf", "ttc", "otc", "woff", "woff2"].contains(&ext.as_str())
        || mime.is_some_and(|m| m.contains("font") || m.contains("truetype") || m.contains("opentype"))
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
                image: s.codec_name.as_deref().is_some_and(|c| IMAGE_SUBTITLES.contains(&c)),
                language: tag(s, "language"),
                title: tag(s, "title"),
                default: flag(s, "default"),
                forced: flag(s, "forced"),
            })
            .collect();
        let fonts = p
            .streams
            .iter()
            .filter(|s| kind(s, "attachment"))
            .enumerate()
            .filter_map(|(i, s)| {
                let name = tag(s, "filename")?;
                is_font(&name, tag(s, "mimetype").as_deref()).then_some((i, name))
            })
            .collect();
        let format = p.format;
        Self {
            container: format.as_ref().and_then(|f| f.format_name.clone()).unwrap_or_default(),
            duration: format.and_then(|f| f.duration?.parse().ok()),
            video,
            audio,
            subtitles,
            fonts,
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
    /// A subtitle track to burn in (needs `VideoMode::H264`). Text tracks need
    /// `text_burn` (their extracted file and fonts); picture tracks are overlaid as is.
    pub burn: Option<usize>,
    pub text_burn: Option<TextBurn>,
    /// Scale down to at most this many lines (needs `VideoMode::H264`).
    pub height: Option<u32>,
    pub info: &'a MediaInfo,
}

/// What ffmpeg needs to draw a text subtitle track: paths relative to `cwd` (keeps
/// drive letters out of the filter, where `:` needs escaping).
pub struct TextBurn {
    pub cwd: PathBuf,
    pub ass: String,
    pub fonts_dir: String,
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
    // The main picture (not cover art), converted if asked: picture subtitles overlaid,
    // text subtitles drawn by libass, scaled down; then the chosen audio track.
    if req.video == VideoMode::H264 {
        let picture_burn = req.burn.filter(|s| req.info.subtitles.get(*s).is_some_and(|s| s.image));
        let mut graph = match picture_burn {
            Some(s) => format!("[0:V:0][0:s:{s}]overlay=eof_action=pass"),
            None => "[0:V:0]null".to_owned(),
        };
        if let Some(t) = req.text_burn.as_ref().filter(|_| picture_burn.is_none()) {
            // Frames start at 0 after seeking; the subtitles use the file's own times.
            let shift = format!("{:.3}", req.start);
            graph.push_str(&format!(",setpts=PTS+{shift}/TB,subtitles=f={}:fontsdir={},setpts=PTS-{shift}/TB", t.ass, t.fonts_dir));
            cmd.current_dir(&t.cwd);
        }
        let source_height = req.info.video.as_ref().map_or(0, |v| v.height);
        if let Some(h) = req.height.filter(|h| *h < source_height) {
            graph.push_str(&format!(",scale=-2:{h}"));
        }
        graph.push_str(",format=yuv420p[v]");
        cmd.args(["-filter_complex", &graph, "-map", "[v]"]);
    } else {
        cmd.args(["-map", "0:V:0?"]);
    }
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
            cmd.args(["-c:v", encoder.name()]);
            cmd.args(encoder.quality_args());
            // Smaller pictures need less: cap the bitrate (later options win).
            let cap = match req.height {
                Some(h) if h <= 480 => Some(("2500k", "5M")),
                Some(h) if h <= 720 => Some(("5M", "10M")),
                _ => None,
            };
            if let Some((rate, buffer)) = cap {
                if encoder == Encoder::VideoToolbox {
                    cmd.args(["-b:v", rate]);
                }
                cmd.args(["-maxrate", rate, "-bufsize", buffer]);
            }
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
                // "Broken pipe": the browser stopped reading (seeked or left), as expected.
                if line.contains("Broken pipe") || line.contains("Error writing trailer") {
                    log::debug!("ffmpeg: {line}");
                } else {
                    log::warn!("ffmpeg: {line}");
                }
            }
        });
    }
    Ok(child)
}

// ---------------------------------------------------------------------------------------
// Subtitles and fonts

impl Tools {
    /// A text subtitle track as ASS (SRT and the like are converted), cached in
    /// `cache_dir`. Subtitles are spread through the whole file, so extracting them takes
    /// a while over a network share (~30s for a 1.3 GB episode); meanwhile the file grows
    /// as ffmpeg reads, and readers can follow it (see [`GrowingFile`]).
    pub fn subtitles(self: &Arc<Self>, cache_dir: &Path, key: (i64, u64), path: &Path, index: usize) -> Result<GrowingFile> {
        let out = cache_dir.join("subs").join(format!("{}-{}-{index}.ass", key.0, key.1));
        let done_marker = out.with_extension("ass.done");
        if done_marker.exists() {
            return Ok(GrowingFile { path: out, done: None });
        }
        let mut running = self.extracting.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(done) = running.get(&out) {
            return Ok(GrowingFile { path: out, done: Some(done.clone()) });
        }
        let ffmpeg = self.ffmpeg.clone().context("ffmpeg isn't installed")?;
        std::fs::create_dir_all(out.parent().unwrap_or(cache_dir))?;
        let (tx, rx) = tokio::sync::watch::channel(None);
        running.insert(out.clone(), rx.clone());
        drop(running);
        let (tools, input, file) = (self.clone(), path.to_path_buf(), out.clone());
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            // Written straight to the cache file and flushed as it goes, for readers to follow.
            let result = command(&ffmpeg)
                .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"])
                .arg(&input)
                .args(["-map", &format!("0:s:{index}"), "-c:s", "ass", "-vn", "-an", "-flush_packets", "1", "-f", "ass"])
                .arg(&file)
                .stdin(Stdio::null())
                .output()
                .await;
            let outcome = match result {
                Ok(o) if o.status.success() => {
                    let _ = std::fs::write(file.with_extension("ass.done"), b"");
                    log::info!("web: extracted subtitles {} in {:?}", file.display(), started.elapsed());
                    Ok(())
                }
                Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_owned()),
                Err(e) => Err(e.to_string()),
            };
            if let Err(e) = &outcome {
                log::warn!("web: extracting subtitles from {}: {e}", input.display());
                let _ = std::fs::remove_file(&file);
            }
            tools.extracting.lock().unwrap_or_else(|e| e.into_inner()).remove(&file);
            let _ = tx.send(Some(outcome));
        });
        Ok(GrowingFile { path: out, done: Some(rx) })
    }

    /// A text subtitle track from `from` seconds on, as ASS with the file's own times,
    /// written to stdout while ffmpeg reads (killed when dropped). Starts within a couple
    /// of seconds wherever `from` is, unlike a full extraction, which has to read
    /// everything before that point first.
    pub fn subtitles_from(&self, path: &Path, index: usize, from: f64) -> Result<tokio::process::Child> {
        let ffmpeg = self.ffmpeg.as_ref().context("ffmpeg isn't installed")?;
        command(ffmpeg)
            // A little earlier, for lines already showing at `from`.
            .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &format!("{:.3}", (from - 10.0).max(0.0)), "-copyts", "-i"])
            .arg(path)
            // The ASS writer otherwise holds events back until it has seen the first one.
            .args(["-map", &format!("0:s:{index}"), "-c:s", "ass", "-vn", "-an", "-flush_packets", "1", "-ignore_readorder", "1", "-f", "ass", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("starting ffmpeg")
    }

    /// The track's extracted ASS file and a folder with all the file's attached fonts,
    /// for burning the subtitles in; None until the track is fully extracted.
    pub async fn text_burn(self: &Arc<Self>, cache_dir: &Path, key: (i64, u64), path: &Path, index: usize) -> Result<Option<TextBurn>> {
        if !self.can_burn_text().await || !self.subtitles(cache_dir, key, path, index)?.is_complete() {
            return Ok(None);
        }
        let fonts_dir = format!("fonts/{}-{}/all", key.0, key.1);
        let dir = cache_dir.join(&fonts_dir);
        let lock = self.lock(&dir);
        let _guard = lock.lock().await;
        if !dir.join(".done").exists() {
            let ffmpeg = self.ffmpeg.as_ref().context("ffmpeg isn't installed")?;
            tokio::fs::create_dir_all(&dir).await?;
            // Dumps every attachment under its own name (ffmpeg complains that there's no
            // output, but does it).
            let _ = command(ffmpeg)
                .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-dump_attachment:t", "", "-i"])
                .arg(path)
                .current_dir(&dir)
                .stdin(Stdio::null())
                .output()
                .await;
            tokio::fs::write(dir.join(".done"), b"").await?;
        }
        Ok(Some(TextBurn {
            cwd: cache_dir.to_path_buf(),
            ass: format!("subs/{}-{}-{index}.ass", key.0, key.1),
            fonts_dir,
        }))
    }

    /// A font attached to the file, cached in `cache_dir`.
    pub async fn font(&self, cache_dir: &Path, key: (i64, u64), path: &Path, attachment: usize, name: &str) -> Result<PathBuf> {
        let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
        let out = cache_dir.join("fonts").join(format!("{}-{}", key.0, key.1)).join(format!("{attachment}-{safe}"));
        let lock = self.lock(&out);
        let _guard = lock.lock().await;
        if out.exists() {
            return Ok(out);
        }
        let ffmpeg = self.ffmpeg.as_ref().context("ffmpeg isn't installed")?;
        if let Some(dir) = out.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        // Attachments sit in the file's header, so this is quick. ffmpeg complains that
        // there's no output, but dumps the attachment anyway.
        let tmp = out.with_extension("part");
        let _ = command(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
            .arg(format!("-dump_attachment:t:{attachment}"))
            .arg(&tmp)
            .arg("-i")
            .arg(path)
            .stdin(Stdio::null())
            .output()
            .await
            .context("running ffmpeg")?;
        if !tmp.exists() {
            bail!("couldn't extract font {name}");
        }
        tokio::fs::rename(&tmp, &out).await?;
        Ok(out)
    }

    /// The time of the keyframe a copied stream starting at `at` really starts from (ffmpeg
    /// can only start copying at a keyframe), so subtitles and the clock line up.
    pub async fn keyframe_before(&self, path: &Path, at: f64) -> Option<f64> {
        let ffprobe = self.ffprobe.as_ref()?;
        let from = (at - 20.0).max(0.0);
        let out = command(ffprobe)
            .args(["-v", "error", "-select_streams", "V:0", "-skip_frame", "nokey", "-show_entries", "frame=pts_time,best_effort_timestamp_time", "-of", "csv=p=0"])
            .args(["-read_intervals", &format!("{from:.3}%{:.3}", at + 0.05)])
            .arg(path)
            .stdin(Stdio::null())
            .output()
            .await
            .ok()?;
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.split(',').find_map(|v| v.trim().parse::<f64>().ok()))
            .filter(|t| *t <= at + 0.001)
            .reduce(f64::max)
    }

    /// One async lock per output file, so two requests don't both run ffmpeg for it.
    fn lock(&self, out: &Path) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        locks.entry(out.to_path_buf()).or_default().clone()
    }
}

/// A system font with Chinese, Japanese and Korean characters, for subtitles whose own
/// fonts lack them (Japanese first: kanji look right): its path and family name.
pub fn cjk_fallback_font() -> Option<(PathBuf, &'static str)> {
    let candidates: &[(&str, &str)] = if cfg!(windows) {
        &[
            (r"C:\Windows\Fonts\YuGothM.ttc", "Yu Gothic Medium"),
            (r"C:\Windows\Fonts\msgothic.ttc", "MS Gothic"),
            (r"C:\Windows\Fonts\msyh.ttc", "Microsoft YaHei"),
            (r"C:\Windows\Fonts\malgun.ttf", "Malgun Gothic"),
        ]
    } else if cfg!(target_os = "macos") {
        &[
            ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", "Hiragino Sans"),
            ("/System/Library/Fonts/Hiragino Sans GB.ttc", "Hiragino Sans GB"),
            ("/System/Library/Fonts/PingFang.ttc", "PingFang SC"),
        ]
    } else {
        &[
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", "Noto Sans CJK JP"),
            ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", "Noto Sans CJK JP"),
        ]
    };
    candidates.iter().map(|(p, family)| (PathBuf::from(p), *family)).find(|(p, _)| p.is_file())
}

/// Reports when a writer finishes: None until then, then Ok or the error.
type Finished = tokio::sync::watch::Receiver<Option<Result<(), String>>>;

/// A file that may still be being written: `done` is None once it's complete, else it
/// reports when the writer finishes.
pub struct GrowingFile {
    pub path: PathBuf,
    done: Option<Finished>,
}

impl GrowingFile {
    pub fn is_complete(&self) -> bool {
        self.done.is_none()
    }

    /// The file's contents as they become available, ending when it's complete.
    pub fn stream(self) -> impl futures_util::Stream<Item = std::io::Result<bytes::Bytes>> {
        use tokio::io::AsyncReadExt;
        futures_util::stream::unfold((self, None::<tokio::fs::File>, false), |(mut g, mut file, mut finished)| async move {
            loop {
                if file.is_none() {
                    match tokio::fs::File::open(&g.path).await {
                        Ok(f) => file = Some(f),
                        // Not created yet: wait for the writer.
                        Err(_) if !finished && g.done.is_some() => {}
                        Err(e) => return (!finished).then_some((Err(e), (g, file, true))),
                    }
                }
                if let Some(f) = file.as_mut() {
                    let mut buf = vec![0; 64 * 1024];
                    match f.read(&mut buf).await {
                        Ok(0) => {}
                        Ok(n) => {
                            buf.truncate(n);
                            return Some((Ok(bytes::Bytes::from(buf)), (g, file, finished)));
                        }
                        Err(e) => return Some((Err(e), (g, file, true))),
                    }
                }
                // At the end of what's written so far.
                if finished {
                    return None;
                }
                let status = g.done.as_ref().map(|d| d.borrow().clone());
                match status {
                    // Complete (or the writer has finished): one more read picks up the rest.
                    None | Some(Some(Ok(()))) => finished = true,
                    Some(Some(Err(e))) => return Some((Err(std::io::Error::other(e)), (g, file, true))),
                    Some(None) => {
                        if let Some(done) = g.done.as_mut() {
                            let _ = tokio::time::timeout(std::time::Duration::from_millis(250), done.changed()).await;
                        }
                    }
                }
            }
        })
    }
}
