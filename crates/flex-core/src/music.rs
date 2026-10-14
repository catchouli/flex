//! Music libraries: tracks' tags, grouped into albums and artists.
//!
//! Folder layouts are too varied to trust ("Artist/Album/Disc 01", "[090325] 9th Single -
//! ...", folders of loose singles), so albums come from the tags: tracks in the same folder
//! (a "Disc N" folder counts as its parent) with the same album tag are one album. Tracks
//! without an album tag are loose singles, gathered per folder.

use std::collections::HashMap;
use std::path::Path;

use lofty::prelude::*;
use serde::{Deserialize, Serialize};

use crate::library::{MediaFile, join_rel};
use crate::natural::natural_cmp;

pub const AUDIO_EXTENSIONS: &[&str] =
    &["mp3", "flac", "opus", "ogg", "oga", "m4a", "aac", "wav", "wma", "aif", "aiff", "ape", "wv", "mka"];

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|a| a.eq_ignore_ascii_case(e)))
}

/// Bump when what's read from files changes, so cached tags are read again.
const TAGS_VERSION: u32 = 1;
/// Album art next to the tracks, best first (matched without case or extension).
const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album", "albumart"];
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];
const VARIOUS: &str = "Various Artists";
/// Folders read at once.
const READERS: usize = 8;
/// Cache new tags after this many folders.
const SAVE_EVERY: usize = 100;

/// What a track's tags say. Empty strings are stored as `None`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub year: Option<i32>,
    /// Seconds (0 if unknown).
    pub duration: f64,
}

fn text(s: Option<impl AsRef<str>>) -> Option<String> {
    s.map(|s| s.as_ref().trim().to_owned()).filter(|s| !s.is_empty())
}

/// Reads a file's tags, and its embedded picture if `want_picture`. Unreadable files give
/// empty tags (they still play; the file name stands in).
pub fn read_tags(path: &Path, want_picture: bool) -> (Tags, Option<Vec<u8>>) {
    let file = match lofty::read_from_path(path) {
        Ok(f) => f,
        Err(e) => {
            log::debug!("tags {}: {e}", path.display());
            return (Tags::default(), None);
        }
    };
    let duration = file.properties().duration().as_secs_f64();
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else {
        return (Tags { duration, ..Tags::default() }, None);
    };
    let tags = Tags {
        title: text(tag.title()),
        artist: text(tag.artist()),
        album: text(tag.album()),
        album_artist: text(tag.get_string(ItemKey::AlbumArtist)),
        track: tag.track().filter(|t| *t > 0),
        disc: tag.disk().filter(|d| *d > 0),
        year: tag.date().map(|d| i32::from(d.year)).filter(|y| *y > 0),
        duration,
    };
    let picture = want_picture.then(|| tag.pictures().first().map(|p| p.data().to_vec())).flatten();
    (tags, picture)
}

/// Splits an artist tag that names several ("A; B", "A / B", "A feat. B").
pub fn split_artists(s: &str) -> Vec<String> {
    let mut parts = vec![s.to_owned()];
    for sep in [";", " / ", " feat. ", " feat ", " ft. ", " Feat. ", " ft "] {
        parts = parts.iter().flat_map(|p| p.split(sep).map(str::to_owned).collect::<Vec<_>>()).collect();
    }
    let mut out: Vec<String> = Vec::new();
    for p in parts.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(p)) {
            out.push(p.to_owned());
        }
    }
    out
}

/// The folder an album lives in: a track's folder, or its parent for "Disc 1", "CD2", ...
pub fn album_dir(rel_path: &str) -> &str {
    let dir = rel_path.rsplit_once('/').map_or("", |(d, _)| d);
    let (parent, name) = dir.rsplit_once('/').unwrap_or(("", dir));
    let lower = name.to_ascii_lowercase();
    let rest = ["disc", "disk", "cd"].iter().find_map(|p| lower.strip_prefix(p));
    let is_disc = rest.is_some_and(|r| {
        let r = r.trim_start_matches([' ', '_', '-', '.']);
        !r.is_empty() && r.chars().take_while(|c| c.is_ascii_digit()).count() > 0
    });
    if is_disc && !dir.is_empty() { parent } else { dir }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub file_id: i64,
    pub title: String,
    /// The track's own artists (may differ from the album's on compilations).
    pub artists: Vec<String>,
    pub number: Option<u32>,
    pub disc: Option<u32>,
    pub duration: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Album {
    /// Stable across rebuilds: folder + album tag.
    pub key: String,
    pub title: String,
    /// Who the album is by, for display ("Various Artists" for mixed compilations).
    pub artist: String,
    /// Everyone on it: album artists first, then other track artists.
    pub artists: Vec<String>,
    pub year: Option<i32>,
    /// In play order.
    pub tracks: Vec<Track>,
    /// Image path: art next to the tracks, or a cached copy of embedded art.
    pub cover: Option<String>,
    /// When its newest track was first seen.
    pub added_at: i64,
    /// Not an album: tracks with no album tag, gathered from one folder. Titled "Singles"
    /// when the folder is named after the artist, otherwise after the folder.
    pub singles: bool,
}

impl Album {
    pub fn duration(&self) -> f64 {
        self.tracks.iter().map(|t| t.duration).sum()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Artist {
    pub name: String,
    /// Indexes into [`MusicCatalog::albums`]: albums by them, newest first.
    pub albums: Vec<usize>,
    /// Albums by others that they're on.
    pub appears_on: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct MusicCatalog {
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub tracks: usize,
}

impl MusicCatalog {
    pub fn album(&self, key: &str) -> Option<(usize, &Album)> {
        self.albums.iter().enumerate().find(|(_, a)| a.key == key)
    }

    pub fn artist(&self, name: &str) -> Option<&Artist> {
        self.artists.iter().find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// The album a file is on, and its position there.
    pub fn track_of(&self, file_id: i64) -> Option<(&Album, usize)> {
        self.albums.iter().find_map(|a| a.tracks.iter().position(|t| t.file_id == file_id).map(|i| (a, i)))
    }
}

/// What's known about one folder of tracks.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FolderTags {
    pub tracks: Vec<(i64, Tags)>,
    pub cover: Option<String>,
}

/// A track while albums are being put together: its file, tags and folder art.
type Member<'a> = (&'a MediaFile, &'a Tags, Option<&'a str>);

/// Sorts tracks into albums and artists.
pub fn build(files: &[MediaFile], folders: &HashMap<String, FolderTags>) -> MusicCatalog {
    let by_id: HashMap<i64, &MediaFile> = files.iter().map(|f| (f.id, f)).collect();
    // (album dir, lowercased album tag) -> tracks with their tags
    let mut groups: HashMap<(String, String), Vec<Member>> = HashMap::new();
    for folder in folders.values() {
        for (id, tags) in &folder.tracks {
            let Some(file) = by_id.get(id) else { continue };
            let key = (album_dir(&file.rel_path).to_owned(), tags.album.as_deref().unwrap_or("").to_lowercase());
            let cover = folder.cover.as_deref().or_else(|| folders.get(&key.0).and_then(|f| f.cover.as_deref()));
            groups.entry(key).or_default().push((file, tags, cover));
        }
    }
    let mut albums: Vec<Album> = groups.into_iter().map(|((dir, _), tracks)| make_album(&dir, tracks)).collect();
    albums.sort_by(|a, b| natural_cmp(&a.artist, &b.artist).then_with(|| a.year.cmp(&b.year)).then_with(|| natural_cmp(&a.title, &b.title)));
    let tracks = albums.iter().map(|a| a.tracks.len()).sum();
    let artists = index_artists(&albums);
    MusicCatalog { albums, artists, tracks }
}

fn most_common<'a>(values: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for v in values {
        match counts.iter_mut().find(|(c, _)| c.eq_ignore_ascii_case(v)) {
            Some((_, n)) => *n += 1,
            None => counts.push((v, 1)),
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(v, _)| v)
}

fn make_album(dir: &str, mut tracks: Vec<Member>) -> Album {
    tracks.sort_by(|a, b| {
        (a.1.disc.unwrap_or(1), a.1.track.unwrap_or(u32::MAX))
            .cmp(&(b.1.disc.unwrap_or(1), b.1.track.unwrap_or(u32::MAX)))
            .then_with(|| natural_cmp(&a.0.rel_path, &b.0.rel_path))
    });
    let album_tag = tracks.iter().find_map(|t| t.1.album.clone());
    let folder_name = dir.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("Singles");
    let album_artist = most_common(tracks.iter().filter_map(|t| t.1.album_artist.as_deref())).map(str::to_owned);
    let track_artists: Vec<Vec<String>> = tracks.iter().map(|t| t.1.artist.as_deref().map(split_artists).unwrap_or_default()).collect();
    let artist = album_artist.clone().unwrap_or_else(|| {
        let firsts: Vec<&str> = track_artists.iter().filter_map(|a| a.first().map(String::as_str)).collect();
        match most_common(firsts.iter().copied()) {
            Some(a) if firsts.iter().all(|f| f.eq_ignore_ascii_case(a)) => a.to_owned(),
            Some(_) => VARIOUS.to_owned(),
            None => "Unknown artist".to_owned(),
        }
    });
    let mut artists: Vec<String> = Vec::new();
    let leading = album_artist.as_deref().map(split_artists).unwrap_or_else(|| if artist == VARIOUS { Vec::new() } else { split_artists(&artist) });
    for a in leading.into_iter().chain(track_artists.iter().flatten().cloned()) {
        if a != "Unknown artist" && !artists.iter().any(|x| x.eq_ignore_ascii_case(&a)) {
            artists.push(a);
        }
    }
    let singles = album_tag.is_none();
    let title = match &album_tag {
        Some(tag) => tag.clone(),
        None if folder_name.eq_ignore_ascii_case(&artist) => "Singles".to_owned(),
        None => crate::library::display_name(folder_name, false),
    };
    let year = tracks.iter().filter_map(|t| t.1.year).min();
    let cover = tracks.iter().find_map(|t| t.2).map(str::to_owned);
    let added_at = tracks.iter().map(|t| t.0.added_at).max().unwrap_or(0);
    let tracks = tracks
        .into_iter()
        .zip(track_artists)
        .map(|((file, tags, _), artists)| Track {
            file_id: file.id,
            title: tags.title.clone().unwrap_or_else(|| title_from_file(file.file_name())),
            artists,
            number: tags.track,
            disc: tags.disc,
            duration: tags.duration,
        })
        .collect();
    Album { key: format!("{dir}\u{1f}{}", album_tag.unwrap_or_default().to_lowercase()), title, artist, artists, year, tracks, cover, added_at, singles }
}

/// "01 - Attack.mp3" -> "Attack".
fn title_from_file(name: &str) -> String {
    let stem = crate::library::display_name(name, true);
    let rest = stem.trim_start_matches(|c: char| c.is_ascii_digit());
    let rest = rest.trim_start_matches([' ', '.', '-', '_']);
    if rest.is_empty() || rest.len() == stem.len() { stem.clone() } else { rest.to_owned() }
}

fn index_artists(albums: &[Album]) -> Vec<Artist> {
    let mut artists: Vec<Artist> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (i, album) in albums.iter().enumerate() {
        let leading = if album.artist == VARIOUS { Vec::new() } else { split_artists(&album.artist) };
        for name in &album.artists {
            let slot = *index.entry(name.to_lowercase()).or_insert_with(|| {
                artists.push(Artist { name: name.clone(), albums: Vec::new(), appears_on: Vec::new() });
                artists.len() - 1
            });
            let list = if leading.iter().any(|l| l.eq_ignore_ascii_case(name)) { &mut artists[slot].albums } else { &mut artists[slot].appears_on };
            list.push(i);
        }
    }
    let newest_first = |list: &mut Vec<usize>| list.sort_by(|a, b| albums[*b].year.cmp(&albums[*a].year).then_with(|| natural_cmp(&albums[*a].title, &albums[*b].title)));
    for a in &mut artists {
        newest_first(&mut a.albums);
        newest_first(&mut a.appears_on);
    }
    artists.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    artists
}

fn folder_fingerprint(files: &[&MediaFile]) -> String {
    use std::hash::{Hash, Hasher};
    let mut entries: Vec<(i64, &str, u64, i64)> = files.iter().map(|f| (f.id, f.rel_path.as_str(), f.size, f.mtime)).collect();
    entries.sort_unstable();
    let mut h = std::hash::DefaultHasher::new();
    (TAGS_VERSION, "music", entries).hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Art in `dir` (absolute), by [`COVER_NAMES`] order.
fn folder_cover(dir: &Path) -> Option<String> {
    let mut best: Option<(usize, std::path::PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let ext_ok = path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)));
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_lowercase) else { continue };
        if !ext_ok {
            continue;
        }
        let rank = COVER_NAMES.iter().position(|n| stem == *n || stem.starts_with(n)).unwrap_or(COVER_NAMES.len());
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, path));
        }
    }
    best.map(|(_, p)| p.to_string_lossy().into_owned())
}

/// Reads one folder's tags (and cover).
fn read_folder(library_id: i64, root: &Path, dir: &str, files: &[&MediaFile], covers_dir: &Path) -> FolderTags {
    let mut cover = folder_cover(&join_rel(root, dir));
    let mut tracks = Vec::with_capacity(files.len());
    for f in files {
        let (tags, picture) = read_tags(&f.abs_path(root), cover.is_none());
        if let Some(bytes) = picture {
            let path = covers_dir.join(format!("{library_id}-{}.img", f.id));
            if std::fs::create_dir_all(covers_dir).and_then(|()| std::fs::write(&path, bytes)).is_ok() {
                cover = Some(path.to_string_lossy().into_owned());
            }
        }
        tracks.push((f.id, tags));
    }
    FolderTags { tracks, cover }
}

/// Reads tags for a library's tracks, using what's cached for unchanged folders.
/// Embedded art is saved in `covers_dir` when a folder has no image of its own.
/// `progress(done, total)` counts folders.
pub fn read_library_cached(
    db: &mut crate::Db,
    library_id: i64,
    root: &Path,
    files: &[MediaFile],
    covers_dir: &Path,
    mut progress: impl FnMut(usize, usize),
) -> anyhow::Result<HashMap<String, FolderTags>> {
    let mut by_dir: HashMap<&str, Vec<&MediaFile>> = HashMap::new();
    for f in files {
        by_dir.entry(f.rel_path.rsplit_once('/').map_or("", |(d, _)| d)).or_default().push(f);
    }
    let mut cache = db.parse_cache(library_id)?;
    let mut out = HashMap::with_capacity(by_dir.len());
    let mut stale = Vec::new();
    for (dir, files) in &by_dir {
        let fingerprint = folder_fingerprint(files);
        let cached = cache
            .remove(*dir)
            .filter(|(f, _)| *f == fingerprint)
            .and_then(|(_, data)| serde_json::from_str::<FolderTags>(&data).ok());
        match cached {
            Some(tags) => {
                out.insert(dir.to_string(), tags);
            }
            None => stale.push((*dir, fingerprint, files)),
        }
    }
    // Reading is mostly waiting on the disk or network share, so several folders at once.
    let total = stale.len();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel::<(String, String, FolderTags)>();
    let mut fresh = Vec::new();
    let mut saved = 0;
    std::thread::scope(|scope| -> anyhow::Result<()> {
        for _ in 0..READERS.min(total) {
            let (tx, stale, next) = (tx.clone(), &stale, &next);
            scope.spawn(move || {
                while let Some((dir, fingerprint, files)) = stale.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                    let folder = read_folder(library_id, root, dir, files, covers_dir);
                    if tx.send((dir.to_string(), fingerprint.clone(), folder)).is_err() {
                        return;
                    }
                }
            });
        }
        drop(tx);
        progress(0, total);
        for (done, (dir, fingerprint, folder)) in rx.into_iter().enumerate() {
            fresh.push((dir.clone(), fingerprint, serde_json::to_string(&folder)?));
            out.insert(dir, folder);
            progress(done + 1, total);
            // Saved as it goes, so a first read that's interrupted needn't start over.
            if fresh.len() - saved >= SAVE_EVERY {
                db.save_parse_cache(library_id, &fresh[saved..], &[])?;
                saved = fresh.len();
            }
        }
        Ok(())
    })?;
    let fresh = &fresh[saved..];
    let gone: Vec<String> = cache.into_keys().collect();
    if !fresh.is_empty() || !gone.is_empty() {
        db.save_parse_cache(library_id, fresh, &gone)?;
    }
    log::info!("library {library_id}: read tags in {total} of {} folders ({} cached)", by_dir.len(), by_dir.len() - total);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: i64, rel_path: &str) -> MediaFile {
        MediaFile { id, library_id: 1, rel_path: rel_path.into(), ..MediaFile::default() }
    }

    fn tags(album: Option<&str>, artist: &str, track: u32) -> Tags {
        Tags { album: album.map(Into::into), artist: Some(artist.into()), track: Some(track), title: Some(format!("T{track}")), ..Tags::default() }
    }

    fn folders(entries: &[(&str, Vec<(i64, Tags)>)]) -> HashMap<String, FolderTags> {
        entries.iter().map(|(d, t)| (d.to_string(), FolderTags { tracks: t.clone(), cover: None })).collect()
    }

    #[test]
    fn disc_folders_belong_to_their_album() {
        assert_eq!(album_dir("lordskh/Disc 01/01 - x.flac"), "lordskh");
        assert_eq!(album_dir("A/B/CD2/x.mp3"), "A/B");
        assert_eq!(album_dir("A/Discovery/x.mp3"), "A/Discovery");
        assert_eq!(album_dir("x.mp3"), "");
    }

    #[test]
    fn splits_artists() {
        assert_eq!(split_artists("A; B feat. C"), ["A", "B", "C"]);
        assert_eq!(split_artists("Simon & Garfunkel"), ["Simon & Garfunkel"]);
        assert_eq!(split_artists("A / a"), ["A"]);
    }

    #[test]
    fn titles_from_file_names() {
        assert_eq!(title_from_file("01 - Attack.mp3"), "Attack");
        assert_eq!(title_from_file("02. 23.30.mp3"), "23.30");
        assert_eq!(title_from_file("1999.mp3"), "1999");
    }

    #[test]
    fn groups_by_album_tag_and_folder() {
        let files = [
            file(1, "SOAD/Hypnotize/02.mp3"),
            file(2, "SOAD/Hypnotize/01.mp3"),
            file(3, "lordskh/Disc 01/01.flac"),
            file(4, "lordskh/Disc 02/01.flac"),
            file(5, "ampache/Taramis/x.opus"),
        ];
        let mut d1 = tags(Some("Disc"), "Lord", 1);
        d1.disc = Some(1);
        let mut d2 = tags(Some("Disc"), "Lord", 1);
        d2.disc = Some(2);
        let catalog = build(
            &files,
            &folders(&[
                ("SOAD/Hypnotize", vec![(1, tags(Some("Hypnotize"), "System Of A Down", 2)), (2, tags(Some("Hypnotize"), "System Of A Down", 1))]),
                ("lordskh/Disc 01", vec![(3, d1)]),
                ("lordskh/Disc 02", vec![(4, d2)]),
                ("ampache/Taramis", vec![(5, Tags::default())]),
            ]),
        );
        assert_eq!(catalog.albums.len(), 3);
        assert_eq!(catalog.tracks, 5);
        let (_, hyp) = catalog.album("SOAD/Hypnotize\u{1f}hypnotize").unwrap();
        assert_eq!(hyp.artist, "System Of A Down");
        assert_eq!(hyp.tracks.iter().map(|t| t.file_id).collect::<Vec<_>>(), [2, 1]);
        let (_, disc) = catalog.album("lordskh\u{1f}disc").unwrap();
        assert_eq!(disc.tracks.iter().map(|t| t.file_id).collect::<Vec<_>>(), [3, 4]);
        let (_, loose) = catalog.album("ampache/Taramis\u{1f}").unwrap();
        assert!(loose.singles);
        assert_eq!(loose.title, "Taramis"); // no artist tag, so the folder names it
        assert_eq!(loose.tracks[0].title, "x");
        assert!(!hyp.singles);
        assert_eq!(catalog.artist("system of a down").unwrap().albums.len(), 1);
    }

    #[test]
    fn loose_tracks_in_an_artist_folder_are_singles() {
        let files = [file(1, "ampache/7DD9/a.opus"), file(2, "ampache/7DD9/b.opus")];
        let catalog = build(&files, &folders(&[("ampache/7DD9", vec![(1, tags(None, "7DD9", 1)), (2, tags(None, "7DD9", 2))])]));
        let album = &catalog.albums[0];
        assert!(album.singles);
        assert_eq!((album.title.as_str(), album.artist.as_str()), ("Singles", "7DD9"));
    }

    #[test]
    fn compilations_are_by_various_artists() {
        let files = [file(1, "Comp/a.mp3"), file(2, "Comp/b.mp3")];
        let catalog = build(&files, &folders(&[("Comp", vec![(1, tags(Some("Hits"), "A", 1)), (2, tags(Some("Hits"), "B feat. C", 2))])]));
        let album = &catalog.albums[0];
        assert_eq!(album.artist, "Various Artists");
        assert_eq!(album.artists, ["A", "B", "C"]);
        let a = catalog.artist("A").unwrap();
        assert!(a.albums.is_empty());
        assert_eq!(a.appears_on, [0]);
        assert!(catalog.artist("Various Artists").is_none());
    }
}
