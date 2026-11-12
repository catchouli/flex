// Topographic Map: depth is height. Contour lines every so often, a heavier one every fifth, a
// colour for each height, and shading as if lit from the north-west. Fading in, the map comes up
// over the film.

vec3 land(float h) {
    if (h < 0.18) return mix(vec3(0.05, 0.2, 0.5), vec3(0.2, 0.55, 0.75), h / 0.18);
    if (h < 0.45) return mix(vec3(0.3, 0.6, 0.25), vec3(0.75, 0.75, 0.4), (h - 0.18) / 0.27);
    if (h < 0.75) return mix(vec3(0.75, 0.75, 0.4), vec3(0.55, 0.32, 0.16), (h - 0.45) / 0.3);
    return mix(vec3(0.55, 0.32, 0.16), vec3(0.98), (h - 0.75) / 0.25);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(0.0, 0.0, 0.0, 1.0);
    float h = depth_at(uv);
    float n = s_lines;
    float at = h * n + u_bass * s_react / 100.0 * 0.5;
    float line = fwidth(at) * 1.2 + 0.015;
    float d = abs(fract(at - 0.5) - 0.5);
    float thin = 1.0 - smoothstep(line * 0.5, line, d);
    float five = abs(fract(at / 5.0 - 0.5) - 0.5) * 5.0;
    float thick = 1.0 - smoothstep(line, line * 1.8, five);
    vec2 slope = depth_slope(uv, 3.0);
    float shade = clamp(0.8 + dot(slope, vec2(1.0, -1.0)) * 14.0, 0.45, 1.3);
    vec3 map;
    if (s_style < 0.5) {
        map = land(floor(h * n) / n) * shade;
        map = mix(map, vec3(0.2, 0.12, 0.06), max(thin * 0.55, thick));
    } else if (s_style < 1.5) {
        map = vec3(0.94, 0.9, 0.8) * shade;
        map = mix(map, vec3(0.5, 0.3, 0.15), max(thin * 0.7, thick));
    } else {
        map = vec3(0.01, 0.01, 0.04) + (0.5 + 0.5 * cos(6.2831853 * (h * 1.3 + vec3(0.6, 0.9, 0.2)))) * max(thin * 0.7, thick) * 1.5;
    }
    float film = s_film / 100.0 * (1.0 - max(thin, thick));
    if (u_amount < 1.0) film = mix(1.0, film, u_amount);
    return vec4(mix(map, c, film), 1.0);
}
