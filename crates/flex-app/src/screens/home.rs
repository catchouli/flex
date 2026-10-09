//! Home: a grid of library cards.

use eframe::egui::{Rect, Ui, pos2, vec2};

use super::{Nav, Screen, library::LibraryView, picker::FolderPicker};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::widgets;

const CARD: [f32; 2] = [520.0, 240.0];
const GAP: f32 = 28.0;

#[derive(Default)]
pub struct Home {
    selected: usize,
    columns: usize,
}

impl Screen for Home {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        let count = sh.libs.len().max(1); // an "Add library" card when empty
        let cols = self.columns.max(1);
        match press.action {
            Action::Left => self.selected = self.selected.saturating_sub(1),
            Action::Right => self.selected = (self.selected + 1).min(count - 1),
            Action::Up if self.selected < cols => return if press.repeat { Nav::None } else { Nav::FocusTopBar },
            Action::Up => self.selected -= cols,
            Action::Down => self.selected = (self.selected + cols).min(count - 1),
            Action::Back if !press.repeat => sh.toast("Hold Back to quit"),
            _ if press.is_activate() => {
                return match sh.libs.get(self.selected) {
                    Some(l) => Nav::push(LibraryView::new(l.lib.id, String::new())),
                    None => Nav::push(FolderPicker::new()),
                };
            }
            _ => {}
        }
        Nav::None
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let painter = ui.painter();
        let style = ui.style();
        let body = widgets::header(painter, rect, "Libraries", "", style);
        self.columns = (((body.width() + GAP) / (CARD[0] + GAP)) as usize).max(1);
        self.selected = self.selected.min(sh.libs.len().saturating_sub(1));

        let card_rect = |i: usize| {
            let (row, col) = (i / self.columns, i % self.columns);
            Rect::from_min_size(
                pos2(body.left() + col as f32 * (CARD[0] + GAP), body.top() + row as f32 * (CARD[1] + GAP)),
                vec2(CARD[0], CARD[1]),
            )
        };
        if sh.libs.is_empty() {
            widgets::card(painter, card_rect(0), "Add library", &["Pick a folder to get started"], focused, style);
        }
        for (i, l) in sh.libs.iter().enumerate() {
            let status = l.status();
            widgets::card(painter, card_rect(i), &l.lib.name, &[l.lib.kind.label(), &status], focused && i == self.selected, style);
        }
        Nav::None
    }
}
