// Clown Car: everything's drawn in each face's own coordinates, so it turns and tilts with them.
// Fading in, the make-up thickens on and the nose grows from nothing.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    // With a mesh the make-up goes right on the skin, and moves with it.
    float on = on_face(uv) * u_amount;
    if (on > 0.0) {
        vec3 film = c;
        float luma = dot(c, vec3(0.3, 0.59, 0.11));
        vec2 a = face_uv(uv);
        c = mix(c, vec3(0.98, 0.97, 0.95) * (0.55 + 0.75 * luma), on * s_paint / 100.0);
        vec2 side = vec2(abs(a.x - 0.5) - 0.154, a.y - 0.62);
        float diamond = 1.0 - smoothstep(0.92, 1.0, abs(side.x) / 0.085 + abs(side.y) / 0.17);
        c = mix(c, vec3(0.1, 0.25, 0.85) * (0.5 + luma), diamond * on);
        float x = (a.x - 0.5) / 0.3;
        float smile = 0.305 + 0.13 * x * x;
        float band = (1.0 - smoothstep(0.038, 0.05, abs(a.y - smile) - 0.012 * (1.0 - x * x))) * (1.0 - smoothstep(0.92, 1.05, abs(x)));
        c = mix(c, vec3(0.85, 0.05, 0.08) * (0.55 + 0.7 * luma), max(band * on, lips_at(uv) * u_amount));
        c = mix(c, film, eye_at(uv));
        c = mix(c, film * 0.5, mouth_at(uv) * u_amount);
    }
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i];
        if (seen <= 0.0) continue;
        float shown = seen * u_amount;
        Face f = face_frame(i, aspect);
        vec2 mid = 0.5 * (f.eyes + f.mouth);
        float luma = dot(c, vec3(0.3, 0.59, 0.11));
        if (!meshed(i)) {
        float paint = oval(p, mid, f, vec2(f.spread * 1.2, f.height * 1.45)) * shown * s_paint / 100.0;
        c = mix(c, vec3(0.98, 0.97, 0.95) * (0.55 + 0.75 * luma), paint);

        for (int e = 0; e < 2; e++) {
            vec2 eye = e == 0 ? f.right : f.left;
            vec2 rel = p - eye;
            vec2 l = abs(vec2(dot(rel, f.across) / (f.spread * 0.3), dot(rel, f.up) / (f.spread * 0.62)));
            float diamond = (1.0 - smoothstep(0.92, 1.0, l.x + l.y)) * shown;
            c = mix(c, vec3(0.1, 0.25, 0.85), diamond);
            c = mix(c, texture(u_video, uv).rgb, oval(p, eye, f, vec2(f.spread * 0.26, f.spread * 0.16)) * seen);
        }

        vec2 rel_mouth = p - f.mouth;
        float wide = f.spread * 0.95;
        float x = dot(rel_mouth, f.across) / wide;
        float y = dot(rel_mouth, f.up) / wide;
        float curve = 0.42 * x * x - 0.05;
        float lip = (1.0 - smoothstep(0.12, 0.17, abs(y - curve) - 0.06 * (1.0 - x * x))) * (1.0 - smoothstep(0.95, 1.05, abs(x)));
        c = mix(c, vec3(0.85, 0.05, 0.08), lip * shown);
        }

        float size = f.spread * 0.2 * s_nose / 100.0 * (1.0 + 0.5 * u_beat * s_react / 100.0) * u_amount;
        size = max(size, 1e-6);
        vec2 n = (p - f.nose) / size;
        float d = length(n);
        vec3 ball = vec3(0.9, 0.05, 0.05) * (0.55 + 0.6 * clamp(dot(normalize(vec3(n, max(1.0 - d * d, 0.0) + 0.001)), normalize(vec3(-0.4, 0.6, 0.7))), 0.0, 1.0));
        ball += vec3(0.8) * (1.0 - smoothstep(0.0, 0.25, length(n - vec2(-0.3, 0.4))));
        c = mix(c, ball, (1.0 - smoothstep(0.95, 1.03, d)) * seen * step(1e-4, u_amount));
    }
    return vec4(c, 1.0);
}
