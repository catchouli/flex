//! The bar across the top: app name, search box, fullscreen toggle and Settings.
//!
//! With the remote: Up from the top of a screen focuses it, Left/Right moves between items,
//! OK on the search box starts typing (needs a keyboard), Down returns to the screen.
//! With a keyboard: just start typing anywhere to search. There's also a Back button on
//! screens that have somewhere to go back to.

use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, Stroke, StrokeKind, TextStyle, Ui, pos2, vec2};

use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::{theme, widgets};

pub const HEIGHT: f32 = 72.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Search,
    Back,
    Fullscreen,
    Settings,
}

pub enum Event {
    None,
    /// Return input focus to the screen below.
    FocusContent,
    /// Enter in the search box.
    Submit,
    /// Escape in the search box: leave it, then act as Back.
    Cancel,
    OpenSettings,
    ToggleFullscreen,
    /// The Back button was clicked or pressed.
    Back,
}

#[derive(Default)]
pub struct TopBar {
    /// Which item has d-pad focus; `None` = the screen has it.
    pub focus: Option<Item>,
    /// Whether the search box had keyboard focus last frame (egui drops it on Esc before
    /// we get to look, so this is how Esc in the box is recognised).
    text_focused_last: bool,
    /// Whether the Back button is showing.
    can_go_back: bool,
}

fn search_id() -> egui::Id {
    egui::Id::new("flex-search")
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
    /// The items in d-pad order (left to right).
    fn items(&self) -> Vec<Item> {
        let back = self.can_go_back.then_some(Item::Back);
        [Some(Item::Search), back, Some(Item::Fullscreen), Some(Item::Settings)].into_iter().flatten().collect()
    }

    pub fn handle(&mut self, ctx: &egui::Context, press: Press) -> Event {
        let items = self.items();
        let i = items.iter().position(|it| Some(*it) == self.focus).unwrap_or(0);
        match press.action {
            Action::Left => self.focus = Some(items[i.saturating_sub(1)]),
            Action::Right => self.focus = Some(items[(i + 1).min(items.len() - 1)]),
            Action::Down | Action::Back => {
                self.focus = None;
                return Event::FocusContent;
            }
            _ if press.is_activate() => match self.focus {
                Some(Item::Settings) => return Event::OpenSettings,
                Some(Item::Fullscreen) => return Event::ToggleFullscreen,
                Some(Item::Back) => return Event::Back,
                _ => focus_text(ctx),
            },
            _ => {}
        }
        Event::None
    }

    /// `can_go_back`: there's a screen to go back to, so show the Back button.
    pub fn ui(&mut self, ui: &mut Ui, rect: Rect, sh: &mut Shared, can_go_back: bool) -> Event {
        self.can_go_back = can_go_back;
        if !can_go_back && self.focus == Some(Item::Back) {
            self.focus = Some(Item::Fullscreen);
        }
        let painter = ui.painter().clone();
        let (heading, small) = (TextStyle::Heading.resolve(ui.style()), TextStyle::Small.resolve(ui.style()));
        let name = painter.text(rect.left_center(), Align2::LEFT_CENTER, "Flex", heading.clone(), theme::FOCUS);
        // The version beside the name, smaller and dimmer, on the same baseline (fonts
        // descend about a fifth of their size below it).
        let baseline = name.bottom() - heading.size * 0.2;
        let version = painter.text(
            pos2(name.right() + 12.0, baseline + small.size * 0.2 - 2.0),
            Align2::LEFT_BOTTOM,
            concat!("v", env!("CARGO_PKG_VERSION")),
            small,
            theme::DIM,
        );

        let search_left = (rect.left() + 170.0).max(version.right() + 36.0);
        let search_rect = Rect::from_min_size(pos2(search_left, rect.center().y - 30.0), vec2(700.0, 60.0));
        let edit = egui::TextEdit::singleline(&mut sh.search_query)
            .id(search_id())
            .hint_text("Search")
            .font(TextStyle::Body)
            .margin(vec2(18.0, 10.0))
            .desired_width(search_rect.width());
        let had_focus = std::mem::replace(&mut self.text_focused_last, false);
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        let response = ui.put(search_rect, edit);
        self.text_focused_last = response.has_focus();

        let mut event = Event::None;
        if had_focus && escape {
            ui.memory_mut(|m| m.surrender_focus(search_id()));
            self.focus = None;
            return Event::Cancel;
        }
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
        if response.lost_focus() && enter {
            self.focus = None;
            event = Event::Submit;
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
        if can_go_back {
            let back_rect = Rect::from_min_size(fs_rect.min - vec2(160.0 + 20.0, 0.0), vec2(160.0, fs_rect.height()));
            let back = ui.interact(back_rect, egui::Id::new("flex-back"), Sense::click());
            widgets::button(&painter, back_rect, "‹ Back", back.hovered() || self.focus == Some(Item::Back), ui.style());
            if back.clicked() {
                event = Event::Back;
            }
        }
        event
    }
}
