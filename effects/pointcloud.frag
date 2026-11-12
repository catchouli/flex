// Point Cloud: looks back along the camera's line for the nearest dot that would land on this
// pixel, by trying a range of depths from near to far. Fading in, the picture breaks from single
// pixels into growing dots that sway apart, and their colour turns cool.

vec3 cool(vec3 c) {
    return vec3(0.2, 0.75, 1.0) * dot(c, vec3(0.33)) * 1.8;
}

vec4 effect(vec2 uv) {
    float t = u_video_time;
    vec2 shape = vec2(u_resolution.y / u_resolution.x, 1.0);
    vec2 cam = vec2(sin(t * 0.5), 0.45 * sin(t * 0.37 + 1.0)) * 0.11 * s_sway / 100.0 * shape * s_depth / 100.0;
    // Not folded into the line above, so that at 1 the sums come out exactly as they did.
    if (u_amount < 1.0) cam *= u_amount;
    float size = mix(1.0, s_size * u_resolution.y / 720.0, u_amount);
    vec2 pixel_size = size / u_resolution;
    float radius = mix(0.75, 0.42 + 0.2 * u_bass * s_react / 100.0, u_amount);
    for (int k = 0; k <= 22; k++) {
        float guess = 1.0 - float(k) / 22.0;
        vec2 home = uv - cam * (guess - 0.5);
        vec2 cell = (floor(home / pixel_size) + 0.5) * pixel_size;
        if (cell != clamp(cell, u_video_rect.xy, u_video_rect.zw)) continue;
        float d = depth_at(cell);
        if (abs(d - guess) > 0.06) continue;
        vec2 landed = cell + cam * (d - 0.5);
        if (length((uv - landed) * u_resolution) < size * radius) {
            vec3 c = texture(u_video, cell).rgb * mix(1.0, 0.55 + 0.75 * d, u_amount);
            return vec4(mix(cool(c), c, mix(1.0, s_glow / 100.0, u_amount)), 1.0);
        }
    }
    return vec4(0.0, 0.0, 0.0, 1.0);
}
