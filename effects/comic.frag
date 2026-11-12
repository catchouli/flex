// Comic Book People: flat, punchy colour printed as dots of cyan, magenta, yellow and black at the
// usual screen angles, with a heavy black line round people and their features. Fading in, the
// print comes up over people and the speech bubbles swell in.

float dots(vec2 p, float amount, float angle, float size) {
    float c = cos(angle), s = sin(angle);
    vec2 q = mat2(c, -s, s, c) * p / size;
    float d = length(fract(q) - 0.5);
    return 1.0 - smoothstep(-0.08, 0.08, d - 0.72 * sqrt(clamp(amount, 0.0, 1.0)));
}

// Features drawn crisply from the mesh, where the picture alone would be mush: an inked line at
// the edge of the lips, eyes and brows.
float features(vec2 uv) {
    return lips_at(uv) + eye_at(uv) + brows_at(uv) + mouth_at(uv) * 0.5 + iris_at(uv) * 0.8;
}

float feature_line(vec2 uv, float wide) {
    vec2 e = wide / u_resolution;
    return clamp((abs(features(uv + vec2(e.x, 0.0)) - features(uv - vec2(e.x, 0.0))) + abs(features(uv + vec2(0.0, e.y)) - features(uv - vec2(0.0, e.y)))) * 1.5, 0.0, 1.0);
}

// Speech bubbles while someone's talking: how recently each mouth was open is kept in the alpha
// of the bottom row of u_prev.
float talking(int face) {
    return texelFetch(u_prev, ivec2(face, 0), 0).a;
}

vec3 bubbles(vec3 c, vec2 uv) {
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float shout = clamp(u_sound_shouting * s_react / 40.0, 0.0, 1.5);
    float biggest = 0.0;
    for (int i = 0; i < u_face_count; i++) biggest = max(biggest, u_faces[i].w - u_faces[i].y);
    for (int i = 0; i < u_face_count; i++) {
        float t = talking(i);
        vec4 b = u_faces[i];
        // Only the people the shot's about get to speak.
        if (t <= 0.02 || u_face_seen[i] < 0.5 || b.w - b.y < biggest * 0.75) continue;
        vec2 size = (b.zw - b.xy) * aspect;
        // On whichever side of the head there's more picture.
        float side = 0.5 * (b.x + b.z) < 0.5 ? 1.0 : -1.0;
        vec2 mouth = 0.5 * (face_point(i, 3) + face_point(i, 4)) * aspect;
        vec2 radius = size * vec2(0.72, 0.36) * smoothstep(0.0, 0.5, t) * (0.5 + 0.5 * u_amount);
        vec2 middle = vec2(mouth.x + side * size.x * 1.25, b.w - size.y * 0.12);
        middle.x = clamp(middle.x, u_video_rect.x * aspect.x + radius.x * 1.05, u_video_rect.z * aspect.x - radius.x * 1.05);
        middle.y = clamp(middle.y, u_video_rect.y + radius.y * 1.1, u_video_rect.w - radius.y * 1.1);
        vec2 q = (p - middle) / max(radius, vec2(1e-4));
        float d = length(q);
        // Shouting gives the bubble spikes, the more so the surer it is.
        float spikes = abs(fract(atan(q.y, q.x) * 2.2282 + 0.25) - 0.5) * 2.0;
        d /= 1.0 + shout * 0.22 * spikes;
        // The tail, a thin triangle back to the mouth.
        vec2 to_mouth = mouth - middle;
        float along = clamp(dot(p - middle, to_mouth) / dot(to_mouth, to_mouth), 0.0, 0.92);
        float tail = length(p - middle - to_mouth * along) / (size.x * 0.13 * (1.0 - along));
        float shape = min(d, 0.6 + 0.4 * tail);
        float line = 2.5 / u_resolution.y / max(radius.y, 1e-4);
        c = mix(c, vec3(0.02), (1.0 - smoothstep(1.0 + line, 1.0 + line * 1.6, shape)) * u_amount);
        c = mix(c, vec3(0.98), (1.0 - smoothstep(1.0 - line * 0.3, 1.0 + line * 0.3, shape)) * u_amount);
        // Scribbled words, jumping with the sound.
        float rows = floor((q.y + 0.6) * 3.0 / 1.2);
        vec2 row = vec2(q.x, fract((q.y + 0.6) * 3.0 / 1.2) - 0.5);
        if (rows >= 0.0 && rows < 3.0 && abs(q.x) < 0.68 - rows * 0.08 * (rows - 1.0)) {
            float word = sin(q.x * 90.0 + rows * 2.0 + floor(u_time * 6.0)) * 0.16 * (0.4 + u_mid);
            float gaps = smoothstep(-0.2, 0.0, sin(q.x * 11.0 + rows * 5.0));
            c = mix(c, vec3(0.03), (1.0 - smoothstep(0.05, 0.1, abs(row.y - word))) * gaps * (1.0 - smoothstep(1.0, 1.02, d)) * u_amount);
        }
    }
    return c;
}

float luma(vec2 uv) {
    return dot(texture(u_video, uv).rgb, vec3(0.3, 0.59, 0.11));
}

// How talkative a face is, for keeping in the alpha.
float remembered(vec2 uv) {
    ivec2 pixel = ivec2(gl_FragCoord.xy);
    if (pixel.y != 0 || pixel.x >= MAX_FACES) return 1.0;
    int i = pixel.x;
    bool here = i < u_face_count && u_face_seen[i] > 0.5 && meshed(i);
    if (!here) return 0.0;
    // While speech is heard a bubble lasts longer between words, so it doesn't keep popping away.
    float heard = clamp(u_sound_speech * s_react / 40.0, 0.0, 1.0);
    return u_face_open[i].z > 0.2 ? 1.0 : talking(i) * pow(0.25, u_dt * (1.0 - 0.8 * heard));
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    float kept = remembered(uv);
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, kept);
    float mask = person_at(uv);
    float person = smoothstep(0.42, 0.6, mask);
    vec2 e = 2.0 / u_resolution * u_resolution.y / 720.0;
    float rim = smoothstep(0.32, 0.42, mask) * (1.0 - smoothstep(0.42, 0.56, mask));
    if (person + rim <= 0.0) return vec4(bubbles(c, uv), kept);
    // Film is dark for print, so it's lifted a long way first.
    vec3 lifted = pow(clamp(c, 0.0, 1.0), vec3(0.5));
    vec3 flat_colour = clamp((floor(lifted * 5.0 + 0.5) / 5.0 - 0.45) * s_punch / 100.0 + 0.45, 0.0, 1.0);
    float size = s_dots * u_resolution.y / 720.0 * (1.0 + 0.25 * u_beat * s_react / 100.0);
    vec2 p = uv * u_resolution;
    float black = 1.0 - max(flat_colour.r, max(flat_colour.g, flat_colour.b));
    vec3 ink = (1.0 - flat_colour - black) / max(1.0 - black, 0.01);
    vec3 paper = vec3(0.98, 0.95, 0.88);
    paper *= 1.0 - vec3(0.95, 0.1, 0.05) * dots(p, ink.r, 0.2618, size);
    paper *= 1.0 - vec3(0.05, 0.9, 0.1) * dots(p, ink.g, 1.309, size);
    paper *= 1.0 - vec3(0.02, 0.08, 0.9) * dots(p, ink.b, 0.0, size);
    paper *= 1.0 - dots(p, max(black - 0.35, 0.0) * 1.3, 0.7854, size);
    float dx = luma(uv + vec2(e.x, 0.0)) - luma(uv - vec2(e.x, 0.0));
    float dy = luma(uv + vec2(0.0, e.y)) - luma(uv - vec2(0.0, e.y));
    float line = clamp(smoothstep(0.1, 0.22, length(vec2(dx, dy))) * s_outline / 100.0 + feature_line(uv, 2.0 * u_resolution.y / 720.0), 0.0, 1.0);
    vec3 printed = mix(paper, vec3(0.02), line);
    vec3 shown = mix(c, printed, person * u_amount);
    return vec4(bubbles(mix(shown, vec3(0.0), rim * s_outline / 70.0 * u_amount), uv), kept);
}
