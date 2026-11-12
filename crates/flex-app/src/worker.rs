//! What the networks' workers (depth, faces, movement, the camera and restyling) have in common.
//! Each runs in a thread of its own, started when an effect first wants it and stopped, freeing
//! the GPU memory, once nothing has for a while. Its state is shared between the UI, the paint
//! callback and the thread.

use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Nothing wanted a worker for this long: it stops.
const IDLE: Duration = Duration::from_secs(20);
/// How often a waiting worker looks to see whether it's still wanted.
const LOOK: Duration = Duration::from_millis(500);

/// A worker's own state, besides what every worker has.
pub trait Job: Send + 'static {
    type Status: Clone;
    fn status(&self) -> &Self::Status;
    /// Whether it's failed at what's wanted now.
    fn failed(&self) -> bool;
    /// Shows it as starting, or as having failed to.
    fn starting(&mut self);
    fn fail(&mut self, why: String);
    /// Its thread has stopped: lets go of what it made, and shows it as off unless it failed.
    fn stopped(&mut self);
}

/// A worker's state: what every worker has, and its own (`job`, which it derefs to).
pub struct Shared<J> {
    pub running: bool,
    last_wanted: Instant,
    /// When `want()` first saw the network had failed, for trying again (see `gpu::still_failed`).
    failed_seen: Option<Instant>,
    pub job: J,
}

impl<J> Shared<J> {
    /// Whether nothing's wanted the worker for long enough that it should stop.
    pub fn idle(&self) -> bool {
        self.last_wanted.elapsed() > IDLE
    }
}

impl<J> Deref for Shared<J> {
    type Target = J;
    fn deref(&self) -> &J {
        &self.job
    }
}

impl<J> DerefMut for Shared<J> {
    fn deref_mut(&mut self) -> &mut J {
        &mut self.job
    }
}

/// A worker, shared between the UI, the paint callback and its thread.
pub struct Worker<J>(Arc<(Mutex<Shared<J>>, Condvar)>);

impl<J> Clone for Worker<J> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<J: Default> Default for Worker<J> {
    fn default() -> Self {
        Self(Arc::new((Mutex::new(Shared { running: false, last_wanted: Instant::now(), failed_seen: None, job: J::default() }), Condvar::new())))
    }
}

impl<J: Job> Worker<J> {
    pub fn lock(&self) -> MutexGuard<'_, Shared<J>> {
        self.0.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Wakes the thread if it's waiting.
    pub fn notify(&self) {
        self.0.1.notify_all();
    }

    pub fn status(&self) -> J::Status {
        self.lock().status().clone()
    }

    /// An effect wants the worker: `want` notes what it wants, and unless the worker's running,
    /// or failed lately, `work` is started in a thread called `name`. Call every frame.
    pub fn want(&self, name: &str, want: impl FnOnce(&mut J), work: impl FnOnce(Self) + Send + 'static) {
        let mut state = self.lock();
        state.last_wanted = Instant::now();
        want(&mut state.job);
        if state.running {
            return;
        }
        if state.failed() && crate::gpu::still_failed(&mut state.failed_seen) {
            return;
        }
        state.running = true;
        state.starting();
        drop(state);
        let worker = self.clone();
        if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(move || work(worker)) {
            let mut state = self.lock();
            state.running = false;
            state.fail(format!("couldn't start: {e}"));
        }
    }

    /// Waits a while for something to change.
    pub fn wait<'a>(&self, state: MutexGuard<'a, Shared<J>>) -> MutexGuard<'a, Shared<J>> {
        self.0.1.wait_timeout(state, LOOK).unwrap_or_else(|e| e.into_inner()).0
    }

    /// Waits for what `take` finds in the state: None once the worker's idle.
    pub fn next<T>(&self, mut take: impl FnMut(&mut Shared<J>) -> Option<T>) -> Option<T> {
        let mut state = self.lock();
        loop {
            if let Some(found) = take(&mut state) {
                return Some(found);
            }
            if state.idle() {
                return None;
            }
            state = self.wait(state);
        }
    }

    /// The thread's stopping, having been idle or failed.
    pub fn stop(&self) {
        let mut state = self.lock();
        state.running = false;
        state.stopped();
    }
}

/// Adds `items` to `queue`, keeping only the newest `most`.
pub fn push_capped<T>(queue: &mut VecDeque<T>, items: impl IntoIterator<Item = T>, most: usize) {
    queue.extend(items);
    let extra = queue.len().saturating_sub(most);
    queue.drain(..extra);
}

/// A network's weights, `file` in the data folder's models, downloaded first if they aren't
/// there yet (`label` saying what they are), with `progress` told the percent done as it goes.
pub fn fetch_model(file: &str, url: &str, label: &str, bytes: u64, sha256: &str, mut progress: impl FnMut(u8)) -> anyhow::Result<PathBuf> {
    let path = flex_core::data_dir().join("models").join(file);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    flex_core::update::download_verified(url, label, bytes, sha256, &path, |done| {
        if done < bytes {
            progress((done * 100 / bytes.max(1)).min(100) as u8);
        }
    })?;
    Ok(path)
}
