// Light Suits: people go dark, with neon running along their outlines and the seams between body
// parts; the parts light up one after another, head to feet, on the beat. From MoGe-2's body parts.
// Fading in, the world dims and the suits come on from the head down to the feet.

int part_at(vec2 uv) {
    vec4 upper = more_masks_at(uv, 3);
    vec4 lower = more_masks_at(uv, 4);
    float best = upper.r;
    int part = 0;
    if (upper.g > best) { best = upper.g; part = 1; }
    if (upper.b > best) { best = upper.b; part = 2; }
    if (upper.a > best) { best = upper.a; part = 3; }
    if (lower.r > best) { best = lower.r; part = 4; }
    if (lower.g > best) { part = 5; }
    return part;
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 1.0);
    float world_dim = 1.0 - s_dim / 100.0 * u_amount;
    vec3 world = c * world_dim;
    float mask = person_at(uv);
    float rim = smoothstep(0.2, 0.45, mask) * (1.0 - smoothstep(0.45, 0.7, mask));
    float person = smoothstep(0.45, 0.6, mask);
    if (person + rim <= 0.0 || u_has_more_masks < 0.5) return vec4(world, 1.0);
    int part = part_at(uv);
    vec2 e = 1.5 / u_resolution * max(1.0, u_resolution.y / 720.0);
    float seam = 0.0;
    for (int k = 0; k < 4; k++) {
        vec2 d = k == 0 ? vec2(e.x, 0.0) : k == 1 ? vec2(-e.x, 0.0) : k == 2 ? vec2(0.0, e.y) : vec2(0.0, -e.y);
        if (person_at(uv + d) > 0.5 && part_at(uv + d) != part) seam = 1.0;
    }
    int who = person_there(uv);
    vec3 neon = hue((who >= 0 ? float(who) * 0.37 : 0.0) + s_hue / 360.0);
    // A wave of light running down the body: head, torso, arms, hands, legs, feet.
    float order = part == 0 ? 0.0 : part == 1 ? 1.0 : part == 2 ? 1.5 : part == 3 ? 2.0 : part == 4 ? 3.0 : 4.0;
    float on = clamp(u_amount * 2.0 - order / 4.0, 0.0, 1.0);
    person *= on;
    rim *= on;
    float wave = fract(u_time * 0.6 * s_speed / 100.0 - order / 5.0);
    float lit = 0.1 + 0.5 * exp(-wave * 6.0) + 0.6 * u_beat * s_react / 100.0;
    vec3 suit = c * 0.12 + neon * lit * 0.35;
    suit = mix(suit, neon * 1.4 + 0.3, seam);
    vec3 shown = mix(world, suit, person);
    shown += neon * rim * 1.2;
    return vec4(shown, 1.0);
}
