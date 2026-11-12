// X-Ray: a skull built in the face's own coordinates, so its jaw and sockets sit where the real
// ones are. The teeth part as far as the real mouth opens. Fading in, the x-ray and its line
// come up over the film.

float box(vec2 p, vec2 size) {
    vec2 d = abs(p) - size;
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0);
}

vec3 skull(vec2 a, float open, vec3 film) {
    vec2 p = vec2(abs(a.x - 0.5), a.y);
    float bone = 0.55 + 0.45 * smoothstep(0.55, 0.1, length((a - vec2(0.5, 0.62)) * vec2(1.0, 0.8)));
    vec3 c = vec3(0.72, 0.82, 0.95) * bone * (0.8 + 0.35 * dot(film, vec3(0.33)));
    // Cheekbones in shadow under the eyes, and the temples.
    c *= 1.0 - 0.5 * smoothstep(0.1, 0.0, length((p - vec2(0.34, 0.36)) * vec2(1.0, 1.6)));
    // Sockets.
    float socket = length((p - vec2(0.155, 0.61)) / vec2(0.115, 0.095));
    c = mix(c, vec3(0.0, 0.01, 0.03), 1.0 - smoothstep(0.86, 1.05, socket));
    float rim = socket - 1.05;
    c += vec3(0.3, 0.5, 0.8) * 0.5 * exp(-rim * rim * 60.0);
    // The nose: an upside-down heart.
    vec2 n = vec2(p.x, a.y - 0.43);
    float nose = max(n.x * 1.5 + n.y * 0.55 - 0.05, -n.y - 0.03);
    c = mix(c, vec3(0.0, 0.01, 0.03), 1.0 - smoothstep(0.0, 0.012, nose));
    // Teeth, top and bottom, apart as far as the mouth is open.
    float gap = 0.008 + open * 0.05;
    float top = a.y - (0.312 + gap);
    float bottom = (0.3 - gap) - a.y;
    float rows = max(-box(vec2(p.x, top - 0.035), vec2(0.13, 0.035)), -box(vec2(p.x, bottom - 0.035), vec2(0.115, 0.035)));
    float teeth = smoothstep(0.0, 0.004, rows);
    float parted = 0.5 + 0.5 * cos(a.x * 6.2831853 * 26.0);
    c = mix(c, vec3(0.01), smoothstep(0.0, 0.01, 0.019 + gap - abs(a.y - 0.306)) * step(p.x, 0.15));
    c = mix(c, vec3(0.93, 0.93, 0.85) * (0.45 + 0.55 * smoothstep(0.2, 0.6, parted)), teeth);
    return c;
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    float wide = s_wide / 100.0;
    float sweep = fract(u_time * s_speed / 200.0) * (1.0 + wide * 2.0) - wide;
    float band = abs(uv.x - sweep);
    float inside = 1.0 - smoothstep(wide * 0.9, wide, band);
    float off_line = (band - wide) * 90.0;
    float line = exp(-off_line * off_line);
    if (inside > 0.0) {
        vec3 negative = 1.0 - c;
        float luma = dot(negative, vec3(0.3, 0.59, 0.11));
        vec3 film = vec3(0.1, 0.2, 0.36) * luma * luma * 1.6;
        float on = on_face(uv);
        if (on > 0.0) {
            int who = face_there(uv);
            film = mix(film, skull(face_uv(uv), u_face_open[max(who, 0)].z, c), on);
        }
        c = mix(c, film, inside * u_amount);
    }
    c += vec3(0.3, 0.7, 1.0) * line * (0.6 + u_beat) * u_amount;
    return vec4(c, 1.0);
}
