// Perfect Symmetry: folds the head over the line down the middle of the face. Only faces with a
// mesh: without one it's usually a face in profile, and there's nothing there to mirror. Fading
// in, the mirrored half comes up over the real one.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i];
        if (seen <= 0.0 || !meshed(i)) continue;
        Face f = face_frame(i, aspect);
        vec2 mid = 0.5 * (f.eyes + f.mouth);
        float amount = oval(p, mid + f.up * f.height * 0.2, f, vec2(f.spread * 1.45, f.height * 1.9)) * seen * u_amount;
        if (amount <= 0.0) continue;
        vec2 rel = p - mid;
        float x = dot(rel, f.across);
        // Use the half nearer the camera: the nose points away from it.
        float keep = s_side > 1.5 ? 1.0 : (s_side > 0.5 ? -1.0 : (dot(f.nose - mid, f.across) > 0.0 ? -1.0 : 1.0));
        vec2 from = p + f.across * (keep * abs(x) - x);
        c = mix(c, texture(u_video, from / aspect).rgb, amount);
        // With a mesh the fold follows the middle of the face even when it's turned.
        if (face_there(uv) == i) {
            vec2 a = face_uv(uv);
            float kept_x = keep < 0.0 ? min(a.x, 1.0 - a.x) : max(a.x, 1.0 - a.x);
            c = mix(c, face_texture(i, vec2(kept_x, a.y)).rgb, on_face(uv) * seen * u_amount);
        }
    }
    return vec4(c, 1.0);
}
