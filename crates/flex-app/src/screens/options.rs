//! Options for one item, opened by holding OK (or right-click): mark watched/unwatched,
//! play from the beginning, and (from Continue Watching) remove it from there.

use eframe::egui::{Rect, Ui};

use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::{PlayRequest, Shared};
use crate::widgets::{self, Icon, ListInput, ListState, Row};

pub enum Target {
    File { library_id: i64, file_id: i64 },
    Folder { library_id: i64, rel: String },
    /// Any set of files, e.g. a season.
    Files { library_id: i64, ids: Vec<i64>, label: String },
}

enum Choice {
    MarkWatched(bool),
    PlayFromStart(PlayRequest),
    RemoveFromContinue,
}

pub struct OptionsMenu {
    target: Target,
    list: ListState,
    /// Opened from Continue Watching: its key there (see `Shared::hide_continue`).
    continue_key: Option<String>,
}

impl OptionsMenu {
    pub fn new(target: Target) -> Self {
        Self { target, list: ListState::default(), continue_key: None }
    }

    /// Opened on a Continue Watching card: also offers to remove it from there.
    pub fn from_continue(target: Target, key: String) -> Self {
        Self { continue_key: Some(key), ..Self::new(target) }
    }

    fn choices(&self, sh: &Shared) -> Vec<(Row, Choice)> {
        let mut choices = self.target_choices(sh);
        if self.continue_key.is_some() {
            choices.push((Row::new(Icon::Remove, "Remove from Continue Watching").detail("Until you watch it again"), Choice::RemoveFromContinue));
        }
        choices
    }

    fn target_choices(&self, sh: &Shared) -> Vec<(Row, Choice)> {
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
            Target::Folder { .. } | Target::Files { .. } => vec![
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
            Target::Files { label, .. } => label.clone(),
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
                    Target::Files { library_id, ids, .. } => (*library_id, ids.clone()),
                };
                sh.set_watched(library_id, &ids, watched);
                Nav::Pop
            }
            Choice::RemoveFromContinue => {
                if let Some(key) = &self.continue_key {
                    sh.hide_continue(key);
                }
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
