// Halos: something on every head, from MoGe-2's who's who: a glowing halo hanging above it, a
// pair of devil's horns, or a gold crown. They sit on the head's top (the head's middle and size
// as the network sees them, hair and all), and the back of a halo goes behind the head. Fading in,
// each grows from nothing where it sits: the halo from its middle, horns out of the head, the
// crown out from the top of the head.
//
// s_kind: 0 halo, 1 horns, 2 crown.

// Distance from p to the segment a-b, and how far along it p is (0 to 1).
vec2 segment(vec2 p, vec2 a, vec2 b) {
    vec2 ab = b - a;
    float t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-8), 0.0, 1.0);
    return vec2(length(p - a - ab * t), t);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float px = 1.0 / u_resolution.y;
    float size = s_size / 100.0;
    float beat = u_beat * s_react / 100.0;
    for (int i = 0; i < u_person_count; i++) {
        vec4 head = u_person_head[i];
        float seen = u_person_seen[i] * smoothstep(0.2, 0.6, head.w) * step(1e-3, u_amount);
        if (seen <= 0.0) continue;
        vec2 h = head.xy * aspect;
        float r = max(head.z, 0.01) * size;
        // Each thing's own size, grown in with u_amount; where it sits goes by r.
        float g = r * max(u_amount, 1e-3);
        // A little sway, each head its own.
        float sway = sin(u_time * 1.3 + float(i) * 2.1) * 0.08;
        vec2 up = vec2(sin(sway), cos(sway));
        vec2 across = vec2(up.y, -up.x);
        vec2 top = h + up * r * 0.95;
        vec2 rel = p - top;
        float x = dot(rel, across), y = dot(rel, up);
        if (s_kind < 0.5) {
            // A ring seen a little from below: an ellipse floating above the head.
            float hover = r * (0.35 + 0.05 * sin(u_time * 2.0 + float(i)));
            vec2 e = vec2(x / (g * 0.85), (y - hover) / (g * 0.24));
            float d = abs(length(e) - 1.0) * g * 0.24;
            float ring = 1.0 - smoothstep(0.0, max(g * 0.05, 1.5 * px), d);
            float glow = exp(-d / max(g * 0.12, 2.0 * px)) * (0.5 + 0.8 * beat);
            // The back half goes behind the head.
            float behind = step(hover, y) * smoothstep(0.4, 0.7, head_at(uv) + hair_at(uv));
            vec3 gold = vec3(1.0, 0.86, 0.45);
            float amount = seen * (1.0 - behind);
            c = mix(c, gold * 1.2, ring * amount);
            c += gold * glow * 0.6 * amount;
        } else if (s_kind < 1.5) {
            // Two horns curling outwards from either side of the crown of the head.
            for (int side = 0; side < 2; side++) {
                float s = side == 0 ? -1.0 : 1.0;
                vec2 base = top - up * r * 0.22 + across * s * r * 0.42;
                vec2 bend = base + up * g * 0.45 + across * s * g * 0.18;
                vec2 tip = bend + up * g * 0.25 - across * s * g * 0.12;
                vec2 a = segment(p, base, bend), b = segment(p, bend, tip);
                float along = a.x < b.x ? a.y * 0.6 : 0.6 + b.y * 0.4;
                float d = min(a.x, b.x);
                float width = g * 0.17 * (1.0 - along * 0.9);
                float horn = 1.0 - smoothstep(width - 1.5 * px, width + 1.5 * px, d);
                // Rounded: lit along one side, ridged along its length.
                float shade = 0.55 + 0.45 * (1.0 - d / max(width, 1e-4)) + 0.1 * sin(along * 40.0);
                vec3 red = mix(vec3(0.55, 0.03, 0.02), vec3(1.0, 0.25, 0.1), along) * shade * (1.0 + 0.6 * beat);
                c = mix(c, red, horn * seen);
            }
        } else {
            // A crown: a gold band with five points, gems along it.
            float half_width = g * 0.75;
            float band = step(abs(x), half_width) * step(-g * 0.15, y) * step(y, g * 0.12);
            float points = abs(fract((x / half_width + 1.0) * 2.0 + 0.5) - 0.5) * 2.0;
            float spikes = step(abs(x), half_width) * step(g * 0.12, y) * step(y, g * 0.12 + points * g * 0.5);
            float crown = clamp(band + spikes, 0.0, 1.0);
            float shine = 0.75 + 0.35 * sin(x / g * 9.0 + u_time * 1.5) + 0.3 * beat;
            vec3 gold = vec3(0.95, 0.72, 0.2) * shine;
            vec2 gem = vec2(fract(x / half_width * 1.5 + 0.5) - 0.5, y / g + 0.02);
            float stone = band * (1.0 - smoothstep(0.08, 0.12, length(gem * vec2(1.0, 4.0))));
            vec3 colour = mix(gold, hue(floor(x / half_width * 1.5 + 0.5) * 0.31 + 0.95) * 0.9, stone);
            c = mix(c, colour, crown * seen);
        }
    }
    return vec4(c, 1.0);
}
