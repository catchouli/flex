// Hypno Eyes: a spiral in each eye, with rings of light spreading from it on the beat. Fading in,
// the spirals and their rings grow out of the middle of each eye.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float t = u_time * s_spin / 100.0;
    vec3 light = vec3(0.0);
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i] * step(1e-3, u_amount);
        if (seen <= 0.0) continue;
        Face f = face_frame(i, aspect);
        float radius = f.spread * 0.33 * s_size / 100.0 * max(u_amount, 1e-3);
        for (int e = 0; e < 2; e++) {
            vec2 eye = e == 0 ? f.right : f.left;
            vec2 rel = (p - eye) / radius;
            // With a mesh, the spiral is in the real eye, spinning around the real iris.
            bool in_eye = meshed(i) && s_where < 0.5;
            if (in_eye) rel = (p - iris_of(i, e) * aspect) / (radius * 0.55);
            float r = length(rel);
            float a = atan(rel.y, rel.x);
            float sp = 0.5 + 0.5 * sin(a * 2.0 + r * 16.0 - t * 5.0);
            vec3 spiral = mix(vec3(0.02), mix(vec3(0.95), hue(r * 0.5 - t * 0.1), s_colour / 100.0), smoothstep(0.4, 0.6, sp));
            float painted = in_eye ? eye_at(uv) * (1.0 - smoothstep(2.0, 2.6, r)) : 1.0 - smoothstep(0.95, 1.05, r);
            c = mix(c, spiral, painted * seen);
            float rings = pow(0.5 + 0.5 * sin(r * 2.5 - t * 3.0), 6.0) * exp(-r * 0.3);
            light += hue(r * 0.12 + t * 0.05) * rings * seen * smoothstep(1.0, 1.4, r) * (in_eye ? 0.14 + 0.8 * u_beat * s_react / 100.0 : 0.05 + 0.5 * u_beat * s_react / 100.0);
        }
    }
    return vec4(c + light, 1.0);
}
