//! An actor's (or voice actor's) page: their photo and biography, what they're in that's
//! in the libraries, then the rest of their films and shows, looked up online (TMDB, or
//! AniList for Japanese voice actors) the first time the page opens and cached.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui::{self, Align2, CornerRadius, Rect, TextStyle, Ui, pos2, vec2};
use flex_core::metadata::{CastMember, CreditKind, Person};
use flex_core::parse::normalize;

use super::details::{PageNav, ROW_LABEL_H, RowKind, SECTION_GAP, TitlePage, hrow, paint_card};
use super::{Nav, Screen};
use crate::images::{Size, paint_cover};
use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::{theme, widgets};

const PHOTO: egui::Vec2 = egui::vec2(240.0, 360.0);
const CARD: egui::Vec2 = egui::vec2(180.0, 334.0);
/// Online credits shown per row.
const MAX_CREDITS: usize = 40;

enum Online {
    /// No id to look them up by (cast saved by an older Flex, until refreshed).
    NoId,
    Loading(Receiver<Result<Person, String>>),
    Ready(Person),
    Failed(String),
}

struct Card {
    title: String,
    sub: String,
    image: Option<String>,
    /// (library id, show id) for ones in a library.
    open: Option<(i64, i64)>,
}

pub struct PersonPage {
    member: CastMember,
    online: Online,
    nav: PageNav,
    /// Rows built for this library state (sum of versions), and whether the online
    /// details had arrived.
    built_for: Option<(u64, bool)>,
    rows: Vec<(RowKind, &'static str, Vec<Card>)>,
}

impl PersonPage {
    pub fn new(sh: &Shared, member: CastMember) -> Self {
        let online = match member.person.clone() {
            None => Online::NoId,
            Some(id) => match sh.cached_person(&id) {
                Some(p) => Online::Ready(p),
                None => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let (token, db_path, ctx) = (sh.tmdb_token(), sh.db_path().to_owned(), sh.ctx.clone());
                    std::thread::spawn(move || {
                        let tmdb = token.map(|t| flex_core::metadata::Tmdb::new(&t));
                        let result = flex_core::metadata::person(tmdb.as_ref(), &id);
                        if let Ok(p) = &result
                            && let Err(e) = flex_core::Db::open(&db_path).and_then(|db| db.save_person(&id, p))
                        {
                            log::warn!("saving {id}: {e:#}");
                        }
                        let _ = tx.send(result.map_err(|e| format!("{e:#}")));
                        ctx.request_repaint();
                    });
                    Online::Loading(rx)
                }
            },
        };
        Self { member, online, nav: PageNav::default(), built_for: None, rows: Vec::new() }
    }

    fn poll(&mut self) {
        let Online::Loading(rx) = &self.online else { return };
        self.online = match rx.try_recv() {
            Ok(Ok(p)) => Online::Ready(p),
            Ok(Err(e)) => Online::Failed(e),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Online::Failed("lookup stopped".into()),
        };
    }

    /// (Re)builds the rows when the libraries change or the online details arrive.
    fn refresh(&mut self, sh: &Shared) {
        self.poll();
        let key = (sh.libs.iter().map(|l| l.version).sum(), matches!(self.online, Online::Ready(_)));
        if self.built_for == Some(key) {
            return;
        }
        self.built_for = Some(key);
        let mut library: Vec<(Option<i32>, Card)> = sh
            .appearances(self.member.person.as_deref(), &self.member.name)
            .into_iter()
            .filter_map(|(library_id, show_id, role)| {
                let entry = sh.lib(library_id)?.catalog()?.show(show_id)?;
                let year = entry.show.year;
                let sub = [year.map(|y| y.to_string()), (!role.is_empty()).then_some(role)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                let card = Card { title: entry.show.title.clone(), sub, image: entry.show.poster.clone(), open: Some((library_id, show_id)) };
                Some((year, card))
            })
            .collect();
        // Newest first.
        library.sort_by_key(|c| std::cmp::Reverse(c.0));
        let mut rows = vec![(RowKind::InLibrary, "In your library", library.into_iter().map(|(_, c)| c).collect::<Vec<_>>())];
        if let Online::Ready(person) = &self.online {
            let anime_titles = sh.anime_titles();
            // Leave out what's already in the libraries.
            let have: HashSet<&str> = rows[0].2.iter().map(|c| c.title.as_str()).collect();
            let other = |kind: CreditKind| -> Vec<Card> {
                person
                    .credits
                    .iter()
                    .filter(|c| c.kind == kind)
                    .filter(|c| match &c.provider_id {
                        Some(pid) => !sh.in_library(pid),
                        None => !std::iter::once(&c.title).chain(&c.other_titles).any(|t| anime_titles.contains(&normalize(t))),
                    })
                    .filter(|c| !have.contains(c.title.as_str()))
                    .take(MAX_CREDITS)
                    .map(|c| {
                        let sub = [c.year.map(|y| y.to_string()), (!c.role.is_empty()).then(|| c.role.clone())];
                        Card { title: c.title.clone(), sub: sub.into_iter().flatten().collect::<Vec<_>>().join(" · "), image: c.poster.clone(), open: None }
                    })
                    .collect()
            };
            let (films, shows, anime) = (other(CreditKind::Movie), other(CreditKind::Tv), other(CreditKind::Anime));
            rows.push((RowKind::Films, "Other films", films));
            rows.push((RowKind::Shows, "Other TV shows", shows));
            if !anime.is_empty() {
                rows.push((RowKind::Shows, "Other anime", anime));
            }
        }
        rows.retain(|(_, _, cards)| !cards.is_empty());
        self.rows = rows;
    }

    fn nav_rows(&self) -> Vec<(RowKind, usize)> {
        self.rows.iter().map(|(k, _, cards)| (*k, cards.len())).collect()
    }

    /// One line under the rows about the online lookup, if there's anything to say.
    fn status(&self) -> Option<String> {
        match &self.online {
            Online::NoId => Some("Other work appears once Flex has refreshed this cast (in the background).".into()),
            Online::Loading(_) => Some("Looking up their other work…".into()),
            Online::Failed(e) => Some(format!("Couldn't look up their other work: {e}")),
            Online::Ready(_) if self.rows.len() <= 1 => Some("Nothing else found online".into()),
            Online::Ready(_) => None,
        }
    }
}

impl Screen for PersonPage {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        self.refresh(sh);
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let rows = self.nav_rows();
        if rows.is_empty() {
            return if press.is(Action::Up) { Nav::FocusTopBar } else { Nav::None };
        }
        if let Some(nav) = self.nav.handle(press, &rows) {
            return nav;
        }
        if !press.is_activate() {
            return Nav::None;
        }
        let (kind, i) = self.nav.focused(&rows).unwrap_or((RowKind::InLibrary, 0));
        let card = self.rows.iter().find(|(k, _, _)| *k == kind).and_then(|(_, _, cards)| cards.get(i));
        match card.and_then(|c| c.open) {
            Some((library_id, show_id)) => Nav::push(TitlePage::new(library_id, show_id)),
            None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        self.refresh(sh);
        let style = ui.style().clone();
        let rows = self.nav_rows();
        let focus = if focused { self.nav.focused(&rows) } else { None };

        // Layout relative to the content top, then scroll the focused row into view.
        let header_h = PHOTO.y;
        let mut y = header_h + SECTION_GAP;
        let mut tops = Vec::new();
        for _ in &self.rows {
            tops.push(y);
            y += ROW_LABEL_H + CARD.y + SECTION_GAP;
        }
        let content_h = y + 60.0;
        if let Some(&top) = tops.get(self.nav.row) {
            let top = if self.nav.row == 0 { 0.0 } else { top };
            let bottom = tops[self.nav.row] + ROW_LABEL_H + CARD.y;
            self.nav.scroll_to(top, bottom, rect.height(), content_h);
        }
        let origin = rect.min - vec2(0.0, self.nav.scroll);
        let painter = ui.painter().with_clip_rect(rect);

        // Photo, name, facts, biography.
        let person = match &self.online {
            Online::Ready(p) => Some(p),
            _ => None,
        };
        let photo_rect = Rect::from_min_size(origin, PHOTO);
        let photo = person.and_then(|p| p.image.clone()).or_else(|| self.member.image.clone());
        match photo.and_then(|p| sh.images.get(&p, Size::Profile)) {
            Some(t) => paint_cover(&painter, photo_rect, &t),
            None => {
                painter.rect_filled(photo_rect, CornerRadius::same(8), theme::ROW);
            }
        }
        let text = Rect::from_min_max(pos2(photo_rect.right() + 44.0, origin.y + 8.0), pos2(rect.right() - 20.0, origin.y + header_h));
        let clip = painter.with_clip_rect(text.intersect(rect));
        clip.text(text.left_top(), Align2::LEFT_TOP, &self.member.name, TextStyle::Heading.resolve(&style), theme::TEXT);
        let mut facts = Vec::new();
        if let Some(p) = person {
            facts.extend(p.born.as_ref().map(|b| format!("Born {b}")));
            facts.extend(p.birthplace.clone());
        }
        clip.text(text.left_top() + vec2(0.0, 72.0), Align2::LEFT_TOP, facts.join("  ·  "), TextStyle::Small.resolve(&style), theme::DIM);
        if let Some(bio) = person.map(|p| p.biography.as_str()).filter(|b| !b.is_empty()) {
            let r = Rect::from_min_max(text.left_top() + vec2(0.0, 124.0), text.max);
            let r = r.with_max_x(r.left() + r.width().min(1100.0));
            widgets::text_block(&painter, r, bio, TextStyle::Body.resolve(&style), theme::TEXT);
        }

        // Rows of cards. `hovered` = (row, item) under the mouse.
        let mut hovered = None;
        for (index, (kind, label, cards)) in self.rows.iter().enumerate() {
            let top = tops[index];
            painter.text(origin + vec2(0.0, top), Align2::LEFT_TOP, *label, TextStyle::Body.resolve(&style), theme::TEXT);
            let slot = PageNav::kind_index(*kind);
            let row_focus = focus.filter(|(k, _)| k == kind).map(|(_, i)| i);
            let area = Rect::from_min_size(origin + vec2(0.0, top + ROW_LABEL_H), vec2(rect.width(), 0.0));
            let images = &mut sh.images;
            hrow(&painter, area, CARD, cards.len(), self.nav.col[slot], &mut self.nav.offset[slot], |p, i, r, _| {
                if widgets::hit(ui, r.intersect(rect)).any() {
                    hovered = Some((index, i));
                }
                let card = &cards[i];
                let image = card.image.as_deref().and_then(|img| images.get(img, Size::Poster));
                paint_card(p, r, &card.title, &card.sub, image.as_ref(), row_focus == Some(i), &style);
            });
        }
        if let Some(status) = self.status() {
            let top = tops.last().map_or(header_h + SECTION_GAP, |t| t + ROW_LABEL_H + CARD.y + SECTION_GAP);
            painter.text(origin + vec2(0.0, top), Align2::LEFT_TOP, status, TextStyle::Small.resolve(&style), theme::DIM);
        }
        if let Some((row, i)) = hovered {
            self.nav.row = row;
            self.nav.col[PageNav::kind_index(self.rows[row].0)] = i;
        }
        if ui.rect_contains_pointer(rect) {
            self.nav.scroll -= ui.input(|i| i.smooth_scroll_delta.y);
        }
        Nav::None
    }
}
