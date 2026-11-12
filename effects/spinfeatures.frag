// Spinning Features: each eye and mouth is a disc of picture turning on the spot. On the mesh where
// there is one, so they stay on the face however it's turned. Fading in, the discs grow from
// nothing.

// Where things are on a face, as face_uv has it.
const vec2 RIGHT_EYE = vec2(0.35, 0.62);
const vec2 LEFT_EYE = vec2(0.65, 0.62);
const vec2 MOUTH = vec2(0.5, 0.305);

// How much of a soft oval on the face `a` is in.
float blob(vec2 a, vec2 centre, vec2 half_size) {
    return 1.0 - smoothstep(0.7, 1.0, length((a - centre) / half_size));
}

// `c` with the face's own pixels at `from` laid over it, by `amount`.
vec3 from_face(vec3 c, int face, vec2 from, float amount) {
    if (amount <= 0.0) return c;
    vec4 skin = face_texture(face, from);
    return mix(c, skin.rgb, skin.a * amount);
}

vec3 spun(vec3 c, vec2 p, vec2 centre, Face f, vec2 half_size, float angle, float seen, vec2 aspect) {
    float amount = oval(p, centre, f, vec2(max(half_size.x, half_size.y))) * seen;
    if (amount <= 0.0) return c;
    return mix(c, texture(u_video, (centre + rotate(p - centre, angle)) / aspect).rgb, amount);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float t = u_video_time * s_speed / 100.0 + u_beat * s_react / 100.0;
    if (u_amount <= 0.0) return vec4(c, 1.0);
    float scale = s_size / 100.0 * u_amount;
    int there = face_there(uv);
    if (there >= 0 && meshed(there)) {
        vec2 a = face_uv(uv);
        float on = on_face(uv) * u_face_seen[there];
        c = from_face(c, there, RIGHT_EYE + rotate(a - RIGHT_EYE, t * 2.0), blob(a, RIGHT_EYE, vec2(0.126 * scale)) * on);
        c = from_face(c, there, LEFT_EYE + rotate(a - LEFT_EYE, -t * 2.0), blob(a, LEFT_EYE, vec2(0.126 * scale)) * on);
        c = from_face(c, there, MOUTH + rotate(a - MOUTH, t * 1.3), blob(a, MOUTH, vec2(0.186 * scale)) * on);
    }
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i];
        if (seen <= 0.0 || meshed(i)) continue;
        Face f = face_frame(i, aspect);
        c = spun(c, p, f.right, f, vec2(f.spread * 0.42) * scale, t * 2.0, seen, aspect);
        c = spun(c, p, f.left, f, vec2(f.spread * 0.42) * scale, -t * 2.0, seen, aspect);
        c = spun(c, p, f.mouth, f, vec2(f.spread * 0.62) * scale, t * 1.3, seen, aspect);
    }
    return vec4(c, 1.0);
}
