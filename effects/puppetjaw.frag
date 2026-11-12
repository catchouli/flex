// Puppet Jaw: everything below the mouth slides down as a piece, like a Monty Python cut-out,
// and there's a dark mouth with teeth behind it. The sound decides how far. Fading in, the jaws
// drop further and further.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float level = s_listen < 0.5 ? u_mid : (s_listen < 1.5 ? u_level : u_bass);
    // Listening to voices, jaws hold still for music, bangs and engines with nobody talking,
    // singing, laughing or shouting over them. Singing counts as music too, so a voice wins.
    if (s_listen < 0.5) {
        float voice = clamp(u_sound_speech + u_sound_singing + u_sound_laughter + u_sound_shouting, 0.0, 1.0);
        float other = clamp(u_sound_music + u_sound_explosions + u_sound_impacts + u_sound_vehicles + u_sound_thunder + u_sound_alarms, 0.0, 1.0);
        level *= 1.0 - 0.85 * other * (1.0 - voice);
    }
    float open = pow(clamp(level, 0.0, 1.0), 1.5) * s_flap / 100.0;
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i];
        if (seen <= 0.0) continue;
        Face f = face_frame(i, aspect);
        float gape = s_listen > 2.5 ? (meshed(i) ? smoothstep(0.03, 0.6, u_face_open[i].z) * s_flap / 100.0 : 0.0) : open;
        float drop = gape * f.height * 0.9 * seen * u_amount;
        if (drop < f.height * 0.03) continue;
        vec2 rel = p - f.mouth;
        float x = dot(rel, f.across);
        float y = dot(rel, f.up);
        float jaw_width = f.spread * 0.95;
        float jaw_depth = f.height * 1.05;
        float below = -y - drop;
        // The jaw, a rounded block moved down.
        if (below > 0.0 && below < jaw_depth) {
            float shape = jaw_width * sqrt(max(1.0 - pow(below / jaw_depth, 2.0), 0.0));
            float inside = 1.0 - smoothstep(shape - 0.003, shape + 0.003, abs(x));
            if (inside > 0.0) c = mix(c, texture(u_video, (p + f.up * drop) / aspect).rgb, inside * seen);
        }
        // The dark hole it leaves.
        if (y < 0.0 && y > -drop) {
            float inside = 1.0 - smoothstep(jaw_width - 0.003, jaw_width + 0.003, abs(x));
            vec3 mouth = mix(vec3(0.22, 0.02, 0.03), vec3(0.03, 0.0, 0.0), -y / max(drop, 1e-4));
            float top = 1.0 - smoothstep(f.height * 0.13, f.height * 0.15, -y);
            float gaps = smoothstep(0.1, 0.2, abs(fract(x / (f.spread * 0.2)) - 0.5) * 2.0);
            mouth = mix(mouth, vec3(0.95, 0.93, 0.85), top * gaps * s_teeth / 100.0);
            c = mix(c, mouth, inside * seen);
        }
    }
    return vec4(c, 1.0);
}
