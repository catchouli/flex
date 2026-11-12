//! The sound the computer is playing, recorded back as it reaches the speakers, for video
//! effects. It's everything that plays, not only the video, so effects follow music from
//! anywhere even with the video muted. Through cpal: loopback on the default output with WASAPI
//! (Windows) and Core Audio (macOS 14.6 and later), and the default output's monitor through
//! PulseAudio (Linux, including PipeWire's PulseAudio server). The output is followed if the
//! default changes. Without any of these, effects fall back to mpv's own sound.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// More than this waiting (chunks) means nobody's collecting them.
const MOST_WAITING: usize = 512;
/// How often the default output is looked at, to follow it when it changes.
const CHECK_EVERY: Duration = Duration::from_millis(1000);
/// Giving up after this many failed tries in a row, for effects to use mpv's sound instead.
const MOST_FAILURES: u32 = 3;
/// The rate asked for where the server converts (PulseAudio).
const RATE: u32 = 48_000;

/// Some sound, as one channel.
pub struct Chunk {
    pub rate: u32,
    pub mono: Vec<f32>,
}

const STARTING: u8 = 0;
const RECORDING: u8 = 1;
const FAILED: u8 = 2;

struct Shared {
    waiting: Mutex<VecDeque<Chunk>>,
    state: AtomicU8,
    stop: AtomicBool,
}

/// Recording, on a thread of its own, until dropped.
pub struct SystemSound {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl SystemSound {
    pub fn start() -> Self {
        let shared = Arc::new(Shared { waiting: Mutex::new(VecDeque::new()), state: AtomicU8::new(STARTING), stop: AtomicBool::new(false) });
        let thread = std::thread::Builder::new().name("system sound".into()).spawn({
            let shared = shared.clone();
            move || record(&shared)
        });
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(e) => {
                log::warn!("video effects: can't record the computer's sound: {e}");
                shared.state.store(FAILED, Ordering::Relaxed);
                None
            }
        };
        Self { shared, thread }
    }

    /// No recording is possible here, so the sound has to come from somewhere else.
    pub fn failed(&self) -> bool {
        self.shared.state.load(Ordering::Relaxed) == FAILED
    }

    /// Recording (rather than still starting, or failed).
    pub fn recording(&self) -> bool {
        self.shared.state.load(Ordering::Relaxed) == RECORDING
    }

    /// Everything recorded since the last call, oldest first.
    pub fn take(&self) -> Vec<Chunk> {
        self.shared.waiting.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect()
    }
}

impl Drop for SystemSound {
    fn drop(&mut self) {
        // Not waited for: this is dropped on the UI thread, and the recording thread may be in
        // the middle of opening the output, which can take seconds. It stops by itself.
        self.shared.stop.store(true, Ordering::Relaxed);
        drop(self.thread.take());
    }
}

/// The device to record the default output from, with its name.
fn source(host: &cpal::Host) -> Result<(cpal::Device, String), String> {
    let output = host.default_output_device().ok_or("no default output")?;
    let id = output.id().map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    {
        // PulseAudio records an output through its monitor source.
        if id.host() != cpal::HostId::PulseAudio {
            return Err("no PulseAudio (or PipeWire's PulseAudio) server".into());
        }
        let monitor = cpal::DeviceId::new(cpal::HostId::PulseAudio, format!("{}.monitor", id.id()));
        let device = host.device_by_id(&monitor).ok_or_else(|| format!("no monitor for {}", id.id()))?;
        Ok((device, monitor.id().to_owned()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Recording from an output device is loopback.
        Ok((output, id.id().to_owned()))
    }
}

/// The default output's name, cheaply, to see whether it changed.
fn default_output(host: &cpal::Host) -> Option<String> {
    let output = host.default_output_device()?;
    output.id().ok().map(|id| id.id().to_owned())
}

fn open(shared: &Arc<Shared>, device: &cpal::Device, broken: &Arc<AtomicBool>) -> Result<cpal::Stream, String> {
    let config = if cfg!(target_os = "linux") {
        // The server converts, so ask for what's wanted. Its own choice of buffer is so large that
        // sound comes in seconds-long lumps: 10 ms keeps up with the picture.
        cpal::StreamConfig { channels: 1, sample_rate: RATE, buffer_size: cpal::BufferSize::Fixed(RATE / 100) }
    } else {
        // Loopback records in the output's own format, without converting.
        let default = device.default_output_config().map_err(|e| e.to_string())?;
        if default.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!("the output's format is {}, not floats", default.sample_format()));
        }
        default.config()
    };
    let channels = usize::from(config.channels.max(1));
    let rate = config.sample_rate;
    let stream = device
        .build_input_stream::<f32, _, _>(
            config,
            {
                let shared = shared.clone();
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mono = data.chunks_exact(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect();
                    let mut waiting = shared.waiting.lock().unwrap_or_else(|e| e.into_inner());
                    if waiting.len() >= MOST_WAITING {
                        waiting.pop_front();
                    }
                    waiting.push_back(Chunk { rate, mono });
                }
            },
            {
                let broken = broken.clone();
                move |e| {
                    log::warn!("video effects: recording the computer's sound: {e}");
                    broken.store(true, Ordering::Relaxed);
                }
            },
            Some(Duration::from_secs(3)),
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

/// The recording thread: opens the default output, and opens it again when that changes or the
/// recording breaks.
fn record(shared: &Arc<Shared>) {
    // One host throughout: each new one is a new connection to the server (PulseAudio).
    let host = cpal::default_host();
    let mut current: Option<(cpal::Stream, String)> = None;
    let mut failures = 0;
    let mut broken = Arc::new(AtomicBool::new(false));
    let mut checked: Option<Instant> = None;
    while !shared.stop.load(Ordering::Relaxed) {
        if checked.is_none_or(|t| t.elapsed() >= CHECK_EVERY) {
            checked = Some(Instant::now());
            let output = default_output(&host);
            let stale = match &current {
                Some((_, name)) => broken.load(Ordering::Relaxed) || output.is_none_or(|o| !name.starts_with(&o)),
                None => true,
            };
            if stale {
                current = None;
                broken = Arc::new(AtomicBool::new(false));
                match source(&host).and_then(|(device, name)| open(shared, &device, &broken).map(|stream| (stream, name))) {
                    Ok((stream, name)) => {
                        log::info!("video effects: recording the computer's sound from {name}");
                        current = Some((stream, name));
                        failures = 0;
                        shared.state.store(RECORDING, Ordering::Relaxed);
                    }
                    Err(e) => {
                        failures += 1;
                        log::warn!("video effects: can't record the computer's sound: {e}");
                        if failures >= MOST_FAILURES {
                            shared.state.store(FAILED, Ordering::Relaxed);
                            return;
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records the real default output for a few seconds (`cargo test -- --ignored`). Starting
    /// can take a couple of seconds while the server wakes the output.
    #[test]
    #[ignore = "needs a sound server"]
    fn records_the_default_output() {
        let sound = SystemSound::start();
        std::thread::sleep(Duration::from_secs(4));
        assert!(sound.recording(), "not recording");
        let _ = sound.take();
        std::thread::sleep(Duration::from_secs(1));
        let chunks = sound.take();
        let samples: usize = chunks.iter().map(|c| c.mono.len()).sum();
        let rate = chunks.first().map_or(0, |c| c.rate);
        println!("{} chunks, {samples} samples at {rate} Hz in a second", chunks.len());
        assert!(samples as f32 > rate as f32 * 0.8, "too little sound for a second");
    }
}
