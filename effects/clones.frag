// Clone Army: copies people, smaller and further up the screen with each row, drawn from the
// back forwards, with the real people in front of them all. Fading in, the clones drop down into
// their places from above.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 1.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 horizon = vec2(0.5, 0.6);
    int rows = int(s_rows + 0.5);
    vec3 fog = vec3(0.55, 0.58, 0.65);
    for (int row = rows; row >= 1; row--) {
        float r = float(row);
        float size = pow(0.72, r);
        float bounce = abs(sin(u_video_time * 3.0 + r)) * 0.02 * (0.3 + u_beat * s_react / 50.0);
        for (int column = -row; column <= row; column++) {
            float shifted = (float(column) + (mod(r, 2.0) > 0.5 ? 0.0 : 0.5)) * 0.42;
            vec2 from = (uv - horizon - vec2(shifted * size, bounce + (1.0 - u_amount) * 1.2)) / size + horizon;
            if (from != clamp(from, u_video_rect.xy, u_video_rect.zw)) continue;
            float there = smoothstep(0.4, 0.7, person_at(from));
            if (there <= 0.0) continue;
            // Each row stands further back, so whatever's nearer hides it.
            if (depth_at(uv) > depth_at(from) * (1.0 - 0.1 * r) + 0.04) continue;
            vec3 clone = mix(texture(u_video, from).rgb, fog, (1.0 - size) * s_fog / 100.0);
            c = mix(c, clone, there);
        }
    }
    float real = smoothstep(0.4, 0.7, person_at(uv));
    return vec4(mix(c, texture(u_video, uv).rgb, real), 1.0);
}
