// Kapow: comic-book sound effects. Each kind of sound Flex hears has a couple of words, and while
// it's heard they keep bursting out of spiky balloons somewhere new, popping in, holding and fading.
// Music sends notes floating up. The words are drawn from a tiny pixel font, rounded off and
// outlined in ink. Fading in, the balloons and notes grow from nothing and the printed look comes
// over the picture.

// A 5x7 pixel font: each letter's top four rows (20 bits, left to right) and bottom three (15).
const ivec2 FONT[26] = ivec2[](
    ivec2(476735, 17969), ivec2(1001022, 17982), ivec2(476688, 16942), ivec2(1001009, 17982), ivec2(1032734, 16927), ivec2(1032734, 16912),
    ivec2(476695, 17967), ivec2(575039, 17969), ivec2(462980, 4238), ivec2(576152, 21073), ivec2(541200, 16927), ivec2(585397, 17969),
    ivec2(575285, 20017), ivec2(476721, 17966), ivec2(1001022, 16912), ivec2(476721, 22093), ivec2(1001022, 21073), ivec2(508430, 1086),
    ivec2(1020036, 4228), ivec2(575025, 17966), ivec2(575025, 17732), ivec2(575029, 22186), ivec2(574788, 4228), ivec2(135300, 4100),
    ivec2(31, 0), ivec2(0, 396));
// Letters: ABCDEFGHIKLMNOPQRSTUVWY!-.
const int WORDS[300] = int[](5, 1, 13, 13, 11, 23, -1, -1, -1, -1, 7, 9, 0, 1, 13, 13, 11, 23, -1, -1, 4, 14, 13, 21, 23, -1, -1, -1, -1, -1, 5, 21, 7, 0, 11, 23, -1, -1, -1, -1, 5, 0, 0, 0, 7, 23, -1, -1, -1, -1, 4, 7, 4, 22, 23, -1, -1, -1, -1, -1, 6, 7, 0, -1, 7, 0, 23, -1, -1, -1, 7, 7, 4, 4, -1, 7, 4, 4, -1, -1, 4, 17, 13, 1, 23, -1, -1, -1, -1, -1, 4, 21, 0, 0, 7, -1, -1, -1, -1, -1, 5, 9, 16, 0, 9, 23, -1, -1, -1, -1, 6, 16, 19, 11, 1, 10, 4, -1, -1, -1, 7, 17, 14, 10, 0, 17, 7, 23, -1, -1, 9, 3, 16, 8, 14, -1, 3, 16, 8, 14, 6, 21, 7, 13, 13, 17, 7, -1, -1, -1, 5, 21, 7, 13, 13, 13, -1, -1, -1, -1, 7, 2, 16, 0, 2, 9, 10, 4, -1, -1, 6, 5, 21, 13, 13, 17, 7, -1, -1, -1, 6, 20, 16, 13, 13, 11, 23, -1, -1, -1, 7, 17, 2, 16, 4, 4, 2, 7, -1, -1, 5, 16, 8, 12, 6, 23, -1, -1, -1, -1, 6, 21, 4, 4, 24, 13, 13, -1, -1, -1, 5, 21, 13, 13, 5, 23, -1, -1, -1, -1, 5, 16, 13, 0, 16, 23, -1, -1, -1, -1, 9, 2, 10, 0, 14, -1, 2, 10, 0, 14, 4, 22, 0, 22, 23, -1, -1, -1, -1, -1, 4, 17, 7, 7, 7, -1, -1, -1, -1, -1, 3, 25, 25, 25, -1, -1, -1, -1, -1, -1, 8, 10, 0, -1, 10, 0, -1, 10, 0, -1, 6, 10, 0, -1, 10, 0, 23, -1, -1, -1);

// The kinds of sound with words, in WORDS' order (two words each), with their balloon and ink.
const int KINDS = 15;
const vec3 BALLOON[KINDS] = vec3[](
    vec3(1.0, 0.82, 0.1), vec3(1.0, 0.95, 0.75), vec3(1.0, 0.35, 0.3), vec3(1.0, 0.75, 0.85), vec3(0.6, 0.8, 1.0),
    vec3(0.85, 0.85, 1.0), vec3(0.55, 0.85, 1.0), vec3(0.85, 0.95, 0.95), vec3(1.0, 0.6, 0.15), vec3(0.9, 0.9, 0.9),
    vec3(1.0, 0.9, 0.2), vec3(0.75, 1.0, 0.6), vec3(1.0, 0.85, 0.55), vec3(0.85, 0.8, 0.95), vec3(1.0, 0.9, 0.6));
const vec3 INK[KINDS] = vec3[](
    vec3(0.9, 0.1, 0.05), vec3(0.85, 0.1, 0.1), vec3(1.0, 0.95, 0.2), vec3(0.85, 0.1, 0.45), vec3(0.1, 0.25, 0.7),
    vec3(0.35, 0.2, 0.75), vec3(0.05, 0.3, 0.75), vec3(0.2, 0.45, 0.55), vec3(0.85, 0.15, 0.0), vec3(0.15, 0.15, 0.2),
    vec3(0.85, 0.05, 0.05), vec3(0.4, 0.2, 0.05), vec3(0.85, 0.3, 0.05), vec3(0.3, 0.25, 0.4), vec3(0.75, 0.15, 0.55));

float heard(int k) {
    if (k == 0) return u_sound_explosions;
    if (k == 1) return u_sound_impacts;
    if (k == 2) return u_sound_shouting;
    if (k == 3) return u_sound_laughter;
    if (k == 4) return u_sound_crying;
    if (k == 5) return u_sound_thunder;
    if (k == 6) return u_sound_water;
    if (k == 7) return u_sound_wind;
    if (k == 8) return u_sound_fire;
    if (k == 9) return u_sound_vehicles;
    if (k == 10) return u_sound_alarms;
    if (k == 11) return u_sound_animals;
    if (k == 12) return u_sound_crowd;
    if (k == 13) return u_sound_silence;
    return u_sound_singing;
}

// Whether a pixel of the font is on: column `col` across the word (each letter is five wide with
// a gap after), row `row` from the top.
bool lit(int word, int col, int row) {
    int len = WORDS[word * 10];
    if (col < 0 || row < 0 || row > 6 || col >= len * 6) return false;
    int at = col / 6, c = col - at * 6;
    if (c == 5) return false;
    int g = WORDS[word * 10 + 1 + at];
    if (g < 0) return false;
    ivec2 bits = FONT[g];
    int bit = row < 4 ? (bits.x >> ((3 - row) * 5 + 4 - c)) : (bits.y >> ((6 - row) * 5 + 4 - c));
    return (bit & 1) == 1;
}

// How far (in font pixels) a point is from the word's lit pixels, nearby.
float word_distance(int word, vec2 p) {
    ivec2 cell = ivec2(floor(p));
    float d = 9.0;
    for (int dy = -1; dy <= 1; dy++) {
        for (int dx = -1; dx <= 1; dx++) {
            ivec2 n = cell + ivec2(dx, dy);
            if (lit(word, n.x, n.y)) {
                vec2 q = abs(p - (vec2(n) + 0.5)) - 0.5;
                d = min(d, length(max(q, 0.0)) + min(max(q.x, q.y), 0.0));
            }
        }
    }
    return d;
}

mat2 turn(float a) {
    float c = cos(a), s = sin(a);
    return mat2(c, s, -s, c);
}

// A balloon with a word: its colour and how much of it covers this point.
vec4 balloon(int k, float amount, vec2 s, float t, float aspect) {
    float period = 1.9 - 0.5 * s_often / 100.0;
    float offset = hash(vec2(float(k), 3.1)) * period;
    float slot = floor((t + offset) / period);
    float phase = fract((t + offset) / period);
    float seed = hash(vec2(slot, float(k)));
    // Pops in past full size, settles, then fades.
    float pop = phase < 0.12 ? sin(phase / 0.12 * 1.9) * 1.12 : 1.0;
    float fade = 1.0 - smoothstep(0.72, 0.95, phase);
    float size = (0.13 + 0.08 * amount) * s_size / 100.0 * pop * u_amount;
    if (size <= 0.001) return vec4(0.0);
    vec2 centre = vec2((0.18 + 0.64 * seed) * aspect, 0.22 + 0.56 * hash(vec2(seed, slot + 2.0)));
    vec2 q = turn((hash(vec2(slot, seed)) - 0.5) * 0.6) * (s - centre) / size;
    float r = length(q);
    if (r > 1.6) return vec4(0.0);

    // The spiky balloon: an uneven star, inked round the edge.
    float spikes = 9.0 + floor(seed * 5.0);
    float a = atan(q.y, q.x) / 6.2831853 * spikes;
    float tip = abs(fract(a) - 0.5) * 2.0;
    float edge = 1.05 + 0.32 * tip * (0.7 + 0.6 * hash(vec2(floor(a), slot)));
    vec3 col = mix(BALLOON[k] * 1.08, BALLOON[k] * 0.85, r / edge);
    // Ben-Day dots, bigger towards the edge.
    vec2 dots = fract(s * 160.0) - 0.5;
    col *= 1.0 - 0.18 * step(length(dots), 0.18 + 0.25 * r / edge);
    float cover = 1.0 - smoothstep(edge - 0.01, edge + 0.01, r);
    float ink = smoothstep(edge - 0.09, edge - 0.07, r);
    col = mix(col, vec3(0.05), ink);

    // The word, squeezed to fit and each letter bobbing.
    int word = k * 2 + int(hash(vec2(slot, seed + 9.0)) * 2.0);
    int len = WORDS[word * 10];
    float unit = min(1.85 / (float(len) * 6.0 - 1.0), 0.15);
    vec2 w = vec2(q.x / unit + (float(len) * 6.0 - 1.0) * 0.5, -q.y / unit + 3.5);
    w.y += sin(t * 9.0 + floor(w.x / 6.0) * 1.3) * 0.35;
    w.x += w.y * -0.15;
    float d = word_distance(word, w);
    col = mix(col, vec3(0.05), 1.0 - smoothstep(0.75, 0.95, d));
    col = mix(col, INK[k] + vec3(0.25) * (1.0 - smoothstep(0.0, 3.0, w.y)), 1.0 - smoothstep(0.2, 0.35, d));
    return vec4(col, cover * fade * smoothstep(0.12, 0.45, amount));
}

// A quaver: its colour and how much it covers `q` (one note high).
vec4 note(vec2 q, vec3 tint) {
    vec2 h = turn(0.45) * (q - vec2(0.0, 0.15));
    float head = length(h * vec2(1.0, 1.45)) - 0.22;
    float stem = max(abs(q.x - 0.19) - 0.045, abs(q.y - 0.62) - 0.47);
    vec2 f = q - vec2(0.19, 1.05);
    float flag = max(abs(f.y + f.x * 0.9 + 0.08 * sin(f.x * 9.0)) - 0.07, max(-f.x, f.x - 0.32));
    float d = min(head, min(stem, flag));
    float cover = 1.0 - smoothstep(0.0, 0.02, d - 0.05);
    vec3 col = mix(tint, vec3(0.05), smoothstep(-0.05, -0.03, d));
    return vec4(col, cover);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    float aspect = u_resolution.x / u_resolution.y;
    vec2 s = vec2(uv.x * aspect, uv.y);
    float t = u_time;

    // A little of a printed comic in the picture itself: punchier colour, inked edges.
    float look = s_look / 100.0 * u_amount;
    if (look > 0.0) {
        vec2 px = 1.0 / u_resolution;
        float l = dot(c, vec3(0.299, 0.587, 0.114));
        float gx = dot(texture(u_video, uv + vec2(px.x, 0.0)).rgb - texture(u_video, uv - vec2(px.x, 0.0)).rgb, vec3(0.333));
        float gy = dot(texture(u_video, uv + vec2(0.0, px.y)).rgb - texture(u_video, uv - vec2(0.0, px.y)).rgb, vec3(0.333));
        vec3 punchy = clamp(mix(vec3(l), c, 1.5) * 1.1 - 0.05, 0.0, 1.0);
        punchy = floor(punchy * 5.0 + 0.5) / 5.0 * 0.5 + punchy * 0.5;
        punchy *= 1.0 - smoothstep(0.08, 0.25, length(vec2(gx, gy)));
        c = mix(c, punchy, look);
    }

    // Notes floating up while there's music, in columns that sway.
    float music = max(u_sound_music, u_sound_singing) * s_notes / 100.0 * u_amount;
    if (music > 0.05) {
        float cell = 0.2;
        vec2 p = s + vec2(0.0, -t * 0.12);
        vec2 id = floor(p / cell);
        vec2 local = (fract(p / cell) - vec2(0.5, 0.35)) * cell / 0.1;
        local.x += sin(t * 2.0 + id.y) * 0.4;
        float h = hash(id);
        if (h < 0.22 * music + 0.03) {
            vec4 n = note(local, mix(vec3(1.0, 0.85, 0.2), vec3(0.4, 0.8, 1.0), hash(id + 3.0)));
            c = mix(c, n.rgb, n.a * smoothstep(0.05, 0.4, music) * (0.6 + 0.4 * u_beat + 0.4 * u_level));
        }
    }

    // The balloons, the loudest kinds drawn last so they land on top.
    for (int k = KINDS - 1; k >= 0; k--) {
        float amount = heard(k);
        if (amount < 0.12) continue;
        vec4 b = balloon(k, amount, s, t, aspect);
        c = mix(c, b.rgb, b.a);
    }
    return vec4(c, 1.0);
}
