// Witness Protection: hides who everyone is. Fading in, the bar grows from nothing, the blocks
// and blur grow from a pixel, and the features smooth away.

// Skin with the features smoothed away: an average of a few bare patches, nearest counting most,
// which keeps the way the light falls across the face.
vec3 bare_skin(int face, vec2 a) {
    vec2 patches[6] = vec2[6](vec2(0.5, 0.78), vec2(0.3, 0.8), vec2(0.7, 0.8), vec2(0.2, 0.45), vec2(0.8, 0.45), vec2(0.5, 0.16));
    vec3 sum = vec3(0.0);
    float total = 0.0;
    for (int n = 0; n < 6; n++) {
        float weight = 1.0 / (dot(a - patches[n], a - patches[n]) + 0.02);
        sum += face_texture(face, patches[n]).rgb * weight;
        total += weight;
    }
    return sum / total;
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (u_amount <= 0.0) return vec4(c, 1.0);
    if (s_style > 2.5) {
        float on = on_face(uv) * u_amount;
        if (on > 0.0) c = mix(c, bare_skin(max(face_there(uv), 0), face_uv(uv)), on);
    }
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float scale = s_size / 100.0;
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i];
        if (seen <= 0.0) continue;
        if (s_style > 2.5 && meshed(i)) continue;
        Face f = face_frame(i, aspect);
        if (s_style < 0.5) {
            vec2 rel = p - f.eyes;
            vec2 l = abs(vec2(dot(rel, f.across) / (f.spread * 1.15 * scale * u_amount), dot(rel, f.up) / (f.spread * 0.34 * scale * u_amount)));
            if (max(l.x, l.y) < 1.0) c = mix(c, vec3(0.0), seen);
            continue;
        }
        vec2 mid = 0.5 * (f.eyes + f.mouth);
        float amount = oval(p, mid, f, vec2(f.spread * 1.35, f.height * 1.5) * scale) * seen;
        if (amount <= 0.0) continue;
        if (s_style < 1.5) {
            float block = f.height / 5.0;
            if (u_amount < 1.0) block = mix(1.0 / u_resolution.y, block, u_amount);
            vec2 at = (floor(p / block) + 0.5) * block;
            c = mix(c, texture(u_video, at / aspect).rgb, step(0.05, amount));
        } else {
            vec3 sum = vec3(0.0);
            for (int k = 0; k < 16; k++) {
                float a = float(k) * 2.3999632, r = sqrt(float(k) / 16.0) * f.height * 0.35 * u_amount;
                sum += texture(u_video, (p + vec2(cos(a), sin(a)) * r) / aspect).rgb;
            }
            c = mix(c, sum / 16.0, amount);
        }
    }
    return vec4(c, 1.0);
}
