// Invisible Man: people are replaced by the room behind them, which Flex builds up from whatever
// it has seen since the last cut, leaving a little bent air around where they are. Fading in,
// people dissolve away in patches.

float someone(vec2 uv) {
    float most = person_at(uv);
    for (int i = 0; i < 8; i++) {
        float a = float(i) * 0.7853982;
        most = max(most, person_at(uv + vec2(cos(a), sin(a)) * vec2(0.012 * u_resolution.y / u_resolution.x, 0.012)));
    }
    return most;
}

vec4 effect(vec2 uv) {
    vec3 video = texture(u_video, uv).rgb;
    if (u_has_plate < 0.5) return vec4(video, 1.0);
    float cover = someone(uv);
    float clear = 1.0 - smoothstep(0.08, 0.3, cover);
    float gone = clamp((u_amount * 1.25 - noise(uv * vec2(u_resolution.x / u_resolution.y, 1.0) * 14.0)) / 0.25, 0.0, 1.0);
    if (gone < 1.0) clear = mix(1.0, clear, gone);
    if (clear >= 1.0) return vec4(video, 1.0);
    float rim = smoothstep(0.08, 0.3, cover) * (1.0 - smoothstep(0.3, 0.8, cover));
    vec2 ripple = vec2(sin(uv.y * 70.0 + u_time * 5.0), cos(uv.x * 60.0 - u_time * 4.0)) * s_shimmer / 100.0 * 0.004 * (0.3 + rim) * gone;
    return vec4(mix(plate_at(uv + ripple), video, clear), 1.0);
}
