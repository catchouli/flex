//! What audio tracks a file has, for details pages: looked up in the background by a
//! headless mpv that opens the file (paused, nothing decoded) and reads its track list.
//! Results are kept in the database (`media_audio`), so each file is opened once.
//!
//! As with screenshots, pages ask every frame while they're showing; requests not
//! repeated for a few seconds are dropped, newest first.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use eframe::egui;
use flex_core::audio::AudioTrack;

use crate::mpv::{Event, Mpv};

const FORGET: Duration = Duration::from_secs(3);
/// Longest wait for a file to open (slow network shares, discs).
const TIMEOUT: f64 = 30.0;
/// Longest wait for Atmos to show up in a track's profile.
const PROFILE_WAIT: Duration = Duration::from_millis(1500);

struct Job {
    path: PathBuf,
    asked: Instant,
}

#[derive(Default)]
struct Queue {
    jobs: HashMap<i64, Job>,
    running: Option<i64>,
}

/// A file id and its audio tracks (or why they couldn't be read).
pub type Probed = (i64, Result<Vec<AudioTrack>, String>);

pub struct AudioProbe {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    pub results: Receiver<Probed>,
}

impl AudioProbe {
    pub fn new(ctx: &egui::Context) -> Self {
        let queue: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        let (tx, results) = channel();
        let (q, ctx) = (queue.clone(), ctx.clone());
        std::thread::spawn(move || work(&q, &tx, &ctx));
        Self { queue, results }
    }

    /// Asks for a file's tracks (again, to keep the request alive).
    pub fn request(&self, file_id: i64, path: PathBuf) {
        let (lock, wake) = &*self.queue;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        if q.running != Some(file_id) {
            q.jobs.insert(
                file_id,
                Job {
                    path,
                    asked: Instant::now(),
                },
            );
            wake.notify_one();
        }
    }
}

/// Worker loop: looks at the most recently requested file, forever.
fn work(queue: &(Mutex<Queue>, Condvar), tx: &Sender<Probed>, ctx: &egui::Context) {
    let mut mpv: Option<Mpv> = None;
    loop {
        let (id, job) = {
            let (lock, wake) = queue;
            let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                q.jobs.retain(|_, j| j.asked.elapsed() < FORGET);
                if let Some(id) = q.jobs.iter().max_by_key(|(_, j)| j.asked).map(|(k, _)| *k) {
                    let job = q.jobs.remove(&id).expect("just found");
                    q.running = Some(id);
                    break (id, job);
                }
                q = wake
                    .wait_timeout(q, Duration::from_secs(1))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        };
        let started = Instant::now();
        let result = probe(&mut mpv, &job);
        match &result {
            Ok(tracks) => log::debug!(
                "audio of {}: {} tracks in {:?}",
                job.path.display(),
                tracks.len(),
                started.elapsed()
            ),
            Err(e) => {
                log::warn!("audio tracks of {}: {e:#}", job.path.display());
                mpv = None; // start afresh in case it's stuck
            }
        }
        queue.0.lock().unwrap_or_else(|e| e.into_inner()).running = None;
        if tx.send((id, result.map_err(|e| format!("{e:#}")))).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// Opens the file with `mpv` (created on first use) and reads its audio tracks.
fn probe(mpv: &mut Option<Mpv>, job: &Job) -> Result<Vec<AudioTrack>> {
    if mpv.is_none() {
        *mpv = Some(Mpv::headless(&[
            ("vo", "null"),
            ("ao", "null"),
            ("pause", "yes"),
            ("hwdec", "no"),
            // Only the headers are wanted: don't read ahead over the network.
            ("cache", "no"),
            ("demuxer-readahead-secs", "0"),
            ("load-scripts", "no"),
        ])?);
    }
    let mpv = mpv.as_ref().expect("just created");
    mpv.load(&job.path)?;
    match mpv.wait_event(TIMEOUT) {
        Some(Event::FileLoaded) => {}
        Some(Event::EndFile { error: Some(e), .. }) => bail!("{e}"),
        Some(Event::EndFile { .. }) => bail!("the file ended before it opened"),
        Some(Event::Shutdown) => bail!("mpv shut down"),
        None => {
            let _ = mpv.command(&["stop"]);
            bail!("timed out");
        }
    }
    // TrueHD and DD+ only say they're Atmos once a packet has been parsed, which can be
    // just after the file opens. Other files wait the whole time: they never will.
    let wait_until = Instant::now() + PROFILE_WAIT;
    let mut tracks = audio_tracks(mpv);
    while tracks
        .iter()
        .any(|t| matches!(t.codec.as_str(), "truehd" | "eac3") && t.profile.is_empty())
        && Instant::now() < wait_until
    {
        std::thread::sleep(Duration::from_millis(50));
        tracks = audio_tracks(mpv);
    }
    let _ = mpv.command(&["stop"]);
    // Wait for the stop so the next file's events don't mix with this one's.
    let deadline = Instant::now() + Duration::from_secs(5);
    while let Some(event) = mpv.wait_event(
        deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f64(),
    ) {
        if matches!(event, Event::EndFile { .. } | Event::Shutdown) {
            break;
        }
    }
    Ok(tracks)
}

/// The audio tracks of the file mpv has open.
fn audio_tracks(mpv: &Mpv) -> Vec<AudioTrack> {
    let count = mpv.get_i64("track-list/count").unwrap_or(0);
    (0..count)
        .filter_map(|i| {
            let prop = |p: &str| {
                mpv.get_string(&format!("track-list/{i}/{p}"))
                    .unwrap_or_default()
            };
            if prop("type") != "audio" {
                return None;
            }
            Some(AudioTrack {
                lang: prop("lang"),
                title: prop("title"),
                codec: prop("codec"),
                profile: prop("codec-profile"),
                channels: mpv
                    .get_i64(&format!("track-list/{i}/demux-channel-count"))
                    .and_then(|c| u32::try_from(c).ok())
                    .unwrap_or(0),
                layout: prop("demux-channels"),
                default: mpv
                    .get_flag(&format!("track-list/{i}/default"))
                    .unwrap_or(false),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the audio tracks of a made-up file with a 5.1 and a stereo track (needs
    /// libmpv and ffmpeg): `cargo test -p flex reads_audio_tracks -- --ignored`.
    #[test]
    #[ignore]
    fn reads_audio_tracks() {
        let video = std::env::temp_dir().join(format!("flex-audio-{}.mkv", std::process::id()));
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=duration=2:rate=10:size=160x90",
            ])
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=duration=2",
                "-f",
                "lavfi",
                "-i",
                "sine=duration=2",
            ])
            .args([
                "-map", "0", "-map", "1", "-map", "2", "-c:a:0", "ac3", "-ac:a:0", "6", "-c:a:1",
                "aac", "-ac:a:1", "2",
            ])
            .args([
                "-metadata:s:a:0",
                "language=eng",
                "-metadata:s:a:1",
                "language=jpn",
            ])
            .arg(&video)
            .status()
            .unwrap();
        assert!(made.success());
        let mut mpv = None;
        let job = Job {
            path: video.clone(),
            asked: Instant::now(),
        };
        let tracks = probe(&mut mpv, &job).unwrap();
        // Twice with the same mpv, as the worker does.
        assert_eq!(probe(&mut mpv, &job).unwrap(), tracks);
        let _ = std::fs::remove_file(&video);
        let described: Vec<String> = tracks.iter().map(AudioTrack::describe).collect();
        assert_eq!(
            described,
            ["English · Dolby Digital 5.1", "Japanese · AAC stereo"],
            "{tracks:?}"
        );
    }
}
