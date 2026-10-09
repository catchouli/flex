//! Detail pages: a show or film (poster, description, play buttons, seasons, cast) and a
//! single episode (still, title, description, play buttons).
//!
//! The page is a stack of horizontal rows (buttons, seasons, cast): Up/Down moves between
//! rows, Left/Right along them. Play/Pause anywhere plays the main button.

use eframe::egui::{self, Align2, Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind, TextStyle, Ui, pos2, vec2};
use flex_core::metadata::CastMember;
use flex_core::progress::{NextUp, next_up};

use super::inspector::MatchInspector;
use super::options::{OptionsMenu, Target};
use super::match_online::MatchOnline;
use super::shows::{SeasonView, ShowOptions};
use super::{Nav, Screen};
use crate::images::{Size, paint_cover};
use crate::input::{Action, Press};
use crate::shared::{PlayRequest, Shared, thousands};
use crate::{theme, widgets};

const POSTER: egui::Vec2 = egui::vec2(320.0, 480.0);
const BUTTON_H: f32 = 64.0;
const SEASON_CARD: egui::Vec2 = egui::vec2(250.0, 100.0);
const CAST_CARD: egui::Vec2 = egui::vec2(160.0, 300.0);
const GAP: f32 = 22.0;
const ROW_LABEL_H: f32 = 52.0;
const SECTION_GAP: f32 = 44.0;

#[derive(Clone)]
enum Button {
    Play { label: String, req: PlayRequest },
    MarkWatched(bool),
    /// Add to (true) or remove from the watchlist.
    Watchlist(bool),
    Folders,
    MatchOnline,
}

impl Button {
    fn label(&self) -> String {
        match self {
            Button::Play { label, .. } => label.clone(),
            Button::MarkWatched(true) => "Mark watched".into(),
            Button::MarkWatched(false) => "Mark unwatched".into(),
            Button::Watchlist(true) => "+ Watchlist".into(),
            Button::Watchlist(false) => "On watchlist ✓".into(),
            Button::Folders => "Folders & seasons…".into(),
            Button::MatchOnline => "Match online…".into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Buttons,
    Seasons,
    Cast,
}

fn play_label(sh: &Shared, library_id: i64, file_id: i64) -> String {
    sh.titles(library_id, file_id).and_then(|t| t.episode).unwrap_or_default()
}

/// "S1E5" for the label of the main button.
fn short_code(sh: &Shared, library_id: i64, file_id: i64) -> String {
    let label = play_label(sh, library_id, file_id);
    label.split(" · ").next().unwrap_or(&label).to_owned()
}

/// Resume / Play buttons for a single file.
fn play_buttons(sh: &Shared, library_id: i64, file_id: i64) -> Vec<Button> {
    let Some(req) = sh.play_request(library_id, file_id) else { return Vec::new() };
    match sh.progress(library_id, file_id).and_then(|p| p.position) {
        Some(pos) => vec![
            Button::Play {
                label: format!("Resume from {}", theme::format_time(pos)),
                req: PlayRequest { start: Some(pos), ..req.clone() },
            },
            Button::Play { label: "Play from start".into(), req: PlayRequest { start: Some(0.0), ..req } },
        ],
        None => vec![Button::Play { label: "Play".into(), req: PlayRequest { start: Some(0.0), ..req } }],
    }
}

/// A horizontal row that scrolls to keep the selected item in view.
fn hrow(
    painter: &Painter,
    // Top-left corner and available width (the height is unused).
    area: Rect,
    item: egui::Vec2,
    count: usize,
    selected: usize,
    offset: &mut usize,
    mut draw: impl FnMut(&Painter, usize, Rect, bool),
) {
    let (origin, width) = (area.min, area.width());
    let visible = (((width + GAP) / (item.x + GAP)) as usize).max(1);
    if selected < *offset {
        *offset = selected;
    } else if selected >= *offset + visible {
        *offset = selected + 1 - visible;
    }
    for (slot, i) in (*offset..count).take(visible).enumerate() {
        let r = Rect::from_min_size(origin + vec2(slot as f32 * (item.x + GAP), 0.0), item);
        draw(painter, i, r, i == selected);
    }
}

fn paint_cast(painter: &Painter, rect: Rect, member: &CastMember, photo: Option<&egui::TextureHandle>, focused: bool, style: &egui::Style) {
    let photo_rect = Rect::from_min_size(rect.min, vec2(rect.width(), rect.width() * 1.5));
    match photo {
        Some(t) => paint_cover(painter, photo_rect, t),
        None => {
            painter.rect_filled(photo_rect, CornerRadius::same(8), theme::ROW);
        }
    }
    if focused {
        painter.rect_stroke(photo_rect, CornerRadius::same(8), Stroke::new(4.0, theme::FOCUS), StrokeKind::Inside);
    }
    let text = Rect::from_min_max(pos2(rect.left(), photo_rect.bottom() + 6.0), rect.max);
    let clip = painter.with_clip_rect(text);
    let color = if focused { theme::FOCUS } else { theme::TEXT };
    clip.text(text.left_top(), Align2::LEFT_TOP, &member.name, TextStyle::Small.resolve(style), color);
    clip.text(text.left_top() + vec2(0.0, 30.0), Align2::LEFT_TOP, &member.role, TextStyle::Small.resolve(style), theme::DIM);
}

fn paint_button(painter: &Painter, rect: Rect, label: &str, focused: bool, primary: bool, style: &egui::Style) {
    let color = if focused {
        widgets::paint_focusable(painter, rect, true, false)
    } else {
        let fill = if primary { Color32::from_rgb(70, 56, 24) } else { theme::ROW };
        painter.rect_filled(rect, CornerRadius::same(8), fill);
        theme::TEXT
    };
    painter.text(rect.center(), Align2::CENTER_CENTER, label, TextStyle::Body.resolve(style), color);
}

fn button_width(painter: &Painter, label: &str, style: &egui::Style) -> f32 {
    painter.layout_no_wrap(label.to_owned(), TextStyle::Body.resolve(style), Color32::WHITE).size().x + 56.0
}

/// Shared focus/scroll handling for the pages.
#[derive(Default)]
struct PageNav {
    row: usize,
    /// Selected item per row kind (buttons, seasons, cast).
    col: [usize; 3],
    offset: [usize; 3],
    scroll: f32,
    reveal: bool,
}

impl PageNav {
    fn kind_index(kind: RowKind) -> usize {
        match kind {
            RowKind::Buttons => 0,
            RowKind::Seasons => 1,
            RowKind::Cast => 2,
        }
    }

    /// Moves focus; returns `Some(nav)` when the press leaves the page or isn't a move.
    fn handle(&mut self, press: Press, rows: &[(RowKind, usize)]) -> Option<Nav> {
        self.row = self.row.min(rows.len().saturating_sub(1));
        let &(kind, len) = rows.get(self.row)?;
        let c = &mut self.col[Self::kind_index(kind)];
        match press.action {
            Action::Up if self.row == 0 => return Some(if press.repeat { Nav::None } else { Nav::FocusTopBar }),
            Action::Up => self.row -= 1,
            Action::Down => self.row = (self.row + 1).min(rows.len() - 1),
            Action::Left => *c = c.saturating_sub(1),
            Action::Right => *c = (*c + 1).min(len.saturating_sub(1)),
            _ => return None,
        }
        self.reveal = true;
        Some(Nav::None)
    }

    fn focused(&self, rows: &[(RowKind, usize)]) -> Option<(RowKind, usize)> {
        rows.get(self.row).map(|&(kind, _)| (kind, self.col[Self::kind_index(kind)]))
    }

    /// Scrolls so the focused row (top..bottom, relative to content) is visible.
    fn scroll_to(&mut self, top: f32, bottom: f32, view_h: f32, content_h: f32) {
        if std::mem::take(&mut self.reveal) {
            if top < self.scroll {
                self.scroll = top;
            } else if bottom > self.scroll + view_h {
                self.scroll = bottom - view_h;
            }
        }
        self.scroll = self.scroll.clamp(0.0, (content_h - view_h).max(0.0));
    }
}

/// A show or film's page.
pub struct TitlePage {
    library_id: i64,
    show_id: i64,
    nav: PageNav,
    /// Cast for the current library version (loaded from the database once), and whether
    /// it's the Japanese voice cast.
    cast: Option<(u64, Vec<CastMember>)>,
    voice: bool,
}

impl TitlePage {
    pub fn new(library_id: i64, show_id: i64) -> Self {
        Self { library_id, show_id, nav: PageNav::default(), cast: None, voice: false }
    }

    fn buttons(&self, sh: &Shared) -> Vec<Button> {
        let Some(state) = sh.lib(self.library_id) else { return Vec::new() };
        let Some(entry) = state.catalog().and_then(|c| c.show(self.show_id)) else { return Vec::new() };
        let mut buttons = if entry.is_series() {
            let play = |id, label: String, start| {
                sh.play_request(self.library_id, id).map(|req| Button::Play { label, req: PlayRequest { start, ..req } })
            };
            let main = match next_up(entry, &state.tree) {
                Some(NextUp::Resume(id)) => {
                    let pos = sh.progress(self.library_id, id).and_then(|p| p.position);
                    play(id, format!("Resume {}", short_code(sh, self.library_id, id)), pos)
                }
                Some(NextUp::Next(id)) => play(id, format!("Play {}", short_code(sh, self.library_id, id)), Some(0.0)),
                Some(NextUp::Start(id)) => play(id, "Play from start".into(), Some(0.0)),
                None => None,
            };
            main.into_iter().collect()
        } else {
            entry.episodes.first().map_or_else(Vec::new, |e| play_buttons(sh, self.library_id, e.file_id))
        };
        let all_watched = entry.episodes.iter().all(|e| state.tree.file(e.file_id).is_some_and(|f| f.progress.watched));
        buttons.push(Button::MarkWatched(!all_watched));
        buttons.push(Button::Watchlist(!sh.on_watchlist(self.library_id, self.show_id)));
        buttons.push(Button::MatchOnline);
        buttons.push(Button::Folders);
        buttons
    }

    fn rows(&self, sh: &Shared, buttons: usize) -> Vec<(RowKind, usize)> {
        let entry = sh.lib(self.library_id).and_then(|l| l.catalog()?.show(self.show_id));
        let mut rows = vec![(RowKind::Buttons, buttons)];
        if let Some(e) = entry.filter(|e| e.is_series()) {
            rows.push((RowKind::Seasons, e.seasons().len()));
        }
        let cast = self.cast.as_ref().map_or(0, |(_, c)| c.len());
        if cast > 0 {
            rows.push((RowKind::Cast, cast));
        }
        rows
    }

    fn load_cast(&mut self, sh: &Shared) {
        let Some(state) = sh.lib(self.library_id) else { return };
        if self.cast.as_ref().is_some_and(|(v, _)| *v == state.version) {
            return;
        }
        let entry = state.catalog().and_then(|c| c.show(self.show_id));
        let (cast, voice) = match entry.and_then(|e| e.show.provider_id.clone()) {
            // Anime: the Japanese voice cast when known.
            Some(pid) => {
                let voice = sh.credits(&pid, "voice");
                if voice.is_empty() { (sh.credits(&pid, "cast"), false) } else { (voice, true) }
            }
            None => (Vec::new(), false),
        };
        self.voice = voice;
        self.cast = Some((state.version, cast));
    }
}

impl Screen for TitlePage {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        self.load_cast(sh);
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let buttons = self.buttons(sh);
        let rows = self.rows(sh, buttons.len());
        if press.is(Action::Menu) {
            // On a season card: options for that season; otherwise for the whole show.
            if let Some((RowKind::Seasons, i)) = self.nav.focused(&rows)
                && let Some(entry) = sh.lib(self.library_id).and_then(|l| l.catalog()?.show(self.show_id))
                && let Some(season) = entry.seasons().get(i).copied()
            {
                let ids = entry.season_episodes(season).map(|e| e.file_id).collect();
                let name = if season == 0 { "Specials".to_owned() } else { format!("Season {season}") };
                let label = format!("{} · {name}", entry.show.title);
                return Nav::push(OptionsMenu::new(Target::Files { library_id: self.library_id, ids, label }));
            }
            return Nav::push(ShowOptions::new(self.library_id, self.show_id));
        }
        if press.is(Action::PlayPause)
            && let Some(Button::Play { req, .. }) = buttons.first()
        {
            return Nav::Play { req: req.clone(), replace: false };
        }
        if let Some(nav) = self.nav.handle(press, &rows) {
            return nav;
        }
        if !press.is_activate() {
            return Nav::None;
        }
        match self.nav.focused(&rows) {
            Some((RowKind::Buttons, i)) => match buttons.get(i).cloned() {
                Some(Button::Play { req, .. }) => Nav::Play { req, replace: false },
                Some(Button::MarkWatched(w)) => {
                    let ids: Vec<i64> = sh
                        .lib(self.library_id)
                        .and_then(|l| l.catalog()?.show(self.show_id))
                        .map_or_else(Vec::new, |e| e.episodes.iter().map(|e| e.file_id).collect());
                    sh.set_watched(self.library_id, &ids, w);
                    Nav::None
                }
                Some(Button::Watchlist(on)) => {
                    sh.set_watchlist(self.library_id, self.show_id, on);
                    Nav::None
                }
                Some(Button::MatchOnline) => Nav::push(MatchOnline::new(sh, self.library_id, self.show_id)),
                Some(Button::Folders) => Nav::push(MatchInspector::new(self.library_id, self.show_id)),
                None => Nav::None,
            },
            Some((RowKind::Seasons, i)) => {
                let season = sh.lib(self.library_id).and_then(|l| l.catalog()?.show(self.show_id)).and_then(|e| e.seasons().get(i).copied());
                season.map_or(Nav::None, |s| Nav::push(SeasonView::new(self.library_id, self.show_id, s)))
            }
            Some((RowKind::Cast, _)) | None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.load_cast(sh);
        let Some(state) = sh.lib(self.library_id) else { return Nav::Pop };
        // The show may have been merged into another or split up.
        let Some(entry) = state.catalog().and_then(|c| c.show(self.show_id)).cloned() else { return Nav::Pop };
        // Season cards: (name, summary).
        let season_cards: Vec<(String, String)> = entry
            .seasons()
            .into_iter()
            .map(|s| {
                let (total, unwatched) = entry.season_episodes(s).fold((0, 0), |(t, u), e| {
                    (t + 1, u + usize::from(!state.tree.file(e.file_id).is_some_and(|f| f.progress.watched)))
                });
                let name = if s == 0 { "Specials".to_owned() } else { format!("Season {s}") };
                let sub = match unwatched {
                    0 => format!("{total} episodes · watched"),
                    u if u == total => format!("{total} episodes"),
                    u => format!("{total} episodes · {u} new"),
                };
                (name, sub)
            })
            .collect();
        let buttons = self.buttons(sh);
        let rows = self.rows(sh, buttons.len());
        let focus = if focused { self.nav.focused(&rows) } else { None };
        let style = ui.style().clone();

        // Vertical layout (relative to the content top), then scroll the focused row into view.
        let header_h = POSTER.y;
        let mut row_tops = Vec::new();
        let mut y = header_h + SECTION_GAP;
        for (kind, _) in &rows {
            match kind {
                RowKind::Buttons => row_tops.push((header_h - BUTTON_H, header_h)),
                RowKind::Seasons => {
                    row_tops.push((y, y + ROW_LABEL_H + SEASON_CARD.y));
                    y += ROW_LABEL_H + SEASON_CARD.y + SECTION_GAP;
                }
                RowKind::Cast => {
                    row_tops.push((y, y + ROW_LABEL_H + CAST_CARD.y));
                    y += ROW_LABEL_H + CAST_CARD.y + SECTION_GAP;
                }
            }
        }
        let content_h = y;
        if let Some(&(top, bottom)) = row_tops.get(self.nav.row) {
            // Show the whole header when the buttons are focused.
            let top = if self.nav.row == 0 { 0.0 } else { top };
            self.nav.scroll_to(top, bottom, rect.height(), content_h);
        }
        let origin = rect.min - vec2(0.0, self.nav.scroll);
        let painter = ui.painter().with_clip_rect(rect);

        // Backdrop behind the header, darkened for legibility.
        if let Some(path) = &entry.show.backdrop
            && let Some(t) = sh.images.get(path, Size::Backdrop)
        {
            let backdrop = Rect::from_min_size(origin, vec2(rect.width(), header_h));
            paint_cover(&painter, backdrop, &t);
            painter.rect_filled(backdrop, CornerRadius::same(8), Color32::from_black_alpha(205));
        }
        let poster_rect = Rect::from_min_size(origin, POSTER);
        match entry.show.poster.as_deref().and_then(|p| sh.images.get(p, Size::Poster)) {
            Some(t) => paint_cover(&painter, poster_rect, &t),
            None => {
                painter.rect_filled(poster_rect, CornerRadius::same(8), theme::ROW);
            }
        }

        // Title, facts, description.
        let text_left = poster_rect.right() + 44.0;
        let text_rect = Rect::from_min_max(pos2(text_left, origin.y + 8.0), pos2(rect.right() - 20.0, origin.y + header_h - BUTTON_H - 20.0));
        let clip = painter.with_clip_rect(text_rect.intersect(rect));
        let title = match entry.show.year {
            Some(y) => format!("{} ({y})", entry.show.title),
            None => entry.show.title.clone(),
        };
        clip.text(text_rect.left_top(), Align2::LEFT_TOP, title, TextStyle::Heading.resolve(&style), theme::TEXT);
        let mut facts = Vec::new();
        if entry.is_series() {
            let seasons = entry.seasons().iter().filter(|s| **s != 0).count();
            facts.push(if seasons == 1 { "1 season".to_owned() } else { format!("{seasons} seasons") });
            facts.push(format!("{} episodes", thousands(entry.episodes.len())));
        }
        if let Some(rt) = entry.show.runtime {
            facts.push(if rt >= 60 { format!("{} h {} min", rt / 60, rt % 60) } else { format!("{rt} min") });
        }
        if let Some(g) = &entry.show.genres {
            facts.push(g.clone());
        }
        if entry.show.state == "unmatched" {
            facts.push("not matched online".into());
        }
        clip.text(text_rect.left_top() + vec2(0.0, 72.0), Align2::LEFT_TOP, facts.join("  ·  "), TextStyle::Small.resolve(&style), theme::DIM);
        if let Some(overview) = entry.show.overview.as_deref().filter(|o| !o.is_empty()) {
            let r = Rect::from_min_max(text_rect.left_top() + vec2(0.0, 124.0), text_rect.max);
            let font = TextStyle::Body.resolve(&style);
            let galley = painter.layout(overview.to_owned(), font, theme::TEXT, r.width().min(1100.0));
            painter.with_clip_rect(r.intersect(rect)).galley(r.min, galley, theme::TEXT);
        }

        // Rows. `hovered` = (row, item) under the mouse.
        let mut hovered = None;
        let mut hit = |row: usize, i: usize, r: Rect| {
            if widgets::hit(ui, r.intersect(rect)).any() {
                hovered = Some((row, i));
            }
        };
        for (index, (kind, count)) in rows.iter().enumerate() {
            let (top, _) = row_tops[index];
            let row_focus = focus.filter(|(k, _)| k == kind).map(|(_, i)| i);
            match kind {
                RowKind::Buttons => {
                    let mut x = text_left;
                    for (i, b) in buttons.iter().enumerate() {
                        let label = b.label();
                        let w = button_width(&painter, &label, &style);
                        let r = Rect::from_min_size(pos2(x, origin.y + top), vec2(w, BUTTON_H));
                        hit(index, i, r);
                        paint_button(&painter, r, &label, row_focus == Some(i), i == 0 && matches!(b, Button::Play { .. }), &style);
                        x += w + 16.0;
                    }
                }
                RowKind::Seasons => {
                    painter.text(origin + vec2(0.0, top), Align2::LEFT_TOP, "Seasons", TextStyle::Body.resolve(&style), theme::TEXT);
                    let sel = self.nav.col[1];
                    hrow(&painter, Rect::from_min_size(origin + vec2(0.0, top + ROW_LABEL_H), vec2(rect.width(), 0.0)), SEASON_CARD, *count, sel, &mut self.nav.offset[1], |p, i, r, _| {
                        hit(index, i, r);
                        let (name, sub) = &season_cards[i];
                        let color = widgets::paint_focusable(p, r, row_focus == Some(i), false);
                        p.text(r.left_top() + vec2(20.0, 16.0), Align2::LEFT_TOP, name, TextStyle::Body.resolve(&style), color);
                        p.text(r.left_bottom() + vec2(20.0, -14.0), Align2::LEFT_BOTTOM, sub, TextStyle::Small.resolve(&style), color);
                    });
                }
                RowKind::Cast => {
                    let label = if self.voice { "Voice cast (Japanese)" } else { "Cast" };
                    painter.text(origin + vec2(0.0, top), Align2::LEFT_TOP, label, TextStyle::Body.resolve(&style), theme::TEXT);
                    let cast = self.cast.as_ref().map(|(_, c)| c.clone()).unwrap_or_default();
                    let sel = self.nav.col[2];
                    let images = &mut sh.images;
                    hrow(&painter, Rect::from_min_size(origin + vec2(0.0, top + ROW_LABEL_H), vec2(rect.width(), 0.0)), CAST_CARD, *count, sel, &mut self.nav.offset[2], |p, i, r, _| {
                        hit(index, i, r);
                        let member = &cast[i];
                        let photo = member.image.as_deref().and_then(|img| images.get(img, Size::Profile));
                        paint_cast(p, r, member, photo.as_ref(), row_focus == Some(i), &style);
                    });
                }
            }
        }
        if let Some((row, i)) = hovered {
            self.nav.row = row;
            self.nav.col[PageNav::kind_index(rows[row].0)] = i;
        }
        if ui.rect_contains_pointer(rect) {
            self.nav.scroll -= ui.input(|i| i.smooth_scroll_delta.y);
        }
        Nav::None
    }
}

/// One episode's page.
pub struct EpisodePage {
    library_id: i64,
    file_id: i64,
    nav: PageNav,
}

impl EpisodePage {
    pub fn new(library_id: i64, file_id: i64) -> Self {
        Self { library_id, file_id, nav: PageNav::default() }
    }

    fn buttons(&self, sh: &Shared) -> Vec<Button> {
        let mut buttons = play_buttons(sh, self.library_id, self.file_id);
        let watched = sh.progress(self.library_id, self.file_id).is_some_and(|p| p.watched);
        buttons.push(Button::MarkWatched(!watched));
        buttons
    }
}

const STILL: egui::Vec2 = egui::vec2(640.0, 360.0);

impl Screen for EpisodePage {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let buttons = self.buttons(sh);
        if press.is(Action::PlayPause)
            && let Some(Button::Play { req, .. }) = buttons.first()
        {
            return Nav::Play { req: req.clone(), replace: false };
        }
        if press.is(Action::Menu) {
            let target = Target::File { library_id: self.library_id, file_id: self.file_id };
            return Nav::push(OptionsMenu::new(target));
        }
        if let Some(nav) = self.nav.handle(press, &[(RowKind::Buttons, buttons.len())]) {
            return nav;
        }
        if !press.is_activate() {
            return Nav::None;
        }
        match buttons.get(self.nav.col[0]).cloned() {
            Some(Button::Play { req, .. }) => Nav::Play { req, replace: false },
            Some(Button::MarkWatched(w)) => {
                sh.set_watched(self.library_id, &[self.file_id], w);
                Nav::None
            }
            _ => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(titles) = sh.titles(self.library_id, self.file_id) else { return Nav::Pop };
        let meta = sh.episode_meta(self.library_id, self.file_id).cloned();
        let buttons = self.buttons(sh);
        let style = ui.style().clone();
        let painter = ui.painter().with_clip_rect(rect);

        let still_rect = Rect::from_min_size(rect.min, STILL);
        match meta.as_ref().and_then(|m| m.still.clone()).and_then(|p| sh.images.get(&p, Size::Still)) {
            Some(t) => paint_cover(&painter, still_rect, &t),
            None => {
                painter.rect_filled(still_rect, CornerRadius::same(8), theme::ROW);
            }
        }
        if let Some(fraction) = sh.progress(self.library_id, self.file_id).and_then(|p| p.fraction()) {
            widgets::paint_progress(&painter, still_rect, fraction as f32, theme::FOCUS);
        }
        let left = still_rect.right() + 44.0;
        let text = Rect::from_min_max(pos2(left, rect.top()), pos2(rect.right(), still_rect.bottom()));
        let clip = painter.with_clip_rect(text);
        let code = titles.episode.as_deref().map(|e| e.split(" · ").next().unwrap_or(e).to_owned()).unwrap_or_default();
        let show_line = match &titles.show {
            Some(show) if !code.is_empty() => format!("{show} · {code}"),
            Some(show) => show.clone(),
            None => String::new(),
        };
        clip.text(text.left_top(), Align2::LEFT_TOP, show_line, TextStyle::Body.resolve(&style), theme::DIM);
        clip.text(text.left_top() + vec2(0.0, 46.0), Align2::LEFT_TOP, &titles.name, TextStyle::Heading.resolve(&style), theme::TEXT);
        let mut facts = Vec::new();
        if let Some(m) = &meta {
            facts.extend(m.air_date.clone());
            facts.extend(m.runtime.map(|r| format!("{r} min")));
        }
        if let Some(p) = sh.progress(self.library_id, self.file_id) {
            if p.watched {
                facts.push("Watched".into());
            } else if let Some(left) = p.remaining_secs() {
                facts.push(theme::format_remaining(left));
            }
        }
        clip.text(text.left_top() + vec2(0.0, 120.0), Align2::LEFT_TOP, facts.join("  ·  "), TextStyle::Small.resolve(&style), theme::DIM);
        if let Some(overview) = meta.as_ref().map(|m| m.overview.clone()).filter(|o| !o.is_empty()) {
            let r = Rect::from_min_max(text.left_top() + vec2(0.0, 170.0), text.max);
            let galley = painter.layout(overview, TextStyle::Body.resolve(&style), theme::TEXT, r.width().min(1000.0));
            painter.with_clip_rect(r).galley(r.min, galley, theme::TEXT);
        }
        let mut x = rect.left();
        let y = still_rect.bottom() + 40.0;
        for (i, b) in buttons.iter().enumerate() {
            let label = b.label();
            let w = button_width(&painter, &label, &style);
            let r = Rect::from_min_size(pos2(x, y), vec2(w, BUTTON_H));
            if widgets::hit(ui, r.intersect(rect)).any() {
                self.nav.col[0] = i;
            }
            paint_button(&painter, r, &label, focused && self.nav.col[0] == i, i == 0, &style);
            x += w + 16.0;
        }
        Nav::None
    }
}

/// Opens the right page for a file: the episode's page, the film's page, or (for files not
/// organised into a show) plays it.
pub fn open_file(sh: &Shared, library_id: i64, file_id: i64) -> Nav {
    let show = sh.lib(library_id).and_then(|l| l.catalog()?.show_of_file(file_id));
    match show {
        Some(s) if s.is_series() => Nav::push(EpisodePage::new(library_id, file_id)),
        Some(s) => Nav::push(TitlePage::new(library_id, s.show.id)),
        None => Nav::play(sh.play_request(library_id, file_id)),
    }
}

/// The page of the show or film a file belongs to, for when that isn't known yet (shows
/// are still being sorted just after starting). Becomes the page once it's known, or goes
/// away if the file isn't part of one.
pub struct TitleOf {
    library_id: i64,
    file_id: i64,
}

impl TitleOf {
    pub fn new(library_id: i64, file_id: i64) -> Self {
        Self { library_id, file_id }
    }

    /// The page to show, `Some(None)` if there isn't one, or `None` while still sorting.
    fn resolve(&self, sh: &Shared) -> Option<Option<TitlePage>> {
        let state = sh.lib(self.library_id)?;
        let catalog = state.catalog()?;
        Some(catalog.show_of_file(self.file_id).map(|s| TitlePage::new(self.library_id, s.show.id)))
    }
}

impl Screen for TitleOf {
    fn handle(&mut self, _sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) { Nav::Pop } else { Nav::None }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, _focused: bool) -> Nav {
        match self.resolve(sh) {
            Some(Some(page)) => Nav::replace(page),
            Some(None) => Nav::Pop,
            None => {
                widgets::message(ui.painter(), rect, "Sorting shows…", ui.style());
                Nav::None
            }
        }
    }
}
