// Neon Grid: outlines from jumps in depth and in the picture, coloured by distance, and a grid
// laid out in perspective using depth as how far away each point is. Fading in, the film dims to
// its setting as the lines and grid come up.

float luma(vec2 uv) {
    return dot(texture(u_video, uv).rgb, vec3(0.3, 0.59, 0.11));
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(0.0, 0.0, 0.0, 1.0);
    float near = depth_at(uv);
    vec2 e = 1.5 / u_resolution * u_resolution.y / 720.0;
    float picture = length(vec2(luma(uv + vec2(e.x, 0.0)) - luma(uv - vec2(e.x, 0.0)), luma(uv + vec2(0.0, e.y)) - luma(uv - vec2(0.0, e.y))));
    float shape = length(depth_slope(uv, 2.0));
    float edge = smoothstep(0.08, 0.25, picture) * 0.8 + smoothstep(0.02, 0.09, shape);
    vec3 tint = hue(s_colour / 360.0 + near * 0.5);
    vec3 out_colour = c * (u_amount < 1.0 ? mix(1.0, s_film / 100.0, u_amount) : s_film / 100.0);
    out_colour += tint * edge * s_lines / 100.0 * u_amount * (1.0 + u_beat * s_react / 100.0) * 1.4;

    float z = 1.0 / (0.08 + near);
    float x = (uv.x - 0.5) * z * 2.0;
    vec2 cells = abs(fract(vec2(x, z - u_video_time * 0.5) * 2.0) - 0.5);
    float grid_line = 1.0 - smoothstep(0.0, 0.04 + 0.02 * z, min(cells.x, cells.y));
    if (u_world_ready > 0.0) {
        // Once the camera's known, a grid that stays where it is in the world.
        vec3 world = world_at(uv);
        vec2 fixed_cells = abs(fract(world.xz * 2.0) - 0.5);
        grid_line = mix(grid_line, 1.0 - smoothstep(0.0, 0.04 + 0.02 * z, min(fixed_cells.x, fixed_cells.y)), u_world_ready);
    }
    out_colour += hue(s_colour / 360.0 + 0.5) * grid_line * s_grid / 100.0 * u_amount * 0.5 * (1.0 - smoothstep(0.1, 0.75, near)) * (0.5 + u_bass * s_react / 100.0);
    return vec4(out_colour, 1.0);
}
