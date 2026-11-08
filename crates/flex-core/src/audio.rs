//! Sound for video effects. libmpv can't hand over the samples it plays, so `Tap` has
//! ffmpeg decode the same file's audio a second time, a little ahead of playback, and
//! `Analyser` turns the part at the playback position into levels, a beat and a
//! spectrum. Going by the playback position keeps it in step through pauses and seeks.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use realfft::{RealFftPlanner, RealToComplex};

/// Samples per second decoded (mono): enough for everything up to 11 kHz.
pub const RATE: u32 = 22_050;
/// Frequency bands in the spectrum, log-spaced from `LOW_HZ` to `HIGH_HZ`.
pub const BANDS: usize = 64;
/// Rows of spectrum history (row 0 is now), `ROWS_PER_SEC` apart in video time.
pub const HISTORY: usize = 128;
pub const ROWS_PER_SEC: f64 = 60.0;
const LOW_HZ: f32 = 30.0;
const HIGH_HZ: f32 = 11_000.0;
/// Samples analysed per frame (~93 ms).
pub const WINDOW: usize = 2048;
/// How far ahead of playback ffmpeg decodes, and how much is kept behind.
const AHEAD: f64 = 8.0;
const BEHIND: f64 = 2.0;
/// A position this far past what's decoded is a seek: start ffmpeg again there.
const SEEK_GAP: f64 = 3.0;
/// Quietest sound shown, below the loudest recently.
const RANGE_DB: f32 = 50.0;
/// How quickly the loudness reference falls after something loud.
const REF_FALL_DB_PER_SEC: f32 = 2.5;

#[derive(Default)]
struct Buffer {
    /// Time of `samples[0]`, in seconds of video.
    start: f64,
    samples: VecDeque<f32>,
    /// Where playback is (for how far ahead to decode, and what to drop).
    position: f64,
    /// ffmpeg finished (or failed).
    done: bool,
    /// The tap moved on: the reader stops.
    stopped: bool,
}

impl Buffer {
    fn end(&self) -> f64 {
        self.start + self.samples.len() as f64 / RATE as f64
    }
}

type Shared = Arc<(Mutex<Buffer>, Condvar)>;

fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, Buffer> {
    shared.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// ffmpeg decoding one file's audio near the playback position.
pub struct Tap {
    ffmpeg: PathBuf,
    path: PathBuf,
    /// ffmpeg's index of the audio stream (mpv's `ff-index`), else the first audio stream.
    stream: Option<i64>,
    current: Option<(Shared, Child)>,
}

impl Tap {
    pub fn new(ffmpeg: &Path, path: &Path, stream: Option<i64>) -> Self {
        Self { ffmpeg: ffmpeg.to_owned(), path: path.to_owned(), stream, current: None }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn stream(&self) -> Option<i64> {
        self.stream
    }

    /// Fills `out` with the samples centred on `at` (seconds of video). False while
    /// they haven't been decoded yet (just started, or after a seek), or if there's no
    /// audio.
    pub fn window(&mut self, at: f64, out: &mut [f32]) -> bool {
        let half = out.len() as f64 / 2.0 / RATE as f64;
        let restart = match &self.current {
            None => true,
            Some((shared, _)) => {
                let mut buf = lock(shared);
                buf.position = at;
                shared.1.notify_all();
                at + half < buf.start || at > buf.end() + SEEK_GAP || (buf.done && at > buf.end() + half)
            }
        };
        if restart {
            if self.current.as_ref().is_some_and(|(s, _)| lock(s).done && lock(s).samples.is_empty()) {
                return false; // no audio at all: don't keep trying
            }
            self.start(at);
        }
        let Some((shared, _)) = &self.current else { return false };
        let buf = lock(shared);
        let first = ((at - half - buf.start) * RATE as f64).round();
        if first < 0.0 || first as usize + out.len() > buf.samples.len() {
            return false;
        }
        for (o, s) in out.iter_mut().zip(buf.samples.range(first as usize..)) {
            *o = *s;
        }
        true
    }

    fn stop(&mut self) {
        if let Some((shared, mut child)) = self.current.take() {
            lock(&shared).stopped = true;
            shared.1.notify_all();
            let _ = child.kill();
            std::thread::spawn(move || child.wait());
        }
    }

    fn start(&mut self, at: f64) {
        self.stop();
        let from = (at - 1.0).max(0.0);
        let map = match self.stream {
            Some(i) => format!("0:{i}"),
            None => "0:a:0?".to_owned(),
        };
        let mut cmd = Command::new(&self.ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &format!("{from:.3}"), "-i"])
            .arg(&self.path)
            .args(["-map", &map, "-vn", "-sn", "-dn", "-ac", "1", "-ar", &RATE.to_string(), "-f", "f32le", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let shared: Shared = Arc::new((Mutex::new(Buffer { start: from, position: at, ..Default::default() }), Condvar::new()));
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                log::warn!("audio for effects: running {}: {e}", self.ffmpeg.display());
                lock(&shared).done = true;
                return;
            }
        };
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else { return };
        let reader = Arc::clone(&shared);
        let _ = std::thread::Builder::new().name("effect-audio".into()).spawn(move || read(stdout, &reader));
        let _ = std::thread::Builder::new().name("effect-audio-err".into()).spawn(move || {
            let (mut stderr, mut text) = (stderr, String::new());
            let _ = stderr.read_to_string(&mut text);
            let text = text.trim();
            if !text.is_empty() {
                log::info!("audio for effects: ffmpeg: {text}");
            }
        });
        log::debug!("audio for effects: decoding from {from:.1}s");
        self.current = Some((shared, child));
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Reads ffmpeg's samples into the buffer, staying `AHEAD` of playback.
fn read(mut stdout: impl Read, shared: &Shared) {
    let mut bytes = vec![0u8; 16 * 1024];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        {
            let mut buf = lock(shared);
            while !buf.stopped && buf.end() > buf.position + AHEAD {
                buf = shared.1.wait_timeout(buf, Duration::from_millis(200)).unwrap_or_else(|e| e.into_inner()).0;
            }
            if buf.stopped {
                return;
            }
        }
        let n = match stdout.read(&mut bytes) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        carry.extend_from_slice(&bytes[..n]);
        let whole = carry.len() / 4 * 4;
        let mut buf = lock(shared);
        buf.samples.extend(carry[..whole].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)));
        carry.drain(..whole);
        // Drop what's well behind playback.
        let old = ((buf.position - BEHIND - buf.start) * RATE as f64).floor();
        if old > 0.0 {
            let n = (old as usize).min(buf.samples.len());
            buf.samples.drain(..n);
            buf.start += n as f64 / RATE as f64;
        }
    }
    lock(shared).done = true;
}

/// What effects get each frame. Levels run 0–1 against the loudest sound recently, so
/// quiet scenes still move things.
#[derive(Clone, Debug, PartialEq)]
pub struct Features {
    pub level: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    /// 1 on a beat (a jump in the bass), fading quickly.
    pub beat: f32,
    /// `HISTORY` rows of `BANDS` values, row 0 the newest.
    pub spectrum: Vec<f32>,
}

impl Default for Features {
    fn default() -> Self {
        Self { level: 0.0, bass: 0.0, mid: 0.0, treble: 0.0, beat: 0.0, spectrum: vec![0.0; BANDS * HISTORY] }
    }
}

pub struct Analyser {
    fft: Arc<dyn RealToComplex<f32>>,
    hann: Vec<f32>,
    /// FFT bins per band, and each band's middle frequency.
    bands: Vec<(usize, usize, f32)>,
    /// Loudness references (power) for the bands and for the overall level.
    band_ref: f32,
    level_ref: f32,
    smooth: [f32; BANDS],
    /// Average bass power over the last second or so, and time since the last beat.
    bass_avg: f32,
    since_beat: f32,
    history: VecDeque<[f32; BANDS]>,
    last_row: Option<i64>,
    features: Features,
    scratch: Vec<f32>,
    spectrum: Vec<realfft::num_complex::Complex<f32>>,
}

impl Default for Analyser {
    fn default() -> Self {
        Self::new()
    }
}

/// Moves towards `target`: quickly when rising, more slowly when falling.
fn ease(current: f32, target: f32, dt: f32, rise: f32, fall: f32) -> f32 {
    let rate = if target > current { rise } else { fall };
    current + (target - current) * (1.0 - (-dt * rate).exp())
}

impl Analyser {
    pub fn new() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(WINDOW);
        let hann = (0..WINDOW).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / WINDOW as f32).cos()).collect();
        let bin_hz = RATE as f32 / WINDOW as f32;
        let bands = (0..BANDS)
            .map(|b| {
                let f = |b: f32| LOW_HZ * (HIGH_HZ / LOW_HZ).powf(b / BANDS as f32);
                let (lo, hi) = (f(b as f32), f(b as f32 + 1.0));
                let first = (lo / bin_hz).round() as usize;
                let last = ((hi / bin_hz).round() as usize).max(first + 1).min(WINDOW / 2);
                (first.min(last - 1), last, (lo * hi).sqrt())
            })
            .collect();
        let spectrum = fft.make_output_vec();
        Self {
            fft,
            hann,
            bands,
            band_ref: 1e-9,
            level_ref: 1e-9,
            smooth: [0.0; BANDS],
            bass_avg: 0.0,
            since_beat: 10.0,
            history: VecDeque::new(),
            last_row: None,
            features: Features::default(),
            scratch: vec![0.0; WINDOW],
            spectrum,
        }
    }

    /// Analyses `window` (`WINDOW` samples centred on the playback position, `None` when
    /// there's no sound to give) at `video_time`; `dt` is the time since the last call.
    pub fn update(&mut self, window: Option<&[f32]>, video_time: f64, dt: f32) -> &Features {
        let dt = dt.clamp(0.0, 0.25);
        let fall = 10f32.powf(-REF_FALL_DB_PER_SEC * dt / 10.0);
        let to_unit = |power: f32, reference: f32| (1.0 + 10.0 * (power / reference).max(1e-12).log10() / RANGE_DB).clamp(0.0, 1.0);
        let mut bass_power = 0.0;
        let mut bands = [0.0f32; BANDS];
        if let Some(window) = window.filter(|w| w.len() == WINDOW) {
            for ((s, w), h) in self.scratch.iter_mut().zip(window).zip(&self.hann) {
                *s = w * h;
            }
            let _ = self.fft.process(&mut self.scratch, &mut self.spectrum);
            let norm = 1.0 / (WINDOW as f32 * WINDOW as f32);
            let mut loudest = 0.0f32;
            for (b, &(first, last, _)) in self.bands.iter().enumerate() {
                let p: f32 = self.spectrum[first..last].iter().map(|c| c.norm_sqr() * norm).sum();
                bands[b] = p;
                loudest = loudest.max(p);
            }
            self.band_ref = (self.band_ref * fall).max(loudest).max(1e-9);
            let rms = window.iter().map(|s| s * s).sum::<f32>() / WINDOW as f32;
            self.level_ref = (self.level_ref * fall).max(rms).max(1e-9);
            self.features.level = ease(self.features.level, to_unit(rms, self.level_ref), dt, 30.0, 8.0);
            // Each range's total power, against the same reference as the bands.
            let range = |lo: f32, hi: f32| -> f32 {
                self.bands.iter().zip(&bands).filter(|((_, _, hz), _)| (lo..hi).contains(hz)).map(|(_, p)| p).sum()
            };
            bass_power = range(0.0, 150.0);
            let (mid, treble) = (range(150.0, 2000.0), range(2000.0, f32::MAX));
            self.features.bass = ease(self.features.bass, to_unit(bass_power, self.band_ref), dt, 30.0, 6.0);
            self.features.mid = ease(self.features.mid, to_unit(mid, self.band_ref), dt, 30.0, 6.0);
            self.features.treble = ease(self.features.treble, to_unit(treble, self.band_ref), dt, 30.0, 6.0);
            for (b, p) in bands.iter_mut().enumerate() {
                *p = to_unit(*p, self.band_ref);
                self.smooth[b] = ease(self.smooth[b], *p, dt, 30.0, 6.0);
            }
        } else {
            for s in &mut self.smooth {
                *s = ease(*s, 0.0, dt, 30.0, 6.0);
            }
            let f = &mut self.features;
            for v in [&mut f.level, &mut f.bass, &mut f.mid, &mut f.treble] {
                *v = ease(*v, 0.0, dt, 30.0, 8.0);
            }
        }

        // A beat: the bass jumping well above its recent average.
        self.since_beat += dt;
        let jump = bass_power > self.bass_avg * 1.6 && self.features.bass > 0.4 && self.since_beat > 0.25;
        self.bass_avg = ease(self.bass_avg, bass_power, dt, 1.5, 1.5);
        if jump {
            self.features.beat = 1.0;
            self.since_beat = 0.0;
        } else {
            self.features.beat *= (-dt * 6.0).exp();
        }

        // History, by video time: it stands still while paused and starts again on a seek.
        let row = (video_time * ROWS_PER_SEC).floor() as i64;
        let new = match self.last_row {
            Some(last) if row >= last && row - last <= HISTORY as i64 => (row - last) as usize,
            _ => {
                self.history.clear();
                HISTORY
            }
        };
        self.last_row = Some(row);
        for _ in 0..new {
            self.history.push_front(self.smooth);
        }
        self.history.truncate(HISTORY);
        if let Some(front) = self.history.front_mut() {
            *front = self.smooth;
        }
        for (r, row) in self.history.iter().enumerate() {
            self.features.spectrum[r * BANDS..(r + 1) * BANDS].copy_from_slice(row);
        }
        &self.features
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, amp: f32, from: usize) -> Vec<f32> {
        (from..from + WINDOW).map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin()).collect()
    }

    #[test]
    fn tones_land_in_their_band() {
        let mut a = Analyser::new();
        let mut f = Features::default();
        for i in 0..60 {
            f = a.update(Some(&tone(60.0, 0.5, i * 368)), i as f64 / 60.0, 1.0 / 60.0).clone();
        }
        assert!(f.bass > 0.8 && f.treble < 0.3, "bass {} treble {}", f.bass, f.treble);
        let loudest = (0..BANDS).max_by(|x, y| f.spectrum[*x].total_cmp(&f.spectrum[*y])).unwrap();
        let hz = a.bands[loudest].2;
        assert!((40.0..90.0).contains(&hz), "60 Hz showed up at {hz} Hz");

        for i in 60..120 {
            f = a.update(Some(&tone(5000.0, 0.5, i * 368)), i as f64 / 60.0, 1.0 / 60.0).clone();
        }
        assert!(f.treble > 0.7 && f.bass < 0.3, "bass {} treble {}", f.bass, f.treble);
    }

    #[test]
    fn quiet_scenes_still_register_after_a_while() {
        let mut a = Analyser::new();
        let mut f = Features::default();
        for i in 0..30 {
            f = a.update(Some(&tone(440.0, 0.8, i * 368)), i as f64 / 60.0, 1.0 / 60.0).clone();
        }
        let loud = f.level;
        // 30 dB quieter: dim at first, then the reference comes down to meet it.
        f = a.update(Some(&tone(440.0, 0.025, 0)), 0.5, 1.0 / 60.0).clone();
        let first = f.level;
        for i in 0..600 {
            f = a.update(Some(&tone(440.0, 0.025, i * 368)), 0.5 + i as f64 / 60.0, 1.0 / 60.0).clone();
        }
        assert!(loud > 0.9 && first < loud && f.level > 0.85, "loud {loud} first {first} later {}", f.level);
    }

    #[test]
    fn beats_come_from_jumps_in_the_bass() {
        let mut a = Analyser::new();
        let mut beats = 0;
        for i in 0..240 {
            // A kick every half second: 80 ms of 60 Hz, then quiet hum.
            let kick = i % 30 < 5;
            let w = if kick { tone(60.0, 0.8, i * 368) } else { tone(60.0, 0.02, i * 368) };
            let f = a.update(Some(&w), i as f64 / 60.0, 1.0 / 60.0);
            if f.beat == 1.0 {
                beats += 1;
            }
        }
        assert!((6..=8).contains(&beats), "{beats} beats in 4 seconds of kicks every 0.5s");
    }

    #[test]
    fn history_follows_video_time() {
        let mut a = Analyser::new();
        let w = tone(1000.0, 0.5, 0);
        for i in 0..10 {
            a.update(Some(&w), 10.0 + i as f64 / ROWS_PER_SEC, 1.0 / 60.0);
        }
        let rows = |a: &Analyser| a.history.len();
        assert_eq!(rows(&a), HISTORY);
        // Paused: the same row over and over.
        let before = a.features.spectrum.clone();
        for _ in 0..30 {
            a.update(Some(&w), 10.0 + 9.0 / ROWS_PER_SEC, 1.0 / 60.0);
        }
        assert_eq!(a.features.spectrum[BANDS..], before[BANDS..], "older rows stay put while paused");
        // Seeking back starts afresh.
        a.update(None, 2.0, 1.0 / 60.0);
        assert_eq!(a.last_row, Some(120));
    }

    /// Decodes a real file through ffmpeg (needs ffmpeg; run with --ignored).
    #[test]
    #[ignore]
    fn taps_a_file() {
        let ffmpeg = crate::find_tool("ffmpeg").expect("ffmpeg");
        let dir = std::env::temp_dir().join(format!("flex-audio-tap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tone.mkv");
        let status = Command::new(&ffmpeg)
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=200:duration=20", "-c:a", "flac"])
            .arg(&file)
            .status()
            .unwrap();
        assert!(status.success());
        let mut tap = Tap::new(&ffmpeg, &file, None);
        let mut out = vec![0.0; WINDOW];
        let wait = |tap: &mut Tap, at: f64, out: &mut [f32]| {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !tap.window(at, out) {
                assert!(std::time::Instant::now() < deadline, "no samples at {at}");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        wait(&mut tap, 2.0, &mut out);
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / WINDOW as f32).sqrt();
        assert!(rms > 0.05, "rms {rms}");
        // The phase of a 200 Hz sine at t: check the samples line up with the time asked for.
        let at = 15.3;
        wait(&mut tap, at, &mut out);
        let start = at - WINDOW as f64 / 2.0 / RATE as f64;
        let expected: Vec<f32> = (0..WINDOW).map(|i| (std::f64::consts::TAU * 200.0 * (start + i as f64 / RATE as f64)).sin() as f32).collect();
        let corr: f32 = out.iter().zip(&expected).map(|(a, b)| a * b).sum::<f32>() / WINDOW as f32;
        assert!(corr > 0.05, "samples don't line up with the position (correlation {corr})");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
