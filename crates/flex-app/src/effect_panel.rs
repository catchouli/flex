//! The effects panel: hold OK (or right-click) while a video plays to turn effects off or on,
//! switch effect, change its settings and set up shuffle, watching the result beside it.
//!
//! Up/Down: pick a row · Left/Right: change it · OK: next value, or open what has an arrow ·
//! Back: out a level. With a mouse, click a row, or click or drag along one with a bar.
//!
//! In the list of effects: Left/Right: another section · OK: keep it · Back: go back to the one
//! before · Hold OK or right-click: in or out of the shuffle, or a favourite with shuffle off.

use eframe::egui::{self, Align2, FontId, Pos2, Rect, TextStyle, pos2, vec2};
use flex_core::effects::{Drift, Effect, Shuffle, SoundEffect, sound_settings_id};
use flex_core::plugins::{SettingKind, SettingSpec};
use flex_plugin::serde_json::{Value, json};

use crate::input::{Action, Press};
use crate::shared::Shared;
use crate::theme;
use crate::widgets;

const WIDTH: f32 = 660.0;
const ROW_HEIGHT: f32 = 60.0;
const LIST_ROW_HEIGHT: f32 = 48.0;
const ROW_GAP: f32 = 4.0;
const PADDING: f32 = 16.0;
const HEADER: f32 = 56.0;
const FOOTER: f32 = 100.0;
/// How much of the right of a row its arrow takes.
const ARROW: f32 = 52.0;
/// How long the list has to rest on an effect before it's shown, so scrolling past doesn't
/// load every shader on the way, in seconds.
const REST: f64 = 0.33;
/// (lowest, highest, step), in seconds.
const TURN: (f32, f32, f32) = (5.0, 600.0, 5.0);
const FADE: (f32, f32, f32) = (0.0, 15.0, 0.5);
const RANDOMISE: (f32, f32, f32) = (0.0, 100.0, 10.0);
const PAIRING: (f32, f32, f32) = (0.0, 100.0, 10.0);
const DRIFT: (f32, f32, f32) = (0.0, 100.0, 10.0);
const DRIFT_SPEED: (f32, f32, f32) = (10.0, 400.0, 10.0);

#[derive(Clone)]
enum Line {
    Power,
    Effect,
    /// Sound effects on or off.
    SoundPower,
    /// Which sound effect.
    Sound,
    Shuffle,
    ByShot,
    Drift,
    DriftSpeed,
    Shortest,
    Longest,
    Fade,
    /// Whether fades go through the plain picture.
    FadeStyle,
    Randomise,
    /// How often shuffle pairs a sound effect with a turn.
    SoundPairing,
    Setting(Box<SettingSpec>),
    /// One of the sound effect's settings.
    SoundSetting(Box<SettingSpec>),
    /// Opens the styles folder, for effects that paint like pictures (see `art`).
    StylesFolder,
    Defaults,
}

/// A value somewhere along a bar.
struct Bar {
    min: f32,
    max: f32,
    value: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    Close,
    /// For the player to deal with, like pausing.
    NotForMe,
}

/// The parts of the list of effects.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    All,
    Favourites,
    People,
    Faces,
    Depth,
    Audio,
}

const SECTIONS: [Section; 6] = [Section::All, Section::Favourites, Section::People, Section::Faces, Section::Depth, Section::Audio];

impl Section {
    fn name(self) -> &'static str {
        match self {
            Section::All => "All",
            Section::Favourites => "Favourites",
            Section::People => "People",
            Section::Faces => "Faces",
            Section::Depth => "Depth",
            Section::Audio => "Audio",
        }
    }

    fn holds(self, sh: &Shared, effect: &Effect) -> bool {
        let m = &effect.manifest;
        match self {
            Section::All => true,
            Section::Favourites => sh.effects.is_favourite(&effect.id),
            Section::People => m.segment || m.plate,
            Section::Faces => m.faces || m.mesh,
            Section::Depth => m.depth,
            Section::Audio => m.audio,
        }
    }
}

/// The list of effects. What it's resting on is shown as you go, until something's kept.
struct Picker {
    section: usize,
    selected: usize,
    first: usize,
    /// How many rows fitted last time, for paging.
    visible: usize,
    /// What to go back to: the effect, and whether effects were on.
    before: (Option<String>, bool),
    shown: Option<String>,
    /// The effect it last came to, and when.
    resting: (Option<String>, f64),
}

impl Picker {
    fn new(sh: &Shared) -> Self {
        let current = sh.effects.remembered().map(|e| e.id.clone());
        let selected = sh.effects.list().iter().position(|e| Some(&e.id) == current.as_ref()).unwrap_or(0);
        Self { section: 0, selected, first: 0, visible: 1, before: (current.clone(), sh.effects.is_on()), shown: current, resting: (None, 0.0) }
    }

    fn ids(&self, sh: &Shared) -> Vec<String> {
        sh.effects.list().iter().filter(|e| SECTIONS[self.section].holds(sh, e)).map(|e| e.id.clone()).collect()
    }

    fn show(&mut self, sh: &mut Shared, id: &str) {
        let (effects, db) = sh.effects_mut();
        effects.choose(Some(id), db);
        self.shown = Some(id.to_owned());
    }

    fn keep(&mut self, sh: &mut Shared, id: &str) {
        if self.shown.as_deref() != Some(id) || !sh.effects.is_on() {
            self.show(sh, id);
        }
    }

    /// Back to how it was when the list opened.
    fn cancel(&self, sh: &mut Shared) {
        let (effects, db) = sh.effects_mut();
        if self.shown != self.before.0 {
            effects.choose(self.before.0.as_deref(), db);
        }
        if effects.is_on() != self.before.1 {
            effects.set_on(self.before.1, db);
        }
    }

    /// The other half of Hold OK: with shuffle on, in or out of it, otherwise a favourite.
    fn mark(&self, sh: &mut Shared, id: &str) {
        let shuffling = sh.effects.shuffle().on;
        let (effects, db) = sh.effects_mut();
        if shuffling {
            effects.set_in_shuffle(id, !effects.in_shuffle(id), db);
        } else {
            effects.set_favourite(id, !effects.is_favourite(id), db);
        }
    }

    fn change_section(&mut self, sh: &Shared, direction: i64) {
        let here = self.ids(sh).get(self.selected).cloned();
        self.section = (self.section as i64 + direction).rem_euclid(SECTIONS.len() as i64) as usize;
        self.selected = self.ids(sh).iter().position(|id| Some(id) == here.as_ref()).unwrap_or(0);
    }

    /// `true` once it's finished with.
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Option<bool> {
        let ids = self.ids(sh);
        let last = ids.len().saturating_sub(1);
        self.selected = self.selected.min(last);
        let here = ids.get(self.selected);
        match press.action {
            Action::Back if !press.repeat => {
                self.cancel(sh);
                return Some(true);
            }
            Action::Ok if !press.repeat => {
                if let Some(id) = here {
                    self.keep(sh, id);
                    return Some(true);
                }
            }
            Action::Menu if !press.repeat => {
                if let Some(id) = here {
                    self.mark(sh, id);
                }
            }
            // Round to the other end, but holding a key down stops there.
            Action::Up if self.selected == 0 && !press.repeat => self.selected = last,
            Action::Down if self.selected == last && !press.repeat => self.selected = 0,
            Action::Up => self.selected = self.selected.saturating_sub(1),
            Action::Down => self.selected = (self.selected + 1).min(last),
            Action::PageUp => self.selected = self.selected.saturating_sub(self.visible),
            Action::PageDown => self.selected = (self.selected + self.visible).min(last),
            Action::Left if !press.repeat => self.change_section(sh, -1),
            Action::Right if !press.repeat => self.change_section(sh, 1),
            Action::Back | Action::Ok | Action::Menu | Action::Left | Action::Right => {}
            _ => return None,
        }
        Some(false)
    }

    /// `true` once it's finished with.
    fn ui(&mut self, ui: &egui::Ui, sh: &mut Shared, inner: Rect) -> bool {
        let ids = self.ids(sh);
        self.selected = self.selected.min(ids.len().saturating_sub(1));
        let painter = ui.painter();
        let (body, small) = (TextStyle::Body.resolve(ui.style()), TextStyle::Small.resolve(ui.style()));
        let text = theme::overlay_text();

        // Which section.
        let top = inner.top() + HEADER;
        let tabs = Rect::from_min_size(pos2(inner.left(), top), vec2(inner.width(), LIST_ROW_HEIGHT));
        let title = format!("{} ({})", SECTIONS[self.section].name(), ids.len());
        painter.text(tabs.center(), Align2::CENTER_CENTER, title, body.clone(), text);
        paint_arrow(painter, pos2(tabs.left() + 28.0, tabs.center().y), -1.0, text);
        paint_arrow(painter, pos2(tabs.right() - 28.0, tabs.center().y), 1.0, text);
        let response = ui.interact(tabs, egui::Id::new("flex-effect-sections"), egui::Sense::click());
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            self.change_section(sh, if p.x < tabs.center().x { -1 } else { 1 });
            return false;
        }

        let list_top = tabs.bottom() + ROW_GAP * 2.0;
        self.visible = (((inner.bottom() - FOOTER - list_top + ROW_GAP) / (LIST_ROW_HEIGHT + ROW_GAP)).floor() as usize).max(1);
        let moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
        let wheel = ui.input(|i| i.events.iter().map(|e| if let egui::Event::MouseWheel { delta, .. } = e { delta.y } else { 0.0 }).sum::<f32>());
        if wheel != 0.0 && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| inner.contains(p)) {
            self.selected = if wheel > 0.0 { self.selected.saturating_sub(1) } else { (self.selected + 1).min(ids.len().saturating_sub(1)) };
        }
        self.first = self.first.min(self.selected).max((self.selected + 1).saturating_sub(self.visible)).min(ids.len().saturating_sub(self.visible));

        if ids.is_empty() {
            let hint = "No favourites yet.\nWith shuffle off, hold OK on an effect to add it.";
            let galley = painter.layout(hint.to_owned(), small.clone(), text, inner.width() - 48.0);
            painter.galley(pos2(inner.left() + 24.0, list_top + 24.0), galley, text);
        }

        let shuffling = sh.effects.shuffle().on;
        let mut kept = None;
        for (n, id) in ids.iter().enumerate().skip(self.first).take(self.visible) {
            let y = list_top + (n - self.first) as f32 * (LIST_ROW_HEIGHT + ROW_GAP);
            let row = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), LIST_ROW_HEIGHT));
            let response = ui.interact(row, egui::Id::new(("flex-effect-list", id)), egui::Sense::click());
            if response.hovered() && moved {
                self.selected = n;
            }
            if response.clicked() {
                self.selected = n;
                kept = Some(id.clone());
            }
            let selected = n == self.selected;
            let colour = widgets::paint_focusable(painter, row, selected, selected);
            let mark = if selected { colour } else { theme::accent() };
            if sh.effects.is_favourite(id) {
                paint_star(painter, pos2(row.left() + 26.0, row.center().y), 11.0, mark);
            }
            let name = sh.effects.list().iter().find(|e| e.id == *id).map_or(id.as_str(), |e| e.manifest.name.as_str());
            let label = Rect::from_min_max(row.min, pos2(row.right() - 48.0, row.bottom()));
            painter.with_clip_rect(label).text(row.left_center() + vec2(50.0, 0.0), Align2::LEFT_CENTER, name, body.clone(), colour);
            if shuffling && sh.effects.in_shuffle(id) {
                let c = pos2(row.right() - 28.0, row.center().y);
                let stroke = egui::Stroke::new(3.5, mark);
                painter.line_segment([c + vec2(-9.0, 0.0), c + vec2(-3.0, 7.0)], stroke);
                painter.line_segment([c + vec2(-3.0, 7.0), c + vec2(9.0, -7.0)], stroke);
            }
        }
        if self.first > 0 {
            paint_more(painter, pos2(inner.center().x, list_top - 4.0), true, text);
        }
        if self.first + self.visible < ids.len() {
            paint_more(painter, pos2(inner.center().x, list_top + self.visible as f32 * (LIST_ROW_HEIGHT + ROW_GAP) + 2.0), false, text);
        }

        let here = ids.get(self.selected).cloned();
        let about = here.as_ref().and_then(|id| sh.effects.list().iter().find(|e| e.id == *id)).map_or(String::new(), |e| match &sh.effects.status {
            Some((false, error)) if self.shown.as_ref() == Some(&e.id) => format!("⚠ {error}"),
            _ => e.manifest.description.clone(),
        });
        paint_help(painter, inner, about, small, text);

        if let Some(id) = kept {
            self.keep(sh, &id);
            return true;
        }
        // Show it once it's been here a moment.
        let now = ui.input(|i| i.time);
        if self.resting.0 != here {
            self.resting = (here, now);
        }
        if let Some(id) = self.resting.0.clone().filter(|id| self.shown.as_ref() != Some(id)) {
            let left = REST - (now - self.resting.1);
            if left <= 0.0 {
                self.show(sh, &id);
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(left));
            }
        }
        false
    }
}

#[derive(Default)]
enum Page {
    #[default]
    Main,
    Shuffle,
    Drift,
    Picker(Picker),
}

#[derive(Default)]
pub struct EffectPanel {
    page: Page,
    selected: usize,
    first: usize,
    /// Where the main page had got to, while on another.
    main_at: (usize, usize),
    rect: Option<Rect>,
    /// The effect with changes not saved yet, from holding a key down.
    unsaved: Option<String>,
}

fn lines(page: &Page, sh: &Shared) -> Vec<Line> {
    match page {
        Page::Shuffle => {
            let mut lines = vec![Line::Shuffle];
            if sh.effects.shuffle().at_cuts {
                lines.push(Line::ByShot);
            }
            lines.extend([Line::Shortest, Line::Longest, Line::Fade, Line::FadeStyle, Line::Randomise, Line::SoundPairing]);
            lines
        }
        Page::Drift => vec![Line::Drift, Line::DriftSpeed],
        Page::Main | Page::Picker(_) => {
            let mut lines = vec![Line::Power, Line::Effect, Line::SoundPower, Line::Sound];
            if sh.effects.is_on() {
                lines.extend([Line::Shuffle, Line::Drift]);
            }
            if let Some(effect) = sh.effects.chosen() {
                lines.extend(effect.settings().cloned().map(Box::new).map(Line::Setting));
                if effect.takes_art() {
                    lines.push(Line::StylesFolder);
                }
                if effect.settings().next().is_some() {
                    lines.push(Line::Defaults);
                }
            }
            if let Some(sound) = shown_sound(sh).filter(|_| sh.effects.sound_is_on()) {
                lines.extend(sound.settings.iter().cloned().map(Box::new).map(Line::SoundSetting));
            }
            lines
        }
    }
}

/// The sound effect whose settings show: the one playing, or else the one picked.
fn shown_sound(sh: &Shared) -> Option<&SoundEffect> {
    sh.effects.sound_playing().or_else(|| sh.effects.chosen_sound())
}

/// Which way shuffle is: 0 off, 1 at random moments, 2 at scene cuts.
fn shuffle_mode(shuffle: Shuffle) -> i64 {
    match (shuffle.on, shuffle.at_cuts) {
        (false, _) => 0,
        (true, false) => 1,
        (true, true) => 2,
    }
}

fn paint_arrow(painter: &egui::Painter, at: Pos2, direction: f32, colour: egui::Color32) {
    let points = vec![at + vec2(-5.0 * direction, -11.0), at + vec2(6.0 * direction, 0.0), at + vec2(-5.0 * direction, 11.0)];
    painter.add(egui::Shape::line(points, egui::Stroke::new(3.0, colour)));
}

/// More above or below.
fn paint_more(painter: &egui::Painter, at: Pos2, up: bool, colour: egui::Color32) {
    let d = if up { -1.0 } else { 1.0 };
    let points = vec![at + vec2(-14.0, -5.0 * d), at + vec2(0.0, 7.0 * d), at + vec2(14.0, -5.0 * d)];
    painter.add(egui::Shape::line(points, egui::Stroke::new(3.0, colour)));
}

fn paint_star(painter: &egui::Painter, c: Pos2, radius: f32, colour: egui::Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(c, colour);
    for i in 0..10 {
        let angle = i as f32 * std::f32::consts::PI / 5.0 - std::f32::consts::FRAC_PI_2;
        let reach = if i % 2 == 0 { radius } else { radius * 0.45 };
        mesh.colored_vertex(c + vec2(angle.cos(), angle.sin()) * reach, colour);
    }
    for i in 0..10u32 {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % 10);
    }
    painter.add(mesh);
}

fn paint_help(painter: &egui::Painter, inner: Rect, help: String, font: FontId, colour: egui::Color32) {
    let galley = painter.layout(help, font, colour, inner.width() - 16.0);
    let area = Rect::from_min_max(pos2(inner.left() + 8.0, inner.bottom() - FOOTER + 16.0), inner.right_bottom());
    painter.with_clip_rect(area).galley(area.min, galley, colour);
}

fn seconds(value: f32) -> String {
    if value >= 90.0 {
        format!("{}:{:02}", (value / 60.0) as u32, value as u32 % 60)
    } else {
        format!("{value} s")
    }
}

/// `value` moved `steps` along, on a whole number of steps from `min` and within range.
fn along(value: f64, steps: f64, min: f64, max: f64, step: f64) -> f64 {
    let at = ((value - min) / step).round() + steps;
    // Rounded so 0.1 steps don't show as 0.30000000000000004.
    (((min + at * step) * 1e4).round() / 1e4).clamp(min, max)
}

fn number(value: f64) -> Value {
    if value.fract() == 0.0 { json!(value as i64) } else { json!(value) }
}

fn step_of(spec: &SettingSpec, min: f64, max: f64) -> f64 {
    spec.step.filter(|s| *s > 0.0).unwrap_or((max - min) / 20.0).max(1e-4)
}

/// The setting moved one place, wrapping round for lists.
fn changed(spec: &SettingSpec, current: Option<&Value>, direction: f64) -> Option<Value> {
    match spec.kind {
        SettingKind::Bool => Some(json!(!current.and_then(Value::as_bool).unwrap_or(false))),
        SettingKind::Choice => {
            let count = spec.options.len() as i64;
            if count == 0 {
                return None;
            }
            let at = current.and_then(Value::as_str).and_then(|c| spec.options.iter().position(|o| o.id == c)).unwrap_or(0) as i64;
            Some(json!(spec.options[(at + direction as i64).rem_euclid(count) as usize].id))
        }
        SettingKind::Number => {
            let (min, max) = (spec.min.unwrap_or(0.0), spec.max.unwrap_or(100.0));
            Some(number(along(current.and_then(Value::as_f64).unwrap_or(min), direction, min, max, step_of(spec, min, max))))
        }
        SettingKind::Text | SettingKind::Button => None,
    }
}

impl EffectPanel {
    pub fn contains(&self, pos: Pos2) -> bool {
        self.rect.is_some_and(|r| r.contains(pos))
    }

    /// The list of effects is open, and what's on shouldn't change under it.
    pub fn picking(&self) -> bool {
        matches!(self.page, Page::Picker(_))
    }

    /// Saves anything left from holding a key down.
    pub fn save(&mut self, sh: &mut Shared) {
        if let Some(id) = self.unsaved.take() {
            let (effects, db) = sh.effects_mut();
            effects.save(&id, db);
        }
    }

    /// The video's ending: nothing still being looked through was kept.
    pub fn leave(&mut self, sh: &mut Shared) {
        if let Page::Picker(picker) = &self.page {
            picker.cancel(sh);
        }
        self.save(sh);
    }

    fn open(&mut self, page: Page) {
        if matches!(self.page, Page::Main) {
            self.main_at = (self.selected, self.first);
        }
        self.page = page;
        (self.selected, self.first) = (0, 0);
    }

    fn back_to_main(&mut self) {
        self.page = Page::Main;
        (self.selected, self.first) = self.main_at;
    }

    pub fn handle(&mut self, sh: &mut Shared, press: Press) -> Outcome {
        if let Page::Picker(picker) = &mut self.page {
            return match picker.handle(sh, press) {
                None => Outcome::NotForMe,
                Some(false) => Outcome::Stay,
                Some(true) => {
                    self.back_to_main();
                    Outcome::Stay
                }
            };
        }
        let lines = lines(&self.page, sh);
        let last = lines.len().saturating_sub(1);
        self.selected = self.selected.min(last);
        if !press.repeat {
            self.save(sh);
        }
        match press.action {
            Action::Back | Action::Menu if !press.repeat => {
                if matches!(self.page, Page::Main) {
                    return Outcome::Close;
                }
                self.back_to_main();
            }
            Action::Up => self.selected = self.selected.saturating_sub(1),
            Action::Down => self.selected = (self.selected + 1).min(last),
            Action::Left | Action::Right => {
                // Holding a key down shouldn't race through whole shaders.
                if !press.repeat || !matches!(lines[self.selected], Line::Power | Line::Effect | Line::Shuffle | Line::Defaults) {
                    self.change(sh, &lines[self.selected], if press.action == Action::Left { -1.0 } else { 1.0 }, press.repeat);
                }
            }
            Action::Ok if !press.repeat => self.activate(sh, &lines[self.selected]),
            Action::Ok => {}
            _ => return Outcome::NotForMe,
        }
        Outcome::Stay
    }

    /// Whether OK on this row opens another page.
    fn opens(&self, line: &Line) -> bool {
        matches!(self.page, Page::Main) && matches!(line, Line::Effect | Line::Shuffle | Line::Drift)
    }

    fn activate(&mut self, sh: &mut Shared, line: &Line) {
        if self.opens(line) {
            self.open(match line {
                Line::Effect => Page::Picker(Picker::new(sh)),
                Line::Shuffle => Page::Shuffle,
                _ => Page::Drift,
            });
            return;
        }
        match line {
            Line::Defaults => self.defaults(sh),
            Line::StylesFolder => {
                let dir = crate::art::styles_dir();
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    log::warn!("making {}: {e}", dir.display());
                }
                crate::logging::open_folder(&dir);
            }
            // There's no "next" for a value along a bar.
            Line::Drift | Line::DriftSpeed | Line::Shortest | Line::Longest | Line::Fade | Line::Randomise | Line::SoundPairing => {}
            Line::Setting(spec) | Line::SoundSetting(spec) if spec.kind == SettingKind::Number => {}
            _ => self.change(sh, line, 1.0, false),
        }
    }

    fn defaults(&mut self, sh: &mut Shared) {
        let Some(effect) = sh.effects.chosen().cloned() else { return };
        let (effects, db) = sh.effects_mut();
        for spec in effect.settings() {
            if let Some(default) = &spec.default {
                effects.set_value(&effect.id, &spec.key, default.clone());
            }
        }
        effects.save(&effect.id, db);
    }

    fn change(&mut self, sh: &mut Shared, line: &Line, direction: f32, held: bool) {
        let (shuffle, drift) = (sh.effects.shuffle(), sh.effects.drift());
        let timed = |value: f32, (min, max, step): (f32, f32, f32)| along(value.into(), direction.into(), min.into(), max.into(), step.into()) as f32;
        match line {
            Line::Power => {
                let on = sh.effects.is_on();
                let (effects, db) = sh.effects_mut();
                effects.set_on(!on, db);
            }
            Line::Effect => {
                // Each effect in turn, either way round.
                let ids: Vec<String> = sh.effects.list().iter().map(|e| e.id.clone()).collect();
                if ids.is_empty() {
                    return;
                }
                let current = sh.effects.remembered().map(|e| e.id.clone());
                let at = ids.iter().position(|id| Some(id) == current.as_ref()).map_or(if direction > 0.0 { -1 } else { 0 }, |at| at as i64);
                let next = ids[(at + direction as i64).rem_euclid(ids.len() as i64) as usize].clone();
                let (effects, db) = sh.effects_mut();
                effects.choose(Some(&next), db);
            }
            Line::Drift => set_drift(sh, Drift { amount: timed(drift.amount, DRIFT), ..drift }),
            Line::DriftSpeed => set_drift(sh, Drift { speed: timed(drift.speed, DRIFT_SPEED), ..drift }),
            Line::Shuffle => {
                let mode = (shuffle_mode(shuffle) + direction as i64).rem_euclid(3);
                set_shuffle(sh, Shuffle { on: mode > 0, at_cuts: mode == 2, ..shuffle });
            }
            Line::ByShot => set_shuffle(sh, Shuffle { by_shot: !shuffle.by_shot, ..shuffle }),
            Line::FadeStyle => set_shuffle(sh, Shuffle { through_normal: !shuffle.through_normal, ..shuffle }),
            Line::Shortest => {
                let shortest = timed(shuffle.shortest, TURN);
                set_shuffle(sh, Shuffle { shortest, longest: shuffle.longest.max(shortest), ..shuffle });
            }
            Line::Longest => {
                let longest = timed(shuffle.longest, TURN);
                set_shuffle(sh, Shuffle { longest, shortest: shuffle.shortest.min(longest), ..shuffle });
            }
            Line::Fade => set_shuffle(sh, Shuffle { fade: timed(shuffle.fade, FADE), ..shuffle }),
            Line::Randomise => set_shuffle(sh, Shuffle { randomise: timed(shuffle.randomise, RANDOMISE), ..shuffle }),
            Line::SoundPairing => set_shuffle(sh, Shuffle { sounds: timed(shuffle.sounds, PAIRING), ..shuffle }),
            Line::SoundPower => {
                let on = sh.effects.sound_is_on();
                let (effects, db) = sh.effects_mut();
                effects.set_sound_on(!on, db);
            }
            Line::Sound => {
                // Each sound effect in turn, either way round.
                let ids: Vec<String> = sh.effects.sound_effects().iter().map(|s| s.id.clone()).collect();
                if ids.is_empty() {
                    return;
                }
                let current = sh.effects.chosen_sound().map(|s| s.id.clone());
                let at = ids.iter().position(|id| Some(id) == current.as_ref()).map_or(if direction > 0.0 { -1 } else { 0 }, |at| at as i64);
                let next = ids[(at + direction as i64).rem_euclid(ids.len() as i64) as usize].clone();
                let (effects, db) = sh.effects_mut();
                effects.choose_sound(Some(&next), db);
            }
            Line::Setting(spec) => {
                let Some(id) = sh.effects.chosen().map(|e| e.id.clone()) else { return };
                let Some(value) = changed(spec, sh.effects.value(&id, &spec.key), direction.into()) else { return };
                self.set(sh, &id, spec, value, held);
            }
            Line::SoundSetting(spec) => {
                let Some(id) = shown_sound(sh).map(|s| sound_settings_id(&s.id)) else { return };
                let Some(value) = changed(spec, sh.effects.value(&id, &spec.key), direction.into()) else { return };
                self.set(sh, &id, spec, value, held);
            }
            Line::StylesFolder | Line::Defaults => {}
        }
    }

    fn set(&mut self, sh: &mut Shared, id: &str, spec: &SettingSpec, value: Value, held: bool) {
        let (effects, db) = sh.effects_mut();
        if held {
            effects.set_value(id, &spec.key, value);
            self.unsaved = Some(id.to_owned());
        } else {
            effects.set_setting(id, &spec.key, value, db);
        }
    }

    /// The mouse put a bar at `fraction` of the way along.
    fn set_along(&mut self, sh: &mut Shared, line: &Line, fraction: f32, released: bool) {
        let (shuffle, drift) = (sh.effects.shuffle(), sh.effects.drift());
        let timed = |(min, max, step): (f32, f32, f32)| along((min + fraction * (max - min)).into(), 0.0, min.into(), max.into(), step.into()) as f32;
        match line {
            Line::Drift => set_drift(sh, Drift { amount: timed(DRIFT), ..drift }),
            Line::DriftSpeed => set_drift(sh, Drift { speed: timed(DRIFT_SPEED), ..drift }),
            Line::Shortest => {
                let shortest = timed(TURN);
                set_shuffle(sh, Shuffle { shortest, longest: shuffle.longest.max(shortest), ..shuffle });
            }
            Line::Longest => {
                let longest = timed(TURN);
                set_shuffle(sh, Shuffle { longest, shortest: shuffle.shortest.min(longest), ..shuffle });
            }
            Line::Fade => set_shuffle(sh, Shuffle { fade: timed(FADE), ..shuffle }),
            Line::Randomise => set_shuffle(sh, Shuffle { randomise: timed(RANDOMISE), ..shuffle }),
            Line::SoundPairing => set_shuffle(sh, Shuffle { sounds: timed(PAIRING), ..shuffle }),
            Line::Setting(spec) | Line::SoundSetting(spec) => {
                let id = if matches!(line, Line::Setting(_)) { sh.effects.chosen().map(|e| e.id.clone()) } else { shown_sound(sh).map(|s| sound_settings_id(&s.id)) };
                let Some(id) = id else { return };
                let (min, max) = (spec.min.unwrap_or(0.0), spec.max.unwrap_or(100.0));
                let value = number(along(min + f64::from(fraction) * (max - min), 0.0, min, max, step_of(spec, min, max)));
                self.set(sh, &id, spec, value, !released);
            }
            _ => {}
        }
    }

    /// What a row says: its name, its value, and a bar if it has one.
    fn describe(sh: &Shared, line: &Line) -> (String, String, Option<Bar>) {
        let (shuffle, drift) = (sh.effects.shuffle(), sh.effects.drift());
        let timed = |value: f32, (min, max, _): (f32, f32, f32)| Some(Bar { min, max, value });
        let chosen = sh.effects.chosen();
        match line {
            Line::Power => ("Video effects".into(), (if sh.effects.is_on() { "On" } else { "Off" }).into(), None),
            Line::SoundPower => ("Sound effects".into(), (if sh.effects.sound_is_on() { "On" } else { "Off" }).into(), None),
            Line::Effect => ("Effect".into(), sh.effects.remembered().map_or("None".into(), |e| e.manifest.name.clone()), None),
            Line::Drift => ("Drift".into(), if drift.amount > 0.0 { format!("{}%", drift.amount) } else { "Off".into() }, timed(drift.amount, DRIFT)),
            Line::DriftSpeed => ("Drift speed".into(), format!("{}%", drift.speed), timed(drift.speed, DRIFT_SPEED)),
            Line::Shuffle => ("Shuffle".into(), ["Off", "Timed", "Scene cuts"][shuffle_mode(shuffle) as usize].into(), None),
            Line::ByShot => ("Suit the shot".into(), (if shuffle.by_shot { "On" } else { "Off" }).into(), None),
            Line::FadeStyle => ("Fade style".into(), (if shuffle.through_normal { "Through the picture" } else { "Crossfade" }).into(), None),
            Line::Shortest => ("Shortest turn".into(), seconds(shuffle.shortest), timed(shuffle.shortest, TURN)),
            Line::Longest => ("Longest turn".into(), seconds(shuffle.longest), timed(shuffle.longest, TURN)),
            Line::Fade => ("Fade".into(), if shuffle.fade > 0.0 { seconds(shuffle.fade) } else { "Off".into() }, timed(shuffle.fade, FADE)),
            Line::Randomise => ("Random settings".into(), if shuffle.randomise > 0.0 { format!("{}%", shuffle.randomise) } else { "Off".into() }, timed(shuffle.randomise, RANDOMISE)),
            Line::SoundPairing => ("Sound effects".into(), if shuffle.sounds > 0.0 { format!("{}% of turns", shuffle.sounds) } else { "Off".into() }, timed(shuffle.sounds, PAIRING)),
            Line::Sound => {
                let picked = sh.effects.chosen_sound().map_or("None".to_owned(), |s| s.name.clone());
                let value = if sh.effects.pairs_sounds() { format!("Shuffled: {}", sh.effects.sound_playing().map_or("none now", |s| s.name.as_str())) } else { picked };
                ("Sound".into(), value, None)
            }
            Line::SoundSetting(spec) => {
                let Some(sound) = shown_sound(sh) else { return (spec.label.clone(), String::new(), None) };
                let value = sh.effects.value(&sound_settings_id(&sound.id), &spec.key);
                let (_, shown, bar) = Self::describe_value(sh, spec, value, false);
                (format!("{} · {}", sound.name, spec.label), shown, bar)
            }
            Line::StylesFolder => ("Your own styles".into(), "Open the folder".into(), None),
            Line::Defaults => ("Back to the defaults".into(), String::new(), None),
            Line::Setting(spec) => Self::describe_value(sh, spec, chosen.and_then(|e| sh.effects.value(&e.id, &spec.key)), true),
        }
    }

    /// A setting's row, with its value. `drifts`: a video effect's numbers wander (see Drift),
    /// and the bar shows where they've got to.
    fn describe_value(sh: &Shared, spec: &SettingSpec, value: Option<&Value>, drifts: bool) -> (String, String, Option<Bar>) {
        let on_off = |on: bool| (if on { "on" } else { "off" }).to_owned();
        match spec.kind {
            SettingKind::Bool => (spec.label.clone(), on_off(value.and_then(Value::as_bool).unwrap_or(false)), None),
            SettingKind::Choice => {
                let id = value.and_then(Value::as_str).unwrap_or("");
                (spec.label.clone(), spec.options.iter().find(|o| o.id == id).map_or(id.to_owned(), |o| o.label.clone()), None)
            }
            _ => {
                let (min, max) = (spec.min.unwrap_or(0.0), spec.max.unwrap_or(100.0));
                let n = value.and_then(Value::as_f64).unwrap_or(min);
                let live = if drifts { sh.effects.live_number(spec).unwrap_or(n) } else { n };
                (spec.label.clone(), format!("{n}{}", spec.unit), Some(Bar { min: min as f32, max: max as f32, value: live as f32 }))
            }
        }
    }

    /// The help for the selected row.
    fn help(sh: &Shared, line: &Line) -> String {
        match line {
            Line::Power => "Turns every video effect off or back on. What you've set up here is kept.".into(),
            Line::SoundPower if crate::sound_effects::unavailable() => {
                "⚠ Sound effects need Flex's own libmpv, and this one isn't it, so the sound stays as it is.".into()
            }
            Line::SoundPower => "Turns sound effects off or on, separately from the video effects.".into(),
            Line::Effect => {
                match (&sh.effects.status, sh.effects.remembered()) {
                    (Some((false, error)), _) => format!("⚠ {error}"),
                    (_, Some(e)) => e.manifest.description.clone(),
                    (_, None) => "No effect".into(),
                }
            }
            Line::Drift => "Every effect's settings wander slowly on their own: this is how far from where they're set.".into(),
            Line::DriftSpeed => "How quickly they wander.".into(),
            Line::Shuffle => match shuffle_mode(sh.effects.shuffle()) {
                0 => "Effects take turns at random, fading from one to the next.".into(),
                1 => "Effects take turns, changing at random moments.".into(),
                _ => "Effects take turns, changing on the frame the shot changes.".into(),
            },
            Line::ByShot => "Mostly picks effects that suit each new shot: faces on close-ups, landscapes on wide shots. Off, any effect is as likely as any other.".into(),
            Line::Shortest | Line::Longest if sh.effects.shuffle().at_cuts => "Cuts are ignored until the shortest turn is up. After the longest it changes anyway.".into(),
            Line::Shortest | Line::Longest => "Each effect stays for a random time between these.".into(),
            Line::Fade => "How long one effect takes to fade into the next. Off switches instantly.".into(),
            Line::FadeStyle => "Crossfade: one effect dissolves into the next. Through the picture: each effect eases itself away to the plain picture in its own way (heads grow back, colours come back), then the next eases in.".into(),
            Line::Randomise => "Each time an effect comes up, its settings move this far at random from yours, which aren't changed.".into(),
            Line::SoundPairing => "How often an effect comes with a sound effect, picked at random. It changes and fades with the picture. Off: the sound effect picked on the Sound row plays throughout.".into(),
            Line::Sound if crate::sound_effects::unavailable() => {
                "⚠ Sound effects need Flex's own libmpv, and this one isn't it, so the sound stays as it is.".into()
            }
            Line::Sound => match (sh.effects.pairs_sounds(), sh.effects.chosen_sound()) {
                (true, _) => "Shuffle is picking the sound effects (Shuffle → Sound effects). The one picked here plays when that's off.".into(),
                (false, Some(sound)) => sound.description.clone(),
                (false, None) => "Sound effects change the film's sound, on their own or alongside a video effect.".into(),
            },
            Line::StylesFolder => {
                let note = crate::art::note();
                let note = if note.is_empty() { String::new() } else { format!(" ({note})") };
                format!(
                    "Any picture you put in this folder becomes a style to pick above, named after its file{note}. Bold, textured pictures make the strongest: paintings, prints, mosaics, fabric. {}",
                    crate::art::styles_dir().display()
                )
            }
            Line::Defaults => String::new(),
            Line::Setting(spec) | Line::SoundSetting(spec) => spec.detail.clone(),
        }
    }

    /// `safe` is the part of the screen it's safe to draw on.
    pub fn ui(&mut self, ui: &egui::Ui, sh: &mut Shared, safe: Rect) {
        let panel = Rect::from_min_max(pos2(safe.right() - WIDTH, safe.top()), safe.right_bottom());
        self.rect = Some(panel);
        let painter = ui.painter();
        theme::paint_overlay(painter, panel, 14.0);
        let inner = panel.shrink(PADDING);
        let (body, small) = (TextStyle::Body.resolve(ui.style()), TextStyle::Small.resolve(ui.style()));
        let text = theme::overlay_text();
        let title = match self.page {
            Page::Main => "Video effects",
            Page::Shuffle => "Shuffle",
            Page::Drift => "Drift",
            Page::Picker(_) => "Choose an effect",
        };
        let hint = match self.page {
            Page::Main => "Back to close",
            Page::Picker(_) if sh.effects.shuffle().on => "Hold OK: in the shuffle",
            Page::Picker(_) => "Hold OK: favourite",
            _ => "Back to return",
        };
        let on_main = matches!(self.page, Page::Main);
        let title_x = if on_main { 8.0 } else { 36.0 };
        painter.text(inner.left_top() + vec2(title_x, 4.0), Align2::LEFT_TOP, title, body.clone(), text);
        painter.text(inner.right_top() + vec2(-8.0, 10.0), Align2::RIGHT_TOP, hint, small.clone(), text.gamma_multiply(0.6));
        if !on_main {
            // The way back, for a mouse.
            paint_arrow(painter, inner.left_top() + vec2(16.0, 20.0), -1.0, text);
            let back = Rect::from_min_size(inner.left_top(), vec2(inner.width() * 0.5, HEADER - 8.0));
            if ui.interact(back, egui::Id::new("flex-effect-back"), egui::Sense::click()).clicked() {
                if let Page::Picker(picker) = &self.page {
                    picker.cancel(sh);
                }
                self.back_to_main();
                return;
            }
        }

        if let Page::Picker(picker) = &mut self.page {
            if picker.ui(ui, sh, inner) {
                self.back_to_main();
            }
            return;
        }

        let lines = lines(&self.page, sh);
        self.selected = self.selected.min(lines.len().saturating_sub(1));
        let visible = (((inner.height() - HEADER - FOOTER + ROW_GAP) / (ROW_HEIGHT + ROW_GAP)).floor() as usize).max(1);
        let moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
        let wheel = ui.input(|i| i.events.iter().map(|e| if let egui::Event::MouseWheel { delta, .. } = e { delta.y } else { 0.0 }).sum::<f32>());
        if wheel != 0.0 && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| panel.contains(p)) {
            self.selected = if wheel > 0.0 { self.selected.saturating_sub(1) } else { (self.selected + 1).min(lines.len().saturating_sub(1)) };
        }
        self.first = self.first.min(self.selected).max((self.selected + 1).saturating_sub(visible)).min(lines.len().saturating_sub(visible));

        let mut clicked = None;
        let mut dragged = None;
        for (n, line) in lines.iter().enumerate().skip(self.first).take(visible) {
            let top = inner.top() + HEADER + (n - self.first) as f32 * (ROW_HEIGHT + ROW_GAP);
            let row = Rect::from_min_size(pos2(inner.left(), top), vec2(inner.width(), ROW_HEIGHT));
            let (name, value, bar) = Self::describe(sh, line);
            let opens = self.opens(line);

            let response = ui.interact(row, egui::Id::new(("flex-effect-panel", n)), egui::Sense::click_and_drag());
            if response.hovered() && moved {
                self.selected = n;
            }
            if let Some(p) = response.interact_pointer_pos() {
                self.selected = n;
                if bar.is_some() && !(opens && p.x >= row.right() - ARROW) {
                    if response.is_pointer_button_down_on() || response.drag_stopped() {
                        dragged = Some((line.clone(), ((p.x - row.left()) / row.width()).clamp(0.0, 1.0), response.drag_stopped()));
                    }
                } else if response.clicked() {
                    clicked = Some(line.clone());
                }
            }

            let selected = n == self.selected;
            let colour = widgets::paint_focusable(painter, row, selected, selected);
            let value_galley = painter.layout_no_wrap(value, body.clone(), colour);
            let value_x = row.right() - if opens { ARROW } else { 20.0 } - value_galley.size().x;
            painter.galley(pos2(value_x, row.center().y - value_galley.size().y / 2.0), value_galley, colour);
            let label = Rect::from_min_max(row.min, pos2(value_x - 16.0, row.bottom()));
            painter.with_clip_rect(label).text(row.left_center() + vec2(20.0, 0.0), Align2::LEFT_CENTER, name, body.clone(), colour);
            if opens {
                paint_arrow(painter, pos2(row.right() - 26.0, row.center().y), 1.0, colour);
            }
            if let Some(bar) = bar {
                let span = (bar.max - bar.min).max(1e-4);
                widgets::paint_progress(painter, row, (bar.value - bar.min) / span, theme::progress_on_panel(selected));
            }
        }

        if self.first > 0 {
            paint_more(painter, pos2(inner.center().x, inner.top() + HEADER - 12.0), true, text);
        }
        let list_bottom = inner.top() + HEADER + visible as f32 * (ROW_HEIGHT + ROW_GAP);
        if self.first + visible < lines.len() {
            paint_more(painter, pos2(inner.center().x, list_bottom + 2.0), false, text);
        }

        if let Some(line) = lines.get(self.selected) {
            paint_help(painter, inner, Self::help(sh, line), small, text);
        }

        if let Some(line) = clicked {
            self.activate(sh, &line);
        }
        if let Some((line, fraction, released)) = dragged {
            self.set_along(sh, &line, fraction, released);
        }
    }
}

fn set_drift(sh: &mut Shared, drift: Drift) {
    let (effects, db) = sh.effects_mut();
    effects.set_drift(drift, db);
}

fn set_shuffle(sh: &mut Shared, shuffle: Shuffle) {
    let (effects, db) = sh.effects_mut();
    effects.set_shuffle(shuffle, db);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_stay_on_the_grid_and_in_range() {
        assert_eq!(along(0.2, 1.0, 0.0, 1.0, 0.1), 0.3);
        assert_eq!(along(0.95, 1.0, 0.0, 1.0, 0.1), 1.0);
        assert_eq!(along(0.0, -1.0, 0.0, 1.0, 0.1), 0.0);
        assert_eq!(along(0.37, 0.0, 0.0, 1.0, 0.25), 0.25);
    }
}
