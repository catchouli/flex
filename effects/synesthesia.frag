// Synesthesia: each kind of sound Flex hears (u_sound_*) has a look of its own, and they mix as
// the sounds do. Booms warp and shake the picture, engines blur it towards the middle, and the
// rest colour it and scatter things over it. Speech calms everything a little, so dialogue
// scenes stay watchable. Fading in, every look grows with the strength, from nothing.

vec3 hsv_hue(float h) {
    return clamp(abs(fract(h + vec3(0.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0) - 1.0, 0.0, 1.0);
}

float luma(vec3 c) {
    return dot(c, vec3(0.299, 0.587, 0.114));
}

// Specks drifting by `drift` (screen heights a second), at most one in each of `cells` cells
// across a screen height, `size` screen heights across. `p` is in screen heights.
float specks(vec2 p, float cells, vec2 drift, float size, float t, float seed) {
    vec2 q = (p - drift * t) * cells;
    vec2 cell = floor(q);
    float h = hash(cell + seed);
    if (h > 0.45) return 0.0;
    vec2 at = vec2(hash(cell + seed + 1.3), hash(cell + seed + 7.1)) * 0.8 + 0.1;
    // Each wobbles a little as it goes, and twinkles.
    at += 0.08 * vec2(sin(t * 2.0 + h * 30.0), cos(t * 1.7 + h * 20.0));
    float twinkle = 0.6 + 0.4 * sin(t * 7.0 + h * 50.0);
    float d = length(fract(q) - at) / (size * cells);
    // A bright core in a soft glow.
    return ((1.0 - smoothstep(0.6, 1.0, d)) + 0.5 * exp(-d * 1.5)) * twinkle;
}

// Rain: thin streaks falling fast and a little slanted.
float rain(vec2 p, float t) {
    vec2 q = vec2(p.x * 90.0 + p.y * 9.0, p.y * 6.0 + t * 14.0);
    vec2 cell = floor(q);
    if (hash(cell) > 0.25) return 0.0;
    vec2 f = fract(q);
    float x = hash(cell + 3.7);
    return (1.0 - smoothstep(0.0, 0.12, abs(f.x - x))) * smoothstep(0.0, 0.7, f.y) * (1.0 - smoothstep(0.85, 1.0, f.y));
}

// A camera flash in a crowd: a bright point with a cross of light, now and then in each cell.
float flashes(vec2 p, float t) {
    vec2 q = p * vec2(4.0, 3.0);
    vec2 cell = floor(q);
    float moment = floor(t * 9.0 + hash(cell) * 9.0);
    if (hash(cell + moment * 0.37) < 0.93) return 0.0;
    vec2 d = fract(q) - vec2(hash(cell + moment), hash(cell + moment + 2.1)) * 0.8 - 0.1;
    float fade = 1.0 - fract(t * 9.0 + hash(cell) * 9.0);
    return (exp(-length(d) * 30.0) * 2.0 + exp(-abs(d.x) * 120.0 - abs(d.y) * 8.0) + exp(-abs(d.y) * 120.0 - abs(d.x) * 8.0)) * fade;
}

vec4 effect(vec2 uv) {
    float aspect = u_resolution.x / u_resolution.y;
    float t = u_time;
    vec2 px = 1.0 / u_resolution;
    vec2 centre = mix(u_video_rect.xy, u_video_rect.zw, 0.5);
    float k = s_strength / 100.0 * u_amount * (1.0 - 0.4 * u_sound_speech);
    float motion = s_motion / 100.0;
    float many = s_specks / 100.0;

    float boom = clamp(max(u_sound_explosions, max(u_sound_impacts * 0.7, u_sound_thunder * 0.6)) * k, 0.0, 2.0);
    float music = u_sound_music * k, sing = u_sound_singing * k, laugh = u_sound_laughter * k;
    float water = u_sound_water * k, wind = u_sound_wind * k, fire = u_sound_fire * k;
    float thunder = u_sound_thunder * k, sad = u_sound_crying * k, alarm = max(u_sound_alarms, u_sound_shouting) * k;
    float crowd = u_sound_crowd * k, engine = u_sound_vehicles * k, quiet = u_sound_silence * k;
    float loud = 0.4 + u_level;

    // Where to look in the picture.
    vec2 d = (uv - centre) * vec2(aspect, 1.0);
    float r = length(d);
    vec2 out_dir = d / max(r, 1e-4) / vec2(aspect, 1.0);
    vec2 p = uv;
    // Booms: rings racing out from the middle, and a shake.
    float front = fract(t * 1.3) * 1.4;
    float ring = sin((r - front) * 45.0) * exp(-abs(r - front) * 9.0);
    p -= out_dir * ring * 0.045 * boom * loud * motion;
    vec2 jolt = vec2(noise(vec2(t * 31.0, 1.7)), noise(vec2(4.3, t * 29.0))) - 0.5;
    p += jolt * 0.035 * (boom * u_level + alarm * 0.3) * motion;
    // Laughter: the picture bounces.
    float bounce = abs(sin(t * 7.0));
    p = centre + (p - centre) * (1.0 - 0.025 * laugh * bounce * motion);
    p.y -= 0.01 * laugh * bounce * motion;
    // Wind: gusts push it sideways, a band at a time.
    p.x += (noise(vec2(uv.y * 4.0, t * 0.9)) - 0.5) * 0.04 * wind * motion;
    // Fire: heat haze rising, strongest low down.
    p.x += (noise(vec2(uv.x * 25.0, uv.y * 35.0 - t * 5.0)) - 0.5) * 0.012 * fire * (1.2 - uv.y) * motion;
    // Water: everything ripples a little.
    p += vec2(sin(uv.y * 60.0 + t * 3.0), cos(uv.x * 45.0 + t * 2.3)) * 0.0025 * water * motion;
    p = clamp(p, u_video_rect.xy + px * u_amount, u_video_rect.zw - px * u_amount);

    // Engines: rushing towards the middle (a zoom blur), and booms split the colours.
    vec3 c = vec3(0.0);
    float rush = 0.012 * engine * motion;
    float split = (0.006 * boom * loud + 0.002 * alarm) * motion;
    for (int i = 0; i < 8; i++) {
        vec2 q = centre + (p - centre) * (1.0 - float(i) * rush);
        vec2 apart = (q - centre) * split;
        c += vec3(texture(u_video, q + apart).r, texture(u_video, q).g, texture(u_video, q - apart).b);
    }
    c /= 8.0;
    float y = luma(c);

    // Music: neon edges in colours that move with the music, and a flash on each beat.
    float e = 0.0;
    if (music > 0.01) {
        float l = luma(texture(u_video, p - vec2(px.x, 0.0)).rgb), rr = luma(texture(u_video, p + vec2(px.x, 0.0)).rgb);
        float b = luma(texture(u_video, p - vec2(0.0, px.y)).rgb), tt = luma(texture(u_video, p + vec2(0.0, px.y)).rgb);
        e = smoothstep(0.04, 0.25, length(vec2(rr - l, tt - b)));
    }
    vec3 neon = hsv_hue(t * 0.08 + y * 0.6 + r * 0.4 + u_bass * 0.3);
    c = mix(c, c * (1.0 - 0.35 * music) + neon * e * (1.2 + u_beat), clamp(music, 0.0, 1.0));
    c += neon * u_beat * 0.12 * music;

    // Singing: warm gold, with sparks floating up.
    vec2 s = vec2(uv.x * aspect, uv.y);
    c = mix(c, c * vec3(1.18, 1.02, 0.78) + vec3(0.05, 0.035, 0.0), clamp(sing * 0.7, 0.0, 1.0));
    c += vec3(1.0, 0.8, 0.35) * specks(s, 12.0, vec2(0.0, 0.08), 0.008, t, 1.0) * sing * many;

    // Laughter: confetti, tumbling down in every colour.
    float bit = specks(s, 14.0, vec2(0.02, -0.12), 0.011, t, 5.0);
    c = mix(c, hsv_hue(hash(floor((s - vec2(0.02, -0.12) * t) * 14.0))), clamp(bit * laugh * many * 1.5, 0.0, 1.0));

    // Water: a blue-green cast and rain.
    c = mix(c, c * vec3(0.8, 0.95, 1.1), clamp(water * 0.6, 0.0, 1.0));
    c += vec3(0.75, 0.85, 0.95) * rain(s, t) * 0.7 * water * many;

    // Wind: pale streaks blowing across.
    c += vec3(0.9) * smoothstep(0.7, 0.95, noise(vec2(s.x * 3.0 - t * 4.0, s.y * 40.0))) * 0.15 * wind;

    // Fire: orange light from below, and embers rising.
    c += vec3(1.0, 0.45, 0.1) * pow(1.0 - uv.y, 2.0) * 0.35 * fire;
    c += vec3(1.0, 0.5, 0.15) * specks(s, 18.0, vec2(0.01, 0.15), 0.006, t, 9.0) * fire * many * 1.5 * (1.3 - uv.y);

    // Thunder: lightning flickers.
    float flicker = step(0.72, noise(vec2(t * 18.0, 3.0))) * step(0.5, noise(vec2(t * 3.0, 8.0)));
    c = mix(c, vec3(0.85, 0.9, 1.0) * (0.6 + y), clamp(flicker * thunder * 0.8, 0.0, 1.0));

    // Booms: the ring glows hot, and there's a flash that fades as it goes out.
    c += vec3(1.0, 0.7, 0.35) * exp(-abs(r - front) * 25.0) * boom * loud * 0.5;
    c += vec3(1.0, 0.75, 0.4) * boom * loud * 0.35 * exp(-front * 3.0);

    // Crowds: cameras going off.
    c += vec3(1.0, 0.97, 0.9) * flashes(s, t) * crowd * many;

    // Crying: the colour drains to blue.
    c = mix(c, vec3(y) * vec3(0.75, 0.85, 1.15), clamp(sad * 0.8, 0.0, 1.0));

    // Alarms and shouting: a red pulse round the edges.
    float edge = smoothstep(0.35, 0.9, r);
    c = mix(c, vec3(1.0, 0.08, 0.05), edge * (0.5 + 0.5 * sin(t * 9.0)) * clamp(alarm * 0.7, 0.0, 1.0));

    // Silence: grey, still and dim at the edges, with a little grain.
    c = mix(c, vec3(y), clamp(quiet * 0.5, 0.0, 1.0));
    c *= 1.0 - edge * 0.45 * quiet;
    c += (hash(uv * u_resolution + fract(t) * 91.0) - 0.5) * 0.05 * quiet;

    c = clamp(c, 0.0, 1.0);
    // The black bars, which the picture's edges are pulled out over, come back as they were.
    if (u_amount < 1.0 && uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) c = mix(texture(u_video, uv).rgb, c, u_amount);
    return vec4(c, 1.0);
}
