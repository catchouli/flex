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
    /// Remote next-track button: next episode.
    #[cfg_attr(not(windows), allow(dead_code))]
    Next,
    /// Remote previous-track button: previous episode.
    #[cfg_attr(not(windows), allow(dead_code))]
    Previous,
    /// Page Down (remote or keyboard): jump down a page in lists.
    PageDown,
    /// Page Up: jump up a page in lists.
    PageUp,
    /// Remote zoom-out or search button, Ctrl/Cmd+F: focuses the search box.
    /// (The remote's own search button also opens Windows search, so zoom-out is nicer.)
    Search,
    /// Remote zoom-in (HID zoom-in / Ctrl + "=" / Ctrl+wheel up): toggles fullscreen.
    ZoomIn,
    /// Holding OK, or right-click: options for the selected item (mark watched, ...).
    Menu,
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
        Key::Space => Action::PlayPause,
        Key::PageDown => Action::PageDown,
        Key::PageUp => Action::PageUp,
        // Remotes usually send BrowserBack for their Back button.
        Key::Escape | Key::Backspace | Key::BrowserBack => Action::Back,
        _ => return None,
    })
}

/// Ctrl/Cmd shortcuts.
fn map_command_key(key: Key) -> Option<Action> {
    match key {
        Key::Plus | Key::Equals => Some(Action::ZoomIn),
        Key::Minus | Key::F => Some(Action::Search),
        _ => None,
    }
}

const BACK_KEYS: [Key; 3] = [Key::Escape, Key::Backspace, Key::BrowserBack];

/// Whether a Back key is currently held down (for hold-to-quit).
pub fn back_held(ctx: &egui::Context) -> bool {
    ctx.input(|i| BACK_KEYS.iter().any(|k| i.key_down(*k)))
}

const LOG_LEN: usize = 12;
/// Holding OK this long opens the options menu instead of activating.
pub const LONG_PRESS_SECS: f64 = 0.5;
/// Ctrl+wheel sends a burst of events per flick; treat it as one press at most this often.
/// (Buttons are single events and aren't limited.)
const WHEEL_ZOOM_DEBOUNCE_SECS: f64 = 0.8;

#[derive(Default)]
pub struct InputLog {
    entries: VecDeque<String>,
    last_wheel_zoom: Option<f64>,
    /// When OK (Enter) went down, and whether that press already became a Menu.
    ok_down: Option<(f64, bool)>,
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
/// While a text field has focus only remote buttons are mapped, so typing works normally.
pub fn collect(ctx: &egui::Context, log: &mut InputLog, text_focused: bool) -> Vec<Press> {
    let mut presses = Vec::new();
    for line in os::drain_diagnostics() {
        log.push(line);
    }
    for (raw, action) in os::drain() {
        log.push(format!("{raw}  ->  {action:?}"));
        presses.push(Press { action, repeat: false });
    }
    if text_focused {
        log.ok_down = None;
        return presses;
    }
    // OK fires on release so that holding it can open the menu instead.
    let (now, enter_down) = ctx.input(|i| (i.time, i.key_down(Key::Enter)));
    let mut open_menu = false;
    if let Some((since, fired)) = &mut log.ok_down
        && !*fired
        && enter_down
    {
        let held = now - *since;
        if held >= LONG_PRESS_SECS {
            *fired = true;
            open_menu = true;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(LONG_PRESS_SECS - held));
        }
    }
    if open_menu {
        log.push("Key Enter (held)  ->  Menu".to_owned());
        presses.push(Press { action: Action::Menu, repeat: false });
    }
    ctx.input(|i| {
        for event in &i.events {
            let (raw, action, repeat) = match event {
                Event::Key { key: Key::Enter, pressed, repeat, modifiers, .. } if modifiers.is_none() => {
                    match (*pressed, *repeat) {
                        (true, false) => {
                            log.ok_down = Some((now, false));
                            continue;
                        }
                        (false, _) => match log.ok_down.take() {
                            Some((_, false)) => ("Key Enter".to_owned(), Some(Action::Ok), false),
                            _ => continue, // already opened the menu, or the press began elsewhere
                        },
                        (true, true) => continue,
                    }
                }
                Event::Key { key, physical_key, pressed: true, repeat, modifiers } => {
                    let physical = physical_key.filter(|p| p != key).map(|p| format!(" (physical {p:?})"));
                    let mods = (!modifiers.is_none()).then(|| format!(" +{modifiers:?}"));
                    let raw = format!("Key {key:?}{}{}", physical.unwrap_or_default(), mods.unwrap_or_default());
                    let action = match (modifiers.command, modifiers.alt) {
                        // Remotes send physical key codes, so check those too: on non-US
                        // layouts (e.g. Dvorak) the physical "=" key isn't "=".
                        (true, false) => map_command_key(*key).or_else(|| physical_key.and_then(map_command_key)),
                        (false, false) => map_key(*key),
                        // Leave other shortcuts alone.
                        _ => None,
                    };
                    (raw, action, *repeat)
                }
                Event::PointerButton { button, pressed: true, .. } => {
                    let action = match button {
                        PointerButton::Extra1 => Some(Action::Back),
                        PointerButton::Secondary => Some(Action::Menu),
                        _ => None,
                    };
                    (format!("Mouse {button:?}"), action, false)
                }
                // Some air mice send Ctrl + scroll wheel for their zoom buttons. Only
                // zooming in is used (fullscreen); zooming out would be too easy to trigger.
                Event::MouseWheel { delta, modifiers, .. } if modifiers.command && delta.y > 0.0 => {
                    if log.last_wheel_zoom.is_some_and(|t| now - t < WHEEL_ZOOM_DEBOUNCE_SECS) {
                        continue;
                    }
                    log.last_wheel_zoom = Some(now);
                    (format!("Ctrl+Wheel {:+.0}", delta.y), Some(Action::ZoomIn), false)
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

/// Remote buttons egui doesn't report (media keys, search, HID zoom). On Windows they're
/// caught from the raw window messages via winit's message hook; on Linux they're read
/// from /dev/input (the user must be in the `input` group); macOS keeps media keys for its
/// own Now Playing, so there the keyboard equivalents are used instead.
pub mod os {
    #[cfg(windows)]
    pub use windows::{drain, drain_diagnostics, hook, set_context, set_diagnostics};

    #[cfg(target_os = "linux")]
    pub use linux::{drain, drain_diagnostics, set_context, set_diagnostics};

    #[cfg(target_os = "linux")]
    mod linux {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Mutex, OnceLock};

        use eframe::egui;
        use evdev::KeyCode;

        use super::super::Action;

        static QUEUE: Mutex<Vec<(String, Action)>> = Mutex::new(Vec::new());
        static DIAGNOSTICS: Mutex<Vec<String>> = Mutex::new(Vec::new());
        static DIAGNOSTICS_ON: AtomicBool = AtomicBool::new(false);
        static CTX: OnceLock<egui::Context> = OnceLock::new();

        /// The buttons egui doesn't report. Arrows, Enter and Back arrive as ordinary keys.
        fn action(key: KeyCode) -> Option<Action> {
            Some(match key {
                KeyCode::KEY_PLAYPAUSE | KeyCode::KEY_PLAY | KeyCode::KEY_PAUSE | KeyCode::KEY_PLAYCD | KeyCode::KEY_PAUSECD => Action::PlayPause,
                KeyCode::KEY_STOP | KeyCode::KEY_STOPCD => Action::Stop,
                KeyCode::KEY_NEXTSONG => Action::Next,
                KeyCode::KEY_PREVIOUSSONG => Action::Previous,
                KeyCode::KEY_SEARCH => Action::Search,
                KeyCode::KEY_ZOOMIN | KeyCode::KEY_ZOOM => Action::ZoomIn,
                KeyCode::KEY_ZOOMOUT => Action::Search,
                _ => return None,
            })
        }

        /// Starts a reader for each input device with any of those buttons.
        pub fn set_context(ctx: &egui::Context) {
            if CTX.set(ctx.clone()).is_err() {
                return;
            }
            let mut found = 0;
            for (path, device) in evdev::enumerate() {
                let has_media = device.supported_keys().is_some_and(|keys| keys.iter().any(|k| action(k).is_some()));
                if !has_media {
                    continue;
                }
                found += 1;
                let name = device.name().unwrap_or("?").to_owned();
                log::info!("remote: reading {} ({name})", path.display());
                std::thread::Builder::new().name("remote-input".into()).spawn(move || read(device, &name)).ok();
            }
            if found == 0 {
                // Usually permissions: /dev/input needs the `input` group.
                log::info!("remote: no readable input devices with media keys (is this user in the `input` group?)");
            }
        }

        fn read(mut device: evdev::Device, name: &str) {
            loop {
                let events = match device.fetch_events() {
                    Ok(events) => events.collect::<Vec<_>>(),
                    Err(e) => {
                        log::info!("remote: {name}: {e}");
                        return;
                    }
                };
                for event in events {
                    let evdev::EventSummary::Key(_, key, value) = event.destructure() else { continue };
                    if DIAGNOSTICS_ON.load(Ordering::Relaxed) {
                        DIAGNOSTICS.lock().unwrap_or_else(|e| e.into_inner()).push(format!("evdev {key:?} value {value} ({name})"));
                    }
                    // 1 = pressed (0 released, 2 auto-repeat).
                    if value == 1
                        && let Some(action) = action(key)
                    {
                        QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push((format!("{key:?}"), action));
                        if let Some(ctx) = CTX.get() {
                            ctx.request_repaint();
                        }
                    }
                }
            }
        }

        pub fn drain() -> Vec<(String, Action)> {
            std::mem::take(&mut *QUEUE.lock().unwrap_or_else(|e| e.into_inner()))
        }

        pub fn drain_diagnostics() -> Vec<String> {
            std::mem::take(&mut *DIAGNOSTICS.lock().unwrap_or_else(|e| e.into_inner()))
        }

        pub fn set_diagnostics(on: bool) {
            DIAGNOSTICS_ON.store(on, Ordering::Relaxed);
        }
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    mod fallback {
        use super::super::Action;

        pub fn drain() -> Vec<(String, Action)> {
            Vec::new()
        }

        pub fn drain_diagnostics() -> Vec<String> {
            Vec::new()
        }

        pub fn set_context(_ctx: &eframe::egui::Context) {}

        pub fn set_diagnostics(_on: bool) {}
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    pub use fallback::*;

    #[cfg(windows)]
    mod windows {
        use std::ffi::c_void;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Mutex, OnceLock};

        use eframe::egui;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            VK_BROWSER_SEARCH, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK, VK_MEDIA_STOP, VK_ZOOM,
        };
        use windows_sys::Win32::UI::Input::{
            GetRawInputData, HRAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT, RIM_TYPEHID,
            RegisterRawInputDevices,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, WM_APPCOMMAND, WM_INPUT, WM_KEYDOWN, WM_SYSKEYDOWN};

        use super::super::Action;

        static QUEUE: Mutex<Vec<(String, Action)>> = Mutex::new(Vec::new());
        static DIAGNOSTICS: Mutex<Vec<String>> = Mutex::new(Vec::new());
        static DIAGNOSTICS_ON: AtomicBool = AtomicBool::new(false);
        static CTX: OnceLock<egui::Context> = OnceLock::new();

        // APPCOMMAND_* values from winuser.h.
        const APPCOMMAND_BROWSER_SEARCH: u32 = 5;
        const APPCOMMAND_MEDIA_NEXTTRACK: u32 = 11;
        const APPCOMMAND_MEDIA_PREVIOUSTRACK: u32 = 12;
        const APPCOMMAND_MEDIA_STOP: u32 = 13;
        const APPCOMMAND_MEDIA_PLAY_PAUSE: u32 = 14;
        const APPCOMMAND_MEDIA_PLAY: u32 = 46;
        const APPCOMMAND_MEDIA_PAUSE: u32 = 47;

        // HID consumer page usages Windows doesn't turn into keys.
        const HID_USAGE_PAGE_CONSUMER: u16 = 0x0C;
        const HID_USAGE_CONSUMER_CONTROL: u16 = 0x01;
        const AC_ZOOM_IN: u16 = 0x022D;
        const AC_ZOOM_OUT: u16 = 0x022E;

        /// Also registers for consumer-control raw input, so buttons Windows doesn't map to
        /// keys (like zoom) still reach us as WM_INPUT. Must run on the UI thread.
        pub fn set_context(ctx: &egui::Context) {
            let _ = CTX.set(ctx.clone());
            let device = RAWINPUTDEVICE {
                usUsagePage: HID_USAGE_PAGE_CONSUMER,
                usUsage: HID_USAGE_CONSUMER_CONTROL,
                dwFlags: 0,
                hwndTarget: std::ptr::null_mut(), // follows keyboard focus
            };
            let ok = unsafe { RegisterRawInputDevices(&device, 1, size_of::<RAWINPUTDEVICE>() as u32) };
            if ok == 0 {
                log::warn!("couldn't register for remote (consumer control) input");
            }
        }

        /// While on, every raw key / HID message is logged for the input tester.
        pub fn set_diagnostics(on: bool) {
            DIAGNOSTICS_ON.store(on, Ordering::Relaxed);
        }

        fn diagnose(line: impl FnOnce() -> String) {
            if DIAGNOSTICS_ON.load(Ordering::Relaxed) {
                DIAGNOSTICS.lock().unwrap_or_else(|e| e.into_inner()).push(line());
                if let Some(ctx) = CTX.get() {
                    ctx.request_repaint();
                }
            }
        }

        fn push(raw: String, action: Action) {
            QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push((raw, action));
            if let Some(ctx) = CTX.get() {
                ctx.request_repaint();
            }
        }

        /// Reads a WM_INPUT HID report and returns any zoom usage in it.
        fn raw_hid_action(handle: HRAWINPUT) -> Option<Action> {
            let header = size_of::<RAWINPUTHEADER>() as u32;
            let mut size = 0u32;
            unsafe { GetRawInputData(handle, RID_INPUT, std::ptr::null_mut(), &mut size, header) };
            let mut buf = vec![0u8; size as usize];
            let read = unsafe { GetRawInputData(handle, RID_INPUT, buf.as_mut_ptr().cast(), &mut size, header) };
            if read == u32::MAX || (read as usize) < header as usize + 8 {
                return None;
            }
            let kind = u32::from_ne_bytes(buf[0..4].try_into().ok()?);
            if kind != RIM_TYPEHID {
                return None;
            }
            // RAWHID: dwSizeHid, dwCount, then dwCount reports of dwSizeHid bytes.
            let at = header as usize;
            let report_size = u32::from_ne_bytes(buf[at..at + 4].try_into().ok()?) as usize;
            let count = u32::from_ne_bytes(buf[at + 4..at + 8].try_into().ok()?) as usize;
            let data = buf.get(at + 8..at + 8 + report_size * count)?;
            let mut action = None;
            for report in data.chunks(report_size.max(1)) {
                diagnose(|| {
                    let hex: Vec<String> = report.iter().map(|b| format!("{b:02X}")).collect();
                    format!("HID consumer report: {}", hex.join(" "))
                });
                // Reports are usually [report id] + 16-bit little-endian usages.
                for pair in report.windows(2) {
                    match u16::from_le_bytes([pair[0], pair[1]]) {
                        AC_ZOOM_IN => action = Some(Action::ZoomIn),
                        AC_ZOOM_OUT => action = Some(Action::Search),
                        _ => {}
                    }
                }
            }
            action
        }

        /// winit message hook: returns true for messages it consumed.
        pub fn hook(msg: *const c_void) -> bool {
            // Safety: winit passes a pointer to the MSG being dispatched.
            let msg = unsafe { &*(msg as *const MSG) };
            match msg.message {
                WM_KEYDOWN | WM_SYSKEYDOWN => {
                    let vk = msg.wParam as u16;
                    diagnose(|| format!("Windows key VK 0x{vk:02X} scan 0x{:02X}", (msg.lParam >> 16) & 0xFF));
                    let action = match vk {
                        VK_MEDIA_PLAY_PAUSE => Action::PlayPause,
                        VK_MEDIA_STOP => Action::Stop,
                        VK_MEDIA_NEXT_TRACK => Action::Next,
                        VK_MEDIA_PREV_TRACK => Action::Previous,
                        VK_BROWSER_SEARCH => Action::Search,
                        VK_ZOOM => Action::ZoomIn,
                        _ => return false,
                    };
                    push(format!("Windows key VK 0x{vk:02X}"), action);
                    true
                }
                WM_APPCOMMAND => {
                    let command = ((msg.lParam as u32) >> 16) & !0xF000;
                    diagnose(|| format!("Windows app command {command}"));
                    let action = match command {
                        APPCOMMAND_MEDIA_PLAY_PAUSE | APPCOMMAND_MEDIA_PLAY | APPCOMMAND_MEDIA_PAUSE => Action::PlayPause,
                        APPCOMMAND_MEDIA_STOP => Action::Stop,
                        APPCOMMAND_MEDIA_NEXTTRACK => Action::Next,
                        APPCOMMAND_MEDIA_PREVIOUSTRACK => Action::Previous,
                        APPCOMMAND_BROWSER_SEARCH => Action::Search,
                        _ => return false,
                    };
                    push(format!("Windows app command {command}"), action);
                    true
                }
                WM_INPUT => {
                    if let Some(action) = raw_hid_action(msg.lParam as HRAWINPUT) {
                        push("HID consumer control".to_owned(), action);
                    }
                    false // let winit/DefWindowProc finish handling WM_INPUT
                }
                _ => false,
            }
        }

        pub fn drain() -> Vec<(String, Action)> {
            std::mem::take(&mut *QUEUE.lock().unwrap_or_else(|e| e.into_inner()))
        }

        pub fn drain_diagnostics() -> Vec<String> {
            std::mem::take(&mut *DIAGNOSTICS.lock().unwrap_or_else(|e| e.into_inner()))
        }
    }
}
