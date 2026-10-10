//! Disc images (`.iso`): telling DVDs from Blu-rays, and finding a DVD's main feature for
//! players without DVD support.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::Path;

/// Which kind of disc an image holds, which decides how it's played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscImage {
    Bluray,
    Dvd,
}

/// How far into an image to look for its top-level folder names.
const SNIFF_BYTES: u64 = 8 << 20;

/// For a disc image (`.iso`), whether it's a DVD or a Blu-ray; `None` for anything else.
/// A DVD's `VIDEO_TS` (or a Blu-ray's `BDMV`) folder name appears in the directory records
/// near the start of the image, as plain bytes or UTF-16. Unrecognised images are assumed
/// to be Blu-rays.
pub fn disc_image(path: &Path) -> Option<DiscImage> {
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("iso")) {
        return None;
    }
    let Ok(file) = File::open(path) else {
        return Some(DiscImage::Bluray);
    };
    let utf16 = |s: &str| s.bytes().flat_map(|b| [0, b]).collect::<Vec<u8>>();
    let dvd = [b"VIDEO_TS".to_vec(), utf16("VIDEO_TS")];
    let bluray = [b"BDMV".to_vec(), utf16("BDMV")];
    let found = |window: &[u8], names: &[Vec<u8>]| names.iter().any(|n| window.windows(n.len()).any(|w| w == n));
    let mut reader = file.take(SNIFF_BYTES);
    let mut buf = vec![0; 1 << 16];
    // Carry the end of each chunk over, so a name split across chunks is still found.
    let mut keep = 0;
    loop {
        let n = match reader.read(&mut buf[keep..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let end = keep + n;
        if found(&buf[..end], &dvd) {
            return Some(DiscImage::Dvd);
        }
        if found(&buf[..end], &bluray) {
            return Some(DiscImage::Bluray);
        }
        keep = 15.min(end);
        buf.copy_within(end - keep..end, 0);
    }
    Some(DiscImage::Bluray)
}

const SECTOR: u64 = 2048;

/// The bytes of a DVD image holding its main feature: the VOBs of the largest title set
/// (`VTS_nn_1.VOB`, `VTS_nn_2.VOB`, ...), which are stored back to back as one MPEG stream.
/// `None` if the image's ISO 9660 directory can't be read or the VOBs aren't contiguous.
pub fn dvd_main_title(path: &Path) -> Option<Range<u64>> {
    let mut file = File::open(path).ok()?;
    let pvd = read_at(&mut file, 16 * SECTOR, SECTOR)?;
    if &pvd[1..6] != b"CD001" {
        return None;
    }
    let root = record(&pvd[156..190])?;
    let video_ts = dir_entries(&mut file, root.extent, root.len)?
        .into_iter()
        .find(|e| e.dir && e.name.eq_ignore_ascii_case("VIDEO_TS"))?;
    // Title set number -> its VOBs as (part, start byte, length).
    let mut sets: BTreeMap<u32, Vec<(u32, u64, u64)>> = BTreeMap::new();
    for e in dir_entries(&mut file, video_ts.extent, video_ts.len)? {
        let upper = e.name.to_ascii_uppercase();
        let Some(rest) = upper.strip_prefix("VTS_").and_then(|r| r.strip_suffix(".VOB")) else { continue };
        let Some((set, part)) = rest.split_once('_') else { continue };
        let (Ok(set), Ok(part)) = (set.parse::<u32>(), part.parse::<u32>()) else { continue };
        // Part 0 is the title set's menu.
        if part == 0 {
            continue;
        }
        sets.entry(set).or_default().push((part, e.extent * SECTOR, e.len));
    }
    let mut vobs = sets.into_values().max_by_key(|vobs| vobs.iter().map(|v| v.2).sum::<u64>())?;
    vobs.sort();
    let contiguous = vobs.windows(2).all(|w| w[0].1 + w[0].2 == w[1].1);
    let (first, last) = (vobs.first()?, vobs.last()?);
    contiguous.then(|| first.1..last.1 + last.2)
}

struct Entry {
    name: String,
    extent: u64,
    len: u64,
    dir: bool,
}

fn read_at(file: &mut File, offset: u64, len: u64) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = vec![0; usize::try_from(len).ok()?];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// Parses one ISO 9660 directory record.
fn record(r: &[u8]) -> Option<Entry> {
    let u32_at = |i: usize| r.get(i..i + 4).map(|b| u64::from(u32::from_le_bytes(b.try_into().unwrap())));
    let name_len = usize::from(*r.get(32)?);
    let name = r.get(33..33 + name_len)?;
    // "NAME.EXT;1": drop the version.
    let name = String::from_utf8_lossy(name);
    let name = name.split(';').next().unwrap_or_default().to_owned();
    Some(Entry { name, extent: u32_at(2)?, len: u32_at(10)?, dir: r.get(25)? & 2 != 0 })
}

/// The records of the directory at sector `extent`. Records never cross a sector boundary;
/// a zero length byte means the rest of the sector is padding.
fn dir_entries(file: &mut File, extent: u64, len: u64) -> Option<Vec<Entry>> {
    let data = read_at(file, extent * SECTOR, len.min(1 << 20))?;
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let n = usize::from(data[i]);
        if n == 0 {
            i = (i / SECTOR as usize + 1) * SECTOR as usize;
            continue;
        }
        out.extend(data.get(i..i + n).and_then(record));
        i += n;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("flex-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn disc_images() {
        let dir = temp_dir("disc");
        let image = |name: &str, marker: &[u8]| {
            // The marker straddles a read-chunk boundary.
            let mut bytes = vec![0u8; (1 << 16) - 3];
            bytes.extend_from_slice(marker);
            bytes.resize(1 << 18, 0);
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            disc_image(&path)
        };
        assert_eq!(image("dvd.ISO", b"VIDEO_TS"), Some(DiscImage::Dvd));
        assert_eq!(image("bd.iso", b"BDMV"), Some(DiscImage::Bluray));
        assert_eq!(image("bd16.iso", b"\0B\0D\0M\0V"), Some(DiscImage::Bluray));
        assert_eq!(image("dvd16.iso", b"\0V\0I\0D\0E\0O\0_\0T\0S"), Some(DiscImage::Dvd));
        assert_eq!(image("blank.iso", b""), Some(DiscImage::Bluray));
        assert_eq!(disc_image(Path::new("film.mkv")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A minimal ISO 9660 image: root -> VIDEO_TS -> the given (name, sector, length) files.
    fn iso(files: &[(&str, u32, u32)]) -> Vec<u8> {
        fn rec(name: &[u8], extent: u32, len: u32, dir: bool) -> Vec<u8> {
            let mut r = vec![0u8; 33];
            r[2..6].copy_from_slice(&extent.to_le_bytes());
            r[10..14].copy_from_slice(&len.to_le_bytes());
            r[25] = if dir { 2 } else { 0 };
            r[32] = name.len() as u8;
            r.extend_from_slice(name);
            if r.len() % 2 == 1 {
                r.push(0);
            }
            r[0] = r.len() as u8;
            r
        }
        let sector = SECTOR as usize;
        let mut img = vec![0u8; 20 * sector];
        img[16 * sector + 1..16 * sector + 6].copy_from_slice(b"CD001");
        let root = rec(b"\0", 18, SECTOR as u32, true);
        img[16 * sector + 156..16 * sector + 156 + root.len()].copy_from_slice(&root);
        let entry = rec(b"VIDEO_TS", 19, SECTOR as u32, true);
        img[18 * sector..18 * sector + entry.len()].copy_from_slice(&entry);
        let mut at = 19 * sector;
        for &(name, extent, len) in files {
            let r = rec(format!("{name};1").as_bytes(), extent, len, false);
            img[at..at + r.len()].copy_from_slice(&r);
            at += r.len();
        }
        img
    }

    #[test]
    fn dvd_main_titles() {
        let dir = temp_dir("dvd");
        let main = |name: &str, files: &[(&str, u32, u32)]| {
            let path = dir.join(name);
            std::fs::write(&path, iso(files)).unwrap();
            dvd_main_title(&path)
        };
        let s = SECTOR;
        let files = [
            ("VIDEO_TS.VOB", 100, 4096),
            ("VTS_01_0.VOB", 102, 2048),
            ("VTS_01_1.VOB", 103, 2048),
            ("VTS_02_0.VOB", 104, 2048),
            ("VTS_02_1.VOB", 105, 4096),
            ("VTS_02_2.VOB", 107, 2048),
        ];
        // Title set 2 is the largest.
        assert_eq!(main("a.iso", &files), Some(105 * s..108 * s));
        // A gap between its VOBs.
        assert_eq!(main("b.iso", &[("VTS_01_1.VOB", 100, 2048), ("VTS_01_2.VOB", 102, 2048)]), None);
        assert_eq!(main("c.iso", &[("VIDEO_TS.VOB", 100, 2048)]), None);
        std::fs::write(dir.join("d.iso"), vec![0u8; 40 * s as usize]).unwrap();
        assert_eq!(dvd_main_title(&dir.join("d.iso")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
