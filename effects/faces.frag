// Faces: draws what effects get, for checking it and for working on an effect. Fading in, the
// overlay comes up over the picture.

float ring(vec2 uv, vec2 at, vec2 aspect) {
    return 1.0 - smoothstep(0.004, 0.007, length((uv - at) * aspect));
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    c = mix(c, c * 0.45, (1.0 - face_at(uv)) * u_amount);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 pixel = 2.0 / u_resolution;
    for (int i = 0; i < u_face_count; i++) {
        vec4 b = u_faces[i];
        vec2 inside = step(b.xy, uv) * step(uv, b.zw);
        vec2 inner = step(b.xy + pixel, uv) * step(uv, b.zw - pixel);
        float edge = inside.x * inside.y * (1.0 - inner.x * inner.y);
        c = mix(c, vec3(0.2, 1.0, 0.4), edge * u_face_seen[i] * u_amount);
        for (int p = 0; p < 5; p++) {
            c = mix(c, vec3(1.0, 0.15, 0.15), ring(uv, face_point(i, p), aspect) * u_face_seen[i] * u_amount);
        }
    }
    return vec4(c, 1.0);
}
