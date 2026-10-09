//! Shown when an episode finishes and there's another in the same folder.
//! OK / Play plays it; Back returns to wherever playback was started from.

use eframe::egui::{Align2, Rect, TextStyle, Ui, pos2, vec2};

use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::{PlayRequest, Shared};
use crate::{theme, widgets};

pub struct UpNext {
    finished: String,
    next: PlayRequest,
    /// 0 = Play, 1 = Back.
    focus: usize,
}

impl UpNext {
    pub fn new(finished: String, next: PlayRequest) -> Self {
        Self { finished, next, focus: 0 }
    }
}

impl Screen for UpNext {
    fn handle(&mut self, _sh: &mut Shared, press: Press) -> Nav {
        let play = Nav::Play { req: self.next.clone(), replace: true };
        match press.action {
            Action::Left => self.focus = 0,
            Action::Right => self.focus = 1,
            Action::Back | Action::Stop if !press.repeat => return Nav::Pop,
            Action::PlayPause | Action::Next if !press.repeat => return play,
            Action::Ok if !press.repeat => return if self.focus == 0 { play } else { Nav::Pop },
            _ => {}
        }
        Nav::None
    }

    fn ui(&mut self, ui: &mut Ui, _sh: &mut Shared, rect: Rect, _focused: bool) -> Nav {
        let painter = ui.painter();
        let style = ui.style();
        let safe = theme::safe_rect(ui.ctx());
        painter.rect_filled(rect, 0, theme::BG);

        let mut y = safe.top() + 180.0;
        let mut line = |text: &str, text_style: TextStyle, color, gap: f32| {
            let clip = painter.with_clip_rect(safe);
            clip.text(pos2(safe.left(), y), Align2::LEFT_TOP, text, text_style.resolve(style), color);
            y += gap;
        };
        line(&format!("Finished: {}", self.finished), TextStyle::Small, theme::DIM, 90.0);
        line("Up next", TextStyle::Body, theme::FOCUS, 48.0);
        line(&self.next.title, TextStyle::Heading, theme::TEXT, 110.0);

        let size = vec2(300.0, 76.0);
        let play = Rect::from_min_size(pos2(safe.left(), y), size);
        let back = play.translate(vec2(size.x + 28.0, 0.0));
        widgets::button(painter, play, "Play", self.focus == 0, style);
        widgets::button(painter, back, "Back", self.focus == 1, style);
        Nav::None
    }

    fn shows_top_bar(&self) -> bool {
        false
    }
}
