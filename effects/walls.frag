// Breathing Walls: the room is the inside of something alive. The walls swell in and out with
// each breath, in a wave from the far end, buckling along folds that the light catches, with a
// wet sheen and veins that throb. Nobody in the room moves with them. Fading in, the walls start
// to breathe more and more deeply as they turn to flesh.

float fbm4(vec2 p) {
    float sum = 0.0, amount = 0.5;
    for (int i = 0; i < 4; i++) {
        sum += amount * noise(p);
        p = p * 2.03 + vec2(17.1, 9.2);
        amount *= 0.5;
    }
    return sum;
}

// In quickly, out slowly, from 0 (empty) to 1 (full).
float lungs(float phase) {
    phase = fract(phase);
    return phase < 0.4 ? smoothstep(0.0, 0.4, phase) : 1.0 - smoothstep(0.4, 1.0, phase);
}

// The ribs and folds of the wall at a point, and how far they're standing out just now.
float flesh(vec2 s, float t, float full) {
    vec2 w = s + 0.35 * vec2(fbm4(s * 1.3 + 11.0 + t * 0.03), fbm4(s * 1.3 + 37.0 - t * 0.02));
    float ribs = pow(1.0 - abs(sin(w.x * 5.5 + 1.2 * sin(w.y * 1.6))), 1.8);
    float folds = fbm4(w * 3.2 + vec2(0.0, t * 0.05));
    return mix(folds, ribs, 0.45 + 0.3 * full) * (0.55 + 0.75 * full);
}

float room_at(vec2 uv) {
    vec4 m = masks_at(uv);
    float wall = smoothstep(0.25, 0.6, m.g);
    float other = (1.0 - smoothstep(0.2, 0.5, m.r)) * (1.0 - smoothstep(0.3, 0.6, m.b));
    return max(wall, other * s_whole / 100.0);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 1.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 centre = 0.5 * (u_video_rect.xy + u_video_rect.zw);
    vec2 q = (uv - centre) * aspect;

    // Soft where the wall meets anything else, so it stretches rather than tears.
    float room = room_at(uv) * 0.4;
    for (int i = 0; i < 6; i++) {
        float a = float(i) * 1.0471976;
        room += room_at(uv + vec2(cos(a), sin(a)) / aspect * 0.018) * 0.1;
    }
    float person = smoothstep(0.3, 0.6, person_at(uv));
    room *= 1.0 - person;
    if (room <= 0.001) return vec4(c, 1.0);

    float near = depth_at(uv);
    float far = 1.0 / (0.25 + near);
    vec2 s = q * mix(1.0, far, 0.7) * 1.4;
    float t = u_video_time;
    float rate = s_rate / 60.0;
    float react = s_react / 100.0;
    // The breath starts at the back of the throat and comes towards you.
    float full = lungs(t * rate - 0.1 * far - 0.12 * length(q)) + 0.25 * u_bass * react;
    float height = flesh(s, t, full);
    float e = 0.012;
    float hx = flesh(s + vec2(e, 0.0), t, full) - flesh(s - vec2(e, 0.0), t, full);
    float hy = flesh(s + vec2(0.0, e), t, full) - flesh(s - vec2(0.0, e), t, full);
    vec2 slope = vec2(hx, hy) / (2.0 * e);

    float amount = s_breath / 100.0 * u_amount * room * (0.5 + near * 0.8);
    float dist = max(length(q), 1e-3);
    float gather = (full - 0.5) * (0.05 + 0.03 * height);
    float ripple = 0.006 * s_folds / 100.0;
    vec2 moved = q * (1.0 - gather * amount / max(dist, 0.35)) + slope * ripple * amount;
    vec2 from = clamp(centre + moved / aspect, u_video_rect.xy, u_video_rect.zw);

    vec3 w = texture(u_video, from).rgb;
    // People shouldn't get dragged along with the wall behind them.
    float over = smoothstep(0.25, 0.6, person_at(from)) * plate_known(from) * u_amount;
    if (over > 0.0) w = mix(w, plate_at(from), over);

    float folds = s_folds / 100.0 * u_amount;
    float shade = 1.0 + clamp(dot(slope, normalize(vec2(-0.45, 0.9))) * 0.22, -0.65, 0.6) * folds;
    w *= shade * mix(1.0, 0.62 + 0.55 * clamp(height, 0.0, 1.0), folds);

    float luma = dot(w, vec3(0.3, 0.59, 0.11));
    vec3 meat = mix(vec3(0.5, 0.08, 0.1), vec3(1.0, 0.55, 0.47), clamp(luma * 1.4, 0.0, 1.0)) * (0.3 + luma * 1.3);
    w = mix(w, meat, s_flesh / 100.0 * u_amount);

    vec2 vp = s * 2.1 + 0.4 * vec2(fbm4(s * 2.0 + 5.0), fbm4(s * 2.0 + 19.0));
    float veins = pow(1.0 - abs(2.0 * fbm4(vp) - 1.0), 26.0) + 0.6 * pow(1.0 - abs(2.0 * fbm4(vp * 2.3 + 8.0) - 1.0), 40.0);
    float throb = 0.55 + 0.45 * full + 0.6 * u_beat * react;
    w = mix(w, vec3(0.32, 0.02, 0.1) * (0.6 + 1.6 * throb), clamp(veins * throb, 0.0, 1.0) * s_veins / 100.0 * u_amount);

    vec3 normal = normalize(vec3(-slope * 0.35, 1.0));
    float gloss = pow(max(dot(normal, normalize(vec3(-0.3, 0.55, 0.78))), 0.0), 48.0);
    w += vec3(1.0, 0.92, 0.9) * gloss * s_wet / 100.0 * u_amount * (0.4 + 1.2 * luma);

    return vec4(mix(c, w, smoothstep(0.0, 0.25, room)), 1.0);
}
