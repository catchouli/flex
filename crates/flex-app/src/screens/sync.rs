//! Settings → Sync: share what you've watched with Flex on other devices through a
//! shared folder (see `crate::sync`).

use eframe::egui::{Rect, Ui};

use super::picker::{FolderPicker, Purpose};
use super::{Nav, Screen, text_entry::TextEntry};
use crate::input::{Action, Press};
use crate::shared::{Shared, now_unix};
use crate::widgets::{self, Icon, ListInput, ListState, Row};

#[derive(Default)]
pub struct SyncSettings {
    list: ListState,
}

/// "Synced 2 minutes ago with TV and Projector", or why not.
fn status(sh: &Shared) -> String {
    let Some(sync) = &sh.sync else { return "Off: choose a folder every device can reach".to_owned() };
    if let Some(e) = &sync.error {
        return format!("Problem: {e}");
    }
    let now = now_unix();
    let peers: Vec<String> = sync.peers.iter().map(|(name, at)| format!("{name} ({})", crate::youtube::ago(*at, now))).collect();
    match (sync.last_written, peers.is_empty()) {
        (None, _) => "Starting…".to_owned(),
        (Some(_), true) => "On · no other devices yet".to_owned(),
        (Some(_), false) => format!("On · other devices: {}", peers.join(", ")),
    }
}

impl Screen for SyncSettings {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let rows = if sh.sync_dir().is_some() { 4 } else { 2 };
        match self.list.handle(press, rows) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(0) => Nav::push(FolderPicker::with_purpose(sh, Purpose::SyncFolder)),
            ListInput::Activate(1) => Nav::push(TextEntry::sync_name(sh)),
            ListInput::Activate(2) => {
                sh.nudge_sync();
                sh.toast("Syncing…");
                Nav::None
            }
            ListInput::Activate(_) => {
                sh.set_sync_dir(None);
                sh.toast("Sync off");
                Nav::None
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let body = widgets::header(ui.painter(), rect, "Sync", &status(sh), ui.style());
        let folder = sh.sync_dir().map_or_else(|| "not set".to_owned(), |d| d.display().to_string());
        let mut rows = vec![
            Row::new(Icon::Folder, format!("Folder: {folder}")).detail("Watched, history, watchlist, fixed matches, YouTube"),
            Row::new(Icon::None, format!("This device: {}", sh.sync_name())).detail("OK to rename"),
        ];
        if sh.sync_dir().is_some() {
            rows.push(Row::new(Icon::Refresh, "Sync now").detail("It also syncs every 30 seconds"));
            rows.push(Row::new(Icon::Remove, "Turn sync off").detail("Keeps everything here as it is"));
        }
        widgets::list(ui, body, &rows, &mut self.list, focused);
        // Keep "2 minutes ago" current.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(10));
        Nav::None
    }
}
