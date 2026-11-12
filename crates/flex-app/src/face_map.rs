//! The face mesh drawn into textures, so an effect can ask about any pixel: which part of a face
//! is there, and where that is on the face. Each face is also unwrapped into an atlas, so its
//! skin can be drawn somewhere else. All of it is redrawn each paint from the mesh for the frame
//! on screen, so it's always of the picture being shown.

use eframe::glow::{self, HasContext};

/// One record per triangle corner, from `tools/face/convert_map.py` in flex-depth.
const CORNERS: &[u8] = include_bytes!("face_map.bin");
const CORNER_BYTES: usize = 12;
const MAP_SIZE: (i32, i32) = (1280, 720);
/// The atlas holds eight faces, four across and two down.
const ATLAS_SIZE: (i32, i32) = (2048, 1024);

/// The first lines of a shader: older desktop GLSL needs asking for fixed locations.
fn start(version: &str, embedded: bool) -> String {
    if embedded { format!("{version}\n") } else { format!("{version}\n#extension GL_ARB_explicit_attrib_location : require\n") }
}

fn vertex_map(version: &str) -> String {
    format!(
        "{version}layout(location = 0) in float a_point;\nlayout(location = 1) in vec2 a_uv;\nlayout(location = 2) in float a_part;\n\
         uniform highp sampler2D u_mesh;\nuniform int u_face;\nout vec2 v_face;\nout float v_part;\n\
         void main() {{\n    vec3 p = texelFetch(u_mesh, ivec2(int(a_point), u_face), 0).xyz;\n    v_face = a_uv;\n    v_part = a_part;\n\
         gl_Position = vec4(p.xy * 2.0 - 1.0, clamp(p.z * 2.0, -0.99, 0.99), 1.0);\n}}\n"
    )
}

fn fragment_map(version: &str, embedded: bool) -> String {
    let precision = if embedded { "precision highp float;\nprecision highp int;\n" } else { "" };
    format!(
        "{version}{precision}in vec2 v_face;\nin float v_part;\nlayout(location = 0) out vec4 map;\nlayout(location = 1) out vec4 parts;\nlayout(location = 2) out vec4 parts2;\n\
         uniform highp sampler2D u_mesh;\nuniform int u_face;\nuniform float u_fade;\nuniform float u_aspect;\nuniform vec2 u_size;\n\
         vec2 mesh(int n) {{ return texelFetch(u_mesh, ivec2(n, u_face), 0).xy * vec2(u_aspect, 1.0); }}\n\
         float iris(vec2 here, int centre) {{\n    vec2 c = mesh(centre);\n    float r = 0.25 * (length(mesh(centre + 1) - c) + length(mesh(centre + 2) - c) + length(mesh(centre + 3) - c) + length(mesh(centre + 4) - c));\n\
             return 1.0 - smoothstep(r * 0.92, r * 1.04, length(here - c));\n}}\n\
         void main() {{\n    int part = int(v_part + 0.5);\n    vec2 here = gl_FragCoord.xy / u_size * vec2(u_aspect, 1.0);\n\
             float looking = part == 4 ? max(iris(here, 468), iris(here, 473)) : 0.0;\n\
             map = vec4(v_face * u_fade, float(u_face + 1) / 8.0, u_fade);\n\
             parts = vec4(part == 0 ? 1.0 : 0.0, part == 1 ? 1.0 : 0.0, part == 2 ? 1.0 : 0.0, part == 3 ? 1.0 : 0.0) * u_fade;\n\
             parts2 = vec4(part == 4 ? 1.0 : 0.0, looking, part == 5 ? 1.0 : 0.0, 0.0) * u_fade;\n}}\n"
    )
}

fn vertex_atlas(version: &str) -> String {
    format!(
        "{version}layout(location = 0) in float a_point;\nlayout(location = 1) in vec2 a_uv;\n\
         uniform highp sampler2D u_mesh;\nuniform int u_face;\nout vec2 v_video;\n\
         void main() {{\n    v_video = texelFetch(u_mesh, ivec2(int(a_point), u_face), 0).xy;\n\
             vec2 tile = (vec2(float(u_face % 4), float(u_face / 4)) + a_uv * 0.96 + 0.02) / vec2(4.0, 2.0);\n\
             gl_Position = vec4(tile * 2.0 - 1.0, 0.0, 1.0);\n}}\n"
    )
}

fn fragment_atlas(version: &str, embedded: bool) -> String {
    let precision = if embedded { "precision highp float;\n" } else { "" };
    format!("{version}{precision}in vec2 v_video;\nout vec4 colour;\nuniform sampler2D u_video;\nuniform float u_fade;\nvoid main() {{\n    colour = vec4(texture(u_video, v_video).rgb, u_fade);\n}}\n")
}

unsafe fn texture(gl: &glow::Context, internal: u32, kind: u32, (w, h): (i32, i32)) -> Result<glow::Texture, String> {
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(glow::TEXTURE_2D, 0, internal as i32, w, h, 0, glow::RGBA, kind, glow::PixelUnpackData::Slice(None));
        for (param, value) in [(glow::TEXTURE_MIN_FILTER, glow::LINEAR), (glow::TEXTURE_MAG_FILTER, glow::LINEAR), (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE), (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE)] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        Ok(texture)
    }
}

pub struct FaceMap {
    fbo: glow::Framebuffer,
    depth: glow::Renderbuffer,
    /// Position on the face, which face, how much of one; then the parts.
    map: glow::Texture,
    parts: glow::Texture,
    parts2: glow::Texture,
    atlas_fbo: glow::Framebuffer,
    atlas: glow::Texture,
    vertices: glow::Buffer,
    vao: glow::VertexArray,
    map_program: glow::Program,
    atlas_program: glow::Program,
}

impl FaceMap {
    pub unsafe fn new(gl: &glow::Context, version: &str, embedded: bool) -> Result<Self, String> {
        unsafe {
            let first = start(version, embedded);
            // Everything is made before any of it is set up, so that if something can't be,
            // what was made can be deleted.
            let map_program = crate::effects::compile(gl, &vertex_map(&first), &fragment_map(&first, embedded));
            let atlas_program = crate::effects::compile(gl, &vertex_atlas(&first), &fragment_atlas(&first, embedded));
            let map = texture(gl, glow::RGBA16F, glow::FLOAT, MAP_SIZE);
            let parts = texture(gl, glow::RGBA8, glow::UNSIGNED_BYTE, MAP_SIZE);
            let parts2 = texture(gl, glow::RGBA8, glow::UNSIGNED_BYTE, MAP_SIZE);
            let atlas = texture(gl, glow::RGBA8, glow::UNSIGNED_BYTE, ATLAS_SIZE);
            let depth = gl.create_renderbuffer();
            let fbo = gl.create_framebuffer();
            let atlas_fbo = gl.create_framebuffer();
            let vao = gl.create_vertex_array();
            let vertices = gl.create_buffer();
            let all = || -> Result<Self, String> {
                Ok(Self {
                    fbo: fbo.clone()?,
                    depth: depth.clone()?,
                    map: map.clone()?,
                    parts: parts.clone()?,
                    parts2: parts2.clone()?,
                    atlas_fbo: atlas_fbo.clone()?,
                    atlas: atlas.clone()?,
                    vertices: vertices.clone()?,
                    vao: vao.clone()?,
                    map_program: map_program.clone()?,
                    atlas_program: atlas_program.clone()?,
                })
            };
            let made = match all() {
                Ok(made) => made,
                Err(e) => {
                    for p in [map_program, atlas_program].into_iter().flatten() {
                        gl.delete_program(p);
                    }
                    for t in [map, parts, parts2, atlas].into_iter().flatten() {
                        gl.delete_texture(t);
                    }
                    for f in [fbo, atlas_fbo].into_iter().flatten() {
                        gl.delete_framebuffer(f);
                    }
                    if let Ok(depth) = depth {
                        gl.delete_renderbuffer(depth);
                    }
                    if let Ok(vao) = vao {
                        gl.delete_vertex_array(vao);
                    }
                    if let Ok(vertices) = vertices {
                        gl.delete_buffer(vertices);
                    }
                    return Err(e);
                }
            };
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(made.depth));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH_COMPONENT24, MAP_SIZE.0, MAP_SIZE.1);
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(made.fbo));
            for (i, t) in [made.map, made.parts, made.parts2].into_iter().enumerate() {
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0 + i as u32, glow::TEXTURE_2D, Some(t), 0);
            }
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::RENDERBUFFER, Some(made.depth));
            gl.draw_buffers(&[glow::COLOR_ATTACHMENT0, glow::COLOR_ATTACHMENT1, glow::COLOR_ATTACHMENT2]);
            let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(made.atlas_fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(made.atlas), 0);
            let complete = complete && gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);

            let floats = corners();
            gl.bind_vertex_array(Some(made.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(made.vertices));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, std::slice::from_raw_parts(floats.as_ptr().cast::<u8>(), floats.len() * 16), glow::STATIC_DRAW);
            let stride = 4 * 4;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 1, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 4);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 1, glow::FLOAT, false, stride, 12);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            if !complete {
                made.release(gl);
                return Err("the face map's framebuffers are incomplete".into());
            }
            Ok(made)
        }
    }

    /// The map, its two sets of parts, and the atlas.
    pub fn textures(&self) -> [glow::Texture; 4] {
        [self.map, self.parts, self.parts2, self.atlas]
    }

    /// Draws `faces` (slot, how much it's showing). `mesh` is `u_mesh`, already up to date for
    /// the picture in `video`; `aspect` is the picture's width over its height.
    pub unsafe fn draw(&self, gl: &glow::Context, mesh: glow::Texture, video: glow::Texture, aspect: f32, faces: &[(usize, f32)]) {
        let count = (CORNERS.len() / CORNER_BYTES) as i32;
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(mesh));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(video));

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            gl.viewport(0, 0, MAP_SIZE.0, MAP_SIZE.1);
            gl.depth_mask(true);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.enable(glow::DEPTH_TEST);
            gl.use_program(Some(self.map_program));
            let at = |name: &str| gl.get_uniform_location(self.map_program, name);
            gl.uniform_1_i32(at("u_mesh").as_ref(), 1);
            gl.uniform_1_f32(at("u_aspect").as_ref(), aspect);
            gl.uniform_2_f32(at("u_size").as_ref(), MAP_SIZE.0 as f32, MAP_SIZE.1 as f32);
            for &(slot, fade) in faces {
                // Each face's depths are only comparable with its own.
                gl.clear(glow::DEPTH_BUFFER_BIT);
                gl.uniform_1_i32(at("u_face").as_ref(), slot as i32);
                gl.uniform_1_f32(at("u_fade").as_ref(), fade);
                gl.draw_arrays(glow::TRIANGLES, 0, count);
            }
            gl.disable(glow::DEPTH_TEST);

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.atlas_fbo));
            gl.viewport(0, 0, ATLAS_SIZE.0, ATLAS_SIZE.1);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.use_program(Some(self.atlas_program));
            let at = |name: &str| gl.get_uniform_location(self.atlas_program, name);
            gl.uniform_1_i32(at("u_mesh").as_ref(), 1);
            gl.uniform_1_i32(at("u_video").as_ref(), 0);
            for &(slot, fade) in faces {
                gl.uniform_1_i32(at("u_face").as_ref(), slot as i32);
                gl.uniform_1_f32(at("u_fade").as_ref(), fade);
                gl.draw_arrays(glow::TRIANGLES, 0, count);
            }
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }

    pub unsafe fn release(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_framebuffer(self.fbo);
            gl.delete_framebuffer(self.atlas_fbo);
            gl.delete_renderbuffer(self.depth);
            for t in [self.map, self.parts, self.parts2, self.atlas] {
                gl.delete_texture(t);
            }
            gl.delete_buffer(self.vertices);
            gl.delete_vertex_array(self.vao);
            gl.delete_program(self.map_program);
            gl.delete_program(self.atlas_program);
        }
    }
}

/// The corners as floats: point, u, v, part.
fn corners() -> Vec<[f32; 4]> {
    CORNERS
        .as_chunks::<CORNER_BYTES>()
        .0
        .iter()
        .map(|c| {
            let point = u16::from_le_bytes([c[0], c[1]]);
            let part = u16::from_le_bytes([c[2], c[3]]);
            [f32::from(point), f32::from_le_bytes([c[4], c[5], c[6], c[7]]), f32::from_le_bytes([c[8], c[9], c[10], c[11]]), f32::from(part)]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_triangle_has_three_corners_on_the_mesh() {
        let corners = corners();
        assert_eq!(corners.len() % 3, 0);
        assert_eq!(corners.len(), 898 * 3);
        assert!(corners.iter().all(|c| c[0] < 468.0 && (0.0..=1.0).contains(&c[1]) && (0.0..=1.0).contains(&c[2]) && c[3] <= 5.0));
        // Corners of a triangle are all one part.
        assert!(corners.as_chunks::<3>().0.iter().all(|t| t[0][3] == t[1][3] && t[1][3] == t[2][3]));
        for part in 0..6 {
            assert!(corners.iter().any(|c| c[3] == part as f32), "no triangles for part {part}");
        }
    }
}
