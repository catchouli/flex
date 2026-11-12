// Cyclops: paints skin from under each eye over it, then puts one big eye between them, made
// from whichever eye is facing the camera. Fading in, skin creeps over the eyes and the big eye
// opens out from nothing between them.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i];
        if (seen <= 0.0) continue;
        Face f = face_frame(i, aspect);
        vec2 hole = vec2(f.spread * 0.42, f.spread * 0.3) * max(u_amount, 1e-4);
        vec2 cheek = -f.up * f.spread * 0.6;
        float covered = max(oval(p, f.right, f, hole), oval(p, f.left, f, hole)) * seen * step(1e-4, u_amount);
        if (covered > 0.0) c = mix(c, texture(u_video, (p + cheek) / aspect).rgb, covered);

        float zoom = s_size / 100.0 * (1.0 + 0.4 * u_bass * s_react / 100.0) * max(u_amount, 1e-4);
        vec2 size = vec2(f.spread * 0.42, f.spread * 0.32) * zoom;
        float amount = oval(p, f.eyes, f, size) * seen * step(1e-4, u_amount);
        if (amount > 0.0) {
            vec2 rel = (p - f.eyes) / zoom;
            float nearer = smoothstep(-0.1, 0.1, dot(f.nose - f.eyes, f.across) / f.spread);
            vec3 right = texture(u_video, (f.right + rel) / aspect).rgb;
            vec3 left = texture(u_video, (f.left + rel) / aspect).rgb;
            c = mix(c, mix(left, right, nearer), amount);
        }
    }
    return vec4(c, 1.0);
}
