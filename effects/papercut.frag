// Paper Theatre: depth is cut into a few flat sheets, drawn from the front back, each shifted
// sideways by more the nearer it is, with a shadow on whatever's just behind it. Fading in, the
// sheets start to sway apart and their rims and shadows come up.

vec4 effect(vec2 uv) {
    vec2 shape = vec2(u_resolution.y / u_resolution.x, 1.0);
    float t = u_video_time;
    float n = max(s_layers, 2.0);
    for (int k = int(n) - 1; k >= 0; k--) {
        float layer = float(k);
        float swing = s_sway / 100.0 * u_amount * (0.012 + 0.03 * layer / n);
        vec2 shift = vec2(sin(t * 0.8 + layer * 1.3), 0.4 * cos(t * 0.6 + layer)) * swing * shape * (1.0 + u_bass * s_react / 100.0);
        vec2 at = uv - shift;
        if (at != clamp(at, u_video_rect.xy, u_video_rect.zw)) continue;
        float level = floor(depth_at(at) * n);
        if (level < layer) {
            continue;
        }
        vec3 c = texture(u_video, at).rgb;
        // Paler at the rim of the sheet, where the card shows.
        float rim = 0.0;
        for (int j = 0; j < 6; j++) {
            float a = float(j) * 1.0471976;
            if (floor(depth_at(at + vec2(cos(a), sin(a)) * 0.006 * shape) * n) < layer) rim = 1.0;
        }
        c = mix(c, vec3(0.97, 0.95, 0.9), rim * s_edge / 100.0 * u_amount);
        // A shadow from the sheet in front.
        float over = 0.0;
        for (int j = 1; j <= 3; j++) {
            float near_level = floor(depth_at(at + vec2(-0.008, 0.012) * float(j) * shape) * n);
            if (near_level > level) over = max(over, 1.0 - float(j - 1) / 3.0);
        }
        c *= 1.0 - over * 0.6 * s_shadow / 100.0 * u_amount;
        return vec4(c, 1.0);
    }
    return vec4(0.0, 0.0, 0.0, 1.0);
}
