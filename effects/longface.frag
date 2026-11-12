// Long Face: everything below the eyes is pulled downwards, and the chest gives way underneath.
// Fading in, the pull grows from none.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    vec2 q = p;
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i];
        if (seen <= 0.0) continue;
        Face f = face_frame(i, aspect);
        vec2 rel = p - f.eyes;
        float across = abs(dot(rel, f.across));
        float down = -dot(rel, f.up);
        float wide = f.spread * 1.25;
        float side = 1.0 - smoothstep(0.75, 1.25, across / wide);
        if (side <= 0.0 || down < -f.height * 0.2) continue;
        float stretch = 1.0 + (s_length / 100.0 - 1.0 + u_bass * s_react / 100.0) * seen;
        float face_length = f.height * 1.75;
        float drawn = face_length * stretch;
        float give = f.height * 1.5;
        float from = down < drawn ? down / stretch : mix(face_length, down, smoothstep(0.0, 1.0, (down - drawn) / give));
        if (down >= drawn + give) continue;
        if (down < 0.0) from = down;
        q += f.up * (down - from) * side * u_amount;
    }
    return vec4(texture(u_video, q / aspect).rgb, 1.0);
}
