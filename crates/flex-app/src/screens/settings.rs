//! Settings: manage libraries (add, rescan, change type, remove), and app options.

use eframe::egui::{Rect, Ui};
use flex_core::LibraryKind;

use super::picker::{FolderPicker, Purpose};
use super::{Nav, Screen, plugins::Plugins, text_entry::TextEntry};
use crate::input::{Action, Press};
use crate::picture::{Upscale, svp};
use crate::shared::{LibraryView, Shared, WATCHED_THRESHOLDS};
use crate::widgets::{self, Icon, ListInput, ListState, Row};

/// Whether this platform has a system tray (see [`crate::tray`]).
const TRAY: bool = cfg!(any(windows, target_os = "macos"));

#[derive(Default)]
pub struct Settings {
    list: ListState,
}

impl Screen for Settings {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let libs = sh.libs.len();
        // Web access, Plugins, YouTube, skipping openings, Picture, Sync, Theme, "Close to system
        // tray" where there's a tray, "Start with Windows", then "Update automatically" in
        // installed copies.
        let tray_row = libs + 14;
        let autostart_row = tray_row + usize::from(TRAY);
        let update = crate::update::installed();
        let rows = autostart_row + usize::from(cfg!(windows)) + usize::from(update);
        match self.list.handle(press, rows) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(i) if i < libs => Nav::push(LibraryOptions::new(sh.libs[i].lib.id)),
            ListInput::Activate(i) if i == libs => Nav::push(FolderPicker::new(sh)),
            ListInput::Activate(i) if i == libs + 1 => {
                let current = sh.watched_threshold();
                let i = WATCHED_THRESHOLDS.iter().position(|t| (t - current).abs() < 0.001).unwrap_or(2);
                sh.set_watched_threshold(WATCHED_THRESHOLDS[(i + 1) % WATCHED_THRESHOLDS.len()]);
                Nav::None
            }
            ListInput::Activate(i) if i == libs + 2 => Nav::push(TextEntry::tmdb_token()),
            ListInput::Activate(i) if i == libs + 3 => Nav::push(FolderPicker::with_purpose(sh, Purpose::PlexDatabase)),
            ListInput::Activate(i) if i == libs + 4 => {
                sh.scan_all(false);
                sh.toast("Scanning all libraries for changes…");
                Nav::None
            }
            ListInput::Activate(i) if i == libs + 5 => {
                sh.scan_all(true);
                sh.toast("Rescanning all libraries from scratch…");
                Nav::None
            }
            ListInput::Activate(i) if i == libs + 6 => {
                crate::logging::open_logs_folder();
                Nav::None
            }
            ListInput::Activate(i) if i == libs + 7 => {
                let on = !sh.web_enabled();
                sh.set_web_enabled(on);
                if let Some(url) = sh.web_url().filter(|_| on) {
                    sh.toast(format!("Web UI on: open {url} on another device"));
                }
                Nav::None
            }
            ListInput::Activate(i) if i == libs + 8 => Nav::push(Plugins::default()),
            ListInput::Activate(i) if i == libs + 9 => Nav::push(super::youtube::YouTubeSettings::default()),
            ListInput::Activate(i) if i == libs + 10 => {
                let next = sh.skip_mode().next();
                sh.set_skip_mode(next);
                Nav::None
            }
            ListInput::Activate(i) if i == libs + 11 => Nav::push(PictureSettings::default()),
            ListInput::Activate(i) if i == libs + 12 => Nav::push(super::sync::SyncSettings::default()),
            ListInput::Activate(i) if i == libs + 13 => Nav::push(super::themes::Themes::default()),
            ListInput::Activate(i) if TRAY && i == tray_row => {
                sh.set_close_to_tray(!sh.close_to_tray);
                Nav::None
            }
            ListInput::Activate(i) if update && i == rows - 1 => {
                let on = !sh.auto_update();
                sh.set_auto_update(on);
                Nav::None
            }
            ListInput::Activate(i) if cfg!(windows) && i == autostart_row => {
                #[cfg(windows)]
                if let Err(e) = crate::autostart::set_enabled(!crate::autostart::is_enabled()) {
                    sh.toast(format!("Couldn't change the startup setting: {e:#}"));
                }
                Nav::None
            }
            ListInput::Activate(_) | ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let body = widgets::header(ui.painter(), rect, "Settings", "Libraries", ui.style());
        let mut rows: Vec<Row> = sh
            .libs
            .iter()
            .map(|l| {
                Row::new(Icon::Folder, &l.lib.name)
                    .detail(format!("{} · {} · {}", l.lib.kind.label(), l.status(), l.lib.root.display()))
            })
            .collect();
        rows.push(Row::new(Icon::Add, "Add library…"));
        let threshold = (sh.watched_threshold() * 100.0).round();
        rows.push(Row::new(Icon::None, format!("Mark as watched after {threshold}%")).detail("OK to change"));
        let tmdb = if sh.tmdb_token().is_some() { "connected" } else { "not set up" };
        rows.push(Row::new(Icon::None, format!("TMDB (titles & posters): {tmdb}")).detail("OK to enter a token"));
        rows.push(Row::new(Icon::Add, "Import from Plex…").detail("Watched state, resume points and history"));
        rows.push(Row::new(Icon::Refresh, "Scan all libraries for changes").detail("New, moved and deleted files"));
        rows.push(Row::new(Icon::Refresh, "Rescan all libraries from scratch").detail("Slower: looks at every file again"));
        rows.push(Row::new(Icon::Folder, "Open logs folder").detail(crate::logging::logs_dir().display().to_string()));
        rows.push(match sh.web_url() {
            Some(url) => Row::new(Icon::None, "Web access: on").detail(format!("Open {url} on phones and PCs at home")),
            None => Row::new(Icon::None, "Web access: off").detail("Browse your libraries from other devices at home"),
        });
        let plugins = sh.plugins.plugins();
        let on = plugins.iter().filter(|p| p.enabled).count();
        rows.push(Row::new(Icon::None, "Plugins…").detail(match plugins.len() {
            0 => "None installed".to_owned(),
            n => format!("{n} installed · {on} on"),
        }));
        let youtube = if sh.youtube_enabled() { "on" } else { "off" };
        rows.push(Row::new(Icon::None, format!("YouTube: {youtube}…")).detail("No ads, SponsorBlock, subscriptions"));
        rows.push(
            Row::new(Icon::None, format!("Skip openings and endings: {}", sh.skip_mode().label()))
                .detail("For files with OP/ED chapters · OK to change"),
        );
        rows.push(Row::new(Icon::None, format!("Picture: {}…", picture_summary(sh))).detail("Anime4K upscaling and 60fps (SVP)"));
        let sync = if sh.sync_dir().is_some() { "on" } else { "off" };
        rows.push(Row::new(Icon::None, format!("Sync with other devices: {sync}…")).detail("Share what you've watched through a shared folder"));
        rows.push(Row::new(Icon::None, format!("Theme: {}…", crate::theme::current().name)).detail("Dark, Final Fantasy VII, Old School RuneScape, Windows 95 and more"));
        if TRAY {
            let on = if sh.close_to_tray { "on" } else { "off" };
            rows.push(Row::new(Icon::None, format!("Close to system tray: {on}")).detail("Closing keeps Flex running in the tray, so it opens instantly"));
        }
        #[cfg(windows)]
        {
            let on = if crate::autostart::is_enabled() { "on" } else { "off" };
            let detail = if sh.close_to_tray { "Starts hidden in the tray" } else { "Opens Flex when you sign in" };
            rows.push(Row::new(Icon::None, format!("Start with Windows: {on}")).detail(detail));
        }
        if crate::update::installed() {
            let on = if sh.auto_update() { "on" } else { "off" };
            let version = crate::update::current_version();
            rows.push(
                Row::new(Icon::None, format!("Update automatically: {on}"))
                    .detail(format!("Version {version} · new versions install while nothing is playing")),
            );
        }
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

pub struct LibraryOptions {
    library_id: i64,
    confirm_remove: bool,
    list: ListState,
}

#[derive(Clone, Copy, PartialEq)]
enum LibraryChoice {
    Scan,
    Rescan,
    Rename,
    Kind,
    View,
    Rematch,
    Upscale,
    Smooth,
    Remove,
}

impl LibraryOptions {
    fn new(library_id: i64) -> Self {
        Self { library_id, confirm_remove: false, list: ListState::default() }
    }

    /// The rows, in order. The picture switches only show while they're on in Settings →
    /// Picture (and not for music).
    fn choices(&self, sh: &Shared) -> Vec<(Row, LibraryChoice)> {
        let Some(state) = sh.lib(self.library_id) else { return Vec::new() };
        let on_off = |on: bool| if on { "on" } else { "off" };
        let mut out = vec![
            (Row::new(Icon::Refresh, "Scan for changes").detail("New, moved and deleted files"), LibraryChoice::Scan),
            (Row::new(Icon::Refresh, "Rescan from scratch").detail("Slower: looks at every file again"), LibraryChoice::Rescan),
            (Row::new(Icon::None, format!("Name: {}", state.lib.name)).detail("OK to rename"), LibraryChoice::Rename),
            (Row::new(Icon::None, format!("Type: {}", state.lib.kind.label())).detail("OK to change"), LibraryChoice::Kind),
            (
                Row::new(Icon::None, match sh.library_view(self.library_id) {
                    LibraryView::Shows => "Opens in: Shows",
                    LibraryView::Folders => "Opens in: Folders",
                })
                .detail("OK to change"),
                LibraryChoice::View,
            ),
            (Row::new(Icon::Refresh, "Re-match online").detail("Redo automatic matches; keeps your fixes"), LibraryChoice::Rematch),
        ];
        if !state.is_music() {
            if sh.picture_upscale() != Upscale::Off {
                let on = sh.library_upscale(self.library_id);
                let row = Row::new(Icon::None, format!("Anime4K upscaling here: {}", on_off(on))).detail("OK to change");
                out.push((row, LibraryChoice::Upscale));
            }
            if sh.picture_smooth() {
                let on = sh.library_smooth(self.library_id);
                let row = Row::new(Icon::None, format!("60fps (SVP) here: {}", on_off(on))).detail("OK to change");
                out.push((row, LibraryChoice::Smooth));
            }
        }
        let remove = if self.confirm_remove {
            "Press OK again to remove (your files are not touched)"
        } else {
            "Remove library"
        };
        out.push((Row::new(Icon::Remove, remove), LibraryChoice::Remove));
        out
    }
}

impl Screen for LibraryOptions {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        let Some(state) = sh.lib(self.library_id) else { return Nav::Pop };
        let kind = state.lib.kind;
        let choices: Vec<LibraryChoice> = self.choices(sh).into_iter().map(|c| c.1).collect();
        let input = self.list.handle(press, choices.len());
        // Moving off "Remove" cancels a pending confirmation.
        self.confirm_remove &= choices.get(self.list.selected) == Some(&LibraryChoice::Remove);
        let choice = match input {
            ListInput::AtTop => return Nav::FocusTopBar,
            ListInput::Activate(i) => choices[i],
            ListInput::None => return Nav::None,
        };
        match choice {
            LibraryChoice::Scan => sh.start_scan(self.library_id, false),
            LibraryChoice::Rescan => sh.start_scan(self.library_id, true),
            LibraryChoice::Rename => return Nav::push(TextEntry::rename_library(sh, self.library_id, "Rename library")),
            LibraryChoice::Kind => {
                let i = LibraryKind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
                let next = LibraryKind::ALL[(i + 1) % LibraryKind::ALL.len()];
                if let Err(e) = sh.set_kind(self.library_id, next) {
                    sh.toast(format!("{e:#}"));
                }
            }
            LibraryChoice::View => {
                let view = match sh.library_view(self.library_id) {
                    LibraryView::Shows => LibraryView::Folders,
                    LibraryView::Folders => LibraryView::Shows,
                };
                sh.set_library_view(self.library_id, view);
            }
            LibraryChoice::Rematch => {
                sh.rematch(self.library_id);
                sh.toast("Re-matching… shows you fixed by hand are kept");
            }
            LibraryChoice::Upscale => {
                let on = !sh.library_upscale(self.library_id);
                sh.set_library_upscale(self.library_id, on);
            }
            LibraryChoice::Smooth => {
                let on = !sh.library_smooth(self.library_id);
                sh.set_library_smooth(self.library_id, on);
            }
            LibraryChoice::Remove if !self.confirm_remove => self.confirm_remove = true,
            LibraryChoice::Remove => {
                if let Err(e) = sh.remove_library(self.library_id) {
                    sh.toast(format!("{e:#}"));
                    return Nav::None;
                }
                sh.toast("Library removed");
                return Nav::Pop;
            }
        }
        Nav::None
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let Some(state) = sh.lib(self.library_id) else { return Nav::Pop };
        let subtitle = format!("{} · {}", state.lib.root.display(), state.status());
        let body = widgets::header(ui.painter(), rect, &state.lib.name, &subtitle, ui.style());
        let rows: Vec<Row> = self.choices(sh).into_iter().map(|c| c.0).collect();
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}

/// "off", "Anime4K", "Anime4K + 60fps", "60fps".
fn picture_summary(sh: &Shared) -> &'static str {
    match (sh.picture_upscale() != Upscale::Off, sh.picture_smooth()) {
        (false, false) => "off",
        (true, false) => "Anime4K",
        (true, true) => "Anime4K + 60fps",
        (false, true) => "60fps",
    }
}

/// Settings → Picture: Anime4K upscaling and 60fps smoothing through SVP, for every
/// library (each can opt out in its options).
#[derive(Default)]
pub struct PictureSettings {
    list: ListState,
}

impl Screen for PictureSettings {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        if press.is(Action::Back) {
            return Nav::Pop;
        }
        match self.list.handle(press, 2) {
            ListInput::AtTop => Nav::FocusTopBar,
            ListInput::Activate(0) => {
                let next = sh.picture_upscale().next();
                sh.set_picture_upscale(next);
                Nav::None
            }
            ListInput::Activate(_) => {
                let on = !sh.picture_smooth();
                sh.set_picture_smooth(on);
                Nav::None
            }
            ListInput::None => Nav::None,
        }
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav {
        let body = widgets::header(ui.painter(), rect, "Picture", "For every library; switch them off in a library's options", ui.style());
        let upscale = sh.picture_upscale();
        let smooth = if sh.picture_smooth() { "on" } else { "off" };
        let svp = match svp::find() {
            Some(_) => "Adds frames with SVP, which must be running",
            None => "Needs SVP (svp-team.com), which isn't installed",
        };
        let rows = [
            Row::new(Icon::None, format!("Anime4K upscaling: {}", upscale.label()))
                .detail("Sharper lines on lower-resolution video, made for anime · OK to change"),
            Row::new(Icon::None, format!("60fps smoothing: {smooth}")).detail(svp),
        ];
        widgets::list(ui, body, &rows, &mut self.list, focused);
        Nav::None
    }
}
