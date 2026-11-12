// Stage Lights: the house lights go down and each person has their own follow spot, a beam
// slanting down from the rig through the haze to a pool of light on them. From MoGe-2's who's who.
//
// Fading in, the house lights go down slowly as the spots and their beams come up.
//
// s_colours: 0 white, 1 a coloured gel each.

vec3 gel(int i) {
    return s_colours < 0.5 ? vec3(1.0, 0.96, 0.88) : mix(vec3(1.0), hue(float(i) * 0.37 + 0.6), 0.65);
}

vec4 effect(vec2 uv) {
    vec3 film = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(film, 1.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float dark = 1.0 - s_dark / 100.0 * u_amount;
    vec3 light = vec3(dark);
    vec3 haze = vec3(0.0);
    float beat = u_beat * s_react / 100.0;
    for (int i = 0; i < u_person_count; i++) {
        float seen = u_person_seen[i];
        if (seen <= 0.0) continue;
        vec4 b = u_person_box[i];
        vec2 middle = vec2(0.5 * (b.x + b.z), 0.5 * (b.y + b.w)) * aspect;
        vec2 half_size = 0.5 * (b.zw - b.xy) * aspect;
        // The spot: an oval round the person, a little bigger than them, soft at the edge.
        float radius = max(half_size.x, half_size.y) * 1.05 * s_size / 100.0 + 0.02;
        vec2 rel = (p - middle) / vec2(radius * 0.8, radius);
        float spot = 1.0 - smoothstep(0.85, 1.0, length(rel));
        // Each spot's lamp hangs up in the rig, over to one side or the other.
        vec2 lamp = vec2(mix(-0.2, aspect.x + 0.2, fract(float(i) * 0.618 + 0.3)), 1.15);
        vec2 to = middle - lamp;
        float along = clamp(dot(p - lamp, to) / dot(to, to), 0.0, 1.0);
        float off = length(p - lamp - to * along);
        float cone = (1.0 - smoothstep(0.0, radius * along * 0.9 + 0.005, off)) * along * step(0.0, dot(p - lamp, to));
        float flicker = 0.95 + 0.05 * sin(u_time * 13.0 + float(i) * 4.0);
        vec3 colour = gel(i) * flicker * (1.0 + 0.5 * beat);
        light += colour * spot * seen * 1.15 * u_amount;
        haze += colour * cone * seen * 0.12 * s_haze / 100.0 * u_amount * (0.7 + 0.3 * noise(p * 6.0 + vec2(u_time * 0.2, 0.0)));
    }
    return vec4(film * light + haze, 1.0);
}
