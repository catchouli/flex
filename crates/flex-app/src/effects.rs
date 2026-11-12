//! Video effects (`flex_core::effects`): a fragment shader run over every frame of the
//! video, entirely on the GPU. Does nothing unless an effect is chosen.
//!
//! With an effect on, mpv draws into our own texture instead of the screen; the plugin's
//! shader then draws from it (and, with `feedback`, from its own previous output) into
//! another texture, which is copied to the screen. The shader file is checked for changes
//! twice a second and reloaded, so it can be edited while a video plays. During a shuffle's
//! fade two effects run at once from the same video, and their outputs are mixed.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui;
use eframe::egui_glow;
use eframe::glow::{self, HasContext};
use flex_core::audio::{self, Analyser, Features, Levels};
use flex_core::effects::ActiveEffect;
use flex_core::plugins::{SettingKind, SettingSpec};
use flex_plugin::serde_json;

use crate::mpv::Mpv;
use crate::picture::{Picture, Upscale};
use crate::shared::Shared;
use crate::signals::{Cuts, FlowGl, HistoryGl, HistorySpot, PlateGl, Stats, delete_target};

/// How often the shader file is checked for changes.
const RECHECK: Duration = Duration::from_millis(500);

pub(crate) struct Target {
    pub texture: glow::Texture,
    pub fbo: glow::Framebuffer,
}

struct Objects {
    /// The size of the whole window, and of the picture in it, as effects work on them.
    window: (i32, i32),
    size: (i32, i32),
    /// What mpv draws, and the picture cut out of that.
    video: Target,
    cropped: Target,
}

/// What's compiled: the file, when it was last changed, and the setting uniforms (which
/// are declared in the shader's header).
#[derive(PartialEq)]
struct Source {
    path: PathBuf,
    modified: Option<SystemTime>,
    uniforms: Vec<String>,
}

/// One effect being drawn.
#[derive(Default)]
struct Slot {
    /// The effect's program; `None` if it didn't compile.
    program: Option<glow::Program>,
    source: Option<Source>,
    checked: Option<Instant>,
    started: Option<Instant>,
    frame: i32,
    size: (i32, i32),
    /// The effect's output, this frame's and last frame's (swapped each frame).
    out: Option<[Target; 2]>,
    /// Which of `out` is drawn this frame.
    current: usize,
    /// Which of `out` was drawn last.
    drawn: usize,
    /// The shader's state for the plugin's status line, and which file it's about.
    status: Option<(PathBuf, (bool, String))>,
    /// Whether the effect fades itself with `u_amount`; if not, it's mixed with the plain
    /// picture instead.
    fades: bool,
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
    held: HeldFrames,
    /// The subtitles for each of those frames, and the ones mpv's just drawn.
    held_words: HeldFrames,
    bars: crate::bars::BarGl,
    words: Option<(Target, (i32, i32))>,
    faces: FaceGl,
    motion: NetFlowGl,
    camera: CameraGl,
    restyle: RestyleGl,
    nca: crate::nca::NcaGl,
    shots: crate::shot_type::Tracker,
    cuts: CutGl,
    history: HistoryGl,
    flow: FlowGl,
    plate: PlateGl,
    stats: Stats,
    last_paint: Option<Instant>,
    copy: Option<glow::Program>,
    slots: [Slot; 2],
    /// Which of `slots` has the chosen effect; the other has the one fading out, if any.
    front: usize,
    /// No GL 3 (or GLES 3): effects can't run at all.
    unsupported: bool,
}

/// What an effect is drawn with this frame, worked out on the UI thread.
#[derive(Clone)]
pub struct Frame {
    pub effect: ActiveEffect,
    /// The effect fading out and how much of it there still is (its `u_amount`), how far
    /// `effect` has faded in over it (0–1), and how much of `effect` there is (see
    /// `flex_core::effects::Fade`).
    pub fading: Option<(ActiveEffect, f32)>,
    pub faded_in: f32,
    pub amount: f32,
    /// The size mpv draws at, relative to the screen.
    pub video_scale: f32,
    /// Shuffle or an effect wants to know where the shot changes.
    pub find_cuts: bool,
    /// What the effects on or coming up want worked out.
    pub needs: flex_core::effects::Needs,
    /// Where the picture is, without black bars (0–1 from the bottom left).
    pub rect: [f32; 4],
    pub video_time: f32,
    /// mpv's position: unlike `video_time`, the same for all of one video frame.
    pub pts: f32,
    /// The sound, when the effect asked for it and there is some.
    pub audio: Option<Arc<Features>>,
    /// The depth worker, when the effect asked for depth.
    pub depth: Option<crate::depth::Link>,
    /// The face worker, when the effect asked for faces.
    pub faces: Option<crate::faces::Link>,
    /// The movement worker, when the effect asked for movement.
    pub motion: Option<crate::flow::Link>,
    /// The camera worker, when the effect asked where the camera is.
    pub camera: Option<crate::camera::Link>,
    /// The restyling worker, when the effect draws on a restyled picture.
    pub restyle: Option<crate::restyle::Link>,
    /// Or the living texture to grow, when its style is one of those.
    pub texture: Option<crate::nca::Link>,
    /// Subtitles are to be kept out of the effect and laid on top of it.
    pub subtitles: bool,
}

/// The player's side: a smooth playback clock, and the sound for effects that want it.
#[derive(Default)]
pub struct Inputs {
    /// Video time, moving smoothly between mpv's position updates (which only change
    /// once per video frame).
    clock: f64,
    last: Option<Instant>,
    /// The channels and layout mpv's audio filter was added for.
    filter: Option<(usize, String)>,
    /// The filter's last report as mpv gave it, to spot new ones.
    metadata: Option<String>,
    /// Reports waiting for their sound to be heard, and the latest one that has been.
    waiting: VecDeque<(Instant, Levels)>,
    levels: Option<Levels>,
    analyser: Option<Analyser>,
    /// Whether subtitles are showing and there's a patched libmpv to take them apart.
    subtitles: bool,
    /// With a patched libmpv: the samples being analysed, and what they gave, waiting to be heard.
    tapped: bool,
    samples: audio::SampleBands,
    heard: VecDeque<(f64, Levels)>,
    /// The computer's own sound, recorded back: what effects hear when it can be had.
    system: Option<crate::system_sound::SystemSound>,
    system_samples: audio::SampleBands,
    /// When the recording last brought any sound (some outputs send nothing during silence).
    system_heard: Option<Instant>,
    /// What kind of sound it is: told the same sound, and eased here.
    sound_kind: Option<crate::sound_kind::SoundKind>,
    kinds: [f32; flex_depth::sound::KINDS.len()],
}

impl Inputs {
    /// Whether there are subtitles for effects to leave out and lay on top afterwards.
    pub fn subtitles(&self) -> bool {
        self.subtitles
    }

    /// Takes in the samples mpv's played lately, with Flex's patched libmpv.
    fn collect_samples(&mut self, mpv: &Mpv) {
        if self.filter.is_none() {
            let _ = crate::audio_tap::take();
            if let Err(e) = mpv.command(&["af", "add", &format!("@{}:flextap", audio::FILTER_LABEL)]) {
                log::warn!("video effects: no sound for effects: {e:#}");
            }
            self.filter = Some((0, "tap".into()));
        }
        for chunk in crate::audio_tap::take() {
            if let Some(kind) = &self.sound_kind {
                kind.push(chunk.rate, &chunk.mono);
            }
            self.samples.push(chunk.pts, chunk.rate, &chunk.mono);
        }
        while let Some(heard) = self.samples.measure() {
            self.heard.push_back(heard);
        }
        let Some(now) = mpv.get_f64("audio-pts") else { return };
        let mut latest: Option<Levels> = None;
        while self.heard.front().is_some_and(|(pts, _)| *pts <= now) {
            let Some((_, levels)) = self.heard.pop_front() else { break };
            // Between frames there are several readings: the loudest of each keeps the beats.
            latest = Some(match latest {
                Some(l) => l.loudest_of(&levels),
                None => levels,
            });
        }
        if latest.is_some() {
            self.levels = latest;
        }
    }

    /// Takes in the computer's sound, recorded since the last frame. It's already being heard,
    /// so it's used as it comes.
    fn collect_system(&mut self, now: Instant, dt: f64) {
        let Some(system) = &self.system else { return };
        let chunks = system.take();
        if chunks.is_empty() {
            // Nothing comes while nothing plays: that's silence.
            if system.recording() && self.system_heard.is_none_or(|t| now.duration_since(t) > Duration::from_millis(150)) {
                let silence = vec![0.0; (dt.min(0.1) * 48_000.0) as usize];
                if let Some(kind) = &self.sound_kind {
                    kind.push(48_000, &silence);
                }
                self.system_samples.push(-1.0, 48_000, &silence);
            }
        } else {
            self.system_heard = Some(now);
        }
        for chunk in chunks {
            if let Some(kind) = &self.sound_kind {
                kind.push(chunk.rate, &chunk.mono);
            }
            self.system_samples.push(-1.0, chunk.rate, &chunk.mono);
        }
        let mut latest: Option<Levels> = None;
        while let Some((_, levels)) = self.system_samples.measure() {
            latest = Some(match latest {
                Some(l) => l.loudest_of(&levels),
                None => levels,
            });
        }
        if latest.is_some() {
            self.levels = latest;
        }
    }

    /// Adds mpv's audio filter, or replaces it if the sound's channels changed.
    fn keep_filter(&mut self, mpv: &Mpv) {
        let (Some(channels), Some(layout)) = (mpv.get_i64("audio-params/channel-count"), mpv.get_string("audio-params/channels")) else { return };
        let wanted = (channels.max(1) as usize, layout);
        if self.filter.as_ref() == Some(&wanted) {
            return;
        }
        self.stop(mpv);
        let filter = format!("@{}:lavfi=[{}]", audio::FILTER_LABEL, audio::filter_graph(wanted.0, &wanted.1));
        if let Err(e) = mpv.command(&["af", "add", &filter]) {
            log::warn!("video effects: no sound for effects: {e:#}");
        }
        self.filter = Some(wanted);
    }

    /// Takes mpv's audio filter out, when nothing wants the sound any more.
    pub fn stop(&mut self, mpv: &Mpv) {
        if self.filter.take().is_some() {
            let _ = mpv.command(&["af", "remove", &format!("@{}", audio::FILTER_LABEL)]);
        }
        self.metadata = None;
        self.waiting.clear();
        self.levels = None;
        self.samples = Default::default();
        self.heard.clear();
        self.system = None;
        self.system_samples = Default::default();
        self.system_heard = None;
        self.sound_kind = None;
        self.kinds = Default::default();
    }

    /// Call once per frame while an effect is on.
    pub fn tick(&mut self, mpv: &Mpv, wants_audio: bool) -> (f32, Option<Arc<Features>>) {
        let now = Instant::now();
        let dt = self.last.map_or(0.0, |t| now.duration_since(t).as_secs_f64());
        self.last = Some(now);
        self.tapped = mpv.set_audio_tap(crate::audio_tap::receive);
        self.subtitles = self.tapped && mpv.get_i64("current-tracks/sub/id").is_some() && mpv.get_flag("sub-visibility").unwrap_or(false);
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

        if !wants_audio {
            self.stop(mpv);
            self.analyser = None;
            return (self.clock as f32, None);
        }
        // The computer's sound where it can be recorded, so effects follow whatever plays;
        // otherwise mpv's, from Flex's patched libmpv or from a filter inside mpv.
        let system = self.system.get_or_insert_with(crate::system_sound::SystemSound::start);
        let from_mpv = system.failed();
        self.sound_kind.get_or_insert_with(crate::sound_kind::SoundKind::start);
        if !from_mpv {
            let _ = crate::audio_tap::take();
            self.collect_system(now, dt);
        } else if self.tapped {
            self.collect_samples(mpv);
        } else {
            self.keep_filter(mpv);
        }
        if let Some((channels, _)) = self.filter.as_ref().filter(|_| from_mpv && !self.tapped).cloned() {
            let metadata = mpv.get_string(&format!("af-metadata/{}", audio::FILTER_LABEL));
            if metadata.is_some() && metadata != self.metadata {
                if let Some(levels) = metadata.as_deref().and_then(|m| audio::read_levels(m, channels)) {
                    self.waiting.push_back((now, levels));
                }
                self.metadata = metadata;
            }
        }
        while self.waiting.front().is_some_and(|(at, _)| now.duration_since(*at) >= HEARD_AFTER) {
            self.levels = self.waiting.pop_front().map(|(_, levels)| levels);
        }
        let analyser = self.analyser.get_or_insert_with(Analyser::new);
        let mut features = analyser.update(self.levels.as_ref(), self.clock, dt as f32).clone();
        // It's heard every half second: eased so effects don't jump, quicker in than out.
        let heard = self.sound_kind.as_ref().map(|k| k.heard()).unwrap_or_default();
        for (k, target) in self.kinds.iter_mut().zip(heard.kinds) {
            let rate = if target > *k { 6.0 } else { 2.0 };
            *k += (target - *k) * (1.0 - (-dt as f32 * rate).exp());
        }
        features.kinds = self.kinds.to_vec();
        features.heard = heard.described;
        (self.clock as f32, Some(Arc::new(features)))
    }
}

/// How long after mpv's audio filter sees some sound it comes out of the speakers.
const HEARD_AFTER: Duration = Duration::from_millis(400);

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
            if let Some(faces) = frame.and_then(|f| f.faces.as_ref()) {
                ui.label(format!("Faces: {}", faces.status().describe()));
            }
            if let Some(restyle) = frame.and_then(|f| f.restyle.as_ref()) {
                ui.label(format!("Style: {}", restyle.status().describe()));
            }
            if let Some(texture) = frame.and_then(|f| f.texture.as_ref()) {
                ui.label(format!("Texture: {}", texture.status().describe()));
            }
            if let Some(a) = frame.and_then(|f| f.audio.as_deref()) {
                ui.separator();
                ui.label("Sound");
                let height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
                for (name, v) in [("level", a.level), ("bass", a.bass), ("mid", a.mid), ("treble", a.treble), ("beat", a.beat)] {
                    ui.add(egui::ProgressBar::new(v.clamp(0.0, 1.0)).desired_height(height).text(format!("{name} {v:.2}")));
                }
                // The kinds it most sounds like, and the classes behind them.
                let mut kinds: Vec<(&str, f32)> = flex_depth::sound::KINDS.iter().map(|k| k.0).zip(a.kinds.iter().copied()).collect();
                kinds.sort_by(|a, b| b.1.total_cmp(&a.1));
                for (name, v) in kinds.into_iter().take(4).filter(|k| k.1 >= 0.05) {
                    ui.add(egui::ProgressBar::new(v.clamp(0.0, 1.0)).desired_height(height).text(format!("{name} {v:.2}")));
                }
                if !a.heard.is_empty() {
                    ui.weak(format!("Hears: {}", a.heard));
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

/// `u_sound_<kind>` for each of `sound::KINDS`, as `HEADER` declares them.
static SOUND_UNIFORMS: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| flex_depth::sound::KINDS.iter().map(|(name, _)| format!("u_sound_{name}")).collect());

/// Uniforms every effect gets.
const HEADER: &str = "\
uniform sampler2D u_video;
// With restyle = \"<setting>\": u_video is the picture redrawn in the style picked, once one has
// come (u_has_restyled is 1), and u_original the real one. u_restyled_size is how many pixels
// the network drew it at (it's scaled up to the video's size), or 0.
uniform sampler2D u_original;
uniform float u_has_restyled;
uniform vec2 u_restyled_size;
uniform sampler2D u_prev;
// How much of the effect there is, from 0 (the picture as it is) to 1 (all of it): it's eased in
// and out as shuffle fades through the plain picture. An effect that uses it should look like the
// plain picture at 0 and grow into itself; one that doesn't is mixed with the plain picture.
uniform float u_amount;
uniform vec2 u_resolution;
uniform float u_time;
uniform int u_frame;
uniform vec4 u_video_rect;
uniform float u_video_time;
uniform float u_dt;
uniform float u_level;
uniform float u_bass;
uniform float u_mid;
uniform float u_treble;
uniform float u_beat;
uniform sampler2D u_spectrum;
// What kind of sound is playing (with `audio = true`), 0 to 1, from about the last second.
uniform float u_sound_speech;
uniform float u_sound_shouting;
uniform float u_sound_laughter;
uniform float u_sound_crying;
uniform float u_sound_singing;
uniform float u_sound_music;
uniform float u_sound_crowd;
uniform float u_sound_animals;
uniform float u_sound_vehicles;
uniform float u_sound_explosions;
uniform float u_sound_thunder;
uniform float u_sound_water;
uniform float u_sound_wind;
uniform float u_sound_fire;
uniform float u_sound_alarms;
uniform float u_sound_impacts;
uniform float u_sound_silence;
uniform float u_sound_happy_music;
uniform float u_sound_sad_music;
uniform float u_sound_tender_music;
uniform float u_sound_exciting_music;
uniform float u_sound_angry_music;
uniform float u_sound_scary_music;
uniform sampler2D u_depth;
uniform float u_has_depth;
// The floor or other level surface, found from depth: its depth is x, y, 1 dotted with xyz, over
// the picture from 0 to 1 with y going up. w is how sure that is, from 0 to 1 (0 with none found).
uniform vec4 u_ground;
// Depth at a point on screen, from 0 (far) to 1 (near); 0 until there is some.
float depth_at(vec2 uv) {
    vec2 p = (uv - u_video_rect.xy) / max(u_video_rect.zw - u_video_rect.xy, vec2(1e-6));
    return texture(u_depth, p).r * u_has_depth;
}
// How depth changes across a point on screen, from `reach` pixels either side: x to the right
// and y up, bigger where it comes nearer that way.
vec2 depth_slope(vec2 uv, float reach) {
    vec2 e = reach / u_resolution;
    return vec2(depth_at(uv + vec2(e.x, 0.0)) - depth_at(uv - vec2(e.x, 0.0)), depth_at(uv + vec2(0.0, e.y)) - depth_at(uv - vec2(0.0, e.y)));
}
uniform sampler2D u_masks;
uniform float u_has_masks;
// How much of a point on screen is a person, a wall or sky, each from 0 to 1.
vec4 masks_at(vec2 uv) {
    vec2 p = (uv - u_video_rect.xy) / max(u_video_rect.zw - u_video_rect.xy, vec2(1e-6));
    return texture(u_masks, p) * u_has_masks;
}
// Where the camera is (with camera = true), so things can stay put in the world. u_world_ready fades
// from 0 to 1 about a second and a half into each shot, and is 0 when there's no telling. A shot's
// world is its own, in no particular scale: a typical distance is about 1.
uniform float u_world_ready;
uniform mat3 u_world_rotation;
uniform vec3 u_world_offset;
uniform vec3 u_world_lens;
uniform vec2 u_world_depth;
// The camera's place in the world, and the way through a point on screen, so that the point in
// the world `distance` away (as world_distance gives it) is origin + direction * distance. The
// world's y points down, as it did on screen when the shot began.
void world_ray(vec2 uv, out vec3 origin, out vec3 direction) {
    vec2 p = (uv - u_video_rect.xy) / max(u_video_rect.zw - u_video_rect.xy, vec2(1e-6));
    vec2 size = u_world_lens.yz;
    vec2 pixel = vec2(p.x * size.x - 0.5, (1.0 - p.y) * size.y - 0.5);
    mat3 to_world = transpose(u_world_rotation);
    origin = -(to_world * u_world_offset);
    direction = to_world * vec3((pixel - (size - 1.0) * 0.5) / u_world_lens.x, 1.0);
}
// Where a point on screen would be in the world if depth_at said `near` there.
vec3 world_point(vec2 uv, float near) {
    vec3 origin, direction;
    world_ray(uv, origin, direction);
    return origin + direction / max(u_world_depth.x * near + u_world_depth.y, 0.02);
}
// Where a point on screen is in the world.
vec3 world_at(vec2 uv) {
    return world_point(uv, depth_at(uv));
}
// Where a point in the world is on screen; z is its distance, and negative if it's behind the camera.
vec3 world_to_screen(vec3 world) {
    vec3 c = u_world_rotation * world + u_world_offset;
    vec2 size = u_world_lens.yz;
    vec2 pixel = u_world_lens.x * c.xy / max(c.z, 1e-3) + (size - 1.0) * 0.5;
    vec2 p = vec2((pixel.x + 0.5) / size.x, 1.0 - (pixel.y + 0.5) / size.y);
    return vec3(u_video_rect.xy + p * (u_video_rect.zw - u_video_rect.xy), c.z);
}
// Where a point on screen was when the shot began: a pattern drawn there stays stuck to what's in
// the picture as the camera moves. Just uv when the camera isn't known.
vec2 shot_uv(vec2 uv) {
    if (u_world_ready <= 0.0) return uv;
    vec3 world = world_at(uv);
    vec2 size = u_world_lens.yz;
    vec2 pixel = u_world_lens.x * world.xy / max(world.z, 1e-3) + (size - 1.0) * 0.5;
    vec2 p = vec2((pixel.x + 0.5) / size.x, 1.0 - (pixel.y + 0.5) / size.y);
    return mix(uv, u_video_rect.xy + p * (u_video_rect.zw - u_video_rect.xy), u_world_ready);
}
// Where something hung at uv when the shot began, this far away, is on screen now: a world_pin of
// any screen position stays put in the world. Just uv when the camera isn't known.
vec2 world_pin(vec2 uv, float distance) {
    if (u_world_ready <= 0.0) return uv;
    vec2 p = (uv - u_video_rect.xy) / max(u_video_rect.zw - u_video_rect.xy, vec2(1e-6));
    vec2 size = u_world_lens.yz;
    vec2 pixel = vec2(p.x * size.x - 0.5, (1.0 - p.y) * size.y - 0.5);
    vec3 ray = vec3((pixel - (size - 1.0) * 0.5) / u_world_lens.x, 1.0) * distance;
    return mix(uv, world_to_screen(ray).xy, u_world_ready);
}
// world_pin for something that shouldn't be lost: as it goes out of the picture it comes back
// towards uv, instead of staying where the camera has left it (and it's uv while the camera isn't
// known, as world_pin is).
vec2 world_pin_nearby(vec2 uv, float distance) {
    vec2 pin = world_pin(uv, distance);
    vec2 p = (pin - u_video_rect.xy) / max(u_video_rect.zw - u_video_rect.xy, vec2(1e-6));
    return mix(pin, uv, smoothstep(0.6, 1.0, max(abs(p.x - 0.5), abs(p.y - 0.5))));
}
// How far away something is that depth_at would call `near`.
float world_distance(float near) {
    return 1.0 / max(u_world_depth.x * near + u_world_depth.y, 0.02);
}
// How tall something `size` big in the world looks on screen, `away` from the camera, as a share
// of the screen's height.
float screen_size(float size, float away) {
    return u_world_lens.x * size / max(away, 1e-3) / u_world_lens.z * (u_video_rect.w - u_video_rect.y);
}
uniform sampler2D u_normals;
uniform float u_has_normals;
// Which way the surface at a point on screen faces: x to the right, y up and z towards the viewer
// (unit length). From the depth network where it gives them (MoGe-2); otherwise worked out from
// depth's slopes, which show outlines but little of the shape within them.
vec3 normal_at(vec2 uv) {
    vec2 p = (uv - u_video_rect.xy) / max(u_video_rect.zw - u_video_rect.xy, vec2(1e-6));
    if (u_has_normals > 0.5) {
        return normalize(texture(u_normals, p).xyz * 2.0 - 1.0);
    }
    // Two texels of the depth map either way; any finer and its noise shows as bumps.
    vec2 e = 2.0 / vec2(518.0, 294.0);
    float dx = texture(u_depth, p + vec2(e.x, 0.0)).r - texture(u_depth, p - vec2(e.x, 0.0)).r;
    float dy = texture(u_depth, p + vec2(0.0, e.y)).r - texture(u_depth, p - vec2(0.0, e.y)).r;
    // Limited, or the jump in depth around everything's outline becomes a hard edge.
    vec2 slope = clamp(vec2(dx, dy), -0.035, 0.035) * u_has_depth;
    return normalize(vec3(-slope, 0.06));
}
float person_at(vec2 uv) {
    return masks_at(uv).r;
}
float wall_at(vec2 uv) {
    return masks_at(uv).g;
}
float sky_at(vec2 uv) {
    return masks_at(uv).b;
}
// With segment = true and MoGe-2's head (u_has_more_masks is 1): more of what each point is,
// each from 0 to 1. ground_at is anything underfoot (floors, roads, grass, sand, snow), and
// building_at the outsides of buildings (inside, walls are wall_at). Of a person, body_skin_at,
// clothes_at and hair_at say what they're made of there, and head_at, torso_at, arm_at, hand_at,
// leg_at and foot_at which part of the body it is, whatever it's wearing: each set adds up to
// person_at.
uniform sampler2DArray u_more_masks;
uniform float u_has_more_masks;
// Layer 0: ground, ceiling, water, plant. 1: window, skin, hair, clothes. 2: vehicle, animal,
// building, furniture. 3: head, torso, arm, hand. 4: leg, foot, and (not to be filtered) which
// person, and how near a person's middle.
vec4 more_masks_at(vec2 uv, int layer) {
    vec2 p = (uv - u_video_rect.xy) / max(u_video_rect.zw - u_video_rect.xy, vec2(1e-6));
    return texture(u_more_masks, vec3(p, float(layer))) * u_has_more_masks;
}
float ground_at(vec2 uv) { return more_masks_at(uv, 0).r; }
float ceiling_at(vec2 uv) { return more_masks_at(uv, 0).g; }
float water_at(vec2 uv) { return more_masks_at(uv, 0).b; }
float plant_at(vec2 uv) { return more_masks_at(uv, 0).a; }
float window_at(vec2 uv) { return more_masks_at(uv, 1).r; }
float body_skin_at(vec2 uv) { return more_masks_at(uv, 1).g; }
float clothes_at(vec2 uv) { return more_masks_at(uv, 1).a; }
float vehicle_at(vec2 uv) { return more_masks_at(uv, 2).r; }
float animal_at(vec2 uv) { return more_masks_at(uv, 2).g; }
float building_at(vec2 uv) { return more_masks_at(uv, 2).b; }
float furniture_at(vec2 uv) { return more_masks_at(uv, 2).a; }
float head_at(vec2 uv) { return more_masks_at(uv, 3).r; }
float torso_at(vec2 uv) { return more_masks_at(uv, 3).g; }
float arm_at(vec2 uv) { return more_masks_at(uv, 3).b; }
float hand_at(vec2 uv) { return more_masks_at(uv, 3).a; }
float leg_at(vec2 uv) { return more_masks_at(uv, 4).r; }
float foot_at(vec2 uv) { return more_masks_at(uv, 4).g; }
// Who's who (with segment = true and MoGe-2's head): up to MAX_PEOPLE people, each keeping their
// number while they're in shot (so one can be given their own colour); a number's free when
// u_person_seen is 0. For each: how sure that they're there (1, fading for a moment once
// they're lost), their box (x0, y0, x1, y1 on screen) and middle; and their head, hands and
// feet (picture-left first), each the middle on screen, its radius as a share of the screen's
// height, and how sure that is from 0 to 1 (0 when it can't be seen).
const int MAX_PEOPLE = 8;
uniform int u_person_count;
uniform float u_person_seen[MAX_PEOPLE];
uniform vec4 u_person_box[MAX_PEOPLE];
uniform vec2 u_person_centre[MAX_PEOPLE];
uniform vec4 u_person_head[MAX_PEOPLE];
uniform vec4 u_person_hands[MAX_PEOPLE * 2];
uniform vec4 u_person_feet[MAX_PEOPLE * 2];
// Which person a point on screen belongs to, or -1 for nobody.
int person_there(vec2 uv) {
    if (u_has_more_masks < 0.5) return -1;
    vec2 p = (uv - u_video_rect.xy) / max(u_video_rect.zw - u_video_rect.xy, vec2(1e-6));
    ivec3 size = textureSize(u_more_masks, 0);
    ivec2 at = clamp(ivec2(p * vec2(size.xy)), ivec2(0), size.xy - 1);
    int who = int(texelFetch(u_more_masks, ivec3(at, 4), 0).b * 255.0 + 0.5) - 1;
    return who < MAX_PEOPLE ? who : -1;
}
// How near a person's middle a point is, 1 at the middle (a soft bump about a twentieth of the
// picture's height across).
float person_middle_at(vec2 uv) { return more_masks_at(uv, 4).a; }
const int MAX_FACES = 8;
uniform int u_face_count;
// Each face: x0, y0, x1, y1 on screen. Its five points: right eye, left eye, nose, right and
// left corners of the mouth. u_face_seen is 1 while a face is found and 0 once it's lost, with
// a moment's blend as one is found.
uniform vec4 u_faces[MAX_FACES];
uniform vec2 u_face_points[MAX_FACES * 5];
uniform float u_face_seen[MAX_FACES];
// With mesh = true. The direction a head points: x to the right of the picture, y up, z towards
// the viewer, and w 1 if it's known. How open the person's right eye, left eye and mouth are,
// from 0 to 1. How much of a mesh a face has (0 for faces side-on), and its 478 points: x, y on
// screen and z, smaller nearer, in picture widths.
uniform vec4 u_face_turn[MAX_FACES];
uniform vec3 u_face_open[MAX_FACES];
uniform float u_face_mesh[MAX_FACES];
uniform highp sampler2D u_mesh;
vec3 mesh_point(int face, int n) {
    return texelFetch(u_mesh, ivec2(n, face), 0).xyz;
}
// With mesh = true, what's at any point on screen. How much of a face is there, and where that
// is on the face: x from 0 at the picture-left edge of the face to 1, y from the chin up to the
// forehead, the same spot on every face whichever way it's turned. Which face is there, or -1.
uniform highp sampler2D u_face_map;
uniform sampler2D u_face_parts;
uniform sampler2D u_face_parts2;
uniform sampler2D u_face_atlas;
uniform float u_has_face_map;
float on_face(vec2 uv) {
    return texture(u_face_map, uv).a * u_has_face_map;
}
vec2 face_uv(vec2 uv) {
    vec4 m = texture(u_face_map, uv);
    return m.xy / max(m.a, 1e-4);
}
int face_there(vec2 uv) {
    ivec2 size = textureSize(u_face_map, 0);
    vec4 m = texelFetch(u_face_map, clamp(ivec2(uv * vec2(size)), ivec2(0), size - 1), 0);
    return m.a * u_has_face_map > 0.01 ? int(m.z * 8.0 + 0.5) - 1 : -1;
}
float skin_at(vec2 uv) { return texture(u_face_parts, uv).r * u_has_face_map; }
float lips_at(vec2 uv) { return texture(u_face_parts, uv).g * u_has_face_map; }
float nose_at(vec2 uv) { return texture(u_face_parts, uv).b * u_has_face_map; }
float brows_at(vec2 uv) { return texture(u_face_parts, uv).a * u_has_face_map; }
float eye_at(vec2 uv) { return texture(u_face_parts2, uv).r * u_has_face_map; }
float iris_at(vec2 uv) { return texture(u_face_parts2, uv).g * u_has_face_map; }
float mouth_at(vec2 uv) { return texture(u_face_parts2, uv).b * u_has_face_map; }
// A face's own pixels at a position on it (as face_uv gives), from the frame on screen, so a face
// can be drawn somewhere else. The alpha is 0 outside the face.
vec4 face_texture(int face, vec2 at) {
    return texture(u_face_atlas, (vec2(float(face % 4), float(face / 4)) + at * 0.96 + 0.02) / vec2(4.0, 2.0));
}
// Whether a face has a mesh worth using, and the middle of its right (0) or left (1) iris.
bool meshed(int face) {
    return u_face_mesh[face] > 0.5;
}
vec2 iris_of(int face, int eye) {
    return mesh_point(face, eye == 0 ? 468 : 473).xy;
}
// How far a head tilts to the side, anticlockwise, in radians, from the line between its eyes.
float face_roll(int face) {
    vec2 across = u_face_points[face * 5 + 1] - u_face_points[face * 5];
    return atan(across.y * u_resolution.y, across.x * u_resolution.x);
}
// Where hair is, with segment = true: from MoGe-2's head where there is one; otherwise (with mesh
// = true too) roughly, as the person above and around each face, leaving out the face itself.
float hair_at(vec2 uv) {
    if (u_has_more_masks > 0.5) {
        return more_masks_at(uv, 1).b;
    }
    float cap = 0.0;
    for (int i = 0; i < u_face_count; i++) {
        vec2 size = max(u_faces[i].zw - u_faces[i].xy, vec2(1e-4));
        vec2 left = u_face_points[i * 5 + 1], right = u_face_points[i * 5];
        vec2 across = left - right;
        vec2 up = normalize(vec2(-across.y, across.x) + vec2(0.0, 1e-5));
        vec2 from = 0.5 * (left + right) + up * size.y * 0.25;
        float above = dot(uv - from, up);
        vec2 p = (uv - from) / (size * vec2(0.85, 0.9));
        cap = max(cap, u_face_seen[i] * smoothstep(-size.y * 0.1, size.y * 0.05, above) * (1.0 - smoothstep(0.8, 1.1, length(p))));
    }
    return cap * person_at(uv) * (1.0 - on_face(uv));
}
vec2 face_point(int face, int point) {
    return u_face_points[face * 5 + point];
}
// A face's points with the screen's shape taken out (multiplied by `aspect`), and its own up and
// across: its eyes, the middle between them, nose and middle of the mouth; how far apart the eyes
// are (spread), and how far it is from them to the mouth (height).
struct Face {
    vec2 right, left, eyes, nose, mouth, up, across;
    float spread, height;
};
Face face_frame(int i, vec2 aspect) {
    Face f;
    f.right = face_point(i, 0) * aspect;
    f.left = face_point(i, 1) * aspect;
    f.eyes = 0.5 * (f.right + f.left);
    f.nose = face_point(i, 2) * aspect;
    f.mouth = 0.5 * (face_point(i, 3) + face_point(i, 4)) * aspect;
    f.height = max(length(f.eyes - f.mouth), 0.004);
    f.up = (f.eyes - f.mouth) / f.height;
    f.across = vec2(f.up.y, -f.up.x);
    f.spread = max(length(f.left - f.right), f.height * 0.55);
    return f;
}
// How much of a soft oval `p` is in, turned with the face; half_size is across and up the face,
// in face_frame's units.
float oval(vec2 p, vec2 centre, Face f, vec2 half_size) {
    vec2 rel = p - centre;
    return 1.0 - smoothstep(0.7, 1.0, length(vec2(dot(rel, f.across), dot(rel, f.up)) / half_size));
}
// How much of a face is at a point on screen, from 0 to 1, soft at the edges.
float face_at(vec2 uv) {
    float amount = 0.0;
    for (int i = 0; i < u_face_count; i++) {
        vec2 centre = 0.5 * (u_faces[i].xy + u_faces[i].zw);
        vec2 radius = max(0.5 * (u_faces[i].zw - u_faces[i].xy), vec2(1e-5));
        amount = max(amount, u_face_seen[i] * (1.0 - smoothstep(0.8, 1.15, length((uv - centre) / radius))));
    }
    return amount;
}
// Which face a point of the picture belongs with, as an index into u_faces, or -1 if there are
// none. With who's who (segment = true), the face on the same person; otherwise, or for a person
// whose face can't be seen, the nearest, with what's underneath a face counting as nearer than
// what's alongside it (bodies hang below faces).
int who_at(vec2 uv) {
    int person = person_there(uv);
    if (person >= 0) {
        for (int i = 0; i < u_face_count; i++) {
            if (u_face_seen[i] > 0.02 && person_there(0.5 * (u_faces[i].xy + u_faces[i].zw)) == person) return i;
        }
    }
    int best = -1;
    float nearest = 1e9;
    for (int i = 0; i < u_face_count; i++) {
        if (u_face_seen[i] <= 0.02) continue;
        vec2 centre = 0.5 * (u_faces[i].xy + u_faces[i].zw);
        vec2 size = max(u_faces[i].zw - u_faces[i].xy, vec2(1e-4));
        vec2 d = (uv - centre) / size;
        if (d.y < 0.0) d.y *= 0.35;
        float away = dot(d, d);
        if (away < nearest) { nearest = away; best = i; }
    }
    return best;
}
// 1 on a cut, fading to 0 in about half a second; and how long the frame on screen has been in
// its shot, in seconds (with cuts = true). u_since_cut is huge until the first cut.
uniform float u_cut;
uniform float u_since_cut;
// When this shot began, in seconds into the video: the same all through a shot, and 0 until the
// first cut (with cuts = true), so something random can be chosen once for a whole shot.
uniform float u_shot_start;
// What sort of shot this is, and what's going on across the last few (with scene = true).
// u_shot_size: 0 very wide to 1 extreme close-up. u_camera: pan, tilt, zoom and roll a second.
// u_handheld: 0 to 1. u_pace: cuts a minute. u_scene: action, dialogue, quiet, montage, each 0 to 1.
uniform float u_shot_size;
uniform vec4 u_camera;
uniform float u_handheld;
uniform float u_pace;
uniform vec4 u_scene;
// The look of the picture (with stats = true). u_palette holds its main colours, each keeping its
// slot as they change, and u_palette_share how much of the picture each covers.
uniform float u_brightness;
uniform float u_colourfulness;
uniform float u_warmth;
uniform vec3 u_palette[4];
uniform float u_palette_share[4];
// Where the brightest part of the picture is, from 0 to 1 across the picture with y going up, and
// in z how much it stands out as one place.
uniform vec3 u_light;
// The picture some seconds ago, up to about one, at half size (with history = true).
uniform sampler2DArray u_history;
uniform float u_has_history;
uniform float u_history_head;
uniform float u_history_count;
uniform float u_history_rate;
vec3 history_at(vec2 uv, float seconds) {
    float frames = clamp(seconds * u_history_rate, 0.0, max(u_history_count - 1.0, 0.0));
    float older = floor(frames);
    float frames_kept = max(float(textureSize(u_history, 0).z), 1.0);
    vec3 a = texture(u_history, vec3(uv, mod(u_history_head - older, frames_kept))).rgb;
    vec3 b = texture(u_history, vec3(uv, mod(u_history_head - older - 1.0, frames_kept))).rgb;
    return mix(a, b, u_history_count > older + 1.0 ? frames - older : 0.0);
}
// How a point of the screen is moving, in screen widths and heights a second (with flow = true).
uniform sampler2D u_flow;
uniform float u_has_flow;
vec2 flow_at(vec2 uv) {
    vec2 size = max(u_video_rect.zw - u_video_rect.xy, vec2(1e-6));
    return texture(u_flow, (uv - u_video_rect.xy) / size).rg * size * u_has_flow;
}
// The room with the people taken out, and how much of a point has actually been seen rather than
// filled in from around it (with plate = true).
uniform sampler2D u_plate;
uniform float u_has_plate;
vec3 plate_at(vec2 uv) {
    vec4 sum = textureLod(u_plate, uv, 0.0);
    for (int lod = 1; lod < 9 && sum.a < 0.97; lod++) {
        sum += textureLod(u_plate, uv, float(lod)) * (1.0 - sum.a);
    }
    return sum.rgb / max(sum.a, 1e-3);
}
float plate_known(vec2 uv) {
    return textureLod(u_plate, uv, 0.0).a * u_has_plate;
}
// A random number from 0 to 1 for a point.
float hash(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}
float hash(vec3 p) {
    p = fract(p * vec3(0.1031, 0.1137, 0.1279));
    p += dot(p, p.yzx + 33.33);
    return fract((p.x + p.y) * p.z);
}
// Smooth noise from 0 to 1, changing about once a unit, and five octaves of it (fbm), from 0 to
// about 1.
float noise(vec2 p) {
    vec2 i = floor(p), f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), f.x), f.y);
}
float fbm(vec2 p) {
    float sum = 0.0, amount = 0.5;
    for (int i = 0; i < 5; i++) {
        sum += amount * noise(p);
        p = p * 2.03 + vec2(17.1, 9.2);
        amount *= 0.5;
    }
    return sum;
}
// A rainbow colour, going round once as t goes from 0 to 1.
vec3 hue(float t) {
    return 0.5 + 0.5 * cos(6.2831853 * (t + vec3(0.0, 0.33, 0.67)));
}
// A colour with its hue turned by `a` radians.
vec3 hue_rotate(vec3 c, float a) {
    vec3 k = vec3(0.57735);
    return c * cos(a) + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - cos(a));
}
// A point turned anticlockwise by `a` radians about the origin.
vec2 rotate(vec2 p, float a) {
    float c = cos(a), s = sin(a);
    return vec2(c * p.x - s * p.y, s * p.x + c * p.y);
}
";

/// The effect's fragment shader: Flex's header, the plugin's file (which defines
/// `vec4 effect(vec2 uv)`) and a `main` calling it. `#line 1` keeps error line numbers
/// matching the plugin's file.
pub fn fragment_source(version: &str, embedded: bool, uniforms: &[String], body: &str) -> String {
    let precision = if embedded { "precision highp float;\nprecision highp int;\nprecision highp sampler2DArray;\n" } else { "" };
    let settings: String = uniforms.iter().map(|u| format!("uniform float {u};\n")).collect();
    format!(
        "{version}\n{precision}{HEADER}{settings}in vec2 v_uv;\nout vec4 flex_color;\n#line 1\n{body}\n\
         void main() {{ flex_color = effect(v_uv); }}\n"
    )
}

pub(crate) fn vertex_source(version: &str) -> String {
    // One triangle covering the screen, with no vertex buffer.
    format!(
        "{version}\nout vec2 v_uv;\nvoid main() {{\n    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));\n\
         v_uv = p;\n    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);\n}}\n"
    )
}

fn copy_source(version: &str, embedded: bool) -> String {
    let precision = if embedded { "precision highp float;\n" } else { "" };
    format!(
        "{version}\n{precision}uniform sampler2D u_tex;\nuniform sampler2D u_from;\nuniform sampler2D u_plain;\nuniform sampler2D u_words;\nuniform float u_has_words;\nuniform vec4 u_picture;\nuniform float u_faded_in;\nuniform float u_tex_mix;\nuniform float u_from_mix;\nuniform float u_flip;\n\
         in vec2 v_uv;\nout vec4 flex_color;\n\
         void main() {{\n    vec2 uv = vec2(v_uv.x, u_flip > 0.5 ? 1.0 - v_uv.y : v_uv.y);\n\
         vec4 words = texture(u_words, uv) * u_has_words;\n\
         vec2 at = (uv - u_picture.xy) / (u_picture.zw - u_picture.xy);\n\
         vec3 plain = texture(u_plain, at).rgb;\n\
         vec3 from = mix(plain, texture(u_from, at).rgb, u_from_mix);\n\
         vec3 now = mix(plain, texture(u_tex, at).rgb, u_tex_mix);\n\
         vec3 picture = at == clamp(at, 0.0, 1.0) ? mix(from, now, u_faded_in) : vec3(0.0);\n\
         flex_color = vec4(picture * (1.0 - words.a) + words.rgb, 1.0);\n}}\n"
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
        self.slots.iter().filter_map(|s| s.status.as_ref()).find(|(path, _)| *path == effect.shader).map(|(_, s)| s)
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
        render: impl FnOnce(i32, i32, i32, Option<i32>),
    ) -> bool {
        // Effects only see the picture itself: to them, its edges are the edges of everything.
        let (effect, rect) = (&frame.effect, [0.0, 0.0, 1.0, 1.0]);
        unsafe {
            if self.unsupported || w <= 0 || h <= 0 {
                return false;
            }
            let version = egui_glow::ShaderVersion::get(gl);
            if !version.is_new_shader_interface() {
                log::warn!("video effects need OpenGL 3 or OpenGL ES 3");
                self.unsupported = true;
                self.slots[self.front].status = Some((effect.shader.clone(), (false, "Needs OpenGL 3".into())));
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
            // As a fade starts, the effect leaving keeps its slot and the new one takes the other.
            let holds = |slot: &Slot, shader: &Path| slot.source.as_ref().is_some_and(|s| s.path == shader);
            if !holds(&self.slots[self.front], &effect.shader)
                && (holds(&self.slots[1 - self.front], &effect.shader) || frame.fading.as_ref().is_some_and(|(from, _)| holds(&self.slots[self.front], &from.shader)))
            {
                self.front = 1 - self.front;
            }
            let (front, back) = (self.front, 1 - self.front);
            self.slots[front].reload(gl, effect, version);
            match &frame.fading {
                Some((from, _)) => self.slots[back].reload(gl, from, version),
                None => self.slots[back].free(gl),
            }
            if self.slots[front].program.is_none() && self.slots[back].program.is_none() {
                return false;
            }
            let (Some(copy), Some(vao)) = (self.copy, self.vao) else { return false };

            // Where the picture is in the window, leaving out black bars, whether mpv added them
            // or they're part of the file. Not while the layout's still settling.
            let picture = self.bars.inside(frame.rect);
            let picture = if picture[2] - picture[0] < 0.2 || picture[3] - picture[1] < 0.2 { [0.0, 0.0, 1.0, 1.0] } else { picture };
            let at_scale = |scale: f32| {
                let across = w as f32 * (picture[2] - picture[0]) * scale;
                let down = h as f32 * (picture[3] - picture[1]) * scale;
                ((across.round() as i32).max(1), (down.round() as i32).max(1))
            };
            let working = frame.video_scale.max(effect.scale).clamp(0.25, 1.0);
            let window = (((w as f32 * working).round() as i32).max(1), ((h as f32 * working).round() as i32).max(1));
            let size = at_scale(working);
            if self.objects.as_ref().is_none_or(|o| o.window != window || o.size != size) {
                self.free_objects(gl);
                let rgba8 = [(glow::RGBA8, glow::UNSIGNED_BYTE)];
                let made = create_target(gl, window, &rgba8).and_then(|video| match create_target(gl, size, &rgba8) {
                    Ok(cropped) => Ok((video, cropped)),
                    Err(e) => {
                        delete_target(gl, video);
                        Err(e)
                    }
                });
                match made {
                    Ok((video, cropped)) => self.objects = Some(Objects { window, size, video, cropped }),
                    Err(e) => {
                        log::warn!("video effects: {e}");
                        self.slots[front].status = Some((effect.shader.clone(), (false, format!("Can't run: {e}"))));
                        return false;
                    }
                }
            }
            let Some(o) = &self.objects else { return false };
            let scales = [Some(effect), frame.fading.as_ref().map(|(from, _)| from)];
            for (slot, drawn) in [front, back].into_iter().zip(scales) {
                let Some(drawn) = drawn else { continue };
                if let Err(e) = self.slots[slot].prepare(gl, at_scale(drawn.scale.clamp(0.25, 1.0))) {
                    log::warn!("video effects: {e}");
                    self.slots[slot].status = Some((drawn.shader.clone(), (false, format!("Can't run: {e}"))));
                    return false;
                }
            }
            // A feedback effect fading in starts from the one fading out, instead of from black.
            if frame.fading.is_some() && effect.feedback && self.slots[front].frame == 0 {
                let [a, b] = &self.slots;
                let (new, old) = if front == 0 { (a, b) } else { (b, a) };
                if let (Some(new_out), Some(old_out)) = (&new.out, &old.out) {
                    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(old_out[old.drawn].fbo));
                    gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(new_out[1 - new.current].fbo));
                    gl.blit_framebuffer(0, 0, old.size.0, old.size.1, 0, 0, new.size.0, new.size.1, glow::COLOR_BUFFER_BIT, glow::LINEAR);
                    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                }
            }

            // Subtitles go on top of the effect, not through it.
            if self.words.as_ref().is_some_and(|(_, at)| *at != window || !frame.subtitles) {
                if let Some((words, _)) = self.words.take() {
                    delete_target(gl, words);
                }
                self.held_words.free(gl);
            }
            if frame.subtitles && self.words.is_none() {
                match create_target(gl, window, &[(glow::RGBA8, glow::UNSIGNED_BYTE)]) {
                    Ok(words) => self.words = Some((words, window)),
                    Err(e) => log::warn!("video effects: subtitles: {e}"),
                }
            }
            render(o.video.fbo.0.get() as i32, window.0, window.1, self.words.as_ref().map(|(words, _)| words.fbo.0.get() as i32));
            self.bars.update(gl, o.video.fbo, window, frame.rect, frame.video_time);
            let x0 = ((picture[0] * window.0 as f32).round() as i32).clamp(0, window.0 - size.0);
            let y0 = ((picture[1] * window.1 as f32).round() as i32).clamp(0, window.1 - size.1);
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(o.video.fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(o.cropped.fbo));
            gl.blit_framebuffer(x0, y0, x0 + size.0, y0 + size.1, 0, 0, size.0, size.1, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            let video = &o.cropped;
            let holding = frame.depth.is_some() || frame.faces.is_some() || frame.motion.is_some() || frame.camera.is_some() || frame.restyle.is_some();
            if holding {
                self.held.push(gl, video, size, frame.pts);
                if let Some((words, _)) = &self.words {
                    self.held_words.push(gl, words, window, frame.pts);
                }
            } else {
                self.held.free(gl);
                self.held_words.free(gl);
            }
            // The networks look at the kept copy of a frame, however many paints later they get
            // to it, so what they find is about exactly the picture that will be drawn.
            let (source, source_time) = self.held.newest().unwrap_or((video.fbo, frame.pts));
            // First: what the networks are told about cuts comes from this having seen the frame.
            if frame.find_cuts {
                self.cuts.update(gl, source, size, rect, source_time);
            }
            match &frame.depth {
                Some(link) => self.depth.update(gl, link, &self.cuts, source, size, rect, source_time, frame.video_time),
                None => self.depth.lateness.delay = 0.0,
            }
            match &frame.faces {
                Some(link) => self.faces.update(gl, link, &self.cuts, source, size, rect, source_time, frame.video_time),
                None => self.faces.lateness.delay = 0.0,
            }
            match &frame.motion {
                Some(link) => self.motion.update(gl, link, &self.cuts, source, size, rect, source_time, frame.video_time),
                None => self.motion.lateness.delay = 0.0,
            }
            match &frame.camera {
                Some(link) => self.camera.update(gl, link, &self.cuts, source, size, rect, source_time, frame.video_time),
                None => self.camera.lateness.delay = 0.0,
            }
            match &frame.restyle {
                Some(link) => self.restyle.update(gl, link, &self.cuts, source, size, rect, source_time, frame.video_time),
                None => self.restyle.lateness.delay = 0.0,
            }
            // The picture is held back to the frame the networks have caught up with.
            let held_back = self.held_back();
            let (video_texture, pts) = if holding {
                self.held.pick(frame.video_time - held_back).unwrap_or((video.texture, frame.pts))
            } else {
                (video.texture, frame.pts)
            };
            let words_texture = self.words.as_ref().map(|(words, _)| {
                self.held_words.pick(frame.video_time - held_back).map_or(words.texture, |w| w.0)
            });
            let (depth_texture, mask_texture, normal_texture, more_textures) = if frame.depth.is_some() { self.depth.map_for(pts) } else { (None, None, None, None) };
            let people = match self.depth.maps.nearest(pts, DEPTH_REACH) {
                Some((m, _)) if frame.depth.is_some() && self.depth.has_more_masks => PeopleUniforms::of(&m.people, rect),
                _ => PeopleUniforms::default(),
            };
            let faces = if frame.faces.is_some() { self.faces.uniforms(pts, rect) } else { FaceUniforms::default() };
            let clues = std::mem::take(&mut self.depth.clues);
            let shot = if frame.needs.scene {
                for (time, found) in clues {
                    self.shots.add_clues(time, found);
                }
                let (height, wide) = ((rect[3] - rect[1]).max(1e-3), (rect[2] - rect[0]).max(1e-3));
                let seen = (0..faces.count as usize).filter(|i| faces.seen[*i] >= 0.5);
                let face = seen.clone().map(|i| crate::shot_type::Face { height: (faces.boxes[i * 4 + 3] - faces.boxes[i * 4 + 1]).abs() / height, x: ((faces.boxes[i * 4] + faces.boxes[i * 4 + 2]) * 0.5 - rect[0]) / wide }).max_by(|a, b| a.height.total_cmp(&b.height));
                let fit = self.motion.fit.take();
                self.shots.update(&crate::shot_type::Moment {
                    time: pts,
                    last_cut: self.cuts.found.since(pts).map(|since| pts - since),
                    faces: seen.count() as u32,
                    face,
                    fit: fit.map(|f| f.0),
                    speed: fit.map(|f| f.1),
                    level: frame.audio.as_ref().map(|a| a.level),
                    beat: frame.audio.as_ref().is_some_and(|a| a.beat > 0.5),
                });
                self.shots.reading()
            } else {
                crate::shot_type::Reading::default()
            };
            if frame.needs.mesh {
                self.faces.upload_mesh(gl, pts, rect);
            }

            let has_face_map = frame.needs.mesh && self.faces.draw_map(gl, version.version_declaration(), version.is_embedded(), video_texture, size, &faces);

            let painted_at = Instant::now();
            let dt = self.last_paint.map_or(1.0 / 60.0, |t| painted_at.duration_since(t).as_secs_f32()).min(0.25);
            self.last_paint = Some(painted_at);
            let decl = version.version_declaration();
            let vertex = vertex_source(decl);
            if frame.needs.history {
                self.history.push(gl, source, size, source_time);
            }
            if let Some(at) = self.cuts.fresh.take() {
                self.flow.zero_at(gl, at);
            }
            // Block matching only until the network's going, or if it can't.
            let network_flow = frame.motion.is_some().then(|| self.motion.at(pts)).flatten();
            if frame.needs.flow && network_flow.is_none() {
                self.flow.update(gl, decl, &vertex, version.is_embedded(), vao, source, size, rect, source_time);
            }
            if frame.needs.plate && let Some(people) = mask_texture.filter(|_| self.depth.has_masks) {
                let reset = self.plate.last_time().is_some_and(|last| self.cuts.found.between(last, pts));
                self.plate.update(gl, decl, &vertex, version.is_embedded(), vao, video_texture, people, size, rect, pts, reset);
            }
            let snap = self.cuts.found.since(pts).is_some_and(|since| since < 0.05);
            self.stats.ease(&self.cuts.stats, if snap { 1.0 } else { (dt * 6.0).min(1.0) });

            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(Some(vao));

            // The restyled picture or living texture first: drawing it uses texture units of its
            // own, so the ones every effect is given are bound after.
            let history = if frame.needs.history { self.history.texture() } else { None };
            let flow = if frame.needs.flow { network_flow.or_else(|| self.flow.at(pts)) } else { None };
            let plate = if frame.needs.plate { self.plate.texture() } else { None };
            let restyled = match (&frame.restyle, &frame.texture) {
                (Some(_), _) => self.restyle.draw(gl, version, vao, video_texture, flow, size, rect, pts),
                (None, Some(link)) => self.nca.draw(gl, version, vao, link, video_texture, flow, size, rect, dt),
                (None, None) => None,
            };
            gl.bind_vertex_array(Some(vao));

            // What every effect is given.
            let audio = frame.audio.as_deref();
            Unit::Video.bind(gl, Some(video_texture));
            if let Some(spectrum) = self.spectrum {
                Unit::Spectrum.bind(gl, Some(spectrum));
                if let Some(a) = audio {
                    let bytes: &[u8] = std::slice::from_raw_parts(a.spectrum.as_ptr().cast(), a.spectrum.len() * 4);
                    gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
                    let (w, h) = (audio::BANDS as i32, audio::HISTORY as i32);
                    gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, w, h, glow::RED, glow::FLOAT, glow::PixelUnpackData::Slice(Some(bytes)));
                }
            }
            Unit::Depth.bind(gl, depth_texture);
            Unit::Masks.bind(gl, mask_texture);
            Unit::Normals.bind(gl, normal_texture);
            Unit::MoreMasks.bind(gl, more_textures);
            Unit::History.bind(gl, history);
            Unit::Flow.bind(gl, flow);
            Unit::Plate.bind(gl, plate);
            Unit::Mesh.bind(gl, if frame.needs.mesh { self.faces.mesh_texture } else { None });
            let map_textures = self.faces.map.as_ref().filter(|_| has_face_map).map(|m| m.textures());
            for (i, unit) in [Unit::FaceMap, Unit::FaceParts, Unit::FaceParts2, Unit::FaceAtlas].into_iter().enumerate() {
                unit.bind(gl, map_textures.map(|t| t[i]));
            }
            let given = Given {
                dt,
                since_cut: if frame.needs.cuts { self.cuts.found.since(pts) } else { None },
                shot_start: if frame.needs.cuts { self.cuts.found.since(pts).map_or(0.0, |since| ((pts - since) * 100.0).round() / 100.0) } else { 0.0 },
                history: history.map(|_| self.history.spot(pts)),
                has_flow: flow.is_some(),
                has_plate: plate.is_some(),
                stats: &self.stats,
                rect,
                video_time: frame.video_time,
                audio,
                has_depth: depth_texture.is_some() && self.depth.serial > 0,
                ground: self.depth.ground,
                has_masks: mask_texture.is_some() && self.depth.has_masks,
                has_normals: normal_texture.is_some() && self.depth.has_normals,
                has_more_masks: more_textures.is_some() && self.depth.has_more_masks,
                people: &people,
                faces: &faces,
                has_face_map,
                shot,
                world: if frame.camera.is_some() { self.camera.at(pts) } else { None },
                has_restyled: restyled.is_some(),
                restyled_size: if frame.restyle.is_some() { self.restyle.shown_size } else { (0, 0) },
                original: ORIGINAL_UNIT as i32,
            };

            // The effects, each at its working size. One drawing on the restyled picture has it
            // as u_video, and the real one as u_original.
            let picture_for = |effect: &ActiveEffect| if effect.restyle { restyled.unwrap_or(video_texture) } else { video_texture };
            gl.active_texture(glow::TEXTURE0 + ORIGINAL_UNIT);
            gl.bind_texture(glow::TEXTURE_2D, Some(video_texture));
            Unit::Video.bind(gl, Some(picture_for(effect)));
            self.slots[front].draw(gl, effect, &given, frame.amount);
            if let Some((from, amount)) = &frame.fading {
                Unit::Video.bind(gl, Some(picture_for(from)));
                self.slots[back].draw(gl, from, &given, *amount);
            }

            // Onto the screen, scaled up if the effect works smaller. One that didn't compile
            // fades as the plain video, and one that doesn't fade itself is mixed with it.
            let shown = |slot: &Slot| slot.program.and(slot.out.as_ref()).map_or(video_texture, |out| out[slot.drawn].texture);
            let mixed = |slot: &Slot, amount: f32| if slot.fades { 1.0 } else { amount };
            let new = shown(&self.slots[front]);
            let new_mix = mixed(&self.slots[front], frame.amount);
            let (old, old_mix, faded_in) = match &frame.fading {
                Some((_, amount)) => (shown(&self.slots[back]), mixed(&self.slots[back], *amount), frame.faded_in),
                None => (new, new_mix, 1.0),
            };
            gl.bind_framebuffer(glow::FRAMEBUFFER, target);
            gl.viewport(0, 0, w, h);
            gl.use_program(Some(copy));
            gl.active_texture(glow::TEXTURE13);
            gl.bind_texture(glow::TEXTURE_2D, words_texture);
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, Some(video_texture));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(old));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(new));
            gl.uniform_1_i32(gl.get_uniform_location(copy, "u_plain").as_ref(), 2);
            gl.uniform_1_f32(gl.get_uniform_location(copy, "u_tex_mix").as_ref(), new_mix);
            gl.uniform_1_f32(gl.get_uniform_location(copy, "u_from_mix").as_ref(), old_mix);
            gl.uniform_1_i32(gl.get_uniform_location(copy, "u_words").as_ref(), 13);
            gl.uniform_4_f32(gl.get_uniform_location(copy, "u_picture").as_ref(), picture[0], picture[1], picture[2], picture[3]);
            gl.uniform_1_f32(gl.get_uniform_location(copy, "u_has_words").as_ref(), if words_texture.is_some() { 1.0 } else { 0.0 });
            gl.uniform_1_i32(gl.get_uniform_location(copy, "u_tex").as_ref(), 0);
            gl.uniform_1_i32(gl.get_uniform_location(copy, "u_from").as_ref(), 1);
            gl.uniform_1_f32(gl.get_uniform_location(copy, "u_faded_in").as_ref(), faded_in);
            // The window's rows run bottom-up like ours; an offscreen target's top-down.
            let flip = if target.is_some() { 1.0 } else { 0.0 };
            gl.uniform_1_f32(gl.get_uniform_location(copy, "u_flip").as_ref(), flip);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            for unit in Unit::ALL {
                unit.bind(gl, None);
            }
            gl.active_texture(glow::TEXTURE0 + ORIGINAL_UNIT);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_vertex_array(None);
            gl.use_program(None);
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
            for slot in &mut self.slots {
                slot.free(gl);
            }
            if let Some(p) = self.copy.take() {
                gl.delete_program(p);
            }
            if let Some(t) = self.spectrum.take() {
                gl.delete_texture(t);
            }
            self.depth.release(gl);
            self.held.free(gl);
            self.bars.release(gl);
            self.faces.release(gl);
            self.cuts.release(gl);
            self.history.release(gl);
            self.flow.release(gl);
            self.motion.release(gl);
            self.camera.release(gl);
            self.restyle.release(gl);
            self.nca.release(gl);
            self.plate.release(gl);
            self.last_paint = None;
            if let Some(vao) = self.vao.take() {
                gl.delete_vertex_array(vao);
            }
        }
    }

    /// How far behind the picture is being held (seconds), which the sound has to match.
    pub fn held_back(&self) -> f32 {
        self.depth.lateness.delay.max(self.faces.lateness.delay).max(self.motion.lateness.delay).max(self.camera.lateness.delay).max(self.restyle.lateness.delay)
    }

    /// What's known of the shot on the screen, for choosing an effect to suit it.
    pub fn shot_hint(&self) -> flex_core::effects::ShotHint {
        let r = self.shots.reading();
        flex_core::effects::ShotHint { size: r.knows_size.then_some(r.size), scene: r.knows_scene.then_some(r.scene) }
    }

    /// Whether a scene cut has reached `shown` in the video since this was last asked.
    pub fn cut_shown(&mut self, shown: f32) -> bool {
        self.cuts.due.take_if(|at| *at <= shown).is_some()
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

    unsafe fn free_objects(&mut self, gl: &glow::Context) {
        unsafe {
            self.held.free(gl);
            self.held_words.free(gl);
            if let Some((words, _)) = self.words.take() {
                delete_target(gl, words);
            }
        }
        if let Some(o) = self.objects.take() {
            unsafe {
                for t in [o.video, o.cropped] {
                    delete_target(gl, t);
                }
            }
        }
    }
}

/// The texture units every effect is given, each under a uniform of its own: bound before
/// effects draw (`u_prev` by each), and let go after.
#[derive(Clone, Copy)]
enum Unit {
    Video,
    Prev,
    Spectrum,
    Depth,
    Masks,
    History,
    Flow,
    Plate,
    Mesh,
    FaceMap,
    FaceParts,
    FaceParts2,
    FaceAtlas,
    Normals,
    MoreMasks,
}

/// `u_original`'s unit, after the rest: the real picture, beside a restyled one. Every GPU has
/// at least 16.
const ORIGINAL_UNIT: u32 = 15;

impl Unit {
    const ALL: [Unit; 15] = [
        Unit::Video,
        Unit::Prev,
        Unit::Spectrum,
        Unit::Depth,
        Unit::Masks,
        Unit::History,
        Unit::Flow,
        Unit::Plate,
        Unit::Mesh,
        Unit::FaceMap,
        Unit::FaceParts,
        Unit::FaceParts2,
        Unit::FaceAtlas,
        Unit::Normals,
        Unit::MoreMasks,
    ];

    fn uniform(self) -> &'static str {
        match self {
            Unit::Video => "u_video",
            Unit::Prev => "u_prev",
            Unit::Spectrum => "u_spectrum",
            Unit::Depth => "u_depth",
            Unit::Masks => "u_masks",
            Unit::History => "u_history",
            Unit::Flow => "u_flow",
            Unit::Plate => "u_plate",
            Unit::Mesh => "u_mesh",
            Unit::FaceMap => "u_face_map",
            Unit::FaceParts => "u_face_parts",
            Unit::FaceParts2 => "u_face_parts2",
            Unit::FaceAtlas => "u_face_atlas",
            Unit::Normals => "u_normals",
            Unit::MoreMasks => "u_more_masks",
        }
    }

    /// Puts `texture` on this unit, leaving it the active one.
    unsafe fn bind(self, gl: &glow::Context, texture: Option<glow::Texture>) {
        let target = if matches!(self, Unit::History | Unit::MoreMasks) { glow::TEXTURE_2D_ARRAY } else { glow::TEXTURE_2D };
        unsafe {
            gl.active_texture(glow::TEXTURE0 + self as u32);
            gl.bind_texture(target, texture);
        }
    }
}

/// What every effect drawn this frame is given, besides the textures already bound.
struct Given<'a> {
    dt: f32,
    since_cut: Option<f32>,
    history: Option<HistorySpot>,
    has_flow: bool,
    has_plate: bool,
    stats: &'a Stats,
    rect: [f32; 4],
    video_time: f32,
    audio: Option<&'a Features>,
    shot_start: f32,
    shot: crate::shot_type::Reading,
    world: Option<crate::camera::Report>,
    has_depth: bool,
    ground: [f32; 4],
    has_masks: bool,
    has_normals: bool,
    has_more_masks: bool,
    people: &'a PeopleUniforms,
    faces: &'a FaceUniforms,
    has_face_map: bool,
    /// Whether there's a restyled picture yet, the size the network drew it at, and the unit
    /// the real one's on.
    has_restyled: bool,
    restyled_size: (usize, usize),
    original: i32,
}

impl Slot {
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
                self.free_targets(gl);
            }
            let status = match std::fs::read_to_string(&source.path) {
                Err(e) => {
                    self.delete_program(gl);
                    let name = source.path.file_name().unwrap_or_default().to_string_lossy();
                    (false, format!("Can't read {name}: {e}"))
                }
                Ok(body) => {
                    self.fades = body.contains("u_amount");
                    let decl = version.version_declaration();
                    let fragment = fragment_source(decl, version.is_embedded(), &source.uniforms, &body);
                    let started = Instant::now();
                    match compile(gl, &vertex_source(decl), &fragment) {
                        Ok(program) => {
                            self.delete_program(gl);
                            self.program = Some(program);
                            log::info!("video effect {}: loaded {} in {} ms", effect.name, source.path.display(), started.elapsed().as_millis());
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

    /// Makes sure its output is `size`.
    unsafe fn prepare(&mut self, gl: &glow::Context, size: (i32, i32)) -> Result<(), String> {
        if self.out.is_some() && self.size == size {
            return Ok(());
        }
        unsafe {
            self.free_targets(gl);
            let rgba8 = (glow::RGBA8, glow::UNSIGNED_BYTE);
            // Feedback fades over many frames: 8 bits a channel would leave trails stuck at
            // faint colours, so half floats where the GPU can draw into them.
            let float = [(glow::RGBA16F, glow::HALF_FLOAT), rgba8];
            let a = create_target(gl, size, &float);
            let b = create_target(gl, size, &float);
            match (a, b) {
                (Ok(a), Ok(b)) => {
                    self.out = Some([a, b]);
                    self.size = size;
                    Ok(())
                }
                (a, b) => {
                    for t in [a, b].into_iter().flatten() {
                        delete_target(gl, t);
                    }
                    Err("framebuffers incomplete".into())
                }
            }
        }
    }

    /// Runs the effect once. Texture units 0 and 2–4 are already bound.
    unsafe fn draw(&mut self, gl: &glow::Context, effect: &ActiveEffect, given: &Given, amount: f32) {
        let (Some(program), Some(targets)) = (self.program, &self.out) else { return };
        unsafe {
            let (out, prev) = (&targets[self.current], &targets[1 - self.current]);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(out.fbo));
            gl.viewport(0, 0, self.size.0, self.size.1);
            gl.use_program(Some(program));
            Unit::Prev.bind(gl, Some(prev.texture));
            let at = |name: &str| gl.get_uniform_location(program, name);
            for unit in Unit::ALL {
                gl.uniform_1_i32(at(unit.uniform()).as_ref(), unit as i32);
            }
            gl.uniform_2_f32(at("u_resolution").as_ref(), self.size.0 as f32, self.size.1 as f32);
            let started = *self.started.get_or_insert_with(Instant::now);
            gl.uniform_1_f32(at("u_time").as_ref(), started.elapsed().as_secs_f32());
            gl.uniform_1_i32(at("u_frame").as_ref(), self.frame);
            let rect = given.rect;
            gl.uniform_4_f32(at("u_video_rect").as_ref(), rect[0], rect[1], rect[2], rect[3]);
            gl.uniform_1_f32(at("u_video_time").as_ref(), given.video_time);
            gl.uniform_1_f32(at("u_dt").as_ref(), given.dt);
            gl.uniform_1_f32(at("u_since_cut").as_ref(), given.since_cut.unwrap_or(1e6));
            gl.uniform_1_f32(at("u_shot_start").as_ref(), given.shot_start);
            gl.uniform_1_f32(at("u_world_ready").as_ref(), given.world.map_or(0.0, |w| w.ready));
            if let Some(w) = given.world {
                // By columns, as GL wants.
                let r = w.rotation;
                let columns = [r[0][0], r[1][0], r[2][0], r[0][1], r[1][1], r[2][1], r[0][2], r[1][2], r[2][2]];
                gl.uniform_matrix_3_f32_slice(at("u_world_rotation").as_ref(), false, &columns);
                gl.uniform_3_f32_slice(at("u_world_offset").as_ref(), &w.offset);
                gl.uniform_3_f32_slice(at("u_world_lens").as_ref(), &w.lens);
                gl.uniform_2_f32_slice(at("u_world_depth").as_ref(), &w.depth);
            }
            gl.uniform_1_f32(at("u_shot_size").as_ref(), given.shot.size);
            gl.uniform_4_f32_slice(at("u_camera").as_ref(), &given.shot.camera);
            gl.uniform_1_f32(at("u_handheld").as_ref(), given.shot.handheld);
            gl.uniform_1_f32(at("u_pace").as_ref(), given.shot.pace);
            gl.uniform_4_f32_slice(at("u_scene").as_ref(), &given.shot.scene);
            gl.uniform_1_f32(at("u_cut").as_ref(), given.since_cut.map_or(0.0, |s| (-s * 6.0).exp()));
            let spot = given.history.unwrap_or_default();
            gl.uniform_1_f32(at("u_has_history").as_ref(), if given.history.is_some() { 1.0 } else { 0.0 });
            gl.uniform_1_f32(at("u_history_head").as_ref(), spot.head);
            gl.uniform_1_f32(at("u_history_count").as_ref(), spot.count);
            gl.uniform_1_f32(at("u_history_rate").as_ref(), spot.rate);
            gl.uniform_1_f32(at("u_has_flow").as_ref(), if given.has_flow { 1.0 } else { 0.0 });
            gl.uniform_1_f32(at("u_has_plate").as_ref(), if given.has_plate { 1.0 } else { 0.0 });
            let stats = given.stats;
            gl.uniform_1_f32(at("u_brightness").as_ref(), stats.brightness);
            gl.uniform_1_f32(at("u_colourfulness").as_ref(), stats.colourfulness);
            gl.uniform_1_f32(at("u_warmth").as_ref(), stats.warmth);
            gl.uniform_3_f32_slice(at("u_palette").as_ref(), stats.palette.as_flattened());
            gl.uniform_1_f32_slice(at("u_palette_share").as_ref(), &stats.share);
            gl.uniform_3_f32(at("u_light").as_ref(), stats.light[0], stats.light[1], stats.light[2]);
            let level = |f: fn(&Features) -> f32| given.audio.map_or(0.0, f);
            gl.uniform_1_f32(at("u_level").as_ref(), level(|a| a.level));
            gl.uniform_1_f32(at("u_bass").as_ref(), level(|a| a.bass));
            gl.uniform_1_f32(at("u_mid").as_ref(), level(|a| a.mid));
            gl.uniform_1_f32(at("u_treble").as_ref(), level(|a| a.treble));
            gl.uniform_1_f32(at("u_beat").as_ref(), level(|a| a.beat));
            for (i, name) in SOUND_UNIFORMS.iter().enumerate() {
                gl.uniform_1_f32(at(name).as_ref(), given.audio.and_then(|a| a.kinds.get(i).copied()).unwrap_or(0.0));
            }
            gl.uniform_1_f32(at("u_amount").as_ref(), if self.fades { amount } else { 1.0 });
            gl.uniform_1_f32(at("u_has_depth").as_ref(), if given.has_depth { 1.0 } else { 0.0 });
            gl.uniform_4_f32(at("u_ground").as_ref(), given.ground[0], given.ground[1], given.ground[2], given.ground[3]);
            gl.uniform_1_f32(at("u_has_masks").as_ref(), if given.has_masks { 1.0 } else { 0.0 });
            gl.uniform_1_f32(at("u_has_normals").as_ref(), if given.has_normals { 1.0 } else { 0.0 });
            gl.uniform_1_f32(at("u_has_more_masks").as_ref(), if given.has_more_masks { 1.0 } else { 0.0 });
            let people = given.people;
            gl.uniform_1_i32(at("u_person_count").as_ref(), people.count);
            gl.uniform_1_f32_slice(at("u_person_seen").as_ref(), &people.seen);
            gl.uniform_4_f32_slice(at("u_person_box").as_ref(), &people.boxes);
            gl.uniform_2_f32_slice(at("u_person_centre").as_ref(), &people.centres);
            gl.uniform_4_f32_slice(at("u_person_head").as_ref(), &people.heads);
            gl.uniform_4_f32_slice(at("u_person_hands").as_ref(), &people.hands);
            gl.uniform_4_f32_slice(at("u_person_feet").as_ref(), &people.feet);
            let faces = given.faces;
            gl.uniform_1_i32(at("u_face_count").as_ref(), faces.count);
            gl.uniform_4_f32_slice(at("u_faces").as_ref(), &faces.boxes);
            gl.uniform_2_f32_slice(at("u_face_points").as_ref(), &faces.points);
            gl.uniform_1_f32_slice(at("u_face_seen").as_ref(), &faces.seen);
            gl.uniform_4_f32_slice(at("u_face_turn").as_ref(), &faces.turn);
            gl.uniform_3_f32_slice(at("u_face_open").as_ref(), &faces.open);
            gl.uniform_1_f32_slice(at("u_face_mesh").as_ref(), &faces.mesh);
            gl.uniform_1_f32(at("u_has_face_map").as_ref(), if given.has_face_map { 1.0 } else { 0.0 });
            let restyled = effect.restyle && given.has_restyled;
            gl.uniform_1_i32(at("u_original").as_ref(), if restyled { given.original } else { 0 });
            gl.uniform_1_f32(at("u_has_restyled").as_ref(), if restyled { 1.0 } else { 0.0 });
            let drawn = if restyled { given.restyled_size } else { (0, 0) };
            gl.uniform_2_f32(at("u_restyled_size").as_ref(), drawn.0 as f32, drawn.1 as f32);
            for (name, value) in &effect.uniforms {
                gl.uniform_1_f32(at(name).as_ref(), *value);
            }
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
        self.drawn = self.current;
        if effect.feedback {
            self.current = 1 - self.current;
        }
        self.frame = self.frame.wrapping_add(1);
    }

    unsafe fn delete_program(&mut self, gl: &glow::Context) {
        if let Some(p) = self.program.take() {
            unsafe { gl.delete_program(p) };
        }
    }

    unsafe fn free_targets(&mut self, gl: &glow::Context) {
        for t in self.out.take().into_iter().flatten() {
            unsafe {
                delete_target(gl, t);
            }
        }
    }

    /// Back to holding nothing.
    unsafe fn free(&mut self, gl: &glow::Context) {
        unsafe {
            self.delete_program(gl);
            self.free_targets(gl);
        }
        *self = Self::default();
    }
}

pub(crate) unsafe fn compile(gl: &glow::Context, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
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

pub(crate) unsafe fn create_target(gl: &glow::Context, (w, h): (i32, i32), formats: &[(u32, u32)]) -> Result<Target, String> {
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
pub(crate) struct Fence(pub(crate) glow::Fence);

unsafe impl Send for Fence {}

/// The picture without black bars (`rect`, 0–1 from the bottom left) in pixels of a frame of
/// `size`, at least a pixel each way: x0, y0, x1, y1.
fn picture_box(rect: [f32; 4], size: (i32, i32)) -> [i32; 4] {
    let (x0, y0) = ((rect[0] * size.0 as f32) as i32, (rect[1] * size.1 as f32) as i32);
    let (x1, y1) = (((rect[2] * size.0 as f32) as i32).max(x0 + 1), ((rect[3] * size.1 as f32) as i32).max(y0 + 1));
    [x0, y0, x1, y1]
}

/// Draws `from`'s box `src` into `to`'s box `dst`, smoothly.
unsafe fn blit(gl: &glow::Context, from: glow::Framebuffer, src: [i32; 4], to: glow::Framebuffer, dst: [i32; 4]) {
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(from));
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(to));
        gl.blit_framebuffer(src[0], src[1], src[2], src[3], dst[0], dst[1], dst[2], dst[3], glow::COLOR_BUFFER_BIT, glow::LINEAR);
    }
}

/// What a frame is drawn small into to be read back, and the buffer it's read back through.
struct Capture {
    target: Target,
    pbo: glow::Buffer,
    size: (usize, usize),
}

/// Frames read back for a worker, a frame late so the CPU never waits for the GPU. `X` is
/// anything else that goes with a frame.
struct Readback<X = ()> {
    capture: Option<Capture>,
    /// The frame being read back: its fence, its position, and what goes with it.
    pending: Option<(Fence, f32, X)>,
    /// The position of the last frame read back, so a paused frame isn't read again and again.
    last_time: Option<f32>,
    /// The position of the last frame handed to the worker.
    handed: Option<f32>,
}

impl<X> Default for Readback<X> {
    fn default() -> Self {
        Self { capture: None, pending: None, last_time: None, handed: None }
    }
}

impl<X: Copy> Readback<X> {
    /// Makes sure frames are read back at `size`, letting go of anything at another size.
    unsafe fn ensure(&mut self, gl: &glow::Context, size: (usize, usize)) -> Result<(), String> {
        if self.capture.as_ref().is_some_and(|c| c.size == size) {
            return Ok(());
        }
        unsafe {
            self.free(gl);
            let target = create_target(gl, (size.0 as i32, size.1 as i32), &[(glow::RGBA8, glow::UNSIGNED_BYTE)])?;
            let pbo = match gl.create_buffer() {
                Ok(pbo) => pbo,
                Err(e) => {
                    delete_target(gl, target);
                    return Err(e);
                }
            };
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(pbo));
            gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, (size.0 * size.1 * 4) as i32, glow::STREAM_READ);
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            self.capture = Some(Capture { target, pbo, size });
        }
        Ok(())
    }

    /// Whether to read back the frame at `time`: nothing's being read back, and it's a new one.
    fn wants(&self, time: f32) -> bool {
        self.pending.is_none() && self.last_time != Some(time)
    }

    /// Starts reading back the frame at `time`, once `draw` has drawn it into the capture;
    /// what `draw` returns goes with it.
    unsafe fn read(&mut self, gl: &glow::Context, time: f32, draw: impl FnOnce(&Capture) -> X) {
        let Some(c) = &self.capture else { return };
        self.last_time = Some(time);
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            let extra = draw(c);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(c.target.fbo));
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(c.pbo));
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
            gl.read_pixels(0, 0, c.size.0 as i32, c.size.1 as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::BufferOffset(0));
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            if let Ok(fence) = gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
                self.pending = Some((Fence(fence), time, extra));
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }

    /// Starts reading back the frame at `time`: the box `from` of `video`, filling the capture.
    unsafe fn read_box(&mut self, gl: &glow::Context, video: glow::Framebuffer, from: [i32; 4], time: f32, extra: X) {
        let draw = |c: &Capture| {
            unsafe { blit(gl, video, from, c.target.fbo, [0, 0, c.size.0 as i32, c.size.1 as i32]) };
            extra
        };
        unsafe { self.read(gl, time, draw) }
    }

    /// Picks up the frame being read back, if it's there and `ready` says so for its position:
    /// `take` gets its pixels (RGBA, bottom row first), their size, its position and what goes
    /// with it.
    unsafe fn collect(&mut self, gl: &glow::Context, ready: impl FnOnce(f32) -> bool, take: impl FnOnce(&[u8], (usize, usize), f32, X)) {
        let Some((Fence(fence), time, extra)) = self.pending.filter(|p| ready(p.1)) else { return };
        unsafe {
            match gl.client_wait_sync(fence, 0, 0) {
                glow::TIMEOUT_EXPIRED => {}
                status => {
                    gl.delete_sync(fence);
                    self.pending = None;
                    if let (true, Some(c)) = (status != glow::WAIT_FAILED, &self.capture) {
                        let bytes = c.size.0 * c.size.1 * 4;
                        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(c.pbo));
                        let ptr = gl.map_buffer_range(glow::PIXEL_PACK_BUFFER, 0, bytes as i32, glow::MAP_READ_BIT);
                        if !ptr.is_null() {
                            take(std::slice::from_raw_parts(ptr, bytes), c.size, time, extra);
                            gl.unmap_buffer(glow::PIXEL_PACK_BUFFER);
                        }
                        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                    }
                }
            }
        }
    }

    /// Hands a network the frame read back, once the cut finder's seen it: `give` gets its
    /// pixels, their size, its position, whether the network should start again since the
    /// last one it was given, and what goes with it.
    unsafe fn hand_over(&mut self, gl: &glow::Context, cuts: &CutGl, give: impl FnOnce(Vec<u8>, (usize, usize), f32, bool, X)) {
        let handed = self.handed;
        let mut taken = None;
        unsafe {
            self.collect(gl, |time| cuts.has_seen(time), |pixels, size, time, extra| {
                taken = Some(time);
                give(pixels.to_vec(), size, time, cuts.reset_between(handed, time), extra);
            });
        }
        if taken.is_some() {
            self.handed = taken;
        }
    }

    /// Lets go of the capture, and any frame being read back.
    unsafe fn free(&mut self, gl: &glow::Context) {
        unsafe {
            if let Some((Fence(fence), _, _)) = self.pending.take() {
                gl.delete_sync(fence);
            }
            if let Some(c) = self.capture.take() {
                delete_target(gl, c.target);
                gl.delete_buffer(c.pbo);
            }
        }
    }

    /// Back to having read nothing back.
    unsafe fn release(&mut self, gl: &glow::Context) {
        unsafe { self.free(gl) };
        self.last_time = None;
    }
}

/// How late what a worker makes arrives after its frame, and how far the picture is held back to
/// match (seconds).
#[derive(Default)]
struct Lateness {
    arrived: u32,
    late: f32,
    delay: f32,
}

impl Lateness {
    /// Something made from the frame at `time` arrived at `clock`. `spare` is extra hold, as
    /// `late` is an average.
    fn arrived(&mut self, clock: f32, time: f32, spare: f32) {
        let late = clock - time;
        // Outside this, it's a seek and says nothing about how long the worker takes.
        if (0.0..0.5).contains(&late) {
            self.arrived += 1;
            self.late = if self.arrived == 1 { late } else { self.late + (late - self.late) * 0.1 };
            if self.arrived >= HOLD_AFTER_MAPS {
                self.delay = next_delay(self.delay, self.late + spare);
            }
        }
    }
}

/// Of things made from frames at these positions, the one nearest `time`, and how far it is.
pub(crate) fn nearest<T>(items: impl Iterator<Item = (T, f32)>, time: f32) -> Option<(T, f32)> {
    items.map(|(item, at)| (item, (at - time).abs())).min_by(|a, b| a.1.total_cmp(&b.1))
}

/// Things made from frames, in a ring, each with its frame's position once it's been filled.
struct TimedRing<T> {
    items: Vec<(T, Option<f32>)>,
    next: usize,
}

impl<T> Default for TimedRing<T> {
    fn default() -> Self {
        Self { items: Vec::new(), next: 0 }
    }
}

impl<T> TimedRing<T> {
    /// The one made from the frame nearest `time`, and its position, if it's within `reach`.
    fn nearest(&self, time: f32, reach: f32) -> Option<(&T, f32)> {
        let found = nearest(self.items.iter().filter_map(|(item, at)| at.map(|at| ((item, at), at))), time);
        found.filter(|(_, away)| *away <= reach).map(|(item, _)| item)
    }

    /// Which slot the next goes in, going round `most`.
    fn take_next(&mut self, most: usize) -> usize {
        let slot = self.next;
        self.next = (slot + 1) % most;
        slot
    }

    /// None is shown again until it's filled anew.
    fn forget(&mut self) {
        self.items.iter_mut().for_each(|item| item.1 = None);
    }

    /// Going backwards to `time`: what's kept is from somewhere else.
    fn forget_after(&mut self, time: f32) {
        if self.items.iter().any(|(_, at)| at.is_some_and(|at| at > time)) {
            self.forget();
        }
    }

    /// Hands back everything, empty.
    fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
        self.next = 0;
        self.items.drain(..).map(|(item, _)| item)
    }
}

/// Adds what's come in for a frame to `list` (oldest first, at most `most`), after letting go of
/// everything if it's gone backwards: what's kept is from somewhere else.
fn keep_in_order<T>(list: &mut VecDeque<T>, item: T, time: impl Fn(&T) -> f32, most: usize) {
    if list.back().is_some_and(|last| time(last) > time(&item)) {
        list.clear();
    }
    list.push_back(item);
    if list.len() > most {
        list.pop_front();
    }
}

/// Faces as the shader's uniforms.
#[derive(Clone, Copy)]
struct FaceUniforms {
    count: i32,
    boxes: [f32; crate::faces::MAX_FACES * 4],
    points: [f32; crate::faces::MAX_FACES * 10],
    seen: [f32; crate::faces::MAX_FACES],
    turn: [f32; crate::faces::MAX_FACES * 4],
    open: [f32; crate::faces::MAX_FACES * 3],
    mesh: [f32; crate::faces::MAX_FACES],
}

impl Default for FaceUniforms {
    fn default() -> Self {
        use crate::faces::MAX_FACES;
        Self { count: 0, boxes: [0.0; MAX_FACES * 4], points: [0.0; MAX_FACES * 10], seen: [0.0; MAX_FACES], turn: [0.0; MAX_FACES * 4], open: [0.0; MAX_FACES * 3], mesh: [0.0; MAX_FACES] }
    }
}

/// What one depth map comes in: for `u_depth`, `u_masks`, `u_normals` and `u_more_masks`, and
/// who's where in it.
#[derive(Clone, Copy)]
struct DepthTextures {
    depth: glow::Texture,
    masks: glow::Texture,
    normals: glow::Texture,
    more: glow::Texture,
    people: crate::people::People,
}

/// The people in a depth map, as `u_person_*` (see `HEADER`), on screen.
struct PeopleUniforms {
    count: i32,
    seen: [f32; MAX_PEOPLE],
    boxes: [f32; MAX_PEOPLE * 4],
    centres: [f32; MAX_PEOPLE * 2],
    heads: [f32; MAX_PEOPLE * 4],
    hands: [f32; MAX_PEOPLE * 8],
    feet: [f32; MAX_PEOPLE * 8],
}

use crate::people::MAX_PEOPLE;

impl Default for PeopleUniforms {
    fn default() -> Self {
        Self { count: 0, seen: [0.0; MAX_PEOPLE], boxes: [0.0; MAX_PEOPLE * 4], centres: [0.0; MAX_PEOPLE * 2], heads: [0.0; MAX_PEOPLE * 4], hands: [0.0; MAX_PEOPLE * 8], feet: [0.0; MAX_PEOPLE * 8] }
    }
}

impl PeopleUniforms {
    /// `people` (fractions of the picture, rows from the top) on screen, where the picture is
    /// `rect` (x0, y0, x1, y1 with y up).
    fn of(people: &crate::people::People, rect: [f32; 4]) -> Self {
        let mut u = Self::default();
        let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
        let at = |p: [f32; 2]| [rect[0] + p[0] * w, rect[1] + (1.0 - p[1]) * h];
        for (i, p) in people.iter().enumerate() {
            if p.seen <= 0.0 {
                continue;
            }
            u.count = i as i32 + 1;
            u.seen[i] = p.seen;
            let (a, b) = (at([p.bounds[0], p.bounds[3]]), at([p.bounds[2], p.bounds[1]]));
            u.boxes[i * 4..][..4].copy_from_slice(&[a[0], a[1], b[0], b[1]]);
            u.centres[i * 2..][..2].copy_from_slice(&at(p.centre));
            // Sizes in screen heights.
            let spot = |s: &crate::people::Spot| {
                let c = at(s.at);
                [c[0], c[1], s.size * h, s.seen]
            };
            u.heads[i * 4..][..4].copy_from_slice(&spot(&p.head));
            for (n, s) in p.hands.iter().enumerate() {
                u.hands[(i * 2 + n) * 4..][..4].copy_from_slice(&spot(s));
            }
            for (n, s) in p.feet.iter().enumerate() {
                u.feet[(i * 2 + n) * 4..][..4].copy_from_slice(&spot(s));
            }
        }
        u
    }
}

/// Depth maps kept, so a held-back frame can have the one made from it.
const DEPTH_MAPS: usize = 6;
/// The most the picture is held back (seconds).
const MOST_HELD: f32 = 0.25;
/// How much longer to hold than depth takes, and how far that has to drift before the hold
/// changes (moving it skips the picture and the sound a little).
const HOLD_MARGIN: f32 = 0.01;
const HOLD_STEP: f32 = 0.015;
/// Depth maps to see before trusting how long they take.
const HOLD_AFTER_MAPS: u32 = 20;
/// The most video frames kept, and the memory they can use.
const HELD_FRAMES: usize = 8;
const HELD_BYTES: usize = 256 << 20;

/// How long to hold the picture, given how late depth maps are arriving.
fn next_delay(current: f32, late: f32) -> f32 {
    let want = (late + HOLD_MARGIN).clamp(0.0, MOST_HELD);
    if (want - current).abs() > HOLD_STEP { want } else { current }
}

/// Of frames at these positions, newest first, the one to show at `at`: the newest that's
/// due, or the oldest if none are yet.
fn pick_frame(times: &[f32], at: f32) -> Option<usize> {
    times.iter().position(|t| *t <= at).or(times.len().checked_sub(1))
}

/// The last few video frames, so the picture can wait for depth.
#[derive(Default)]
struct HeldFrames {
    slots: Vec<Target>,
    /// Newest first: which slot, and the frame's position.
    frames: std::collections::VecDeque<(usize, f32)>,
}

impl HeldFrames {
    /// Keeps a copy of this frame, if it's a new one.
    unsafe fn push(&mut self, gl: &glow::Context, video: &Target, size: (i32, i32), pts: f32) {
        unsafe {
            let newest = self.frames.front().map(|f| f.1);
            if newest == Some(pts) {
                return;
            }
            // A seek: what's kept is from somewhere else.
            if newest.is_some_and(|n| pts < n || pts > n + 1.0) {
                self.frames.clear();
            }
            if self.slots.is_empty() {
                let most = (HELD_BYTES / (size.0.max(1) as usize * size.1.max(1) as usize * 4)).clamp(2, HELD_FRAMES);
                for _ in 0..most {
                    match create_target(gl, size, &[(glow::RGBA8, glow::UNSIGNED_BYTE)]) {
                        Ok(t) => self.slots.push(t),
                        Err(e) => {
                            log::warn!("video effects: holding frames back: {e}");
                            break;
                        }
                    }
                }
            }
            if self.slots.is_empty() {
                return;
            }
            let slot = if self.frames.len() == self.slots.len() { self.frames.pop_back().map_or(0, |f| f.0) } else { self.frames.len() };
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(video.fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.slots[slot].fbo));
            gl.blit_framebuffer(0, 0, size.0, size.1, 0, 0, size.0, size.1, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.frames.push_front((slot, pts));
        }
    }

    /// The newest frame kept, to read from, and its position.
    fn newest(&self) -> Option<(glow::Framebuffer, f32)> {
        self.frames.front().map(|&(slot, time)| (self.slots[slot].fbo, time))
    }

    /// The frame to show at `at`, and its position.
    fn pick(&self, at: f32) -> Option<(glow::Texture, f32)> {
        let times: Vec<f32> = self.frames.iter().map(|f| f.1).collect();
        pick_frame(&times, at).map(|i| (self.slots[self.frames[i].0].texture, self.frames[i].1))
    }

    unsafe fn free(&mut self, gl: &glow::Context) {
        self.frames.clear();
        for t in self.slots.drain(..) {
            unsafe {
                delete_target(gl, t);
            }
        }
    }
}

/// The furthest (seconds) a depth map is used from its frame: well past the time between maps
/// on a slow GPU, but a map from before a seek, or from a worker that's stopped, isn't kept on.
const DEPTH_REACH: f32 = 1.0;

/// Small frames read back for the depth worker, and the depth maps it sends back as textures.
#[derive(Default)]
struct DepthGl {
    readback: Readback,
    /// The latest depth maps and the video frames they're of.
    maps: TimedRing<DepthTextures>,
    failed: bool,
    /// Whether masks, normals and more masks came with the latest map.
    has_masks: bool,
    has_normals: bool,
    has_more_masks: bool,
    /// Where the ground is, from the newest depth map.
    ground: [f32; 4],
    /// New clues to the shot, and the positions they're from.
    clues: Vec<(f32, crate::shot_type::Clues)>,
    /// The newest depth map.
    serial: u64,
    lateness: Lateness,
}

impl DepthGl {
    /// The depth map, masks, normals and more masks made from the frame nearest `time`, if one's
    /// within `DEPTH_REACH` of it.
    #[allow(clippy::type_complexity)]
    fn map_for(&self, time: f32) -> (Option<glow::Texture>, Option<glow::Texture>, Option<glow::Texture>, Option<glow::Texture>) {
        match self.maps.nearest(time, DEPTH_REACH) {
            Some((m, _)) => (Some(m.depth), Some(m.masks), Some(m.normals), Some(m.more)),
            None => (None, None, None, None),
        }
    }

    /// Picks up a frame read back last time and hands it over, stores a new depth map, and
    /// reads back this frame if the worker's free. `time` is the frame's position and `clock`
    /// the smooth playback time.
    #[allow(clippy::too_many_arguments)]
    unsafe fn update(&mut self, gl: &glow::Context, link: &crate::depth::Link, cuts: &CutGl, video: glow::Framebuffer, size: (i32, i32), rect: [f32; 4], time: f32, clock: f32) {
        use crate::depth::{HEIGHT, WIDTH};
        unsafe {
            if self.failed {
                return;
            }
            if self.maps.items.is_empty() {
                match self.readback.ensure(gl, (WIDTH, HEIGHT)).and_then(|()| create_depth_maps(gl)) {
                    Ok(maps) => self.maps.items = maps.into_iter().map(|m| (m, None)).collect(),
                    Err(e) => {
                        log::warn!("depth: {e}");
                        self.readback.free(gl);
                        self.failed = true;
                        return;
                    }
                }
            }
            self.readback.hand_over(gl, cuts, |rgba, _, at, cut, ()| link.submit(rgba, at, cut));
            for map in link.take_maps() {
                // A map carried to a frame may be followed by a better one of that frame.
                let most = self.maps.items.len();
                let slot = self.maps.items.iter().position(|(_, at)| *at == Some(map.time)).unwrap_or_else(|| self.maps.take_next(most));
                let o = self.maps.items[slot].0;
                gl.bind_texture(glow::TEXTURE_2D, Some(o.depth));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
                let data: &[u8] = std::slice::from_raw_parts(map.values.as_ptr().cast(), map.values.len() * 4);
                let (w, h) = (WIDTH as i32, HEIGHT as i32);
                gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, w, h, glow::RED, glow::FLOAT, glow::PixelUnpackData::Slice(Some(data)));
                if let Some(masks) = &map.masks {
                    gl.bind_texture(glow::TEXTURE_2D, Some(o.masks));
                    gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, w, h, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(masks)));
                }
                self.has_masks = map.masks.is_some();
                if let Some(normals) = &map.normals {
                    gl.bind_texture(glow::TEXTURE_2D, Some(o.normals));
                    gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, w, h, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(normals)));
                }
                self.has_normals = map.normals.is_some();
                if let Some(more) = &map.more_masks {
                    gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(o.more));
                    let layers = crate::depth::MORE_LAYERS as i32;
                    gl.tex_sub_image_3d(glow::TEXTURE_2D_ARRAY, 0, 0, 0, 0, w, h, layers, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(more)));
                    gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
                }
                self.has_more_masks = map.more_masks.is_some();
                self.maps.items[slot].0.people = map.people;
                gl.bind_texture(glow::TEXTURE_2D, None);
                self.serial = map.serial;
                self.ground = map.ground;
                if let Some(clues) = map.clues {
                    self.clues.push((map.time, clues));
                }
                self.maps.items[slot].1 = Some(map.time);
                if !map.ahead {
                    self.lateness.arrived(clock, map.time, 0.0);
                }
            }
            if self.readback.wants(time) && link.ready_for_frame(cuts.reset_between(self.readback.handed, time)) {
                // Just the picture, without black bars.
                self.readback.read_box(gl, video, picture_box(rect, size), time, ());
            }
        }
    }

    unsafe fn release(&mut self, gl: &glow::Context) {
        unsafe {
            self.readback.release(gl);
            for o in self.maps.drain() {
                for texture in [o.depth, o.masks, o.normals, o.more] {
                    gl.delete_texture(texture);
                }
            }
        }
        self.has_masks = false;
        self.has_normals = false;
        self.has_more_masks = false;
        self.serial = 0;
        self.lateness = Lateness::default();
        self.failed = false;
    }
}

/// Restyled pictures kept, so a held-back frame can have the one made from it.
const RESTYLED: usize = 4;
/// The furthest (seconds) a restyled picture is moved along to a frame: well past the time
/// between pictures, but not so far that one from before a seek, or from a worker that's
/// stopped, is dragged about for long.
const RESTYLE_REACH: f32 = 0.5;

/// The fragment shader that moves a restyled picture along the movement to the frame shown,
/// leaving the black bars as they are.
fn restyle_source(version: &str, embedded: bool) -> String {
    let precision = if embedded { "precision highp float;\n" } else { "" };
    format!(
        "{version}\n{precision}uniform sampler2D u_video;\nuniform sampler2D u_styled;\nuniform sampler2D u_flow;\nuniform float u_has_flow;\nuniform vec4 u_rect;\nuniform float u_dt;\n\
         in vec2 v_uv;\nout vec4 flex_color;\n\
         void main() {{\n    vec2 p = (v_uv - u_rect.xy) / max(u_rect.zw - u_rect.xy, vec2(1e-6));\n\
         if (p != clamp(p, 0.0, 1.0)) {{ flex_color = texture(u_video, v_uv); return; }}\n\
         // What's here now was here less how far it's moved since the restyled frame.\n\
         vec2 moved = texture(u_flow, p).rg * u_has_flow * u_dt;\n\
         flex_color = vec4(texture(u_styled, clamp(p - moved, 0.0, 1.0)).rgb, 1.0);\n}}\n"
    )
}

/// Frames read back for the restyling worker, at the size it asks for, the restyled pictures it
/// sends back, and the one for the frame shown, moved along with the picture to it.
#[derive(Default)]
struct RestyleGl {
    readback: Readback,
    failed: bool,
    /// Restyled pictures: texture and size, with the position of their frame.
    pictures: TimedRing<(glow::Texture, (usize, usize))>,
    /// The style they're in, by id.
    style: String,
    /// The restyled picture for the frame shown, the size of the video, and the size the
    /// network drew it at.
    out: Option<(Target, (i32, i32))>,
    shown_size: (usize, usize),
    program: Option<glow::Program>,
    lateness: Lateness,
}

impl RestyleGl {
    /// Picks up a frame read back last time and hands it over, stores new restyled pictures, and
    /// reads back this frame if the worker's free. As `DepthGl::update`.
    #[allow(clippy::too_many_arguments)]
    unsafe fn update(&mut self, gl: &glow::Context, link: &crate::restyle::Link, cuts: &CutGl, video: glow::Framebuffer, size: (i32, i32), rect: [f32; 4], time: f32, clock: f32) {
        unsafe {
            if self.failed {
                return;
            }
            self.readback.hand_over(gl, cuts, |rgba, size, at, cut, ()| link.submit(rgba, size, at, cut));
            // Pictures in another style aren't shown once a new one's wanted.
            let style = link.style();
            if style != self.style {
                self.pictures.forget();
                self.style = style;
            }
            for picture in link.take() {
                if picture.style != self.style {
                    continue;
                }
                let slot = self.pictures.take_next(RESTYLED);
                if slot == self.pictures.items.len() {
                    match gl.create_texture() {
                        Ok(t) => self.pictures.items.push(((t, (0, 0)), None)),
                        Err(e) => {
                            log::warn!("restyle: {e}");
                            self.failed = true;
                            return;
                        }
                    }
                }
                let ((texture, kept), at) = &mut self.pictures.items[slot];
                let (w, h) = (picture.size.0 as i32, picture.size.1 as i32);
                gl.bind_texture(glow::TEXTURE_2D, Some(*texture));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
                let data = glow::PixelUnpackData::Slice(Some(&picture.rgba));
                if *kept != picture.size {
                    gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, data);
                    for (param, value) in [(glow::TEXTURE_MIN_FILTER, glow::LINEAR), (glow::TEXTURE_MAG_FILTER, glow::LINEAR), (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE), (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE)] {
                        gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
                    }
                    *kept = picture.size;
                } else {
                    gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, w, h, glow::RGBA, glow::UNSIGNED_BYTE, data);
                }
                gl.bind_texture(glow::TEXTURE_2D, None);
                *at = Some(picture.time);
                self.lateness.arrived(clock, picture.time, 0.0);
            }
            if self.readback.wants(time) && link.ready_for_frame(cuts.reset_between(self.readback.handed, time)) {
                // Just the picture, without black bars, at the size the worker wants for its shape.
                let from = picture_box(rect, size);
                let wanted = link.size_for((from[2] - from[0]) as f32 / (from[3] - from[1]) as f32);
                if let Err(e) = self.readback.ensure(gl, wanted) {
                    log::warn!("restyle: {e}");
                    self.failed = true;
                    return;
                }
                self.readback.read_box(gl, video, from, time, ());
            }
        }
    }

    /// The restyled picture for the frame at `time`, the size of the video (`size`): the one
    /// nearest it, moved along `flow` (if there's any) to it. None until there is one within
    /// `RESTYLE_REACH`.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw(&mut self, gl: &glow::Context, version: egui_glow::ShaderVersion, vao: glow::VertexArray, video: glow::Texture, flow: Option<glow::Texture>, size: (i32, i32), rect: [f32; 4], time: f32) -> Option<glow::Texture> {
        unsafe {
            let (&(texture, drawn), at) = self.pictures.nearest(time, RESTYLE_REACH)?;
            self.shown_size = drawn;
            if self.program.is_none() {
                let decl = version.version_declaration();
                match compile(gl, &vertex_source(decl), &restyle_source(decl, version.is_embedded())) {
                    Ok(p) => self.program = Some(p),
                    Err(e) => {
                        log::warn!("restyle: {e}");
                        self.failed = true;
                        return None;
                    }
                }
            }
            if self.out.as_ref().is_none_or(|(_, s)| *s != size) {
                if let Some((t, _)) = self.out.take() {
                    delete_target(gl, t);
                }
                match create_target(gl, size, &[(glow::RGBA8, glow::UNSIGNED_BYTE)]) {
                    Ok(t) => self.out = Some((t, size)),
                    Err(e) => {
                        log::warn!("restyle: {e}");
                        self.failed = true;
                        return None;
                    }
                }
            }
            let (Some(program), Some((out, _))) = (self.program, &self.out) else { return None };
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(out.fbo));
            gl.viewport(0, 0, size.0, size.1);
            gl.use_program(Some(program));
            gl.bind_vertex_array(Some(vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(video));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, flow);
            let u = |name: &str| gl.get_uniform_location(program, name);
            gl.uniform_1_i32(u("u_video").as_ref(), 0);
            gl.uniform_1_i32(u("u_styled").as_ref(), 1);
            gl.uniform_1_i32(u("u_flow").as_ref(), 2);
            gl.uniform_1_f32(u("u_has_flow").as_ref(), if flow.is_some() { 1.0 } else { 0.0 });
            gl.uniform_4_f32(u("u_rect").as_ref(), rect[0], rect[1], rect[2], rect[3]);
            gl.uniform_1_f32(u("u_dt").as_ref(), time - at);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            Some(out.texture)
        }
    }

    unsafe fn release(&mut self, gl: &glow::Context) {
        unsafe {
            self.readback.release(gl);
            for (texture, _) in self.pictures.drain() {
                gl.delete_texture(texture);
            }
            if let Some((t, _)) = self.out.take() {
                delete_target(gl, t);
            }
            if let Some(p) = self.program.take() {
                gl.delete_program(p);
            }
        }
        self.lateness = Lateness::default();
        self.readback.handed = None;
        self.failed = false;
    }
}

/// Small frames read back for the face worker, and what it finds, eased from one report to
/// the next so faces glide at the screen's rate.
#[derive(Default)]
struct FaceGl {
    /// With each frame, how much of the capture the picture fills.
    readback: Readback<(f32, f32)>,
    failed: bool,
    /// Each face's mesh, a row each, for `u_mesh`.
    mesh_texture: Option<glow::Texture>,
    /// That mesh drawn into a map of the picture.
    map: Option<crate::face_map::FaceMap>,
    map_failed: bool,
    /// What's come in lately, oldest first.
    reports: VecDeque<crate::faces::Report>,
    lateness: Lateness,
}

/// How long a camera report can be used from the frame it's of (seconds), and how many are kept.
const CAMERA_REACH: f32 = 0.2;
const CAMERA_KEPT: usize = 48;
/// The width frames are followed at, in pixels.
const CAMERA_WIDTH: usize = 640;

/// Gets frames to the camera worker, and keeps what it finds.
#[derive(Default)]
struct CameraGl {
    readback: Readback,
    failed: bool,
    reports: VecDeque<crate::camera::Report>,
    lateness: Lateness,
}

impl CameraGl {
    /// The report for the frame nearest `time`.
    fn at(&self, time: f32) -> Option<crate::camera::Report> {
        nearest(self.reports.iter().map(|r| (r, r.time)), time).filter(|(_, away)| *away < CAMERA_REACH).map(|(r, _)| *r)
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn update(&mut self, gl: &glow::Context, link: &crate::camera::Link, cuts: &CutGl, video: glow::Framebuffer, size: (i32, i32), rect: [f32; 4], time: f32, clock: f32) {
        unsafe {
            if self.failed {
                return;
            }
            self.readback.hand_over(gl, cuts, |rgba, size, at, cut, ()| link.submit(rgba, size, at, cut));
            for report in link.take_reports() {
                self.lateness.arrived(clock, report.time, MOTION_SPARE);
                keep_in_order(&mut self.reports, report, |r| r.time, CAMERA_KEPT);
            }
            if self.readback.wants(time) && link.ready_for_frame() {
                let aspect = (rect[2] - rect[0]) * size.0 as f32 / ((rect[3] - rect[1]) * size.1 as f32).max(1.0);
                let want = (CAMERA_WIDTH, (((CAMERA_WIDTH as f32 / aspect.max(0.1)) / 2.0).round() as usize * 2).clamp(64, 1024));
                if let Err(e) = self.readback.ensure(gl, want) {
                    log::warn!("camera: {e}");
                    self.failed = true;
                    return;
                }
                self.readback.read_box(gl, video, picture_box(rect, size), time, ());
            }
        }
    }

    unsafe fn release(&mut self, gl: &glow::Context) {
        unsafe { self.readback.release(gl) };
        self.reports.clear();
        self.lateness = Lateness::default();
        self.failed = false;
    }
}

/// How many frames' movement are kept, and the furthest (seconds) a frame's can be used from.
const MOTION_KEPT: usize = 12;
const MOTION_REACH: f32 = 0.25;
/// Extra hold for movement, as `late` is an average.
const MOTION_SPARE: f32 = 0.02;

/// Gets frames to the movement worker, and what it finds back into textures for effects.
#[derive(Default)]
struct NetFlowGl {
    readback: Readback,
    failed: bool,
    /// A texture of movement for each frame.
    kept: TimedRing<Target>,
    lateness: Lateness,
    /// What the newest movement says about the camera, and how fast things are going in it.
    fit: Option<(crate::shot_type::Fit, f32)>,
}

impl NetFlowGl {
    #[allow(clippy::too_many_arguments)]
    unsafe fn update(&mut self, gl: &glow::Context, link: &crate::flow::Link, cuts: &CutGl, video: glow::Framebuffer, size: (i32, i32), rect: [f32; 4], time: f32, clock: f32) {
        use crate::flow::{FLOW_HEIGHT, FLOW_WIDTH, HEIGHT, WIDTH};
        unsafe {
            if self.failed {
                return;
            }
            if self.kept.items.is_empty() {
                let float = [(glow::RGBA16F, glow::HALF_FLOAT)];
                let made = self.readback.ensure(gl, (WIDTH, HEIGHT)).and_then(|()| {
                    let mut kept = Vec::with_capacity(MOTION_KEPT);
                    for _ in 0..MOTION_KEPT {
                        match create_target(gl, (FLOW_WIDTH as i32, FLOW_HEIGHT as i32), &float) {
                            Ok(target) => kept.push((target, None)),
                            Err(e) => {
                                // Nothing made is kept.
                                for (target, _) in kept {
                                    delete_target(gl, target);
                                }
                                return Err(e);
                            }
                        }
                    }
                    Ok(kept)
                });
                match made {
                    Ok(kept) => self.kept.items = kept,
                    Err(e) => {
                        log::warn!("flow: {e}");
                        self.readback.free(gl);
                        self.failed = true;
                        return;
                    }
                }
            }
            self.readback.hand_over(gl, cuts, |rgba, _, at, cut, ()| link.submit(rgba, at, cut));
            for report in link.take_reports() {
                self.lateness.arrived(clock, report.time, MOTION_SPARE);
                self.kept.forget_after(report.time);
                if let Some(flow) = &report.flow {
                    let aspect = (rect[2] - rect[0]) * size.0 as f32 / ((rect[3] - rect[1]) * size.1 as f32).max(1.0);
                    self.fit = Some((crate::shot_type::fit(flow, (FLOW_WIDTH, FLOW_HEIGHT), aspect), crate::depth::fast_speed(flow)));
                }
                let texels = crate::flow::texels(report.flow.as_deref());
                let slot = self.kept.take_next(MOTION_KEPT);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.kept.items[slot].0.texture));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
                let data: &[u8] = std::slice::from_raw_parts(texels.as_ptr().cast(), texels.len() * 4);
                gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, FLOW_WIDTH as i32, FLOW_HEIGHT as i32, glow::RGBA, glow::FLOAT, glow::PixelUnpackData::Slice(Some(data)));
                gl.bind_texture(glow::TEXTURE_2D, None);
                self.kept.items[slot].1 = Some(report.time);
            }
            if self.readback.wants(time) && link.ready_for_frame() {
                // The picture without black bars, squeezed to fill it, as flow is in fractions
                // of the picture. In GL's rows, as everything else here.
                self.readback.read_box(gl, video, picture_box(rect, size), time, ());
            }
        }
    }

    /// The movement worked out for the frame nearest `time`, if it's near enough.
    fn at(&self, time: f32) -> Option<glow::Texture> {
        self.kept.nearest(time, MOTION_REACH).map(|(target, _)| target.texture)
    }

    unsafe fn release(&mut self, gl: &glow::Context) {
        unsafe {
            self.readback.release(gl);
            for target in self.kept.drain() {
                delete_target(gl, target);
            }
        }
        self.failed = false;
        self.lateness = Lateness::default();
    }
}

/// The tiny picture compared from frame to frame to find cuts.
const CUT_SIZE: (i32, i32) = (crate::signals::STATS_SIZE.0 as i32, crate::signals::STATS_SIZE.1 as i32);
/// A halfway size, as one big step down would only look at a few pixels of the frame.
const CUT_HALFWAY: (i32, i32) = (128, 72);
/// A cut changes the picture this much on average (0–255 a channel), and this many times as much
/// as the frames before it changed, which rules out fast action.
const CUT_CHANGE: f32 = 20.0;
const CUT_OVER_USUAL: f32 = 4.0;
/// A change this big is enough for the networks to start again.
const RESET_CHANGE: f32 = 10.0;

/// Spots a sudden change in the picture.
#[derive(Default)]
struct CutFinder {
    last: Vec<u8>,
    last_time: f32,
    usual: f32,
    was_cut: bool,
    was_reset: bool,
}

impl CutFinder {
    /// `rgba` is this frame's tiny picture, from `time` in the video. Says whether it's a cut,
    /// and whether the networks should start again, which also goes for the smaller changes
    /// dark films cut with: wrongly forgetting a frame costs them little.
    fn look(&mut self, rgba: &[u8], time: f32) -> (bool, bool) {
        // A seek: not the film cutting.
        let continues = self.last.len() == rgba.len() && time >= self.last_time && time < self.last_time + 1.0;
        let (mut cut, mut reset) = (false, false);
        if continues {
            let change: u32 = self.last.as_chunks::<4>().0.iter().zip(rgba.as_chunks::<4>().0).map(|(a, b)| (0..3).map(|c| u32::from(a[c].abs_diff(b[c]))).sum::<u32>()).sum();
            let change = change as f32 / (rgba.len() / 4 * 3) as f32;
            cut = !self.was_cut && change > CUT_CHANGE && change > self.usual * CUT_OVER_USUAL;
            reset = !self.was_reset && change > RESET_CHANGE && change > self.usual * CUT_OVER_USUAL;
            if !cut && !reset {
                self.usual += (change - self.usual) * 0.25;
            }
        } else {
            self.usual = 0.0;
        }
        self.last.clear();
        self.last.extend_from_slice(rgba);
        self.last_time = time;
        self.was_cut = cut;
        self.was_reset = reset;
        (cut, reset)
    }
}

/// Reads back a tiny copy of each new frame (a frame late, so the CPU never waits for the GPU).
#[derive(Default)]
struct CutGl {
    /// The tiny copy, and a halfway size it's made through.
    readback: Readback,
    halfway: Option<Target>,
    failed: bool,
    finder: CutFinder,
    /// The newest cut found that hasn't reached the screen yet.
    due: Option<f32>,
    /// Cuts found lately, and one not dealt with yet.
    found: Cuts,
    fresh: Option<f32>,
    /// Those, and the smaller changes the networks start again at too.
    resets: Cuts,
    /// What the picture was like when last looked at.
    stats: Stats,
    seen_at: Option<f32>,
}

impl CutGl {
    /// Whether this has seen the frame at `time` yet, so a network can be told about it.
    fn has_seen(&self, time: f32) -> bool {
        self.failed || self.halfway.is_none() || self.seen_at.is_some_and(|seen| seen >= time)
    }

    /// Whether the networks should start again at the frame at `time`, having last been given
    /// the one at `after`.
    fn reset_between(&self, after: Option<f32>, time: f32) -> bool {
        after.is_some_and(|after| self.resets.between(after, time))
    }

    unsafe fn update(&mut self, gl: &glow::Context, video: glow::Framebuffer, size: (i32, i32), rect: [f32; 4], time: f32) {
        unsafe {
            if self.failed {
                return;
            }
            if self.halfway.is_none() {
                let made = self.readback.ensure(gl, (CUT_SIZE.0 as usize, CUT_SIZE.1 as usize)).and_then(|()| create_target(gl, CUT_HALFWAY, &[(glow::RGBA8, glow::UNSIGNED_BYTE)]));
                match made {
                    Ok(halfway) => self.halfway = Some(halfway),
                    Err(e) => {
                        log::warn!("scene cuts: {e}");
                        self.readback.free(gl);
                        self.failed = true;
                        return;
                    }
                }
            }
            let Some(halfway) = self.halfway.as_ref() else { return };
            self.readback.collect(gl, |_| true, |tiny, _, at, ()| {
                if self.seen_at.is_some_and(|last| at < last || at > last + 1.0) {
                    self.found.clear();
                    self.resets.clear();
                }
                self.seen_at = Some(at);
                let (cut, reset) = self.finder.look(tiny, at);
                if cut {
                    log::debug!("scene cut at {at:.3}");
                    self.due = Some(at);
                    self.found.add(at);
                    self.fresh = Some(at);
                }
                if reset || cut {
                    self.resets.add(at);
                }
                self.stats = self.stats.measure(tiny);
            });
            if self.readback.wants(time) {
                let halfway_box = [0, 0, CUT_HALFWAY.0, CUT_HALFWAY.1];
                self.readback.read(gl, time, |small| {
                    blit(gl, video, picture_box(rect, size), halfway.fbo, halfway_box);
                    blit(gl, halfway.fbo, halfway_box, small.target.fbo, [0, 0, CUT_SIZE.0, CUT_SIZE.1]);
                });
            }
        }
    }

    unsafe fn release(&mut self, gl: &glow::Context) {
        unsafe {
            self.readback.release(gl);
            if let Some(halfway) = self.halfway.take() {
                delete_target(gl, halfway);
            }
        }
        self.failed = false;
        self.finder = CutFinder::default();
        self.due = None;
        self.found.clear();
        self.resets.clear();
        self.fresh = None;
        self.seen_at = None;
    }
}

/// Extra hold for faces, as `late` is an average and a frame shown before its report has to
/// make do with the last frame's.
const FACE_SPARE: f32 = 0.025;

/// How many reports are kept to draw from: over a second's worth.
const FACE_REPORTS: usize = 32;

/// The faces at `time`, between the reports either side of it.
fn faces_between(reports: &VecDeque<crate::faces::Report>, time: f32) -> crate::faces::Slots {
    let none = [None; crate::faces::MAX_FACES];
    let (before, after) = match reports.iter().position(|r| r.time > time) {
        None => return reports.back().map_or(none, |r| r.slots),
        Some(0) => return reports[0].slots,
        Some(i) => (&reports[i - 1], &reports[i]),
    };
    if after.after_cut {
        return before.slots;
    }
    let along = (time - before.time) / (after.time - before.time).max(1e-6);
    let mut out = none;
    for (out, (a, b)) in out.iter_mut().zip(before.slots.iter().zip(&after.slots)) {
        *out = match (a, b) {
            (Some(a), Some(b)) => {
                use crate::faces::{HAS_MESH, HAS_TURN, MESH_AT, TURN_AT};
                let mut mixed = *a;
                for (n, (v, to)) in mixed.coords.iter_mut().zip(b.coords).enumerate() {
                    // A part only one side has isn't blended with the other's nothing.
                    let (from_has, to_has) = match n {
                        n if (MESH_AT..TURN_AT).contains(&n) => (a.coords[HAS_MESH] > 0.0, b.coords[HAS_MESH] > 0.0),
                        n if (TURN_AT..HAS_MESH).contains(&n) => (a.coords[HAS_TURN] > 0.0, b.coords[HAS_TURN] > 0.0),
                        _ => (true, true),
                    };
                    match (from_has, to_has) {
                        (false, true) => *v = to,
                        (true, false) => {}
                        _ => *v += (to - *v) * along,
                    }
                }
                // Found: blended in. Lost: gone halfway between, rather than fading.
                mixed.seen = if b.seen >= a.seen { a.seen + (b.seen - a.seen) * along } else if along < 0.5 { a.seen } else { b.seen };
                Some(mixed)
            }
            (a, b) => a.or(*b),
        };
    }
    out
}

impl FaceGl {
    /// Picks up a frame read back last time and hands it over, collects new reports, and reads
    /// back this frame if the worker's free. `time` is the frame's position and `clock` the
    /// smooth playback time.
    #[allow(clippy::too_many_arguments)]
    unsafe fn update(&mut self, gl: &glow::Context, link: &crate::faces::Link, cuts: &CutGl, video: glow::Framebuffer, size: (i32, i32), rect: [f32; 4], time: f32, clock: f32) {
        unsafe {
            if self.failed {
                return;
            }
            // How many times the detector's size frames are read back at.
            let scale = link.scale();
            let (width, height) = (crate::faces::WIDTH * scale, crate::faces::HEIGHT * scale);
            if let Err(e) = self.readback.ensure(gl, (width, height)) {
                log::warn!("faces: {e}");
                self.failed = true;
                return;
            }
            self.readback.hand_over(gl, cuts, |rgba, _, at, cut, filled| link.submit(rgba, scale, filled, at, cut));
            for report in link.take_reports() {
                self.lateness.arrived(clock, report.time, FACE_SPARE);
                keep_in_order(&mut self.reports, report, |r| r.time, FACE_REPORTS);
            }
            if self.readback.wants(time) && link.ready_for_frame() {
                // The picture without black bars, its shape kept and the rest left black: a
                // squashed face is harder to find. In GL's rows, the top of the picture is
                // at the top of the capture.
                let [x0, y0, x1, y1] = picture_box(rect, size);
                let fit = (width as f32 / (x1 - x0) as f32).min(height as f32 / (y1 - y0) as f32);
                let (w, h) = ((((x1 - x0) as f32 * fit) as i32).clamp(1, width as i32), (((y1 - y0) as f32 * fit) as i32).clamp(1, height as i32));
                self.readback.read(gl, time, |capture| {
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(capture.target.fbo));
                    gl.clear_color(0.0, 0.0, 0.0, 1.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);
                    blit(gl, video, [x0, y0, x1, y1], capture.target.fbo, [0, height as i32 - h, w, height as i32]);
                    (w as f32 / width as f32, h as f32 / height as f32)
                });
            }
        }
    }

    /// The faces in the frame at `time`, as the shader wants them.
    fn uniforms(&self, time: f32, rect: [f32; 4]) -> FaceUniforms {
        let mut out = FaceUniforms::default();
        for (i, face) in faces_between(&self.reports, time).iter().enumerate() {
            let Some(face) = face else { continue };
            let (width, height) = (rect[2] - rect[0], rect[3] - rect[1]);
            let x = |v: f32| rect[0] + v * width;
            let y = |v: f32| rect[3] - v * height;
            out.boxes[i * 4..i * 4 + 4].copy_from_slice(&[x(face.coords[0]), y(face.coords[3]), x(face.coords[2]), y(face.coords[1])]);
            for p in 0..5 {
                out.points[(i * 5 + p) * 2] = x(face.coords[4 + p * 2]);
                out.points[(i * 5 + p) * 2 + 1] = y(face.coords[5 + p * 2]);
            }
            out.seen[i] = face.seen;
            use crate::faces::{HAS_MESH, HAS_TURN, OPEN_AT, TURN_AT};
            let known = face.coords[HAS_TURN];
            let forward = &face.coords[TURN_AT..TURN_AT + 3];
            let length = forward.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
            out.turn[i * 4..i * 4 + 4].copy_from_slice(&[forward[0] / length, forward[1] / length, forward[2] / length, known]);
            out.open[i * 3..i * 3 + 3].copy_from_slice(&face.coords[OPEN_AT..OPEN_AT + 3]);
            out.mesh[i] = face.coords[HAS_MESH];
            out.count = i as i32 + 1;
        }
        out
    }

    /// Puts the meshes for the frame at `time` where `mesh_point` finds them.
    unsafe fn upload_mesh(&mut self, gl: &glow::Context, time: f32, rect: [f32; 4]) {
        use crate::faces::{MAX_FACES, MESH_AT, MESH_POINTS};
        unsafe {
            if self.mesh_texture.is_none() {
                let Ok(texture) = gl.create_texture() else { return };
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA32F as i32, MESH_POINTS as i32, MAX_FACES as i32, 0, glow::RGBA, glow::FLOAT, glow::PixelUnpackData::Slice(None));
                self.mesh_texture = Some(texture);
            }
            let mut data = vec![0.0f32; MESH_POINTS * MAX_FACES * 4];
            for (i, face) in faces_between(&self.reports, time).iter().enumerate() {
                let Some(face) = face else { continue };
                for n in 0..MESH_POINTS {
                    let p = &face.coords[MESH_AT + n * 3..MESH_AT + n * 3 + 3];
                    let at = (i * MESH_POINTS + n) * 4;
                    data[at] = rect[0] + p[0] * (rect[2] - rect[0]);
                    data[at + 1] = rect[3] - p[1] * (rect[3] - rect[1]);
                    data[at + 2] = p[2];
                    data[at + 3] = 1.0;
                }
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, self.mesh_texture);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            let bytes = std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 4);
            gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, MESH_POINTS as i32, MAX_FACES as i32, glow::RGBA, glow::FLOAT, glow::PixelUnpackData::Slice(Some(bytes)));
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }

    /// Draws the faces in `shown` into the map, returning whether there is one.
    unsafe fn draw_map(&mut self, gl: &glow::Context, version: &str, embedded: bool, video: glow::Texture, size: (i32, i32), shown: &FaceUniforms) -> bool {
        unsafe {
            let Some(mesh) = self.mesh_texture else { return false };
            if self.map.is_none() && !self.map_failed {
                match crate::face_map::FaceMap::new(gl, version, embedded) {
                    Ok(map) => self.map = Some(map),
                    Err(e) => {
                        log::warn!("face map: {e}");
                        self.map_failed = true;
                    }
                }
            }
            let Some(map) = &self.map else { return false };
            let faces: Vec<(usize, f32)> = (0..shown.count as usize).map(|i| (i, shown.mesh[i] * shown.seen[i])).filter(|(_, fade)| *fade > 0.01).collect();
            map.draw(gl, mesh, video, size.0 as f32 / size.1.max(1) as f32, &faces);
            true
        }
    }

    unsafe fn release(&mut self, gl: &glow::Context) {
        unsafe {
            self.readback.release(gl);
            if let Some(texture) = self.mesh_texture.take() {
                gl.delete_texture(texture);
            }
            if let Some(map) = self.map.take() {
                map.release(gl);
            }
        }
        self.map_failed = false;
        self.failed = false;
        self.reports.clear();
        self.lateness = Lateness::default();
    }
}

/// Textures for `DEPTH_MAPS` depth maps, starting empty.
unsafe fn create_depth_maps(gl: &glow::Context) -> Result<Vec<DepthTextures>, String> {
    use crate::depth::{HEIGHT, WIDTH};
    unsafe {
        let size = (WIDTH as i32, HEIGHT as i32);
        let zeros = vec![0u8; WIDTH * HEIGHT * 4];
        let mut made = Vec::new();
        let mut arrays = Vec::new();
        let mut texture = |internal: u32, format: u32, kind: u32| -> Result<glow::Texture, String> {
            let texture = gl.create_texture()?;
            made.push(texture);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, internal as i32, size.0, size.1, 0, format, kind, glow::PixelUnpackData::Slice(Some(&zeros)));
            for (param, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
            }
            Ok(texture)
        };
        let maps: Result<Vec<DepthTextures>, String> = (0..DEPTH_MAPS)
            .map(|_| {
                let mut rgba = || texture(glow::RGBA8, glow::RGBA, glow::UNSIGNED_BYTE);
                let (masks, normals) = (rgba()?, rgba()?);
                let depth = texture(glow::R16F, glow::RED, glow::FLOAT)?;
                let more = gl.create_texture()?;
                arrays.push(more);
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(more));
                let layers = crate::depth::MORE_LAYERS;
                let zeros = vec![0u8; WIDTH * HEIGHT * 4 * layers];
                gl.tex_image_3d(glow::TEXTURE_2D_ARRAY, 0, glow::RGBA8 as i32, size.0, size.1, layers as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&zeros)));
                for (param, value) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, param, value as i32);
                }
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
                Ok(DepthTextures { masks, normals, more, depth, people: Default::default() })
            })
            .collect();
        gl.bind_texture(glow::TEXTURE_2D, None);
        if maps.is_err() {
            // Nothing made is kept.
            for texture in made.into_iter().chain(arrays) {
                gl.delete_texture(texture);
            }
        }
        maps
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
    fn the_header_declares_every_kind_of_sound() {
        for name in SOUND_UNIFORMS.iter() {
            assert!(HEADER.contains(&format!("uniform float {name};\n")), "{name}");
        }
        assert_eq!(HEADER.matches("uniform float u_sound_").count(), SOUND_UNIFORMS.len());
    }

    #[test]
    fn status_takes_the_first_line_of_the_log() {
        assert_eq!(first_line("\n  0:3(12): error: syntax error\n0:4: more\n"), "0:3(12): error: syntax error");
        assert_eq!(first_line(""), "unknown error");
    }
}

#[cfg(test)]
mod hold_tests {
    use super::*;

    fn report(time: f32, x: f32, after_cut: bool) -> crate::faces::Report {
        let mut slots = [None; crate::faces::MAX_FACES];
        slots[0] = Some(crate::faces::Shown { coords: [x; crate::faces::COORDS], seen: 1.0 });
        crate::faces::Report { time, after_cut, slots }
    }

    #[test]
    fn a_sudden_change_is_a_cut_but_steady_action_and_seeks_are_not() {
        let frame = |v: u8| vec![v; 64];
        let mut f = CutFinder::default();
        assert!(!f.look(&frame(50), 0.0).0, "nothing to compare with");
        assert!(!f.look(&frame(52), 0.04).0);
        assert!(f.look(&frame(150), 0.08).0);
        assert!(!f.look(&frame(151), 0.12).0);
        assert!(!f.look(&frame(250), 5.0).0, "a seek");
        // Everything changing a lot, frame after frame.
        let mut f = CutFinder::default();
        let mut cuts = 0;
        for i in 0..60 {
            cuts += u32::from(f.look(&frame(if i % 2 == 0 { 40 } else { 80 }), i as f32 * 0.04).0);
        }
        assert!(cuts <= 1, "{cuts}");
    }

    #[test]
    fn the_networks_start_again_at_smaller_changes_than_effects_call_a_cut() {
        let frame = |v: u8| vec![v; 64];
        let mut f = CutFinder::default();
        f.look(&frame(20), 0.0);
        f.look(&frame(20), 0.04);
        assert_eq!(f.look(&frame(35), 0.08), (false, true), "dark films cut like this");
    }

    #[test]
    fn faces_are_drawn_where_they_were_at_that_frame() {
        let reports = VecDeque::from([report(1.0, 0.2, false), report(2.0, 0.4, false), report(3.0, 0.9, true)]);
        let x = |t: f32| faces_between(&reports, t)[0].unwrap().coords[0];
        assert!((x(1.5) - 0.3).abs() < 1e-5, "between two reports");
        assert_eq!(x(0.5), 0.2, "before them all");
        assert_eq!(x(4.0), 0.9, "after them all");
        assert_eq!(x(2.5), 0.4, "not across a cut");
        assert!(faces_between(&VecDeque::new(), 1.0)[0].is_none());
    }

    #[test]
    fn the_newest_frame_that_is_due_is_shown() {
        let newest_first = [1.25, 1.2, 1.15, 1.1];
        assert_eq!(pick_frame(&newest_first, 1.22), Some(1));
        assert_eq!(pick_frame(&newest_first, 2.0), Some(0));
        // Nothing's due yet: the oldest there is.
        assert_eq!(pick_frame(&newest_first, 1.0), Some(3));
        assert_eq!(pick_frame(&[], 1.0), None);
    }

    #[test]
    fn every_unit_is_its_own_number_and_name() {
        for (i, unit) in Unit::ALL.into_iter().enumerate() {
            assert_eq!(unit as usize, i);
            assert_eq!(Unit::ALL.iter().filter(|u| u.uniform() == unit.uniform()).count(), 1);
        }
        assert_eq!(ORIGINAL_UNIT as usize, Unit::ALL.len());
    }

    #[test]
    fn the_nearest_is_found_within_reach() {
        let mut ring = TimedRing { items: vec![("a", Some(1.0)), ("b", Some(2.0)), ("c", None)], next: 0 };
        assert_eq!(ring.nearest(1.7, 0.5).map(|(x, at)| (*x, at)), Some(("b", 2.0)));
        assert!(ring.nearest(3.0, 0.5).is_none());
        ring.forget_after(1.5);
        assert!(ring.nearest(1.0, 1.0).is_none(), "gone backwards: all forgotten");
        let mut list = VecDeque::from([1.0, 2.0]);
        keep_in_order(&mut list, 3.0, |t| *t, 2);
        assert_eq!(list, [2.0, 3.0]);
        keep_in_order(&mut list, 0.5, |t| *t, 2);
        assert_eq!(list, [0.5]);
    }

    #[test]
    fn the_hold_only_moves_when_depth_drifts_enough() {
        assert_eq!(next_delay(0.0, 0.07), 0.07 + HOLD_MARGIN);
        let settled = 0.08;
        assert_eq!(next_delay(settled, 0.075), settled);
        assert_eq!(next_delay(settled, 0.11), 0.11 + HOLD_MARGIN);
        assert_eq!(next_delay(0.0, 2.0), MOST_HELD);
    }
}
