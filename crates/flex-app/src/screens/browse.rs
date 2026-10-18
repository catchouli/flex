//! A library's shows and films, or the watchlist: a poster grid (or list) with a toolbar
//! for the view, sort order and watched filter. Each remembers its own choices.
//!
//! Up from the top row reaches the toolbar; OK on a setting opens a small dropdown.

use eframe::egui::{self, Align2, Color32, CornerRadius, Painter, Rect, TextStyle, Ui, pos2, vec2};
use flex_core::natural::natural_cmp;

use super::history::HistoryView;
use super::library::LibraryView;
use super::shows::{ShowOptions, open_show};
use super::{Nav, Screen};
use crate::images::{Size, paint_cover};
use crate::input::{Action, Press};
use crate::shared::{LibState, Shared, thousands};
use crate::theme;
use crate::widgets::{self, GridSpec, GridState, Icon, ListInput, ListState, Row};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Posters,
    List,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Added,
    Played,
    Title,
    Released,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Unwatched,
    Watched,
    /// Not found online (or not looked up yet): to match by hand.
    Unmatched,
}

const LAYOUTS: [(Layout, &str, &str); 2] = [(Layout::Posters, "Posters", "posters"), (Layout::List, "List", "list")];
const SORTS: [(Sort, &str, &str); 4] = [
    (Sort::Added, "Recently added", "added"),
    (Sort::Played, "Recently played", "played"),
    (Sort::Title, "A–Z", "title"),
    (Sort::Released, "Release date", "released"),
];
const FILTERS: [(Filter, &str, &str); 4] = [
    (Filter::All, "All", "all"),
    (Filter::Unwatched, "Unwatched", "unwatched"),
    (Filter::Watched, "Watched", "watched"),
    (Filter::Unmatched, "Not matched", "unmatched"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
struct Prefs {
    layout: Layout,
    sort: Sort,
    filter: Filter,
}

/// What the screen shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Library(i64),
    /// Every library's watchlisted shows and films; "Recently added" = added to the list.
    Watchlist,
}

impl Source {
    fn key(self) -> String {
        match self {
            Source::Library(id) => format!("browse:{id}"),
            Source::Watchlist => "browse:watchlist".to_owned(),
        }
    }
}

impl Prefs {
    /// Stored as "posters|added|all".
    fn load(sh: &Shared, source: Source) -> Self {
        let saved = sh.setting(&source.key()).unwrap_or_default();
        let mut parts = saved.split('|');
        let pick = |part: Option<&str>| part.unwrap_or_default().to_owned();
        let (l, s, f) = (pick(parts.next()), pick(parts.next()), pick(parts.next()));
        Self {
            layout: LAYOUTS.iter().find(|x| x.2 == l).map_or(Layout::Posters, |x| x.0),
            sort: SORTS.iter().find(|x| x.2 == s).map_or(Sort::Added, |x| x.0),
            filter: FILTERS.iter().find(|x| x.2 == f).map_or(Filter::All, |x| x.0),
        }
    }

    fn save(&self, sh: &mut Shared, source: Source) {
        let l = LAYOUTS.iter().find(|x| x.0 == self.layout).map_or("posters", |x| x.2);
        let s = SORTS.iter().find(|x| x.0 == self.sort).map_or("added", |x| x.2);
        let f = FILTERS.iter().find(|x| x.0 == self.filter).map_or("all", |x| x.2);
        sh.set_setting(&source.key(), &format!("{l}|{s}|{f}"));
    }
}

/// Toolbar buttons, left to right.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    View,
    Sort,
    Show,
    Folders,
    History,
}

const TOOLS: [Tool; 5] = [Tool::View, Tool::Sort, Tool::Show, Tool::Folders, Tool::History];

impl Tool {
    fn label(self, prefs: &Prefs) -> String {
        match self {
            Tool::View => format!("View: {}", LAYOUTS.iter().find(|x| x.0 == prefs.layout).map_or("", |x| x.1)),
            Tool::Sort => format!("Sort: {}", SORTS.iter().find(|x| x.0 == prefs.sort).map_or("", |x| x.1)),
            Tool::Show => format!("Show: {}", FILTERS.iter().find(|x| x.0 == prefs.filter).map_or("", |x| x.1)),
            Tool::Folders => "Browse folders".to_owned(),
            Tool::History => "History".to_owned(),
        }
    }

    /// Choices in this tool's dropdown, and which is current.
    fn options(self, prefs: &Prefs) -> (Vec<&'static str>, usize) {
        match self {
            Tool::View => (LAYOUTS.iter().map(|x| x.1).collect(), LAYOUTS.iter().position(|x| x.0 == prefs.layout).unwrap_or(0)),
            Tool::Sort => (SORTS.iter().map(|x| x.1).collect(), SORTS.iter().position(|x| x.0 == prefs.sort).unwrap_or(0)),
            Tool::Show => (FILTERS.iter().map(|x| x.1).collect(), FILTERS.iter().position(|x| x.0 == prefs.filter).unwrap_or(0)),
            Tool::Folders | Tool::History => (Vec::new(), 0),
        }
    }

    fn apply(self, prefs: &mut Prefs, choice: usize) {
        match self {
            Tool::View => prefs.layout = LAYOUTS[choice].0,
            Tool::Sort => prefs.sort = SORTS[choice].0,
            Tool::Show => prefs.filter = FILTERS[choice].0,
            Tool::Folders | Tool::History => {}
        }
    }
}

/// One show or film as shown in the grid/list.
struct Item {
    library_id: i64,
    show_id: i64,
    title: String,
    year: Option<i32>,
    poster: Option<String>,
    series: bool,
    episodes: usize,
    unwatched: usize,
    /// Partly watched film: fraction.
    progress: Option<f64>,
    unmatched: bool,
}

fn build_items(sh: &Shared, source: Source, prefs: &Prefs) -> Vec<Item> {
    // (library, show, watchlist "added" time)
    let entries: Vec<(&LibState, &flex_core::catalog::ShowEntry, Option<i64>)> = match source {
        Source::Library(id) => {
            let Some(state) = sh.lib(id) else { return Vec::new() };
            let Some(catalog) = state.catalog() else { return Vec::new() };
            catalog.shows.iter().map(|e| (state, e, None)).collect()
        }
        Source::Watchlist => sh
            .watchlist_shows()
            .into_iter()
            .filter_map(|(lib, show, added)| {
                let state = sh.lib(lib)?;
                Some((state, state.catalog()?.show(show)?, Some(added)))
            })
            .collect(),
    };
    // (item, added, released, last played)
    let mut items: Vec<(Item, i64, String, i64)> = entries
        .into_iter()
        .filter_map(|(state, entry, listed)| {
            let files: Vec<_> = entry.episodes.iter().filter_map(|e| state.tree.file(e.file_id)).collect();
            let unwatched = files.iter().filter(|f| !f.progress.watched).count();
            let keep = match prefs.filter {
                Filter::All => true,
                Filter::Unwatched => unwatched > 0,
                Filter::Watched => unwatched == 0,
                Filter::Unmatched => entry.show.provider_id.is_none(),
            };
            if !keep {
                return None;
            }
            let added = listed.unwrap_or_else(|| files.iter().map(|f| f.added_at).max().unwrap_or(0));
            // "2019-04-01"-style dates sort as text; fall back to the year.
            let released = entry.show.release_date.clone().or_else(|| entry.show.year.map(|y| format!("{y:04}"))).unwrap_or_default();
            let series = entry.is_series();
            let progress = if series { None } else { files.first().and_then(|f| f.progress.fraction()) };
            let played = files.iter().filter_map(|f| f.progress.last_played).max().unwrap_or(0);
            Some((
                Item {
                    library_id: state.lib.id,
                    show_id: entry.show.id,
                    title: entry.show.title.clone(),
                    year: entry.show.year,
                    poster: entry.show.poster.clone(),
                    series,
                    episodes: files.len(),
                    unwatched,
                    progress,
                    unmatched: entry.show.state == "unmatched",
                },
                added,
                released,
                played,
            ))
        })
        .collect();
    match prefs.sort {
        Sort::Added => items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| natural_cmp(&a.0.title, &b.0.title))),
        Sort::Played => items.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| natural_cmp(&a.0.title, &b.0.title))),
        Sort::Title => items.sort_by(|a, b| natural_cmp(&a.0.title, &b.0.title)),
        // Newest first; undated last.
        Sort::Released => items.sort_by(|a, b| {
            a.2.is_empty().cmp(&b.2.is_empty()).then_with(|| b.2.cmp(&a.2)).then_with(|| natural_cmp(&a.0.title, &b.0.title))
        }),
    }
    items.into_iter().map(|(item, ..)| item).collect()
}

/// Poster card size (2:3 poster plus two lines of text).
const CARD_W: f32 = 230.0;
const POSTER_H: f32 = CARD_W * 1.5;
const GRID: GridSpec = GridSpec { card: egui::vec2(CARD_W, POSTER_H + 76.0), gap: 26.0 };
const TOOLBAR_H: f32 = 64.0;

pub struct ShowsView {
    source: Source,
    prefs: Prefs,
    /// `Some(i)` = toolbar button i has focus; `None` = the grid/list.
    toolbar: Option<usize>,
    /// An open dropdown: which tool, and the highlighted choice.
    popup: Option<(Tool, ListState)>,
    grid: GridState,
    list: ListState,
    items: Vec<Item>,
    /// (library version, prefs) the items were built for.
    built_for: Option<(u64, Prefs)>,
    /// Left edge of each toolbar button at the last draw, to place its dropdown.
    tool_x: [f32; TOOLS.len()],
}

impl ShowsView {
    pub fn new(sh: &Shared, library_id: i64) -> Self {
        Self::with_source(sh, Source::Library(library_id))
    }

    pub fn with_source(sh: &Shared, source: Source) -> Self {
        Self {
            source,
            prefs: Prefs::load(sh, source),
            toolbar: None,
            popup: None,
            grid: GridState::default(),
            list: ListState::default(),
            items: Vec::new(),
            built_for: None,
            tool_x: [0.0; TOOLS.len()],
        }
    }

    /// Toolbar buttons: the watchlist has no folders or history of its own.
    fn tools(&self) -> &'static [Tool] {
        match self.source {
            Source::Library(_) => &TOOLS,
            Source::Watchlist => &TOOLS[..3],
        }
    }

    fn refresh(&mut self, sh: &Shared) {
        let version = match self.source {
            Source::Library(id) => sh.lib(id).map_or(0, |l| l.version),
            Source::Watchlist => sh.libs.iter().map(|l| l.version).sum::<u64>() + sh.watchlist_version,
        };
        let key = (version, self.prefs);
        if self.built_for != Some(key) {
            // Keep the same show selected across rebuilds when possible.
            // If it's gone (e.g. just matched, under "Not matched"), stay at the same place.
            let index = self.selected_index();
            let selected = index.and_then(|i| self.items.get(i)).map(|it| it.show_id);
            let same_prefs = self.built_for.is_some_and(|(_, p)| p == self.prefs);
            self.items = build_items(sh, self.source, &self.prefs);
            let new_index = match selected.and_then(|id| self.items.iter().position(|it| it.show_id == id)) {
                Some(i) => i,
                None if same_prefs => index.unwrap_or(0).min(self.items.len().saturating_sub(1)),
                None => 0,
            };
            self.grid.select(new_index);
            self.list.select(new_index);
            self.built_for = Some(key);
        }
    }

    fn selected_index(&self) -> Option<usize> {
        let i = match self.prefs.layout {
            Layout::Posters => self.grid.selected,
            Layout::List => self.list.selected,
        };
        (i < self.items.len()).then_some(i)
    }

    fn activate_tool(&mut self, tool: Tool) -> Nav {
        let library = match self.source {
            Source::Library(id) => Some(id),
            Source::Watchlist => None,
        };
        match (tool, library) {
            (Tool::Folders, Some(id)) => Nav::push(LibraryView::new(id, String::new())),
            (Tool::History, Some(id)) => Nav::push(HistoryView::new(id)),
            (Tool::Folders | Tool::History, None) => Nav::None,
            _ => {
                let (_, current) = tool.options(&self.prefs);
                let mut list = ListState::default();
                list.select(current);
                self.popup = Some((tool, list));
                Nav::None
            }
        }
    }

    fn handle_popup(&mut self, sh: &mut Shared, press: Press) -> Nav {
        let Some((tool, list)) = &mut self.popup else { return Nav::None };
        let tool = *tool;
        if press.is(Action::Back) {
            self.popup = None;
            return Nav::None;
        }
        let (options, _) = tool.options(&self.prefs);
        if let ListInput::Activate(choice) = list.handle(press, options.len()) {
            tool.apply(&mut self.prefs, choice);
            self.prefs.save(sh, self.source);
            self.popup = None;
        }
        Nav::None
    }

    fn handle_toolbar(&mut self, press: Press, at: usize) -> Nav {
        match press.action {
            Action::Left => self.toolbar = Some(at.saturating_sub(1)),
            Action::Right => self.toolbar = Some((at + 1).min(self.tools().len() - 1)),
            Action::Down => self.toolbar = None,
            Action::Up if !press.repeat => return Nav::FocusTopBar,
            Action::Back if !press.repeat => return Nav::Pop,
            _ if press.is_activate() => return self.activate_tool(self.tools()[at]),
            _ => {}
        }
        Nav::None
    }

    fn draw_toolbar(&mut self, ui: &Ui, rect: Rect, focused: bool, style: &egui::Style) {
        let painter = ui.painter();
        let font = TextStyle::Small.resolve(style);
        let mut x = rect.left();
        for (i, tool) in self.tools().iter().enumerate() {
            let label = tool.label(&self.prefs);
            let width = painter.layout_no_wrap(label.clone(), font.clone(), Color32::WHITE).size().x + 44.0;
            let r = Rect::from_min_size(pos2(x, rect.top()), vec2(width, rect.height()));
            self.tool_x[i] = x;
            if widgets::hit(ui, r).any() {
                self.toolbar = Some(i);
            }
            let color = widgets::paint_focusable(painter, r, focused && self.toolbar == Some(i), false);
            painter.text(r.center(), Align2::CENTER_CENTER, label, font.clone(), color);
            x += width + 14.0;
        }
    }

    /// Where the open dropdown goes: under its toolbar button.
    fn popup_rect(&self, toolbar: Rect) -> Option<Rect> {
        let (tool, _) = self.popup.as_ref()?;
        let (options, _) = tool.options(&self.prefs);
        let index = TOOLS.iter().position(|t| t == tool).unwrap_or(0);
        let x = self.tool_x[index];
        Some(Rect::from_min_size(pos2(x, toolbar.bottom() + 8.0), vec2(420.0, 28.0 + 70.0 * options.len() as f32)))
    }

    /// The open dropdown, under its toolbar button.
    fn draw_popup(&mut self, ui: &mut Ui, toolbar: Rect) {
        let Some(rect) = self.popup_rect(toolbar) else { return };
        let Some((tool, list)) = &mut self.popup else { return };
        let (options, current) = tool.options(&self.prefs);
        ui.painter().rect_filled(rect.expand(6.0), CornerRadius::same(12), theme::overlay());
        let rows: Vec<Row> = options
            .iter()
            .enumerate()
            .map(|(i, label)| Row::new(Icon::None, *label).detail(if i == current { "current" } else { "" }))
            .collect();
        widgets::list(ui, rect.shrink(8.0), &rows, list, true);
    }
}

fn paint_card(painter: &Painter, rect: Rect, item: &Item, poster: Option<&egui::TextureHandle>, focused: bool, style: &egui::Style) {
    let poster_rect = Rect::from_min_size(rect.min, vec2(rect.width(), POSTER_H));
    match poster {
        Some(texture) => paint_cover(painter, poster_rect, texture),
        None => {
            // No poster (yet): the title on a plain tile.
            painter.rect_filled(poster_rect, CornerRadius::same(8), theme::panel());
            let galley = painter.layout(item.title.clone(), TextStyle::Body.resolve(style), theme::dim(), poster_rect.width() - 30.0);
            painter.galley(poster_rect.center() - galley.size() / 2.0, galley, theme::dim());
        }
    }
    // Watched state: an unwatched-episode count, a tick when all watched, or progress.
    let badge_c = poster_rect.right_top() + vec2(-26.0, 26.0);
    if item.unwatched == 0 && item.episodes > 0 {
        widgets::paint_tick(painter, badge_c);
    } else if item.series && item.unwatched > 0 {
        let text = thousands(item.unwatched);
        let font = TextStyle::Small.resolve(style);
        let galley = painter.layout_no_wrap(text, font, theme::on_accent());
        let badge = Rect::from_center_size(badge_c, vec2(galley.size().x.max(18.0) + 18.0, 36.0));
        let badge = badge.translate(vec2(18.0 - badge.width() / 2.0, 0.0));
        painter.rect_filled(badge, CornerRadius::same(18), theme::accent());
        painter.galley(badge.center() - galley.size() / 2.0, galley, theme::on_accent());
    }
    if let Some(fraction) = item.progress {
        widgets::paint_progress(painter, poster_rect, fraction as f32, theme::progress());
    }
    theme::paint_picture_frame(painter, poster_rect, focused);
    let text_color = if focused { theme::highlight() } else { theme::text() };
    let text_rect = Rect::from_min_max(pos2(rect.left(), poster_rect.bottom() + 8.0), rect.max);
    let clip = painter.with_clip_rect(text_rect);
    clip.text(text_rect.left_top(), Align2::LEFT_TOP, &item.title, TextStyle::Small.resolve(style), text_color);
    let sub = match (item.year, item.series) {
        (Some(y), true) => format!("{y} · {} episodes", thousands(item.episodes)),
        (None, true) => format!("{} episodes", thousands(item.episodes)),
        (Some(y), false) => y.to_string(),
        (None, false) => String::new(),
    };
    let sub = if item.unmatched { format!("{sub} · not matched") } else { sub };
    clip.text(text_rect.left_top() + vec2(0.0, 32.0), Align2::LEFT_TOP, sub, TextStyle::Small.resolve(style), theme::dim());
}

impl Screen for ShowsView {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        self.refresh(sh);
        if self.popup.is_some() {
            return self.handle_popup(sh, press);
        }
        if let Some(at) = self.toolbar {
            return self.handle_toolbar(press, at);
        }
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let selected = self.selected_index().map(|i| (self.items[i].library_id, self.items[i].show_id));
        if press.is(Action::Menu) {
            return selected.map_or(Nav::None, |(lib, id)| Nav::push(ShowOptions::new(lib, id)));
        }
        let input = match self.prefs.layout {
            Layout::Posters => self.grid.handle(press, self.items.len()),
            Layout::List => self.list.handle(press, self.items.len()),
        };
        match input {
            ListInput::AtTop => {
                self.toolbar = Some(0);
                Nav::None
            }
            ListInput::Activate(i) => {
                let (library_id, show_id) = (self.items[i].library_id, self.items[i].show_id);
                match sh.lib(library_id).and_then(|l| l.catalog()?.show(show_id)).cloned() {
                    Some(entry) => open_show(sh, library_id, &entry),
                    None => Nav::None,
                }
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let (name, subtitle, organised) = match self.source {
            Source::Library(id) => {
                let Some(state) = sh.lib(id) else { return Nav::Pop };
                let movies = state.lib.kind == flex_core::LibraryKind::Movies;
                let what = if movies { "movies" } else { "shows" };
                let subtitle = match state.catalog() {
                    Some(c) if self.items.len() == c.shows.len() => {
                        format!("{} {what} · {}", thousands(self.items.len()), state.status())
                    }
                    Some(c) => format!("{} of {} {what} · {}", thousands(self.items.len()), thousands(c.shows.len()), state.status()),
                    None => "Organising…".to_owned(),
                };
                (state.lib.name.clone(), subtitle, state.catalog().is_some())
            }
            Source::Watchlist => {
                let total = sh.watchlist_shows().len();
                let subtitle = if total == 1 { "1 title".to_owned() } else { format!("{} titles", thousands(total)) };
                ("Watchlist".to_owned(), subtitle, true)
            }
        };
        let style = ui.style().clone();
        let body = widgets::header(ui.painter(), rect, &name, &subtitle, &style);
        let toolbar = Rect::from_min_size(body.min, vec2(body.width(), TOOLBAR_H));
        if let Some(popup) = self.popup_rect(toolbar) {
            // Clicking outside the dropdown closes it.
            let outside = ui.input(|i| i.pointer.primary_clicked() && i.pointer.hover_pos().is_some_and(|p| !popup.contains(p)));
            if outside {
                // Only closes it: nothing underneath gets the click.
                self.popup = None;
                widgets::modal(ui.ctx(), Rect::NOTHING);
            } else {
                widgets::modal(ui.ctx(), popup.expand(6.0));
            }
        }
        self.draw_toolbar(ui, toolbar, focused, &style);
        let content = Rect::from_min_max(pos2(body.left(), toolbar.bottom() + 24.0), body.max);
        let content_focused = focused && self.toolbar.is_none() && self.popup.is_none();

        if self.items.is_empty() {
            let message = match (organised, self.prefs.filter) {
                (false, _) => "Organising… shows will appear shortly",
                (true, Filter::All) if self.source == Source::Watchlist => {
                    "Nothing here yet: add shows and films from their page or by holding OK"
                }
                (true, Filter::Unwatched) => "Everything here has been watched",
                (true, Filter::Watched) => "Nothing watched here yet",
                (true, Filter::Unmatched) => "Everything here is matched online",
                (true, Filter::All) => "No videos here",
            };
            widgets::message(ui.painter(), content, message, &style);
        } else {
            match self.prefs.layout {
                Layout::Posters => {
                    let (items, images) = (&self.items, &mut sh.images);
                    let hit = widgets::grid(ui, content, items.len(), GRID, &mut self.grid, content_focused, |painter, i, r, f| {
                        let item = &items[i];
                        let poster = item.poster.as_deref().and_then(|p| images.get(p, Size::Poster));
                        paint_card(painter, r, item, poster.as_ref(), f, &style);
                    });
                    if hit.any() {
                        self.toolbar = None;
                    }
                }
                Layout::List => {
                    let rows: Vec<Row> = self
                        .items
                        .iter()
                        .map(|it| {
                            let title = match it.year {
                                Some(y) => format!("{} ({y})", it.title),
                                None => it.title.clone(),
                            };
                            let mut detail = match (it.series, it.unwatched) {
                                (true, 0) => "All watched".to_owned(),
                                (true, u) if u == it.episodes => format!("{} episodes", thousands(it.episodes)),
                                (true, u) => format!("{} episodes · {} unwatched", thousands(it.episodes), thousands(u)),
                                (false, 0) => "Watched".to_owned(),
                                (false, _) => String::new(),
                            };
                            if it.unmatched {
                                detail.push_str(" · not matched online");
                            }
                            let icon = if it.series { Icon::Folder } else { Icon::Video };
                            Row::new(icon, title).detail(detail).progress(it.progress).dim(it.unwatched == 0)
                        })
                        .collect();
                    if widgets::list(ui, content, &rows, &mut self.list, content_focused).any() {
                        self.toolbar = None;
                    }
                }
            }
        }
        self.draw_popup(ui, toolbar);
        Nav::None
    }
}
