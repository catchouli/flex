// Fractal Zoom: the picture holds a smaller copy of itself behind its people, which holds another,
// and so on, all live. Zooming by one whole copy lands back where it started, so
// the fall never ends. Fading in, the copies open up from the middle of the picture outwards.

vec3 turned(vec3 c, float a) {
    float s = sin(a), k = cos(a);
    return c * k + cross(vec3(0.57735), c) * s + vec3(0.57735) * dot(vec3(0.57735), c) * (1.0 - k);
}

// One copy's colour at `q`, which is `level` copies down.
vec3 copy_at(vec2 q, float level, vec2 low, vec2 high) {
    vec3 c = texture(u_video, q).rgb;
    // A glowing frame round each copy, from how near its edge this point is.
    vec2 room = min(q - low, high - q) / (high - low);
    c += vec3(1.0, 0.8, 0.45) * (1.0 - smoothstep(0.0, 0.03, min(room.x, room.y))) * (0.25 + u_beat * 0.6);
    return turned(c, level * 0.9 * s_hue / 100.0);
}

vec4 effect(vec2 uv) {
    vec2 low = u_video_rect.xy, high = u_video_rect.zw;
    if (uv != clamp(uv, low, high)) return vec4(0.0, 0.0, 0.0, 1.0);
    vec2 centre = 0.5 * (low + high) + vec2(0.08 * sin(u_time * 0.37), 0.06 * cos(u_time * 0.29));
    if (s_centre > 0.5 && u_face_count > 0) centre = mix(centre, face_point(0, 2), u_face_seen[0]);

    float size = s_window / 100.0;
    float fall = fract(u_time * 0.28 * s_speed / 100.0);
    vec2 levels[17];
    levels[0] = centre + (uv - centre) * pow(size, fall);
    int last = 0;
    for (int i = 0; i < 16; i++) {
        vec2 deeper = centre + (levels[last] - centre) / size;
        if (deeper != clamp(deeper, low, high)) break;
        last++;
        levels[last] = deeper;
    }

    // From the innermost copy outwards, each copy's people go over everything inside it. The
    // outermost, about to fly past the camera, melts away so the loop doesn't show.
    vec3 c = copy_at(levels[last], fall - float(last), low, high);
    for (int i = last - 1; i >= 0; i--) {
        float person = smoothstep(0.4, 0.7, person_at(levels[i])) * s_people / 100.0;
        person = max(person, smoothstep(0.76, 0.86, depth_at(levels[i])) * s_near / 100.0);
        if (i == 0) person *= 1.0 - smoothstep(0.55, 1.0, fall);
        if (person > 0.0) c = mix(c, copy_at(levels[i], fall - float(i), low, high), person);
    }
    // The window into the copies opens out from the middle, past the picture's corners at all of it.
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    float open = 4.0 * u_amount * u_amount;
    float shown = 1.0 - smoothstep(open - 0.4, open, length((uv - centre) * aspect));
    c = mix(texture(u_video, uv).rgb, c, shown);
    return vec4(c, 1.0);
}
