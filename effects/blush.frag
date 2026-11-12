// Blush Meter: the loudness drives how red faces get and how much steam comes off them. Laughter
// brings an embarrassed blush to the cheeks, and shouting turns the whole face red and steaming.
// Fading in, the colour rises in faces and the steam starts to come.

// Embarrassment is drawn the cartoon way, with a few short slanted strokes across a cheek, where
// `q` runs from -1 to 1 over it, outwards and up.
float strokes(vec2 q) {
    return smoothstep(0.55, 0.85, sin((q.x - 0.5 * q.y) * 7.0)) * (1.0 - smoothstep(0.2, 0.8, length(q * vec2(0.9, 1.6))));
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float shy = clamp(u_sound_laughter, 0.0, 1.0);
    float anger = clamp(u_sound_shouting, 0.0, 1.0);
    float heat = mix(smoothstep(0.1, 0.9, u_level), 1.0, 0.8 * anger);
    float over = max(smoothstep(s_boil / 100.0, s_boil / 100.0 + 0.3, u_level), anger);
    // The cheeks go red with the heat, or with embarrassment.
    float cheeks = mix(0.3 + 0.7 * heat, 1.0, shy);
    vec3 steam_light = vec3(0.0);
    // With a mesh, only skin goes red, and the cheeks stay on the cheeks as the head turns.
    float skin = skin_at(uv) + nose_at(uv);
    if (skin > 0.0) {
        vec2 a = face_uv(uv);
        vec2 cheek = vec2(abs(a.x - 0.5) - 0.225, a.y - 0.46) / vec2(0.14, 0.1);
        float amount = s_blush / 100.0 * skin * u_amount;
        c = mix(c, c * vec3(1.35, 0.55, 0.5), heat * amount);
        c = mix(c, vec3(0.95, 0.15, 0.2) * (0.5 + dot(c, vec3(0.33))), (1.0 - smoothstep(0.3, 1.0, length(cheek))) * cheeks * amount * 0.8);
        c = mix(c, vec3(0.7, 0.05, 0.12), strokes(cheek) * shy * amount * 0.7);
    }
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i];
        if (seen <= 0.0) continue;
        Face f = face_frame(i, aspect);
        vec2 mid = 0.5 * (f.eyes + f.mouth);
        float amount = s_blush / 100.0 * seen * u_amount;
        if (!meshed(i)) c = mix(c, c * vec3(1.35, 0.55, 0.5), oval(p, mid, f, vec2(f.spread * 1.2, f.height * 1.45)) * heat * amount);
        for (int e = -1; e <= 1; e += 2) {
            vec2 cheek = f.eyes - f.up * f.height * 0.55 + f.across * float(e) * f.spread * 0.62;
            if (!meshed(i)) {
                c = mix(c, vec3(0.95, 0.15, 0.2), oval(p, cheek, f, vec2(f.spread * 0.36, f.spread * 0.24)) * cheeks * amount * 0.8);
                vec2 rel = p - cheek;
                c = mix(c, vec3(0.7, 0.05, 0.12), strokes(vec2(dot(rel, f.across) * float(e), dot(rel, f.up)) / (f.spread * vec2(0.3, 0.2))) * shy * amount * 0.7);
            }
            if (over <= 0.0) continue;
            vec2 ear = f.eyes + f.across * float(e) * f.spread * 1.1;
            vec2 rel = p - ear;
            float up = dot(rel, f.up) / f.height;
            if (up < 0.0 || up > 3.0) continue;
            float along = dot(rel, f.across) / f.height - float(e) * up * 0.25;
            float wander = (fbm(vec2(along * 3.0, up * 2.0 - u_time * 1.5) + float(i)) - 0.47) * 0.5;
            float width = 0.09 + up * 0.18;
            float puff = (1.0 - smoothstep(0.0, width, abs(along + wander))) * (1.0 - smoothstep(0.5, 3.0, up)) * smoothstep(0.0, 0.15, up);
            steam_light += vec3(0.85) * puff * over * seen * s_steam / 60.0 * u_amount;
        }
    }
    return vec4(c + steam_light, 1.0);
}
