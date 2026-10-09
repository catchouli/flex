use std::collections::VecDeque;
use std::path::PathBuf;

use eframe::egui::{self, Align2, CornerRadius, Key, Rect, TextStyle, ViewportCommand, pos2, vec2};

use crate::input::{self, InputLog};
use crate::player::Player;
use crate::screens::{Nav, Screen, home::Home, search::SearchView, settings::Settings};
use crate::shared::{PlayRequest, Shared};
use crate::theme;
use crate::topbar::{self, TopBar};

const CURSOR_HIDE_SECS: f64 = 2.0;
const TOAST_SECS: f64 = 4.0;
/// Holding Back this long quits; the prompt appears after `QUIT_PROMPT_SECS`.
const QUIT_HOLD_SECS: f64 = 1.5;
const QUIT_PROMPT_SECS: f64 = 0.4;
const FULLSCREEN_SETTING: &str = "fullscreen";

pub struct FlexApp {
    sh: Shared,
    /// Never empty: Home is always at the bottom.
    stack: Vec<Box<dyn Screen>>,
    top_bar: TopBar,
    last_query: String,
    input_log: InputLog,
    show_input_log: bool,
    back_hold_since: Option<f64>,
    toasts: VecDeque<(String, f64)>,
    last_pointer_move: f64,
    /// Restore saved fullscreen once the window exists (fullscreen at creation misbehaves
    /// on Windows: the window ends up borderless in a corner).
    restore_fullscreen: bool,
    frames: u64,
}

impl FlexApp {
    /// `start` may be a video to play immediately (handy for testing playback).
    pub fn new(cc: &eframe::CreationContext<'_>, start: Option<PathBuf>, windowed: bool) -> anyhow::Result<Self> {
        theme::install(&cc.egui_ctx);
        input::media::set_context(&cc.egui_ctx);
        let sh = Shared::new(&cc.egui_ctx, cc.get_proc_address.clone())?;
        let restore_fullscreen = !windowed && sh.setting_bool(FULLSCREEN_SETTING);
        let mut app = Self {
            restore_fullscreen,
            frames: 0,
            sh,
            stack: vec![Box::new(Home::default())],
            top_bar: TopBar::default(),
            last_query: String::new(),
            input_log: InputLog::default(),
            show_input_log: false,
            back_hold_since: None,
            toasts: VecDeque::new(),
            last_pointer_move: 0.0,
        };
        if let Some(path) = start.filter(|p| p.is_file() && flex_core::library::is_video(p)) {
            let title = flex_core::library::display_name(&path.file_name().unwrap_or_default().to_string_lossy(), true);
            let req = PlayRequest { path, title, library_id: -1, file_id: -1 };
            app.apply(&cc.egui_ctx, Nav::Play { req, replace: false });
        }
        Ok(app)
    }

    fn top(&mut self) -> &mut dyn Screen {
        self.stack.last_mut().expect("screen stack is never empty").as_mut()
    }

    fn apply(&mut self, ctx: &egui::Context, nav: Nav) {
        match nav {
            Nav::None => {}
            Nav::Push(screen) => {
                self.top_bar.focus = None;
                self.stack.push(screen);
            }
            Nav::Pop => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                }
            }
            Nav::Replace(screen) => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                }
                self.stack.push(screen);
            }
            Nav::Play { req, replace } => self.play(ctx, req, replace),
            Nav::FocusTopBar => self.top_bar.focus = Some(topbar::Item::Search),
        }
    }

    fn play(&mut self, ctx: &egui::Context, req: PlayRequest, replace: bool) {
        let Some(proc_address) = self.sh.proc_address.clone() else {
            self.sh.toast("Video needs the OpenGL renderer, which isn't available");
            return;
        };
        match Player::open(ctx, proc_address, req) {
            Ok(player) => {
                if replace && self.stack.len() > 1 {
                    self.stack.pop();
                }
                self.top_bar.focus = None;
                self.stack.push(Box::new(player));
            }
            Err(e) => {
                let hint = match crate::mpv::ffi::api() {
                    Ok(_) => "",
                    Err(_) if cfg!(windows) => "Put libmpv-2.dll next to flex.exe (see README). ",
                    Err(_) => "Install libmpv or set FLEX_LIBMPV to its path (see README). ",
                };
                log::error!("{e:#}");
                self.sh.toast(format!("{hint}{e:#}"));
            }
        }
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        let fullscreen = !ctx.input(|i| i.viewport().fullscreen).unwrap_or(false);
        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(fullscreen));
        self.sh.set_setting_bool(FULLSCREEN_SETTING, fullscreen);
    }

    fn open_settings(&mut self) {
        self.top_bar.focus = None;
        self.stack.push(Box::new(Settings::default()));
    }

    /// Keeps the search screen in step with the search box: typing shows results
    /// (returning to them if you'd navigated deeper), clearing the box closes them.
    fn sync_search(&mut self) {
        let query = self.sh.search_query.trim().to_owned();
        if query == self.last_query {
            return;
        }
        self.last_query = query.clone();
        let search_at = self.stack.iter().position(|s| s.is_search());
        match (query.is_empty(), search_at) {
            (true, Some(i)) => self.stack.truncate(i.max(1)),
            (false, Some(i)) => self.stack.truncate(i + 1),
            (false, None) => self.stack.push(Box::new(SearchView::default())),
            (true, None) => {}
        }
    }

    fn global_keys(&mut self, ctx: &egui::Context) {
        let (f1, f11) = ctx.input(|i| (i.key_pressed(Key::F1), i.key_pressed(Key::F11)));
        if f1 {
            self.show_input_log = !self.show_input_log;
        }
        if f11 {
            self.toggle_fullscreen(ctx);
        }
    }

    /// Double-clicking the window toggles fullscreen, except on the top bar
    /// (where double-click selects text in the search box).
    fn double_click_fullscreen(&mut self, ctx: &egui::Context, top_bar: Option<Rect>) {
        let (double, pos) =
            ctx.input(|i| (i.pointer.button_double_clicked(egui::PointerButton::Primary), i.pointer.interact_pos()));
        let on_top_bar = top_bar.zip(pos).is_some_and(|(bar, p)| bar.contains(p));
        if double && !on_top_bar {
            self.toggle_fullscreen(ctx);
        }
    }

    /// Quits when Back is held for `QUIT_HOLD_SECS`, showing a prompt while held.
    fn hold_to_quit(&mut self, ui: &egui::Ui, now: f64, text_focused: bool) {
        let ctx = ui.ctx();
        if text_focused || !input::back_held(ctx) {
            self.back_hold_since = None;
            return;
        }
        let held = now - *self.back_hold_since.get_or_insert(now);
        if held >= QUIT_HOLD_SECS {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        } else if held >= QUIT_PROMPT_SECS {
            let screen = ctx.content_rect();
            let rect = Rect::from_center_size(screen.center(), vec2(560.0, 150.0));
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(14), theme::OSD_BG);
            painter.text(rect.center() - vec2(0.0, 22.0), Align2::CENTER_CENTER, "Keep holding to quit", TextStyle::Body.resolve(ui.style()), theme::TEXT);
            let bar = Rect::from_min_size(pos2(rect.left() + 40.0, rect.bottom() - 46.0), vec2(rect.width() - 80.0, 10.0));
            painter.rect_filled(bar, CornerRadius::same(5), egui::Color32::from_white_alpha(50));
            let t = ((held - QUIT_PROMPT_SECS) / (QUIT_HOLD_SECS - QUIT_PROMPT_SECS)) as f32;
            painter.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * t, bar.height())), CornerRadius::same(5), theme::FOCUS);
        }
        ctx.request_repaint();
    }

    fn draw_toasts(&mut self, ui: &egui::Ui, now: f64) {
        for msg in self.sh.take_toasts() {
            log::info!("{msg}");
            // A repeat of a message already showing just refreshes it instead of stacking.
            self.toasts.retain(|(m, _)| *m != msg);
            self.toasts.push_back((msg, now + TOAST_SECS));
        }
        self.toasts.retain(|(_, until)| *until > now);
        let Some(next_expiry) = self.toasts.iter().map(|(_, t)| *t).reduce(f64::min) else { return };
        let safe = theme::safe_rect(ui.ctx());
        let font = TextStyle::Body.resolve(ui.style());
        let mut bottom = safe.bottom();
        for (msg, _) in self.toasts.iter().rev() {
            let galley = ui.painter().layout(msg.clone(), font.clone(), theme::TEXT, safe.width() * 0.7);
            let rect = Rect::from_center_size(
                pos2(safe.center().x, bottom - galley.size().y / 2.0 - 16.0),
                galley.size() + vec2(48.0, 32.0),
            );
            ui.painter().rect_filled(rect, CornerRadius::same(12), theme::OSD_BG);
            ui.painter().galley(rect.center() - galley.size() / 2.0, galley, theme::TEXT);
            bottom = rect.top() - 12.0;
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(next_expiry - now));
    }

    fn update_cursor(&mut self, ctx: &egui::Context, now: f64, playing: bool) {
        if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
            self.last_pointer_move = now;
        }
        let idle = now - self.last_pointer_move;
        if playing || idle > CURSOR_HIDE_SECS {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(CURSOR_HIDE_SECS - idle + 0.05));
        }
    }

    fn input_log_overlay(&self, ui: &egui::Ui) {
        let safe = theme::safe_rect(ui.ctx());
        let rect = Rect::from_min_size(pos2(safe.right() - 760.0, safe.top() + 90.0), vec2(760.0, 560.0));
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(12), theme::OSD_BG);
        let mono = TextStyle::Monospace.resolve(ui.style());
        let mut y = rect.top() + 20.0;
        painter.text(pos2(rect.left() + 24.0, y), Align2::LEFT_TOP, "Input log (F1 to hide)", mono.clone(), theme::FOCUS);
        for entry in self.input_log.entries() {
            y += 40.0;
            painter.text(pos2(rect.left() + 24.0, y), Align2::LEFT_TOP, entry, mono.clone(), theme::TEXT);
        }
    }
}

impl eframe::App for FlexApp {
    /// Runs even while the window is hidden, so scans finish without anyone watching.
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sh.poll_scans();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        theme::update_scale(&ctx);
        self.frames += 1;
        if self.restore_fullscreen && self.frames > 2 {
            self.restore_fullscreen = false;
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(true));
        }
        self.global_keys(&ctx);

        let text_focused = topbar::text_focused(&ctx);
        let presses = input::collect(&ctx, &mut self.input_log, text_focused);
        if self.top().shows_top_bar() && !text_focused && topbar::typed_text(&ctx) {
            // Start searching as soon as someone types; the box picks up this frame's text.
            topbar::focus_text(&ctx);
        }
        for press in presses {
            let nav = if self.top_bar.focus.is_some() && self.top().shows_top_bar() {
                match self.top_bar.handle(&ctx, press) {
                    topbar::Event::OpenSettings => self.open_settings(),
                    topbar::Event::ToggleFullscreen => self.toggle_fullscreen(&ctx),
                    _ => {}
                }
                Nav::None
            } else {
                let sh = &mut self.sh;
                self.stack.last_mut().expect("never empty").handle(sh, press)
            };
            self.apply(&ctx, nav);
        }

        ui.painter().rect_filled(ctx.content_rect(), CornerRadius::ZERO, theme::BG);
        let safe = theme::safe_rect(&ctx);
        let bar = self.top().shows_top_bar().then(|| Rect::from_min_size(safe.min, vec2(safe.width(), topbar::HEIGHT)));
        self.double_click_fullscreen(&ctx, bar);
        let content = if let Some(bar) = bar {
            match self.top_bar.ui(ui, bar, &mut self.sh) {
                topbar::Event::Submit => {
                    self.sync_search();
                    let sh = &mut self.sh;
                    let nav = match self.stack.last_mut() {
                        Some(s) if s.is_search() => s.activate_first(sh),
                        _ => Nav::None,
                    };
                    self.apply(&ctx, nav);
                }
                topbar::Event::Cancel => self.sh.search_query.clear(),
                topbar::Event::OpenSettings => self.open_settings(),
                topbar::Event::ToggleFullscreen => self.toggle_fullscreen(&ctx),
                topbar::Event::FocusContent | topbar::Event::None => {}
            }
            self.sync_search();
            Rect::from_min_max(pos2(safe.left(), safe.top() + topbar::HEIGHT + 36.0), safe.max)
        } else {
            ctx.content_rect()
        };

        let focused = self.top_bar.focus.is_none() && !topbar::text_focused(&ctx);
        let sh = &mut self.sh;
        let nav = self.stack.last_mut().expect("never empty").ui(ui, sh, content, focused);
        self.apply(&ctx, nav);

        let playing = !self.top().shows_top_bar();
        self.update_cursor(&ctx, now, playing);
        self.draw_toasts(ui, now);
        self.hold_to_quit(ui, now, text_focused);
        if self.show_input_log {
            self.input_log_overlay(ui);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Free any mpv render context while the GL context is still alive.
        self.stack.clear();
    }
}

/// Shown instead of the app if it can't start (e.g. the database can't be opened).
pub struct StartupError(pub String);

impl eframe::App for StartupError {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::update_scale(ui.ctx());
        let safe = theme::safe_rect(ui.ctx());
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(safe));
        ui.heading("Flex couldn't start");
        ui.label(&self.0);
    }
}
