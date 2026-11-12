// Tiny Heads: the opposite of Big Heads. The head is drawn small on its chin, and the ring of
// picture around it is stretched in to fill the gap, so nothing has to be invented. Fading in,
// heads shrink from their own size.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    vec2 q = p;
    float closest = 1.0;
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i];
        if (seen <= 0.0) continue;
        Face f = face_frame(i, aspect);
        vec2 size = (u_faces[i].zw - u_faces[i].xy) * aspect;
        vec2 chin = 0.5 * (u_faces[i].xy + u_faces[i].zw) * aspect - f.up * size.y * 0.5;
        float zoom = mix(1.0, s_size / 100.0 * (1.0 - 0.25 * u_bass * s_react / 100.0), seen * u_amount);
        vec2 head = size * vec2(0.8, 0.75);
        vec2 real_middle = chin + f.up * size.y * 0.62;
        vec2 drawn_middle = chin + f.up * size.y * 0.62 * zoom;
        vec2 d = p - drawn_middle;
        vec2 n = vec2(dot(d, f.across), dot(d, f.up)) / head;
        float r = max(length(n), 1e-4);
        const float OUTER = 2.2;
        if (r >= OUTER || r / OUTER >= closest) continue;
        closest = r / OUTER;
        float inner = zoom * 1.05;
        float s = r < inner ? r / zoom : mix(1.05, OUTER, (r - inner) / (OUTER - inner));
        vec2 centre = mix(real_middle, drawn_middle, smoothstep(inner, OUTER, r));
        vec2 dir = n / r * s * head;
        q = centre + f.across * dir.x + f.up * dir.y;
    }
    return vec4(texture(u_video, q / aspect).rgb, 1.0);
}
