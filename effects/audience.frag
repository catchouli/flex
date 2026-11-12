// Studio Audience: the sitcom treatment. Lit signs along the top tell the audience what to do, and
// they light up when Flex hears it: LAUGH with laughter, APPLAUSE with clapping and cheering,
// ON AIR while people talk, QUIET when it goes silent. Laughter also brings up the warm, bright
// look of a studio set. Fading in, the signs grow down from the top and the studio light comes up.

// A 5x7 pixel font: each letter's top four rows (20 bits, left to right) and bottom three (15).
const ivec2 FONT[26] = ivec2[](
    ivec2(476735, 17969), ivec2(1001022, 17982), ivec2(476688, 16942), ivec2(1001009, 17982), ivec2(1032734, 16927), ivec2(1032734, 16912),
    ivec2(476695, 17967), ivec2(575039, 17969), ivec2(462980, 4238), ivec2(576152, 21073), ivec2(541200, 16927), ivec2(585397, 17969),
    ivec2(575285, 20017), ivec2(476721, 17966), ivec2(1001022, 16912), ivec2(476721, 22093), ivec2(1001022, 21073), ivec2(508430, 1086),
    ivec2(1020036, 4228), ivec2(575025, 17966), ivec2(575025, 17732), ivec2(575029, 22186), ivec2(574788, 4228), ivec2(135300, 4100),
    ivec2(31, 0), ivec2(0, 396));
// Letters: ABCDEFGHIKLMNOPQRSTUVWY!-.
const int WORDS[40] = int[](5, 10, 0, 19, 6, 7, -1, -1, -1, -1, 8, 0, 14, 14, 10, 0, 19, 17, 4, -1, 6, 13, 12, -1, 0, 8, 16, -1, -1, -1, 5, 15, 19, 8, 4, 18, -1, -1, -1, -1);

const int SIGNS = 4;

float lit_for(int i) {
    if (i == 0) return u_sound_laughter;
    if (i == 1) return u_sound_crowd;
    if (i == 2) return u_sound_speech;
    return u_sound_silence;
}

// Whether a pixel of the font is on: column `col` across the word, row `row` from the top.
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

float box(vec2 p, vec2 half_size, float round_by) {
    vec2 q = abs(p) - half_size + round_by;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - round_by;
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    float aspect = u_resolution.x / u_resolution.y;
    vec2 s = vec2(uv.x * aspect, uv.y);
    float t = u_time;
    float size = s_size / 100.0;

    // Laughter: the bright, warm, slightly soft light of a studio set.
    float laugh = u_sound_laughter * s_studio / 100.0 * u_amount;
    vec3 studio = pow(c, vec3(0.85)) * vec3(1.08, 1.03, 0.95) + vec3(0.03, 0.025, 0.02);
    c = mix(c, studio, clamp(laugh, 0.0, 1.0));

    // The signs, spread along the top, each just wide enough for its word.
    // No bigger than lets the widest (APPLAUSE) fit in its quarter of the width.
    float height = min(0.075 * size, 0.2 * aspect * (u_video_rect.z - u_video_rect.x) / 4.4) * u_amount;
    float unit = height * 0.55 / 7.0;
    float top = u_video_rect.w - 0.025 - height * 0.5;
    vec3 glow = vec3(0.0);
    for (int i = 0; i < SIGNS; i++) {
        int len = WORDS[i * 10];
        vec2 centre = vec2((u_video_rect.x + (float(i) + 0.5) / float(SIGNS) * (u_video_rect.z - u_video_rect.x)) * aspect, top);
        vec2 half_size = vec2(((float(len) * 6.0 - 1.0) * unit + height * 0.7) * 0.5, height * 0.5);
        vec2 q = s - centre;
        float d = box(q, half_size, height * 0.18);
        // Coming on with a buzz, as old signs do.
        float on = smoothstep(0.15, 0.5, lit_for(i));
        on *= 0.8 + 0.2 * step(0.25, hash(vec2(floor(t * 25.0), float(i))));
        vec3 colour = i == 3 ? vec3(1.0, 0.6, 0.1) : vec3(1.0, 0.1, 0.08);
        // Its light on the picture around it.
        glow += colour * exp(-max(d, 0.0) / max(height * 0.6, 1e-5)) * on * 0.35;
        if (d < height * 0.12) {
            float frame = smoothstep(0.0, -0.003, d);
            vec3 sign = mix(vec3(0.12, 0.1, 0.1), mix(vec3(0.18, 0.02, 0.02), colour * 0.75, on), smoothstep(-height * 0.07, -height * 0.09, d));
            vec2 w = vec2(q.x / unit + (float(len) * 6.0 - 1.0) * 0.5, -q.y / unit + 3.5);
            float letters = 1.0 - smoothstep(0.1, 0.3, word_distance(i, w));
            sign = mix(sign, mix(vec3(0.35, 0.3, 0.3), vec3(1.0, 0.97, 0.9), on), letters);
            // A sheen on the glass.
            sign += vec3(0.08) * smoothstep(0.0, height * 0.5, q.y + height * 0.1) * frame;
            c = mix(c, sign, frame * s_opacity / 100.0);
        }
    }
    c += glow * s_opacity / 100.0 * u_amount;
    return vec4(clamp(c, 0.0, 1.0), 1.0);
}
