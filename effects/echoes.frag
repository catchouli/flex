// Echoes: real older frames, each tinted its own colour and added in only where it differs from
// now, so still things stay as they are. Fading in, the echoes spread back in time from now and
// grow stronger.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec3 sum = vec3(0.0);
    int count = int(s_copies + 0.5);
    float gap = s_gap / 100.0 * (1.0 + 0.5 * u_bass * s_react / 100.0) * u_amount;
    for (int i = count; i >= 1; i--) {
        float n = float(i);
        vec3 then = history_at(uv, n * gap);
        float differs = smoothstep(0.06, 0.25, length(then - c));
        vec3 tint = mix(vec3(1.0), hue(n * 0.17 + u_time * 0.05) * 1.8, s_colour / 100.0);
        sum = max(sum, then * tint * differs * (1.0 - (n - 1.0) / float(count + 1)));
    }
    return vec4(max(c, sum * s_strength / 60.0 * u_amount), 1.0);
}
