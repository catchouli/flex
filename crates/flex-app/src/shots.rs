//! Screenshots: a handful of frames spread through a video, grabbed in the background by a
//! headless mpv (`vo=image`) and kept as JPEGs in the data folder's `shots` cache.
//!
//! Screens ask for shots every frame while they're on screen. Only shots asked for in the
//! last few seconds are taken, newest request first, so scrolling past a season or leaving
//! the page doesn't leave a long queue of work behind.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use eframe::egui;

use crate::mpv::{Event, Mpv};

/// Shots per video.
pub const COUNT: usize = 12;
/// The shot used as an episode's thumbnail when TMDB has no still.
pub const THUMB: usize = 3;
/// Requests not repeated for this long are dropped.
const FORGET: Duration = Duration::from_secs(3);
/// Longest wait for one shot (slow network shares).
const TIMEOUT: f64 = 60.0;
const WORKERS: usize = 2;
/// Width of the saved JPEGs.
const WIDTH: u32 = 1280;

/// Where shot `index` is taken, as a percentage of the running time: from 5% (past cold
/// opens) to 90% (before the credits).
pub fn percent(index: usize) -> f64 {
    5.0 + 85.0 * index as f64 / (COUNT - 1) as f64
}

type Key = (i64, usize);

struct Job {
    video: PathBuf,
    out: PathBuf,
    asked: Instant,
}

#[derive(Default)]
struct Queue {
    jobs: HashMap<Key, Job>,
    /// Being taken right now (so they aren't queued twice).
    running: Vec<Key>,
}

#[derive(Clone, PartialEq, Eq)]
pub enum Shot {
    /// The image is ready: an image path for [`crate::images::Images::get`].
    Ready(String),
    Pending,
    Failed(String),
}

pub struct Shots {
    dir: PathBuf,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    results: Receiver<(Key, Result<(), String>)>,
    known: HashMap<Key, Shot>,
}

impl Shots {
    pub fn new(ctx: &egui::Context, dir: PathBuf) -> Self {
        let queue: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        let (tx, results) = channel();
        for worker in 0..WORKERS {
            let (queue, tx, ctx) = (queue.clone(), tx.clone(), ctx.clone());
            let scratch = dir.join(format!("tmp-{worker}"));
            std::thread::spawn(move || work(&queue, &tx, &ctx, &scratch));
        }
        Self { dir, queue, results, known: HashMap::new() }
    }

    /// Shot `index` of a video, starting it in the background if needed. `size` tells a
    /// replaced file apart from the one the shots were taken of.
    pub fn get(&mut self, file_id: i64, size: u64, video: &Path, index: usize) -> Shot {
        let key = (file_id, index);
        if let Some(shot @ (Shot::Ready(_) | Shot::Failed(_))) = self.known.get(&key) {
            return shot.clone();
        }
        let out = self.dir.join(format!("{file_id}-{size}")).join(format!("{:02}.jpg", index + 1));
        if !self.known.contains_key(&key) && out.exists() {
            let shot = Shot::Ready(format!("file:{}", out.display()));
            self.known.insert(key, shot.clone());
            return shot;
        }
        self.known.insert(key, Shot::Pending);
        let (lock, wake) = &*self.queue;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        if !q.running.contains(&key) {
            q.jobs.insert(key, Job { video: video.to_owned(), out, asked: Instant::now() });
            wake.notify_one();
        }
        Shot::Pending
    }

    /// Picks up finished shots. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok((key, result)) = self.results.try_recv() {
            match result {
                Ok(()) => {
                    // Looked up again (and found on disk) on the next request.
                    self.known.remove(&key);
                }
                Err(e) => {
                    self.known.insert(key, Shot::Failed(e));
                }
            }
        }
        // Pending shots that were dropped from the queue are asked for again next time.
        let (lock, _) = &*self.queue;
        let q = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.known.retain(|k, s| *s != Shot::Pending || q.jobs.contains_key(k) || q.running.contains(k));
    }
}

/// Worker loop: takes the most recently requested shot, forever.
fn work(queue: &(Mutex<Queue>, Condvar), tx: &Sender<(Key, Result<(), String>)>, ctx: &egui::Context, scratch: &Path) {
    let mut mpv: Option<Mpv> = None;
    loop {
        let (key, job) = {
            let (lock, wake) = queue;
            let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                q.jobs.retain(|_, j| j.asked.elapsed() < FORGET);
                let newest = q.jobs.iter().max_by_key(|(_, j)| j.asked).map(|(k, _)| *k);
                if let Some(key) = newest {
                    let job = q.jobs.remove(&key).expect("just found");
                    q.running.push(key);
                    break (key, job);
                }
                q = wake.wait_timeout(q, Duration::from_secs(1)).unwrap_or_else(|e| e.into_inner()).0;
            }
        };
        let started = Instant::now();
        let result = take(&mut mpv, &job, key.1, scratch);
        match &result {
            Ok(()) => log::debug!("shot {} of {} in {:?}", key.1 + 1, job.video.display(), started.elapsed()),
            Err(e) => {
                log::warn!("screenshot of {}: {e:#}", job.video.display());
                mpv = None; // start afresh in case it's stuck
            }
        }
        queue.0.lock().unwrap_or_else(|e| e.into_inner()).running.retain(|k| *k != key);
        if tx.send((key, result.map_err(|e| format!("{e:#}")))).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// Takes one shot with `mpv` (created on first use).
fn take(mpv: &mut Option<Mpv>, job: &Job, index: usize, scratch: &Path) -> Result<()> {
    if mpv.is_none() {
        let scale = format!("scale=w={WIDTH}:h=-2");
        let outdir = scratch.to_string_lossy().into_owned();
        *mpv = Some(Mpv::headless(&[
            ("vo", "image"),
            ("vo-image-format", "jpg"),
            ("vo-image-jpeg-quality", "85"),
            ("vo-image-outdir", &outdir),
            ("vf", &scale),
            ("frames", "1"),
            ("ao", "null"),
            ("aid", "no"),
            ("sid", "no"),
            ("hwdec", "no"),
            // The nearest keyframe is close enough, and much quicker.
            ("hr-seek", "no"),
        ])?);
    }
    let mpv = mpv.as_ref().expect("just created");
    // vo=image numbers its files from 1 for each video: start with an empty folder.
    let _ = std::fs::remove_dir_all(scratch);
    std::fs::create_dir_all(scratch).with_context(|| format!("creating {}", scratch.display()))?;
    mpv.set_string("start", &format!("{:.1}%", percent(index)))?;
    mpv.command(&["loadfile", &job.video.to_string_lossy()])?;
    let deadline = Instant::now() + Duration::from_secs_f64(TIMEOUT);
    loop {
        let left = deadline.saturating_duration_since(Instant::now()).as_secs_f64();
        match mpv.wait_event(left) {
            Some(Event::EndFile { error: Some(e), .. }) => bail!("{e}"),
            Some(Event::EndFile { .. }) => break,
            Some(Event::Shutdown) => bail!("mpv shut down"),
            Some(Event::FileLoaded) => {}
            None => {
                let _ = mpv.command(&["stop"]);
                bail!("timed out");
            }
        }
    }
    let image = std::fs::read_dir(scratch)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|x| x == "jpg"))
        .context("no frame (the video may be shorter than expected)")?;
    if let Some(dir) = job.out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::rename(&image, &job.out).or_else(|_| std::fs::copy(&image, &job.out).map(|_| ()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Takes every shot of `FLEX_SHOT_VIDEO` into `FLEX_SHOT_OUT` (needs libmpv; run by hand).
    #[test]
    #[ignore]
    fn takes_shots() {
        let video = PathBuf::from(std::env::var("FLEX_SHOT_VIDEO").expect("FLEX_SHOT_VIDEO"));
        let out = PathBuf::from(std::env::var("FLEX_SHOT_OUT").expect("FLEX_SHOT_OUT"));
        let mut mpv = None;
        for index in 0..COUNT {
            let started = Instant::now();
            let job = Job { video: video.clone(), out: out.join(format!("{:02}.jpg", index + 1)), asked: Instant::now() };
            take(&mut mpv, &job, index, &out.join("tmp")).unwrap();
            eprintln!("shot {index}: {:?}", started.elapsed());
        }
    }
}
