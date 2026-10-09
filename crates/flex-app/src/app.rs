use std::path::PathBuf;

use eframe::egui::{self, Align2, CornerRadius, Key, Rect, TextStyle, ViewportCommand, pos2, vec2};

use crate::browser::{self, Browser};
use crate::input::{self, InputLog};
use crate::mpv::GetProcAddress;
use crate::player::{self, Player};
use crate::theme;

const CURSOR_HIDE_SECS: f64 = 2.0;

pub struct FlexApp {
    browser: Browser,
    player: Option<Player>,
    input_log: InputLog,
    show_input_log: bool,
    proc_address: Option<GetProcAddress>,
    last_pointer_move: f64,
}

impl FlexApp {
    /// `start` may be a folder to browse or a video to play immediately.
    pub fn new(cc: &eframe::CreationContext<'_>, start: Option<PathBuf>) -> Self {
        theme::install(&cc.egui_ctx);
        let ctx = &cc.egui_ctx;
        let play = start.as_ref().filter(|p| p.is_file() && browser::is_video(p)).cloned();
        let browse_dir = match &play {
            Some(file) => file.parent().map(PathBuf::from),
            None => start.or_else(default_start_dir),
        };
        let mut app = Self {
            browser: Browser::new(ctx, browse_dir),
            player: None,
            input_log: InputLog::default(),
            show_input_log: false,
            proc_address: cc.get_proc_address.clone(),
            last_pointer_move: 0.0,
        };
        if let Some(file) = play {
            app.play(ctx, &file);
        }
        app
    }

    fn play(&mut self, ctx: &egui::Context, path: &std::path::Path) {
        let Some(proc_address) = self.proc_address.clone() else {
            self.browser.set_error("Video needs the OpenGL renderer, which isn't available");
            return;
        };
        match Player::open(ctx, proc_address, path) {
            Ok(p) => self.player = Some(p),
            Err(e) => {
                let hint = if crate::mpv::ffi::api().is_err() {
                    if cfg!(windows) {
                        "Put libmpv-2.dll next to flex.exe (see README).\n"
                    } else {
                        "Install libmpv or set FLEX_LIBMPV to its path (see README).\n"
                    }
                } else {
                    ""
                };
                self.browser.set_error(format!("{hint}{e:#}"));
            }
        }
    }

    fn stop(&mut self, error: Option<String>) {
        self.player = None;
        if let Some(e) = error {
            self.browser.set_error(e);
        }
    }

    fn global_keys(&mut self, ctx: &egui::Context) {
        let (f1, f11, fullscreen) =
            ctx.input(|i| (i.key_pressed(Key::F1), i.key_pressed(Key::F11), i.viewport().fullscreen));
        if f1 {
            self.show_input_log = !self.show_input_log;
        }
        if f11 {
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(!fullscreen.unwrap_or(false)));
        }
    }

    fn update_cursor(&mut self, ctx: &egui::Context) {
        let (now, moved) = ctx.input(|i| (i.time, i.pointer.delta() != egui::Vec2::ZERO));
        if moved {
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
        let rect = Rect::from_min_size(pos2(safe.right() - 760.0, safe.top()), vec2(760.0, 560.0));
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(12), theme::OSD_BG);
        let mut y = rect.top() + 20.0;
        let mono = TextStyle::Monospace.resolve(ui.style());
        painter.text(pos2(rect.left() + 24.0, y), Align2::LEFT_TOP, "Input log (F1 to hide)", mono.clone(), theme::FOCUS);
        y += 44.0;
        for entry in self.input_log.entries() {
            painter.text(pos2(rect.left() + 24.0, y), Align2::LEFT_TOP, entry, mono.clone(), theme::TEXT);
            y += 38.0;
        }
    }
}

fn default_start_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return None; // drive list
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let media = home.join("media");
    Some(if media.is_dir() { media } else { home })
}

impl eframe::App for FlexApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        theme::update_scale(&ctx);
        self.global_keys(&ctx);
        let presses = input::collect(&ctx, &mut self.input_log);
        let now = ctx.input(|i| i.time);

        if let Some(player) = &mut self.player {
            let mut outcome = player::Outcome::Continue;
            for press in presses {
                if let player::Outcome::Exit(e) = player.handle(now, press) {
                    outcome = player::Outcome::Exit(e);
                    break;
                }
            }
            if let player::Outcome::Continue = outcome {
                outcome = player.ui(ui);
            }
            if let player::Outcome::Exit(error) = outcome {
                self.stop(error);
                ctx.request_repaint();
            }
        } else {
            for press in presses {
                if let Some(path) = self.browser.handle(&ctx, press) {
                    self.play(&ctx, &path);
                    ctx.request_repaint();
                    break;
                }
            }
            let status = format!(
                "Last input: {}    ·    F1: input log",
                self.input_log.latest().unwrap_or("none yet")
            );
            ui.painter().rect_filled(ctx.content_rect(), CornerRadius::ZERO, theme::BG);
            self.browser.ui(ui, Some(&status));
            self.update_cursor(&ctx);
        }

        if self.show_input_log {
            self.input_log_overlay(ui);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Free the mpv render context while the GL context is still alive.
        self.player = None;
    }
}
