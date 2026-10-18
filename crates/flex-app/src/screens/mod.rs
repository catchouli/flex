//! Screens live on a stack: Back pops to whatever you came from, so the player and
//! end screen always return to the view playback was started from.

pub mod browse;
pub mod details;
pub mod history;
pub mod music;
pub mod home;
pub mod inspector;
pub mod library;
pub mod match_online;
pub mod options;
pub mod picker;
pub mod person;
pub mod plex_import;
pub mod plugins;
pub mod text_entry;
pub mod resume;
pub mod screenshots;
pub mod search;
pub mod settings;
pub mod shows;
pub mod sync;
pub mod up_next;
pub mod youtube;

use eframe::egui::{Pos2, Rect, Ui};

use crate::input::Press;
use crate::shared::{PlayRequest, Shared};

pub enum Nav {
    None,
    Push(Box<dyn Screen>),
    Pop,
    Replace(Box<dyn Screen>),
    /// Start playback; `replace` swaps out the current screen instead of stacking on it.
    Play { req: PlayRequest, replace: bool },
    /// Open `screen`, then play on top of it, so Back from the player lands there
    /// (e.g. the show's page when starting from Continue Watching).
    PlayOver { screen: Box<dyn Screen>, req: PlayRequest },
    /// Move input focus up to the top bar.
    FocusTopBar,
}

impl Nav {
    pub fn push(screen: impl Screen + 'static) -> Self {
        Self::Push(Box::new(screen))
    }

    pub fn replace(screen: impl Screen + 'static) -> Self {
        Self::Replace(Box::new(screen))
    }

    pub fn play(req: Option<PlayRequest>) -> Self {
        req.map_or(Self::None, |req| Self::Play { req, replace: false })
    }
}

pub trait Screen {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav;

    /// Draws into `rect`. `focused` is false while the top bar has input focus.
    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, rect: Rect, focused: bool) -> Nav;

    /// Full-screen screens (player, up next) hide the top bar.
    fn shows_top_bar(&self) -> bool {
        true
    }

    fn is_search(&self) -> bool {
        false
    }

    fn is_player(&self) -> bool {
        false
    }

    /// For players: what's playing, for plugins subscribed to `playback`.
    fn playback(&self) -> Option<flex_plugin::Playback> {
        None
    }

    /// For the video player: the library file playing, as (library id, file id).
    fn playing_file(&self) -> Option<(i64, i64)> {
        None
    }

    /// For the player: whether `pos` is on its on-screen controls, where a click
    /// shouldn't pause or play.
    fn on_controls(&self, _pos: Pos2) -> bool {
        false
    }

    /// Called when the screen leaves the stack: popped, replaced, or the app closing.
    fn on_leave(&mut self, _sh: &mut Shared) {}

    /// For the search screen: open the best result (Enter in the search box).
    fn activate_first(&mut self, _sh: &mut Shared) -> Nav {
        Nav::None
    }
}
