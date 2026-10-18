//! "Folders & seasons": which folders make up a show and as which season, with fixes:
//! change a folder's season, move it to another show, or split it into its own show.
//! Fixes are stored as manual and never undone by rescans.

use eframe::egui::{self, Key, Rect, TextStyle, Ui, vec2};
use flex_core::catalog::PlacedGroup;

use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::theme;
use crate::widgets::{self, Icon, ListInput, ListState, Row};

fn season_label(season: i32) -> String {
    if season == 0 { "Specials".to_owned() } else { format!("Season {season}") }
}

fn groups_of(sh: &Shared, library_id: i64, show_id: i64) -> Option<(String, Vec<PlacedGroup>)> {
    let entry = sh.lib(library_id)?.catalog()?.show(show_id)?;
    let mut groups = entry.groups.clone();
    groups.sort_by(|a, b| a.season.cmp(&b.season).then_with(|| flex_core::natural::natural_cmp(&a.key, &b.key)));
    Some((entry.show.title.clone(), groups))
}

pub struct MatchInspector {
    library_id: i64,
    show_id: i64,
    list: ListState,
}

impl MatchInspector {
    pub fn new(library_id: i64, show_id: i64) -> Self {
        Self { library_id, show_id, list: ListState::default() }
    }
}

impl Screen for MatchInspector {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let Some((_, groups)) = groups_of(sh, self.library_id, self.show_id) else { return Nav::Pop };
        match self.list.handle(press, groups.len()) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) => Nav::push(GroupEditor::new(self.library_id, self.show_id, &groups[i])),
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        // The show disappears if its last folder was moved away.
        let Some((title, groups)) = groups_of(sh, self.library_id, self.show_id) else { return Nav::Pop };
        let subtitle = "Folders in this show. OK to change one; changes are kept on rescans.";
        let body = widgets::header(ui.painter(), rect, &format!("{title}: folders & seasons"), subtitle, ui.style());
        let rows: Vec<Row> = groups
            .iter()
            .map(|g| {
                let how = if g.manual { "set by hand" } else { "automatic" };
                let videos = if g.files == 1 { "1 video".to_owned() } else { format!("{} videos", g.files) };
                Row::new(Icon::Folder, format!("{}  ·  {}", season_label(g.season), g.key))
                    .detail(format!("{videos} · {how}"))
            })
            .collect();
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

const MAX_SEASON: i32 = 99;

pub struct GroupEditor {
    library_id: i64,
    show_id: i64,
    key: String,
    /// Season being chosen with Left/Right; saved with OK.
    season: i32,
    list: ListState,
}

impl GroupEditor {
    fn new(library_id: i64, show_id: i64, group: &PlacedGroup) -> Self {
        Self { library_id, show_id, key: group.key.clone(), season: group.season, list: ListState::default() }
    }
}

impl Screen for GroupEditor {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        // Row 0 adjusts the season with Left/Right instead of paging.
        if self.list.selected == 0 {
            match press.action {
                Action::Left => {
                    self.season = (self.season - 1).max(0);
                    return Nav::None;
                }
                Action::Right => {
                    self.season = (self.season + 1).min(MAX_SEASON);
                    return Nav::None;
                }
                _ => {}
            }
        }
        match self.list.handle(press, 3) {
            ListInput::Activate(0) => {
                sh.place_group(self.library_id, &self.key, self.show_id, Some(self.season));
                sh.toast(format!("Saved as {}", season_label(self.season).to_lowercase()));
                Nav::Pop
            }
            ListInput::Activate(1) => Nav::replace(ShowPicker::new(self.library_id, self.show_id, &self.key)),
            ListInput::Activate(_) => {
                sh.split_group(self.library_id, &self.key);
                sh.toast("Moved into its own show");
                Nav::Pop
            }
            _ => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, _rect: Rect, focused: bool) -> Nav {
        // Without the top bar the screen gets the whole window: keep to the safe area.
        let rect = theme::safe_rect(ui.ctx());
        let Some((title, groups)) = groups_of(sh, self.library_id, self.show_id) else { return Nav::Pop };
        if !groups.iter().any(|g| g.key == self.key) {
            return Nav::Pop; // moved elsewhere
        }
        let body = widgets::header(ui.painter(), rect, &self.key, &format!("In {title}"), ui.style());
        let rows = [
            Row::new(Icon::None, format!("◀  {}  ▶", season_label(self.season))).detail("Left/Right to change, OK to save"),
            Row::new(Icon::Folder, "Move to another show…"),
            Row::new(Icon::Add, "Make it a separate show"),
        ];
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

/// Picks the show a folder should move to. Typing (keyboard) filters the list.
pub struct ShowPicker {
    library_id: i64,
    from_show: i64,
    key: String,
    filter: String,
    list: ListState,
}

impl ShowPicker {
    fn new(library_id: i64, from_show: i64, key: &str) -> Self {
        Self { library_id, from_show, key: key.to_owned(), filter: String::new(), list: ListState::default() }
    }

    fn candidates(&self, sh: &Shared) -> Vec<(i64, String)> {
        let Some(catalog) = sh.lib(self.library_id).and_then(|l| l.catalog()) else { return Vec::new() };
        let filter = flex_core::parse::normalize(&self.filter);
        catalog
            .shows
            .iter()
            .filter(|s| s.show.id != self.from_show)
            .filter(|s| filter.is_empty() || flex_core::parse::normalize(&s.show.title).contains(&filter))
            .map(|s| (s.show.id, s.show.title.clone()))
            .collect()
    }

    fn filter_id() -> egui::Id {
        egui::Id::new("flex-show-picker-filter")
    }
}

impl Screen for ShowPicker {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let shows = self.candidates(sh);
        match self.list.handle(press, shows.len()) {
            ListInput::AtTop => {
                sh.ctx.memory_mut(|m| m.request_focus(Self::filter_id()));
                Nav::None
            }
            ListInput::Activate(i) => {
                sh.place_group(self.library_id, &self.key, shows[i].0, None);
                sh.toast(format!("Moved to {}", shows[i].1));
                Nav::Pop
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let body = widgets::header(ui.painter(), rect, "Move to another show", &self.key, ui.style());
        let field = Rect::from_min_size(body.min, vec2(body.width().min(700.0), 56.0));
        let edit = egui::TextEdit::singleline(&mut self.filter)
            .id(Self::filter_id())
            .hint_text("Type to filter (keyboard)")
            .font(TextStyle::Body)
            .margin(vec2(16.0, 10.0))
            .desired_width(field.width());
        let response = ui.put(field, edit);
        if response.changed() {
            self.list.select(0);
        }
        if response.has_focus() && ui.input(|i| i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::Enter)) {
            response.surrender_focus();
        }
        let list_rect = Rect::from_min_max(egui::pos2(body.left(), field.bottom() + 16.0), body.max);
        let rows: Vec<Row> = self.candidates(sh).into_iter().map(|(_, title)| Row::new(Icon::Folder, title)).collect();
        if rows.is_empty() {
            widgets::message(ui.painter(), list_rect, "No matching shows", ui.style());
        }
        widgets::list(ui, list_rect, &rows, &mut self.list, focused);
        Nav::None
    }

    fn shows_top_bar(&self) -> bool {
        false
    }
}
