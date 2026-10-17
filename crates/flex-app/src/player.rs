//! Full-screen video playback with a minimal d-pad OSD.
//!
//! OK / Play-Pause: pause/play · Left/Right: seek (hold to go faster) · Up: subtitles ·
//! Down: audio · Back: hide the OSD, or stop if it's already hidden · Stop: stop.
//! F2: playback stats (dropped frames, decoder, refresh rate).
//! Mouse: moving it shows the OSD; click or drag the seek bar to seek.
//!
//! The subtitle track chosen with Up is remembered (by name and language, or "off") and
//! picked again in later files that have a track like it.

use std::sync::Arc;

use anyhow::Result;
use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, TextStyle, pos2, vec2};
use eframe::egui_glow;
use eframe::glow::{self, HasContext};

use flex_core::youtube::sponsorblock::{self, Segment};
use flex_plugin::{Light, Playback, PlaybackState};

use crate::ambient::Margins;
use crate::input::{Action, Press};
use crate::mpv::{self, GetProcAddress, Mpv, Renderer};
use crate::screens::{Nav, Screen, up_next::UpNext};
use crate::shared::{PlayRequest, Shared};
use crate::theme;

const OSD_SECS: f64 = 3.0;
/// How often the playback position is saved while playing.
const SAVE_EVERY_SECS: f64 = 10.0;
const MESSAGE_SECS: f64 = 2.5;
/// While dragging the seek bar, how often to seek to where the mouse is.
const DRAG_SEEK_SECS: f64 = 0.1;
/// How far above and below the seek bar it still takes the mouse.
const BAR_GRAB: f32 = 20.0;
/// Extra mpv options for testing, e.g. `FLEX_MPV_OPTS="hwdec=no,profile=fast"`.
const OPTS_ENV: &str = "FLEX_MPV_OPTS";
const SUBTITLE_SETTING: &str = "last_subtitle";
/// Separates title and language in the saved subtitle choice.
const SUB_SEP: char = '\u{1f}';

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
    /// For the history: when this session started, the furthest position recorded, whether
    /// it reached "watched", and whether the session has been logged.
    session_start: i64,
    recorded: Option<f64>,
    became_watched: bool,
    logged: bool,
    osd_until: f64,
    /// Track whose current selection is being shown, and until when.
    message: Option<(Track, f64)>,
    /// The user changed subtitles: save the new choice once mpv has applied it.
    sub_changed: bool,
    /// Display refresh rate last passed to mpv.
    display_fps: Option<f64>,
    show_stats: bool,
    /// Where the OSD panel was last drawn, if it's showing (clicks there don't pause).
    osd_panel: Option<Rect>,
    /// While the seek bar is held: the position under the mouse, and when it last seeked.
    drag: Option<(f64, f64)>,
    /// For a YouTube video: its streams are found in the background, then loaded.
    youtube: Option<YouTubePlay>,
}

struct YouTubePlay {
    id: String,
    loaded: bool,
    /// SponsorBlock segments to skip, once known.
    segments: Option<Vec<Segment>>,
    /// Position at the last check, to tell playing into a segment from seeking into it.
    last_pos: Option<f64>,
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
        let youtube = req.youtube_id().map(|id| YouTubePlay { id: id.to_owned(), loaded: false, segments: None, last_pos: None });
        if youtube.is_none() {
            mpv.load(&req.path)?;
        }
        let now = ctx.input(|i| i.time);
        Ok(Self {
            req,
            mpv,
            renderer,
            loaded: false,
            finished: false,
            last_save: now,
            duration: None,
            session_start: crate::shared::now_unix(),
            recorded: None,
            became_watched: false,
            logged: false,
            osd_until: now + OSD_SECS,
            message: None,
            sub_changed: false,
            display_fps: None,
            show_stats: false,
            osd_panel: None,
            drag: None,
            youtube,
        })
    }

    fn save_progress(&mut self, sh: &mut Shared) {
        if self.finished || !self.loaded {
            return;
        }
        if let (Some(position), Some(duration)) = (self.mpv.get_f64("time-pos"), self.duration)
            && let Some(p) = sh.record_progress(&self.req, position, duration)
        {
            self.recorded = Some(position);
            self.became_watched |= p.watched && p.position.is_none();
        }
    }

    /// Logs this viewing session in the history (once), if anything was recorded.
    fn log_session(&mut self, sh: &mut Shared) {
        if let Some(position) = self.recorded
            && !self.logged
        {
            sh.add_history(&self.req, self.session_start, Some(position), self.became_watched);
            self.logged = true;
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
                self.sub_changed = true;
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

    /// The current subtitle choice as stored: "off" or title + SUB_SEP + language.
    fn current_sub(&self) -> String {
        if self.mpv.get_i64("current-tracks/sub/id").is_none() {
            return "off".to_owned();
        }
        let prop = |p: &str| self.mpv.get_string(&format!("current-tracks/sub/{p}")).unwrap_or_default();
        format!("{}{SUB_SEP}{}", prop("title"), prop("lang"))
    }

    /// Picks the subtitle track that best matches the remembered choice, if any does.
    fn apply_saved_sub(&self, sh: &Shared) {
        let Some(saved) = sh.setting(SUBTITLE_SETTING) else { return };
        if saved == "off" {
            let _ = self.mpv.set_string("sid", "no");
            return;
        }
        let (title, lang) = saved.split_once(SUB_SEP).unwrap_or((&saved, ""));
        let count = self.mpv.get_i64("track-list/count").unwrap_or(0);
        let mut best: Option<(u8, i64)> = None;
        for i in 0..count {
            let prop = |p: &str| self.mpv.get_string(&format!("track-list/{i}/{p}")).unwrap_or_default();
            if prop("type") != "sub" {
                continue;
            }
            let Some(id) = self.mpv.get_i64(&format!("track-list/{i}/id")) else { continue };
            let same_title = !title.is_empty() && prop("title").eq_ignore_ascii_case(title);
            let same_lang = !lang.is_empty() && prop("lang").eq_ignore_ascii_case(lang);
            // Same name matters most; a language-only match only counts if the saved
            // track had no name.
            let score = match (same_title, same_lang) {
                (true, true) => 3,
                (true, false) => 2,
                (false, true) if title.is_empty() => 1,
                _ => 0,
            };
            if score > 0 && best.is_none_or(|(s, _)| score > s) {
                best = Some((score, id));
            }
        }
        if let Some((_, id)) = best {
            let _ = self.mpv.set_string("sid", &id.to_string());
        }
    }

    fn stats_lines(&self) -> Vec<String> {
        let f = |p: &str| self.mpv.get_f64(p);
        let i = |p: &str| self.mpv.get_i64(p).map_or("-".to_owned(), |v| v.to_string());
        let s = |p: &str| self.mpv.get_string(p).unwrap_or_else(|| "-".to_owned());
        vec![
            format!("Video: {} {}x{}", s("video-codec"), i("width"), i("height")),
            format!("Decoder: hwdec {}", s("hwdec-current")),
            format!(
                "Video fps {:.3} · display {:.3} Hz",
                f("container-fps").unwrap_or(0.0),
                f("display-fps").unwrap_or(0.0)
            ),
            format!("Sync: {} · speed {:.4}", s("video-sync"), f("speed").unwrap_or(1.0) * f("audio-speed-correction").unwrap_or(1.0)),
            format!("Dropped: {} by output · {} by decoder", i("frame-drop-count"), i("decoder-frame-drop-count")),
            format!("Late frames: {} · mistimed: {}", i("vo-delayed-frame-count"), i("mistimed-frame-count")),
        ]
    }

    /// `light`: what the ambient light shows, with a swatch, when it's on.
    fn draw_stats(&self, ui: &egui::Ui, safe: Rect, light: Option<&Light>) {
        let painter = ui.painter();
        let mono = TextStyle::Monospace.resolve(ui.style());
        let mut lines = self.stats_lines();
        if let Some(light) = light {
            let [r, g, b] = light.zones[0];
            lines.push(format!("Ambient light: #{r:02x}{g:02x}{b:02x} at {:.0}%", light.brightness * 100.0));
        }
        let rect = Rect::from_min_size(safe.min, vec2(860.0, 32.0 + 36.0 * lines.len() as f32));
        painter.rect_filled(rect, CornerRadius::same(10), theme::OSD_BG);
        for (n, line) in lines.iter().enumerate() {
            let pos = rect.min + vec2(20.0, 16.0 + 36.0 * n as f32);
            painter.text(pos, Align2::LEFT_TOP, line, mono.clone(), theme::TEXT);
        }
        if let Some(light) = light {
            // The colour as the strip shows it (colour scaled by brightness), turned from
            // LED values back into screen ones.
            let srgb = |c: u8| (flex_core::ambient::linear_to_srgb(f32::from(c) / 255.0 * light.brightness) * 255.0).round() as u8;
            let [r, g, b] = light.zones[0].map(srgb);
            let swatch = Rect::from_min_size(rect.right_top() + vec2(-80.0, rect.height() - 52.0), vec2(56.0, 36.0));
            painter.rect_filled(swatch, CornerRadius::same(6), Color32::from_rgb(r, g, b));
        }
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

    /// YouTube: loads the streams once found, and skips SponsorBlock segments.
    fn update_youtube(&mut self, sh: &mut Shared, ctx: &egui::Context) -> Option<Nav> {
        let yt = self.youtube.as_mut()?;
        if !yt.loaded {
            match sh.youtube.resolved(&yt.id) {
                Some(Ok(video)) => {
                    if self.req.title == "YouTube" {
                        self.req.title = video.title.clone();
                    }
                    log::info!("youtube: playing {} ({:?}p)", video.title, video.height);
                    if let Err(e) = self.mpv.command(&["loadfile", &video.relayed_mpv_url()]) {
                        sh.toast(format!("Couldn't play that video: {e:#}"));
                        return Some(Nav::Pop);
                    }
                    yt.loaded = true;
                    // Where playback starts, so a segment right there (e.g. a sponsor read
                    // at 0:00) counts as played into.
                    yt.last_pos = Some(self.req.start.unwrap_or(0.0));
                }
                Some(Err(e)) => {
                    let e = e.clone();
                    sh.toast(format!("Couldn't play that video: {e}"));
                    return Some(Nav::Pop);
                }
                None => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            }
            return None;
        }
        if yt.segments.is_none() {
            yt.segments = sh.youtube.segments(&yt.id).map(<[Segment]>::to_vec);
        }
        if let (Some(segments), Some(pos)) = (&yt.segments, self.mpv.get_f64("time-pos")) {
            let skip = yt.last_pos.and_then(|before| sponsorblock::entered(segments, before, pos));
            if let Some(segment) = skip {
                let _ = self.mpv.command(&["seek", &format!("{:.3}", segment.end), "absolute"]);
                sh.toast(format!("Skipped: {}", sponsorblock::category_name(&segment.category)));
                yt.last_pos = Some(segment.end);
            } else {
                yt.last_pos = Some(pos);
            }
        }
        None
    }

    fn draw(&mut self, ui: &mut egui::Ui, sh: &mut Shared) -> Nav {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        if let Some(nav) = self.update_youtube(sh, &ctx) {
            return nav;
        }
        for event in self.mpv.poll_events() {
            match event {
                mpv::Event::FileLoaded => {
                    self.loaded = true;
                    self.apply_saved_sub(sh);
                }
                mpv::Event::EndFile { error: Some(e), .. } => {
                    sh.toast(format!("Couldn't play {}: {e}", self.req.title));
                    return Nav::Pop;
                }
                mpv::Event::EndFile { eof: true, .. } => {
                    if let Some(duration) = self.duration
                        && sh.record_progress(&self.req, duration, duration).is_some()
                    {
                        self.recorded = Some(duration);
                        self.became_watched = true;
                    }
                    self.log_session(sh);
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

        if std::mem::take(&mut self.sub_changed) {
            sh.set_setting(SUBTITLE_SETTING, &self.current_sub());
        }
        // Tell mpv the real refresh rate (it can't see the display through the render API).
        if let Some(fps) = sh.display_fps
            && self.display_fps != Some(fps)
        {
            self.display_fps = Some(fps);
            let value = format!("{fps:.3}");
            if self.mpv.set_string("display-fps-override", &value).is_err() {
                let _ = self.mpv.set_string("override-display-fps", &value); // mpv < 0.37
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
            self.show_stats = !self.show_stats;
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
        // Ambient light: sample the picture (without the black bars) right after it's drawn.
        let sampler = sh.plugins.wants_light().then(|| sh.ambient.sampler.clone());
        let margin = |k: &str| self.mpv.get_i64(&format!("osd-dimensions/{k}")).unwrap_or(0) as i32;
        let margins = if sampler.is_some() {
            Margins { left: margin("ml"), right: margin("mr"), top: margin("mt"), bottom: margin("mb") }
        } else {
            Margins::default()
        };
        let callback = egui_glow::CallbackFn::new(move |info, painter| {
            let target = painter.intermediate_fbo();
            let fbo = target.map_or(0, |f| f.0.get() as i32);
            let [w, h] = info.screen_size_px;
            renderer.render(fbo, w as i32, h as i32, target.is_none());
            if let Some(sampler) = &sampler {
                let mut sampler = sampler.lock().unwrap_or_else(|e| e.into_inner());
                unsafe { sampler.capture(painter.gl(), target, w as i32, h as i32, margins, target.is_none()) };
            }
            // mpv doesn't restore GL state; egui resets the rest after callbacks.
            unsafe { painter.gl().bind_framebuffer(glow::FRAMEBUFFER, target) };
        });
        ui.painter().add(egui::PaintCallback { rect: screen, callback: Arc::new(callback) });

        let painter = ui.painter();
        let safe = theme::safe_rect(&ctx);
        if !self.loaded {
            let asking = self.youtube.as_ref().is_some_and(|yt| !yt.loaded);
            let text = if asking { "Asking YouTube…" } else { "Loading…" };
            painter.text(screen.center(), Align2::CENTER_CENTER, text, TextStyle::Heading.resolve(ui.style()), theme::TEXT);
        }

        if self.show_stats {
            self.draw_stats(ui, safe, sh.ambient.current.as_ref());
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
            self.osd_until = now + OSD_SECS;
        }
        let paused = self.paused();
        if self.loaded && !paused {
            // With display-synced timing mpv decides per refresh which frame to show, so it
            // must be asked every refresh while playing, not only when it signals an update.
            ctx.request_repaint();
        }
        if paused || now < self.osd_until || self.drag.is_some() {
            self.draw_osd(ui, safe, now, paused);
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else {
            self.osd_panel = None;
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

    fn draw_osd(&mut self, ui: &egui::Ui, safe: Rect, now: f64, paused: bool) {
        let duration = self.mpv.get_f64("duration").unwrap_or(0.0);
        let panel = Rect::from_min_max(pos2(safe.left(), safe.bottom() - 150.0), safe.right_bottom());
        self.osd_panel = Some(panel);
        let inner = panel.shrink2(vec2(32.0, 24.0));
        let bar = Rect::from_min_size(pos2(inner.left(), inner.bottom() - 26.0), vec2(inner.width() - 300.0, 10.0));

        // Seek bar: pressing seeks to the mouse, dragging follows it (on keyframes, so it
        // keeps up), and letting go seeks there exactly.
        let grab = bar.expand2(vec2(0.0, BAR_GRAB));
        let response = ui.interact(grab, egui::Id::new("flex-seek-bar"), egui::Sense::click_and_drag());
        let at = |x: f32| duration * ((x - bar.left()) / bar.width()).clamp(0.0, 1.0) as f64;
        if duration > 0.0
            && response.is_pointer_button_down_on()
            && let Some(p) = response.interact_pointer_pos()
        {
            let target = at(p.x);
            let last = self.drag.map_or(f64::NEG_INFINITY, |(_, t)| t);
            if now - last >= DRAG_SEEK_SECS {
                self.command(&["seek", &format!("{target:.3}"), "absolute+keyframes"]);
                self.drag = Some((target, now));
            } else {
                self.drag = Some((target, last));
            }
            self.osd_until = now + OSD_SECS;
            ui.ctx().request_repaint();
        } else if let Some((target, _)) = self.drag.take() {
            // Where it was let go: the last move can arrive in the same frame as the release.
            let target = ui.input(|i| i.pointer.latest_pos()).map_or(target, |p| at(p.x));
            self.command(&["seek", &format!("{target:.3}"), "absolute+exact"]);
            self.osd_until = now + OSD_SECS;
        }
        // Until mpv catches up, show where the bar is being dragged to.
        let pos = self.drag.map(|(t, _)| t).or_else(|| self.mpv.get_f64("time-pos")).unwrap_or(0.0);

        let painter = ui.painter();
        painter.rect_filled(panel, CornerRadius::same(14), theme::OSD_BG);
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

        painter.rect_filled(bar, CornerRadius::same(5), Color32::from_white_alpha(50));
        if duration > 0.0 {
            let mut done = bar;
            done.set_width(bar.width() * (pos / duration).clamp(0.0, 1.0) as f32);
            painter.rect_filled(done, CornerRadius::same(5), theme::FOCUS);
            if response.hovered() || self.drag.is_some() {
                painter.circle_filled(pos2(done.right(), bar.center().y), 12.0, theme::FOCUS);
            }
            // The time under the mouse, above the bar.
            if let Some(p) = response.hover_pos().filter(|_| self.drag.is_none()) {
                let font = TextStyle::Body.resolve(ui.style());
                let galley = painter.layout_no_wrap(theme::format_time(at(p.x)), font, theme::TEXT);
                let x = p.x.clamp(bar.left(), bar.right());
                let label = Rect::from_center_size(pos2(x, panel.top() - 32.0), galley.size() + vec2(24.0, 12.0));
                painter.rect_filled(label, CornerRadius::same(8), theme::OSD_BG);
                painter.galley(label.center() - galley.size() / 2.0, galley, theme::TEXT);
            }
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

    fn playback(&self) -> Option<Playback> {
        let state = if self.paused() { PlaybackState::Paused } else { PlaybackState::Playing };
        Some(Playback { state, title: self.req.title.clone(), kind: "video".into() })
    }

    fn on_controls(&self, pos: egui::Pos2) -> bool {
        self.osd_panel.is_some_and(|panel| panel.contains(pos))
    }

    fn on_leave(&mut self, sh: &mut Shared) {
        self.save_progress(sh);
        self.log_session(sh);
    }
}
