//! Adding a library: pick a folder (drive letters on Windows), then pick its type.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui::{Rect, Ui};
use flex_core::LibraryKind;

use super::{Nav, Screen, text_entry::TextEntry};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::widgets::{self, Icon, ListInput, ListState, Row};

/// A folder (or drive) choice, with an optional note like "Network drive".
type Entry = (PathBuf, String);

fn list_subdirs(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut dirs = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name.starts_with('$') || name.starts_with('@') {
            continue;
        }
        let file_type = item.file_type()?;
        let is_dir = if file_type.is_symlink() { item.path().is_dir() } else { file_type.is_dir() };
        if is_dir {
            dirs.push((item.path(), String::new()));
        }
    }
    dirs.sort_by(|a, b| flex_core::natural::natural_cmp(&a.0.to_string_lossy(), &b.0.to_string_lossy()));
    Ok(dirs)
}

/// Drive letters, straight from Windows. Deliberately never touches the drives themselves:
/// probing a disconnected network drive can block for a long time.
#[cfg(windows)]
fn roots() -> Vec<Entry> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| {
            let root = format!("{}:\\", (b'A' + i) as char);
            let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
            // DRIVE_* values from winbase.h.
            let kind = match unsafe { GetDriveTypeW(wide.as_ptr()) } {
                2 => "Removable drive",
                3 => "Local disk",
                4 => "Network drive",
                5 => "CD/DVD drive",
                _ => "",
            };
            (PathBuf::from(root), kind.to_owned())
        })
        .collect()
}

#[cfg(not(windows))]
fn roots() -> Vec<Entry> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push((PathBuf::from(home), "Home".to_owned()));
    }
    roots.push((PathBuf::from("/"), String::new()));
    if Path::new("/Volumes").is_dir() {
        roots.push((PathBuf::from("/Volumes"), "Drives and shares".to_owned()));
    }
    roots
}

fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Folder picker. Each level is browsed in place; Back goes up a level, and from the top
/// level leaves the picker.
pub struct FolderPicker {
    /// `None` = top level (drives on Windows).
    dir: Option<PathBuf>,
    /// History of (folder, selected row) to restore when going back up.
    trail: Vec<(Option<PathBuf>, usize)>,
    dirs: Vec<Entry>,
    pending: Option<Receiver<std::io::Result<Vec<Entry>>>>,
    error: Option<String>,
    list: ListState,
}

impl FolderPicker {
    pub fn new(sh: &Shared) -> Self {
        let mut picker =
            Self { dir: None, trail: Vec::new(), dirs: Vec::new(), pending: None, error: None, list: ListState::default() };
        picker.open(sh, None, 0);
        picker
    }

    /// Lists `dir` (or the drives) on a background thread: drives and shares can be slow.
    fn open(&mut self, sh: &Shared, dir: Option<PathBuf>, select: usize) {
        self.dir = dir.clone();
        self.error = None;
        self.dirs.clear();
        self.list.select(select);
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = sh.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(dir.map_or_else(|| Ok(roots()), |d| list_subdirs(&d)));
            ctx.request_repaint();
        });
        self.pending = Some(rx);
    }

    fn poll(&mut self) {
        let Some(rx) = &self.pending else { return };
        match rx.try_recv() {
            Ok(Ok(dirs)) => self.dirs = dirs,
            Ok(Err(e)) => self.error = Some(format!("Couldn't open folder: {e}")),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {}
        }
        self.pending = None;
    }

    /// Row 0 is "Use this folder" when inside a folder.
    fn offset(&self) -> usize {
        usize::from(self.dir.is_some())
    }
}

impl Screen for FolderPicker {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        self.poll();
        if press.is(Action::Back) {
            return match self.trail.pop() {
                Some((dir, selected)) => {
                    self.open(sh, dir, selected);
                    Nav::None
                }
                None => Nav::Pop,
            };
        }
        match self.list.handle(press, self.dirs.len() + self.offset()) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(0) if self.dir.is_some() => {
                Nav::replace(KindPicker::new(self.dir.clone().unwrap_or_default()))
            }
            ListInput::Activate(i) => {
                let target = self.dirs[i - self.offset()].0.clone();
                self.trail.push((self.dir.clone(), i));
                self.open(sh, Some(target), 0);
                Nav::None
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, _sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.poll();
        let location = self.dir.as_ref().map_or("Choose a drive or folder".to_owned(), |d| d.display().to_string());
        let subtitle = self.error.clone().unwrap_or(location);
        let body = widgets::header(ui.painter(), rect, "Add library", &subtitle, ui.style());
        if self.pending.is_some() {
            widgets::message(ui.painter(), body, "Loading…", ui.style());
            return Nav::None;
        }
        let mut rows = Vec::new();
        if let Some(dir) = &self.dir {
            rows.push(Row::new(Icon::Add, format!("Use “{}” as a library", name_of(dir))));
        }
        rows.extend(self.dirs.iter().map(|(d, note)| {
            let name = if self.dir.is_none() { d.display().to_string() } else { name_of(d) };
            Row::new(Icon::Folder, name).detail(note.as_str())
        }));
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

pub struct KindPicker {
    root: PathBuf,
    list: ListState,
}

impl KindPicker {
    pub fn new(root: PathBuf) -> Self {
        Self { root, list: ListState::default() }
    }
}

fn kind_hint(kind: LibraryKind) -> &'static str {
    match kind {
        LibraryKind::Movies => "One film per file or folder",
        LibraryKind::Tv => "Shows with seasons and episodes",
        LibraryKind::Anime => "Anime series, matched with AniList",
    }
}

impl Screen for KindPicker {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        match self.list.handle(press, LibraryKind::ALL.len()) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) => match sh.add_library(&self.root, LibraryKind::ALL[i]) {
                Ok(id) => {
                    sh.toast(format!("Added {}, scanning…", self.root.display()));
                    Nav::replace(TextEntry::rename_library(sh, id, "Name this library"))
                }
                Err(e) => {
                    sh.toast(format!("{e:#}"));
                    Nav::None
                }
            },
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, _sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let subtitle = format!("What's in {}?", self.root.display());
        let body = widgets::header(ui.painter(), rect, "Library type", &subtitle, ui.style());
        let rows: Vec<Row> =
            LibraryKind::ALL.iter().map(|k| Row::new(Icon::None, k.label()).detail(kind_hint(*k))).collect();
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
