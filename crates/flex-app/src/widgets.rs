//! Reusable TV-style building blocks: focusable lists, cards and buttons.

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, TextStyle, Ui,
    vec2,
};

use crate::input::{Action, Press};
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    None,
    Folder,
    Video,
    Add,
    Refresh,
    Remove,
}

pub struct Row {
    pub icon: Icon,
    pub title: String,
    /// Right-aligned secondary text.
    pub detail: String,
    /// Partly-watched fraction, drawn as a thin bar along the bottom.
    pub progress: Option<f32>,
    /// De-emphasised (e.g. already watched).
    pub dim: bool,
}

impl Row {
    pub fn new(icon: Icon, title: impl Into<String>) -> Self {
        Self { icon, title: title.into(), detail: String::new(), progress: None, dim: false }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn progress(mut self, fraction: Option<f64>) -> Self {
        self.progress = fraction.map(|f| f as f32);
        self
    }

    pub fn dim(mut self, dim: bool) -> Self {
        self.dim = dim;
        self
    }
}

/// A thin progress bar along the bottom of `rect`.
pub fn paint_progress(painter: &Painter, rect: Rect, fraction: f32, color: Color32) {
    let bar = Rect::from_min_size(egui::pos2(rect.left() + 12.0, rect.bottom() - 9.0), vec2(rect.width() - 24.0, 5.0));
    painter.rect_filled(bar, 2, Color32::from_white_alpha(40));
    let mut done = bar;
    done.set_width(bar.width() * fraction.clamp(0.0, 1.0));
    painter.rect_filled(done, 2, color);
}

const ROW_HEIGHT: f32 = 64.0;
const ROW_GAP: f32 = 6.0;
const PAGE: usize = 8;

pub enum ListInput {
    None,
    /// Up was pressed on the first row.
    AtTop,
    Activate(usize),
}

#[derive(Default)]
pub struct ListState {
    pub selected: usize,
    scroll_pending: bool,
}

impl ListState {
    pub fn select(&mut self, i: usize) {
        self.selected = i;
        self.scroll_pending = true;
    }

    pub fn handle(&mut self, press: Press, len: usize) -> ListInput {
        let last = len.saturating_sub(1);
        let before = self.selected;
        match press.action {
            Action::Up if self.selected == 0 => return if press.repeat { ListInput::None } else { ListInput::AtTop },
            Action::Up => self.selected -= 1,
            Action::Down => self.selected = (self.selected + 1).min(last),
            Action::Left | Action::PageUp => self.selected = self.selected.saturating_sub(PAGE),
            Action::Right | Action::PageDown => self.selected = (self.selected + PAGE).min(last),
            _ if press.is_activate() && len > 0 => return ListInput::Activate(self.selected),
            _ => {}
        }
        self.scroll_pending |= before != self.selected;
        ListInput::None
    }
}

pub fn paint_icon(painter: &Painter, c: Pos2, icon: Icon, color: Color32) {
    match icon {
        Icon::None => {}
        Icon::Folder => {
            painter.rect_filled(Rect::from_min_size(c + vec2(-14.0, -13.0), vec2(12.0, 6.0)), 2, color);
            painter.rect_filled(Rect::from_min_size(c + vec2(-14.0, -9.0), vec2(28.0, 20.0)), 3, color);
        }
        Icon::Video => {
            let points = vec![c + vec2(-9.0, -12.0), c + vec2(13.0, 0.0), c + vec2(-9.0, 12.0)];
            painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        }
        Icon::Add => {
            painter.rect_filled(Rect::from_center_size(c, vec2(26.0, 5.0)), 2, color);
            painter.rect_filled(Rect::from_center_size(c, vec2(5.0, 26.0)), 2, color);
        }
        Icon::Refresh => {
            painter.circle_stroke(c, 11.0, Stroke::new(4.0, color));
        }
        Icon::Remove => {
            painter.line_segment([c + vec2(-10.0, -10.0), c + vec2(10.0, 10.0)], Stroke::new(5.0, color));
            painter.line_segment([c + vec2(10.0, -10.0), c + vec2(-10.0, 10.0)], Stroke::new(5.0, color));
        }
    }
}

/// Background + focus ring shared by rows, cards and buttons. Returns the text colour to use.
pub fn paint_focusable(painter: &Painter, rect: Rect, focused: bool, selected: bool) -> Color32 {
    let radius = CornerRadius::same(8);
    if focused {
        painter.rect_filled(rect, radius, theme::FOCUS);
        painter.rect_stroke(rect, radius, Stroke::new(3.0, Color32::WHITE), StrokeKind::Outside);
        Color32::BLACK
    } else {
        painter.rect_filled(rect, radius, theme::ROW);
        if selected {
            painter.rect_stroke(rect, radius, Stroke::new(2.0, theme::FOCUS), StrokeKind::Inside);
        }
        theme::TEXT
    }
}

/// A vertical list filling `rect`. `focused` = the list (not the top bar) has input focus.
pub fn list(ui: &mut Ui, rect: Rect, rows: &[Row], state: &mut ListState, focused: bool) {
    state.selected = state.selected.min(rows.len().saturating_sub(1));
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let body = TextStyle::Body.resolve(ui.style());
    let small = TextStyle::Small.resolve(ui.style());
    egui::ScrollArea::vertical()
        .id_salt("list")
        .max_height(rect.height())
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(&mut ui, |ui| {
            for (i, row) in rows.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
                let selected = i == state.selected;
                let color = paint_focusable(ui.painter(), r, focused && selected, selected);
                let color = if row.dim && !(focused && selected) { theme::DIM } else { color };
                if let Some(fraction) = row.progress {
                    let bar_color = if focused && selected { Color32::BLACK } else { theme::FOCUS };
                    paint_progress(ui.painter(), r, fraction, bar_color);
                }
                let text_x = if row.icon == Icon::None { 24.0 } else { 68.0 };
                paint_icon(ui.painter(), r.left_center() + vec2(36.0, 0.0), row.icon, color);

                let detail_w = if row.detail.is_empty() {
                    0.0
                } else {
                    let galley = ui.painter().layout_no_wrap(row.detail.clone(), small.clone(), color);
                    let w = galley.size().x.min(r.width() * 0.45);
                    let pos = r.right_center() - vec2(24.0 + w, galley.size().y / 2.0);
                    ui.painter().with_clip_rect(Rect::from_min_size(pos, vec2(w, r.height()))).galley(pos, galley, color);
                    w + 32.0
                };
                let title_clip = Rect::from_min_max(r.min, egui::pos2(r.right() - detail_w - 12.0, r.bottom()));
                ui.painter().with_clip_rect(title_clip.intersect(r.shrink(8.0))).text(
                    r.left_center() + vec2(text_x, 0.0),
                    Align2::LEFT_CENTER,
                    &row.title,
                    body.clone(),
                    color,
                );
                if selected && state.scroll_pending {
                    ui.scroll_to_rect(r, Some(Align::Center));
                }
                ui.add_space(ROW_GAP);
            }
        });
    state.scroll_pending = false;
}

/// A large card: title plus up to two lines of detail.
pub fn card(painter: &Painter, rect: Rect, title: &str, lines: &[&str], focused: bool, style: &egui::Style) {
    let color = paint_focusable(painter, rect, focused, false);
    let inner = rect.shrink(24.0);
    let clip = painter.with_clip_rect(rect.shrink(8.0));
    clip.text(inner.left_top(), Align2::LEFT_TOP, title, TextStyle::Heading.resolve(style), color);
    let mut y = inner.bottom();
    for line in lines.iter().rev() {
        clip.text(egui::pos2(inner.left(), y), Align2::LEFT_BOTTOM, *line, TextStyle::Body.resolve(style), color);
        y -= 40.0;
    }
}

/// A centred-label button.
pub fn button(painter: &Painter, rect: Rect, label: &str, focused: bool, style: &egui::Style) {
    let color = paint_focusable(painter, rect, focused, false);
    painter.text(rect.center(), Align2::CENTER_CENTER, label, TextStyle::Body.resolve(style), color);
}

/// Screen title with an optional dim subtitle; returns the rect left below it.
pub fn header(painter: &Painter, rect: Rect, title: &str, subtitle: &str, style: &egui::Style) -> Rect {
    let clip = painter.with_clip_rect(rect);
    clip.text(rect.left_top(), Align2::LEFT_TOP, title, TextStyle::Heading.resolve(style), theme::TEXT);
    let mut top = rect.top() + 70.0;
    if !subtitle.is_empty() {
        clip.text(egui::pos2(rect.left(), top), Align2::LEFT_TOP, subtitle, TextStyle::Small.resolve(style), theme::DIM);
        top += 40.0;
    }
    Rect::from_min_max(egui::pos2(rect.left(), top + 10.0), rect.max)
}

/// Wrapped dim text at the top of `rect`, at most `max_lines` lines; returns its height.
pub fn paragraph(painter: &Painter, rect: Rect, text: &str, max_lines: usize, style: &egui::Style) -> f32 {
    let font = TextStyle::Small.resolve(style);
    let line_h = font.size * 1.35;
    let galley = painter.layout(text.to_owned(), font, theme::DIM, rect.width().min(1400.0));
    let height = galley.size().y.min(line_h * max_lines as f32);
    let clip = Rect::from_min_size(rect.min, vec2(rect.width(), height));
    painter.with_clip_rect(clip).galley(rect.min, galley, theme::DIM);
    height
}

/// Dim centred message, for empty states.
pub fn message(painter: &Painter, rect: Rect, text: &str, style: &egui::Style) {
    painter.text(rect.center_top() + vec2(0.0, 80.0), Align2::CENTER_TOP, text, TextStyle::Body.resolve(style), theme::DIM);
}
