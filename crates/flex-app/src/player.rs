//! Full-screen video playback with a minimal d-pad OSD.
//!
//! OK / Play-Pause: pause/play · Left/Right: seek (hold to go faster) · Up: subtitles ·
//! Down: audio · Back: hide the OSD, or stop if it's already hidden · Stop: stop.

use std::sync::Arc;

use anyhow::Result;
use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, TextStyle, pos2, vec2};
use eframe::egui_glow;
use eframe::glow::{self, HasContext};

use crate::input::{Action, Press};
use crate::mpv::{self, GetProcAddress, Mpv, Renderer};
use crate::screens::{Nav, Screen, up_next::UpNext};
use crate::shared::{PlayRequest, Shared};
use crate::theme;

const OSD_SECS: f64 = 3.0;
/// How often the playback position is saved while playing.
const SAVE_EVERY_SECS: f64 = 10.0;
const MESSAGE_SECS: f64 = 2.5;
/// Extra mpv options for testing, e.g. `FLEX_MPV_OPTS="hwdec=no,profile=fast"`.
const OPTS_ENV: &str = "FLEX_MPV_OPTS";

#[derive(Clone, Copy)]
enum Track {
    Sub,
    Audio,
}

pub struct Player {
    req: PlayRequest,
    mpv: Arc<Mpv>,
    renderer: Arc<Renderer>,
    loaded: bool,
    /// Set once the file played to the end (progress already recorded as watched).
    finished: bool,
    last_save: f64,
    /// Last known duration; mpv forgets it once the file ends.
    duration: Option<f64>,
    osd_until: f64,
    /// Track whose current selection is being shown, and until when.
    message: Option<(Track, f64)>,
}

impl Player {
    pub fn open(ctx: &egui::Context, proc_address: GetProcAddress, req: PlayRequest) -> Result<Self> {
        let extra = std::env::var(OPTS_ENV).unwrap_or_default();
        let options: Vec<(&str, &str)> = extra.split(',').filter_map(|kv| kv.split_once('=')).collect();
        let mpv = Arc::new(Mpv::new(ctx, &options)?);
        let renderer = Arc::new(Renderer::new(mpv.clone(), proc_address, ctx)?);
        if let Some(start) = req.start.filter(|s| *s > 0.0) {
            mpv.set_string("start", &format!("{start:.1}"))?;
        }
        log::info!("playing {}", req.path.display());
        mpv.command(&["loadfile", &Self::load_target(&mpv, &req.path)?])?;
        let now = ctx.input(|i| i.time);
        Ok(Self {
            req,
            mpv,
            renderer,
            loaded: false,
            finished: false,
            last_save: now,
            duration: None,
            osd_until: now + OSD_SECS,
            message: None,
        })
    }

    /// What to hand mpv's `loadfile`: the file itself, or for a disc folder (`VIDEO_TS` or a
    /// folder of `.VOB`s, or `BDMV`) the disc URL with the folder set as the device.
    fn load_target(mpv: &Mpv, path: &std::path::Path) -> Result<String> {
        if !path.is_dir() {
            return Ok(path.to_string_lossy().into_owned());
        }
        let is_bluray = path.file_name().is_some_and(|n| n.eq_ignore_ascii_case("BDMV"));
        if is_bluray {
            let device = path.parent().unwrap_or(path);
            mpv.set_string("bluray-device", &device.to_string_lossy())?;
            Ok("bd://".to_owned())
        } else {
            mpv.set_string("dvd-device", &path.to_string_lossy())?;
            Ok("dvd://".to_owned())
        }
    }

    fn save_progress(&mut self, sh: &mut Shared) {
        if self.finished || !self.loaded {
            return;
        }
        if let (Some(position), Some(duration)) = (self.mpv.get_f64("time-pos"), self.duration) {
            sh.record_progress(&self.req, position, duration);
        }
    }

    fn paused(&self) -> bool {
        self.mpv.get_flag("pause").unwrap_or(false)
    }

    fn command(&self, args: &[&str]) {
        if let Err(e) = self.mpv.command(args) {
            log::warn!("{e:#}");
        }
    }

    fn handle_press(&mut self, now: f64, press: Press) -> Nav {
        let osd_visible = now < self.osd_until;
        match press.action {
            Action::Ok | Action::PlayPause if !press.repeat => {
                let _ = self.mpv.set_flag("pause", !self.paused());
            }
            Action::Stop if !press.repeat => return Nav::Pop,
            Action::Left | Action::Right => {
                let step = if press.repeat { 30 } else { 10 };
                let step = if press.action == Action::Left { -step } else { step };
                self.command(&["seek", &step.to_string(), "relative"]);
            }
            Action::Up if !press.repeat => {
                self.command(&["cycle", "sub"]);
                self.message = Some((Track::Sub, now + MESSAGE_SECS));
            }
            Action::Down if !press.repeat => {
                self.command(&["cycle", "audio"]);
                self.message = Some((Track::Audio, now + MESSAGE_SECS));
            }
            Action::Back if !press.repeat => {
                if osd_visible && !self.paused() {
                    self.osd_until = 0.0;
                    return Nav::None;
                }
                return Nav::Pop;
            }
            _ => return Nav::None,
        }
        if !matches!(press.action, Action::Up | Action::Down) {
            self.osd_until = now + OSD_SECS;
        }
        Nav::None
    }

    fn track_label(&self, track: Track) -> String {
        let (kind, name) = match track {
            Track::Sub => ("sub", "Subtitles"),
            Track::Audio => ("audio", "Audio"),
        };
        let prop = |p: &str| self.mpv.get_string(&format!("current-tracks/{kind}/{p}")).filter(|s| !s.is_empty());
        let Some(id) = self.mpv.get_i64(&format!("current-tracks/{kind}/id")) else {
            return format!("{name}: Off");
        };
        let mut label = format!("{name}: #{id}");
        if let Some(lang) = prop("lang") {
            label += &format!(" [{lang}]");
        }
        if let Some(title) = prop("title") {
            label += &format!(" {title}");
        }
        if let Some(codec) = prop("codec") {
            label += &format!(" ({codec})");
        }
        label
    }

    fn draw(&mut self, ui: &mut egui::Ui, sh: &mut Shared) -> Nav {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        for event in self.mpv.poll_events() {
            match event {
                mpv::Event::FileLoaded => self.loaded = true,
                mpv::Event::EndFile { error: Some(e), .. } => {
                    sh.toast(format!("Couldn't play {}: {e}", self.req.title));
                    return Nav::Pop;
                }
                mpv::Event::EndFile { eof: true, .. } => {
                    if let Some(duration) = self.duration {
                        sh.record_progress(&self.req, duration, duration);
                    }
                    self.finished = true;
                    return match sh.next_after(&self.req) {
                        Some(next) => Nav::replace(UpNext::new(self.req.title.clone(), next)),
                        None => Nav::Pop,
                    };
                }
                mpv::Event::Shutdown => return Nav::Pop,
                mpv::Event::EndFile { .. } => {}
            }
        }

        if self.loaded {
            self.duration = self.mpv.get_f64("duration").or(self.duration);
            if now - self.last_save >= SAVE_EVERY_SECS {
                self.last_save = now;
                self.save_progress(sh);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(SAVE_EVERY_SECS));
        }

        let screen = ctx.content_rect();
        let renderer = self.renderer.clone();
        let callback = egui_glow::CallbackFn::new(move |info, painter| {
            let target = painter.intermediate_fbo();
            let fbo = target.map_or(0, |f| f.0.get() as i32);
            let [w, h] = info.screen_size_px;
            renderer.render(fbo, w as i32, h as i32, target.is_none());
            // mpv doesn't restore GL state; egui resets the rest after callbacks.
            unsafe { painter.gl().bind_framebuffer(glow::FRAMEBUFFER, target) };
        });
        ui.painter().add(egui::PaintCallback { rect: screen, callback: Arc::new(callback) });
        ctx.set_cursor_icon(egui::CursorIcon::None);

        let painter = ui.painter();
        let safe = theme::safe_rect(&ctx);
        if !self.loaded {
            painter.text(screen.center(), Align2::CENTER_CENTER, "Loading…", TextStyle::Heading.resolve(ui.style()), theme::TEXT);
        }

        let paused = self.paused();
        if paused || now < self.osd_until {
            self.draw_osd(ui, safe, paused);
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if let Some((track, until)) = self.message {
            if now < until {
                let text = self.track_label(track);
                let font = TextStyle::Body.resolve(ui.style());
                let galley = painter.layout_no_wrap(text, font, theme::TEXT);
                let rect = Rect::from_center_size(
                    pos2(safe.center().x, safe.top() + 40.0),
                    galley.size() + vec2(48.0, 24.0),
                );
                painter.rect_filled(rect, CornerRadius::same(10), theme::OSD_BG);
                painter.galley(rect.center() - galley.size() / 2.0, galley, theme::TEXT);
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(until - now));
            } else {
                self.message = None;
            }
        }
        Nav::None
    }

    fn draw_osd(&self, ui: &egui::Ui, safe: Rect, paused: bool) {
        let painter = ui.painter();
        let pos = self.mpv.get_f64("time-pos").unwrap_or(0.0);
        let duration = self.mpv.get_f64("duration").unwrap_or(0.0);

        let panel = Rect::from_min_max(pos2(safe.left(), safe.bottom() - 150.0), safe.right_bottom());
        painter.rect_filled(panel, CornerRadius::same(14), theme::OSD_BG);
        let inner = panel.shrink2(vec2(32.0, 24.0));

        let mut title_pos = inner.left_top();
        if paused {
            // Pause symbol drawn as two bars: fonts don't reliably have a glyph for it.
            for x in [0.0, 14.0] {
                let bar = Rect::from_min_size(title_pos + vec2(x, 6.0), vec2(8.0, 26.0));
                painter.rect_filled(bar, CornerRadius::same(2), theme::TEXT);
            }
            title_pos.x += 44.0;
        }
        painter.text(title_pos, Align2::LEFT_TOP, &self.req.title, TextStyle::Body.resolve(ui.style()), theme::TEXT);
        let time = format!("{} / {}", theme::format_time(pos), theme::format_time(duration));
        painter.text(inner.right_bottom(), Align2::RIGHT_BOTTOM, &time, TextStyle::Body.resolve(ui.style()), theme::TEXT);

        let bar = Rect::from_min_size(pos2(inner.left(), inner.bottom() - 26.0), vec2(inner.width() - 300.0, 10.0));
        painter.rect_filled(bar, CornerRadius::same(5), Color32::from_white_alpha(50));
        if duration > 0.0 {
            let mut done = bar;
            done.set_width(bar.width() * (pos / duration).clamp(0.0, 1.0) as f32);
            painter.rect_filled(done, CornerRadius::same(5), theme::FOCUS);
        }
    }
}

impl Screen for Player {
    fn handle(&mut self, sh: &mut Shared, press: Press) -> Nav {
        let now = sh.ctx.input(|i| i.time);
        match press.action {
            Action::Next if !press.repeat => match sh.next_after(&self.req) {
                Some(req) => Nav::Play { req, replace: true },
                None => {
                    sh.toast("No next episode in this folder");
                    Nav::None
                }
            },
            Action::Previous if !press.repeat => match sh.prev_before(&self.req) {
                Some(req) => Nav::Play { req, replace: true },
                None => {
                    sh.toast("No previous episode in this folder");
                    Nav::None
                }
            },
            _ => self.handle_press(now, press),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, sh: &mut Shared, _rect: Rect, _focused: bool) -> Nav {
        self.draw(ui, sh)
    }

    fn shows_top_bar(&self) -> bool {
        false
    }

    fn is_player(&self) -> bool {
        true
    }

    fn on_leave(&mut self, sh: &mut Shared) {
        self.save_progress(sh);
    }
}
