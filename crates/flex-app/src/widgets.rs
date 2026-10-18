//! Reusable TV-style building blocks: focusable lists, cards and buttons.

use eframe::egui::{
    self, Align, Align2, Color32, Painter, Pos2, Rect, Sense, Stroke, TextStyle, Ui,
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
    Star,
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

/// The "watched" badge: a tick in a dark circle centred on `c`.
pub fn paint_tick(painter: &Painter, c: Pos2) {
    painter.circle_filled(c, 18.0, Color32::from_black_alpha(190));
    let tick = [c + vec2(-8.0, 0.0), c + vec2(-2.0, 7.0), c + vec2(9.0, -7.0)];
    painter.line_segment([tick[0], tick[1]], Stroke::new(3.5, theme::accent()));
    painter.line_segment([tick[1], tick[2]], Stroke::new(3.5, theme::accent()));
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
        Icon::Star => {
            // A pentagon with a point on each side.
            let at = |deg: f32, r: f32| c + vec2(0.0, 1.5) + vec2(deg.to_radians().cos(), deg.to_radians().sin()) * r;
            let inner: Vec<Pos2> = (0..5).map(|i| at(-54.0 + 72.0 * i as f32, 6.5)).collect();
            painter.add(egui::Shape::convex_polygon(inner.clone(), color, Stroke::NONE));
            for i in 0..5 {
                let points = vec![inner[(i + 4) % 5], at(-90.0 + 72.0 * i as f32, 16.0), inner[i]];
                painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
            }
        }
    }
}

/// Background + focus look shared by rows, cards and buttons (from the theme). Returns
/// the text colour to use.
pub fn paint_focusable(painter: &Painter, rect: Rect, focused: bool, selected: bool) -> Color32 {
    theme::paint_panel(painter, rect, focused, selected)
}

/// The mouse over something selectable.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    None,
    /// The pointer moved onto it: select it.
    Hover,
    /// Left-clicked: select it; the app then sends OK.
    Click,
}

impl Hit {
    pub fn any(self) -> bool {
        self != Hit::None
    }
}

fn click_id() -> egui::Id {
    egui::Id::new("flex-pointer-click")
}

fn hover_id() -> egui::Id {
    egui::Id::new("flex-pointer-hover")
}

fn modal_id() -> egui::Id {
    egui::Id::new("flex-pointer-modal")
}

fn blocked_id() -> egui::Id {
    egui::Id::new("flex-pointer-blocked")
}

/// While a dropdown is open, only things inside `rect` respond to the mouse this frame.
pub fn modal(ctx: &egui::Context, rect: Rect) {
    ctx.data_mut(|d| d.insert_temp(modal_id(), Some(rect)));
}

/// Whether the mouse is over `rect` (as far as it's visible in `ui`). Only a moving pointer
/// counts as hovering, so scrolling under a still mouse doesn't change the selection.
/// A click is remembered for [`take_click`]: the screen selects the item here, then the app
/// activates it with an OK press.
pub fn hit(ui: &Ui, rect: Rect) -> Hit {
    if !pointer_over(ui, rect) {
        return Hit::None;
    }
    let (moved, clicked) = ui.input(|i| (i.pointer.delta() != egui::Vec2::ZERO, i.pointer.primary_clicked()));
    if ui.ctx().data(|d| d.get_temp::<Vec<Rect>>(blocked_id())).unwrap_or_default().iter().any(|b| pointer_over(ui, *b)) {
        return Hit::None; // on a scroll arrow
    }
    if clicked {
        ui.ctx().data_mut(|d| d.insert_temp(click_id(), true));
        Hit::Click
    } else if moved {
        ui.ctx().data_mut(|d| d.insert_temp(hover_id(), true));
        Hit::Hover
    } else {
        Hit::None
    }
}

/// Whether the mouse is on the part of `rect` visible in `ui`, and not under a dropdown
/// or something drawn on top.
fn pointer_over(ui: &Ui, rect: Rect) -> bool {
    let visible = rect.intersect(ui.clip_rect());
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()).filter(|p| visible.contains(*p)) else { return false };
    if ui.ctx().data(|d| d.get_temp::<Option<Rect>>(modal_id())).flatten().is_some_and(|m| !m.contains(pos)) {
        return false;
    }
    ui.ctx().layer_id_at(pos).is_none_or(|l| l == ui.layer_id())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

const HINT_RADIUS: f32 = 26.0;
/// Room for a [`ScrollHints`] arrow beside a row that the arrow shouldn't cover.
pub const HINT_SPACE: f32 = 2.0 * HINT_RADIUS + 12.0;

/// Arrows on the ends of a sideways row where more items are hidden. Clicking one asks
/// the row to scroll that way.
///
/// Make it before checking the items' [`hit`]s, so items under an arrow ignore the mouse,
/// and paint it after drawing them.
pub struct ScrollHints {
    arrows: Vec<(Side, Rect, bool)>,
    pub clicked: Option<Side>,
}

impl ScrollHints {
    /// `row` spans the visible items.
    pub fn new(ui: &Ui, row: Rect, more_left: bool, more_right: bool) -> Self {
        let y = row.center().y;
        let size = vec2(2.0 * HINT_RADIUS, 2.0 * HINT_RADIUS);
        let mut arrows = Vec::new();
        let mut clicked = None;
        let inset = HINT_RADIUS + 6.0;
        for (side, more, x) in [(Side::Left, more_left, row.left() + inset), (Side::Right, more_right, row.right() - inset)] {
            if !more {
                continue;
            }
            let rect = Rect::from_center_size(egui::pos2(x, y), size);
            let over = pointer_over(ui, rect);
            if over {
                ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<Vec<Rect>>(blocked_id()).push(rect));
                if ui.input(|i| i.pointer.primary_clicked()) {
                    clicked = Some(side);
                }
            }
            arrows.push((side, rect, over));
        }
        Self { arrows, clicked }
    }

    pub fn paint(&self, painter: &Painter) {
        for &(side, rect, over) in &self.arrows {
            let c = rect.center();
            painter.circle_filled(c, HINT_RADIUS, theme::overlay());
            let dir = if side == Side::Left { -1.0 } else { 1.0 };
            let points = vec![c + vec2(-5.0 * dir, -11.0), c + vec2(6.0 * dir, 0.0), c + vec2(-5.0 * dir, 11.0)];
            let color = if over { theme::accent() } else { theme::text() };
            painter.add(egui::Shape::line(points, Stroke::new(4.0, color)));
        }
    }
}

/// What the mouse did to the screen's items this frame (see [`hit`]); clears it.
pub fn take_hit(ctx: &egui::Context) -> Hit {
    ctx.data_mut(|d| {
        d.remove_temp::<Option<Rect>>(modal_id());
        d.remove_temp::<Vec<Rect>>(blocked_id());
        let clicked = d.remove_temp::<bool>(click_id()).unwrap_or(false);
        let hovered = d.remove_temp::<bool>(hover_id()).unwrap_or(false);
        if clicked { Hit::Click } else if hovered { Hit::Hover } else { Hit::None }
    })
}

/// A vertical list filling `rect`. `focused` = the list (not the top bar) has input focus.
/// Returns what the mouse did; the hovered or clicked row is already selected.
pub fn list(ui: &mut Ui, rect: Rect, rows: &[Row], state: &mut ListState, focused: bool) -> Hit {
    state.selected = state.selected.min(rows.len().saturating_sub(1));
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let mut result = Hit::None;
    let body = TextStyle::Body.resolve(ui.style());
    let small = TextStyle::Small.resolve(ui.style());
    egui::ScrollArea::vertical()
        .id_salt("list")
        .max_height(rect.height())
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(&mut ui, |ui| {
            for (i, row) in rows.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
                let hit = hit(ui, r);
                if hit.any() {
                    state.selected = i;
                    result = hit;
                }
                let selected = i == state.selected;
                let color = paint_focusable(ui.painter(), r, focused && selected, selected);
                let color = if row.dim && !(focused && selected) { theme::panel_dim() } else { color };
                if let Some(fraction) = row.progress {
                    let bar_color = theme::progress_on_panel(focused && selected);
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
                theme::panel_shadow_text(
                    &ui.painter().with_clip_rect(title_clip.intersect(r.shrink(8.0))),
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
    result
}

/// A large card: title plus up to two lines of detail.
pub fn card(painter: &Painter, rect: Rect, title: &str, lines: &[&str], focused: bool, style: &egui::Style) {
    let color = paint_focusable(painter, rect, focused, false);
    let inner = rect.shrink(24.0);
    let clip = painter.with_clip_rect(rect.shrink(8.0));
    theme::panel_shadow_text(&clip, inner.left_top(), Align2::LEFT_TOP, title, TextStyle::Heading.resolve(style), color);
    let mut y = inner.bottom();
    for line in lines.iter().rev() {
        theme::panel_shadow_text(&clip, egui::pos2(inner.left(), y), Align2::LEFT_BOTTOM, line, TextStyle::Body.resolve(style), color);
        y -= 40.0;
    }
}

/// A centred-label button.
pub fn button(painter: &Painter, rect: Rect, label: &str, focused: bool, style: &egui::Style) {
    let color = paint_focusable(painter, rect, focused, false);
    theme::panel_shadow_text(painter, rect.center(), Align2::CENTER_CENTER, label, TextStyle::Body.resolve(style), color);
}

/// Screen title with an optional dim subtitle; returns the rect left below it.
pub fn header(painter: &Painter, rect: Rect, title: &str, subtitle: &str, style: &egui::Style) -> Rect {
    // A title bar reaches a little above `rect`.
    let clip = painter.with_clip_rect(rect.with_min_y(rect.top() - 12.0));
    let title = theme::paint_title(&clip, rect, title, TextStyle::Heading.resolve(style));
    let mut top = rect.top() + 70.0;
    if !subtitle.is_empty() {
        // Clear of a title bar.
        top = top.max(title.bottom() + 8.0);
        theme::shadow_text(&clip, egui::pos2(rect.left(), top), Align2::LEFT_TOP, subtitle, TextStyle::Small.resolve(style), theme::dim());
        top += 40.0;
    }
    Rect::from_min_max(egui::pos2(rect.left(), top + 10.0), rect.max)
}

/// Wrapped dim text at the top of `rect`, at most `max_lines` lines; returns its height.
pub fn paragraph(painter: &Painter, rect: Rect, text: &str, max_lines: usize, style: &egui::Style) -> f32 {
    let font = TextStyle::Small.resolve(style);
    let line_h = font.size * 1.35;
    let galley = painter.layout(text.to_owned(), font, theme::dim(), rect.width().min(1400.0));
    let height = galley.size().y.min(line_h * max_lines as f32);
    let clip = Rect::from_min_size(rect.min, vec2(rect.width(), height));
    painter.with_clip_rect(clip).galley(rect.min, galley, theme::dim());
    height
}

/// Wrapped text filling `rect` from the top, cut off after the last line that fits whole.
pub fn text_block(painter: &Painter, rect: Rect, text: &str, font: egui::FontId, color: Color32) {
    let galley = painter.layout(text.to_owned(), font, color, rect.width());
    let fits = galley.rows.iter().map(|r| r.rect().bottom()).take_while(|b| *b <= rect.height()).last().unwrap_or(0.0);
    let clip = Rect::from_min_size(rect.min, vec2(rect.width(), fits));
    painter.with_clip_rect(clip.intersect(painter.clip_rect())).galley(rect.min, galley, color);
}

/// Dim centred message, for empty states.
pub fn message(painter: &Painter, rect: Rect, text: &str, style: &egui::Style) {
    painter.text(rect.center_top() + vec2(0.0, 80.0), Align2::CENTER_TOP, text, TextStyle::Body.resolve(style), theme::dim());
}

/// Selection and scroll position for a [`grid`].
#[derive(Default)]
pub struct GridState {
    pub selected: usize,
    /// Columns at the last draw (navigation needs it).
    cols: usize,
    /// Rows that fit on screen at the last draw (for paging).
    visible_rows: usize,
    scroll: f32,
    reveal: bool,
}

impl GridState {
    pub fn select(&mut self, i: usize) {
        self.selected = i;
        self.reveal = true;
    }

    /// D-pad navigation over `len` items. Up from the first row reports `AtTop`.
    pub fn handle(&mut self, press: Press, len: usize) -> ListInput {
        if len == 0 {
            return if press.is(Action::Up) { ListInput::AtTop } else { ListInput::None };
        }
        let cols = self.cols.max(1);
        let last = len - 1;
        let page = cols * self.visible_rows.max(1);
        let before = self.selected;
        let s = self.selected.min(last);
        self.selected = match press.action {
            Action::Left if !s.is_multiple_of(cols) => s - 1,
            Action::Right if s < last && !(s + 1).is_multiple_of(cols) => s + 1,
            Action::Up if s < cols => return if press.repeat { ListInput::None } else { ListInput::AtTop },
            Action::Up => s - cols,
            // Down from the row above a short last row lands on its last item.
            Action::Down if s / cols < last / cols => (s + cols).min(last),
            Action::PageUp => s.saturating_sub(page),
            Action::PageDown => (s + page).min(last),
            _ if press.is_activate() => return ListInput::Activate(s),
            _ => s,
        };
        self.reveal |= before != self.selected;
        ListInput::None
    }
}

/// Card size and spacing for a [`grid`].
#[derive(Clone, Copy)]
pub struct GridSpec {
    pub card: egui::Vec2,
    pub gap: f32,
}

/// A scrolling grid of `len` cards filling `rect`. Only visible cards are drawn (so posters
/// load lazily); `draw(painter, index, card_rect, focused)` paints one. Returns what the
/// mouse did, like [`list`].
pub fn grid(
    ui: &mut Ui,
    rect: Rect,
    len: usize,
    spec: GridSpec,
    state: &mut GridState,
    focused: bool,
    mut draw: impl FnMut(&Painter, usize, Rect, bool),
) -> Hit {
    let GridSpec { card, gap } = spec;
    let cols = (((rect.width() + gap) / (card.x + gap)) as usize).max(1);
    let row_h = card.y + gap;
    let rows = len.div_ceil(cols);
    state.cols = cols;
    state.visible_rows = ((rect.height() / row_h) as usize).max(1);
    state.selected = state.selected.min(len.saturating_sub(1));
    let content_h = rows as f32 * row_h;
    if std::mem::take(&mut state.reveal) {
        let top = (state.selected / cols) as f32 * row_h;
        if top < state.scroll {
            state.scroll = top;
        } else if top + card.y > state.scroll + rect.height() {
            state.scroll = top + card.y - rect.height() + gap;
        }
    }
    if ui.rect_contains_pointer(rect) {
        state.scroll -= ui.input(|i| i.smooth_scroll_delta.y);
    }
    state.scroll = state.scroll.clamp(0.0, (content_h - rect.height()).max(0.0));

    let painter = ui.painter().with_clip_rect(rect);
    let mut result = Hit::None;
    let first_row = (state.scroll / row_h) as usize;
    let last_row = (((state.scroll + rect.height()) / row_h) as usize + 1).min(rows);
    for row in first_row..last_row {
        for col in 0..cols {
            let i = row * cols + col;
            if i >= len {
                break;
            }
            let min = egui::pos2(rect.left() + col as f32 * (card.x + gap), rect.top() + row as f32 * row_h - state.scroll);
            let r = Rect::from_min_size(min, card);
            let hit = hit(ui, r.intersect(rect));
            if hit.any() {
                state.selected = i;
                result = hit;
            }
            draw(&painter, i, r, focused && i == state.selected);
        }
    }
    result
}
