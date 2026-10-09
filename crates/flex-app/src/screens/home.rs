//! Home: a Continue Watching row (when there's anything to continue) above a grid of
//! library cards.

use eframe::egui::{Align2, Color32, Pos2, Rect, Style, TextStyle, Ui, pos2, vec2};
use flex_core::library::display_name;
use flex_core::progress::{ContinueItem, ContinueKind};

use super::options::{OptionsMenu, Target};
use super::{Nav, Screen, library::LibraryView, picker::FolderPicker};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::{theme, widgets};

const LIBRARY_CARD: [f32; 2] = [520.0, 240.0];
const CONTINUE_CARD: [f32; 2] = [520.0, 200.0];
const GAP: f32 = 28.0;
const SECTION_GAP: f32 = 48.0;
const MAX_CONTINUE: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Continue,
    Libraries,
}

/// Text for a Continue Watching card.
struct CardText {
    show: String,
    episode: String,
    status: String,
    fraction: Option<f64>,
}

pub struct Home {
    section: Section,
    /// Selected index in each section.
    cont: usize,
    lib: usize,
    /// First visible Continue Watching card (the row scrolls sideways).
    cont_offset: usize,
    lib_columns: usize,
    items: Vec<ContinueItem>,
    /// Library versions the items were computed for.
    items_key: Option<Vec<(i64, u64)>>,
}

impl Default for Home {
    fn default() -> Self {
        Self {
            section: Section::Continue,
            cont: 0,
            lib: 0,
            cont_offset: 0,
            lib_columns: 1,
            items: Vec::new(),
            items_key: None,
        }
    }
}

fn describe(sh: &Shared, item: &ContinueItem) -> Option<CardText> {
    let file = sh.lib(item.library_id)?.tree.file(item.file_id)?;
    let episode = display_name(file.file_name(), true);
    let show = match file.rel_path.split_once('/') {
        Some((top, _)) => display_name(top, false),
        None => episode.clone(), // a movie at the library root
    };
    let episode = if show == episode { String::new() } else { episode };
    let (status, fraction) = match item.kind {
        ContinueKind::Resume => {
            let left = file.progress.remaining_secs().map_or_else(String::new, theme::format_remaining);
            (left, file.progress.fraction())
        }
        ContinueKind::Next => ("Up next".to_owned(), None),
    };
    Some(CardText { show, episode, status, fraction })
}

fn continue_card(ui: &Ui, rect: Rect, text: &CardText, focused: bool, style: &Style) {
    let painter = ui.painter();
    let color = widgets::paint_focusable(painter, rect, focused, false);
    let clip = painter.with_clip_rect(rect.shrink(8.0));
    let inner = rect.shrink(24.0);
    clip.text(inner.left_top(), Align2::LEFT_TOP, &text.show, TextStyle::Body.resolve(style), color);
    clip.text(inner.left_top() + vec2(0.0, 44.0), Align2::LEFT_TOP, &text.episode, TextStyle::Small.resolve(style), color);
    clip.text(inner.left_bottom() - vec2(0.0, 8.0), Align2::LEFT_BOTTOM, &text.status, TextStyle::Small.resolve(style), color);
    if let Some(f) = text.fraction {
        widgets::paint_progress(painter, rect, f as f32, if focused { Color32::BLACK } else { theme::FOCUS });
    }
}

impl Home {
    fn refresh(&mut self, sh: &Shared) {
        let key: Vec<(i64, u64)> = sh.libs.iter().map(|l| (l.lib.id, l.version)).collect();
        if self.items_key.as_ref() != Some(&key) {
            let was_empty = self.items.is_empty();
            self.items = sh.continue_watching(MAX_CONTINUE);
            self.items_key = Some(key);
            // Something new to continue: that's most likely what you want next.
            if was_empty && !self.items.is_empty() {
                self.section = Section::Continue;
                self.cont = 0;
            }
        }
        if self.items.is_empty() {
            self.section = Section::Libraries;
        }
        self.cont = self.cont.min(self.items.len().saturating_sub(1));
        self.lib = self.lib.min(sh.libs.len().saturating_sub(1));
    }

    fn handle_continue(&mut self, sh: &Shared, press: Press) -> Nav {
        let item = self.items.get(self.cont).copied();
        match press.action {
            Action::Left => self.cont = self.cont.saturating_sub(1),
            Action::Right => self.cont = (self.cont + 1).min(self.items.len().saturating_sub(1)),
            Action::Up if !press.repeat => return Nav::FocusTopBar,
            Action::Down => self.section = Section::Libraries,
            Action::Menu if !press.repeat => {
                if let Some(i) = item {
                    return Nav::push(OptionsMenu::new(Target::File { library_id: i.library_id, file_id: i.file_id }));
                }
            }
            _ if press.is_activate() => {
                if let Some(i) = item {
                    return Nav::play(sh.play_request(i.library_id, i.file_id));
                }
            }
            _ => {}
        }
        Nav::None
    }

    fn handle_libraries(&mut self, sh: &Shared, press: Press) -> Nav {
        let count = sh.libs.len().max(1); // an "Add library" card when empty
        let cols = self.lib_columns.max(1);
        match press.action {
            Action::Left => self.lib = self.lib.saturating_sub(1),
            Action::Right => self.lib = (self.lib + 1).min(count - 1),
            Action::Up if self.lib < cols => {
                if press.repeat {
                    return Nav::None;
                }
                if self.items.is_empty() {
                    return Nav::FocusTopBar;
                }
                self.section = Section::Continue;
            }
            Action::Up => self.lib -= cols,
            Action::Down if self.lib + cols < count => self.lib += cols,
            _ if press.is_activate() => {
                return match sh.libs.get(self.lib) {
                    Some(l) => Nav::push(LibraryView::new(l.lib.id, String::new())),
                    None => Nav::push(FolderPicker::new(sh)),
                };
            }
            _ => {}
        }
        Nav::None
    }
}

impl Screen for Home {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        self.refresh(sh);
        if press.is(Action::Back) {
            sh.toast("Hold Back to quit");
            return Nav::None;
        }
        match self.section {
            Section::Continue => self.handle_continue(sh, press),
            Section::Libraries => self.handle_libraries(sh, press),
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let style = ui.style().clone();
        let mut top = rect.top();

        if !self.items.is_empty() {
            let area = Rect::from_min_max(pos2(rect.left(), top), rect.max);
            let body = widgets::header(ui.painter(), area, "Continue Watching", "", &style);
            let visible = (((body.width() + GAP) / (CONTINUE_CARD[0] + GAP)) as usize).max(1);
            // Scroll sideways to keep the selected card in view.
            if self.cont < self.cont_offset {
                self.cont_offset = self.cont;
            } else if self.cont >= self.cont_offset + visible {
                self.cont_offset = self.cont + 1 - visible;
            }
            for (slot, i) in (self.cont_offset..self.items.len()).take(visible).enumerate() {
                let Some(text) = describe(sh, &self.items[i]) else { continue };
                let min = Pos2::new(body.left() + slot as f32 * (CONTINUE_CARD[0] + GAP), body.top());
                let selected = focused && self.section == Section::Continue && i == self.cont;
                continue_card(ui, Rect::from_min_size(min, CONTINUE_CARD.into()), &text, selected, &style);
            }
            top = body.top() + CONTINUE_CARD[1] + SECTION_GAP;
        }

        let painter = ui.painter();
        let area = Rect::from_min_max(pos2(rect.left(), top), rect.max);
        let body = widgets::header(painter, area, "Libraries", "", &style);
        self.lib_columns = (((body.width() + GAP) / (LIBRARY_CARD[0] + GAP)) as usize).max(1);
        let card_rect = |i: usize| {
            let (row, col) = (i / self.lib_columns, i % self.lib_columns);
            let min = pos2(body.left() + col as f32 * (LIBRARY_CARD[0] + GAP), body.top() + row as f32 * (LIBRARY_CARD[1] + GAP));
            Rect::from_min_size(min, LIBRARY_CARD.into())
        };
        let libs_focused = focused && self.section == Section::Libraries;
        if sh.libs.is_empty() {
            widgets::card(painter, card_rect(0), "Add library", &["Pick a folder to get started"], libs_focused, &style);
        }
        for (i, l) in sh.libs.iter().enumerate() {
            let status = l.status();
            let lines = [l.lib.kind.label(), status.as_str()];
            widgets::card(painter, card_rect(i), &l.lib.name, &lines, libs_focused && i == self.lib, &style);
        }
        Nav::None
    }
}
