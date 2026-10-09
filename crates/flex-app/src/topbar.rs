//! The bar across the top: app name, search box, fullscreen toggle and Settings.
//!
//! With the remote: Up from the top of a screen focuses it, Left/Right moves between items,
//! OK on the search box starts typing (needs a keyboard), Down returns to the screen.
//! With a keyboard: just start typing anywhere to search.

use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, Stroke, StrokeKind, TextStyle, Ui, pos2, vec2};

use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::{theme, widgets};

pub const HEIGHT: f32 = 72.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Search,
    Fullscreen,
    Settings,
}

const ITEMS: [Item; 3] = [Item::Search, Item::Fullscreen, Item::Settings];

pub enum Event {
    None,
    /// Return input focus to the screen below.
    FocusContent,
    /// Enter in the search box.
    Submit,
    /// Escape in the search box.
    Cancel,
    OpenSettings,
    ToggleFullscreen,
}

#[derive(Default)]
pub struct TopBar {
    /// Which item has d-pad focus; `None` = the screen has it.
    pub focus: Option<Item>,
}

fn search_id() -> egui::Id {
    egui::Id::new("flex-search")
}

/// True while the search box is accepting typed text.
pub fn text_focused(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.has_focus(search_id()))
}

pub fn focus_text(ctx: &egui::Context) {
    ctx.memory_mut(|m| m.request_focus(search_id()));
}

/// Whether printable text was typed this frame (to start a search by just typing).
pub fn typed_text(ctx: &egui::Context) -> bool {
    ctx.input(|i| {
        !i.modifiers.command
            && i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if !t.trim().is_empty()))
    })
}

impl TopBar {
    pub fn handle(&mut self, ctx: &egui::Context, press: Press) -> Event {
        let i = ITEMS.iter().position(|it| Some(*it) == self.focus).unwrap_or(0);
        match press.action {
            Action::Left => self.focus = Some(ITEMS[i.saturating_sub(1)]),
            Action::Right => self.focus = Some(ITEMS[(i + 1).min(ITEMS.len() - 1)]),
            Action::Down | Action::Back => {
                self.focus = None;
                return Event::FocusContent;
            }
            _ if press.is_activate() => match self.focus {
                Some(Item::Settings) => return Event::OpenSettings,
                Some(Item::Fullscreen) => return Event::ToggleFullscreen,
                _ => focus_text(ctx),
            },
            _ => {}
        }
        Event::None
    }

    pub fn ui(&mut self, ui: &mut Ui, rect: Rect, sh: &mut Shared) -> Event {
        let painter = ui.painter().clone();
        painter.text(rect.left_center(), Align2::LEFT_CENTER, "Flex", TextStyle::Heading.resolve(ui.style()), theme::FOCUS);

        let search_rect = Rect::from_min_size(pos2(rect.left() + 170.0, rect.center().y - 30.0), vec2(700.0, 60.0));
        let edit = egui::TextEdit::singleline(&mut sh.search_query)
            .id(search_id())
            .hint_text("Search")
            .font(TextStyle::Body)
            .margin(vec2(18.0, 10.0))
            .desired_width(search_rect.width());
        let response = ui.put(search_rect, edit);

        let mut event = Event::None;
        if response.has_focus() {
            self.focus = Some(Item::Search);
            if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                ui.memory_mut(|m| m.surrender_focus(search_id()));
                self.focus = None;
                event = Event::FocusContent;
            }
        } else if self.focus == Some(Item::Search) {
            painter.rect_stroke(search_rect, CornerRadius::same(6), Stroke::new(3.0, theme::FOCUS), StrokeKind::Outside);
        }
        if response.lost_focus() {
            let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            if enter {
                self.focus = None;
                event = Event::Submit;
            } else if escape {
                event = Event::Cancel;
            }
        }

        let settings_rect = Rect::from_min_size(pos2(rect.right() - 220.0, rect.center().y - 30.0), vec2(220.0, 60.0));
        widgets::button(&painter, settings_rect, "Settings", self.focus == Some(Item::Settings), ui.style());
        if ui.interact(settings_rect, egui::Id::new("flex-settings"), Sense::click()).clicked() {
            event = Event::OpenSettings;
        }
        let fullscreen = ui.ctx().input(|i| i.viewport().fullscreen).unwrap_or(false);
        let fs_rect = settings_rect.translate(vec2(-settings_rect.width() - 20.0, 0.0));
        let fs_label = if fullscreen { "Windowed" } else { "Fullscreen" };
        widgets::button(&painter, fs_rect, fs_label, self.focus == Some(Item::Fullscreen), ui.style());
        if ui.interact(fs_rect, egui::Id::new("flex-fullscreen"), Sense::click()).clicked() {
            event = Event::ToggleFullscreen;
        }
        event
    }
}
