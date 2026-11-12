// Surveillance: a security camera's picture (grey or night-vision green, noisy, with scan lines),
// tracking everyone: brackets round each person with their number, a sight on their head, and
// animals and vehicles outlined. A blinking REC and the time in the corner. From MoGe-2's who's who.
// Fading in, the camera's picture comes over the film, the brackets grow out from the corners and
// the writing comes up.
//
// s_look: 0 grey, 1 night vision.

// A digit drawn as seven segments, in a box 1 wide and 2 tall from p = (0, 0).
float digit(vec2 p, int n) {
    int bits = n == 0 ? 63 : n == 1 ? 6 : n == 2 ? 91 : n == 3 ? 79 : n == 4 ? 102 : n == 5 ? 109 : n == 6 ? 125 : n == 7 ? 7 : n == 8 ? 127 : 111;
    float w = 0.16;
    float d = 1e9;
    // a top, b upper right, c lower right, d bottom, e lower left, f upper left, g middle.
    vec4 segs[7];
    segs[0] = vec4(0.0, 2.0, 1.0, 2.0);
    segs[1] = vec4(1.0, 2.0, 1.0, 1.0);
    segs[2] = vec4(1.0, 1.0, 1.0, 0.0);
    segs[3] = vec4(0.0, 0.0, 1.0, 0.0);
    segs[4] = vec4(0.0, 0.0, 0.0, 1.0);
    segs[5] = vec4(0.0, 1.0, 0.0, 2.0);
    segs[6] = vec4(0.0, 1.0, 1.0, 1.0);
    for (int k = 0; k < 7; k++) {
        if (((bits >> k) & 1) == 0) continue;
        vec2 a = segs[k].xy, b = segs[k].zw;
        vec2 ab = b - a;
        float t = clamp(dot(p - a, ab) / dot(ab, ab), 0.0, 1.0);
        d = min(d, length(p - a - ab * t));
    }
    return 1.0 - step(w, d);
}

// A whole number of `places` digits, `size` tall, its bottom left at `at` (all in screen heights).
float number(vec2 p, vec2 at, float size, int value, int places) {
    vec2 q = (p - at) / (size * 0.5);
    float lit = 0.0;
    for (int k = 0; k < places; k++) {
        int place = places - 1 - k;
        int n = value;
        for (int j = 0; j < place; j++) n /= 10;
        lit = max(lit, digit(q - vec2(float(k) * 1.6, 0.0), n - (n / 10) * 10));
    }
    return lit;
}

float outline(vec2 uv, float mask, float px) {
    float m = smoothstep(0.4, 0.6, mask);
    float around = 0.0;
    for (int k = 0; k < 4; k++) {
        vec2 d = (k == 0 ? vec2(1.0, 0.0) : k == 1 ? vec2(-1.0, 0.0) : k == 2 ? vec2(0.0, 1.0) : vec2(0.0, -1.0)) * px * 2.0;
        around = max(around, abs(m - smoothstep(0.4, 0.6, vehicle_at(uv + d) + animal_at(uv + d))));
    }
    return around;
}

vec4 effect(vec2 uv) {
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float px = 1.0 / u_resolution.y * max(1.0, u_resolution.y / 720.0);
    vec3 film = texture(u_video, uv).rgb;
    float luma = dot(film, vec3(0.3, 0.59, 0.11));
    luma += (hash(uv * u_resolution + fract(u_time) * 91.0) - 0.5) * 0.08;
    luma *= 0.9 + 0.1 * sin(uv.y * u_resolution.y * 1.5);
    vec2 v = uv - 0.5;
    luma *= 1.0 - dot(v, v) * 0.9;
    vec3 tint = s_look < 0.5 ? vec3(0.92, 0.96, 1.0) : vec3(0.35, 1.0, 0.45);
    vec3 c = s_look < 0.5 ? vec3(luma) * tint : pow(vec3(max(luma, 0.0)), vec3(0.7)) * tint;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) c = vec3(0.0);
    if (u_amount < 1.0) c = mix(film, c, u_amount);
    vec3 ink = s_look < 0.5 ? vec3(1.0, 0.85, 0.2) : vec3(0.85, 1.0, 0.85);
    // Animals and vehicles: outlined.
    float things = outline(uv, vehicle_at(uv) + animal_at(uv), px);
    c = mix(c, vec3(1.0, 0.55, 0.1), things * step(0.5, fract((p.x + p.y) * 60.0)) * u_amount);
    for (int i = 0; i < u_person_count; i++) {
        float seen = u_person_seen[i];
        if (seen <= 0.0) continue;
        vec4 b = u_person_box[i];
        vec2 lo = b.xy * aspect - 4.0 * px, hi = b.zw * aspect + 4.0 * px;
        vec2 size = hi - lo;
        float arm = min(size.x, size.y) * 0.22 * u_amount;
        // Brackets at the corners.
        vec2 q = p - lo;
        vec2 r = hi - p;
        float w = 2.0 * px;
        float corner = 0.0;
        if (p.x > lo.x - w && p.x < hi.x + w && p.y > lo.y - w && p.y < hi.y + w) {
            float left = step(abs(q.x), w), right = step(abs(r.x), w), bottom = step(abs(q.y), w), top = step(abs(r.y), w);
            float near_x = step(q.x, arm) + step(r.x, arm), near_y = step(q.y, arm) + step(r.y, arm);
            corner = clamp((left + right) * near_y + (top + bottom) * near_x, 0.0, 1.0);
        }
        // The number above it, and a sight on the head.
        float label = number(p, vec2(lo.x, hi.y + 6.0 * px), 0.035, i + 1, 2);
        vec4 head = u_person_head[i];
        float sight = 0.0;
        if (head.w > 0.2) {
            vec2 h = p - head.xy * aspect;
            float hr = max(head.z, 0.01) * 0.8;
            float ring = 1.0 - smoothstep(w * 0.5, w, abs(length(h) - hr));
            float hairs = step(abs(h.x), w * 0.5) * step(hr * 0.5, abs(h.y)) * step(abs(h.y), hr * 1.4) + step(abs(h.y), w * 0.5) * step(hr * 0.5, abs(h.x)) * step(abs(h.x), hr * 1.4);
            sight = max(ring, hairs) * (0.6 + 0.4 * step(0.5, fract(u_time * 2.0 + float(i) * 0.3)));
        }
        c = mix(c, ink, max(max(corner, label), sight) * seen * u_amount);
    }
    // REC, blinking, and the time.
    vec2 corner_at = vec2(u_video_rect.x * aspect.x + 0.04, u_video_rect.w - 0.07);
    float dot_ = 1.0 - smoothstep(0.012, 0.014, length(p - corner_at - vec2(0.0, 0.02)));
    c = mix(c, vec3(1.0, 0.1, 0.1), dot_ * step(0.5, fract(u_time)) * u_amount);
    int t = int(u_video_time);
    float clock = number(p, corner_at + vec2(0.04, 0.0), 0.04, t / 3600, 2)
        + number(p, corner_at + vec2(0.04 + 0.075, 0.0), 0.04, (t / 60) % 60, 2)
        + number(p, corner_at + vec2(0.04 + 0.15, 0.0), 0.04, t % 60, 2);
    c = mix(c, vec3(1.0), clamp(clock, 0.0, 1.0) * u_amount);
    return vec4(c, 1.0);
}
