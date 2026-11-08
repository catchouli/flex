//! Depth for video effects that ask for it (`depth = true`): Depth Anything V2 Small on the
//! GPU through flex-depth, in a thread of its own. The player's paint callback hands it small
//! frames (see `effects::Renderer`) whenever it's free, and picks up depth maps as they come.
//!
//! The model (99 MB) is downloaded into the data folder the first time it's needed. The
//! thread stops, freeing the GPU memory, once nothing has asked for depth for a while.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use flex_depth::Depth;

/// The model's picture, in patches of 14 pixels: 518 × 294, about 16:9.
const GRID: (usize, usize) = (37, 21);
pub const WIDTH: usize = GRID.0 * flex_depth::PATCH;
pub const HEIGHT: usize = GRID.1 * flex_depth::PATCH;
/// Nothing wanted depth for this long: stop.
const IDLE: Duration = Duration::from_secs(20);
/// How much of each new depth map goes into what effects see (the rest is the last one),
/// and how quickly the range used to scale depth to 0–1 follows the picture.
const BLEND: f32 = 0.6;
const RANGE_FOLLOW: f32 = 0.2;
/// At most this often, to leave the GPU time for the video (~20 depth maps a second).
const MIN_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Off,
    Starting,
    /// Downloading the model: percent done.
    Downloading(u8),
    Ready {
        adapter: String,
        /// How long the last depth map took.
        ms: f32,
    },
    Failed(String),
}

impl Status {
    pub fn describe(&self) -> String {
        match self {
            Status::Off => "off".into(),
            Status::Starting => "starting…".into(),
            Status::Downloading(p) => format!("downloading the depth model ({p}%)…"),
            Status::Ready { adapter, ms } => format!("{ms:.0} ms a frame on {adapter}"),
            Status::Failed(e) => format!("⚠ {e}"),
        }
    }
}

/// A depth map for effects: `values` from 0 (far) to 1 (near), bottom row first (as GL
/// textures go), `WIDTH` × `HEIGHT`.
pub struct DepthMap {
    pub values: Vec<f32>,
    pub serial: u64,
}

struct State {
    status: Status,
    running: bool,
    /// A frame waiting: RGBA, bottom row first (as read back from GL), `WIDTH` × `HEIGHT`.
    input: Option<Vec<u8>>,
    busy: bool,
    output: Option<Arc<DepthMap>>,
    last_wanted: Instant,
}

/// Shared between the UI, the paint callback and the worker.
#[derive(Clone)]
pub struct Link(Arc<(Mutex<State>, Condvar)>);

impl Default for Link {
    fn default() -> Self {
        let state = State { status: Status::Off, running: false, input: None, busy: false, output: None, last_wanted: Instant::now() };
        Self(Arc::new((Mutex::new(state), Condvar::new())))
    }
}

fn model_path() -> PathBuf {
    flex_core::data_dir().join("models").join("depth-anything-v2-small.safetensors")
}

impl Link {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// An effect wants depth: starts the worker if it isn't running. Call every frame.
    pub fn wanted(&self) {
        let mut state = self.lock();
        state.last_wanted = Instant::now();
        if state.running || matches!(state.status, Status::Failed(_)) {
            return;
        }
        state.running = true;
        state.status = Status::Starting;
        drop(state);
        let link = self.clone();
        if let Err(e) = std::thread::Builder::new().name("depth".into()).spawn(move || link.work()) {
            let mut state = self.lock();
            state.running = false;
            state.status = Status::Failed(format!("couldn't start: {e}"));
        }
    }

    pub fn status(&self) -> Status {
        self.lock().status.clone()
    }

    /// Whether the worker would take a frame now.
    pub fn ready_for_frame(&self) -> bool {
        let state = self.lock();
        matches!(state.status, Status::Ready { .. }) && state.input.is_none() && !state.busy
    }

    /// Hands over a frame (RGBA, bottom row first, `WIDTH` × `HEIGHT`).
    pub fn submit(&self, rgba: Vec<u8>) {
        self.lock().input = Some(rgba);
        self.0.1.notify_all();
    }

    pub fn latest(&self) -> Option<Arc<DepthMap>> {
        self.lock().output.clone()
    }

    fn work(&self) {
        let path = model_path();
        let result = (|| -> anyhow::Result<Depth> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let total = flex_depth::WEIGHTS_BYTES;
            let progress = |done: u64| {
                let pct = (done * 100 / total.max(1)).min(100) as u8;
                let mut state = self.lock();
                if state.status != Status::Downloading(pct) && done < total {
                    state.status = Status::Downloading(pct);
                }
            };
            flex_core::update::download_verified(flex_depth::WEIGHTS_URL, "the depth model", total, flex_depth::WEIGHTS_SHA256, &path, progress)?;
            Depth::load(&path, GRID)
        })();
        let mut depth = match result {
            Ok(depth) => depth,
            Err(e) => {
                log::warn!("depth: {e:#}");
                let mut state = self.lock();
                state.status = Status::Failed(format!("{e:#}"));
                state.running = false;
                return;
            }
        };
        log::info!("depth: running on {}", depth.adapter());
        self.lock().status = Status::Ready { adapter: depth.adapter().to_owned(), ms: 0.0 };

        let mut previous: Option<Vec<f32>> = None;
        let mut range: Option<(f32, f32)> = None;
        let mut serial = 0;
        let mut rgb = vec![0u8; WIDTH * HEIGHT * 3];
        loop {
            let input = {
                let mut state = self.lock();
                loop {
                    if let Some(input) = state.input.take() {
                        state.busy = true;
                        break Some(input);
                    }
                    if state.last_wanted.elapsed() > IDLE {
                        break None;
                    }
                    state = self.0.1.wait_timeout(state, Duration::from_millis(500)).unwrap_or_else(|e| e.into_inner()).0;
                }
            };
            let Some(rgba) = input else { break };
            // GL's rows run bottom-up; the model's top-down.
            for y in 0..HEIGHT {
                let src = &rgba[(HEIGHT - 1 - y) * WIDTH * 4..][..WIDTH * 4];
                for (x, px) in src.as_chunks::<4>().0.iter().enumerate() {
                    rgb[(y * WIDTH + x) * 3..][..3].copy_from_slice(&px[..3]);
                }
            }
            let started = Instant::now();
            let result = depth.run(&rgb);
            let ms = started.elapsed().as_secs_f32() * 1000.0;
            // Still "busy" while waiting, so no frame is read back in the meantime.
            std::thread::sleep(MIN_INTERVAL.saturating_sub(started.elapsed()));
            let raw = match result {
                Ok(raw) => raw,
                Err(e) => {
                    log::warn!("depth: {e:#}");
                    let mut state = self.lock();
                    state.status = Status::Failed(format!("{e:#}"));
                    state.busy = false;
                    break;
                }
            };
            // Scaled to 0–1 by a range that follows the picture's smoothly (so it doesn't
            // pump), blended with the last map, and flipped back to bottom-up.
            let (lo, hi) = raw.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
            let (lo, hi) = match range {
                Some((l, h)) => (l + (lo - l) * RANGE_FOLLOW, h + (hi - h) * RANGE_FOLLOW),
                None => (lo, hi),
            };
            range = Some((lo, hi));
            let scale = 1.0 / (hi - lo).max(1e-6);
            let mut values = vec![0.0; WIDTH * HEIGHT];
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    values[(HEIGHT - 1 - y) * WIDTH + x] = ((raw[y * WIDTH + x] - lo) * scale).clamp(0.0, 1.0);
                }
            }
            if let Some(prev) = &previous {
                for (v, p) in values.iter_mut().zip(prev) {
                    *v = p + (*v - p) * BLEND;
                }
            }
            serial += 1;
            previous = Some(values.clone());
            let mut state = self.lock();
            state.output = Some(Arc::new(DepthMap { values, serial }));
            state.busy = false;
            state.status = Status::Ready { adapter: depth.adapter().to_owned(), ms };
        }
        drop(depth);
        log::info!("depth: stopped");
        let mut state = self.lock();
        state.running = false;
        state.output = None;
        if matches!(state.status, Status::Ready { .. }) {
            state.status = Status::Off;
        }
    }
}
