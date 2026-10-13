//! Turns file and folder names into titles, seasons and episode numbers.
//!
//! `hunch` (a guessit port) does most of the work. On top of it:
//! - an episode number is only trusted if it varies between files in the same folder;
//!   otherwise the number that does vary is used ("Show 01 (x264 2.0x2)" style names);
//! - a season comes from an explicit marker in the file name, else the nearest
//!   "Season N" / "SN" folder, ignoring ranges like "Season 1-2";
//! - "Extras", "Specials", "NCOP" and similar count as specials (season 0).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::library::MediaFile;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Parsed {
    pub title: Option<String>,
    pub year: Option<i32>,
    /// `Some(0)` for specials and extras.
    pub season: Option<i32>,
    pub episode: Option<i32>,
}

static FILE_SEASON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bS(\d{1,2})(?:E\d|\b)|\bseason[ ._-]*(\d{1,2})\b").unwrap());
static DIR_SEASON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:season|series)[ ._-]*(\d{1,2})\b|\bS(\d{1,2})\b").unwrap());
static SEASON_RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:S|season[ ._-]*)\d{1,2}\s*[-~&+]\s*(?:S|season[ ._-]*)?\d{1,2}\b").unwrap());
static SPECIALS_DIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:extras?|specials?|bonus|featurettes?|nc(?:op|ed)s?|creditless|sps?|menus?)\b").unwrap()
});
static EXTRA_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:NCOP|NCED|NC ?OP|NC ?ED)\d*\b").unwrap());
/// Bits of release names whose digits aren't episode numbers.
static NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?ix)
        \[[0-9a-f]{8}\] | \([0-9a-f]{8}\)        # CRC32
        | \b\d{3,4}[pi]\b | \b\d{3,4}x\d{3,4}\b  # resolutions
        | [xh]\.?26[45] | \b\d{1,2}[ -]?bits?\b  # codecs, bit depth
        | \b\d\.\d\b | \b(?:19|20)\d{2}\b         # audio channels, years
    ")
    .unwrap()
});
/// Encoding/language leftovers at the end of a title ("1080p;H265", "JP", "tvdb4", "001").
static TRAILING_TAGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:[\s._;,-]+(?:(?:480|576|720|1080|2160)p?|[xh]\.?26[45]|hevc|avc|jp|jap|jpn|eng|dub|sub|raw|tvdb[\d-]*))+$|\s\d{1,3}$").unwrap()
});
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());
static TRAILING_JUNK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[\s._-]*(?:\(?\d{1,4}\s*-\s*\d{1,4}\)?|(?:the\s+)?(?:complete|full)(?:\s+series)?|batch|collection|\bS\d{1,2}(?:\s*-\s*S?\d{1,2})?|season[\s._-]*\d+(?:\s*-\s*\d+)?|\b\d+(?:st|nd|rd|th)(?:\s+season)?)\s*$",
    )
        .unwrap()
});

fn parent_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(d, _)| d)
}

fn file_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// Whether a folder name means extras/specials ("Extras", "Featurettes", "NCOP"...).
pub fn is_specials_dir(name: &str) -> bool {
    SPECIALS_DIR.is_match(name)
}

static DVD_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:VTS_\d{2}_\d|VIDEO_TS)\.(?:VOB|IFO|BUP)$").unwrap());

/// For a file inside a DVD (`VIDEO_TS`) or Blu-ray (`BDMV`) folder structure, the path of
/// that disc folder, which plays as a whole disc. DVD files sitting directly in a folder
/// (no `VIDEO_TS`) make that folder the disc.
pub fn disc_folder(rel_path: &str) -> Option<&str> {
    let mut end = 0;
    for component in rel_path.split('/') {
        let start = end;
        end += component.len() + 1;
        if component.eq_ignore_ascii_case("VIDEO_TS") || component.eq_ignore_ascii_case("BDMV") {
            return Some(&rel_path[..start + component.len()]);
        }
    }
    DVD_FILE.is_match(file_name(rel_path)).then(|| parent_of(rel_path))
}

/// Season from a folder name, or `None` (also for ranges such as "Season 1-2").
pub fn dir_season(name: &str) -> Option<i32> {
    if SPECIALS_DIR.is_match(name) {
        return Some(0);
    }
    if SEASON_RANGE.is_match(name) {
        return None;
    }
    let caps = DIR_SEASON.captures(name)?;
    caps.get(1).or(caps.get(2))?.as_str().parse().ok()
}

/// Season from the nearest folder that names one, walking up from the file.
fn season_from_dirs(rel_path: &str) -> Option<i32> {
    let mut dir = parent_of(rel_path);
    while !dir.is_empty() {
        if let Some(s) = dir_season(file_name(dir)) {
            return Some(s);
        }
        dir = parent_of(dir);
    }
    None
}

/// A cleaned-up show title from a folder or file name.
pub fn clean_title(raw: &str) -> String {
    let mut t = raw.trim().to_owned();
    // Strip trailing "01-40", "Complete", "S1 - S3", "Season 1-2" (possibly repeatedly).
    loop {
        let stripped = TRAILING_JUNK.replace(&t, "").trim().to_owned();
        let stripped = TRAILING_TAGS.replace(&stripped, "").trim().to_owned();
        if stripped == t || stripped.is_empty() {
            break;
        }
        t = stripped;
    }
    t.trim_matches(|c: char| c == '-' || c == '.' || c.is_whitespace()).to_owned()
}

/// The digit runs in a name, ignoring resolutions, codecs, CRCs and the like.
fn numbers(name: &str) -> Vec<i32> {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let cleaned = NOISE.replace_all(stem, " ");
    NUMBER.find_iter(&cleaned).filter_map(|m| m.as_str().parse().ok()).collect()
}

/// For files in one folder: the position of the number that varies most between them,
/// taken as the episode number.
fn varying_numbers(names: &[&str]) -> Vec<Option<i32>> {
    let nums: Vec<Vec<i32>> = names.iter().map(|n| numbers(n)).collect();
    let max_len = nums.iter().map(Vec::len).max().unwrap_or(0);
    let best = (0..max_len)
        .map(|k| {
            let values: HashSet<i32> = nums.iter().filter_map(|n| n.get(k).copied()).filter(|v| *v < 5000).collect();
            (values.len(), k)
        })
        .filter(|(distinct, _)| *distinct > 1)
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    match best {
        Some((_, k)) => nums.iter().map(|n| n.get(k).copied().filter(|v| *v < 5000)).collect(),
        None => vec![None; names.len()],
    }
}

/// How many neighbouring file names hunch gets as context.
const CONTEXT_SIBLINGS: usize = 8;

/// Parses episodic files (TV / anime), folder by folder so siblings give context.
pub fn parse_episodes(files: &[MediaFile]) -> HashMap<i64, Parsed> {
    let mut by_dir: HashMap<&str, Vec<&MediaFile>> = HashMap::new();
    for f in files {
        by_dir.entry(parent_of(&f.rel_path)).or_default().push(f);
    }
    let mut out = HashMap::new();
    for (dir, files) in by_dir {
        let mut files = files;
        files.sort_by(|a, b| crate::natural::natural_cmp(a.file_name(), b.file_name()));
        let names: Vec<&str> = files.iter().map(|f| f.file_name()).collect();
        // A few neighbours give hunch enough context; the whole folder makes it quadratic
        // (a 1,000-episode folder took most of a minute).
        let parsed: Vec<hunch::HunchResult> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let lo = i.saturating_sub(CONTEXT_SIBLINGS / 2);
                let hi = (lo + CONTEXT_SIBLINGS + 1).min(names.len());
                let siblings: Vec<&str> = names[lo..hi].iter().copied().filter(|s| s != n).collect();
                hunch::hunch_with_context(n, &siblings)
            })
            .collect();

        // Trust hunch's episode numbers only if they actually distinguish the files.
        let hunch_eps: Vec<Option<i32>> = parsed.iter().map(|p| p.episode()).collect();
        let distinct: HashSet<i32> = hunch_eps.iter().flatten().copied().collect();
        let trusted = files.len() < 3 || distinct.len() * 2 > files.len();
        // Also needed when hunch found nothing for some files.
        let fallback = if trusted && hunch_eps.iter().all(Option::is_some) {
            vec![None; names.len()]
        } else {
            varying_numbers(&names)
        };

        let dir_season = season_from_dirs(&format!("{dir}/x"));
        for (i, (file, p)) in files.iter().zip(&parsed).enumerate() {
            let name = names[i];
            let episode = if trusted { hunch_eps[i].or(fallback[i]) } else { fallback[i] };
            let extra = EXTRA_FILE.is_match(name) || p.media_type() == Some(hunch::MediaType::Extra);
            let file_season = FILE_SEASON
                .captures(name)
                .and_then(|c| c.get(1).or(c.get(2)))
                .and_then(|m| m.as_str().parse().ok());
            let season = if extra { Some(0) } else { file_season.or(dir_season) };
            out.insert(
                file.id,
                Parsed { title: p.title().map(clean_title), year: p.year(), season, episode },
            );
        }
    }
    out
}

/// Parses movie files one by one (siblings in a big movies folder only confuse things).
pub fn parse_movies(files: &[MediaFile]) -> HashMap<i64, Parsed> {
    files
        .iter()
        .map(|f| {
            let p = hunch::hunch(f.file_name());
            let mut title = p.title().map(clean_title);
            if let (Some(t), Some(part)) = (&mut title, p.part()) {
                t.push_str(&format!(" Part {part}"));
            }
            // Folder names often carry the year when the file doesn't.
            let year = p.year().or_else(|| {
                let dir = parent_of(&f.rel_path);
                (!dir.is_empty()).then(|| hunch::hunch(file_name(dir)).year()).flatten()
            });
            (f.id, Parsed { title, year, season: None, episode: None })
        })
        .collect()
}

/// Whether a folder's files look like episodes (most carry an episode number). Used to
/// spot TV series that live inside a movies library.
pub fn looks_episodic(ids: &[i64], parsed: &HashMap<i64, Parsed>) -> bool {
    let numbered = ids.iter().filter(|id| parsed.get(id).is_some_and(|p| p.episode.is_some())).count();
    ids.len() >= 3 && numbered * 3 >= ids.len() * 2
}

/// Parses a whole library: episodes for TV/anime; for movies, films one by one plus any
/// folders that turn out to hold a series.
pub fn parse_library(kind: crate::LibraryKind, files: &[MediaFile]) -> HashMap<i64, Parsed> {
    by_folder(files).into_values().flat_map(|files| parse_folder(kind, &files)).collect()
}

/// Bump when parsing changes (including a hunch upgrade), so cached results are redone.
pub const PARSER_VERSION: u32 = 1;

/// A library's files by folder (relative path, "" for the root).
pub fn by_folder(files: &[MediaFile]) -> HashMap<&str, Vec<MediaFile>> {
    let mut by_dir: HashMap<&str, Vec<MediaFile>> = HashMap::new();
    for f in files {
        by_dir.entry(parent_of(&f.rel_path)).or_default().push(f.clone());
    }
    by_dir
}

/// Parses the files of one folder. Each folder is parsed independently, so results can be
/// cached per folder (see [`folder_fingerprint`]).
pub fn parse_folder(kind: crate::LibraryKind, files: &[MediaFile]) -> HashMap<i64, Parsed> {
    if kind.is_episodic() {
        return parse_episodes(files);
    }
    let parsed = parse_movies(files);
    // A series inside a movies library.
    let ids: Vec<i64> = files.iter().map(|f| f.id).collect();
    let quick: HashMap<i64, Parsed> = files
        .iter()
        .map(|f| (f.id, Parsed { episode: hunch::hunch(f.file_name()).episode(), ..Default::default() }))
        .collect();
    if looks_episodic(&ids, &quick) { parse_episodes(files) } else { parsed }
}

/// Identifies what a folder's parse depends on: the parser, the library kind and the
/// folder's files. Only compared for equality, so the hash needn't be stable across builds.
pub fn folder_fingerprint(kind: crate::LibraryKind, files: &[MediaFile]) -> String {
    use std::hash::{Hash, Hasher};
    let mut entries: Vec<(i64, &str)> = files.iter().map(|f| (f.id, f.rel_path.as_str())).collect();
    entries.sort_unstable();
    let mut h = std::hash::DefaultHasher::new();
    (PARSER_VERSION, kind.as_str(), entries).hash(&mut h);
    format!("{:016x}", h.finish())
}

/// One library parses at a time, so organising several at startup can't take over the
/// whole machine (the UI froze on the TV PC when they all used every core).
static PARSING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Parses folders on half the cores at low priority, biggest first so one huge folder
/// doesn't finish last.
pub fn parse_folders(kind: crate::LibraryKind, folders: &[&[MediaFile]]) -> Vec<HashMap<i64, Parsed>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    if folders.is_empty() {
        return Vec::new();
    }
    let _one_at_a_time = PARSING.lock().unwrap_or_else(|e| e.into_inner());
    let mut order: Vec<usize> = (0..folders.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(folders[i].len()));
    let next = AtomicUsize::new(0);
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    let threads = (cores / 2).max(1).min(folders.len());
    let mut out: Vec<HashMap<i64, Parsed>> = vec![HashMap::new(); folders.len()];
    let done: Vec<(usize, HashMap<i64, Parsed>)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    lower_thread_priority();
                    let mut mine = Vec::new();
                    while let Some(&i) = order.get(next.fetch_add(1, Ordering::Relaxed)) {
                        mine.push((i, parse_folder(kind, folders[i])));
                    }
                    mine
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().expect("parser thread panicked")).collect()
    });
    for (i, parsed) in done {
        out[i] = parsed;
    }
    out
}

/// Lets the UI thread come first while parsing.
fn lower_thread_priority() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

/// [`parse_library`], reusing the parses of folders that haven't changed since last time
/// (stored in the database) and parsing the rest in parallel.
pub fn parse_library_cached(
    db: &mut crate::Db,
    library_id: i64,
    kind: crate::LibraryKind,
    files: &[MediaFile],
) -> anyhow::Result<HashMap<i64, Parsed>> {
    let mut cache = db.parse_cache(library_id)?;
    let mut out = HashMap::with_capacity(files.len());
    let mut stale: Vec<(&str, String, Vec<MediaFile>)> = Vec::new();
    let folders = by_folder(files);
    for (dir, files) in &folders {
        let fingerprint = folder_fingerprint(kind, files);
        let cached = cache
            .remove(*dir)
            .filter(|(f, _)| *f == fingerprint)
            .and_then(|(_, data)| serde_json::from_str::<Vec<(i64, Parsed)>>(&data).ok());
        match cached {
            Some(parsed) => out.extend(parsed),
            None => stale.push((dir, fingerprint, files.clone())),
        }
    }
    let slices: Vec<&[MediaFile]> = stale.iter().map(|(_, _, f)| f.as_slice()).collect();
    let mut fresh = Vec::with_capacity(stale.len());
    for ((dir, fingerprint, _), parsed) in stale.iter().zip(parse_folders(kind, &slices)) {
        let mut rows: Vec<(i64, Parsed)> = parsed.into_iter().collect();
        rows.sort_unstable_by_key(|(id, _)| *id);
        fresh.push((dir.to_string(), fingerprint.clone(), serde_json::to_string(&rows)?));
        out.extend(rows);
    }
    // Whatever is left in `cache` is for folders that are gone.
    let gone: Vec<String> = cache.into_keys().collect();
    if !fresh.is_empty() || !gone.is_empty() {
        db.save_parse_cache(library_id, &fresh, &gone)?;
    }
    log::info!("library {library_id}: parsed {} of {} folders ({} cached)", fresh.len(), folders.len(), folders.len() - fresh.len());
    Ok(out)
}

/// Lower-cased letters and digits only, for deciding whether two titles are the same show.
pub fn normalize(title: &str) -> String {
    let t: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { ' ' })
        .collect();
    let words: Vec<&str> = t.split_whitespace().collect();
    let words = match words.first() {
        Some(&"the") if words.len() > 1 => &words[1..],
        _ => &words[..],
    };
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> Vec<MediaFile> {
        paths
            .iter()
            .enumerate()
            .map(|(i, p)| MediaFile { id: i as i64, rel_path: (*p).into(), ..Default::default() })
            .collect()
    }

    fn episodes(paths: &[&str]) -> Vec<(Option<i32>, Option<i32>)> {
        let fs = files(paths);
        let parsed = parse_episodes(&fs);
        fs.iter().map(|f| (parsed[&f.id].season, parsed[&f.id].episode)).collect()
    }

    #[test]
    fn cached_parse_matches_and_redoes_only_changed_folders() {
        use crate::LibraryKind::Anime;
        let mut db = crate::Db::open_in_memory().unwrap();
        let lib = db.add_library("anime", std::path::Path::new("/anime"), Anime).unwrap();
        let mut fs = files(&["A/Show A - 01.mkv", "A/Show A - 02.mkv", "B/Show B - 01.mkv", "C/Show C - 01.mkv"]);
        let first = parse_library_cached(&mut db, lib, Anime, &fs).unwrap();
        assert_eq!(first, parse_library(Anime, &fs));
        assert_eq!(db.parse_cache(lib).unwrap().len(), 3);

        // Poison B's cached entry: it must be reused as is while B is unchanged.
        let poisoned = serde_json::to_string(&[(2, Parsed { title: Some("cached".into()), ..Default::default() })]).unwrap();
        let fp = folder_fingerprint(Anime, &fs[2..3]);
        db.save_parse_cache(lib, &[("B".into(), fp, poisoned)], &[]).unwrap();
        // Rename a file in A and drop folder C: A is parsed again, C is forgotten.
        fs[1].rel_path = "A/Show A - 03.mkv".into();
        fs.pop();
        let second = parse_library_cached(&mut db, lib, Anime, &fs).unwrap();
        assert_eq!(second[&2].title.as_deref(), Some("cached"));
        assert_eq!(second[&1].episode, Some(3));
        assert!(!second.contains_key(&3));
        assert_eq!(db.parse_cache(lib).unwrap().len(), 2);
    }

    #[test]
    fn untrusted_episode_numbers_fall_back_to_the_varying_number() {
        let dir = "Tenkuu no Escaflowne (BDRip 1440x1080p x265 HEVC FLACx3 5.1+2.0x2)(Triple Audio)[sxales]";
        let paths: Vec<String> = (1..=4)
            .map(|e| format!("{dir}/Tenkuu no Escaflowne - {e:02} (BDRip 1440x1080p x265 HEVC FLACx3 5.1+2.0x2)(Triple Audio)[sxales].mkv"))
            .collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let eps: Vec<_> = episodes(&refs).into_iter().map(|(_, e)| e).collect();
        assert_eq!(eps, [Some(1), Some(2), Some(3), Some(4)]);
    }

    #[test]
    fn bare_numbers_and_crcs() {
        let paths = [
            "Bush Baby 01-40/[Tipota] Daisougen no Chiisana Tenshi Bush Baby 01 (DVD 640x480 x264 10bit AC3) [0BBF27A2].mp4",
            "Bush Baby 01-40/[Tipota] Daisougen no Chiisana Tenshi Bush Baby 02 (DVD 640x480 x264 10bit AC3) [1A2B3C4D].mp4",
            "Bush Baby 01-40/[Tipota] Daisougen no Chiisana Tenshi Bush Baby 03 (DVD 640x480 x264 10bit AC3) [99999999].mp4",
        ];
        let eps: Vec<_> = episodes(&paths).into_iter().map(|(_, e)| e).collect();
        assert_eq!(eps, [Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn seasons_from_files_and_nearest_folder() {
        let got = episodes(&[
            "[Anime Time] Attack on Titan S1 - S3+OVA/Attack on Titan S2/Attack On Titan S2 - 01.mkv",
            "[Judas] Log Horizon (Season 1-2)/Season 2/[Judas] Log Horizon - 03.mkv",
            "[Judas] Log Horizon (Season 1-2)/Season 1/Extras/[Judas] Log Horizon S1 - NCED01.mkv",
            "Re Zero/Season 1/ReZERO - S01E02 - Reunion With The Witch.mkv",
            "(CBB) Dorohedoro (BD 1080p)/(CBB) Dorohedoro - 05 (BD 1080p)(Dual-Audio).mkv",
        ]);
        assert_eq!(got[0].0, Some(2), "nearest folder wins over the top folder's range");
        assert_eq!(got[1], (Some(2), Some(3)));
        assert_eq!(got[2].0, Some(0), "extras are specials");
        assert_eq!(got[3], (Some(1), Some(2)));
        assert_eq!(got[4], (None, Some(5)));
    }

    #[test]
    fn folder_seasons() {
        assert_eq!(dir_season("Season 03"), Some(3));
        assert_eq!(dir_season("Attack on Titan S2"), Some(2));
        assert_eq!(dir_season("[Judas] Log Horizon (Season 1-2) [BD 1080p]"), None);
        assert_eq!(dir_season("[Anime Time] Attack on Titan S1 - S3+OVA [BD]"), None);
        assert_eq!(dir_season("Extras"), Some(0));
        assert_eq!(dir_season("Mobile Suit Gundam Wing"), None);
    }

    #[test]
    fn disc_folders() {
        assert_eq!(disc_folder("Film (1978)/VIDEO_TS/VTS_01_1.VOB"), Some("Film (1978)/VIDEO_TS"));
        assert_eq!(disc_folder("Film/DVD1/video_ts/VIDEO_TS.VOB"), Some("Film/DVD1/video_ts"));
        assert_eq!(disc_folder("Film/BDMV/STREAM/00001.m2ts"), Some("Film/BDMV"));
        assert_eq!(disc_folder("THE_DRAGON_FAMILY/VTS_01_2.VOB"), Some("THE_DRAGON_FAMILY"));
        assert_eq!(disc_folder("Film/Film.mkv"), None);
    }

    #[test]
    fn titles() {
        assert_eq!(clean_title("Daisougen no Chiisana Tenshi Bush Baby 01-40"), "Daisougen no Chiisana Tenshi Bush Baby");
        assert_eq!(clean_title("Heion Sedai no Idaten-tachi (01-11)"), "Heion Sedai no Idaten-tachi");
        assert_eq!(clean_title("Code Geass Full Series"), "Code Geass");
        assert_eq!(clean_title("Dandadan 2nd"), "Dandadan");
        assert_eq!(clean_title("Dandadan 2nd Season"), "Dandadan");
        assert_eq!(clean_title("Inukami! 1080p;H265"), "Inukami!");
        assert_eq!(clean_title("Seto no Hanayome 1080"), "Seto no Hanayome");
        assert_eq!(clean_title("Golden Boy JP"), "Golden Boy");
        assert_eq!(clean_title("Sergeant Keroro 001"), "Sergeant Keroro");
        assert_eq!(clean_title("One Piece tvdb4"), "One Piece");
        assert_eq!(clean_title("One Piece tvdb4-81797"), "One Piece");
        assert_eq!(clean_title("Ghost in the Shell 2.0"), "Ghost in the Shell 2.0");
        assert_eq!(clean_title("86"), "86", "never strips a title to nothing");
        assert_eq!(clean_title("Mobile Suit Gundam 0083"), "Mobile Suit Gundam 0083");
        assert_eq!(normalize("The Lost Universe!"), "lost universe");
        assert_eq!(normalize("Attack On Titan"), normalize("attack on titan"));
    }

    #[test]
    fn movies_keep_year_and_part() {
        let fs = files(&[
            "Christmas.Vacation.1989.REMASTERED.1080p.BluRay.X264-AMIABLE.mkv",
            "Back to the Future Part III 1990 1080p BluRay AAC 7.1 x265-numa.mkv",
            "Lettera Amorosa (1995) -- João César Monteiro/Lettera.Amorosa.mkv",
        ]);
        let p = parse_movies(&fs);
        assert_eq!((p[&0].title.as_deref(), p[&0].year), (Some("Christmas Vacation"), Some(1989)));
        assert_eq!(p[&1].title.as_deref(), Some("Back to the Future Part 3"));
        assert_eq!(p[&2].year, Some(1995), "year from the folder");
    }
}
