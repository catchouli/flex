// War Paint: marks drawn in the face's own coordinates, so they stay on the skin as it moves.
// Fading in, each mark is drawn on from its wide end.

vec3 soft_hue(float t) {
    return 0.55 + 0.45 * cos(6.2831853 * (t + vec3(0.0, 0.33, 0.67)));
}

float slash(vec2 p, vec2 a, vec2 b, float wide) {
    vec2 pa = p - a, ba = b - a;
    float along = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    float m = 1.0 - smoothstep(wide * (1.0 - along) * 0.7, wide * (1.0 - along) + 0.004, length(pa - ba * along));
    // Drawn on as far as it's faded in.
    if (u_amount < 1.0) m *= 1.0 - smoothstep(u_amount * 1.05 - 0.05, u_amount * 1.05, along);
    return m;
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    float on = on_face(uv);
    if (on <= 0.0) return vec4(c, 1.0);
    vec2 a = face_uv(uv);
    int who = face_there(uv);
    // Mirrored about the nose: x is how far out from it.
    vec2 p = vec2(abs(a.x - 0.5), a.y);
    float m = slash(p, vec2(0.12, 0.5), vec2(0.42, 0.56), 0.035);
    m = max(m, slash(p, vec2(0.13, 0.43), vec2(0.4, 0.44), 0.03));
    m = max(m, slash(p, vec2(0.15, 0.36), vec2(0.34, 0.32), 0.025));
    m = max(m, slash(p, vec2(0.0, 0.88), vec2(0.17, 0.72), 0.03));
    m = max(m, slash(p, vec2(0.0, 0.8), vec2(0.1, 0.68), 0.02));
    m = max(m, slash(p, vec2(0.0, 0.6), vec2(0.0, 0.47), 0.024));
    m = max(m, slash(p, vec2(0.0, 0.2), vec2(0.0, 0.08), 0.03));
    m = max(m, slash(p, vec2(0.07, 0.19), vec2(0.09, 0.11), 0.02));
    m *= (1.0 - eye_at(uv)) * (1.0 - lips_at(uv)) * (1.0 - mouth_at(uv)) * on;
    float beat = u_beat * s_react / 100.0;
    vec3 ink = soft_hue(float(who) * 0.27 + 0.55 + u_time * 0.02);
    float luma = dot(c, vec3(0.3, 0.59, 0.11));
    c = mix(c, ink * (0.25 + luma * 1.2), m);
    c += ink * m * (0.15 + s_glow / 100.0 * (0.5 + beat * 1.4));
    return vec4(c, 1.0);
}
