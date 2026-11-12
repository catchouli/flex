// Giraffe Necks: each head is lifted off and the bit of neck under the chin is stretched to reach it.
// Fading in, the necks grow from their own length.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float t = u_video_time;
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i];
        if (seen <= 0.0) continue;
        Face f = face_frame(i, aspect);
        vec2 size = (u_faces[i].zw - u_faces[i].xy) * aspect;
        vec2 chin = 0.5 * (u_faces[i].xy + u_faces[i].zw) * aspect - f.up * size.y * 0.5;
        float lift = size.y * s_lift / 100.0 * 1.4 * (1.0 + 0.3 * u_bass * s_react / 100.0) * seen * u_amount;
        float swing = sin(t * 1.3 + float(i) * 2.0) * s_sway / 100.0 * size.x * 0.9 * u_amount;
        vec2 rel = p - chin;
        float up = dot(rel, f.up);
        float along = clamp(up / max(lift, 1e-4), 0.0, 1.0);
        float bent = swing * along * along;
        float x = dot(rel, f.across) - bent;
        float neck = size.x * 0.27;
        // The neck.
        float low = size.y * 0.3;
        if (up > -low && up < lift + size.y * 0.1 && abs(x) < neck) {
            float reach = (up + low) / (lift + low);
            // Short of all of it, the neck is drawn from more of itself, all of itself at none.
            vec2 from = chin + f.across * x + f.up * (-low + reach * low * 0.95 + reach * (lift + low * 0.05) * (1.0 - u_amount));
            float shade = mix(1.0, 0.8 + 0.25 * cos(x / neck * 1.57), u_amount);
            c = mix(c, texture(u_video, from / aspect).rgb * shade, (1.0 - smoothstep(neck * 0.9, neck, abs(x))) * seen);
        }
        // The head, on top of it.
        vec2 lifted = p - f.up * lift - f.across * swing;
        vec2 middle = chin + f.up * size.y * 0.62;
        vec2 l = vec2(dot(lifted - middle, f.across) / (size.x * 0.8), dot(lifted - middle, f.up) / (size.y * 0.78));
        float head = 1.0 - smoothstep(0.86, 1.0, length(l));
        if (head > 0.0 && lift > 0.0) c = mix(c, texture(u_video, lifted / aspect).rgb, head * seen);
    }
    return vec4(c, 1.0);
}
