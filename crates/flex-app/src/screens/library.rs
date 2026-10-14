//! Folder view of one library. Each folder level is its own screen, so Back returns to
//! the parent with the same item still selected.

use eframe::egui::{Rect, Ui};
use flex_core::library::display_name;
use flex_core::tree::Node;

use super::options::{OptionsMenu, Target};
use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::{Shared, thousands};
use crate::theme;
use crate::widgets::{self, Icon, ListInput, ListState, Row};

pub struct LibraryView {
    library_id: i64,
    /// Folder shown, relative to the library root ("" = root).
    dir: String,
    nodes: Vec<Node>,
    version: Option<u64>,
    list: ListState,
}

impl LibraryView {
    pub fn new(library_id: i64, dir: String) -> Self {
        Self { library_id, dir, nodes: Vec::new(), version: None, list: ListState::default() }
    }

    fn refresh(&mut self, sh: &Shared) {
        let Some(state) = sh.lib(self.library_id) else { return };
        if self.version != Some(state.version) {
            self.nodes = state.tree.children(&self.dir);
            self.version = Some(state.version);
        }
    }
}

/// Opens a folder: plays it directly if it holds a single video (a movie in its own folder).
pub fn open_folder(sh: &Shared, library_id: i64, rel: &str) -> Nav {
    let single = sh.lib(library_id).filter(|l| !l.is_music()).and_then(|l| l.tree.single_video_in(rel)).map(|f| f.id);
    match single {
        Some(file_id) => Nav::play(sh.play_request(library_id, file_id)),
        None => Nav::push(LibraryView::new(library_id, rel.to_owned())),
    }
}

/// A video row: dimmed with "Watched" once watched, time left and a bar if partway.
/// A video row titled `title` (e.g. the episode's name from TMDB).
pub fn file_row_titled(f: &flex_core::MediaFile, title: String) -> Row {
    let p = f.progress;
    let row = Row::new(Icon::Video, title);
    match p.remaining_secs() {
        Some(left) => row.detail(theme::format_remaining(left)).progress(p.fraction()),
        None if p.watched => row.detail("Watched").dim(true),
        None => row,
    }
}

impl Screen for LibraryView {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        self.refresh(sh);
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        if press.is(Action::Menu) {
            let library_id = self.library_id;
            return match self.nodes.get(self.list.selected) {
                Some(Node::Dir { rel, .. }) => Nav::push(OptionsMenu::new(Target::Folder { library_id, rel: rel.clone() })),
                Some(Node::File(f)) => Nav::push(OptionsMenu::new(Target::File { library_id, file_id: f.id })),
                None => Nav::None,
            };
        }
        match self.list.handle(press, self.nodes.len()) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) => match &self.nodes[i] {
                Node::Dir { rel, .. } => open_folder(sh, self.library_id, rel),
                Node::File(f) if sh.lib(self.library_id).is_some_and(|l| l.is_music()) => {
                    super::music::play_file(sh, self.library_id, f.id)
                }
                Node::File(f) => Nav::play(sh.play_request(self.library_id, f.id)),
            },
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let Some(state) = sh.lib(self.library_id) else { return Nav::Pop }; // library removed
        let title = match self.dir.rsplit_once('/') {
            Some((_, name)) => name,
            None if self.dir.is_empty() => &state.lib.name,
            None => &self.dir,
        };
        let subtitle = if self.dir.is_empty() {
            format!("{} · {}", state.lib.kind.label(), state.status())
        } else {
            format!("{} › {}", state.lib.name, self.dir.replace('/', " › "))
        };
        let body = widgets::header(ui.painter(), rect, title, &subtitle, ui.style());

        if self.nodes.is_empty() {
            let msg = if state.scanning.is_some() { "Scanning… videos will appear when it finishes" } else { "No videos here" };
            widgets::message(ui.painter(), body, msg, ui.style());
            return Nav::None;
        }
        let rows: Vec<Row> = self
            .nodes
            .iter()
            .map(|n| match n {
                Node::Dir { name, videos, unwatched, .. } => {
                    let count = match videos {
                        1 => "1 video".to_owned(),
                        n => format!("{} videos", thousands(*n)),
                    };
                    let detail = match unwatched {
                        0 => "All watched".to_owned(),
                        u if u == videos => count,
                        u => format!("{count} · {} unwatched", thousands(*u)),
                    };
                    Row::new(Icon::Folder, name.as_str()).detail(detail).dim(*unwatched == 0)
                }
                // Clean names in the folder view too, with the file name as a fallback.
                Node::File(f) => {
                    let title = sh.titles(self.library_id, f.id).map(|t| t.episode.unwrap_or(t.name));
                    file_row_titled(f, title.unwrap_or_else(|| display_name(f.file_name(), true)))
                }
            })
            .collect();
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
