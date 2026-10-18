use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui::{self, Align2, CornerRadius, Key, Rect, TextStyle, ViewportCommand, pos2, vec2};

use crate::input::{self, Action, InputLog, Press};
use crate::instance::Command;
use crate::player::Player;
use crate::screens::{Nav, Screen, home::Home, resume::ResumePrompt, search::SearchView, settings::Settings};
use crate::shared::{PlayRequest, Shared};
use crate::widgets::{self, Hit};
use crate::theme;
use crate::topbar::{self, TopBar};
use crate::update::Progress;

const CURSOR_HIDE_SECS: f64 = 2.0;
const TOAST_SECS: f64 = 4.0;
/// Holding Back this long closes Flex (to the tray if there is one); the prompt appears
/// after `QUIT_PROMPT_SECS`.
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
    /// (text, shown until, key).
    toasts: VecDeque<(String, f64, Option<String>)>,
    last_pointer_move: f64,
    /// Restore saved fullscreen once the window exists (fullscreen at creation misbehaves
    /// on Windows: the window ends up borderless in a corner).
    restore_fullscreen: bool,
    frames: u64,
    /// Fullscreen state we last asked for. The window's own report can't be relied on:
    /// on macOS it lags while animating and sometimes stays stale afterwards.
    fullscreen: bool,
    /// From the tray icon and later launches of Flex.
    commands: Receiver<Command>,
    command_tx: Sender<Command>,
    /// Present while "Close to system tray" is on (and there is a tray): closing the
    /// window then hides Flex there instead of quitting.
    tray: Option<crate::tray::Tray>,
    /// Hide the window on the next frame (closed, or started with `--tray`).
    hide_pending: bool,
    /// The window is hidden in the tray.
    hidden: bool,
    /// Quit was chosen from the tray: let the window close for real.
    quitting: bool,
}

impl FlexApp {
    /// `start` may be a video to play immediately (handy for testing playback).
    /// `hidden` starts Flex in the tray (when it starts with Windows).
    pub fn new(cc: &eframe::CreationContext<'_>, start: Option<PathBuf>, windowed: bool, hidden: bool) -> anyhow::Result<Self> {
        input::os::set_context(&cc.egui_ctx);
        let mut sh = Shared::new(&cc.egui_ctx, cc.get_proc_address.clone())?;
        theme::install(&cc.egui_ctx, sh.setting(crate::screens::themes::THEME_SETTING).as_deref());
        sh.start_web_if_enabled();
        let (command_tx, commands) = channel();
        crate::instance::listen(command_tx.clone(), cc.egui_ctx.clone());
        let tray = if sh.close_to_tray { crate::tray::create(command_tx.clone(), &cc.egui_ctx) } else { None };
        #[cfg(windows)]
        crate::autostart::refresh();
        let restore_fullscreen = !windowed && sh.setting_bool(FULLSCREEN_SETTING);
        let mut app = Self {
            restore_fullscreen,
            frames: 0,
            fullscreen: false,
            sh,
            stack: vec![Box::new(Home::default())],
            top_bar: TopBar::default(),
            last_query: String::new(),
            input_log: InputLog::default(),
            show_input_log: false,
            back_hold_since: None,
            toasts: VecDeque::new(),
            last_pointer_move: 0.0,
            hide_pending: hidden && tray.is_some(),
            hidden: false,
            commands,
            command_tx,
            tray,
            quitting: false,
        };
        if let Some(path) = start.as_ref().filter(|p| p.is_file() && flex_core::library::is_video(p)) {
            let title = flex_core::library::display_name(&path.file_name().unwrap_or_default().to_string_lossy(), true);
            let req = PlayRequest { path: path.clone(), title, library_id: -1, file_id: -1, start: Some(0.0) };
            app.apply(&cc.egui_ctx, Nav::Play { req, replace: false });
        } else if let Some(id) = start.as_ref().and_then(|s| flex_core::youtube::video_id(&s.to_string_lossy())) {
            // `flex <YouTube URL>` plays it (with YouTube set up).
            app.apply(&cc.egui_ctx, Nav::Play { req: PlayRequest::youtube(&id, "YouTube", Some(0.0)), replace: false });
        }
        Ok(app)
    }

    fn top(&mut self) -> &mut dyn Screen {
        self.stack.last_mut().expect("screen stack is never empty").as_mut()
    }

    /// Pops the top screen (never Home), letting it clean up first.
    fn pop(&mut self) {
        if self.stack.len() > 1
            && let Some(mut screen) = self.stack.pop()
        {
            screen.on_leave(&mut self.sh);
        }
    }

    fn truncate(&mut self, len: usize) {
        while self.stack.len() > len.max(1) {
            self.pop();
        }
    }

    fn apply(&mut self, ctx: &egui::Context, nav: Nav) {
        match nav {
            Nav::None => {}
            Nav::Push(screen) => {
                self.top_bar.focus = None;
                self.stack.push(screen);
            }
            Nav::Pop => self.pop(),
            Nav::Replace(screen) => {
                self.pop();
                self.stack.push(screen);
            }
            Nav::Play { req, replace } => self.play(ctx, req, replace),
            Nav::PlayOver { screen, req } => {
                self.top_bar.focus = None;
                self.stack.push(screen);
                self.play(ctx, req, false);
            }
            Nav::FocusTopBar => self.top_bar.focus = Some(topbar::Item::Search),
        }
    }

    fn play(&mut self, ctx: &egui::Context, req: PlayRequest, replace: bool) {
        let saved = match req.youtube_id() {
            Some(id) => {
                if self.sh.youtube.tools.is_none() {
                    self.sh.toast("YouTube isn't set up yet: switch it on in Settings → YouTube");
                    return;
                }
                let id = id.to_owned();
                self.sh.youtube.forget(&id);
                self.sh.prepare_youtube(&id);
                self.sh.yt_progress(&id).and_then(|p| p.position)
            }
            None => self.sh.progress(req.library_id, req.file_id).and_then(|p| p.position),
        };
        // Ask before resuming, unless the caller already chose where to start.
        if req.start.is_none()
            && let Some(position) = saved
        {
            if replace {
                self.pop();
            }
            self.top_bar.focus = None;
            self.stack.push(Box::new(ResumePrompt::new(req, position)));
            return;
        }
        let Some(proc_address) = self.sh.proc_address.clone() else {
            self.sh.toast("Video needs the OpenGL renderer, which isn't available");
            return;
        };
        let picture = self.sh.picture_for(&req);
        let sub_key = self.sh.subtitle_key(&req);
        match Player::open(ctx, proc_address, req, picture, sub_key) {
            Ok(player) => {
                if replace {
                    self.pop();
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

    /// Runs a downloaded update once nothing is loaded in the player (playing or paused).
    fn install_update(&mut self) {
        if let Some(message) = self.sh.updater.take_message() {
            self.sh.toast(message);
        }
        let Some((version, installer)) = self.sh.updater.ready() else { return };
        // Automatic updates wait for playback to end (checked again when the screen
        // changes); "Update now" doesn't (progress is saved as Flex quits).
        if !self.sh.updater.asked() && self.stack.iter().any(|s| s.is_player()) {
            return;
        }
        if let Err(e) = self.sh.updater.install(version, &installer, self.hidden) {
            log::warn!("{e:#}");
            self.sh.toast(format!("Couldn't install the update: {e:#}"));
        }
    }

    /// The update's progress, shown along the bottom until it's done. Automatic updates
    /// keep quiet during videos.
    fn update_status(&self) -> Option<String> {
        let quiet = !self.sh.updater.asked() && self.stack.iter().any(|s| s.is_player());
        Some(match self.sh.updater.progress() {
            Progress::Checking => "Checking for updates…".to_owned(),
            Progress::Downloading { version, done, total } if !quiet => {
                format!("Found Flex {version}: downloading… {}%", done * 100 / total.max(1))
            }
            Progress::Ready { version, .. } if !quiet => format!("Flex {version} downloaded: installing…"),
            Progress::Installing { version } => format!("Installing Flex {version}… Flex will restart in a moment"),
            _ => return None,
        })
    }

    /// Stops playback (saving progress) and hides the window; Flex stays in the tray.
    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        while self.top().is_player() {
            self.pop();
        }
        self.back_hold_since = None;
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        self.hidden = true;
        log::info!("hidden in the tray");
    }

    /// Handles the tray icon and later launches, and turns closing the window into hiding
    /// it. Runs while the window is hidden too.
    fn show_window(&mut self, ctx: &egui::Context) {
        self.hide_pending = false;
        self.hidden = false;
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn handle_commands(&mut self, ctx: &egui::Context) {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::Show => self.show_window(ctx),
                Command::UpdateNow => {
                    // Shown, so the progress along the bottom can be seen.
                    self.show_window(ctx);
                    self.sh.updater.update_now(ctx);
                }
                Command::Quit => {
                    log::info!("quitting");
                    self.quitting = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                    // In case the hidden window never gets to process the close.
                    std::thread::spawn(|| {
                        std::thread::sleep(std::time::Duration::from_secs(5));
                        log::warn!("didn't close in time; exiting");
                        std::process::exit(0);
                    });
                }
            }
            ctx.request_repaint();
        }
        // Follow the setting: add or remove the tray icon.
        if self.sh.close_to_tray != self.tray.is_some() {
            self.tray = if self.sh.close_to_tray { crate::tray::create(self.command_tx.clone(), ctx) } else { None };
        }
        if self.tray.is_some() && !self.quitting && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.hide_pending = true;
            ctx.request_repaint();
        }
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.set_fullscreen(ctx, !self.fullscreen);
    }

    fn set_fullscreen(&mut self, ctx: &egui::Context, fullscreen: bool) {
        self.fullscreen = fullscreen;
        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(fullscreen));
        self.sh.set_setting_bool(FULLSCREEN_SETTING, fullscreen);
    }


    /// Carries out the phone remote's commands: presses go where the TV remote's would.
    fn handle_remote(&mut self, ctx: &egui::Context) {
        use flex_web::remote::{Key, Remote};
        for command in std::mem::take(&mut self.sh.remote) {
            match command {
                Remote::Key(key) => {
                    let action = match key {
                        Key::Up => Action::Up,
                        Key::Down => Action::Down,
                        Key::Left => Action::Left,
                        Key::Right => Action::Right,
                        Key::Ok => Action::Ok,
                        Key::Back => Action::Back,
                        Key::PlayPause => Action::PlayPause,
                        Key::Menu => Action::Menu,
                    };
                    let nav = self.route(ctx, Press { action, repeat: false });
                    self.apply(ctx, nav);
                }
                Remote::Play { library_id, file_id } => match self.sh.play_request(library_id, file_id) {
                    Some(req) => self.play_from_remote(ctx, req),
                    None => log::warn!("remote: no file {file_id} in library {library_id}"),
                },
                Remote::YouTube { id, title } => {
                    let req = PlayRequest::youtube(&id, &title, None);
                    self.play_from_remote(ctx, req);
                }
                Remote::SubtitlesOff => {
                    let sh = &mut self.sh;
                    self.stack.last_mut().expect("never empty").subtitles_off(sh);
                }
            }
            ctx.request_repaint();
        }
    }

    /// Plays on the TV, in place of whatever's playing there now.
    fn play_from_remote(&mut self, ctx: &egui::Context, req: PlayRequest) {
        self.show_window(ctx);
        let replace = self.top().is_player();
        self.play(ctx, req, replace);
    }

    /// Sends a press to the top bar if it has focus, otherwise to the current screen.
    fn route(&mut self, ctx: &egui::Context, press: Press) -> Nav {
        if self.top_bar.focus.is_some() && self.top().shows_top_bar() {
            match self.top_bar.handle(ctx, press) {
                topbar::Event::OpenSettings => self.open_settings(),
                topbar::Event::ToggleFullscreen => self.toggle_fullscreen(ctx),
                topbar::Event::Back => {
                    self.top_bar.focus = None;
                    let sh = &mut self.sh;
                    let back = Press { action: Action::Back, repeat: false };
                    return self.stack.last_mut().expect("never empty").handle(sh, back);
                }
                _ => {}
            }
            Nav::None
        } else {
            let sh = &mut self.sh;
            self.stack.last_mut().expect("never empty").handle(sh, press)
        }
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
            (true, Some(i)) => self.truncate(i),
            (false, Some(i)) => self.truncate(i + 1),
            (false, None) => self.stack.push(Box::new(SearchView::default())),
            (true, None) => {}
        }
    }

    fn global_keys(&mut self, ctx: &egui::Context) {
        let (f1, f11) = ctx.input(|i| (i.key_pressed(Key::F1), i.key_pressed(Key::F11)));
        if f1 {
            self.show_input_log = !self.show_input_log;
            input::os::set_diagnostics(self.show_input_log);
        }
        if f11 {
            self.toggle_fullscreen(ctx);
        }
    }

    /// On the video a click pauses or plays, and a double-click toggles fullscreen (its two
    /// clicks pause and unpause, so playback carries on). Elsewhere the mouse is left free
    /// for normal clicking.
    fn player_clicks(&mut self, ctx: &egui::Context) {
        if !self.top().is_player() {
            return;
        }
        // Where the click happened: egui forgets the press origin on release, which is
        // when the click registers, but the pointer is still there.
        let (click, double, at) = ctx.input(|i| {
            (i.pointer.primary_clicked(), i.pointer.button_double_clicked(egui::PointerButton::Primary), i.pointer.latest_pos())
        });
        // Clicks on the seek bar seek instead (the player handles them).
        if at.is_some_and(|pos| self.top().on_controls(pos)) {
            return;
        }
        if click {
            let nav = self.route(ctx, Press { action: Action::PlayPause, repeat: false });
            self.apply(ctx, nav);
        }
        if double {
            self.toggle_fullscreen(ctx);
        }
    }

    /// Closes when Back is held for `QUIT_HOLD_SECS`, showing a prompt while held.
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
            theme::paint_overlay(painter, rect, 14.0);
            let prompt = if self.tray.is_some() { "Keep holding to close" } else { "Keep holding to quit" };
            painter.text(rect.center() - vec2(0.0, 22.0), Align2::CENTER_CENTER, prompt, TextStyle::Body.resolve(ui.style()), theme::overlay_text());
            let bar = Rect::from_min_size(pos2(rect.left() + 40.0, rect.bottom() - 46.0), vec2(rect.width() - 80.0, 10.0));
            painter.rect_filled(bar, CornerRadius::same(5), theme::overlay_text().gamma_multiply(0.22));
            let t = ((held - QUIT_PROMPT_SECS) / (QUIT_HOLD_SECS - QUIT_PROMPT_SECS)) as f32;
            painter.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * t, bar.height())), CornerRadius::same(5), theme::accent());
        }
        ctx.request_repaint();
    }

    /// Toasts, newest at the bottom, with `sticky` (e.g. update progress) below them
    /// for as long as it's given.
    fn draw_toasts(&mut self, ui: &egui::Ui, now: f64, sticky: Option<String>) {
        for toast in self.sh.take_toasts() {
            add_toast(&mut self.toasts, toast, now);
        }
        self.toasts.retain(|(_, until, _)| *until > now);
        let next_expiry = self.toasts.iter().map(|(_, t, _)| *t).reduce(f64::min);
        if next_expiry.is_none() && sticky.is_none() {
            return;
        }
        let safe = theme::safe_rect(ui.ctx());
        let font = TextStyle::Body.resolve(ui.style());
        let mut bottom = safe.bottom();
        for msg in sticky.iter().chain(self.toasts.iter().rev().map(|(m, _, _)| m)) {
            let galley = ui.painter().layout(msg.clone(), font.clone(), theme::overlay_text(), safe.width() * 0.7);
            let rect = Rect::from_center_size(
                pos2(safe.center().x, bottom - galley.size().y / 2.0 - 16.0),
                galley.size() + vec2(48.0, 32.0),
            );
            theme::paint_overlay(ui.painter(), rect, 12.0);
            ui.painter().galley(rect.center() - galley.size() / 2.0, galley, theme::overlay_text());
            bottom = rect.top() - 12.0;
        }
        if let Some(next_expiry) = next_expiry {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(next_expiry - now));
        }
    }

    /// Shows the cursor while the mouse moves; hides it after a moment still.
    fn update_cursor(&mut self, ctx: &egui::Context, now: f64) {
        if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
            self.last_pointer_move = now;
        }
        let idle = now - self.last_pointer_move;
        if idle > CURSOR_HIDE_SECS {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(CURSOR_HIDE_SECS - idle + 0.05));
        }
    }

    fn input_log_overlay(&self, ui: &egui::Ui) {
        let safe = theme::safe_rect(ui.ctx());
        let rect = Rect::from_min_size(pos2(safe.right() - 760.0, safe.top() + 90.0), vec2(760.0, 560.0));
        let painter = ui.painter();
        theme::paint_overlay(painter, rect, 12.0);
        let mono = TextStyle::Monospace.resolve(ui.style());
        let mut y = rect.top() + 20.0;
        painter.text(pos2(rect.left() + 24.0, y), Align2::LEFT_TOP, "Input log (F1 to hide)", mono.clone(), theme::accent());
        for entry in self.input_log.entries() {
            y += 40.0;
            painter.text(pos2(rect.left() + 24.0, y), Align2::LEFT_TOP, entry, mono.clone(), theme::overlay_text());
        }
    }
}

impl eframe::App for FlexApp {
    /// Runs even while the window is hidden, so scans finish without anyone watching.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sh.poll_scans();
        self.sh.poll_youtube();
        self.sh.poll_sync();
        let playing = self.top().playback().map(|p| (p, self.top().playing_file()));
        self.sh.poll_plugins(playing);
        self.handle_commands(ctx);
        self.handle_remote(ctx);
        self.install_update();
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        theme::update_scale(&ctx);
        self.sh.display_fps = frame
            .winit_window()
            .and_then(|w| w.current_monitor())
            .and_then(|m| m.refresh_rate_millihertz())
            .map(|mhz| f64::from(mhz) / 1000.0);
        self.frames += 1;
        // Not on the first frame: eframe shows the window after painting it.
        if self.hide_pending && self.frames > 1 {
            self.hide_pending = false;
            self.hide_to_tray(&ctx);
        }
        if self.restore_fullscreen && self.frames > 2 {
            self.restore_fullscreen = false;
            self.set_fullscreen(&ctx, true);
        }
        self.global_keys(&ctx);

        let text_focused = ctx.text_edit_focused();
        let presses = input::collect(&ctx, &mut self.input_log, text_focused);
        if self.top().shows_top_bar() && !text_focused && topbar::typed_text(&ctx) {
            // Start searching as soon as someone types; the box picks up this frame's text.
            topbar::focus_text(&ctx);
        }
        for press in presses {
            match press.action {
                Action::ZoomIn => {
                    if !press.repeat {
                        self.toggle_fullscreen(&ctx);
                    }
                }
                Action::Search => {
                    // Not from the player: a stray press shouldn't end playback.
                    if self.top().shows_top_bar() {
                        topbar::focus_text(&ctx);
                        self.top_bar.focus = Some(topbar::Item::Search);
                    }
                }
                _ => {
                    let nav = self.route(&ctx, press);
                    self.apply(&ctx, nav);
                }
            }
        }
        self.player_clicks(&ctx);

        theme::paint_background(ui.painter(), ctx.content_rect());
        let safe = theme::safe_rect(&ctx);
        let content = if self.top().shows_top_bar() {
            let bar = Rect::from_min_size(safe.min, vec2(safe.width(), topbar::HEIGHT));
            match self.top_bar.ui(ui, bar, &mut self.sh, self.stack.len() > 1) {
                topbar::Event::Submit => {
                    self.sync_search();
                    let sh = &mut self.sh;
                    let nav = match self.stack.last_mut() {
                        Some(s) if s.is_search() => s.activate_first(sh),
                        _ => Nav::None,
                    };
                    self.apply(&ctx, nav);
                }
                topbar::Event::Cancel | topbar::Event::Back => {
                    let sh = &mut self.sh;
                    let back = Press { action: Action::Back, repeat: false };
                    let nav = self.stack.last_mut().expect("never empty").handle(sh, back);
                    self.apply(&ctx, nav);
                }
                topbar::Event::OpenSettings => self.open_settings(),
                topbar::Event::ToggleFullscreen => self.toggle_fullscreen(&ctx),
                topbar::Event::FocusContent | topbar::Event::None => {}
            }
            self.sync_search();
            Rect::from_min_max(pos2(safe.left(), safe.top() + topbar::HEIGHT + 36.0), safe.max)
        } else {
            ctx.content_rect()
        };

        // Keep clear of the theme's foreground (e.g. the farmyard's flock).
        let content = if self.top().is_player() {
            content
        } else {
            let clear = ctx.content_rect().bottom() - theme::foreground_height();
            content.with_max_y(content.max.y.min(clear))
        };
        let focused = self.top_bar.focus.is_none() && !ctx.text_edit_focused();
        let sh = &mut self.sh;
        let nav = self.stack.last_mut().expect("never empty").ui(ui, sh, content, focused);
        self.apply(&ctx, nav);
        // The screen has selected whatever the mouse is on; a click then acts like OK.
        match widgets::take_hit(&ctx) {
            Hit::None => {}
            Hit::Hover => self.top_bar.focus = None,
            Hit::Click => {
                self.top_bar.focus = None;
                let sh = &mut self.sh;
                let ok = Press { action: Action::Ok, repeat: false };
                let nav = self.stack.last_mut().expect("never empty").handle(sh, ok);
                self.apply(&ctx, nav);
            }
        }

        if !self.top().is_player() {
            theme::paint_foreground(ui.painter(), ctx.content_rect());
        }
        self.update_cursor(&ctx, now);
        let status = self.update_status();
        self.draw_toasts(ui, now, status);
        self.hold_to_quit(ui, now, text_focused);
        if self.show_input_log {
            self.input_log_overlay(ui);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::bg().to_normalized_gamma_f32()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Save playback progress, and free any mpv render context while GL is still alive.
        while let Some(mut screen) = self.stack.pop() {
            screen.on_leave(&mut self.sh);
        }
        // Gives plugins a moment to turn lights off.
        self.sh.plugins.shutdown();
    }
}

/// Adds a toast to those showing: a repeat of one already up just refreshes it, and one
/// with a key replaces the toast with that key (or, with no text, only removes it).
fn add_toast(toasts: &mut VecDeque<(String, f64, Option<String>)>, toast: flex_core::plugins::Toast, now: f64) {
    match &toast.key {
        Some(key) => toasts.retain(|(_, _, k)| k.as_ref() != Some(key)),
        None => toasts.retain(|(m, _, _)| *m != toast.text),
    }
    if toast.text.is_empty() {
        return;
    }
    log::info!("{}", toast.text);
    toasts.push_back((toast.text, now + toast.seconds.unwrap_or(TOAST_SECS), toast.key));
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use flex_core::plugins::Toast;

    use super::{TOAST_SECS, add_toast};

    #[test]
    fn keyed_toasts_stay_until_replaced_or_cleared() {
        let mut toasts = VecDeque::new();
        let kept = |text: &str| Toast { text: text.into(), seconds: Some(600.0), key: Some("plugin:trakt:code".into()) };
        add_toast(&mut toasts, kept("Enter ABCD"), 0.0);
        add_toast(&mut toasts, Toast::new("Saved"), 1.0);
        add_toast(&mut toasts, Toast::new("Saved"), 2.0);
        assert_eq!(toasts.len(), 2, "a repeat refreshes rather than stacks");
        assert_eq!(toasts[0].1, 600.0, "kept for its own time");
        assert_eq!(toasts[1].1, 2.0 + TOAST_SECS);
        add_toast(&mut toasts, kept("Enter EFGH"), 3.0);
        assert_eq!(toasts.iter().filter(|t| t.2.is_some()).count(), 1, "same key replaces");
        add_toast(&mut toasts, Toast { text: String::new(), seconds: None, key: Some("plugin:trakt:code".into()) }, 4.0);
        assert!(toasts.iter().all(|t| t.2.is_none()), "empty text clears it");
        assert_eq!(toasts.len(), 1);
    }
}
