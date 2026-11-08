//! Video effects (`flex_core::effects`): a fragment shader run over every frame of the
//! video, entirely on the GPU. Does nothing unless an effect is chosen.
//!
//! With an effect on, mpv draws into our own texture instead of the screen; the plugin's
//! shader then draws from it (and, with `feedback`, from its own previous output) into
//! another texture, which is copied to the screen. The shader file is checked for changes
//! twice a second and reloaded, so it can be edited while a video plays.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui;
use eframe::egui_glow;
use eframe::glow::{self, HasContext};
use flex_core::audio::{self, Analyser, Features, Tap};
use flex_core::effects::ActiveEffect;
use flex_core::plugins::{SettingKind, SettingSpec};
use flex_plugin::serde_json;

use crate::mpv::Mpv;
use crate::picture::{Picture, Upscale};
use crate::shared::Shared;

/// How often the shader file is checked for changes.
const RECHECK: Duration = Duration::from_millis(500);

struct Target {
    texture: glow::Texture,
    fbo: glow::Framebuffer,
}

struct Objects {
    size: (i32, i32),
    /// What mpv draws.
    video: Target,
    /// The effect's output, this frame's and last frame's (swapped each frame).
    out: [Target; 2],
}

/// What's compiled: the file, when it was last changed, and the setting uniforms (which
/// are declared in the shader's header).
#[derive(PartialEq)]
struct Source {
    path: PathBuf,
    modified: Option<SystemTime>,
    uniforms: Vec<String>,
}

/// Lives on the GL thread (inside the player's paint callback); the UI thread only reads
/// the status.
#[derive(Default)]
pub struct Renderer {
    objects: Option<Objects>,
    vao: Option<glow::VertexArray>,
    /// `u_spectrum`: BANDS × HISTORY.
    spectrum: Option<glow::Texture>,
    depth: DepthGl,
    copy: Option<glow::Program>,
    /// The effect's program; `None` if it didn't compile.
    program: Option<glow::Program>,
    source: Option<Source>,
    checked: Option<Instant>,
    started: Option<Instant>,
    frame: i32,
    /// Which of `out` is drawn this frame.
    current: usize,
    /// The shader's state for the plugin's status line, and which file it's about.
    status: Option<(PathBuf, (bool, String))>,
    /// No GL 3 (or GLES 3): effects can't run at all.
    unsupported: bool,
}

/// What an effect is drawn with this frame, worked out on the UI thread.
#[derive(Clone)]
pub struct Frame {
    pub effect: ActiveEffect,
    /// Where the picture is, without black bars (0–1 from the bottom left).
    pub rect: [f32; 4],
    pub video_time: f32,
    /// The sound, when the effect asked for it and there is some.
    pub audio: Option<Arc<Features>>,
    /// The depth worker, when the effect asked for depth.
    pub depth: Option<crate::depth::Link>,
}

/// The player's side: a smooth playback clock, and the sound for effects that want it.
#[derive(Default)]
pub struct Inputs {
    /// Video time, moving smoothly between mpv's position updates (which only change
    /// once per video frame).
    clock: f64,
    last: Option<Instant>,
    tap: Option<Tap>,
    analyser: Option<Analyser>,
    window: Vec<f32>,
    no_ffmpeg: bool,
}

impl Inputs {
    /// Call once per frame while an effect is on; `path` is the playing file.
    pub fn tick(&mut self, mpv: &Mpv, path: &Path, effect: &ActiveEffect) -> (f32, Option<Arc<Features>>) {
        let now = Instant::now();
        let dt = self.last.map_or(0.0, |t| now.duration_since(t).as_secs_f64());
        self.last = Some(now);
        let paused = mpv.get_flag("pause").unwrap_or(false) || mpv.get_flag("paused-for-cache").unwrap_or(false);
        match mpv.get_f64("time-pos") {
            Some(pos) if paused || (pos - self.clock).abs() > 0.25 => self.clock = pos,
            Some(pos) => {
                // Run at playback speed, gently pulled towards mpv's position.
                self.clock += dt * mpv.get_f64("speed").unwrap_or(1.0);
                self.clock += (pos - self.clock) * (dt * 4.0).min(1.0);
            }
            None => {}
        }

        if !effect.audio || !path.is_file() {
            self.tap = None;
            self.analyser = None;
            return (self.clock as f32, None);
        }
        let stream = match mpv.get_flag("current-tracks/audio/external") {
            Some(true) => None, // a separate audio file: fall back to the video's own
            _ => mpv.get_i64("current-tracks/audio/ff-index"),
        };
        if self.tap.as_ref().is_none_or(|t| t.path() != path || t.stream() != stream) && !self.no_ffmpeg {
            match flex_core::find_tool("ffmpeg") {
                Some(ffmpeg) => self.tap = Some(Tap::new(&ffmpeg, path, stream)),
                None => {
                    log::warn!("video effects: no ffmpeg, so no sound for effects");
                    self.no_ffmpeg = true;
                }
            }
        }
        self.window.resize(audio::WINDOW, 0.0);
        let have = self.tap.as_mut().is_some_and(|tap| tap.window(self.clock, &mut self.window));
        let analyser = self.analyser.get_or_insert_with(Analyser::new);
        let features = analyser.update(have.then_some(self.window.as_slice()), self.clock, dt as f32);
        (self.clock as f32, Some(Arc::new(features.clone())))
    }
}

/// F3 in the player: a floating window for choosing the effect and changing its
/// settings (and Anime4K and SVP) while the video plays. Changes are kept, as in
/// Settings. Returns this video's new picture options when Anime4K or SVP changed.
pub fn window(ctx: &egui::Context, open: &mut bool, sh: &mut Shared, frame: Option<&Frame>, picture: Picture) -> Option<Picture> {
    let mut new_picture = None;
    let right = ctx.content_rect().right();
    egui::Window::new("Video effects")
        .open(open)
        .default_pos(egui::pos2(right - 560.0, 60.0))
        .default_width(520.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.spacing_mut().slider_width = 260.0;
            ui.weak("F3 to hide · changes are kept");

            // Which effect.
            let names: Vec<(String, String)> = sh.effects.list().iter().map(|e| (e.id.clone(), e.manifest.name.clone())).collect();
            let current = sh.effects.chosen().map(|e| e.id.clone());
            let current_name = sh.effects.chosen().map_or("Off".to_owned(), |e| e.manifest.name.clone());
            let mut pick: Option<Option<String>> = None;
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("effect").selected_text(current_name).show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_none(), "Off").clicked() {
                        pick = Some(None);
                    }
                    for (id, name) in &names {
                        if ui.selectable_label(current.as_deref() == Some(id), name).clicked() {
                            pick = Some(Some(id.clone()));
                        }
                    }
                });
                if ui.button("Look for new").on_hover_text("New or removed files in the effects folders").clicked() {
                    sh.refresh_effects();
                }
            });
            if let Some(pick) = pick {
                let (effects, db) = sh.effects_mut();
                effects.choose(pick.as_deref(), db);
            }

            if let Some(effect) = sh.effects.chosen().cloned() {
                let warn = egui::Color32::from_rgb(255, 140, 110);
                if let Some((false, error)) = &sh.effects.status {
                    ui.colored_label(warn, error);
                }
                if let Some(problem) = &effect.problem {
                    ui.colored_label(warn, problem);
                }
                if !effect.manifest.description.is_empty() {
                    ui.weak(&effect.manifest.description);
                }
                let specs: Vec<SettingSpec> = effect.settings().cloned().collect();
                egui::Grid::new("effect-settings").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                    for spec in &specs {
                        setting(ui, sh, &effect.id, spec);
                        ui.end_row();
                    }
                });
                if !specs.is_empty() && ui.button("Back to the defaults").clicked() {
                    let (effects, db) = sh.effects_mut();
                    for spec in specs.iter().filter(|s| s.default.is_some()) {
                        effects.set_value(&effect.id, &spec.key, spec.default.clone().unwrap_or_default());
                    }
                    effects.save(&effect.id, db);
                }
            }

            if let Some(depth) = frame.and_then(|f| f.depth.as_ref()) {
                ui.separator();
                ui.label(format!("Depth: {}", depth.status().describe()));
            }
            if let Some(a) = frame.and_then(|f| f.audio.as_deref()) {
                ui.separator();
                ui.label("Sound");
                for (name, v) in [("level", a.level), ("bass", a.bass), ("mid", a.mid), ("treble", a.treble), ("beat", a.beat)] {
                    let height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
                    ui.add(egui::ProgressBar::new(v.clamp(0.0, 1.0)).desired_height(height).text(format!("{name} {v:.2}")));
                }
            }

            ui.separator();
            ui.label("Picture (as in Settings → Picture)");
            let upscale = picture.upscale.unwrap_or(Upscale::Off);
            let mut chosen = upscale;
            ui.horizontal(|ui| {
                ui.label("Anime4K");
                egui::ComboBox::from_id_salt("anime4k").selected_text(upscale.label()).show_ui(ui, |ui| {
                    for u in Upscale::ALL {
                        ui.selectable_value(&mut chosen, u, u.label());
                    }
                });
            });
            if chosen != upscale {
                sh.set_picture_upscale(chosen);
                new_picture = Some(Picture { upscale: (chosen != Upscale::Off).then_some(chosen), ..picture });
            }
            let mut smooth = picture.smooth;
            if ui.checkbox(&mut smooth, "60fps smoothing (SVP)").changed() {
                sh.set_picture_smooth(smooth);
                let base = new_picture.unwrap_or(picture);
                new_picture = Some(Picture { smooth: smooth && sh.picture_smooth(), ..base });
            }
        });
    new_picture
}

/// One setting's label and control. Sliders save when let go.
fn setting(ui: &mut egui::Ui, sh: &mut Shared, id: &str, spec: &SettingSpec) {
    let label = ui.label(&spec.label);
    if !spec.detail.is_empty() {
        label.on_hover_text(&spec.detail);
    }
    let value = sh.effects.value(id, &spec.key).cloned();
    let (effects, db) = sh.effects_mut();
    match spec.kind {
        SettingKind::Number => {
            let (min, max) = (spec.min.unwrap_or(0.0), spec.max.unwrap_or(100.0));
            let mut v = value.as_ref().and_then(serde_json::Value::as_f64).unwrap_or(min);
            let mut slider = egui::Slider::new(&mut v, min..=max).suffix(&spec.unit);
            if let Some(step) = spec.step.filter(|s| *s > 0.0) {
                slider = slider.step_by(step);
            }
            let r = ui.add(slider);
            if r.changed() {
                let v = if v.fract() == 0.0 { serde_json::json!(v as i64) } else { serde_json::json!(v) };
                effects.set_value(id, &spec.key, v);
            }
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                effects.save(id, db);
            }
        }
        SettingKind::Bool => {
            let mut on = value.as_ref().and_then(serde_json::Value::as_bool).unwrap_or(false);
            if ui.checkbox(&mut on, "").changed() {
                effects.set_setting(id, &spec.key, serde_json::json!(on), db);
            }
        }
        SettingKind::Choice => {
            let current = value.as_ref().and_then(serde_json::Value::as_str).unwrap_or("").to_owned();
            let shown = spec.options.iter().find(|o| o.id == current).map_or(current.clone(), |o| o.label.clone());
            let mut pick = None;
            egui::ComboBox::from_id_salt(("effect-choice", &spec.key)).selected_text(shown).show_ui(ui, |ui| {
                for o in &spec.options {
                    if ui.selectable_label(o.id == current, &o.label).clicked() {
                        pick = Some(o.id.clone());
                    }
                }
            });
            if let Some(pick) = pick {
                effects.set_setting(id, &spec.key, serde_json::json!(pick), db);
            }
        }
        SettingKind::Text | SettingKind::Button => {
            ui.label("");
        }
    }
}

/// Uniforms every effect gets.
const HEADER: &str = "\
uniform sampler2D u_video;
uniform sampler2D u_prev;
uniform vec2 u_resolution;
uniform float u_time;
uniform int u_frame;
uniform vec4 u_video_rect;
uniform float u_video_time;
uniform float u_level;
uniform float u_bass;
uniform float u_mid;
uniform float u_treble;
uniform float u_beat;
uniform sampler2D u_spectrum;
uniform sampler2D u_depth;
uniform float u_has_depth;
// Depth at a point on screen, from 0 (far) to 1 (near); 0 until there is some.
float depth_at(vec2 uv) {
    vec2 p = (uv - u_video_rect.xy) / max(u_video_rect.zw - u_video_rect.xy, vec2(1e-6));
    return texture(u_depth, p).r * u_has_depth;
}
";

/// The effect's fragment shader: Flex's header, the plugin's file (which defines
/// `vec4 effect(vec2 uv)`) and a `main` calling it. `#line 1` keeps error line numbers
/// matching the plugin's file.
pub fn fragment_source(version: &str, embedded: bool, uniforms: &[String], body: &str) -> String {
    let precision = if embedded { "precision highp float;\nprecision highp int;\n" } else { "" };
    let settings: String = uniforms.iter().map(|u| format!("uniform float {u};\n")).collect();
    format!(
        "{version}\n{precision}{HEADER}{settings}in vec2 v_uv;\nout vec4 flex_color;\n#line 1\n{body}\n\
         void main() {{ flex_color = effect(v_uv); }}\n"
    )
}

fn vertex_source(version: &str) -> String {
    // One triangle covering the screen, with no vertex buffer.
    format!(
        "{version}\nout vec2 v_uv;\nvoid main() {{\n    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));\n\
         v_uv = p;\n    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);\n}}\n"
    )
}

fn copy_source(version: &str, embedded: bool) -> String {
    let precision = if embedded { "precision highp float;\n" } else { "" };
    format!(
        "{version}\n{precision}uniform sampler2D u_tex;\nuniform float u_flip;\nin vec2 v_uv;\nout vec4 flex_color;\n\
         void main() {{\n    vec2 uv = vec2(v_uv.x, u_flip > 0.5 ? 1.0 - v_uv.y : v_uv.y);\n\
         flex_color = vec4(texture(u_tex, uv).rgb, 1.0);\n}}\n"
    )
}

/// The first line of a compiler log, for the status line (the whole log goes to the log).
fn first_line(log: &str) -> &str {
    log.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("unknown error")
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Renderer {
    /// The status line for this effect, once its shader has been tried.
    pub fn status(&self, effect: &ActiveEffect) -> Option<&(bool, String)> {
        self.status.as_ref().filter(|(path, _)| *path == effect.shader).map(|(_, s)| s)
    }

    /// Draws the video through `effect` into `target` (`w`×`h` pixels). `render(fbo, w, h)`
    /// draws mpv's frame (bottom-up rows). `rect` is where the picture is, without black
    /// bars, in 0–1 coordinates from the bottom left. Returns false, having drawn nothing,
    /// when the effect can't run: then the caller draws the video as usual.
    ///
    /// # Safety
    /// Must be called on the GL thread with `gl` current. Leaves GL state (program,
    /// textures, viewport, blending) for egui to reset.
    pub unsafe fn paint(
        &mut self,
        gl: &glow::Context,
        frame: &Frame,
        target: Option<glow::Framebuffer>,
        (w, h): (i32, i32),
        render: impl FnOnce(i32, i32, i32),
    ) -> bool {
        let (effect, rect) = (&frame.effect, frame.rect);
        unsafe {
            if self.unsupported || w <= 0 || h <= 0 {
                return false;
            }
            let version = egui_glow::ShaderVersion::get(gl);
            if !version.is_new_shader_interface() {
                log::warn!("video effects need OpenGL 3 or OpenGL ES 3");
                self.unsupported = true;
                self.status = Some((effect.shader.clone(), (false, "Needs OpenGL 3".into())));
                return false;
            }
            if self.vao.is_none() {
                match self.create_shared(gl, version) {
                    Ok(()) => {}
                    Err(e) => {
                        log::warn!("video effects: {e}");
                        self.unsupported = true;
                        return false;
                    }
                }
            }
            self.reload(gl, effect, version);
            let (Some(program), Some(copy), Some(vao)) = (self.program, self.copy, self.vao) else { return false };

            let scale = effect.scale.clamp(0.25, 1.0);
            let size = (((w as f32 * scale).round() as i32).max(1), ((h as f32 * scale).round() as i32).max(1));
            if self.objects.as_ref().is_none_or(|o| o.size != size) {
                self.free_objects(gl);
                match create_objects(gl, size) {
                    Ok(o) => self.objects = Some(o),
                    Err(e) => {
                        log::warn!("video effects: {e}");
                        self.status = Some((effect.shader.clone(), (false, format!("Can't run: {e}"))));
                        return false;
                    }
                }
            }
            let Some(o) = &self.objects else { return false };

            render(o.video.fbo.0.get() as i32, size.0, size.1);
            let depth_texture = match &frame.depth {
                Some(link) => self.depth.update(gl, link, o.video.fbo, size, rect, frame.video_time),
                None => None,
            };

            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(Some(vao));
            let (out, prev) = (&o.out[self.current], &o.out[1 - self.current]);

            // The effect, at the working size.
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(out.fbo));
            gl.viewport(0, 0, size.0, size.1);
            gl.use_program(Some(program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(o.video.texture));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(prev.texture));
            let at = |name: &str| gl.get_uniform_location(program, name);
            gl.uniform_1_i32(at("u_video").as_ref(), 0);
            gl.uniform_1_i32(at("u_prev").as_ref(), 1);
            gl.uniform_2_f32(at("u_resolution").as_ref(), size.0 as f32, size.1 as f32);
            let started = *self.started.get_or_insert_with(Instant::now);
            gl.uniform_1_f32(at("u_time").as_ref(), started.elapsed().as_secs_f32());
            gl.uniform_1_i32(at("u_frame").as_ref(), self.frame);
            gl.uniform_4_f32(at("u_video_rect").as_ref(), rect[0], rect[1], rect[2], rect[3]);
            gl.uniform_1_f32(at("u_video_time").as_ref(), frame.video_time);
            let a = frame.audio.as_deref();
            let level = |f: fn(&Features) -> f32| a.map_or(0.0, f);
            gl.uniform_1_f32(at("u_level").as_ref(), level(|a| a.level));
            gl.uniform_1_f32(at("u_bass").as_ref(), level(|a| a.bass));
            gl.uniform_1_f32(at("u_mid").as_ref(), level(|a| a.mid));
            gl.uniform_1_f32(at("u_treble").as_ref(), level(|a| a.treble));
            gl.uniform_1_f32(at("u_beat").as_ref(), level(|a| a.beat));
            if let Some(spectrum) = self.spectrum {
                gl.active_texture(glow::TEXTURE2);
                gl.bind_texture(glow::TEXTURE_2D, Some(spectrum));
                if let Some(a) = a {
                    let bytes: &[u8] = std::slice::from_raw_parts(a.spectrum.as_ptr().cast(), a.spectrum.len() * 4);
                    gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
                    let (w, h) = (audio::BANDS as i32, audio::HISTORY as i32);
                    gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, w, h, glow::RED, glow::FLOAT, glow::PixelUnpackData::Slice(Some(bytes)));
                }
                gl.uniform_1_i32(at("u_spectrum").as_ref(), 2);
            }
            gl.active_texture(glow::TEXTURE3);
            gl.bind_texture(glow::TEXTURE_2D, depth_texture);
            gl.uniform_1_i32(at("u_depth").as_ref(), 3);
            gl.uniform_1_f32(at("u_has_depth").as_ref(), if depth_texture.is_some() && self.depth.serial > 0 { 1.0 } else { 0.0 });
            for (name, value) in &effect.uniforms {
                gl.uniform_1_f32(at(name).as_ref(), *value);
            }
            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            // Onto the screen, scaled up if the effect works smaller.
            gl.bind_framebuffer(glow::FRAMEBUFFER, target);
            gl.viewport(0, 0, w, h);
            gl.use_program(Some(copy));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(out.texture));
            gl.uniform_1_i32(gl.get_uniform_location(copy, "u_tex").as_ref(), 0);
            // The window's rows run bottom-up like ours; an offscreen target's top-down.
            let flip = if target.is_some() { 1.0 } else { 0.0 };
            gl.uniform_1_f32(gl.get_uniform_location(copy, "u_flip").as_ref(), flip);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            gl.bind_texture(glow::TEXTURE_2D, None);
            for unit in [glow::TEXTURE2, glow::TEXTURE3] {
                gl.active_texture(unit);
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_vertex_array(None);
            gl.use_program(None);
            if effect.feedback {
                self.current = 1 - self.current;
            }
            self.frame = self.frame.wrapping_add(1);
            true
        }
    }

    /// Frees everything (no effect is on).
    ///
    /// # Safety
    /// GL thread, `gl` current.
    pub unsafe fn release(&mut self, gl: &glow::Context) {
        unsafe {
            self.free_objects(gl);
            if let Some(p) = self.program.take() {
                gl.delete_program(p);
            }
            if let Some(p) = self.copy.take() {
                gl.delete_program(p);
            }
            if let Some(t) = self.spectrum.take() {
                gl.delete_texture(t);
            }
            self.depth.release(gl);
            if let Some(vao) = self.vao.take() {
                gl.delete_vertex_array(vao);
            }
            self.source = None;
            self.checked = None;
            self.started = None;
        }
    }

    pub fn holds_gl(&self) -> bool {
        self.vao.is_some() || self.objects.is_some()
    }

    unsafe fn create_shared(&mut self, gl: &glow::Context, version: egui_glow::ShaderVersion) -> Result<(), String> {
        unsafe {
            let decl = version.version_declaration();
            let copy = compile(gl, &vertex_source(decl), &copy_source(decl, version.is_embedded()))?;
            match gl.create_vertex_array() {
                Ok(vao) => {
                    self.vao = Some(vao);
                    self.copy = Some(copy);
                    self.spectrum = create_spectrum(gl).map_err(|e| log::warn!("video effects: no spectrum texture: {e}")).ok();
                    Ok(())
                }
                Err(e) => {
                    gl.delete_program(copy);
                    Err(e)
                }
            }
        }
    }

    /// Compiles the effect when it's new or its file changed.
    unsafe fn reload(&mut self, gl: &glow::Context, effect: &ActiveEffect, version: egui_glow::ShaderVersion) {
        unsafe {
            let same_file = self.source.as_ref().is_some_and(|s| s.path == effect.shader);
            if same_file && self.checked.is_some_and(|t| t.elapsed() < RECHECK) {
                return;
            }
            self.checked = Some(Instant::now());
            let uniforms: Vec<String> = effect.uniforms.iter().map(|(n, _)| n.clone()).collect();
            let source = Source { path: effect.shader.clone(), modified: modified(&effect.shader), uniforms };
            if self.source.as_ref() == Some(&source) {
                return;
            }
            if !same_file {
                // A different effect: start afresh.
                self.started = None;
                self.frame = 0;
                self.free_objects(gl);
            }
            let status = match std::fs::read_to_string(&source.path) {
                Err(e) => {
                    self.delete_program(gl);
                    let name = source.path.file_name().unwrap_or_default().to_string_lossy();
                    (false, format!("Can't read {name}: {e}"))
                }
                Ok(body) => {
                    let decl = version.version_declaration();
                    let fragment = fragment_source(decl, version.is_embedded(), &source.uniforms, &body);
                    match compile(gl, &vertex_source(decl), &fragment) {
                        Ok(program) => {
                            self.delete_program(gl);
                            self.program = Some(program);
                            log::info!("video effect {}: loaded {}", effect.name, source.path.display());
                            (true, "Running".to_owned())
                        }
                        Err(e) => {
                            // Keep showing the plain video until it's fixed.
                            self.delete_program(gl);
                            log::warn!("video effect {}: {}:\n{e}", effect.name, source.path.display());
                            (false, format!("Shader error: {}", first_line(&e)))
                        }
                    }
                }
            };
            self.status = Some((source.path.clone(), status));
            self.source = Some(source);
        }
    }

    unsafe fn delete_program(&mut self, gl: &glow::Context) {
        if let Some(p) = self.program.take() {
            unsafe { gl.delete_program(p) };
        }
    }

    unsafe fn free_objects(&mut self, gl: &glow::Context) {
        if let Some(o) = self.objects.take() {
            for t in [o.video].into_iter().chain(o.out) {
                unsafe {
                    gl.delete_framebuffer(t.fbo);
                    gl.delete_texture(t.texture);
                }
            }
        }
    }
}

unsafe fn compile(gl: &glow::Context, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        let mut error = None;
        for (kind, source) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                error.get_or_insert_with(|| gl.get_shader_info_log(shader));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        if error.is_none() {
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                error = Some(gl.get_program_info_log(program));
            }
        }
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        match error {
            None => Ok(program),
            Some(e) => {
                gl.delete_program(program);
                Err(e)
            }
        }
    }
}

unsafe fn create_target(gl: &glow::Context, (w, h): (i32, i32), formats: &[(u32, u32)]) -> Result<Target, String> {
    unsafe {
        for &(internal, kind) in formats {
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            let none = glow::PixelUnpackData::Slice(None);
            gl.tex_image_2d(glow::TEXTURE_2D, 0, internal as i32, w, h, 0, glow::RGBA, kind, none);
            for (param, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            let fbo = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE {
                gl.disable(glow::SCISSOR_TEST);
                gl.clear_color(0.0, 0.0, 0.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                return Ok(Target { texture, fbo });
            }
            gl.delete_framebuffer(fbo);
            gl.delete_texture(texture);
        }
        Err("framebuffers incomplete".into())
    }
}

/// A GL fence, only touched on the GL thread (the renderer sits in a Mutex shared with the
/// UI thread, which only reads its status).
#[derive(Clone, Copy)]
struct Fence(glow::Fence);

unsafe impl Send for Fence {}

struct DepthObjects {
    /// The picture shrunk to the model's size, and a buffer to read it back through.
    capture: Target,
    pbo: glow::Buffer,
    /// `u_depth`.
    texture: glow::Texture,
}

/// Small frames read back for the depth worker (a frame late, so the CPU never waits for
/// the GPU), and the depth maps it sends back as a texture.
#[derive(Default)]
struct DepthGl {
    gl: Option<DepthObjects>,
    failed: bool,
    pending: Option<Fence>,
    /// The video time of the last frame sent, so a paused frame isn't sent again and again.
    last_time: Option<f32>,
    /// The depth map in the texture.
    serial: u64,
}

impl DepthGl {
    /// Picks up a frame read back last time and hands it over, uploads a new depth map, and
    /// reads back this frame if the worker's free. Returns the depth texture.
    unsafe fn update(&mut self, gl: &glow::Context, link: &crate::depth::Link, video: glow::Framebuffer, size: (i32, i32), rect: [f32; 4], time: f32) -> Option<glow::Texture> {
        use crate::depth::{HEIGHT, WIDTH};
        unsafe {
            if self.failed {
                return None;
            }
            if self.gl.is_none() {
                match create_depth(gl) {
                    Ok(o) => self.gl = Some(o),
                    Err(e) => {
                        log::warn!("depth: {e}");
                        self.failed = true;
                        return None;
                    }
                }
            }
            let o = self.gl.as_ref()?;
            let bytes = WIDTH * HEIGHT * 4;
            if let Some(Fence(fence)) = self.pending {
                match gl.client_wait_sync(fence, 0, 0) {
                    glow::TIMEOUT_EXPIRED => {}
                    status => {
                        gl.delete_sync(fence);
                        self.pending = None;
                        if status != glow::WAIT_FAILED {
                            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(o.pbo));
                            let ptr = gl.map_buffer_range(glow::PIXEL_PACK_BUFFER, 0, bytes as i32, glow::MAP_READ_BIT);
                            if !ptr.is_null() {
                                link.submit(std::slice::from_raw_parts(ptr, bytes).to_vec());
                                gl.unmap_buffer(glow::PIXEL_PACK_BUFFER);
                            }
                            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                        }
                    }
                }
            }
            if let Some(map) = link.latest().filter(|m| m.serial != self.serial) {
                gl.bind_texture(glow::TEXTURE_2D, Some(o.texture));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
                let data: &[u8] = std::slice::from_raw_parts(map.values.as_ptr().cast(), map.values.len() * 4);
                let (w, h) = (WIDTH as i32, HEIGHT as i32);
                gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, w, h, glow::RED, glow::FLOAT, glow::PixelUnpackData::Slice(Some(data)));
                gl.bind_texture(glow::TEXTURE_2D, None);
                self.serial = map.serial;
            }
            if self.pending.is_none() && link.ready_for_frame() && self.last_time != Some(time) {
                self.last_time = Some(time);
                // Just the picture, without black bars.
                let (x0, y0) = ((rect[0] * size.0 as f32) as i32, (rect[1] * size.1 as f32) as i32);
                let (x1, y1) = ((rect[2] * size.0 as f32) as i32, (rect[3] * size.1 as f32) as i32);
                gl.disable(glow::SCISSOR_TEST);
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(video));
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(o.capture.fbo));
                gl.blit_framebuffer(x0, y0, x1.max(x0 + 1), y1.max(y0 + 1), 0, 0, WIDTH as i32, HEIGHT as i32, glow::COLOR_BUFFER_BIT, glow::LINEAR);
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(o.capture.fbo));
                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(o.pbo));
                gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
                gl.read_pixels(0, 0, WIDTH as i32, HEIGHT as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::BufferOffset(0));
                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                if let Ok(fence) = gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
                    self.pending = Some(Fence(fence));
                }
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            }
            Some(o.texture)
        }
    }

    unsafe fn release(&mut self, gl: &glow::Context) {
        unsafe {
            if let Some(Fence(fence)) = self.pending.take() {
                gl.delete_sync(fence);
            }
            if let Some(o) = self.gl.take() {
                gl.delete_framebuffer(o.capture.fbo);
                gl.delete_texture(o.capture.texture);
                gl.delete_buffer(o.pbo);
                gl.delete_texture(o.texture);
            }
            self.serial = 0;
            self.last_time = None;
        }
    }
}

unsafe fn create_depth(gl: &glow::Context) -> Result<DepthObjects, String> {
    use crate::depth::{HEIGHT, WIDTH};
    unsafe {
        let size = (WIDTH as i32, HEIGHT as i32);
        let capture = create_target(gl, size, &[(glow::RGBA8, glow::UNSIGNED_BYTE)])?;
        let pbo = gl.create_buffer()?;
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(pbo));
        gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, (WIDTH * HEIGHT * 4) as i32, glow::STREAM_READ);
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        let zeros = vec![0u8; WIDTH * HEIGHT * 4];
        gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::R16F as i32, size.0, size.1, 0, glow::RED, glow::FLOAT, glow::PixelUnpackData::Slice(Some(&zeros)));
        for (param, value) in [
            (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
            (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        Ok(DepthObjects { capture, pbo, texture })
    }
}

/// The spectrum: one float per band and history row, starting silent.
unsafe fn create_spectrum(gl: &glow::Context) -> Result<glow::Texture, String> {
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        let zeros = vec![0u8; audio::BANDS * audio::HISTORY * 4];
        let data = glow::PixelUnpackData::Slice(Some(&zeros));
        // Half floats: filterable everywhere (plain floats aren't on GLES).
        gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::R16F as i32, audio::BANDS as i32, audio::HISTORY as i32, 0, glow::RED, glow::FLOAT, data);
        for (param, value) in [
            (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
            (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        Ok(texture)
    }
}

unsafe fn create_objects(gl: &glow::Context, size: (i32, i32)) -> Result<Objects, String> {
    unsafe {
        let rgba8 = (glow::RGBA8, glow::UNSIGNED_BYTE);
        // Feedback fades over many frames: 8 bits a channel would leave trails stuck at
        // faint colours, so half floats where the GPU can draw into them.
        let float = [(glow::RGBA16F, glow::HALF_FLOAT), rgba8];
        let video = create_target(gl, size, &[rgba8])?;
        let a = create_target(gl, size, &float);
        let b = create_target(gl, size, &float);
        match (a, b) {
            (Ok(a), Ok(b)) => Ok(Objects { size, video, out: [a, b] }),
            (a, b) => {
                for t in [Ok(video), a, b].into_iter().flatten() {
                    gl.delete_framebuffer(t.fbo);
                    gl.delete_texture(t.texture);
                }
                Err("framebuffers incomplete".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_source_declares_settings_and_keeps_line_numbers() {
        let body = "vec4 effect(vec2 uv) {\n    return texture(u_video, uv) * s_gain;\n}";
        let src = fragment_source("#version 330", false, &["s_gain".into()], body);
        assert!(src.starts_with("#version 330\nuniform sampler2D u_video;"));
        assert!(src.contains("uniform float s_gain;\n"));
        let after = src.split("#line 1\n").nth(1).unwrap();
        assert!(after.starts_with("vec4 effect"), "the plugin's file starts at line 1");
        assert!(src.trim_end().ends_with("void main() { flex_color = effect(v_uv); }"));
        let es = fragment_source("#version 300 es", true, &[], body);
        assert!(es.starts_with("#version 300 es\nprecision highp float;"));
    }

    #[test]
    fn status_takes_the_first_line_of_the_log() {
        assert_eq!(first_line("\n  0:3(12): error: syntax error\n0:4: more\n"), "0:3(12): error: syntax error");
        assert_eq!(first_line(""), "unknown error");
    }
}
