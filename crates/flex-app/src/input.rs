//! Maps keyboard / remote / mouse input to the actions the UI understands,
//! and keeps a short log of raw input for the on-screen input tester.

use std::collections::VecDeque;

use eframe::egui::{self, Event, Key, PointerButton};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
    /// Space or the remote's play/pause button. Acts like OK outside the player.
    PlayPause,
    /// The remote's stop button (Windows only: macOS keeps media keys for itself).
    #[cfg_attr(not(windows), allow(dead_code))]
    Stop,
}

#[derive(Clone, Copy, Debug)]
pub struct Press {
    pub action: Action,
    /// True for auto-repeat events while a key is held.
    pub repeat: bool,
}

impl Press {
    pub fn is(&self, action: Action) -> bool {
        self.action == action && !self.repeat
    }

    /// OK or Play/Pause, which both mean "go" outside the player.
    pub fn is_activate(&self) -> bool {
        !self.repeat && matches!(self.action, Action::Ok | Action::PlayPause)
    }
}

fn map_key(key: Key) -> Option<Action> {
    Some(match key {
        Key::ArrowUp => Action::Up,
        Key::ArrowDown => Action::Down,
        Key::ArrowLeft => Action::Left,
        Key::ArrowRight => Action::Right,
        Key::Enter => Action::Ok,
        Key::Space => Action::PlayPause,
        // Remotes usually send BrowserBack for their Back button.
        Key::Escape | Key::Backspace | Key::BrowserBack => Action::Back,
        _ => return None,
    })
}

const BACK_KEYS: [Key; 3] = [Key::Escape, Key::Backspace, Key::BrowserBack];

/// Whether a Back key is currently held down (for hold-to-quit).
pub fn back_held(ctx: &egui::Context) -> bool {
    ctx.input(|i| BACK_KEYS.iter().any(|k| i.key_down(*k)))
}

const LOG_LEN: usize = 12;

#[derive(Default)]
pub struct InputLog {
    entries: VecDeque<String>,
}

impl InputLog {
    fn push(&mut self, entry: String) {
        if self.entries.len() == LOG_LEN {
            self.entries.pop_back();
        }
        self.entries.push_front(entry);
    }

    /// Most recent first.
    pub fn entries(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }
}

/// Collects this frame's actions, recording every raw key / button press in `log`.
/// While a text field has focus only media keys are mapped, so typing works normally.
pub fn collect(ctx: &egui::Context, log: &mut InputLog, text_focused: bool) -> Vec<Press> {
    let mut presses = Vec::new();
    for action in media::drain() {
        log.push(format!("Media key  ->  {action:?}"));
        presses.push(Press { action, repeat: false });
    }
    if text_focused {
        return presses;
    }
    ctx.input(|i| {
        for event in &i.events {
            let (raw, action, repeat) = match event {
                Event::Key { key, physical_key, pressed: true, repeat, modifiers } => {
                    let physical = physical_key.filter(|p| p != key).map(|p| format!(" (physical {p:?})"));
                    let mods = (!modifiers.is_none()).then(|| format!(" +{modifiers:?}"));
                    let raw = format!("Key {key:?}{}{}", physical.unwrap_or_default(), mods.unwrap_or_default());
                    // Leave shortcuts like Cmd+Backspace alone.
                    let action = if modifiers.command || modifiers.alt { None } else { map_key(*key) };
                    (raw, action, *repeat)
                }
                Event::PointerButton { button, pressed: true, .. } => {
                    let action = (*button == PointerButton::Extra1).then_some(Action::Back);
                    (format!("Mouse {button:?}"), action, false)
                }
                _ => continue,
            };
            if !repeat {
                let mapped = action.map_or("unmapped".to_owned(), |a| format!("{a:?}"));
                log.push(format!("{raw}  ->  {mapped}"));
            }
            if let Some(action) = action {
                presses.push(Press { action, repeat });
            }
        }
    });
    presses
}

/// Media keys (play/pause, stop). egui doesn't report these, so on Windows they're caught
/// from the raw window messages via winit's message hook.
pub mod media {
    use super::Action;

    #[cfg(windows)]
    mod imp {
        use std::ffi::c_void;
        use std::sync::{Mutex, OnceLock};

        use eframe::egui;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_MEDIA_PLAY_PAUSE, VK_MEDIA_STOP};
        use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, WM_APPCOMMAND, WM_KEYDOWN};

        use super::Action;

        static QUEUE: Mutex<Vec<Action>> = Mutex::new(Vec::new());
        static CTX: OnceLock<egui::Context> = OnceLock::new();

        // APPCOMMAND_* values from winuser.h.
        const APPCOMMAND_MEDIA_STOP: u32 = 13;
        const APPCOMMAND_MEDIA_PLAY_PAUSE: u32 = 14;
        const APPCOMMAND_MEDIA_PLAY: u32 = 46;
        const APPCOMMAND_MEDIA_PAUSE: u32 = 47;

        pub fn set_context(ctx: &egui::Context) {
            let _ = CTX.set(ctx.clone());
        }

        /// winit message hook: returns true for messages it consumed.
        pub fn hook(msg: *const c_void) -> bool {
            // Safety: winit passes a pointer to the MSG being dispatched.
            let msg = unsafe { &*(msg as *const MSG) };
            let action = match msg.message {
                WM_KEYDOWN => match msg.wParam as u16 {
                    VK_MEDIA_PLAY_PAUSE => Action::PlayPause,
                    VK_MEDIA_STOP => Action::Stop,
                    _ => return false,
                },
                WM_APPCOMMAND => match ((msg.lParam as u32) >> 16) & !0xF000 {
                    APPCOMMAND_MEDIA_PLAY_PAUSE | APPCOMMAND_MEDIA_PLAY | APPCOMMAND_MEDIA_PAUSE => Action::PlayPause,
                    APPCOMMAND_MEDIA_STOP => Action::Stop,
                    _ => return false,
                },
                _ => return false,
            };
            QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push(action);
            if let Some(ctx) = CTX.get() {
                ctx.request_repaint();
            }
            true
        }

        pub fn drain() -> Vec<Action> {
            std::mem::take(&mut *QUEUE.lock().unwrap_or_else(|e| e.into_inner()))
        }
    }

    #[cfg(windows)]
    pub use imp::{hook, set_context};

    #[cfg(windows)]
    pub fn drain() -> Vec<Action> {
        imp::drain()
    }

    /// macOS routes media keys to the system's Now Playing; Space covers play/pause there.
    #[cfg(not(windows))]
    pub fn drain() -> Vec<Action> {
        Vec::new()
    }

    #[cfg(not(windows))]
    pub fn set_context(_ctx: &eframe::egui::Context) {}
}
