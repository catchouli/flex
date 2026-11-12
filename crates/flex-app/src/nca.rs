//! Living textures for video effects: the neural cellular automata of "Self-Organising Textures"
//! (Niklasson, Mordvintsev, Randazzo and Levin, Distill 2021; CC BY 4.0), run on the GL side as
//! the article's own demo runs them. Each cell of a grid over the picture has 12 numbers; each
//! step it sees them and its neighbours' (Sobel x and y and a Laplacian), runs a little network
//! (48 to 96 to 12) and adds what comes out, half the cells at a time, at random. The first three
//! numbers are its colour. Grown over the film, fed a little of its colours and carried along
//! with its movement, the textures crawl over the picture and heal round what moves.
//!
//! The models are the article's (its demo's `models.json`: each layer's weights for every
//! texture in one PNG, 8 bits each), downloaded the first time a texture is wanted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};
use eframe::egui_glow;
use eframe::glow::{self, HasContext};

use crate::effects::{Target, compile, create_target, vertex_source};

const URL: &str = "https://raw.githubusercontent.com/distillpub/post--selforg-textures/24ef9c1eaf3bda8360abc331d5d9509bb7b2a1f1/public/demo/models.json";
const SHA256: &str = "32fdd6d0ae6434185abf166b86bb4b75dcf3a0ecbf1c1646e3b1b3ae03b41bee";
const BYTES: u64 = 445_928;
const FILE: &str = "selforg-textures.json";
pub const CREDIT: &str = "Self-Organising Textures (Niklasson, Mordvintsev, Randazzo and Levin, Distill), CC BY 4.0";

/// The cells' numbers, what each sees, and the hidden layer.
const CHANNELS: usize = 12;
const SEES: usize = 4 * CHANNELS;
const HIDDEN: usize = 96;
/// The weights as the step shader reads them: rows of HIDDEN (24 RGBA texels): the first layer's
/// SEES rows and its bias, then the second's HIDDEN rows and its bias, CHANNELS of each used.
const ROWS: usize = SEES + 1 + HIDDEN + 1;

/// The grid's height in cells; its width keeps the picture's shape.
const CELLS_HIGH: i32 = 160;
/// Steps a second (a few a frame), and how much of the film's colour goes into each.
const STEPS_A_SECOND: f32 = 60.0;
const FEED: f32 = 0.03;

/// The textures effects can pick: what they're called, and the article's name for each.
pub struct Texture {
    pub id: &'static str,
    pub label: &'static str,
    model: &'static str,
}

/// The ones from the article's Inception neurons ("mixed...") are its strangest.
pub const TEXTURES: &[Texture] = &[
    Texture { id: "tex-honeycomb", label: "Honeycomb", model: "honeycombed_0171" },
    Texture { id: "tex-cobwebs", label: "Cobwebs", model: "cobwebbed_0141" },
    Texture { id: "tex-bubbles", label: "Bubbles", model: "bubbly_0101" },
    Texture { id: "tex-swirls", label: "Swirls", model: "swirly_0071" },
    Texture { id: "tex-marble", label: "Marble veins", model: "veined_0141" },
    Texture { id: "tex-cracked", label: "Cracked earth", model: "cracked_0085" },
    Texture { id: "tex-chrome", label: "Liquid chrome", model: "spiralled_0040" },
    Texture { id: "tex-fingerprints", label: "Fingerprints", model: "spiralled_0112" },
    Texture { id: "tex-candy", label: "Candy stripes", model: "spiralled_0124" },
    Texture { id: "tex-zebra", label: "Zebra", model: "striped_0005" },
    Texture { id: "tex-foam", label: "Foam", model: "interlaced_0191" },
    Texture { id: "tex-mondrian", label: "Mondrian", model: "mondrian" },
    Texture { id: "tex-leopard", label: "Leopard", model: "mixed4c_52" },
    Texture { id: "tex-feathers", label: "Psychedelic feathers", model: "mixed4c_438" },
    Texture { id: "tex-scales", label: "Rainbow scales", model: "mixed4c_364" },
    Texture { id: "tex-eyes", label: "Eyes", model: "mixed4b_8" },
    Texture { id: "tex-staring", label: "Staring eyes", model: "mixed4a_472" },
    Texture { id: "tex-maze", label: "Maze", model: "mixed4b_70" },
    Texture { id: "tex-coils", label: "Coils", model: "mixed4d_313" },
    Texture { id: "tex-cells", label: "Cells", model: "mixed4d_42" },
];

pub fn texture(id: &str) -> Option<&'static Texture> {
    TEXTURES.iter().find(|t| t.id == id)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Off,
    Downloading(u8),
    Ready(&'static str),
    Failed(String),
}

impl Status {
    pub fn describe(&self) -> String {
        match self {
            Status::Off => "off".into(),
            Status::Downloading(p) => format!("downloading the textures ({p}%)…"),
            Status::Ready(label) => format!("{label}: growing · {CREDIT}"),
            Status::Failed(e) => format!("⚠ {e}"),
        }
    }
}

struct State {
    status: Status,
    /// The texture wanted, and every model's weights once they're here (by the article's names).
    wanted: &'static Texture,
    models: Option<Arc<HashMap<String, Arc<Vec<f32>>>>>,
    loading: bool,
    /// When `wanted()` first saw the models had failed, for trying again (see `gpu::still_failed`).
    failed_seen: Option<Instant>,
}

/// Shared between the player (which says what's wanted) and the GL side (which grows it).
#[derive(Clone)]
pub struct Link(Arc<Mutex<State>>);

impl Default for Link {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(State { status: Status::Off, wanted: &TEXTURES[0], models: None, loading: false, failed_seen: None })))
    }
}

impl Link {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// An effect wants `id` (a `TEXTURES` id) grown: fetches the models if they aren't here.
    /// Call every frame.
    pub fn wanted(&self, id: &str) {
        let Some(t) = texture(id) else { return };
        let mut state = self.lock();
        state.wanted = t;
        if state.models.is_some() {
            state.status = Status::Ready(t.label);
        }
        if state.models.is_some() || state.loading || (matches!(state.status, Status::Failed(_)) && crate::gpu::still_failed(&mut state.failed_seen)) {
            return;
        }
        state.loading = true;
        drop(state);
        let link = self.clone();
        let started = std::thread::Builder::new().name("textures".into()).spawn(move || {
            let loaded = link.load();
            let mut state = link.lock();
            state.loading = false;
            match loaded {
                Ok(models) => {
                    state.status = Status::Ready(state.wanted.label);
                    state.models = Some(Arc::new(models));
                }
                Err(e) => {
                    log::warn!("textures: {e:#}");
                    state.status = Status::Failed(format!("{e:#}"));
                }
            }
        });
        if let Err(e) = started {
            let mut state = self.lock();
            state.loading = false;
            state.status = Status::Failed(format!("couldn't start: {e}"));
        }
    }

    pub fn status(&self) -> Status {
        self.lock().status.clone()
    }

    /// The texture wanted and its weights (see `ROWS`), once they're here.
    fn current(&self) -> Option<(&'static Texture, Arc<Vec<f32>>)> {
        let state = self.lock();
        let models = state.models.as_ref()?;
        Some((state.wanted, models.get(state.wanted.model)?.clone()))
    }

    fn load(&self) -> Result<HashMap<String, Arc<Vec<f32>>>> {
        let path = crate::worker::fetch_model(FILE, URL, "the textures", BYTES, SHA256, |pct| self.lock().status = Status::Downloading(pct))?;
        read_models(&std::fs::read(&path)?)
    }
}

/// Every model in the article's `models.json`, as the step shader wants its weights.
fn read_models(json: &[u8]) -> Result<HashMap<String, Arc<Vec<f32>>>> {
    let v: serde_json::Value = serde_json::from_slice(json)?;
    let names: Vec<String> = serde_json::from_value(v["model_names"].clone()).context("textures: no names")?;
    let layers = v["layers"].as_array().context("textures: no layers")?;
    ensure!(layers.len() == 2, "textures: {} layers, not 2", layers.len());
    // Each layer: an [in + 1][out] block for each model, side by side in a grid of `layout`.
    let mut decoded = Vec::new();
    for (l, (inputs, outputs)) in layers.iter().zip([(SEES, HIDDEN), (HIDDEN, CHANNELS)]) {
        let shape: Vec<usize> = serde_json::from_value(l["shape"].clone())?;
        let layout: Vec<usize> = serde_json::from_value(l["layout"].clone())?;
        let scale = l["scale"].as_f64().context("textures: no scale")? as f32;
        ensure!(shape == [inputs + 1, outputs] && layout.len() == 2, "textures: a layer is {shape:?}");
        let data = l["data"].as_str().context("textures: no data")?;
        let png = base64(data.split_once(',').context("textures: not a data URL")?.1)?;
        let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)?.to_rgba8();
        let width = layout[0] * outputs / 4;
        ensure!(image.width() as usize == width && image.height() as usize == layout[1] * (inputs + 1), "textures: a layer's picture is {}x{}", image.width(), image.height());
        decoded.push((image, layout[0], inputs, outputs, scale));
    }
    let mut models = HashMap::new();
    for (idx, name) in names.into_iter().enumerate() {
        let mut w = vec![0.0f32; ROWS * HIDDEN];
        let mut row0 = 0;
        for (image, across, inputs, outputs, scale) in &decoded {
            let (mx, my) = (idx % across, idx / across);
            if my * (inputs + 1) >= image.height() as usize {
                bail!("textures: {name} isn't in the pictures");
            }
            for i in 0..=*inputs {
                for o in 0..*outputs {
                    let px = image.get_pixel((mx * outputs / 4 + o / 4) as u32, (my * (inputs + 1) + i) as u32);
                    w[(row0 + i) * HIDDEN + o] = (f32::from(px[o % 4]) - 127.0) / 255.0 * scale;
                }
            }
            row0 += inputs + 1;
        }
        models.insert(name, Arc::new(w));
    }
    Ok(models)
}

fn base64(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' => continue,
            _ => bail!("textures: bad base64"),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

// ---- The GL side -----------------------------------------------------------------------------

fn step_source(version: &str, embedded: bool) -> String {
    // Three outputs need their places given: built into GLSL ES 3.00, an extension before 3.30.
    let precision = if embedded { "precision highp float;\nprecision highp sampler2D;\n" } else { "#extension GL_ARB_explicit_attrib_location : require\n" };
    format!(
        "{version}\n{precision}\
uniform sampler2D u_s0;\nuniform sampler2D u_s1;\nuniform sampler2D u_s2;\nuniform sampler2D u_w;\n\
uniform sampler2D u_video;\nuniform sampler2D u_flow;\nuniform float u_has_flow;\nuniform vec4 u_rect;\n\
uniform float u_moved;\nuniform float u_seed;\nuniform float u_feed;\nuniform float u_fresh;\n\
in vec2 v_uv;\nlayout(location = 0) out vec4 o0;\nlayout(location = 1) out vec4 o1;\nlayout(location = 2) out vec4 o2;\n\
// \"Hash without Sine\" (David Hoskins), as the article's demo uses.\n\
float hash13(vec3 p3) {{ p3 = fract(p3 * .1031); p3 += dot(p3, p3.yzx + 33.33); return fract((p3.x + p3.y) * p3.z); }}\n\
vec4 cell(int k, ivec2 c) {{\n\
    ivec2 size = textureSize(u_s0, 0);\n\
    c = clamp(c, ivec2(0), size - 1);\n\
    return k == 0 ? texelFetch(u_s0, c, 0) : k == 1 ? texelFetch(u_s1, c, 0) : texelFetch(u_s2, c, 0);\n\
}}\n\
vec4 w(int x, int y) {{ return texelFetch(u_w, ivec2(x, y), 0); }}\n\
void main() {{\n\
    ivec2 here = ivec2(gl_FragCoord.xy);\n\
    vec2 size = vec2(textureSize(u_s0, 0));\n\
    vec2 p = (vec2(here) + 0.5) / size;\n\
    vec2 film = u_rect.xy + p * (u_rect.zw - u_rect.xy);\n\
    // What's here now was here less how far it's moved.\n\
    ivec2 from = ivec2(floor((p - texture(u_flow, p).rg * u_has_flow * u_moved) * size));\n\
    // What each cell sees: itself, then Sobel x and y and a Laplacian of its neighbours (y up).\n\
    vec4 s[3]; vec4 dx[3]; vec4 dy[3]; vec4 lap[3];\n\
    for (int k = 0; k < 3; k++) {{\n\
        vec4 n[9];\n\
        for (int j = 0; j < 9; j++) n[j] = cell(k, from + ivec2(j % 3 - 1, j / 3 - 1));\n\
        s[k] = n[4];\n\
        dx[k] = (n[2] + 2.0 * n[5] + n[8] - n[0] - 2.0 * n[3] - n[6]) / 8.0;\n\
        dy[k] = (n[6] + 2.0 * n[7] + n[8] - n[0] - 2.0 * n[1] - n[2]) / 8.0;\n\
        lap[k] = (n[0] + 2.0 * n[1] + n[2] + 2.0 * n[3] - 12.0 * n[4] + 2.0 * n[5] + n[6] + 2.0 * n[7] + n[8]) / 8.0;\n\
    }}\n\
    vec4 seen[12];\n\
    for (int k = 0; k < 3; k++) {{\n\
        seen[k] = clamp(s[k], -2.0, 2.0); seen[3 + k] = clamp(dx[k], -2.0, 2.0);\n\
        seen[6 + k] = clamp(dy[k], -2.0, 2.0); seen[9 + k] = clamp(lap[k], -2.0, 2.0);\n\
    }}\n\
    vec4 h[24];\n\
    for (int o = 0; o < 24; o++) h[o] = w(o, {SEES});\n\
    for (int i = 0; i < {SEES}; i++) {{\n\
        float v = seen[i / 4][i % 4];\n\
        for (int o = 0; o < 24; o++) h[o] += v * w(o, i);\n\
    }}\n\
    vec4 u[3];\n\
    for (int o = 0; o < 3; o++) u[o] = w(o, {last});\n\
    for (int i = 0; i < {HIDDEN}; i++) {{\n\
        float v = clamp(h[i / 4][i % 4], 0.0, 2.0);\n\
        for (int o = 0; o < 3; o++) u[o] += v * w(o, {first} + i);\n\
    }}\n\
    float live = hash13(vec3(vec2(here), u_seed)) < 0.5 ? 1.0 : 0.0;\n\
    for (int k = 0; k < 3; k++) s[k] = clamp(s[k] + u[k] * live, -2.0, 2.0);\n\
    // A little of the film's colour, kept in the first three (all of it when starting).\n\
    vec3 colour = texture(u_video, film).rgb * 2.0 - 1.0;\n\
    s[0].rgb = mix(s[0].rgb, colour, max(u_feed, u_fresh));\n\
    o0 = s[0]; o1 = s[1]; o2 = s[2];\n\
}}\n",
        last = SEES + 1 + HIDDEN,
        first = SEES + 1,
    )
}

fn show_source(version: &str, embedded: bool) -> String {
    let precision = if embedded { "precision highp float;\n" } else { "" };
    format!(
        "{version}\n{precision}uniform sampler2D u_video;\nuniform sampler2D u_s0;\nuniform vec4 u_rect;\nin vec2 v_uv;\nout vec4 flex_color;\n\
         void main() {{\n    vec2 p = (v_uv - u_rect.xy) / max(u_rect.zw - u_rect.xy, vec2(1e-6));\n\
         if (p != clamp(p, 0.0, 1.0)) {{ flex_color = texture(u_video, v_uv); return; }}\n\
         flex_color = vec4(clamp(texture(u_s0, p).rgb * 0.5 + 0.5, 0.0, 1.0), 1.0);\n}}\n"
    )
}

/// Three float textures (the 12 numbers) and a framebuffer drawing to all three.
struct Cells {
    fbo: glow::Framebuffer,
    textures: [glow::Texture; 3],
}

/// The grid, grown on the GL side, and the picture of it for the frame shown.
#[derive(Default)]
pub struct NcaGl {
    failed: bool,
    cells: Vec<Cells>,
    grid: (i32, i32),
    front: usize,
    weights: Option<glow::Texture>,
    model: Option<&'static str>,
    step: Option<glow::Program>,
    show: Option<glow::Program>,
    out: Option<(Target, (i32, i32))>,
    steps: u32,
    owed: f32,
}

impl NcaGl {
    /// Grows the texture `link` wants by however many steps `dt` (seconds) is worth, over the
    /// picture in `video` (`rect` of `size`), carried along `flow`, and draws it the size of the
    /// video (the video itself outside `rect`). None until the models are here.
    ///
    /// # Safety
    /// GL thread, `gl` current.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn draw(&mut self, gl: &glow::Context, version: egui_glow::ShaderVersion, vao: glow::VertexArray, link: &Link, video: glow::Texture, flow: Option<glow::Texture>, size: (i32, i32), rect: [f32; 4], dt: f32) -> Option<glow::Texture> {
        unsafe {
            if self.failed {
                return None;
            }
            let (tex, weights) = link.current()?;
            match self.prepare(gl, version, tex, &weights, size, rect) {
                Ok(()) => {}
                Err(e) => {
                    log::warn!("textures: {e}");
                    self.failed = true;
                    return None;
                }
            }
            let (Some(step), Some(show), Some(w)) = (self.step, self.show, self.weights) else { return None };
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(Some(vao));

            // A few steps a frame, however fast frames come (a pause doesn't run it on).
            self.owed = (self.owed + dt.clamp(0.0, 0.1) * STEPS_A_SECOND).min(4.0);
            let steps = (self.owed.floor() as u32).max(1);
            self.owed -= steps as f32;
            gl.use_program(Some(step));
            let u = |name: &str| gl.get_uniform_location(step, name);
            for (i, name) in ["u_s0", "u_s1", "u_s2", "u_w", "u_video", "u_flow"].into_iter().enumerate() {
                gl.uniform_1_i32(u(name).as_ref(), i as i32);
            }
            gl.uniform_1_f32(u("u_has_flow").as_ref(), if flow.is_some() { 1.0 } else { 0.0 });
            gl.uniform_4_f32(u("u_rect").as_ref(), rect[0], rect[1], rect[2], rect[3]);
            gl.uniform_1_f32(u("u_moved").as_ref(), dt.clamp(0.0, 0.1) / steps as f32);
            gl.uniform_1_f32(u("u_feed").as_ref(), FEED);
            gl.active_texture(glow::TEXTURE3);
            gl.bind_texture(glow::TEXTURE_2D, Some(w));
            gl.active_texture(glow::TEXTURE4);
            gl.bind_texture(glow::TEXTURE_2D, Some(video));
            gl.active_texture(glow::TEXTURE5);
            gl.bind_texture(glow::TEXTURE_2D, flow);
            gl.viewport(0, 0, self.grid.0, self.grid.1);
            for _ in 0..steps {
                let (from, to) = (self.front, 1 - self.front);
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.cells[to].fbo));
                for (k, t) in self.cells[from].textures.iter().enumerate() {
                    gl.active_texture(glow::TEXTURE0 + k as u32);
                    gl.bind_texture(glow::TEXTURE_2D, Some(*t));
                }
                gl.uniform_1_f32(u("u_seed").as_ref(), (self.steps % 1000) as f32 * 1.618);
                gl.uniform_1_f32(u("u_fresh").as_ref(), if self.steps == 0 { 1.0 } else { 0.0 });
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
                self.front = to;
                self.steps += 1;
            }

            // The picture of it, the size of the video.
            let (out, _) = self.out.as_ref()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(out.fbo));
            gl.viewport(0, 0, size.0, size.1);
            gl.use_program(Some(show));
            let u = |name: &str| gl.get_uniform_location(show, name);
            gl.uniform_1_i32(u("u_video").as_ref(), 0);
            gl.uniform_1_i32(u("u_s0").as_ref(), 1);
            gl.uniform_4_f32(u("u_rect").as_ref(), rect[0], rect[1], rect[2], rect[3]);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(video));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.cells[self.front].textures[0]));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            for unit in 1..6 {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            Some(out.texture)
        }
    }

    /// Compiles, makes the grid for the picture's shape and the output, and loads the weights
    /// if the texture's changed.
    unsafe fn prepare(&mut self, gl: &glow::Context, version: egui_glow::ShaderVersion, tex: &'static Texture, weights: &[f32], size: (i32, i32), rect: [f32; 4]) -> Result<(), String> {
        unsafe {
            let decl = version.version_declaration();
            if self.step.is_none() {
                self.step = Some(compile(gl, &vertex_source(decl), &step_source(decl, version.is_embedded()))?);
                self.show = Some(compile(gl, &vertex_source(decl), &show_source(decl, version.is_embedded()))?);
            }
            let (pw, ph) = ((rect[2] - rect[0]) * size.0 as f32, (rect[3] - rect[1]) * size.1 as f32);
            let grid = (((CELLS_HIGH as f32 * pw / ph.max(1.0)).round() as i32).clamp(16, 1024), CELLS_HIGH);
            if self.grid != grid || self.cells.is_empty() {
                self.free_cells(gl);
                for _ in 0..2 {
                    self.cells.push(cells(gl, grid)?);
                }
                self.grid = grid;
                self.steps = 0;
            }
            if self.model != Some(tex.model) {
                let t = match self.weights {
                    Some(t) => t,
                    None => {
                        let t = gl.create_texture()?;
                        self.weights = Some(t);
                        t
                    }
                };
                gl.bind_texture(glow::TEXTURE_2D, Some(t));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
                let bytes: &[u8] = std::slice::from_raw_parts(weights.as_ptr().cast(), weights.len() * 4);
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA32F as i32, (HIDDEN / 4) as i32, ROWS as i32, 0, glow::RGBA, glow::FLOAT, glow::PixelUnpackData::Slice(Some(bytes)));
                for (param, value) in [(glow::TEXTURE_MIN_FILTER, glow::NEAREST), (glow::TEXTURE_MAG_FILTER, glow::NEAREST)] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
                }
                gl.bind_texture(glow::TEXTURE_2D, None);
                self.model = Some(tex.model);
            }
            if self.out.as_ref().is_none_or(|(_, s)| *s != size) {
                if let Some((t, _)) = self.out.take() {
                    crate::signals::delete_target(gl, t);
                }
                self.out = Some((create_target(gl, size, &[(glow::RGBA8, glow::UNSIGNED_BYTE)])?, size));
            }
            Ok(())
        }
    }

    unsafe fn free_cells(&mut self, gl: &glow::Context) {
        unsafe {
            for c in self.cells.drain(..) {
                gl.delete_framebuffer(c.fbo);
                for t in c.textures {
                    gl.delete_texture(t);
                }
            }
        }
    }

    /// Frees everything.
    ///
    /// # Safety
    /// GL thread, `gl` current.
    pub unsafe fn release(&mut self, gl: &glow::Context) {
        unsafe {
            self.free_cells(gl);
            for p in [self.step.take(), self.show.take()].into_iter().flatten() {
                gl.delete_program(p);
            }
            if let Some(t) = self.weights.take() {
                gl.delete_texture(t);
            }
            if let Some((t, _)) = self.out.take() {
                crate::signals::delete_target(gl, t);
            }
            *self = Self { failed: self.failed, ..Default::default() };
        }
    }
}

/// A grid of cells (all zero): three RGBA16F textures, each a framebuffer attachment.
unsafe fn cells(gl: &glow::Context, (w, h): (i32, i32)) -> Result<Cells, String> {
    unsafe {
        let mut textures = Vec::new();
        for _ in 0..3 {
            let t = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA16F as i32, w, h, 0, glow::RGBA, glow::HALF_FLOAT, glow::PixelUnpackData::Slice(None));
            for (param, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
            }
            textures.push(t);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        let fbo = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        for (k, t) in textures.iter().enumerate() {
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0 + k as u32, glow::TEXTURE_2D, Some(*t), 0);
        }
        gl.draw_buffers(&[glow::COLOR_ATTACHMENT0, glow::COLOR_ATTACHMENT1, glow::COLOR_ATTACHMENT2]);
        let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
        if complete {
            gl.disable(glow::SCISSOR_TEST);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        let textures: [glow::Texture; 3] = [textures[0], textures[1], textures[2]];
        if !complete {
            gl.delete_framebuffer(fbo);
            for t in textures {
                gl.delete_texture(t);
            }
            return Err("textures: the grid's framebuffer isn't complete (no float colour buffers?)".into());
        }
        Ok(Cells { fbo, textures })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes() {
        assert_eq!(base64("aGVsbG8gd29ybGQ=").unwrap(), b"hello world");
        assert_eq!(base64("AAEC/w==").unwrap(), [0, 1, 2, 255]);
    }

    #[test]
    fn effects_can_pick_every_texture_there_is() {
        let ids: Vec<&str> = TEXTURES.iter().map(|t| t.id).collect();
        assert_eq!(ids, flex_core::effects::TEXTURES);
    }

    /// The models file (`FLEX_NCA_MODELS=models.json cargo test -p flex nca_models_read -- --ignored`)
    /// has every texture effects can pick, with weights in a sensible range.
    #[test]
    #[ignore]
    fn nca_models_read() {
        let models = read_models(&std::fs::read(std::env::var("FLEX_NCA_MODELS").expect("FLEX_NCA_MODELS")).unwrap()).unwrap();
        assert_eq!(models.len(), 66);
        for t in TEXTURES {
            let w = &models[t.model];
            assert_eq!(w.len(), ROWS * HIDDEN);
            let biggest = w.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(biggest > 0.01 && biggest < 3.0, "{}: {biggest}", t.id);
        }
    }
}
