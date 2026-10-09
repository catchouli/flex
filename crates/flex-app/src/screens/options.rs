//! Options for one item, opened by holding OK (or right-click): mark watched/unwatched,
//! play from the beginning.

use eframe::egui::{Rect, Ui};

use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::{PlayRequest, Shared};
use crate::widgets::{self, Icon, ListInput, ListState, Row};

pub enum Target {
    File { library_id: i64, file_id: i64 },
    Folder { library_id: i64, rel: String },
}

enum Choice {
    MarkWatched(bool),
    PlayFromStart(PlayRequest),
}

pub struct OptionsMenu {
    target: Target,
    list: ListState,
}

impl OptionsMenu {
    pub fn new(target: Target) -> Self {
        Self { target, list: ListState::default() }
    }

    fn choices(&self, sh: &Shared) -> Vec<(Row, Choice)> {
        match &self.target {
            Target::File { library_id, file_id } => {
                let Some(progress) = sh.progress(*library_id, *file_id) else { return Vec::new() };
                let mut out = Vec::new();
                if progress.position.is_some()
                    && let Some(req) = sh.play_request(*library_id, *file_id)
                {
                    out.push((Row::new(Icon::Video, "Play from beginning"), Choice::PlayFromStart(req)));
                }
                if progress.watched {
                    out.push((Row::new(Icon::Remove, "Mark as unwatched"), Choice::MarkWatched(false)));
                } else {
                    out.push((Row::new(Icon::Add, "Mark as watched"), Choice::MarkWatched(true)));
                }
                out
            }
            Target::Folder { .. } => vec![
                (Row::new(Icon::Add, "Mark all as watched"), Choice::MarkWatched(true)),
                (Row::new(Icon::Remove, "Mark all as unwatched"), Choice::MarkWatched(false)),
            ],
        }
    }

    fn title(&self, sh: &Shared) -> String {
        match &self.target {
            Target::File { library_id, file_id } => {
                sh.play_request(*library_id, *file_id).map_or_else(String::new, |r| r.title)
            }
            Target::Folder { rel, .. } => rel.rsplit('/').next().unwrap_or(rel).to_owned(),
        }
    }
}

impl Screen for OptionsMenu {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) || press.is(Action::Menu) {
            return Nav::Pop;
        }
        let mut choices = self.choices(sh);
        let ListInput::Activate(i) = self.list.handle(press, choices.len()) else { return Nav::None };
        match choices.swap_remove(i).1 {
            Choice::PlayFromStart(req) => Nav::Play { req: PlayRequest { start: Some(0.0), ..req }, replace: true },
            Choice::MarkWatched(watched) => {
                let (library_id, ids) = match &self.target {
                    Target::File { library_id, file_id } => (*library_id, vec![*file_id]),
                    Target::Folder { library_id, rel } => {
                        (*library_id, sh.lib(*library_id).map_or_else(Vec::new, |l| l.tree.ids_under(rel)))
                    }
                };
                sh.set_watched(library_id, &ids, watched);
                Nav::Pop
            }
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let title = self.title(sh);
        let body = widgets::header(ui.painter(), rect, &title, "Options", ui.style());
        let rows: Vec<Row> = self.choices(sh).into_iter().map(|(row, _)| row).collect();
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
