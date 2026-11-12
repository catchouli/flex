// Marionettes: everyone hangs from strings, tied to their hands and either side of their head,
// running up to a wooden control bar above them that rocks as they move. From MoGe-2's who's who.
// Fading in, the bar grows out from its middle and the strings let down from it.

// Distance from p to the segment a-b.
float segment(vec2 p, vec2 a, vec2 b) {
    vec2 ab = b - a;
    float t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-8), 0.0, 1.0);
    return length(p - a - ab * t);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float px = 1.0 / u_resolution.y;
    float width = px * max(1.0, u_resolution.y / 720.0) * s_thickness / 100.0;
    vec3 thread = vec3(0.92, 0.9, 0.82);
    for (int i = 0; i < u_person_count; i++) {
        float seen = u_person_seen[i];
        vec4 head = u_person_head[i];
        if (seen <= 0.0 || head.w < 0.2 || u_amount <= 0.0) continue;
        vec2 h = head.xy * aspect;
        float r = max(head.z, 0.01);
        // The control bar: above the head, as high as the person's tall, rocking with the beat.
        float tall = u_person_box[i].w - u_person_box[i].y;
        vec2 bar = vec2(h.x, min(h.y + r + max(tall, 0.15) * 0.45, u_video_rect.w + 0.02));
        float rock = sin(u_time * 1.7 + float(i) * 1.9) * 0.15 + u_beat * 0.1 * s_react / 100.0;
        vec2 along = vec2(cos(rock), sin(rock));
        float span = r * 3.2 * u_amount;
        vec2 ends[2];
        ends[0] = bar - along * span;
        ends[1] = bar + along * span;
        float strings = 0.0;
        // Either side of the head, to the middle of the bar.
        for (int side = 0; side < 2; side++) {
            vec2 at = h + vec2(side == 0 ? -r * 0.7 : r * 0.7, r * 0.4);
            vec2 top = bar + along * (side == 0 ? -0.25 : 0.25) * span;
            strings = max(strings, 1.0 - smoothstep(width * 0.5, width * 1.5, segment(p, mix(top, at, u_amount), top)));
        }
        // Hands, to the ends of the bar.
        for (int n = 0; n < 2; n++) {
            vec4 hand = u_person_hands[i * 2 + n];
            if (hand.w < 0.2) continue;
            strings = max(strings, smoothstep(0.2, 0.6, hand.w) * (1.0 - smoothstep(width * 0.5, width * 1.5, segment(p, mix(ends[n], hand.xy * aspect, u_amount), ends[n]))));
        }
        // Lit where light catches them, so they're seen against dark and light alike.
        float catchlight = 0.75 + 0.25 * sin(p.y * 140.0 + u_time * 3.0);
        c = mix(c, thread * catchlight, strings * seen * 0.9);
        // The bar, a wooden cross.
        float wood = min(segment(p, ends[0], ends[1]), segment(p, bar - vec2(-along.y, along.x) * span * 0.35, bar + vec2(-along.y, along.x) * span * 0.35));
        float cross_piece = 1.0 - smoothstep(r * 0.08 * u_amount, r * 0.08 * u_amount + 1.5 * px, wood);
        vec3 brown = vec3(0.45, 0.27, 0.12) * (0.8 + 0.3 * noise(p * 300.0 * vec2(1.0, 0.1)));
        c = mix(c, brown, cross_piece * seen);
    }
    return vec4(c, 1.0);
}
