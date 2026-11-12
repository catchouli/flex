// Graffiti: painted in shot_uv, where each point was on screen when the shot began, so it stays on
// what it's on. Only walls take paint (inside, or buildings' outsides), and people, animals and
// vehicles go in front of it. Fading in, the paint goes on in speckles, as if sprayed.

// Value noise on hash(vec3) rather than hash(vec2), and four octaves of it.
float noise_h3(vec2 p) {
    vec2 i = floor(p), f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    float a = hash(vec3(i, 0.0)), b = hash(vec3(i + vec2(1.0, 0.0), 0.0));
    float c = hash(vec3(i + vec2(0.0, 1.0), 0.0)), d = hash(vec3(i + vec2(1.0, 1.0), 0.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

float fbm_h3(vec2 p) {
    float sum = 0.0, amount = 0.5;
    for (int i = 0; i < 4; i++) {
        sum += amount * noise_h3(p);
        p = p * 2.03 + vec2(17.1, 9.2);
        amount *= 0.5;
    }
    return sum;
}

float segment(vec2 p, vec2 a, vec2 b) {
    vec2 pa = p - a, ba = b - a;
    return length(pa - ba * clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0));
}

// A made-up letter of two to four strokes inside a unit box: distance to its nearest stroke.
float letter(vec2 p, vec3 seed) {
    float d = 1e3;
    vec2 at = vec2(hash(seed), hash(seed + 1.0));
    int strokes = 2 + int(hash(seed + 2.0) * 3.0);
    for (int i = 0; i < 4; i++) {
        if (i >= strokes) break;
        vec2 to = vec2(hash(seed + float(i) * 3.1 + 4.0), hash(seed + float(i) * 3.1 + 5.0));
        d = min(d, segment(p, at, to));
        at = to;
    }
    return d;
}

// A stencil: a star, a heart or an arrow, as a distance (inside below 0) in a unit box.
float stencil(vec2 p, float kind) {
    p = p * 2.0 - 1.0;
    if (kind < 0.33) {
        float a = atan(p.y, p.x), r = length(p);
        float edge = 0.55 + 0.35 * cos(5.0 * a) * 0.5 + 0.2;
        return r - edge * 0.85;
    } else if (kind < 0.66) {
        p.y = -p.y * 1.1 + 0.25;
        float a = atan(abs(p.x), p.y) / 3.14159;
        float h = 0.9 * (13.0 * a - 22.0 * a * a + 10.0 * a * a * a) / (6.0 - 5.0 * a);
        return length(p) - h;
    }
    float shaft = max(abs(p.y) - 0.18, abs(p.x + 0.2) - 0.6);
    float head = max(abs(p.y) - (0.8 - p.x) * 0.8, -(p.x - 0.4));
    return min(shaft, max(head, p.x - 0.95));
}

vec4 effect(vec2 uv) {
    vec3 film = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw) || u_world_ready <= 0.0) return vec4(film, 1.0);
    float in_front = person_at(uv) + animal_at(uv) + vehicle_at(uv);
    float wall = smoothstep(0.35, 0.7, wall_at(uv) + building_at(uv)) * (1.0 - smoothstep(0.3, 0.6, in_front));
    if (wall <= 0.0) return vec4(film, 1.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    float shot = mod(u_shot_start * 100.0, 971.0);
    float cell_size = 0.42 * s_size / 100.0;
    vec2 p = shot_uv(uv) * aspect / cell_size;
    vec2 cell = floor(p);
    vec3 seed = vec3(cell, shot);
    if (hash(seed) > 0.25 + 0.6 * s_amount / 100.0) return vec4(film, 1.0);
    vec2 f = fract(p);
    float kind = hash(seed + 1.0);
    vec3 colour = hue(hash(seed + 2.0)) * 1.2 + 0.1;
    float paint = 0.0, outline = 0.0, glow_d = 1e3;
    if (kind < 0.55) {
        // A tag: three or four letters along a slight slope.
        int count = 3 + int(hash(seed + 3.0) * 2.0);
        vec2 start = vec2(0.08, 0.35 + 0.2 * hash(seed + 4.0));
        float w = 0.84 / float(count);
        float d = 1e3;
        for (int i = 0; i < 4; i++) {
            if (i >= count) break;
            vec2 box = start + vec2(float(i) * w, float(i) * 0.03 * (hash(seed + 5.0) - 0.5));
            vec2 local = (f - box) / vec2(w * 0.9, 0.32);
            if (local.x > -0.3 && local.x < 1.3 && local.y > -0.3 && local.y < 1.3) d = min(d, letter(local, seed + float(i) * 11.0) * w * 0.9);
        }
        paint = 1.0 - smoothstep(0.022, 0.028, d);
        outline = 1.0 - smoothstep(0.038, 0.045, d);
        glow_d = d;
        // Drips running down from the bottom of the letters.
        float column = floor(f.x * 40.0);
        float drip = hash(vec3(column, cell + 9.0)) > 0.8 ? 1.0 : 0.0;
        float length_ = 0.12 * hash(vec3(column, cell + 10.0));
        paint = max(paint, drip * step(abs(fract(f.x * 40.0) - 0.5), 0.18) * step(start.y - length_, f.y) * step(f.y, start.y + 0.05) * outline);
    } else if (kind < 0.85) {
        float d = stencil((f - 0.15) / 0.7, hash(seed + 6.0)) * 0.35;
        paint = 1.0 - smoothstep(-0.005, 0.005, d);
        outline = paint;
        glow_d = max(d, 0.0);
    } else {
        // A splash, with spatter around it.
        float r = length(f - 0.5) + 0.12 * (fbm_h3((f - 0.5) * 6.0 + cell) - 0.5);
        paint = (1.0 - smoothstep(0.22, 0.24, r)) + step(0.92, noise_h3(f * 38.0 + cell * 3.0)) * (1.0 - smoothstep(0.25, 0.42, r));
        outline = paint;
        glow_d = max(r - 0.23, 0.0);
    }
    // Sprayed: speckled where the paint thins out.
    float speckle = smoothstep(0.35, 0.65, noise_h3(f * 120.0 + cell * 7.0));
    paint = clamp(paint * mix(0.85, 1.0, speckle), 0.0, 1.0);
    wall *= clamp(u_amount * 1.6 - 0.6 * noise_h3(f * 60.0 + cell * 5.0), 0.0, 1.0);
    float luma = dot(film, vec3(0.3, 0.59, 0.11));
    vec3 c = film;
    c = mix(c, vec3(0.04) * (0.5 + luma), (outline - paint) * wall);
    c = mix(c, colour * (0.45 + 0.9 * luma), paint * wall);
    c += colour * exp(-glow_d * 40.0) * 0.35 * s_glow / 100.0 * wall;
    return vec4(mix(film, c, u_world_ready), 1.0);
}
