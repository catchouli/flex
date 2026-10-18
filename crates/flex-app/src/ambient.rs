//! Ambient light for LED strips behind the TV: samples the video while it plays, works out
//! colours (`flex_core::ambient`) and hands them to plugins subscribed to `light`. Does
//! nothing at all unless such a plugin is running.
//!
//! Sampling is cheap: in the player's paint callback the frame mpv just drew is blitted
//! into a 1024×576 texture, mipmapped down to 32×18 on the GPU, and read back through a
//! pixel buffer a frame later (so the CPU never waits for the GPU), ~30 times a second.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::glow::{self, HasContext};
use flex_core::ambient::{self, Boost, Grid, Linear, Mode, Noticeable, Smoother};
use flex_core::plugins::Host;
use flex_plugin::Light;

/// Updates per second sent to lights (the Bluetooth strip keeps up with ~30).
const RATE: Duration = Duration::from_millis(40);
/// A sample older than this means the video stalled (seeking, loading): hold the colour.
const STALE: Duration = Duration::from_millis(600);
/// Unchanged colours are sent again this often, so a restarted plugin catches up.
const RESEND: Duration = Duration::from_secs(2);

const BASE: (i32, i32) = (1024, 576);
/// Mip level read back: 1024×576 / 2^5 = 32×18.
const LEVEL: i32 = 5;
const GRID: (i32, i32) = (BASE.0 >> LEVEL, BASE.1 >> LEVEL);

struct GlObjects {
    texture: glow::Texture,
    draw: glow::Framebuffer,
    read: glow::Framebuffer,
    pbo: glow::Buffer,
}

/// A GL fence. Only ever touched on the GL thread (the sampler just sits in a Mutex
/// shared with the UI thread, which only reads `latest`).
#[derive(Clone, Copy)]
struct Fence(glow::Fence);

unsafe impl Send for Fence {}

/// The GPU side; lives on the GL thread (inside paint callbacks).
#[derive(Default)]
pub struct Sampler {
    gl: Option<GlObjects>,
    /// Setting up failed (e.g. no GL 3): don't try again.
    failed: bool,
    /// A read-back in flight: its fence, and whether rows come bottom-up.
    pending: Option<(Fence, bool)>,
    last_capture: Option<Instant>,
    latest: Option<(Grid, Instant)>,
}

/// Where the picture is within the frame mpv drew: black bar sizes in pixels.
#[derive(Clone, Copy, Default)]
pub struct Margins {
    pub left: i32,
    pub right: i32,
    pub top: i32,
    pub bottom: i32,
}

impl Sampler {
    /// Call right after mpv renders into `src` (`None` = the window), `w`×`h` pixels.
    /// `bottom_up`: whether the picture's top is at the framebuffer's top (GL rows run
    /// bottom-up), as when mpv renders with flip_y.
    ///
    /// # Safety
    /// Must be called on the GL thread with `gl` current. Leaves the read/draw
    /// framebuffers bound to our objects: rebind `src` afterwards.
    pub unsafe fn capture(&mut self, gl: &glow::Context, src: Option<glow::Framebuffer>, w: i32, h: i32, m: Margins, bottom_up: bool) {
        unsafe {
            self.collect(gl);
            if self.failed || self.pending.is_some() || self.last_capture.is_some_and(|t| t.elapsed() < RATE) {
                return;
            }
            if self.gl.is_none() {
                match create(gl) {
                    Ok(objects) => self.gl = Some(objects),
                    Err(e) => {
                        log::warn!("ambient light: can't sample video: {e}");
                        self.failed = true;
                        return;
                    }
                }
            }
            let Some(o) = &self.gl else { return };
            let (x0, x1) = (m.left.max(0), w - m.right.max(0));
            let (y0, y1) = if bottom_up { (m.bottom.max(0), h - m.top.max(0)) } else { (m.top.max(0), h - m.bottom.max(0)) };
            if x1 - x0 < GRID.0 || y1 - y0 < GRID.1 {
                return;
            }
            self.last_capture = Some(Instant::now());
            let scissor = gl.is_enabled(glow::SCISSOR_TEST);
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, src);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(o.draw));
            gl.blit_framebuffer(x0, y0, x1, y1, 0, 0, BASE.0, BASE.1, glow::COLOR_BUFFER_BIT, glow::LINEAR);
            gl.bind_texture(glow::TEXTURE_2D, Some(o.texture));
            gl.generate_mipmap(glow::TEXTURE_2D);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(o.read));
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(o.pbo));
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
            gl.read_pixels(0, 0, GRID.0, GRID.1, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::BufferOffset(0));
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            if let Ok(fence) = gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
                self.pending = Some((Fence(fence), bottom_up));
            }
            if scissor {
                gl.enable(glow::SCISSOR_TEST);
            }
        }
    }

    /// Picks up a finished read-back, if the GPU is done with it.
    unsafe fn collect(&mut self, gl: &glow::Context) {
        unsafe {
            let (Some((Fence(fence), bottom_up)), Some(o)) = (self.pending, &self.gl) else { return };
            match gl.client_wait_sync(fence, 0, 0) {
                glow::ALREADY_SIGNALED | glow::CONDITION_SATISFIED => {}
                glow::TIMEOUT_EXPIRED => return,
                _ => {
                    gl.delete_sync(fence);
                    self.pending = None;
                    return;
                }
            }
            gl.delete_sync(fence);
            self.pending = None;
            let len = (GRID.0 * GRID.1 * 4) as usize;
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(o.pbo));
            let ptr = gl.map_buffer_range(glow::PIXEL_PACK_BUFFER, 0, len as i32, glow::MAP_READ_BIT);
            if !ptr.is_null() {
                let bytes = std::slice::from_raw_parts(ptr, len);
                let row = (GRID.0 * 4) as usize;
                let rgba = if bottom_up { bytes.chunks(row).rev().flatten().copied().collect() } else { bytes.to_vec() };
                gl.unmap_buffer(glow::PIXEL_PACK_BUFFER);
                self.latest = Some((Grid { width: GRID.0 as usize, height: GRID.1 as usize, rgba }, Instant::now()));
            }
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
        }
    }

    /// The newest sample, unless it's stale.
    fn fresh(&self) -> Option<&Grid> {
        self.latest.as_ref().filter(|(_, at)| at.elapsed() < STALE).map(|(g, _)| g)
    }
}

unsafe fn create(gl: &glow::Context) -> Result<GlObjects, String> {
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        let none = glow::PixelUnpackData::Slice(None);
        gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, BASE.0, BASE.1, 0, glow::RGBA, glow::UNSIGNED_BYTE, none);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, LEVEL);
        gl.generate_mipmap(glow::TEXTURE_2D);
        gl.bind_texture(glow::TEXTURE_2D, None);

        let draw = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(draw));
        gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
        let draw_ok = gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
        let read = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(read));
        gl.framebuffer_texture_2d(glow::READ_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), LEVEL);
        let read_ok = gl.check_framebuffer_status(glow::READ_FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;

        let pbo = gl.create_buffer()?;
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(pbo));
        gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, GRID.0 * GRID.1 * 4, glow::STREAM_READ);
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
        if !(draw_ok && read_ok) {
            gl.delete_texture(texture);
            gl.delete_framebuffer(draw);
            gl.delete_framebuffer(read);
            gl.delete_buffer(pbo);
            return Err("framebuffers incomplete".into());
        }
        Ok(GlObjects { texture, draw, read, pbo })
    }
}

/// One cycling option in Settings → Ambient light.
pub struct Choice {
    pub key: &'static str,
    pub label: &'static str,
    pub options: &'static [&'static str],
    pub default: usize,
    /// Help shown beside it.
    pub detail: &'static str,
}

pub const MODE: usize = 0;
const SMOOTHING: usize = 1;
const SATURATION: usize = 2;
const BRIGHTNESS: usize = 3;
const CURVE: usize = 4;
const IDLE: usize = 5;
const IDLE_BRIGHTNESS: usize = 6;

const CHANGE: &str = "OK to change";

pub const CHOICES: [Choice; 7] = [
    Choice {
        key: "ambient_mode",
        label: "Colour from",
        options: &["The whole picture", "The edges of the picture", "The most noticeable colour"],
        default: 0,
        detail: CHANGE,
    },
    Choice { key: "ambient_smoothing", label: "Smoothing", options: &["Off", "Quick", "Medium", "Slow"], default: 2, detail: CHANGE },
    Choice {
        key: "ambient_saturation",
        label: "Colours",
        options: &["As on screen", "A bit more vivid", "More vivid", "Very vivid"],
        default: 1,
        detail: CHANGE,
    },
    // A new key: the old "ambient_brightness" stored indexes into a different list.
    Choice {
        key: "ambient_boost",
        label: "Brightness",
        options: &["20%", "40%", "60%", "80%", "100%", "150%", "200%", "300%", "400%"],
        default: 4,
        detail: "Over 100% boosts it; too much washes bright colours out",
    },
    Choice {
        key: "ambient_curve",
        label: "Brightness follows",
        options: &["The light from the screen", "How bright the picture looks"],
        default: 0,
        detail: "The second is much brighter in most scenes",
    },
    Choice {
        key: "ambient_idle",
        label: "Outside videos",
        options: &["Warm white", "Soft white", "Cool white", "Off", "Keep the last colour"],
        default: 0,
        detail: CHANGE,
    },
    Choice {
        key: "ambient_idle_brightness",
        label: "Brightness outside videos",
        options: &["10%", "20%", "30%", "50%", "100%"],
        default: 1,
        detail: CHANGE,
    },
];

const SMOOTHING_SECS: [f32; 4] = [0.0, 0.12, 0.3, 0.8];
const SATURATIONS: [f32; 4] = [1.0, 1.2, 1.5, 2.0];
const GAINS: [f32; 9] = [0.2, 0.4, 0.6, 0.8, 1.0, 1.5, 2.0, 3.0, 4.0];
/// As LED (linear) values: what the strip shows at full brightness.
const IDLE_COLOURS: [Linear; 3] = [[1.0, 0.45, 0.12], [1.0, 0.7, 0.4], [1.0, 1.0, 1.0]];
const IDLE_BRIGHTNESSES: [f32; 5] = [0.1, 0.2, 0.3, 0.5, 1.0];
/// How quickly the light fades to the idle colour when leaving a video.
const IDLE_FADE_SECS: f32 = 0.8;

/// What to light when no video is showing.
enum Idle {
    Colour(Linear),
    Hold,
}

pub struct Ambient {
    pub sampler: Arc<Mutex<Sampler>>,
    /// Index into each of [`CHOICES`]' options.
    pub values: [usize; CHOICES.len()],
    /// Per zone count (plugins can want different numbers of zones).
    smoothers: HashMap<usize, Smoother>,
    /// For "the most noticeable colour" (remembers its pick between frames).
    noticeable: Noticeable,
    last_tick: Option<Instant>,
    /// Nothing to do before this (colours settled), unless what's on screen changes.
    next_tick: Option<Instant>,
    was_video: bool,
    /// What lights are showing (one zone), for the player's stats overlay.
    pub current: Option<Light>,
}

impl Ambient {
    pub fn new(setting: impl Fn(&str) -> Option<String>) -> Self {
        let values = std::array::from_fn(|i| {
            let c = &CHOICES[i];
            setting(c.key).and_then(|v| v.parse().ok()).filter(|v| *v < c.options.len()).unwrap_or(c.default)
        });
        Self {
            sampler: Arc::default(),
            values,
            smoothers: HashMap::new(),
            noticeable: Noticeable::default(),
            last_tick: None,
            next_tick: None,
            was_video: false,
            current: None,
        }
    }

    /// How zones are averaged. "The most noticeable colour" is for one-colour lights; ones
    /// with several zones use the edges for it.
    pub fn mode(&self) -> Mode {
        if self.values[MODE] == 0 { Mode::Whole } else { Mode::Edges }
    }

    fn noticeable_mode(&self) -> bool {
        self.values[MODE] == 2
    }

    fn idle(&self) -> Idle {
        let scale = IDLE_BRIGHTNESSES[self.values[IDLE_BRIGHTNESS]];
        match IDLE_COLOURS.get(self.values[IDLE]) {
            Some(rgb) => Idle::Colour(rgb.map(|c| c * scale)),
            None if self.values[IDLE] == 3 => Idle::Colour([0.0; 3]),
            None => Idle::Hold,
        }
    }

    /// Sends the next light update if one is due. `video`: a video is on screen (its
    /// samples are used; otherwise lights fade to the idle colour). Returns when to call
    /// again.
    pub fn tick(&mut self, plugins: &Host, video: bool) -> Option<Duration> {
        if !plugins.wants_light() {
            self.smoothers.clear();
            self.last_tick = None;
            self.next_tick = None;
            self.current = None;
            return None;
        }
        let now = Instant::now();
        if std::mem::replace(&mut self.was_video, video) != video {
            self.next_tick = None;
        }
        if let Some(wait) = self.next_tick.and_then(|t| t.checked_duration_since(now)) {
            return Some(wait);
        }
        let dt = self.last_tick.map_or(0.0, |t| (now - t).as_secs_f32());
        self.last_tick = Some(now);

        let grid = if video { self.sampler.lock().unwrap_or_else(|e| e.into_inner()).fresh().cloned() } else { None };
        let (mode, idle) = (self.mode(), self.idle());
        // Boost and saturation are for the picture; the idle colour has its own brightness.
        let boost = Boost { gain: GAINS[self.values[BRIGHTNESS]], perceived: self.values[CURVE] == 1, ..Boost::default() };
        let saturation = if grid.is_some() { SATURATIONS[self.values[SATURATION]] } else { 1.0 };
        let tau = if grid.is_some() { SMOOTHING_SECS[self.values[SMOOTHING]] } else { IDLE_FADE_SECS };
        let mut settled = true;
        let noticeable_mode = self.noticeable_mode();
        let noticeable = &mut self.noticeable;
        let smoothers = &mut self.smoothers;
        let mut one_zone = None;
        plugins.send_light(|zones| {
            let smoother = smoothers.entry(zones).or_default();
            let held = smoother.current().to_vec();
            let target = match (&grid, &idle) {
                (Some(grid), _) if noticeable_mode && zones == 1 => boost.apply(&[noticeable.pick(grid)]),
                (Some(grid), _) => boost.apply(&grid.zones(zones, mode)),
                // Video without a fresh frame (loading, seeking), or "keep the last colour".
                (None, _) if (video || matches!(idle, Idle::Hold)) && !held.is_empty() => held,
                (None, Idle::Colour(c)) => vec![*c; zones],
                (None, _) => vec![[0.0; 3]; zones],
            };
            let target = if target.len() == zones { target } else { vec![[0.0; 3]; zones] };
            let light = ambient::to_light(smoother.step(&target, dt, tau), saturation);
            settled &= grid.is_none() && smoother.settled(&target);
            if zones == 1 {
                one_zone = Some(light.clone());
            }
            light
        });
        self.current = one_zone;
        // Settled idle colours only get a slow keep-alive (for plugins that restarted).
        let wait = if settled { RESEND } else { RATE };
        self.next_tick = Some(now + wait);
        Some(wait)
    }

    /// A setting changed: update the lights straight away.
    pub fn changed(&mut self) {
        self.next_tick = None;
    }
}
