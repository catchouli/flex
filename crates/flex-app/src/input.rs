//! Maps keyboard / remote / mouse input to the six actions the UI understands,
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
}

#[derive(Clone, Copy, Debug)]
pub struct Press {
    pub action: Action,
    /// True for auto-repeat events while a key is held.
    pub repeat: bool,
}

fn map_key(key: Key) -> Option<Action> {
    Some(match key {
        Key::ArrowUp => Action::Up,
        Key::ArrowDown => Action::Down,
        Key::ArrowLeft => Action::Left,
        Key::ArrowRight => Action::Right,
        Key::Enter | Key::Space => Action::Ok,
        // Remotes usually send BrowserBack for their Back button.
        Key::Escape | Key::Backspace | Key::BrowserBack => Action::Back,
        _ => return None,
    })
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

    pub fn latest(&self) -> Option<&str> {
        self.entries.front().map(String::as_str)
    }
}

/// Collects this frame's actions, recording every raw key / button press in `log`.
pub fn collect(ctx: &egui::Context, log: &mut InputLog) -> Vec<Press> {
    let mut presses = Vec::new();
    ctx.input(|i| {
        for event in &i.events {
            let (raw, action, repeat) = match event {
                Event::Key { key, physical_key, pressed: true, repeat, modifiers } => {
                    let physical = physical_key.filter(|p| p != key).map(|p| format!(" (physical {p:?})"));
                    let mods = (!modifiers.is_none()).then(|| format!(" +{modifiers:?}"));
                    let raw = format!("Key {key:?}{}{}", physical.unwrap_or_default(), mods.unwrap_or_default());
                    (raw, map_key(*key), *repeat)
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
