// Wax Museum: below each face, columns of the picture are pulled from the face's lower half,
// each column to its own length, so the chin runs in drips. Fading in, the drips run longer
// from nothing.

float drip(float x, float t) {
    return 0.5 + 0.28 * sin(x * 31.0 + t) + 0.14 * sin(x * 83.0 - t * 1.7) + 0.08 * sin(x * 171.0 + t * 0.6);
}

vec4 effect(vec2 uv) {
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    vec2 q = uv;
    float t = u_video_time * s_speed;
    for (int i = 0; i < u_face_count; i++) {
        vec4 b = u_faces[i];
        vec2 size = b.zw - b.xy;
        if (u_face_seen[i] <= 0.0 || size.x <= 0.0) continue;
        float inside = (b.x + b.z) * 0.5;
        float edge = 1.0 - smoothstep(0.32, 0.5, abs(uv.x - inside) / size.x);
        if (edge <= 0.0) continue;
        float length_of = size.y * 1.8 * s_melt / 100.0 * drip(uv.x / max(size.x, 1e-3), t + float(i)) * u_face_seen[i] * u_amount;
        float chin = b.y + size.y * 0.32;
        float bottom = chin - length_of;
        if (uv.y < chin && uv.y > bottom) {
            float along = (chin - uv.y) / max(length_of, 1e-4);
            float eased = 1.0 - (1.0 - along) * (1.0 - along);
            q.y = mix(q.y, chin + size.y * 0.04 * sin(along * 9.0 + t) + size.y * 0.02 - eased * size.y * 0.1, edge);
            q.x += edge * size.x * 0.03 * sin(along * 11.0 + uv.x * 40.0 + t * 2.0) * along;
        }
    }
    return vec4(texture(u_video, clamp(q, u_video_rect.xy, u_video_rect.zw)).rgb, 1.0);
}
