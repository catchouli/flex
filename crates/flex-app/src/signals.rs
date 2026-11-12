//! What effects can know about the picture besides depth, masks and faces, all worked out on the
//! GPU without a network: older frames, how things are moving, and the room behind people. The
//! paint callback keeps them up to date, only for the effects that ask.

use std::collections::VecDeque;

use eframe::glow::{self, HasContext};

use crate::effects::{Target, compile, create_target};

/// How many older frames effects can reach, about a second of film.
pub const HISTORY_FRAMES: usize = 24;
/// Older frames and the room behind people are kept at no more than this width.
const MOST_WIDE: i32 = 960;
/// The small pictures compared to find movement, and how many results are kept to match the
/// picture being held back.
const FLOW_PICTURE: (i32, i32) = (320, 180);
const FLOW_SIZE: (i32, i32) = (160, 90);
const FLOW_KEPT: usize = 8;
/// How quickly the room behind people follows what's visible, per frame.
const PLATE_RATE: f32 = 0.35;

fn kept_size(size: (i32, i32)) -> (i32, i32) {
    let width = (size.0 / 2).clamp(1, MOST_WIDE);
    (width, ((size.1 as f64 * width as f64 / size.0.max(1) as f64).round() as i32).max(1))
}

/// Whether going from `last` to `now` is ordinary playback, not a seek.
fn continues(last: f32, now: f32) -> bool {
    now >= last && now < last + 1.0
}

/// A mipmapped copy of `create_target`'s texture, for reading at smaller sizes.
unsafe fn mipmapped(gl: &glow::Context, size: (i32, i32), filter: u32) -> Result<Target, String> {
    unsafe {
        let target = create_target(gl, size, &[(glow::RGBA8, glow::UNSIGNED_BYTE)])?;
        gl.bind_texture(glow::TEXTURE_2D, Some(target.texture));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter as i32);
        gl.generate_mipmap(glow::TEXTURE_2D);
        gl.bind_texture(glow::TEXTURE_2D, None);
        Ok(target)
    }
}

pub(crate) unsafe fn delete_target(gl: &glow::Context, target: Target) {
    unsafe {
        gl.delete_framebuffer(target.fbo);
        gl.delete_texture(target.texture);
    }
}

/// The last second or so of frames, in a ring of layers in one array texture.
#[derive(Default)]
pub struct HistoryGl {
    objects: Option<(glow::Texture, glow::Framebuffer, (i32, i32))>,
    failed: bool,
    times: [Option<f32>; HISTORY_FRAMES],
    newest: usize,
    last_time: Option<f32>,
}

/// Where a frame is in the ring, how many frames there are behind it, and frames per second.
#[derive(Clone, Copy, Default)]
pub struct HistorySpot {
    pub head: f32,
    pub count: f32,
    pub rate: f32,
}

impl HistoryGl {
    pub fn texture(&self) -> Option<glow::Texture> {
        self.objects.as_ref().map(|o| o.0)
    }

    /// Adds this frame, if it's a new one.
    pub unsafe fn push(&mut self, gl: &glow::Context, source: glow::Framebuffer, size: (i32, i32), time: f32) {
        if self.failed || self.last_time == Some(time) {
            return;
        }
        let wanted = kept_size(size);
        unsafe {
            if self.objects.as_ref().is_some_and(|o| o.2 != wanted) {
                self.release(gl);
            }
            if self.objects.is_none() {
                match Self::create(gl, wanted) {
                    Ok(o) => self.objects = Some(o),
                    Err(e) => {
                        log::warn!("video effects: older frames: {e}");
                        self.failed = true;
                        return;
                    }
                }
            }
            let Some((texture, fbo, _)) = self.objects else { return };
            if self.last_time.is_some_and(|last| !continues(last, time)) {
                self.times = [None; HISTORY_FRAMES];
            }
            self.newest = (self.newest + 1) % HISTORY_FRAMES;
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(source));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_layer(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, Some(texture), 0, self.newest as i32);
            gl.blit_framebuffer(0, 0, size.0, size.1, 0, 0, wanted.0, wanted.1, glow::COLOR_BUFFER_BIT, glow::LINEAR);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        self.times[self.newest] = Some(time);
        self.last_time = Some(time);
    }

    unsafe fn create(gl: &glow::Context, size: (i32, i32)) -> Result<(glow::Texture, glow::Framebuffer, (i32, i32)), String> {
        unsafe {
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(texture));
            let none = glow::PixelUnpackData::Slice(None);
            gl.tex_image_3d(glow::TEXTURE_2D_ARRAY, 0, glow::RGBA8 as i32, size.0, size.1, HISTORY_FRAMES as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, none);
            for (param, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, param, value as i32);
            }
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
            Ok((texture, gl.create_framebuffer()?, size))
        }
    }

    /// Where the frame at `time` is.
    pub fn spot(&self, time: f32) -> HistorySpot {
        let nearest = self.times.iter().enumerate().filter_map(|(i, t)| t.map(|t| (i, (t - time).abs()))).min_by(|a, b| a.1.total_cmp(&b.1));
        let Some((head, _)) = nearest else { return HistorySpot::default() };
        let start = self.times[head].unwrap_or(time);
        let (mut count, mut oldest) = (1usize, start);
        while count < HISTORY_FRAMES {
            let next = (head + HISTORY_FRAMES - count) % HISTORY_FRAMES;
            match self.times[next] {
                Some(t) if t < oldest => oldest = t,
                _ => break,
            }
            count += 1;
        }
        let rate = if count > 1 && start > oldest { (count - 1) as f32 / (start - oldest) } else { 24.0 };
        HistorySpot { head: head as f32, count: count as f32, rate }
    }

    pub unsafe fn release(&mut self, gl: &glow::Context) {
        if let Some((texture, fbo, _)) = self.objects.take() {
            unsafe {
                gl.delete_texture(texture);
                gl.delete_framebuffer(fbo);
            }
        }
        self.times = [None; HISTORY_FRAMES];
        self.last_time = None;
    }
}

const FLOW_MATCH: &str = "
uniform sampler2D u_cur;
uniform sampler2D u_prev;
uniform sampler2D u_coarse;
uniform float u_lod;
uniform float u_step;
uniform float u_radius;
uniform float u_use_coarse;
in vec2 v_uv;
out vec4 flex_color;
float cost(vec2 d, vec2 texel) {
    float sum = 0.0;
    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            vec2 q = v_uv + vec2(float(x), float(y)) * texel;
            sum += dot(abs(textureLod(u_cur, q, u_lod).rgb - textureLod(u_prev, q - d, u_lod).rgb), vec3(0.3, 0.59, 0.11));
        }
    }
    return sum;
}
void main() {
    vec2 texel = 1.0 / vec2(textureSize(u_cur, int(u_lod)));
    vec2 base = u_use_coarse > 0.5 ? textureLod(u_coarse, v_uv, 0.0).rg : vec2(0.0);
    vec2 best = base;
    float least = 1e9;
    int radius = int(u_radius);
    for (int y = -radius; y <= radius; y++) {
        for (int x = -radius; x <= radius; x++) {
            vec2 d = base + vec2(float(x), float(y)) * texel * u_step;
            // Blank areas match anything: they shouldn't be seen to move.
            float c = cost(d, texel) + 0.02 * length(d / texel);
            if (c < least) { least = c; best = d; }
        }
    }
    flex_color = vec4(best, 0.0, 1.0);
}
";

const FLOW_FINISH: &str = "
uniform sampler2D u_in;
uniform float u_per_second;
in vec2 v_uv;
out vec4 flex_color;
void main() {
    vec2 t = 1.0 / vec2(textureSize(u_in, 0));
    vec2 sum = texture(u_in, v_uv).rg * 2.0;
    sum += texture(u_in, v_uv + vec2(t.x, 0.0)).rg + texture(u_in, v_uv - vec2(t.x, 0.0)).rg;
    sum += texture(u_in, v_uv + vec2(0.0, t.y)).rg + texture(u_in, v_uv - vec2(0.0, t.y)).rg;
    flex_color = vec4(sum / 6.0 * u_per_second, 0.0, 1.0);
}
";

struct FlowObjects {
    pictures: [Target; 2],
    coarse: Target,
    middle: Target,
    fine: Target,
    kept: Vec<(Target, Option<f32>)>,
    matching: glow::Program,
    finish: glow::Program,
}

/// How things are moving: block matching from small pictures up to larger ones, in a few small
/// fragment passes. Kept per frame so a held-back picture gets its own.
#[derive(Default)]
pub struct FlowGl {
    objects: Option<FlowObjects>,
    failed: bool,
    current: usize,
    next_kept: usize,
    last_time: Option<f32>,
}

impl FlowGl {
    unsafe fn create(gl: &glow::Context, decl: &str, vertex: &str, embedded: bool) -> Result<FlowObjects, String> {
        let float = [(glow::RGBA16F, glow::HALF_FLOAT)];
        let precision = if embedded { "precision highp float;\nprecision highp int;\n" } else { "" };
        unsafe {
            let pictures = [mipmapped(gl, FLOW_PICTURE, glow::LINEAR_MIPMAP_NEAREST)?, mipmapped(gl, FLOW_PICTURE, glow::LINEAR_MIPMAP_NEAREST)?];
            let coarse = create_target(gl, (FLOW_SIZE.0 / 4, FLOW_SIZE.1 / 4), &float)?;
            let middle = create_target(gl, (FLOW_SIZE.0 / 2, FLOW_SIZE.1 / 2), &float)?;
            let fine = create_target(gl, FLOW_SIZE, &float)?;
            let mut kept = Vec::with_capacity(FLOW_KEPT);
            for _ in 0..FLOW_KEPT {
                kept.push((create_target(gl, FLOW_SIZE, &float)?, None));
            }
            let matching = compile(gl, vertex, &format!("{decl}\n{precision}{FLOW_MATCH}"))?;
            let finish = compile(gl, vertex, &format!("{decl}\n{precision}{FLOW_FINISH}"))?;
            Ok(FlowObjects { pictures, coarse, middle, fine, kept, matching, finish })
        }
    }

    /// Works out this frame's movement, if it's a new one.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn update(&mut self, gl: &glow::Context, decl: &str, vertex: &str, embedded: bool, vao: glow::VertexArray, source: glow::Framebuffer, size: (i32, i32), rect: [f32; 4], time: f32) {
        if self.failed || self.last_time == Some(time) {
            return;
        }
        unsafe {
            if self.objects.is_none() {
                match Self::create(gl, decl, vertex, embedded) {
                    Ok(o) => self.objects = Some(o),
                    Err(e) => {
                        log::warn!("video effects: no movement: {e}");
                        self.failed = true;
                        return;
                    }
                }
            }
            let Some(o) = &mut self.objects else { return };
            let current = 1 - self.current;
            let (x0, y0) = ((rect[0] * size.0 as f32) as i32, (rect[1] * size.1 as f32) as i32);
            let (x1, y1) = (((rect[2] * size.0 as f32) as i32).max(x0 + 1), ((rect[3] * size.1 as f32) as i32).max(y0 + 1));
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(source));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(o.pictures[current].fbo));
            gl.blit_framebuffer(x0, y0, x1, y1, 0, 0, FLOW_PICTURE.0, FLOW_PICTURE.1, glow::COLOR_BUFFER_BIT, glow::LINEAR);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(o.pictures[current].texture));
            gl.generate_mipmap(glow::TEXTURE_2D);

            let slot = self.next_kept;
            self.next_kept = (slot + 1) % FLOW_KEPT;
            let moving = self.last_time.filter(|last| continues(*last, time)).map(|last| time - last).filter(|dt| *dt > 0.0);
            let Some(dt) = moving else {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(o.kept[slot].0.fbo));
                gl.clear_color(0.0, 0.0, 0.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                o.kept[slot].1 = Some(time);
                self.current = current;
                self.last_time = Some(time);
                return;
            };

            gl.bind_vertex_array(Some(vao));
            gl.use_program(Some(o.matching));
            let at = |name: &str| gl.get_uniform_location(o.matching, name);
            gl.uniform_1_i32(at("u_cur").as_ref(), 0);
            gl.uniform_1_i32(at("u_prev").as_ref(), 1);
            gl.uniform_1_i32(at("u_coarse").as_ref(), 2);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(o.pictures[self.current].texture));
            // Picture level, step between tries, how many tries each way, and where it draws.
            let levels = [
                (3.0, 1.0, 3.0, &o.coarse, None, (FLOW_SIZE.0 / 4, FLOW_SIZE.1 / 4)),
                (2.0, 1.0, 2.0, &o.middle, Some(&o.coarse), (FLOW_SIZE.0 / 2, FLOW_SIZE.1 / 2)),
                (1.0, 0.5, 2.0, &o.fine, Some(&o.middle), FLOW_SIZE),
            ];
            for (lod, step, radius, target, coarse, pass_size) in levels {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.fbo));
                gl.viewport(0, 0, pass_size.0, pass_size.1);
                gl.active_texture(glow::TEXTURE2);
                gl.bind_texture(glow::TEXTURE_2D, coarse.map(|c| c.texture));
                gl.uniform_1_f32(at("u_lod").as_ref(), lod);
                gl.uniform_1_f32(at("u_step").as_ref(), step);
                gl.uniform_1_f32(at("u_radius").as_ref(), radius);
                gl.uniform_1_f32(at("u_use_coarse").as_ref(), if coarse.is_some() { 1.0 } else { 0.0 });
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            gl.use_program(Some(o.finish));
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(o.kept[slot].0.fbo));
            gl.viewport(0, 0, FLOW_SIZE.0, FLOW_SIZE.1);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(o.fine.texture));
            gl.uniform_1_i32(gl.get_uniform_location(o.finish, "u_in").as_ref(), 0);
            gl.uniform_1_f32(gl.get_uniform_location(o.finish, "u_per_second").as_ref(), 1.0 / dt);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            o.kept[slot].1 = Some(time);
            self.current = current;
            self.last_time = Some(time);
        }
    }

    /// A cut isn't movement: clears what was worked out for the frame at `time`.
    pub unsafe fn zero_at(&mut self, gl: &glow::Context, time: f32) {
        let Some(o) = &self.objects else { return };
        let Some((target, _)) = o.kept.iter().find(|(_, t)| *t == Some(time)) else { return };
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.fbo));
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }

    /// The movement worked out for the frame nearest `time`.
    pub fn at(&self, time: f32) -> Option<glow::Texture> {
        let o = self.objects.as_ref()?;
        crate::effects::nearest(o.kept.iter().filter_map(|(target, t)| t.map(|t| (target, t))), time).map(|(target, _)| target.texture)
    }

    pub unsafe fn release(&mut self, gl: &glow::Context) {
        if let Some(o) = self.objects.take() {
            unsafe {
                for t in o.pictures.into_iter().chain([o.coarse, o.middle, o.fine]).chain(o.kept.into_iter().map(|k| k.0)) {
                    delete_target(gl, t);
                }
                gl.delete_program(o.matching);
                gl.delete_program(o.finish);
            }
        }
        self.last_time = None;
    }
}

const PLATE_UPDATE: &str = "
uniform sampler2D u_frame;
uniform sampler2D u_people;
uniform sampler2D u_old;
uniform vec4 u_rect;
uniform float u_reset;
uniform float u_rate;
in vec2 v_uv;
out vec4 flex_color;
void main() {
    vec2 p = (v_uv - u_rect.xy) / max(u_rect.zw - u_rect.xy, vec2(1e-6));
    float person = texture(u_people, p).r;
    for (int i = 0; i < 8; i++) {
        float a = float(i) * 0.7853982;
        person = max(person, texture(u_people, p + vec2(cos(a) * 0.56, sin(a)) * 0.03).r);
    }
    float clear = 1.0 - smoothstep(0.06, 0.25, person);
    vec4 old = u_reset > 0.5 ? vec4(0.0) : textureLod(u_old, v_uv, 0.0);
    // Kept with colour multiplied by how much is known, so smaller sizes fill the gaps.
    flex_color = mix(old, vec4(texture(u_frame, v_uv).rgb, 1.0), clear * mix(1.0, u_rate, old.a));
}
";

/// The room behind people: what's been visible since the last cut. Anything not seen yet is filled
/// in from the mipmap levels, by the shader's `plate_at`.
#[derive(Default)]
pub struct PlateGl {
    objects: Option<([Target; 2], glow::Program, (i32, i32))>,
    failed: bool,
    current: usize,
    last_time: Option<f32>,
}

impl PlateGl {
    pub fn texture(&self) -> Option<glow::Texture> {
        self.objects.as_ref().filter(|_| self.last_time.is_some()).map(|o| o.0[self.current].texture)
    }

    pub fn last_time(&self) -> Option<f32> {
        self.last_time
    }

    /// Updates it from the frame on screen and its masks, starting afresh if `reset`.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn update(&mut self, gl: &glow::Context, decl: &str, vertex: &str, embedded: bool, vao: glow::VertexArray, picture: glow::Texture, people: glow::Texture, size: (i32, i32), rect: [f32; 4], time: f32, reset: bool) {
        if self.failed || self.last_time == Some(time) {
            return;
        }
        let wanted = kept_size(size);
        let filter = glow::LINEAR_MIPMAP_LINEAR;
        unsafe {
            if self.objects.as_ref().is_some_and(|o| o.2 != wanted) {
                self.release(gl);
            }
            if self.objects.is_none() {
                let precision = if embedded { "precision highp float;\nprecision highp int;\n" } else { "" };
                let made = mipmapped(gl, wanted, filter).and_then(|a| {
                    let b = mipmapped(gl, wanted, filter)?;
                    let program = compile(gl, vertex, &format!("{decl}\n{precision}{PLATE_UPDATE}"))?;
                    Ok(([a, b], program, wanted))
                });
                match made {
                    Ok(o) => self.objects = Some(o),
                    Err(e) => {
                        log::warn!("video effects: no room behind people: {e}");
                        self.failed = true;
                        return;
                    }
                }
            }
            let Some((targets, program, _)) = &self.objects else { return };
            let program = *program;
            let reset = reset || self.last_time.is_none_or(|last| !continues(last, time));
            let next = 1 - self.current;
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(Some(vao));
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(targets[next].fbo));
            gl.viewport(0, 0, wanted.0, wanted.1);
            gl.use_program(Some(program));
            for (unit, texture, name, index) in [(glow::TEXTURE0, picture, "u_frame", 0), (glow::TEXTURE1, people, "u_people", 1), (glow::TEXTURE2, targets[self.current].texture, "u_old", 2)] {
                gl.active_texture(unit);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.uniform_1_i32(gl.get_uniform_location(program, name).as_ref(), index);
            }
            gl.uniform_4_f32(gl.get_uniform_location(program, "u_rect").as_ref(), rect[0], rect[1], rect[2], rect[3]);
            gl.uniform_1_f32(gl.get_uniform_location(program, "u_reset").as_ref(), if reset { 1.0 } else { 0.0 });
            gl.uniform_1_f32(gl.get_uniform_location(program, "u_rate").as_ref(), PLATE_RATE);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(targets[next].texture));
            gl.generate_mipmap(glow::TEXTURE_2D);
            self.current = next;
        }
        self.last_time = Some(time);
    }

    pub unsafe fn release(&mut self, gl: &glow::Context) {
        if let Some(([a, b], program, _)) = self.objects.take() {
            unsafe {
                delete_target(gl, a);
                delete_target(gl, b);
                gl.delete_program(program);
            }
        }
        self.last_time = None;
    }
}

pub const PALETTE: usize = 4;

/// The size of picture `Stats` measures.
pub const STATS_SIZE: (usize, usize) = (32, 18);

/// The overall look of the picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub brightness: f32,
    pub colourfulness: f32,
    /// -1 (blue) to 1 (orange).
    pub warmth: f32,
    pub palette: [[f32; 3]; PALETTE],
    pub share: [f32; PALETTE],
    /// Where the brightest part is, across and up the picture from 0 to 1, and how much it
    /// stands out as one place.
    pub light: [f32; 3],
}

impl Default for Stats {
    fn default() -> Self {
        Self { brightness: 0.0, colourfulness: 0.0, warmth: 0.0, palette: [[0.1; 3], [0.35; 3], [0.6; 3], [0.85; 3]], share: [0.25; PALETTE], light: [0.5, 1.0, 0.0] }
    }
}

impl Stats {
    /// Measures an RGBA picture, starting the palette from `self`'s so colours keep their slots.
    pub fn measure(&self, rgba: &[u8]) -> Self {
        let pixels: Vec<[f32; 3]> = rgba.as_chunks::<4>().0.iter().map(|p| [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0]).collect();
        let n = pixels.len().max(1) as f32;
        let mut out = *self;
        out.brightness = pixels.iter().map(|p| 0.3 * p[0] + 0.59 * p[1] + 0.11 * p[2]).sum::<f32>() / n;
        out.colourfulness = pixels.iter().map(|p| p.iter().copied().fold(0.0, f32::max) - p.iter().copied().fold(1.0, f32::min)).sum::<f32>() / n;
        out.warmth = (pixels.iter().map(|p| p[0] - p[2]).sum::<f32>() / n * 3.0).clamp(-1.0, 1.0);
        out.light = brightest(rgba);
        let distance = |a: &[f32; 3], b: &[f32; 3]| (0..3).map(|c| (a[c] - b[c]).powi(2)).sum::<f32>();
        for _ in 0..3 {
            let (mut sums, mut counts) = ([[0.0f32; 3]; PALETTE], [0u32; PALETTE]);
            for p in &pixels {
                let nearest = (0..PALETTE).min_by(|a, b| distance(p, &out.palette[*a]).total_cmp(&distance(p, &out.palette[*b]))).unwrap_or(0);
                for c in 0..3 {
                    sums[nearest][c] += p[c];
                }
                counts[nearest] += 1;
            }
            for k in 0..PALETTE {
                if counts[k] > 0 {
                    out.palette[k] = sums[k].map(|s| s / counts[k] as f32);
                } else {
                    // A colour nothing's using goes to whatever's least well covered, if anything is.
                    let gap = |p: &[f32; 3]| out.palette.iter().map(|c| distance(p, c)).fold(f32::MAX, f32::min);
                    if let Some(far) = pixels.iter().max_by(|a, b| gap(a).total_cmp(&gap(b))).filter(|p| gap(p) > 0.02) {
                        out.palette[k] = *far;
                    }
                }
                out.share[k] = counts[k] as f32 / n;
            }
        }
        out
    }

    /// Moves towards `target`, `amount` of the way (0–1).
    pub fn ease(&mut self, target: &Self, amount: f32) {
        let step = |a: &mut f32, b: f32| *a += (b - *a) * amount;
        step(&mut self.brightness, target.brightness);
        step(&mut self.colourfulness, target.colourfulness);
        step(&mut self.warmth, target.warmth);
        // Slower, so the light doesn't hop about between lamps.
        for (now, new) in self.light.iter_mut().zip(target.light) {
            *now += (new - *now) * amount * 0.25;
        }
        for k in 0..PALETTE {
            step(&mut self.share[k], target.share[k]);
            for c in 0..3 {
                step(&mut self.palette[k][c], target.palette[k][c]);
            }
        }
    }
}

/// Where the brightest part of an RGBA picture is (bottom row first), favouring higher up.
fn brightest(rgba: &[u8]) -> [f32; 3] {
    let (width, height) = STATS_SIZE;
    let lumas: Vec<f32> = rgba.as_chunks::<4>().0.iter().map(|p| (0.3 * f32::from(p[0]) + 0.59 * f32::from(p[1]) + 0.11 * f32::from(p[2])) / 255.0).collect();
    if lumas.len() != width * height {
        return [0.5, 1.0, 0.0];
    }
    let mean = lumas.iter().sum::<f32>() / lumas.len() as f32;
    let peak = lumas.iter().copied().fold(0.0, f32::max);
    let line = mean + (peak - mean) * 0.65;
    let place = |at: usize| ((at % width) as f32 / (width - 1) as f32, (at / width) as f32 / (height - 1) as f32);
    let (mut sum, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for (at, l) in lumas.iter().enumerate() {
        let (x, y) = place(at);
        let weight = (l - line).max(0.0) * (0.5 + y);
        sum += weight;
        sx += weight * x;
        sy += weight * y;
    }
    if sum <= 0.0 {
        return [0.5, 1.0, 0.0];
    }
    let (x, y) = (sx / sum, sy / sum);
    let mut spread = 0.0;
    for (at, l) in lumas.iter().enumerate() {
        let (px, py) = place(at);
        spread += (l - line).max(0.0) * (0.5 + py) * ((px - x).powi(2) + (py - y).powi(2));
    }
    let tight = (1.0 - (spread / sum).sqrt() / 0.4).clamp(0.0, 1.0);
    [x, y, ((peak - mean) * 2.2).clamp(0.0, 1.0) * tight]
}

/// Recent cuts, oldest first, to tell how long the frame on screen has been in its shot.
#[derive(Default)]
pub struct Cuts {
    recent: VecDeque<f32>,
}

impl Cuts {
    pub fn add(&mut self, time: f32) {
        self.recent.push_back(time);
        if self.recent.len() > 8 {
            self.recent.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.recent.clear();
    }

    /// Seconds since the last cut before `time`, if there's been one.
    pub fn since(&self, time: f32) -> Option<f32> {
        self.recent.iter().rev().find(|t| **t <= time).map(|t| time - t)
    }

    /// Whether a cut happened after `after`, up to `time`.
    pub fn between(&self, after: f32, time: f32) -> bool {
        self.recent.iter().any(|t| *t > after && *t <= time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_light_is_found_where_the_picture_is_brightest() {
        let (width, height) = STATS_SIZE;
        let mut rgba = vec![30u8; width * height * 4];
        for y in 13..16 {
            for x in 24..27 {
                rgba[(y * width + x) * 4..][..3].fill(250);
            }
        }
        let light = Stats::default().measure(&rgba).light;
        assert!((light[0] - 25.0 / 31.0).abs() < 0.02 && (light[1] - 14.0 / 17.0).abs() < 0.03, "{light:?}");
        assert!(light[2] > 0.5, "{light:?}");
        assert!(Stats::default().measure(&vec![90u8; width * height * 4]).light[2] < 0.01);
    }

    #[test]
    fn the_palette_finds_the_main_colours_and_keeps_them_in_their_slots() {
        let mut rgba = Vec::new();
        for _ in 0..300 {
            rgba.extend([230, 20, 20, 255]);
        }
        for _ in 0..100 {
            rgba.extend([20, 20, 230, 255]);
        }
        let first = Stats::default().measure(&rgba);
        let red = (0..PALETTE).find(|k| first.palette[*k][0] > 0.8 && first.palette[*k][2] < 0.2).expect("red");
        assert!((first.share[red] - 0.75).abs() < 0.01);
        assert!(first.warmth > 0.5);
        let again = first.measure(&rgba);
        assert!((again.palette[red][0] - first.palette[red][0]).abs() < 1e-4, "same slot");
    }

    #[test]
    fn cuts_say_how_long_a_shot_has_run() {
        let mut cuts = Cuts::default();
        cuts.add(5.0);
        cuts.add(9.0);
        assert_eq!(cuts.since(10.0), Some(1.0));
        assert_eq!(cuts.since(8.0), Some(3.0));
        assert_eq!(cuts.since(4.0), None);
        assert!(cuts.between(8.0, 9.5) && !cuts.between(9.0, 9.5));
    }

    #[test]
    fn kept_copies_are_half_size_up_to_a_limit() {
        assert_eq!(kept_size((1280, 720)), (640, 360));
        assert_eq!(kept_size((3840, 2160)), (960, 540));
    }
}
