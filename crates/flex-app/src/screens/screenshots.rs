//! A video's screenshots: a grid of frames from through the video, taken the first time
//! the page is opened (see [`crate::shots`]). OK shows one full screen; Left/Right step
//! through them there, Back returns to the grid.

use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, TextStyle, Ui, vec2};

use super::{Nav, Screen};
use crate::images::{Size, paint_cover};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::shots::{self, Shot};
use crate::widgets::{self, GridSpec, GridState, ListInput};
use crate::theme;

const CARD_W: f32 = 420.0;
const GRID: GridSpec = GridSpec { card: egui::vec2(CARD_W, CARD_W * 9.0 / 16.0), gap: 24.0 };

pub struct ScreenshotsView {
    library_id: i64,
    file_id: i64,
    grid: GridState,
    /// The shot shown full screen.
    viewing: Option<usize>,
}

impl ScreenshotsView {
    pub fn new(library_id: i64, file_id: i64) -> Self {
        Self { library_id, file_id, grid: GridState::default(), viewing: None }
    }
}

/// Draws `texture` as large as fits in `rect`, keeping its shape.
fn paint_contain(painter: &egui::Painter, rect: Rect, texture: &egui::TextureHandle) {
    let size = texture.size_vec2();
    let scale = (rect.width() / size.x).min(rect.height() / size.y);
    let r = Rect::from_center_size(rect.center(), size * scale);
    painter.image(texture.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
}

impl Screen for ScreenshotsView {
    fn shows_top_bar(&self) -> bool {
        self.viewing.is_none()
    }

    fn handle(&mut self, _sh: &mut Shared, press: Press) -> Nav {
        if let Some(i) = self.viewing {
            match press.action {
                Action::Back if !press.repeat => self.viewing = None,
                Action::Left => self.viewing = Some(i.saturating_sub(1)),
                Action::Right | Action::Ok | Action::PlayPause => self.viewing = Some((i + 1).min(shots::COUNT - 1)),
                _ => {}
            }
            if let Some(i) = self.viewing {
                self.grid.select(i);
            }
            return Nav::None;
        }
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        match self.grid.handle(press, shots::COUNT) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) => {
                self.viewing = Some(i);
                Nav::None
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(titles) = sh.titles(self.library_id, self.file_id) else { return Nav::Pop };
        let shots: Vec<Shot> = (0..shots::COUNT).map(|i| sh.shot(self.library_id, self.file_id, i)).collect();
        let style = ui.style().clone();

        if let Some(i) = self.viewing {
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::ZERO, Color32::BLACK);
            match &shots[i] {
                Shot::Ready(path) => {
                    if let Some(t) = sh.images.get(path, Size::Shot) {
                        paint_contain(painter, rect, &t);
                    }
                }
                Shot::Pending => widgets::message(painter, rect, "Taking screenshot…", &style),
                Shot::Failed(e) => widgets::message(painter, rect, &format!("Couldn't take this one: {e}"), &style),
            }
            let label = format!("{} / {}  ·  {}", i + 1, shots::COUNT, titles.full());
            let font = TextStyle::Small.resolve(&style);
            let galley = painter.layout_no_wrap(label, font, theme::text());
            let pill = Rect::from_min_size(rect.left_bottom() + vec2(40.0, -86.0), galley.size() + vec2(36.0, 20.0));
            painter.rect_filled(pill, CornerRadius::same(10), theme::overlay());
            painter.galley(pill.min + vec2(18.0, 10.0), galley, theme::text());
            // A click on the picture goes back to the grid.
            if widgets::hit(ui, rect) == widgets::Hit::Click {
                self.viewing = None;
                widgets::take_hit(ui.ctx());
            }
            return Nav::None;
        }

        let done = shots.iter().filter(|s| !matches!(s, Shot::Pending)).count();
        let status = if done < shots::COUNT { format!(" · taking screenshots… {done}/{}", shots::COUNT) } else { String::new() };
        let body = widgets::header(ui.painter(), rect, "Screenshots", &format!("{}{status}", titles.full()), &style);
        let images = &mut sh.images;
        widgets::grid(ui, body, shots::COUNT, GRID, &mut self.grid, focused, |painter, i, r, focus| {
            match &shots[i] {
                Shot::Ready(path) => match images.get(path, Size::Shot) {
                    Some(t) => paint_cover(painter, r, &t),
                    None => {
                        painter.rect_filled(r, CornerRadius::same(8), theme::panel());
                    }
                },
                Shot::Pending => {
                    painter.rect_filled(r, CornerRadius::same(8), theme::panel());
                    painter.text(r.center(), Align2::CENTER_CENTER, "…", TextStyle::Heading.resolve(&style), theme::dim());
                }
                Shot::Failed(e) => {
                    painter.rect_filled(r, CornerRadius::same(8), theme::panel());
                    let galley = painter.layout(format!("Couldn't take this one: {e}"), TextStyle::Small.resolve(&style), theme::dim(), r.width() - 40.0);
                    painter.with_clip_rect(r.shrink(12.0)).galley(r.center() - galley.size() / 2.0, galley, theme::dim());
                }
            }
            theme::paint_picture_frame(painter, r, focus);
        });
        Nav::None
    }
}
