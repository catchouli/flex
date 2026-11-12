//! Sound for video effects. The computer's own sound, recorded back, or the samples Flex's
//! patched libmpv hands over, are measured by `SampleBands` about a hundred times a second.
//! Without either, a filter inside mpv splits the sound into frequency bands and reports how loud
//! each one is, several times a second (`filter_graph`, `read_levels`). `Analyser` turns any of them into
//! levels, a beat and a spectrum. Nothing reads the file a second time, and it works for
//! anything mpv can play.

use std::collections::{HashMap, VecDeque};

/// Bands in the spectrum effects get, log-spaced from `LOW_HZ` to `HIGH_HZ`.
pub const BANDS: usize = 64;
/// Rows of spectrum history (row 0 is now), `ROWS_PER_SEC` apart in video time.
pub const HISTORY: usize = 128;
pub const ROWS_PER_SEC: f64 = 60.0;
const LOW_HZ: f32 = 30.0;
const HIGH_HZ: f32 = 11_000.0;
/// Bands mpv's filter measures; `BANDS` are stretched from these. amerge takes 64 inputs at
/// most, and more bands cost more of the audio thread.
pub const FILTER_BANDS: usize = 32;
/// The filter's label in mpv: `af-metadata/<label>` has what it measures.
pub const FILTER_LABEL: &str = "flex-audio";
/// How narrow each band's filter is. Neighbours overlap a little, so nothing falls between them.
const FILTER_Q: f32 = 2.63;
/// Quietest sound shown, below the loudest recently.
const RANGE_DB: f32 = 50.0;
/// How quickly the loudness reference falls after something loud.
const REF_FALL_DB_PER_SEC: f32 = 2.5;

fn filter_hz(band: usize) -> f32 {
    LOW_HZ * (HIGH_HZ / LOW_HZ).powf((band as f32 + 0.5) / FILTER_BANDS as f32)
}

/// The libavfilter graph measuring the sound, for audio with this many channels and this
/// layout (mpv's `audio-params`). Each band becomes an extra channel beside the originals so a
/// single astats reports them all on the frames mpv sees; pan then drops them, leaving the
/// sound as it was.
pub fn filter_graph(channels: usize, layout: &str) -> String {
    let labels = || (0..FILTER_BANDS).map(|b| format!("[b{b}]"));
    let mut graph = format!("asplit={}[o]{};", FILTER_BANDS + 1, labels().collect::<String>());
    for b in 0..FILTER_BANDS {
        graph += &format!("[b{b}]aformat=channel_layouts=mono,bandpass=f={:.1}:width_type=q:w={FILTER_Q}[m{b}];", filter_hz(b));
    }
    graph += "[o]";
    for b in 0..FILTER_BANDS {
        graph += &format!("[m{b}]");
    }
    let keep: Vec<String> = (0..channels).map(|c| format!("c{c}=c{c}")).collect();
    graph += &format!(
        "amerge=inputs={},astats=metadata=1:reset=1:measure_overall=none:measure_perchannel=RMS_level,pan={layout}|{}",
        FILTER_BANDS + 1,
        keep.join("|")
    );
    graph
}

/// What the filter measured once, as power.
#[derive(Clone, Debug, PartialEq)]
pub struct Levels {
    pub overall: f32,
    pub bands: [f32; FILTER_BANDS],
}

impl Levels {
    /// The louder of these and `other`, band by band.
    pub fn loudest_of(&self, other: &Levels) -> Levels {
        let mut bands = self.bands;
        for (a, b) in bands.iter_mut().zip(&other.bands) {
            *a = a.max(*b);
        }
        Levels { overall: self.overall.max(other.overall), bands }
    }
}

/// How many samples each measurement looks at, and how many it moves on by.
const WINDOW: usize = 2048;
const HOP: usize = 512;
/// A gap this big (seconds) between what's expected and what comes is a seek.
const SAMPLE_JUMP: f64 = 0.2;

/// Measures sound as it comes, into the same bands the filter would give.
pub struct SampleBands {
    buffer: VecDeque<f32>,
    /// When the sample at the front of `buffer` plays, and the sample rate.
    front_pts: f64,
    rate: u32,
    window: Vec<f32>,
    window_power: f32,
}

impl Default for SampleBands {
    fn default() -> Self {
        let window: Vec<f32> = (0..WINDOW).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / WINDOW as f32).cos()).collect();
        let window_power = window.iter().map(|w| w * w).sum();
        Self { buffer: VecDeque::new(), front_pts: 0.0, rate: 48_000, window, window_power }
    }
}

impl SampleBands {
    /// Some more sound, starting at `pts` (negative if that's unknown).
    pub fn push(&mut self, pts: f64, rate: u32, mono: &[f32]) {
        let expected = self.front_pts + self.buffer.len() as f64 / f64::from(self.rate);
        if self.buffer.is_empty() || rate != self.rate || (pts >= 0.0 && (pts - expected).abs() > SAMPLE_JUMP) {
            self.buffer.clear();
            self.rate = rate;
            self.front_pts = pts.max(0.0);
        }
        self.buffer.extend(mono);
    }

    /// The next measurement, if there's enough sound for one, and the moment it's centred on.
    pub fn measure(&mut self) -> Option<(f64, Levels)> {
        if self.buffer.len() < WINDOW {
            return None;
        }
        let mut re = [0f32; WINDOW];
        let mut im = [0f32; WINDOW];
        let mut overall = 0.0;
        for (i, sample) in self.buffer.iter().take(WINDOW).enumerate() {
            overall += sample * sample;
            re[i] = sample * self.window[i];
        }
        fft(&mut re, &mut im);
        let scale = 2.0 / (WINDOW as f32 * self.window_power);
        let hz_per_bin = self.rate as f32 / WINDOW as f32;
        let mut bands = [0f32; FILTER_BANDS];
        for (b, band) in bands.iter_mut().enumerate() {
            let edge = |at: f32| LOW_HZ * (HIGH_HZ / LOW_HZ).powf(at / FILTER_BANDS as f32) / hz_per_bin;
            let (low, high) = (edge(b as f32), edge(b as f32 + 1.0));
            let first = (low + 0.5).floor().max(1.0) as usize;
            let last = ((high + 0.5).floor() as usize).min(WINDOW / 2 - 1);
            for k in first..=last.max(first) {
                // How much of this bin is inside the band, so narrow low bands still get a share.
                let inside = ((k as f32 + 0.5).min(high) - (k as f32 - 0.5).max(low)).clamp(0.0, 1.0);
                *band += (re[k] * re[k] + im[k] * im[k]) * scale * inside;
            }
        }
        let centre = self.front_pts + (WINDOW / 2) as f64 / f64::from(self.rate);
        self.buffer.drain(..HOP);
        self.front_pts += HOP as f64 / f64::from(self.rate);
        Some((centre, Levels { overall: overall / WINDOW as f32, bands }))
    }
}

/// A fast Fourier transform, in place. The length has to be a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut size = 2;
    while size <= n {
        let (sin, cos) = (-std::f32::consts::TAU / size as f32).sin_cos();
        for start in (0..n).step_by(size) {
            let (mut wr, mut wi) = (1.0f32, 0.0f32);
            for k in 0..size / 2 {
                let (a, b) = (start + k, start + k + size / 2);
                let (tr, ti) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                (re[a], im[a]) = (re[a] + tr, im[a] + ti);
                (wr, wi) = (wr * cos - wi * sin, wr * sin + wi * cos);
            }
        }
        size *= 2;
    }
}

/// Reads mpv's `af-metadata/<FILTER_LABEL>` (JSON), for audio with `channels` channels. astats
/// numbers channels from 1, the originals first.
pub fn read_levels(metadata: &str, channels: usize) -> Option<Levels> {
    let map: HashMap<String, String> = serde_json::from_str(metadata).ok()?;
    let power = |channel: usize| -> Option<f32> {
        let db: f32 = map.get(&format!("lavfi.astats.{channel}.RMS_level"))?.parse().ok()?;
        Some(10f32.powf(db / 10.0))
    };
    let mut overall = 0.0;
    for c in 1..=channels {
        overall += power(c)?;
    }
    let mut bands = [0.0; FILTER_BANDS];
    for (b, band) in bands.iter_mut().enumerate() {
        *band = power(channels + 1 + b)?;
    }
    Some(Levels { overall: overall / channels.max(1) as f32, bands })
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
    /// What kind of sound it is, 0 to 1 for each of flex-depth's `sound::KINDS`, eased. Empty
    /// until it's known.
    pub kinds: Vec<f32>,
    /// The likeliest kinds of sound, for showing.
    pub heard: String,
}

impl Default for Features {
    fn default() -> Self {
        Self { level: 0.0, bass: 0.0, mid: 0.0, treble: 0.0, beat: 0.0, spectrum: vec![0.0; BANDS * HISTORY], kinds: Vec::new(), heard: String::new() }
    }
}

pub struct Analyser {
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
        Self {
            band_ref: 1e-9,
            level_ref: 1e-9,
            smooth: [0.0; BANDS],
            bass_avg: 0.0,
            since_beat: 10.0,
            history: VecDeque::new(),
            last_row: None,
            features: Features::default(),
        }
    }

    /// Takes the latest `levels` (`None` when there's no sound to give) at `video_time`; `dt`
    /// is the time since the last call. Call every frame: the filter reports less often than
    /// that, so the same levels are given again in between and everything eases towards them.
    pub fn update(&mut self, levels: Option<&Levels>, video_time: f64, dt: f32) -> &Features {
        let dt = dt.clamp(0.0, 0.25);
        let fall = 10f32.powf(-REF_FALL_DB_PER_SEC * dt / 10.0);
        let to_unit = |power: f32, reference: f32| (1.0 + 10.0 * (power / reference).max(1e-12).log10() / RANGE_DB).clamp(0.0, 1.0);
        let mut bass_power = 0.0;
        if let Some(levels) = levels {
            let bands = &levels.bands;
            let loudest = bands.iter().fold(0.0f32, |a, b| a.max(*b));
            self.band_ref = (self.band_ref * fall).max(loudest).max(1e-9);
            let rms = levels.overall;
            self.level_ref = (self.level_ref * fall).max(rms).max(1e-9);
            self.features.level = ease(self.features.level, to_unit(rms, self.level_ref), dt, 30.0, 8.0);
            // Each range's total power, against the same reference as the bands.
            let range = |lo: f32, hi: f32| -> f32 {
                bands.iter().enumerate().filter(|(b, _)| (lo..hi).contains(&filter_hz(*b))).map(|(_, p)| p).sum()
            };
            bass_power = range(0.0, 150.0);
            let (mid, treble) = (range(150.0, 2000.0), range(2000.0, f32::MAX));
            self.features.bass = ease(self.features.bass, to_unit(bass_power, self.band_ref), dt, 30.0, 6.0);
            self.features.mid = ease(self.features.mid, to_unit(mid, self.band_ref), dt, 30.0, 6.0);
            self.features.treble = ease(self.features.treble, to_unit(treble, self.band_ref), dt, 30.0, 6.0);
            // Stretched to `BANDS`, blending between the filter's bands.
            for (b, smooth) in self.smooth.iter_mut().enumerate() {
                let at = ((b as f32 + 0.5) * FILTER_BANDS as f32 / BANDS as f32 - 0.5).clamp(0.0, FILTER_BANDS as f32 - 1.0);
                let (i, t) = (at as usize, at.fract());
                let (a, z) = (to_unit(bands[i], self.band_ref), to_unit(bands[(i + 1).min(FILTER_BANDS - 1)], self.band_ref));
                *smooth = ease(*smooth, a + (z - a) * t, dt, 30.0, 6.0);
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

    fn sine(hz: f32, count: usize) -> Vec<f32> {
        (0..count).map(|i| (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin()).collect()
    }

    #[test]
    fn a_tone_lands_in_its_own_band_at_its_own_loudness() {
        let mut bands = SampleBands::default();
        bands.push(0.0, 48_000, &sine(1000.0, 4096));
        let (_, levels) = bands.measure().unwrap();
        let loudest = (0..FILTER_BANDS).max_by(|a, b| levels.bands[*a].total_cmp(&levels.bands[*b])).unwrap();
        assert!((filter_hz(loudest) / 1000.0 - 1.0).abs() < 0.2, "band {loudest} at {}", filter_hz(loudest));
        let total: f32 = levels.bands.iter().sum();
        assert!((total - 0.5).abs() < 0.03, "a sine of amplitude 1 has a power of a half: {total}");
        assert!((levels.overall - 0.5).abs() < 0.01, "{}", levels.overall);
    }

    #[test]
    fn measurements_say_when_they_are_about() {
        let mut bands = SampleBands::default();
        bands.push(10.0, 48_000, &vec![0.0; 3000]);
        let (first, _) = bands.measure().unwrap();
        assert!((first - (10.0 + 1024.0 / 48_000.0)).abs() < 1e-6);
        let (second, _) = bands.measure().unwrap();
        assert!((second - first - 512.0 / 48_000.0).abs() < 1e-6);
        assert!(bands.measure().is_none(), "not enough left");
    }

    #[test]
    fn a_seek_starts_again() {
        let mut bands = SampleBands::default();
        bands.push(5.0, 48_000, &vec![0.0; 1000]);
        bands.push(60.0, 48_000, &vec![0.0; 2048]);
        let (at, _) = bands.measure().unwrap();
        assert!((at - (60.0 + 1024.0 / 48_000.0)).abs() < 1e-6);
    }

    #[test]
    fn the_loudest_of_two_readings_keeps_each_peak() {
        let quiet = Levels { overall: 0.1, bands: [0.2; FILTER_BANDS] };
        let mut loud = quiet.clone();
        loud.bands[3] = 0.9;
        let both = quiet.loudest_of(&loud);
        assert_eq!((both.bands[3], both.bands[4], both.overall), (0.9, 0.2, 0.1));
    }

    /// What the filter would report for one tone over a very quiet floor.
    fn tone(hz: f32, amp: f32) -> Levels {
        let mut bands = [1e-9; FILTER_BANDS];
        let nearest = (0..FILTER_BANDS).min_by(|a, b| (filter_hz(*a) / hz).ln().abs().total_cmp(&(filter_hz(*b) / hz).ln().abs())).unwrap();
        bands[nearest] = amp * amp / 2.0;
        Levels { overall: amp * amp / 2.0, bands }
    }

    fn spectrum_hz(band: usize) -> f32 {
        LOW_HZ * (HIGH_HZ / LOW_HZ).powf((band as f32 + 0.5) / BANDS as f32)
    }

    #[test]
    fn the_graph_measures_bands_and_restores_the_channels() {
        let graph = filter_graph(6, "5.1");
        assert!(graph.starts_with("asplit=33[o]"), "{graph}");
        assert_eq!(graph.matches("bandpass=").count(), FILTER_BANDS);
        assert!(graph.contains("amerge=inputs=33,astats="), "{graph}");
        assert!(graph.ends_with("pan=5.1|c0=c0|c1=c1|c2=c2|c3=c3|c4=c4|c5=c5"), "{graph}");
    }

    #[test]
    fn levels_are_read_after_the_original_channels() {
        let mut map: Vec<String> = vec![r#""lavfi.astats.1.RMS_level":"-10""#.into(), r#""lavfi.astats.2.RMS_level":"-30""#.into()];
        for b in 0..FILTER_BANDS {
            let db = if b == 4 { "-20" } else { "-inf" };
            map.push(format!(r#""lavfi.astats.{}.RMS_level":"{db}""#, b + 3));
        }
        let levels = read_levels(&format!("{{{}}}", map.join(",")), 2).unwrap();
        assert!((levels.bands[4] - 0.01).abs() < 1e-5, "{:?}", levels.bands[4]);
        assert_eq!(levels.bands[5], 0.0);
        assert!((levels.overall - 0.0505).abs() < 1e-4, "{}", levels.overall);
        assert!(read_levels("{}", 2).is_none());
        assert!(read_levels("nonsense", 2).is_none());
    }

    #[test]
    fn tones_land_in_their_band() {
        let mut a = Analyser::new();
        let mut f = Features::default();
        for i in 0..60 {
            f = a.update(Some(&tone(60.0, 0.5)), i as f64 / 60.0, 1.0 / 60.0).clone();
        }
        assert!(f.bass > 0.8 && f.treble < 0.3, "bass {} treble {}", f.bass, f.treble);
        let loudest = (0..BANDS).max_by(|x, y| f.spectrum[*x].total_cmp(&f.spectrum[*y])).unwrap();
        assert!((40.0..90.0).contains(&spectrum_hz(loudest)), "60 Hz showed up at {} Hz", spectrum_hz(loudest));

        for i in 60..120 {
            f = a.update(Some(&tone(5000.0, 0.5)), i as f64 / 60.0, 1.0 / 60.0).clone();
        }
        assert!(f.treble > 0.7 && f.bass < 0.3, "bass {} treble {}", f.bass, f.treble);
    }

    #[test]
    fn quiet_scenes_still_register_after_a_while() {
        let mut a = Analyser::new();
        let mut f = Features::default();
        for i in 0..30 {
            f = a.update(Some(&tone(440.0, 0.8)), i as f64 / 60.0, 1.0 / 60.0).clone();
        }
        let loud = f.level;
        // 30 dB quieter: dim at first, then the reference comes down to meet it.
        f = a.update(Some(&tone(440.0, 0.025)), 0.5, 1.0 / 60.0).clone();
        let first = f.level;
        for i in 0..600 {
            f = a.update(Some(&tone(440.0, 0.025)), 0.5 + i as f64 / 60.0, 1.0 / 60.0).clone();
        }
        assert!(loud > 0.9 && first < loud && f.level > 0.85, "loud {loud} first {first} later {}", f.level);
    }

    #[test]
    fn beats_come_from_jumps_in_the_bass_even_with_few_reports() {
        let mut a = Analyser::new();
        let mut beats = 0;
        for i in 0..240 {
            // A kick every half second, with the filter reporting 10 times a second.
            let report = i / 6;
            let w = if report % 5 == 0 { tone(60.0, 0.8) } else { tone(60.0, 0.02) };
            if a.update(Some(&w), i as f64 / 60.0, 1.0 / 60.0).beat == 1.0 {
                beats += 1;
            }
        }
        assert!((7..=9).contains(&beats), "{beats} beats in 4 seconds of kicks every 0.5s");
    }

    #[test]
    fn history_follows_video_time() {
        let mut a = Analyser::new();
        let w = tone(1000.0, 0.5);
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
}
