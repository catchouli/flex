// Mannequins: people become artists' figures: smooth wood, toy plastic or chrome, shaded by
// which way each bit of them faces, with a seam at every joint where one body part meets another,
// and a blank egg for a head. From MoGe-2's body parts and normals. Fading in, people turn into
// figures in spreading patches.
//
// s_style: 0 wood, 1 plastic, 2 chrome.

// Which body part is where (0 head, 1 torso, 2 arm, 3 hand, 4 leg, 5 foot).
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

vec3 toy(int part) {
    if (part == 0 || part == 3) return vec3(1.0, 0.82, 0.62);
    if (part == 1) return vec3(0.85, 0.12, 0.1);
    if (part == 2) return vec3(0.95, 0.75, 0.1);
    if (part == 4) return vec3(0.1, 0.3, 0.85);
    return vec3(0.12, 0.12, 0.14);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 1.0);
    float person = smoothstep(0.4, 0.6, person_at(uv));
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    person *= clamp((u_amount * 1.25 - noise(uv * aspect * 14.0)) / 0.25, 0.0, 1.0);
    if (person <= 0.0 || u_has_more_masks < 0.5) return vec4(c, 1.0);
    int part = part_at(uv);
    // A seam where any nearby point is another part.
    vec2 e = 2.0 * s_seams / 100.0 / u_resolution * max(1.0, u_resolution.y / 720.0);
    float seam = 0.0;
    for (int k = 0; k < 4; k++) {
        vec2 d = k == 0 ? vec2(e.x, 0.0) : k == 1 ? vec2(-e.x, 0.0) : k == 2 ? vec2(0.0, e.y) : vec2(0.0, -e.y);
        if (person_at(uv + d) > 0.5 && part_at(uv + d) != part) seam = 1.0;
    }
    vec3 n = normal_at(uv);
    vec3 to_light = normalize(vec3(-0.45, 0.6, 0.65));
    float diffuse = max(dot(n, to_light), 0.0);
    float spec = pow(max(dot(reflect(-to_light, n), vec3(0.0, 0.0, 1.0)), 0.0), 24.0);
    float luma = dot(c, vec3(0.3, 0.59, 0.11));
    // The room's own light still falls on them, a little.
    float room = 0.6 + 0.8 * luma;
    // The head's a blank: no face shows through.
    if (part == 0) room = 1.0;
    vec3 shown;
    if (s_style < 0.5) {
        // The grain moves with the person.
        int who = person_there(uv);
        vec2 grain_at = (uv - (who >= 0 ? u_person_box[who].xy : vec2(0.0))) * aspect;
        float grain = fbm(grain_at * vec2(9.0, 70.0) + float(part) * 13.0);
        vec3 wood = mix(vec3(0.62, 0.42, 0.24), vec3(0.86, 0.66, 0.42), grain);
        shown = wood * (0.35 + 0.75 * diffuse) * room + spec * 0.15;
    } else if (s_style < 1.5) {
        shown = toy(part) * (0.3 + 0.8 * diffuse) * room + vec3(spec * 0.6);
    } else {
        // Chrome: the picture reflected, warped by the shape.
        vec3 r = reflect(vec3(0.0, 0.0, -1.0), n);
        vec3 seen = texture(u_video, clamp(0.5 + r.xy * vec2(0.45, -0.45), u_video_rect.xy, u_video_rect.zw)).rgb;
        shown = mix(vec3(dot(seen, vec3(0.33))), seen, 0.4) * 1.15 * vec3(0.92, 0.95, 1.0) + spec * 0.8;
    }
    shown *= 1.0 - seam * 0.75;
    return vec4(mix(c, shown, person), 1.0);
}
