//! D-pad driven folder browser. Lists folders and video files; on Windows the level
//! above a drive root is a list of drive letters.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui::{self, Align, Color32, CornerRadius, Sense, Stroke, StrokeKind, vec2};

use crate::input::{Action, Press};
use crate::theme;

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "wmv", "webm", "ts", "m2ts", "mts", "mpg", "mpeg", "vob", "ogv",
    "flv", "3gp", "rmvb", "divx",
];

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.iter().any(|v| v.eq_ignore_ascii_case(e)))
}

#[derive(Clone)]
struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

/// Compares strings so that "Ep 2" sorts before "Ep 10".
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut n = String::new();
                    while let Some(c) = it.next_if(char::is_ascii_digit) {
                        n.push(c);
                    }
                    n
                };
                let (na, nb) = (take(&mut a), take(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn list_dir(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.starts_with('$') {
            continue;
        }
        let path = item.path();
        // file_type() comes free with the listing; only symlinks need an extra (slow on SMB) stat.
        let file_type = item.file_type()?;
        let is_dir = if file_type.is_symlink() {
            std::fs::metadata(&path).is_ok_and(|m| m.is_dir())
        } else {
            file_type.is_dir()
        };
        if is_dir || is_video(&path) {
            entries.push(Entry { name, path, is_dir });
        }
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| natural_cmp(&a.name, &b.name)));
    Ok(entries)
}

#[cfg(windows)]
fn list_drives() -> Vec<Entry> {
    (b'A'..=b'Z')
        .map(|l| PathBuf::from(format!("{}:\\", l as char)))
        .filter(|p| p.exists())
        .map(|path| Entry { name: path.display().to_string(), path, is_dir: true })
        .collect()
}

#[cfg(not(windows))]
fn list_drives() -> Vec<Entry> {
    vec![Entry { name: "/".into(), path: "/".into(), is_dir: true }]
}

/// Folder or play-triangle icon centred on `c`, drawn with shapes so it doesn't depend on font glyphs.
fn paint_icon(painter: &egui::Painter, c: egui::Pos2, is_dir: bool, color: Color32) {
    if is_dir {
        let tab = egui::Rect::from_min_size(c + vec2(-14.0, -13.0), vec2(12.0, 6.0));
        let body = egui::Rect::from_min_size(c + vec2(-14.0, -9.0), vec2(28.0, 20.0));
        painter.rect_filled(tab, CornerRadius::same(2), color);
        painter.rect_filled(body, CornerRadius::same(3), color);
    } else {
        let points = vec![c + vec2(-9.0, -12.0), c + vec2(13.0, 0.0), c + vec2(-9.0, 12.0)];
        painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
    }
}

pub struct Browser {
    /// `None` = the drive list.
    dir: Option<PathBuf>,
    entries: Vec<Entry>,
    selected: usize,
    /// Entry to select once the pending listing arrives (used when going up a level).
    select_after_load: Option<PathBuf>,
    pending: Option<Receiver<std::io::Result<Vec<Entry>>>>,
    error: Option<String>,
    scroll_pending: bool,
}

impl Browser {
    pub fn new(ctx: &egui::Context, start: Option<PathBuf>) -> Self {
        let mut b = Self {
            dir: None,
            entries: Vec::new(),
            selected: 0,
            select_after_load: None,
            pending: None,
            error: None,
            scroll_pending: true,
        };
        b.open(ctx, start);
        b
    }

    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    fn open(&mut self, ctx: &egui::Context, dir: Option<PathBuf>) {
        self.dir = dir.clone();
        self.entries.clear();
        self.selected = 0;
        self.scroll_pending = true;
        let Some(dir) = dir else {
            self.entries = list_drives();
            self.pending = None;
            self.apply_select_after_load();
            return;
        };
        // Listing a network share can be slow; don't block the UI on it.
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(list_dir(&dir));
            ctx.request_repaint();
        });
        self.pending = Some(rx);
    }

    fn apply_select_after_load(&mut self) {
        if let Some(target) = self.select_after_load.take() {
            self.selected = self.entries.iter().position(|e| e.path == target).unwrap_or(0);
        }
    }

    fn poll(&mut self) {
        let Some(rx) = &self.pending else { return };
        match rx.try_recv() {
            Ok(Ok(entries)) => {
                self.entries = entries;
                self.pending = None;
                self.apply_select_after_load();
                self.scroll_pending = true;
            }
            Ok(Err(e)) => {
                self.error = Some(format!("Couldn't open folder: {e}"));
                self.pending = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.pending = None,
        }
    }

    /// Handles a press; returns a video path when the user picks one to play.
    pub fn handle(&mut self, ctx: &egui::Context, press: Press) -> Option<PathBuf> {
        const PAGE: usize = 8;
        let last = self.entries.len().saturating_sub(1);
        let before = self.selected;
        match press.action {
            Action::Up => self.selected = self.selected.saturating_sub(1),
            Action::Down => self.selected = (self.selected + 1).min(last),
            Action::Left => self.selected = self.selected.saturating_sub(PAGE),
            Action::Right => self.selected = (self.selected + PAGE).min(last),
            Action::Ok if press.repeat => {}
            Action::Ok => {
                self.error = None;
                let entry = self.entries.get(self.selected)?.clone();
                if entry.is_dir {
                    self.open(ctx, Some(entry.path));
                } else {
                    return Some(entry.path);
                }
            }
            Action::Back if press.repeat => {}
            Action::Back => {
                self.error = None;
                if let Some(dir) = self.dir.clone() {
                    let parent = dir.parent().map(Path::to_path_buf);
                    if parent.is_some() || cfg!(windows) {
                        self.select_after_load = Some(dir);
                        self.open(ctx, parent);
                    }
                }
            }
        }
        self.scroll_pending |= before != self.selected;
        None
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, status_line: Option<&str>) {
        self.poll();
        let safe = theme::safe_rect(ui.ctx());
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(safe));

        let title = self.dir.as_ref().map_or("This PC".to_owned(), |d| d.display().to_string());
        ui.label(egui::RichText::new(title).heading());
        if let Some(err) = &self.error {
            ui.colored_label(Color32::from_rgb(255, 110, 110), err);
        }
        ui.add_space(16.0);

        let footer_h = 40.0;
        let list_h = ui.available_height() - footer_h;
        if self.pending.is_some() {
            ui.label("Loading…");
        } else if self.entries.is_empty() {
            ui.label(egui::RichText::new("No folders or videos here").color(theme::DIM));
        }
        egui::ScrollArea::vertical()
            .max_height(list_h)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(&mut ui, |ui| {
                for (i, entry) in self.entries.iter().enumerate() {
                    let focused = i == self.selected;
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
                    let radius = CornerRadius::same(8);
                    ui.painter().rect_filled(rect, radius, if focused { theme::FOCUS } else { theme::ROW });
                    if focused {
                        ui.painter().rect_stroke(rect, radius, Stroke::new(3.0, Color32::WHITE), StrokeKind::Outside);
                    }
                    let color = if focused { Color32::BLACK } else { theme::TEXT };
                    paint_icon(ui.painter(), rect.left_center() + vec2(36.0, 0.0), entry.is_dir, color);
                    // Clip long names to the row.
                    ui.painter().with_clip_rect(rect.shrink(12.0)).text(
                        rect.left_center() + vec2(68.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        &entry.name,
                        egui::TextStyle::Body.resolve(ui.style()),
                        color,
                    );
                    if focused && self.scroll_pending {
                        ui.scroll_to_rect(rect, Some(Align::Center));
                    }
                    ui.add_space(6.0);
                }
            });
        self.scroll_pending = false;

        if let Some(status) = status_line {
            let footer = egui::Rect::from_min_max(safe.left_bottom() - vec2(0.0, footer_h), safe.right_bottom());
            ui.painter().text(
                footer.left_center(),
                egui::Align2::LEFT_CENTER,
                status,
                egui::TextStyle::Small.resolve(ui.style()),
                theme::DIM,
            );
        }
    }
}
