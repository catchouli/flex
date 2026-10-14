//! Posters, stills and actor photos: downloaded once in the background, kept on disk in
//! the data folder's `images` cache, and turned into textures only while they're on screen.

use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

/// Which rendition to fetch; also the largest size kept in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Size {
    /// Show/film posters.
    Poster,
    /// Episode stills on an episode's page.
    Still,
    /// Actor photos.
    Profile,
    /// Wide background images.
    Backdrop,
    /// Screenshots taken locally (see [`crate::shots`]).
    Shot,
}

impl Size {
    /// TMDB image size name, and the longest side kept after decoding.
    fn spec(self) -> (&'static str, u32) {
        match self {
            Size::Poster => ("w342", 513),
            Size::Still => ("w780", 780),
            Size::Profile => ("w185", 278),
            Size::Backdrop => ("w1280", 1280),
            Size::Shot => ("shot", 1280),
        }
    }
}

const TMDB_IMAGES: &str = "https://image.tmdb.org/t/p";
/// Textures kept in memory; the least recently drawn are dropped beyond this.
const MAX_TEXTURES: usize = 300;
const WORKERS: usize = 3;

struct Job {
    key: String,
    url: String,
    file: PathBuf,
    max_side: u32,
}

enum Entry {
    Loading,
    Ready { texture: TextureHandle, last_used: u64 },
    Failed,
}

pub struct Images {
    ctx: egui::Context,
    dir: PathBuf,
    jobs: Sender<Job>,
    results: Receiver<(String, Option<ColorImage>)>,
    entries: HashMap<String, Entry>,
    frame: u64,
}

fn hashed(s: &str) -> String {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Reads the cached file or downloads it, then decodes and shrinks it.
fn load(job: &Job, agent: &ureq::Agent) -> anyhow::Result<ColorImage> {
    let bytes = match std::fs::read(&job.file) {
        Ok(bytes) => bytes,
        Err(e) if job.url.is_empty() => return Err(e.into()),
        Err(_) => {
            let bytes = agent.get(&job.url).call()?.body_mut().with_config().limit(20_000_000).read_to_vec()?;
            if let Some(dir) = job.file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // Write then rename so a half-written file is never mistaken for a cached one.
            let tmp = job.file.with_extension("part");
            std::fs::write(&tmp, &bytes)?;
            std::fs::rename(&tmp, &job.file)?;
            bytes
        }
    };
    let image = image::load_from_memory(&bytes)?;
    let image = if image.width().max(image.height()) > job.max_side {
        image.thumbnail(job.max_side, job.max_side)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

impl Images {
    pub fn new(ctx: &egui::Context, dir: PathBuf) -> Self {
        let (jobs, job_rx) = channel::<Job>();
        let (result_tx, results) = channel();
        let job_rx = Arc::new(Mutex::new(job_rx));
        for _ in 0..WORKERS {
            let (job_rx, result_tx, ctx) = (job_rx.clone(), result_tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                // Wikimedia turns away requests that don't say who's asking.
                let agent: ureq::Agent = ureq::Agent::config_builder()
                    .timeout_global(Some(Duration::from_secs(30)))
                    .user_agent(flex_core::musicbrainz::USER_AGENT)
                    .build()
                    .into();
                loop {
                    let job = match job_rx.lock().unwrap_or_else(|e| e.into_inner()).recv() {
                        Ok(job) => job,
                        Err(_) => return, // app closing
                    };
                    let image = load(&job, &agent).map_err(|e| log::debug!("image {}: {e:#}", job.url)).ok();
                    if result_tx.send((job.key, image)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        }
        Self { ctx: ctx.clone(), dir, jobs, results, entries: HashMap::new(), frame: 0 }
    }

    /// The image as a texture if it's ready; otherwise starts loading it and returns None.
    /// `path` is a TMDB image path ("/abc.jpg"), a full URL, or "file:" and a local path.
    pub fn get(&mut self, path: &str, size: Size) -> Option<TextureHandle> {
        let (tmdb_size, max_side) = size.spec();
        let key = format!("{tmdb_size}{path}");
        match self.entries.get_mut(&key) {
            Some(Entry::Ready { texture, last_used }) => {
                *last_used = self.frame;
                return Some(texture.clone());
            }
            Some(Entry::Loading | Entry::Failed) => return None,
            None => {}
        }
        let (url, file) = if let Some(local) = path.strip_prefix("file:") {
            (String::new(), PathBuf::from(local))
        } else if path.starts_with("http") {
            let ext = path.rsplit('.').next().filter(|e| e.len() <= 4).unwrap_or("img");
            (path.to_owned(), self.dir.join("web").join(format!("{}.{ext}", hashed(path))))
        } else {
            let name = path.trim_start_matches('/');
            (format!("{TMDB_IMAGES}/{tmdb_size}/{name}"), self.dir.join(tmdb_size).join(name))
        };
        self.entries.insert(key.clone(), Entry::Loading);
        let _ = self.jobs.send(Job { key, url, file, max_side });
        None
    }

    /// Turns finished loads into textures and drops textures that haven't been drawn for a
    /// while. Call once per frame.
    pub fn poll(&mut self) {
        self.frame += 1;
        while let Ok((key, image)) = self.results.try_recv() {
            let entry = match image {
                Some(image) => Entry::Ready {
                    texture: self.ctx.load_texture(&key, image, TextureOptions::LINEAR),
                    last_used: self.frame,
                },
                None => Entry::Failed,
            };
            self.entries.insert(key, entry);
        }
        let ready = self.entries.values().filter(|e| matches!(e, Entry::Ready { .. })).count();
        if ready > MAX_TEXTURES {
            let mut by_age: Vec<(u64, String)> = self
                .entries
                .iter()
                .filter_map(|(k, e)| match e {
                    Entry::Ready { last_used, .. } => Some((*last_used, k.clone())),
                    _ => None,
                })
                .collect();
            by_age.sort();
            for (_, key) in by_age.into_iter().take(ready - MAX_TEXTURES) {
                self.entries.remove(&key); // reloads from disk if needed again
            }
        }
    }
}

/// Draws `texture` scaled to fill `rect` (cropping the overflow), with rounded corners.
pub fn paint_cover(painter: &egui::Painter, rect: egui::Rect, texture: &TextureHandle) {
    let [w, h] = texture.size();
    let (img_aspect, rect_aspect) = (w as f32 / h as f32, rect.width() / rect.height());
    // UV sub-rectangle that matches the rect's aspect ratio.
    let uv = if img_aspect > rect_aspect {
        let visible = rect_aspect / img_aspect;
        egui::Rect::from_min_max(egui::pos2((1.0 - visible) / 2.0, 0.0), egui::pos2((1.0 + visible) / 2.0, 1.0))
    } else {
        let visible = img_aspect / rect_aspect;
        egui::Rect::from_min_max(egui::pos2(0.0, (1.0 - visible) / 2.0), egui::pos2(1.0, (1.0 + visible) / 2.0))
    };
    let shape = egui::epaint::RectShape::filled(rect, egui::CornerRadius::same(8), egui::Color32::WHITE)
        .with_texture(texture.id(), uv);
    painter.add(shape);
}
