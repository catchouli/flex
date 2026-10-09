//! Importing watched state, resume positions and history from a Plex database: reads it
//! and shows which Plex library maps to which Flex library, then imports on OK.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui::{Rect, Ui};
use flex_core::db::ImportSummary;
use flex_core::plex::{self, ImportPlan};
use flex_core::{Db, Tree};

use super::{Nav, Screen};
use crate::input::{Action, Press};
use crate::shared::{Shared, thousands};
use crate::widgets::{self, Icon, ListInput, ListState, Row};

struct Preview {
    account: String,
    plan: ImportPlan,
}

enum State {
    Reading(Receiver<Result<Preview, String>>),
    Ready(Box<Preview>),
    Importing(Receiver<Result<ImportSummary, String>>),
    Failed(String),
}

pub struct PlexImport {
    path: PathBuf,
    state: State,
    list: ListState,
}

impl PlexImport {
    pub fn new(sh: &Shared, path: PathBuf) -> Self {
        let libraries: Vec<(i64, Vec<flex_core::MediaFile>)> =
            sh.libs.iter().map(|l| (l.lib.id, l.tree.files().to_vec())).collect();
        let (tx, rx) = channel();
        let (ctx, plex_path) = (sh.ctx.clone(), path.clone());
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<Preview> {
                let data = plex::read(&plex_path)?;
                let trees: Vec<(i64, Tree)> = libraries.into_iter().map(|(id, files)| (id, Tree::new(files))).collect();
                let refs: Vec<(i64, &Tree)> = trees.iter().map(|(id, t)| (*id, t)).collect();
                let plan = plex::plan(&data, &refs);
                Ok(Preview { account: data.account, plan })
            })();
            let _ = tx.send(result.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        Self { path, state: State::Reading(rx), list: ListState::default() }
    }

    fn start_import(&mut self, sh: &Shared, preview: Box<Preview>) {
        let (tx, rx) = channel();
        let (ctx, db_path) = (sh.ctx.clone(), sh.db_path().to_path_buf());
        std::thread::spawn(move || {
            let result = Db::open(&db_path).and_then(|mut db| db.import_plex(&preview.plan));
            let _ = tx.send(result.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        self.state = State::Importing(rx);
    }

    /// Advances background work; returns `Nav::Pop` once the import has finished.
    fn poll(&mut self, sh: &mut Shared) -> Nav {
        match &self.state {
            State::Reading(rx) => match rx.try_recv() {
                Ok(Ok(preview)) => self.state = State::Ready(Box::new(preview)),
                Ok(Err(e)) => self.state = State::Failed(e),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.state = State::Failed("Reading stopped unexpectedly".into()),
            },
            State::Importing(rx) => match rx.try_recv() {
                Ok(Ok(summary)) => {
                    sh.reload_files();
                    sh.toast(format!(
                        "Imported from Plex: {} files updated, {} history entries added",
                        thousands(summary.files_updated),
                        thousands(summary.history_added)
                    ));
                    return Nav::Pop;
                }
                Ok(Err(e)) => self.state = State::Failed(e),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.state = State::Failed("Import stopped unexpectedly".into()),
            },
            State::Ready(_) | State::Failed(_) => {}
        }
        Nav::None
    }
}

impl Screen for PlexImport {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        let importing = matches!(self.state, State::Importing(_));
        if press.is(Action::Back) && !importing {
            return Nav::Pop;
        }
        if !matches!(self.state, State::Ready(_)) {
            return Nav::None;
        }
        // Row 0: Import, row 1: Cancel; the rest describe the mapping.
        match self.list.handle(press, 2) {
            ListInput::Activate(0) => {
                if let State::Ready(preview) = std::mem::replace(&mut self.state, State::Failed(String::new())) {
                    self.start_import(sh, preview);
                }
                Nav::None
            }
            ListInput::Activate(_) => Nav::Pop,
            _ => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        if let Nav::Pop = self.poll(sh) {
            return Nav::Pop;
        }
        let subtitle = self.path.display().to_string();
        let body = widgets::header(ui.painter(), rect, "Import from Plex", &subtitle, ui.style());
        let preview = match &self.state {
            State::Reading(_) => {
                widgets::message(ui.painter(), body, "Reading the Plex database…", ui.style());
                return Nav::None;
            }
            State::Importing(_) => {
                widgets::message(ui.painter(), body, "Importing…", ui.style());
                return Nav::None;
            }
            State::Failed(e) => {
                widgets::message(ui.painter(), body, &format!("Couldn't read it: {e}"), ui.style());
                return Nav::None;
            }
            State::Ready(preview) => preview,
        };
        let plan = &preview.plan;
        let summary = format!(
            "Plex account “{}”: watch state for {} files and {} history entries can be imported. \
             Files already marked watched in Flex stay watched; resume points come from whichever \
             was played more recently. {} entries couldn't be matched to files (deleted, or in a \
             library Flex doesn't have).",
            preview.account,
            thousands(plan.progress.len()),
            thousands(plan.history.len()),
            thousands(plan.unmatched_items + plan.unmatched_views),
        );
        let mut list_rect = body;
        list_rect.min.y += widgets::paragraph(ui.painter(), body, &summary, 4, ui.style()) + 16.0;
        let mut rows = vec![
            Row::new(Icon::Add, "Import").detail("OK to import"),
            Row::new(Icon::Remove, "Cancel"),
        ];
        for s in &plan.sections {
            let target = s.library_id.and_then(|id| sh.lib(id)).map(|l| l.lib.name.clone());
            let (title, detail) = match target {
                Some(name) => (format!("{} → {name}", s.root), format!("{} of {} files", thousands(s.matched), thousands(s.total))),
                None => (format!("{} → not imported", s.root), "no matching Flex library".to_owned()),
            };
            rows.push(Row::new(Icon::Folder, title).detail(detail).dim(s.library_id.is_none()));
        }
        widgets::list(ui, list_rect, &rows, &mut self.list, focused);
        Nav::None
    }
}
