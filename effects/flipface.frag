// Upside-Down Face: flips the eyes and mouth over where they are, or the whole face. On the mesh
// where there is one, so they stay on the face however it's turned. Fading in, the flipped ovals
// grow out from their middles, and a whole face on the mesh comes through gradually.

// Where things are on a face, as face_uv has it.
const vec2 RIGHT_EYE = vec2(0.35, 0.62);
const vec2 LEFT_EYE = vec2(0.65, 0.62);
const vec2 MOUTH = vec2(0.5, 0.305);

// How much of a soft oval on the face `a` is in.
float blob(vec2 a, vec2 centre, vec2 half_size) {
    // Grown in with u_amount, from nothing.
    half_size *= max(u_amount, 1e-3);
    return (1.0 - smoothstep(0.7, 1.0, length((a - centre) / half_size))) * step(1e-3, u_amount);
}

// `c` with the face's own pixels at `from` laid over it, by `amount`.
vec3 from_face(vec3 c, int face, vec2 from, float amount) {
    if (amount <= 0.0) return c;
    vec4 skin = face_texture(face, from);
    return mix(c, skin.rgb, skin.a * amount);
}

vec3 flipped(vec3 c, vec2 p, vec2 centre, Face f, vec2 half_size, float seen, vec2 aspect) {
    float amount = oval(p, centre, f, half_size * max(u_amount, 1e-3)) * seen * step(1e-3, u_amount);
    if (amount <= 0.0) return c;
    vec2 rel = p - centre;
    vec2 from = centre + f.across * dot(rel, f.across) - f.up * dot(rel, f.up);
    return mix(c, texture(u_video, from / aspect).rgb, amount);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    int there = face_there(uv);
    if (there >= 0 && meshed(there)) {
        vec2 a = face_uv(uv);
        float on = on_face(uv) * u_face_seen[there];
        if (s_style > 0.5) {
            // From chin to hairline.
            c = from_face(c, there, vec2(a.x, 0.94 - a.y), on * u_amount);
        } else {
            c = from_face(c, there, vec2(a.x, 2.0 * RIGHT_EYE.y - a.y), blob(a, RIGHT_EYE, vec2(0.15, 0.108)) * on);
            c = from_face(c, there, vec2(a.x, 2.0 * LEFT_EYE.y - a.y), blob(a, LEFT_EYE, vec2(0.15, 0.108)) * on);
            c = from_face(c, there, vec2(a.x, 2.0 * MOUTH.y - a.y), blob(a, MOUTH, vec2(0.225, 0.15)) * on);
        }
    }
    for (int i = 0; i < u_face_count; i++) {
        if (u_face_seen[i] <= 0.0 || meshed(i)) continue;
        Face f = face_frame(i, aspect);
        if (s_style > 0.5) {
            c = flipped(c, p, 0.5 * (f.eyes + f.mouth), f, vec2(f.spread * 1.35, f.height * 1.35), u_face_seen[i], aspect);
        } else {
            vec2 eye = vec2(f.spread * 0.5, f.spread * 0.36);
            c = flipped(c, p, f.right, f, eye, u_face_seen[i], aspect);
            c = flipped(c, p, f.left, f, eye, u_face_seen[i], aspect);
            c = flipped(c, p, f.mouth, f, vec2(f.spread * 0.75, f.height * 0.5), u_face_seen[i], aspect);
        }
    }
    return vec4(c, 1.0);
}
