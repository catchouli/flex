// Thunderstorm: layers of falling streaks each at their own distance, hidden by anything nearer,
// and splashing on the top of things. Lightning is kept soft and brief, as flashes are hard on eyes.
// Fading in, the sky darkens and the rain starts, falling more and more heavily.

// One layer of rain. Every column of it falls at its own speed, with drops of its own spacing and
// length, so nothing lines up from one column to the next.
float streaks(vec2 p, float scale, float t, float wind, float rain) {
    float x = (p.x - p.y * wind) * scale;
    float column = floor(x);
    float speed = 2.2 + 1.8 * hash(vec2(column, scale));
    float spacing = 0.12 + 0.2 * hash(vec2(column + 7.0, scale));
    float y = (p.y + t * speed) / spacing + 17.0 * hash(vec2(column + 3.0, scale));
    vec2 id = vec2(column, floor(y));
    if (hash(id + scale) > rain || u_amount <= 0.0) return 0.0;
    float size = 0.3 + 0.6 * hash(id + 9.0);
    float along = (fract(y) - (1.0 - size) * hash(id + 5.0)) / size;
    if (along < 0.0 || along > 1.0) return 0.0;
    float across = abs(fract(x) - 0.5 - 0.3 * (hash(id + 3.0) - 0.5));
    return (1.0 - smoothstep(0.02, 0.09, across)) * smoothstep(0.0, 0.25, along) * (1.0 - smoothstep(0.55, 1.0, along)) * (0.5 + 0.5 * hash(id + 11.0));
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(0.0, 0.0, 0.0, 1.0);
    vec2 p = uv * vec2(u_resolution.x / u_resolution.y, 1.0);
    // Wrapped, so the sums below don't run out of precision over a long film.
    float t = mod(u_time, 900.0);
    float near = depth_at(uv);
    float gloom = s_dark / 100.0 * u_amount;
    float react = s_react / 100.0;
    vec3 graded = mix(c, c * vec3(0.5, 0.6, 0.8) * 0.8, gloom);

    // Thunder heard in the film brings the lightning more often, and brighter.
    float thunder = u_sound_thunder * react;
    float often = 0.62 - 0.2 * thunder;
    float strike = pow(max(noise(vec2(t * 2.3, 7.0)) - often, 0.0) / (1.0 - often), 2.0) * (1.0 + 0.8 * thunder) + u_beat * 0.25 * react;
    strike *= s_lightning / 100.0 * u_amount;
    graded += c * vec3(0.8, 0.85, 1.0) * strike * (0.4 + 1.6 * (1.0 - near));

    // Gusts: the wind comes and goes, and the rain gets heavier and lighter in passing sheets.
    // Wind heard in the film slants it further the way it's already blowing, and rain or water
    // heard makes it heavier.
    float wind = (s_wind / 100.0 + (s_wind < 0.0 ? -0.7 : 0.7) * u_sound_wind * react) * 0.32 * (0.7 + 0.8 * noise(vec2(t * 0.27, 3.0)));
    float pour = u_sound_water * react;
    float rain = (s_rain / 100.0 + (1.0 - s_rain / 100.0) * 0.9 * pour) * u_amount;
    float sheets = 0.55 + 0.45 * smoothstep(0.3, 0.7, noise(vec2(p.x * 2.2 + t * 0.7, t * 0.33)));
    float fall = 0.0;
    for (int i = 0; i < 5; i++) {
        float f = float(i);
        // Each layer stays put in the air as the camera moves.
        vec2 at_layer = p - (world_pin(vec2(0.5), world_distance(0.15 + f * 0.2)) - vec2(0.5)) * vec2(u_resolution.x / u_resolution.y, 1.0);
        fall += streaks(at_layer + f * 3.1, 110.0 - f * 19.0, t, wind, rain) * step(near, 0.15 + f * 0.2) * (0.22 + f * 0.09) * sheets;
    }
    graded += vec3(0.75, 0.82, 0.95) * fall * (1.0 + 2.0 * strike) * (1.0 + 0.6 * pour);

    // Splashes on the top of things.
    float ledge = smoothstep(0.02, 0.06, near - depth_at(uv + vec2(0.0, 8.0 / u_resolution.y)));
    if (ledge > 0.0) {
        vec2 q = p * 90.0;
        vec2 cell = floor(q);
        float time_slot = floor(t * 7.0 + hash(cell) * 7.0);
        if (hash(cell + time_slot) > 0.93) graded += vec3(0.8, 0.86, 1.0) * ledge * (1.0 - smoothstep(0.15, 0.4, length(fract(q) - 0.5))) * rain;
    }
    return vec4(graded, 1.0);
}
