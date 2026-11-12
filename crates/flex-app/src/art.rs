//! Styles from pictures for Magenta's stylization (flex-depth's `magenta`), which effects pick as
//! `art:` styles (see flex-core's `effects::ART`): the paintings Flex comes with (their style
//! numbers made by flex-depth's tools/magenta/styles.py, in art_styles.bin), and pictures put in
//! the styles folder, turned into styles once each by the style network on the CPU, in a thread
//! of its own, and kept in a cache. "art:morph" drifts from one to the next along the film.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant, UNIX_EPOCH};

use flex_core::effects::{ART, ART_MINE};
use flex_depth::magenta::{self, STYLE, SIDE};
use flex_plugin::Choice;
use serde::{Deserialize, Serialize};

/// A painting Flex comes with: its id after "art:", name, and who painted it when (all public
/// domain).
pub struct Painting {
    pub id: &'static str,
    pub label: &'static str,
    pub by: &'static str,
}

/// In art_styles.bin's order.
pub const PAINTINGS: [Painting; 21] = [
    Painting { id: "starry-night", label: "The Starry Night", by: "Vincent van Gogh, 1889" },
    Painting { id: "great-wave", label: "The Great Wave", by: "Katsushika Hokusai, about 1831" },
    Painting { id: "scream", label: "The Scream", by: "Edvard Munch, 1893" },
    Painting { id: "kiss", label: "The Kiss", by: "Gustav Klimt, 1908" },
    Painting { id: "composition-vii", label: "Composition VII", by: "Wassily Kandinsky, 1913" },
    Painting { id: "impression-sunrise", label: "Impression, Sunrise", by: "Claude Monet, 1872" },
    Painting { id: "temeraire", label: "The Fighting Temeraire", by: "J. M. W. Turner, 1839" },
    Painting { id: "sudden-shower", label: "Sudden Shower", by: "Utagawa Hiroshige, 1857" },
    Painting { id: "wheatfield-crows", label: "Wheatfield with Crows", by: "Vincent van Gogh, 1890" },
    Painting { id: "blue-horses", label: "The Large Blue Horses", by: "Franz Marc, 1911" },
    Painting { id: "castle-sun", label: "Castle and Sun", by: "Paul Klee, 1928" },
    Painting { id: "earthly-delights", label: "The Garden of Earthly Delights", by: "Hieronymus Bosch, about 1500" },
    Painting { id: "zodiac", label: "Zodiac", by: "Alphonse Mucha, 1896" },
    Painting { id: "tiger-storm", label: "Tiger in a Tropical Storm", by: "Henri Rousseau, 1891" },
    Painting { id: "city-rises", label: "The City Rises", by: "Umberto Boccioni, 1910" },
    Painting { id: "strawberry-thief", label: "Strawberry Thief", by: "William Morris, 1883" },
    Painting { id: "jellyfish", label: "Jellyfish", by: "Ernst Haeckel, 1904" },
    Painting { id: "ten-largest", label: "The Ten Largest, No. 3", by: "Hilma af Klint, 1907" },
    Painting { id: "feneon", label: "Portrait of Félix Fénéon", by: "Paul Signac, 1890" },
    Painting { id: "skeleton-spectre", label: "Takiyasha and the Skeleton Spectre", by: "Utagawa Kuniyoshi, about 1844" },
    Painting { id: "windows", label: "Windows Open Simultaneously", by: "Robert Delaunay, 1912" },
];

/// Each painting's style, in `PAINTINGS` order.
static BUILT_IN: LazyLock<Vec<Vec<f32>>> = LazyLock::new(|| {
    let bytes = include_bytes!("art_styles.bin");
    bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect::<Vec<_>>().chunks(STYLE).map(<[f32]>::to_vec).collect()
});

/// "art:morph": each style for this long, in seconds of film...
const HOLD: f32 = 10.0;
/// ...then this long blending into the next.
const BLEND: f32 = 5.0;
/// The folder isn't looked at more often than this.
const RESCAN: Duration = Duration::from_secs(5);
const PICTURES: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// Where pictures to paint like go.
pub fn styles_dir() -> PathBuf {
    flex_core::data_dir().join("styles")
}

fn cache_path() -> PathBuf {
    flex_core::data_dir().join("models").join("my-styles.json")
}

/// A picture from the styles folder, as a style.
#[derive(Clone, Serialize, Deserialize)]
struct Mine {
    file: String,
    bytes: u64,
    /// When it was last changed (seconds since 1970): changed pictures are turned into styles
    /// again.
    modified: u64,
    style: Vec<f32>,
}

/// A picture that couldn't be used: its size and when it was changed (as in `Mine`), and why.
type Failed = (u64, u64, String);

#[derive(Default)]
struct State {
    mine: Vec<Mine>,
    scanning: bool,
    scanned: Option<Instant>,
    /// What the last look at the folder found wrong, or is doing.
    note: String,
    /// Pictures that couldn't be turned into styles, by file: not tried again until they change.
    failed: HashMap<String, Failed>,
    /// Goes up whenever `mine` changes.
    generation: u64,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    let mine = std::fs::read(cache_path()).ok().and_then(|b| serde_json::from_slice::<Vec<Mine>>(&b).ok()).unwrap_or_default();
    Mutex::new(State { generation: 1, mine: mine.into_iter().filter(|m| m.style.len() == STYLE).collect(), ..Default::default() })
});

fn lock() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// An effect is painting in an `art:` style: looks in the styles folder now and then for new
/// pictures to turn into styles. Call every frame.
pub fn wanted() {
    let mut state = lock();
    if state.scanning || state.scanned.is_some_and(|t| t.elapsed() < RESCAN) {
        return;
    }
    state.scanning = true;
    drop(state);
    let spawned = std::thread::Builder::new().name("art styles".into()).spawn(|| {
        let note = scan().err().map(|e| format!("{e:#}"));
        let mut state = lock();
        state.scanning = false;
        state.scanned = Some(Instant::now());
        if let Some(note) = note {
            log::warn!("styles folder: {note}");
            state.note = note;
        }
    });
    if spawned.is_err() {
        lock().scanning = false;
    }
}

/// What's being done with the styles folder, or what went wrong, if anything.
pub fn note() -> String {
    lock().note.clone()
}

/// A number that changes whenever the styles folder's pictures do.
pub fn generation() -> u64 {
    lock().generation
}

/// The styles folder's pictures as choices for effects' settings, and a number that changes
/// when they do.
pub fn choices() -> (u64, Vec<Choice>) {
    let state = lock();
    (state.generation, state.mine.iter().map(|m| Choice { id: format!("{ART_MINE}{}", m.file), label: label_of(&m.file) }).collect())
}

/// A picture's name for showing: its file's, without the extension.
fn label_of(file: &str) -> String {
    Path::new(file).file_stem().map_or(file.into(), |s| s.to_string_lossy().replace(['-', '_'], " "))
}

fn picture_files() -> anyhow::Result<Vec<(String, u64, u64)>> {
    let dir = styles_dir();
    std::fs::create_dir_all(&dir)?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let picture = Path::new(&name).extension().is_some_and(|e| PICTURES.contains(&e.to_string_lossy().to_lowercase().as_str()));
        let Ok(meta) = entry.metadata() else { continue };
        if picture && meta.is_file() {
            let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            out.push((name, meta.len(), modified));
        }
    }
    out.sort();
    Ok(out)
}

/// Looks in the styles folder: pictures gone are forgotten, and new or changed ones turned into
/// styles (once Magenta's weights are here: the restyle worker downloads them).
fn scan() -> anyhow::Result<()> {
    let files = picture_files()?;
    let (known, failed) = {
        let state = lock();
        let known: HashMap<String, Mine> = state.mine.iter().map(|m| (m.file.clone(), m.clone())).collect();
        (known, state.failed.clone())
    };
    let mut kept: Vec<Mine> = Vec::new();
    let mut new = Vec::new();
    // Pictures that couldn't be used: those still there and unchanged aren't tried again.
    let mut problems: HashMap<String, Failed> = HashMap::new();
    for (file, bytes, modified) in files {
        match (known.get(&file), failed.get(&file)) {
            (Some(m), _) if m.bytes == bytes && m.modified == modified => kept.push(m.clone()),
            (_, Some(f)) if f.0 == bytes && f.1 == modified => {
                problems.insert(file, f.clone());
            }
            _ => new.push((file, bytes, modified)),
        }
    }
    let gone = kept.len() != known.len();
    if new.is_empty() {
        if gone {
            publish(kept)?;
        }
        set_problems(problems);
        return Ok(());
    }
    let weights = flex_core::data_dir().join("models").join(magenta::MODEL.file);
    if !weights.exists() {
        // The restyle worker is downloading it.
        set_problems(problems);
        return Ok(());
    }
    let total = new.len();
    lock().note = format!("turning {total} picture{} into styles…", if total == 1 { "" } else { "s" });
    let net = magenta::StyleNet::new(&magenta::read(&std::fs::read(&weights)?)?)?;
    for (i, (file, bytes, modified)) in new.into_iter().enumerate() {
        lock().note = format!("turning pictures into styles: {} of {total}…", i + 1);
        let started = Instant::now();
        match picture(&styles_dir().join(&file)).and_then(|p| net.style(&p)) {
            Ok(style) => {
                log::info!("styles folder: {file} in {} ms", started.elapsed().as_millis());
                kept.push(Mine { file, bytes, modified, style });
            }
            Err(e) => {
                log::warn!("styles folder: {file}: {e:#}");
                problems.insert(file, (bytes, modified, format!("{e:#}")));
            }
        }
    }
    kept.sort_by(|a, b| a.file.cmp(&b.file));
    publish(kept)?;
    set_problems(problems);
    Ok(())
}

/// Remembers the pictures that couldn't be used, and says which (or nothing, when there are none).
fn set_problems(problems: HashMap<String, Failed>) {
    let mut said: Vec<String> = problems.iter().map(|(file, (_, _, e))| format!("{file}: {e}")).collect();
    said.sort();
    let mut state = lock();
    state.note = if said.is_empty() { String::new() } else { format!("couldn't use {}", said.join("; ")) };
    state.failed = problems;
}

fn publish(mine: Vec<Mine>) -> anyhow::Result<()> {
    let path = cache_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_vec(&mine)?)?;
    let mut state = lock();
    state.mine = mine;
    state.generation += 1;
    Ok(())
}

/// A picture as the style network takes it: the square in the middle, `SIDE` x `SIDE`,
/// [3][SIDE][SIDE] from 0 to 1.
fn picture(path: &Path) -> anyhow::Result<Vec<f32>> {
    let rgb = image::open(path)?.to_rgb8();
    let (w, h) = rgb.dimensions();
    let side = w.min(h);
    let square = image::imageops::crop_imm(&rgb, (w - side) / 2, (h - side) / 2, side, side).to_image();
    let small = image::imageops::resize(&square, SIDE as u32, SIDE as u32, image::imageops::FilterType::Triangle);
    let n = SIDE * SIDE;
    let mut planes = vec![0.0; 3 * n];
    for (i, p) in small.pixels().enumerate() {
        for c in 0..3 {
            planes[c * n + i] = f32::from(p[c]) / 255.0;
        }
    }
    Ok(planes)
}

/// The styles "art:morph" goes through, in its order: every one (or with `mine`, the styles
/// folder's, if it has any), mixed up the same way every time.
fn morph_list(state: &State, mine: bool) -> Vec<(String, Vec<f32>)> {
    let built_in = PAINTINGS.iter().zip(BUILT_IN.iter()).map(|(p, s)| (p.label.to_owned(), s.clone()));
    let theirs = state.mine.iter().map(|m| (label_of(&m.file), m.style.clone()));
    let mut list: Vec<(String, Vec<f32>)> = if mine && !state.mine.is_empty() { theirs.collect() } else { built_in.chain(theirs).collect() };
    // FNV-1a of the name: a fixed order that isn't alphabetical.
    let hash = |s: &str| s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    list.sort_by_key(|(name, _)| hash(name));
    list
}

/// The style `id` ("art:...") paints in at `time` (seconds into the film), and its name.
pub fn style_at(id: &str, time: f32) -> Option<(Vec<f32>, String)> {
    let state = lock();
    let name = id.strip_prefix(ART)?;
    if name == "morph" || name == "morph-mine" {
        let list = morph_list(&state, name == "morph-mine");
        let period = HOLD + BLEND;
        let turn = (time.max(0.0) / period).floor();
        let at = time.max(0.0) - turn * period;
        let (a, b) = (&list[turn as usize % list.len()], &list[(turn as usize + 1) % list.len()]);
        if at < HOLD {
            return Some((a.1.clone(), a.0.clone()));
        }
        let t = (at - HOLD) / BLEND;
        let t = t * t * (3.0 - 2.0 * t);
        let mixed = a.1.iter().zip(&b.1).map(|(x, y)| x + (y - x) * t).collect();
        return Some((mixed, format!("{} → {}", a.0, b.0)));
    }
    if let Some(file) = id.strip_prefix(ART_MINE) {
        return state.mine.iter().find(|m| m.file == file).map(|m| (m.style.clone(), label_of(&m.file)));
    }
    PAINTINGS.iter().zip(BUILT_IN.iter()).find(|(p, _)| p.id == name).map(|(p, s)| (s.clone(), format!("{} ({})", p.label, p.by)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_painting_has_its_style() {
        assert_eq!(BUILT_IN.len(), PAINTINGS.len());
        assert!(BUILT_IN.iter().all(|s| s.len() == STYLE && s.iter().all(|v| v.is_finite())));
        // Styles differ.
        assert_ne!(BUILT_IN[0], BUILT_IN[1]);
    }

    #[test]
    fn morphing_holds_each_style_then_blends_into_the_next() {
        let (first, name) = style_at("art:morph", 1.0).unwrap();
        assert_eq!(style_at("art:morph", HOLD - 0.5).unwrap().0, first);
        assert!(!name.contains('→'));
        let (halfway, blending) = style_at("art:morph", HOLD + BLEND / 2.0).unwrap();
        assert!(blending.contains('→'));
        let (next, _) = style_at("art:morph", HOLD + BLEND + 1.0).unwrap();
        assert_ne!(next, first);
        for i in 0..STYLE {
            assert!((halfway[i] - (first[i] + next[i]) / 2.0).abs() < 1e-4);
        }
        assert_eq!(style_at("art:scream", 3.0).unwrap().0, BUILT_IN[2]);
        assert!(style_at("art:nothing", 0.0).is_none());
    }
}
