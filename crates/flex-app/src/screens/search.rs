//! Search results for the query typed in the top bar, with a row of scopes along the top:
//! Movies & shows (default), each library, Episodes, Music (with a music library), Folders
//! & files. Up from the first result reaches the scopes; Left/Right switches.

use eframe::egui::{Align2, Rect, TextStyle, Ui, pos2, vec2};
use flex_core::search::{Item, Scope, Target};

use super::details::{EpisodePage, TitlePage, open_file};
use super::music::{AlbumPage, ArtistPage, play_album};
use super::options::{self, OptionsMenu};
use super::{Nav, Screen, library::open_folder};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::widgets::{self, Icon, ListInput, ListState, Row};

const MAX_RESULTS: usize = 100;
const SCOPE_H: f32 = 60.0;

#[derive(Default)]
pub struct SearchView {
    /// (query, scope) the results are for.
    searched: Option<(String, Scope)>,
    results: Vec<Item>,
    scope: usize,
    /// Focus is on the scope row.
    on_scopes: bool,
    list: ListState,
}


/// The scopes in order, with labels.
fn scopes(sh: &Shared) -> Vec<(Scope, String)> {
    let mut out = vec![(Scope::Titles, "Movies & shows".to_owned())];
    out.extend(sh.libs.iter().filter(|l| !l.is_music()).map(|l| (Scope::Library(l.lib.id), l.lib.name.clone())));
    out.push((Scope::Episodes, "Episodes".to_owned()));
    if sh.libs.iter().any(|l| l.is_music()) {
        out.push((Scope::Music, "Music".to_owned()));
    }
    out.push((Scope::Paths, "Folders & files".to_owned()));
    out
}

impl SearchView {
    fn refresh(&mut self, sh: &mut Shared) {
        let all = scopes(sh);
        self.scope = self.scope.min(all.len() - 1);
        let key = (sh.search_query.clone(), all[self.scope].0);
        if self.searched.as_ref() != Some(&key) {
            // A new query starts back at Movies & shows.
            if self.searched.as_ref().is_some_and(|(q, _)| *q != key.0) && self.scope != 0 {
                self.scope = 0;
                return self.refresh(sh);
            }
            self.results = sh.search(&key.0, key.1, MAX_RESULTS);
            self.searched = Some(key);
            self.list.select(0);
        }
    }

    fn open(&self, sh: &mut Shared, i: usize) -> Nav {
        match &self.results[i].target {
            Target::Show { library_id, show_id } => Nav::push(TitlePage::new(*library_id, *show_id)),
            Target::Episode { library_id, file_id } => Nav::push(EpisodePage::new(*library_id, *file_id)),
            Target::Folder { library_id, rel } => open_folder(sh, *library_id, rel),
            Target::File { library_id, file_id } => open_file(sh, *library_id, *file_id),
            Target::Album { library_id, key } => Nav::push(AlbumPage::new(*library_id, key.clone())),
            Target::Artist { library_id, name } => Nav::push(ArtistPage::new(sh, *library_id, name.clone())),
            Target::Track { library_id, album, index } => play_album(sh, *library_id, album, *index, false),
        }
    }
}

impl Screen for SearchView {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        self.refresh(sh);
        if press.is(Action::Back) {
            sh.search_query.clear();
            return Nav::Pop;
        }
        if self.on_scopes {
            let count = scopes(sh).len();
            match press.action {
                Action::Left => self.scope = self.scope.saturating_sub(1),
                Action::Right => self.scope = (self.scope + 1).min(count - 1),
                Action::Down => self.on_scopes = false,
                Action::Up if !press.repeat => return Nav::FocusTopBar,
                _ if press.is_activate() => self.on_scopes = false,
                _ => {}
            }
            return Nav::None;
        }
        if press.is(Action::Menu) {
            let Some(item) = self.results.get(self.list.selected) else { return Nav::None };
            let target = match &item.target {
                Target::Show { library_id, show_id } => return Nav::push(super::shows::ShowOptions::new(*library_id, *show_id)),
                Target::Folder { library_id, rel } => options::Target::Folder { library_id: *library_id, rel: rel.clone() },
                Target::File { library_id, file_id } | Target::Episode { library_id, file_id } => {
                    options::Target::File { library_id: *library_id, file_id: *file_id }
                }
                // Music has no options menu.
                Target::Album { .. } | Target::Artist { .. } | Target::Track { .. } => return Nav::None,
            };
            return Nav::push(OptionsMenu::new(target));
        }
        match self.list.handle(press, self.results.len()) {
            ListInput::AtTop => {
                self.on_scopes = true;
                Nav::None
            }
            ListInput::Activate(i) => self.open(sh, i),
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let subtitle = match self.results.len() {
            0 => String::new(),
            MAX_RESULTS => format!("Top {MAX_RESULTS} matches"),
            n => format!("{n} matches"),
        };
        let title = format!("“{}”", sh.search_query.trim());
        let style = ui.style().clone();
        let body = widgets::header(ui.painter(), rect, &title, &subtitle, &style);

        // Scope row.
        let painter = ui.painter().clone();
        let font = TextStyle::Small.resolve(&style);
        let mut x = body.left();
        for (i, (_, label)) in scopes(sh).iter().enumerate() {
            let w = painter.layout_no_wrap(label.clone(), font.clone(), crate::theme::TEXT).size().x + 44.0;
            let r = Rect::from_min_size(pos2(x, body.top()), vec2(w, SCOPE_H));
            if widgets::hit(ui, r).any() {
                (self.scope, self.on_scopes) = (i, true);
            }
            let color = widgets::paint_focusable(&painter, r, focused && self.on_scopes && i == self.scope, i == self.scope);
            painter.text(r.center(), Align2::CENTER_CENTER, label, font.clone(), color);
            x += w + 14.0;
        }
        let list_rect = Rect::from_min_max(pos2(body.left(), body.top() + SCOPE_H + 24.0), body.max);
        if self.results.is_empty() {
            widgets::message(ui.painter(), list_rect, "Nothing found here. Up, then Left/Right to search elsewhere", &style);
            return Nav::None;
        }
        let rows: Vec<Row> = self
            .results
            .iter()
            .map(|item| {
                let icon = match item.target {
                    Target::Show { .. } | Target::Folder { .. } | Target::Album { .. } | Target::Artist { .. } => Icon::Folder,
                    Target::Episode { .. } | Target::File { .. } | Target::Track { .. } => Icon::Video,
                };
                Row::new(icon, item.title.as_str()).detail(item.context.as_str())
            })
            .collect();
        if widgets::list(ui, list_rect, &rows, &mut self.list, focused && !self.on_scopes).any() {
            self.on_scopes = false;
        }
        Nav::None
    }

    fn is_search(&self) -> bool {
        true
    }

    fn activate_first(&mut self, sh: &mut Shared) -> Nav {
        self.refresh(sh);
        if self.results.is_empty() { Nav::None } else { self.open(sh, 0) }
    }
}
