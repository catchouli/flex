// Hall of Faces: draws each face again, several times, in a ring that turns around it. Fading in,
// the copies come out of the face to their places in the ring.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    int copies = int(floor(s_copies));
    for (int i = 0; i < u_face_count; i++) {
        if (u_face_seen[i] <= 0.0) continue;
        float shown = u_face_seen[i] * u_amount;
        vec2 centre = 0.5 * (u_faces[i].xy + u_faces[i].zw) * aspect;
        vec2 radius = 0.5 * (u_faces[i].zw - u_faces[i].xy) * aspect;
        float size = max(radius.x, radius.y);
        float breathe = 1.0 + 0.25 * u_bass + 0.1 * sin(u_time * 1.3 + float(i));
        for (int k = 0; k < copies; k++) {
            float a = 6.2831853 * float(k) / float(copies) + u_time * 0.4 * (mod(float(i), 2.0) * 2.0 - 1.0);
            vec2 at = centre + vec2(cos(a), sin(a)) * size * s_reach / 100.0 * u_amount * breathe;
            vec2 rel = p - at;
            float spin = a * 0.5 + u_time * 0.3;
            vec2 turned = mat2(cos(spin), -sin(spin), sin(spin), cos(spin)) * rel;
            float d = length(turned / max(radius * 0.8, vec2(1e-4)));
            float mask = (1.0 - smoothstep(0.8, 1.0, d)) * shown;
            if (mask > 0.0) {
                vec3 copy = texture(u_video, (centre + turned / 0.8) / aspect).rgb;
                if (meshed(i)) {
                    // Cut out along the edge of the face, like a mask on a stick.
                    vec2 at_face = turned / max(radius * 1.7, vec2(1e-4)) + vec2(0.5, 0.47);
                    vec4 skin = face_texture(i, clamp(at_face, 0.0, 1.0));
                    copy = skin.rgb;
                    mask = skin.a * shown;
                }
                c = mix(c, hue_rotate(copy, a + u_time * 0.5) * 1.15, mask);
            }
        }
    }
    return vec4(c, 1.0);
}
