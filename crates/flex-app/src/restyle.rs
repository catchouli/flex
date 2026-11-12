//! The picture redrawn in a style, for video effects that ask for it (`restyle = "<setting>"`):
//! AnimeGANv3's anime styles and the other networks in flex-depth's `animegan`, and Magenta's
//! stylization in the style of any picture (`art:` styles, see `art`), on the GPU through
//! flex-depth, in a thread of its own. The paint
//! callback hands it frames of the picture (see `effects::Renderer`) whenever it's free, at the
//! size it asks for, and picks up the redrawn ones as they come.
//!
//! A style's network (4 MB) is downloaded into the data folder the first time it's wanted. The
//! thread stops, freeing the GPU memory, once nothing has asked for a style for a while.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flex_core::effects::ART;
use flex_depth::animegan::{self, AnimeGan};
use flex_depth::magenta;

use crate::worker::{Job, Worker, fetch_model, push_capped};

/// The sizes it can run at (pixels in all), biggest first; each keeps the picture's shape.
const LEVELS: [usize; 4] = [230_000, 180_000, 140_000, 100_000];
const START_LEVEL: usize = 1;
/// A picture taking longer than this, on average (ms), moves to a smaller size; a bigger one is
/// tried if it should take no longer than ROOM.
const TOO_SLOW: f32 = 45.0;
const ROOM: f32 = 30.0;
const PACE_FOLLOW: f32 = 0.15;
/// How long to wait after changing size before judging the new one.
const SETTLE: Duration = Duration::from_secs(3);
/// After a size turned out too slow, how long before going back up to it.
const RETRY: Duration = Duration::from_secs(60);
/// At most this often (a film's every frame, if the GPU has room): between them, the nearest one
/// is moved along with the picture.
const MIN_INTERVAL: Duration = Duration::from_millis(41);
/// How much of the last picture's colours each norm keeps (see `AnimeGan::run`). Measured on
/// film, the network hardly pumps: most of what changes is what moves, so this is light.
const EASE: f32 = 0.5;
/// Pictures waiting for the player to collect them.
const MOST_WAITING: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Off,
    Starting,
    /// Downloading a style: which, and percent done.
    Downloading(&'static str, u8),
    Ready {
        style: &'static animegan::Style,
        adapter: String,
        /// How long the last picture took.
        ms: f32,
        size: (usize, usize),
    },
    /// What went wrong, and with which style: another is still tried.
    Failed(String, String),
}

impl Status {
    pub fn describe(&self) -> String {
        match self {
            Status::Off => "off".into(),
            Status::Starting => "starting…".into(),
            Status::Downloading(style, p) => format!("downloading the {style} style ({p}%)…"),
            Status::Ready { style, ms, .. } if *ms == 0.0 => format!("{}: ready · {}", style.label, style.credit),
            Status::Ready { style, adapter, ms, size } => format!("{}: {ms:.0} ms a picture at {}×{} on {adapter} · {}", style.label, size.0, size.1, style.credit),
            Status::Failed(e, _) => format!("⚠ {e}"),
        }
    }
}

/// A redrawn picture: RGBA, bottom row first (as GL textures go), `size`.
pub struct Restyled {
    pub rgba: Vec<u8>,
    pub size: (usize, usize),
    /// The position of the frame it's of.
    pub time: f32,
    /// The style it's in, by id.
    pub style: String,
}

/// A frame handed over: RGBA, bottom row first, its size, its position, and whether there's been
/// a scene cut since the last one.
type Input = (Vec<u8>, (usize, usize), f32, bool);

pub struct State {
    status: Status,
    /// The style wanted, by id.
    style: String,
    /// A frame waiting.
    input: Option<Input>,
    output: VecDeque<Arc<Restyled>>,
    not_before: Option<Instant>,
    /// Into `LEVELS`, kept so the next video starts at the size that worked for this one.
    level: usize,
    /// The painting (or paintings) Magenta's network is painting like.
    painting: String,
}

impl Default for State {
    fn default() -> Self {
        Self { status: Status::Off, style: String::new(), input: None, output: VecDeque::new(), not_before: None, level: START_LEVEL, painting: String::new() }
    }
}

impl Job for State {
    type Status = Status;
    fn status(&self) -> &Status {
        &self.status
    }
    /// A style that failed doesn't stop another being tried.
    fn failed(&self) -> bool {
        matches!(&self.status, Status::Failed(_, failed) if *failed == self.style)
    }
    fn starting(&mut self) {
        self.status = Status::Starting;
    }
    fn fail(&mut self, why: String) {
        self.status = Status::Failed(why, self.style.clone());
    }
    fn stopped(&mut self) {
        self.output.clear();
        self.not_before = None;
        if matches!(self.status, Status::Ready { .. }) {
            self.status = Status::Off;
        }
    }
}

/// Shared between the UI, the paint callback and the worker.
pub type Link = Worker<State>;

/// The network for a style id: Magenta's for every `art:` style.
fn network(id: &str) -> Option<&'static animegan::Style> {
    if id.starts_with(ART) { Some(&magenta::MODEL) } else { animegan::style(id) }
}

impl Link {
    /// An effect wants the picture in `style` (an `animegan::STYLES` id): starts the worker if it
    /// isn't running. Call every frame.
    pub fn wanted(&self, style: &str) {
        let want = |state: &mut State| {
            if state.style != style {
                state.style = style.to_owned();
                self.notify();
            }
        };
        self.want("restyle", want, |link| link.work());
    }

    /// What it's painting like, for an `art:` style: the painting's name.
    pub fn painting(&self) -> Option<String> {
        let state = self.lock();
        (state.style.starts_with(ART) && !state.painting.is_empty()).then(|| state.painting.clone())
    }

    /// The size to hand frames over at, for a picture of this shape (width over height): fewer
    /// pixels for styles that are more work each.
    pub fn size_for(&self, aspect: f32) -> (usize, usize) {
        let state = self.lock();
        let work = network(&state.style).map_or(1.0, |s| s.work);
        animegan::size_for(f64::from(aspect), (LEVELS[state.level] as f32 / work) as usize)
    }

    /// Whether the worker would take a frame now. `cut`: there's been a scene cut since the last.
    pub fn ready_for_frame(&self, cut: bool) -> bool {
        let state = self.lock();
        matches!(state.status, Status::Ready { .. }) && state.input.is_none() && (cut || state.not_before.is_none_or(|t| Instant::now() >= t))
    }

    /// Hands over a frame (RGBA, bottom row first, of `size`), its position, and whether
    /// there's been a scene cut since the last frame handed over.
    pub fn submit(&self, rgba: Vec<u8>, size: (usize, usize), time: f32, cut: bool) {
        self.lock().input = Some((rgba, size, time, cut));
        self.notify();
    }

    /// The style wanted, by id.
    pub fn style(&self) -> String {
        self.lock().style.clone()
    }

    /// The pictures that have come in since the last call, oldest first.
    pub fn take(&self) -> Vec<Arc<Restyled>> {
        self.lock().output.drain(..).collect()
    }

    /// Downloads `style` if it isn't here yet (showing how far it's got).
    fn fetch(&self, style: &'static animegan::Style) -> anyhow::Result<Vec<u8>> {
        let progress = |pct| self.lock().status = Status::Downloading(style.label, pct);
        let path = fetch_model(style.file, style.url, &format!("the {} style", style.label), style.bytes, style.sha256, progress)?;
        Ok(std::fs::read(&path)?)
    }

    fn work(&self) {
        // The style loaded, its file, and the network at the size last asked for.
        let mut loaded: Option<(&'static animegan::Style, Vec<u8>)> = None;
        let mut net: Option<AnimeGan> = None;
        let mut pace = Pace::new(self.lock().level, Instant::now());
        let mut first = true;
        // The style Magenta's network was last given, and its name.
        let mut painting: Option<(Vec<f32>, String)> = None;
        let fail = |e: anyhow::Error, style: &str| {
            log::warn!("restyle: {e:#}");
            let mut state = self.lock();
            state.status = Status::Failed(format!("{e:#}"), style.to_owned());
            state.running = false;
            state.input = None;
            state.output.clear();
        };
        loop {
            let wanted = self.lock().style.clone();
            let Some(style) = network(&wanted) else {
                fail(anyhow::anyhow!("there's no {wanted:?} style"), &wanted);
                return;
            };
            if loaded.as_ref().is_none_or(|(s, _)| s.id != style.id) {
                // The old one goes first: a slow GPU may not have room for both.
                drop(net.take());
                match self.fetch(style) {
                    Ok(bytes) => loaded = Some((style, bytes)),
                    Err(e) => return fail(e, &wanted),
                }
                first = true;
                let mut state = self.lock();
                state.status = Status::Ready { style, adapter: String::new(), ms: 0.0, size: (0, 0) };
                state.input = None;
            }
            // Nothing once another style's wanted.
            let Some(input) = self.next(|state| if state.style != wanted { Some(None) } else { state.input.take().map(Some) }) else {
                drop(net);
                log::info!("restyle: stopped");
                self.stop();
                return;
            };
            let Some((rgba, size, time, cut)) = input else { continue };
            if net.as_ref().is_none_or(|n| n.size() != size) {
                drop(net.take());
                let bytes = &loaded.as_ref().expect("loaded above").1;
                match AnimeGan::from_bytes(bytes, size) {
                    Ok(n) => {
                        log::info!("restyle: {} at {}×{} on {}", style.label, size.0, size.1, n.adapter());
                        net = Some(n);
                        painting = None;
                    }
                    Err(e) => return fail(e, &wanted),
                }
                first = true;
            }
            let Some(n) = net.as_mut() else { continue };
            let (w, h) = size;
            // GL's rows run bottom-up; the network's top-down.
            let mut rgb = vec![0u8; w * h * 3];
            for y in 0..h {
                let src = &rgba[(h - 1 - y) * w * 4..][..w * 4];
                for (x, px) in src.as_chunks::<4>().0.iter().enumerate() {
                    rgb[(y * w + x) * 3..][..3].copy_from_slice(&px[..3]);
                }
            }
            if n.takes_styles() {
                // Paintings that aren't there any more paint like the first.
                let picked = crate::art::style_at(&wanted, time).or_else(|| crate::art::style_at(&format!("{ART}{}", crate::art::PAINTINGS[0].id), time));
                if let Some((numbers, name)) = picked.filter(|(numbers, _)| painting.as_ref().is_none_or(|(last, _)| last != numbers)) {
                    if let Err(e) = n.set_style(&numbers) {
                        return fail(e, &wanted);
                    }
                    self.lock().painting = name.clone();
                    painting = Some((numbers, name));
                }
            }
            let started = Instant::now();
            let turn = crate::gpu::turn(crate::gpu::Net::Restyle);
            let result = n.run(&rgb, if first || cut { 0.0 } else { EASE });
            let ms = turn.ms();
            drop(turn);
            first = false;
            let out = match result {
                Ok(out) => out,
                Err(e) => return fail(e, &wanted),
            };
            let mut flipped = vec![0u8; out.len()];
            for y in 0..h {
                flipped[(h - 1 - y) * w * 4..][..w * 4].copy_from_slice(&out[y * w * 4..][..w * 4]);
            }
            {
                let mut state = self.lock();
                state.status = Status::Ready { style, adapter: n.adapter().to_owned(), ms, size };
                push_capped(&mut state.output, [Arc::new(Restyled { rgba: flipped, size, time, style: wanted.clone() })], MOST_WAITING);
                state.not_before = Some(started + crate::gpu::rest(crate::gpu::Net::Restyle, MIN_INTERVAL));
            }
            if let Some(level) = pace.update(ms, Instant::now()) {
                log::info!("restyle: {ms:.0} ms a picture; now {} pixels", LEVELS[level]);
                self.lock().level = level;
            }
        }
    }
}

/// Moves between sizes, from how long pictures take.
struct Pace {
    level: usize,
    average: Option<f32>,
    since: Instant,
    /// When each level last turned out too slow.
    too_slow: [Option<Instant>; LEVELS.len()],
}

impl Pace {
    fn new(level: usize, now: Instant) -> Self {
        Self { level: level.min(LEVELS.len() - 1), average: None, since: now, too_slow: [None; LEVELS.len()] }
    }

    /// The new level, if it's time to move.
    fn update(&mut self, ms: f32, now: Instant) -> Option<usize> {
        let average = self.average.map_or(ms, |a| a + (ms - a) * PACE_FOLLOW);
        self.average = Some(average);
        if now.duration_since(self.since) < SETTLE {
            return None;
        }
        let next = if average > TOO_SLOW && self.level + 1 < LEVELS.len() {
            self.too_slow[self.level] = Some(now);
            self.level + 1
        } else if self.level > 0
            && average * LEVELS[self.level - 1] as f32 / LEVELS[self.level] as f32 <= ROOM
            && self.too_slow[self.level - 1].is_none_or(|t| now.duration_since(t) > RETRY)
        {
            self.level - 1
        } else {
            return None;
        };
        self.level = next;
        self.average = None;
        self.since = now;
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_can_pick_every_style_there_is() {
        let ids: Vec<&str> = flex_depth::animegan::STYLES.iter().map(|s| s.id).collect();
        assert_eq!(ids, flex_core::effects::RESTYLES);
    }

    #[test]
    fn slow_pictures_move_to_a_smaller_size_and_back_when_theres_room() {
        let start = Instant::now();
        let mut pace = Pace::new(1, start);
        assert_eq!(pace.update(60.0, start), None, "settling");
        assert_eq!(pace.update(60.0, start + SETTLE), Some(2));
        // Room again, but that size was too slow a moment ago.
        assert_eq!(pace.update(10.0, start + SETTLE * 3), None);
        assert_eq!(pace.update(10.0, start + SETTLE * 3 + RETRY), Some(1));
    }
}
