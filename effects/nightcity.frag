// Night City: the shot turned to night. The sky goes deep blue and fills with stars, buildings
// go dark with their windows lit one by one (some going on and off as the night goes by), real
// windows glow, streetlight slides over vehicles, and people are lit by the moon. The stars and
// windows stay put as the camera moves (camera = true). Fading in, night comes down gradually.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 1.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 pinned = shot_uv(uv) * aspect;
    float shot = mod(u_shot_start * 100.0, 971.0);
    float luma = dot(c, vec3(0.3, 0.59, 0.11));
    float night = s_dark / 100.0 * u_amount;
    // Everything: dark and blue, the brightest things kept a little.
    vec3 shown = mix(c, vec3(luma) * vec3(0.35, 0.45, 0.8), night * 0.8) * mix(1.0, 0.28 + 0.5 * luma, night);
    // People: moonlight from above, so they stand out a little.
    float person = smoothstep(0.4, 0.6, person_at(uv));
    vec3 moonlit = vec3(luma) * vec3(0.55, 0.65, 0.95) * (0.5 + 0.6 * max(normal_at(uv).y, 0.0));
    shown = mix(shown, moonlit, person * night * 0.8);
    // Sky.
    float sky = smoothstep(0.3, 0.6, sky_at(uv));
    if (sky > 0.0) {
        vec3 dusk = mix(vec3(0.02, 0.03, 0.09), vec3(0.06, 0.08, 0.2), smoothstep(1.0, 0.3, uv.y));
        vec2 cell = floor(pinned * 90.0);
        float star = step(0.985, hash(cell + shot));
        vec2 in_cell = fract(pinned * 90.0) - 0.5;
        float twinkle = 0.6 + 0.4 * sin(u_time * (2.0 + hash(cell) * 4.0) + hash(cell + 3.0) * 6.28);
        float sparkle = star * (1.0 - smoothstep(0.0, 0.18, length(in_cell))) * twinkle * s_stars / 100.0;
        shown = mix(shown, dusk + vec3(sparkle), sky * night);
    }
    // Buildings: rows of windows, some lit.
    float building = smoothstep(0.35, 0.65, building_at(uv)) * (1.0 - sky);
    if (building > 0.0) {
        float size = 0.03 * s_size / 100.0;
        vec2 w = pinned / vec2(size, size * 1.3);
        vec2 cell = floor(w);
        vec2 f = fract(w);
        float pane = step(0.18, f.x) * step(f.x, 0.82) * step(0.22, f.y) * step(f.y, 0.85);
        // Each window goes on or off now and then, at its own times.
        float era = floor(u_time / 7.0 + hash(cell + 11.0) * 9.0);
        float on = step(1.0 - s_lit / 100.0, hash(cell + shot + era * 0.37));
        vec3 lamp = mix(vec3(1.0, 0.72, 0.35), vec3(0.75, 0.85, 1.0), step(0.8, hash(cell + 5.0)));
        lamp *= 0.7 + 0.3 * hash(cell + 2.0);
        shown = mix(shown, shown * 0.6, building * night);
        shown += lamp * pane * on * building * night * 0.9;
    }
    // Real windows glow warm.
    float window = smoothstep(0.35, 0.65, window_at(uv));
    shown = mix(shown, vec3(1.0, 0.75, 0.4) * (0.5 + 0.5 * luma), window * night * 0.7);
    // Vehicles: streetlight sliding over their paint.
    float vehicle = smoothstep(0.35, 0.65, vehicle_at(uv));
    float sheen = pow(0.5 + 0.5 * sin(pinned.x * 18.0 - u_time * 2.5), 12.0);
    shown += vec3(1.0, 0.6, 0.25) * sheen * vehicle * luma * 1.5 * night;
    return vec4(shown, 1.0);
}
