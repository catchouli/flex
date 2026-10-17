//! Home: a Continue Watching row (when there's anything to continue: library files and,
//! with YouTube on, part-watched videos) above a grid of library cards.

use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Style, TextStyle, TextureHandle, Ui, pos2, vec2};
use flex_core::library::display_name;
use flex_core::progress::{ContinueItem, ContinueKind};
use flex_core::youtube::store::WatchProgress;

use super::options::{OptionsMenu, Target};
use super::browse::{ShowsView, Source};
use super::details::{TitleOf, TitlePage};
use super::{Nav, Screen, library::LibraryView, picker::FolderPicker};
use crate::images::{Size, paint_cover};
use crate::input::{Action, Press};
use crate::shared::{PlayRequest, Shared};
use crate::shots::{self, Shot};
use crate::{theme, widgets};

const LIBRARY_CARD: [f32; 2] = [520.0, 240.0];
const CONTINUE_CARD: [f32; 2] = [520.0, 200.0];
const GAP: f32 = 28.0;
const SECTION_GAP: f32 = 48.0;
const MAX_CONTINUE: usize = 20;
/// Height of a section heading (as laid out by `widgets::header` without a subtitle).
const HEADER_H: f32 = 80.0;

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

/// A Continue Watching card.
#[derive(Clone, PartialEq)]
enum Entry {
    Library(ContinueItem),
    YouTube { id: String, progress: WatchProgress },
}

impl Entry {
    fn last_played(&self) -> i64 {
        match self {
            Entry::Library(i) => i.last_played,
            Entry::YouTube { progress, .. } => progress.updated_at,
        }
    }

    /// Its key for `Shared::hide_continue` (also identifies it across refreshes).
    fn key(&self) -> String {
        match self {
            Entry::Library(i) => i.hide_key(),
            Entry::YouTube { id, .. } => format!("yt:{id}"),
        }
    }
}

pub struct Home {
    section: Section,
    /// Selected index in each section.
    cont: usize,
    lib: usize,
    /// First visible Continue Watching card (the row scrolls sideways).
    cont_offset: usize,
    lib_columns: usize,
    items: Vec<Entry>,
    /// Library versions (then YouTube's and Continue Watching's) the items were computed for.
    items_key: Option<Vec<(i64, u64)>>,
    /// The YouTube video last asked to be prepared (when its card was selected).
    prepared: Option<String>,
    /// Vertical scroll, and whether to bring the selection into view on the next frame
    /// (set when the selection moves, so mouse-wheel scrolling isn't undone).
    scroll: f32,
    reveal_selection: bool,
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
            prepared: None,
            scroll: 0.0,
            reveal_selection: true,
        }
    }
}

fn describe(sh: &Shared, entry: &Entry) -> Option<CardText> {
    let item = match entry {
        Entry::Library(item) => item,
        Entry::YouTube { progress, .. } => {
            let left = match (progress.position, progress.duration) {
                (Some(pos), Some(dur)) => theme::format_remaining(dur - pos),
                _ => String::new(),
            };
            let fraction = progress.position.zip(progress.duration).filter(|(_, d)| *d > 0.0).map(|(p, d)| p / d);
            return Some(CardText { show: progress.title.clone(), episode: progress.channel.clone(), status: left, fraction });
        }
    };
    let state = sh.lib(item.library_id)?;
    let file = state.tree.file(item.file_id)?;
    let titles = sh.titles(item.library_id, item.file_id)?;
    // The show's (matched) title once organised; the top folder name until then.
    let show = match (&titles.show, file.rel_path.split_once('/')) {
        (Some(s), _) => s.clone(),
        (None, Some((top, _))) => display_name(top, false),
        (None, None) => titles.name.clone(), // a movie at the library root
    };
    let episode = titles.episode.unwrap_or_else(|| if titles.show.is_none() { titles.name } else { String::new() });
    let (status, fraction) = match item.kind {
        ContinueKind::Resume => {
            let left = file.progress.remaining_secs().map_or_else(String::new, theme::format_remaining);
            (left, file.progress.fraction())
        }
        ContinueKind::Next => ("Up next".to_owned(), None),
    };
    Some(CardText { show, episode, status, fraction })
}

/// The picture for a Continue Watching card, as shown elsewhere for what it'll play: an
/// episode's TMDB still or a film's backdrop, else a screenshot from the video itself.
fn card_image(sh: &mut Shared, entry: &Entry) -> Option<TextureHandle> {
    let item = match entry {
        Entry::Library(item) => item,
        Entry::YouTube { id, .. } => return sh.images.get(&flex_core::youtube::feeds::thumbnail(id), Size::Still),
    };
    let entry = sh.lib(item.library_id)?.catalog()?.show_of_file(item.file_id);
    let tmdb = match entry {
        Some(e) if e.is_series() => sh.episode_meta(item.library_id, item.file_id).and_then(|m| m.still.clone()).map(|p| (p, Size::Still)),
        Some(e) => e.show.backdrop.clone().map(|p| (p, Size::Backdrop)),
        None => None,
    };
    match tmdb {
        Some((path, size)) => sh.images.get(&path, size),
        None => match sh.shot(item.library_id, item.file_id, shots::THUMB) {
            Shot::Ready(path) => sh.images.get(&path, Size::Shot),
            _ => None,
        },
    }
}

fn continue_card(ui: &Ui, rect: Rect, text: &CardText, image: Option<&TextureHandle>, focused: bool, style: &Style) {
    let painter = ui.painter();
    let color = match image {
        Some(t) => {
            // The picture, darkened so the text reads over it.
            paint_cover(painter, rect, t);
            painter.rect_filled(rect, CornerRadius::same(8), Color32::from_black_alpha(if focused { 140 } else { 175 }));
            if focused {
                painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(5.0, theme::FOCUS), StrokeKind::Inside);
            }
            theme::TEXT
        }
        None => widgets::paint_focusable(painter, rect, focused, false),
    };
    let clip = painter.with_clip_rect(rect.shrink(8.0));
    let inner = rect.shrink(24.0);
    clip.text(inner.left_top(), Align2::LEFT_TOP, &text.show, TextStyle::Body.resolve(style), color);
    clip.text(inner.left_top() + vec2(0.0, 44.0), Align2::LEFT_TOP, &text.episode, TextStyle::Small.resolve(style), color);
    clip.text(inner.left_bottom() - vec2(0.0, 8.0), Align2::LEFT_BOTTOM, &text.status, TextStyle::Small.resolve(style), color);
    if let Some(f) = text.fraction {
        let bar = if focused && image.is_none() { Color32::BLACK } else { theme::FOCUS };
        widgets::paint_progress(painter, rect, f as f32, bar);
    }
}

impl Home {
    fn refresh(&mut self, sh: &Shared) {
        let mut key: Vec<(i64, u64)> = sh.libs.iter().map(|l| (l.lib.id, l.version)).collect();
        key.push((-1, sh.continue_version));
        if sh.youtube_enabled() {
            key.push((crate::shared::YOUTUBE_LIBRARY, sh.youtube.version));
        }
        if self.items_key.as_ref() != Some(&key) {
            let was_empty = self.items.is_empty();
            let selected = self.items.get(self.cont).map(Entry::key);
            let mut items: Vec<Entry> = sh.continue_watching(MAX_CONTINUE).into_iter().map(Entry::Library).collect();
            if sh.youtube_enabled() {
                items.extend(sh.yt_continue(MAX_CONTINUE).into_iter().map(|(id, progress)| Entry::YouTube { id, progress }));
            }
            // Library entries come most recent first; slot YouTube in by when it was played.
            items.sort_by_key(|e| std::cmp::Reverse(e.last_played()));
            items.truncate(MAX_CONTINUE);
            self.items = items;
            self.items_key = Some(key);
            // Keep the item you moved to selected when the row reorders; on the first card
            // (where Home starts), stay on the first card, e.g. once shows are sorted.
            if self.cont > 0
                && let Some(i) = selected.and_then(|k| self.items.iter().position(|e| e.key() == k))
            {
                self.cont = i;
            }
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
        self.lib = self.lib.min(sh.libs.len().max(1) + usize::from(sh.youtube_enabled()));
    }

    fn handle_continue(&mut self, sh: &Shared, press: Press) -> Nav {
        let entry = self.items.get(self.cont).cloned();
        match press.action {
            Action::Left => self.cont = self.cont.saturating_sub(1),
            Action::Right => self.cont = (self.cont + 1).min(self.items.len().saturating_sub(1)),
            Action::Up if !press.repeat => return Nav::FocusTopBar,
            Action::Down => self.section = Section::Libraries,
            Action::Menu if !press.repeat => match &entry {
                Some(e @ Entry::Library(i)) => {
                    let target = Target::File { library_id: i.library_id, file_id: i.file_id };
                    return Nav::push(OptionsMenu::from_continue(target, e.key()));
                }
                Some(Entry::YouTube { id, progress }) => {
                    return Nav::push(super::youtube::VideoOptions::continuing(id, &progress.title, &progress.channel));
                }
                None => {}
            },
            _ if press.is_activate() => {
                let i = match entry {
                    Some(Entry::Library(i)) => i,
                    Some(Entry::YouTube { id, progress }) => {
                        return Nav::Play { req: PlayRequest::youtube(&id, &progress.title, None), replace: false };
                    }
                    None => return Nav::None,
                };
                let Some(req) = sh.play_request(i.library_id, i.file_id) else { return Nav::None };
                // Back from the player returns to the show or film's page rather than Home.
                let screen: Box<dyn Screen> = match sh.lib(i.library_id).and_then(|l| l.catalog()?.show_of_file(i.file_id)) {
                    Some(s) => Box::new(TitlePage::new(i.library_id, s.show.id)),
                    // Shows are still being sorted: the page appears once they are.
                    None => Box::new(TitleOf::new(i.library_id, i.file_id)),
                };
                return Nav::PlayOver { screen, req };
            }
            _ => {}
        }
        Nav::None
    }

    fn handle_libraries(&mut self, sh: &Shared, press: Press) -> Nav {
        // The Watchlist card, YouTube (when on), then each library (or an "Add library"
        // card when there are none).
        let yt = usize::from(sh.youtube_enabled());
        let count = 1 + yt + sh.libs.len().max(1);
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
            _ if press.is_activate() && self.lib == 0 => {
                return Nav::push(ShowsView::with_source(sh, Source::Watchlist));
            }
            _ if press.is_activate() && yt == 1 && self.lib == 1 => {
                return Nav::push(super::youtube::YouTubeHome::default());
            }
            _ if press.is_activate() => {
                return match sh.libs.get(self.lib - 1 - yt) {
                    Some(l) if l.is_music() => Nav::push(super::music::AlbumsView::new(sh, l.lib.id)),
                    Some(l) => match sh.library_view(l.lib.id) {
                        crate::shared::LibraryView::Shows => Nav::push(ShowsView::new(sh, l.lib.id)),
                        crate::shared::LibraryView::Folders => Nav::push(LibraryView::new(l.lib.id, String::new())),
                    },
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
        self.reveal_selection = true;
        match self.section {
            Section::Continue => self.handle_continue(sh, press),
            Section::Libraries => self.handle_libraries(sh, press),
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let style = ui.style().clone();

        // Lay out vertically (relative to the top of the content), then scroll so the
        // selection is visible, then draw clipped to `rect`.
        let lib_columns = (((rect.width() + GAP) / (LIBRARY_CARD[0] + GAP)) as usize).max(1);
        let cont_h = if self.items.is_empty() { 0.0 } else { HEADER_H + CONTINUE_CARD[1] + SECTION_GAP };
        let yt = usize::from(sh.youtube_enabled());
        let lib_rows = (1 + yt + sh.libs.len().max(1)).div_ceil(lib_columns);
        let content_h = cont_h + HEADER_H + lib_rows as f32 * (LIBRARY_CARD[1] + GAP);
        if std::mem::take(&mut self.reveal_selection) {
            let (sel_top, sel_bottom) = match self.section {
                Section::Continue => (0.0, cont_h),
                Section::Libraries => {
                    let row = self.lib / lib_columns;
                    let card_top = cont_h + HEADER_H + row as f32 * (LIBRARY_CARD[1] + GAP);
                    (if row == 0 { cont_h } else { card_top }, card_top + LIBRARY_CARD[1])
                }
            };
            if sel_top < self.scroll {
                self.scroll = sel_top;
            } else if sel_bottom > self.scroll + rect.height() {
                self.scroll = sel_bottom - rect.height() + GAP;
            }
        }
        if ui.rect_contains_pointer(rect) {
            self.scroll -= ui.input(|i| i.smooth_scroll_delta.y);
        }
        self.scroll = self.scroll.clamp(0.0, (content_h - rect.height()).max(0.0));

        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        child.set_clip_rect(rect);
        let ui = &mut child;
        let mut top = rect.top() - self.scroll;

        if !self.items.is_empty() {
            let area = Rect::from_min_max(pos2(rect.left(), top), pos2(rect.right(), top + content_h));
            let body = widgets::header(ui.painter(), area, "Continue Watching", "", &style);
            let visible = (((body.width() + GAP) / (CONTINUE_CARD[0] + GAP)) as usize).max(1);
            // Scroll sideways to keep the selected card in view.
            if self.cont < self.cont_offset {
                self.cont_offset = self.cont;
            } else if self.cont >= self.cont_offset + visible {
                self.cont_offset = self.cont + 1 - visible;
            }
            let count = self.items.len();
            let shown = (count - self.cont_offset).min(visible);
            let row = Rect::from_min_size(body.min, vec2(shown as f32 * (CONTINUE_CARD[0] + GAP) - GAP, CONTINUE_CARD[1]));
            let hints = widgets::ScrollHints::new(ui, row, self.cont_offset > 0, self.cont_offset + visible < count);
            for (slot, i) in (self.cont_offset..self.items.len()).take(visible).enumerate() {
                let Some(text) = describe(sh, &self.items[i]) else { continue };
                let min = Pos2::new(body.left() + slot as f32 * (CONTINUE_CARD[0] + GAP), body.top());
                if widgets::hit(ui, Rect::from_min_size(min, CONTINUE_CARD.into())).any() {
                    (self.section, self.cont) = (Section::Continue, i);
                }
                let selected = focused && self.section == Section::Continue && i == self.cont;
                let image = card_image(sh, &self.items[i]);
                continue_card(ui, Rect::from_min_size(min, CONTINUE_CARD.into()), &text, image.as_ref(), selected, &style);
            }
            // A selected YouTube card starts finding its streams, so OK plays sooner.
            if focused
                && self.section == Section::Continue
                && let Some(Entry::YouTube { id, .. }) = self.items.get(self.cont)
                && self.prepared.as_deref() != Some(id)
            {
                self.prepared = Some(id.clone());
                sh.prepare_youtube(id);
            }
            hints.paint(ui.painter());
            // An arrow selects the first hidden card that way and scrolls a page.
            match hints.clicked {
                Some(widgets::Side::Left) => {
                    self.cont = self.cont_offset - 1;
                    self.cont_offset = self.cont_offset.saturating_sub(visible);
                    self.section = Section::Continue;
                }
                Some(widgets::Side::Right) => {
                    self.cont = self.cont_offset + visible;
                    self.cont_offset = (self.cont_offset + visible).min(count - visible);
                    self.section = Section::Continue;
                }
                None => {}
            }
            top = body.top() + CONTINUE_CARD[1] + SECTION_GAP;
        }

        let painter = ui.painter();
        let area = Rect::from_min_max(pos2(rect.left(), top), pos2(rect.right(), top + content_h));
        let body = widgets::header(painter, area, "Libraries", "", &style);
        let cols = (((body.width() + GAP) / (LIBRARY_CARD[0] + GAP)) as usize).max(1);
        self.lib_columns = cols;
        let card_rect = |i: usize| {
            let (row, col) = (i / cols, i % cols);
            let min = pos2(body.left() + col as f32 * (LIBRARY_CARD[0] + GAP), body.top() + row as f32 * (LIBRARY_CARD[1] + GAP));
            Rect::from_min_size(min, LIBRARY_CARD.into())
        };
        for i in 0..1 + yt + sh.libs.len().max(1) {
            if widgets::hit(ui, card_rect(i)).any() {
                (self.section, self.lib) = (Section::Libraries, i);
            }
        }
        let libs_focused = focused && self.section == Section::Libraries;
        let listed = sh.watchlist_shows().len();
        let count = match listed {
            0 => "Nothing added yet".to_owned(),
            1 => "1 title".to_owned(),
            n => format!("{n} titles"),
        };
        widgets::card(painter, card_rect(0), "Watchlist", &["Shows and films to watch", &count], libs_focused && self.lib == 0, &style);
        if yt == 1 {
            let status = if sh.youtube.tools.is_none() { "Setting up…" } else { "Subscriptions and search" };
            widgets::card(painter, card_rect(1), "YouTube", &["No ads, sponsors skipped", status], libs_focused && self.lib == 1, &style);
        }
        if sh.libs.is_empty() {
            let i = 1 + yt;
            widgets::card(painter, card_rect(i), "Add library", &["Pick a folder to get started"], libs_focused && self.lib == i, &style);
        }
        for (i, l) in sh.libs.iter().enumerate() {
            let status = l.status();
            let lines = [l.lib.kind.label(), status.as_str()];
            let i = i + 1 + yt;
            widgets::card(painter, card_rect(i), &l.lib.name, &lines, libs_focused && i == self.lib, &style);
        }
        Nav::None
    }
}
