//! The samples mpv is playing, from Flex's own patch to libmpv (`packaging/mpv`). They arrive on
//! an audio thread, and wait here until the next frame collects them.

use std::collections::VecDeque;
use std::ffi::{c_double, c_int, c_void};
use std::sync::Mutex;

/// Some sound, as one channel, and when in the video it starts.
pub struct Chunk {
    pub pts: f64,
    pub rate: u32,
    pub mono: Vec<f32>,
}

/// More than this waiting (chunks) means nobody's collecting them.
const MOST_WAITING: usize = 512;

static WAITING: Mutex<VecDeque<Chunk>> = Mutex::new(VecDeque::new());

/// What mpv calls.
pub unsafe extern "C" fn receive(_ctx: *mut c_void, samples: *const f32, channels: c_int, frames: c_int, rate: c_int, pts: c_double) {
    let (channels, frames) = (channels.max(1) as usize, frames.max(0) as usize);
    if samples.is_null() || frames == 0 {
        return;
    }
    // SAFETY: mpv passes `frames` frames of `channels` floats, valid for this call.
    let samples = unsafe { std::slice::from_raw_parts(samples, frames * channels) };
    let mono = samples.chunks_exact(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect();
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    if waiting.len() >= MOST_WAITING {
        waiting.pop_front();
    }
    waiting.push_back(Chunk { pts, rate: rate.max(1) as u32, mono });
}

/// Everything that's arrived since the last call, oldest first.
pub fn take() -> Vec<Chunk> {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect()
}
