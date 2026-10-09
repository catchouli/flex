//! "Resume from 12:34 / Start from beginning", shown before playing something with a
//! saved position.

use eframe::egui::{Align2, Rect, TextStyle, Ui, pos2, vec2};

use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::{PlayRequest, Shared};
use crate::{theme, widgets};

pub struct ResumePrompt {
    req: PlayRequest,
    position: f64,
    /// 0 = Resume, 1 = Start from beginning.
    focus: usize,
}

impl ResumePrompt {
    pub fn new(req: PlayRequest, position: f64) -> Self {
        Self { req, position, focus: 0 }
    }

    fn play(&self, start: f64) -> Nav {
        Nav::Play { req: PlayRequest { start: Some(start), ..self.req.clone() }, replace: true }
    }
}

impl Screen for ResumePrompt {
    fn handle(&mut self, _sh: &mut Shared, press: Press) -> Nav {
        match press.action {
            Action::Left | Action::Up => self.focus = 0,
            Action::Right | Action::Down => self.focus = 1,
            Action::Back | Action::Stop if !press.repeat => return Nav::Pop,
            _ if press.is_activate() => return self.play(if self.focus == 0 { self.position } else { 0.0 }),
            _ => {}
        }
        Nav::None
    }

    fn ui(&mut self, ui: &mut Ui, _sh: &mut Shared, rect: Rect, _focused: bool) -> Nav {
        let painter = ui.painter();
        let style = ui.style();
        let safe = theme::safe_rect(ui.ctx());
        painter.rect_filled(rect, 0, theme::BG);

        let clip = painter.with_clip_rect(safe);
        let top = safe.top() + 220.0;
        clip.text(pos2(safe.left(), top), Align2::LEFT_TOP, &self.req.title, TextStyle::Heading.resolve(style), theme::TEXT);

        let size = vec2(480.0, 76.0);
        let resume = Rect::from_min_size(pos2(safe.left(), top + 120.0), size);
        let restart = resume.translate(vec2(size.x + 28.0, 0.0));
        let label = format!("Resume from {}", theme::format_time(self.position));
        widgets::button(painter, resume, &label, self.focus == 0, style);
        widgets::button(painter, restart, "Start from beginning", self.focus == 1, style);
        Nav::None
    }

    fn shows_top_bar(&self) -> bool {
        false
    }
}
