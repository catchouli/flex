// Diorama: moves the viewpoint. Each point of the picture is pushed sideways by how far it
// is from the distance that stays still, so to draw a pixel we walk from near to far until
// we reach the surface that lands there; whatever gets uncovered smears from its edges. Fading in,
// the camera's movement grows from standing still.
//
// s_style: 0 floating camera, 1 vertigo (a dolly zoom), 2 red and cyan 3D.

// Past the picture's edge, a mirror image: clamping would streak its last row of pixels.
vec2 fit(vec2 q) {
    vec2 size = u_video_rect.zw - u_video_rect.xy;
    vec2 a = mod((q - u_video_rect.xy) / size, 2.0);
    return u_video_rect.xy + size * (1.0 - abs(1.0 - a));
}

vec2 look(vec2 uv, vec2 off) {
    float focus = s_focus / 100.0;
    float step_size = 1.0 / 31.0;
    for (int i = 0; i < 32; i++) {
        float z = 1.0 - float(i) * step_size;
        vec2 q = fit(uv + off * (z - focus));
        if (depth_at(q) >= z) {
            // Closes in on where exactly it was crossed, which keeps outlines smooth.
            float above = z + step_size;
            for (int j = 0; j < 5; j++) {
                float middle = 0.5 * (z + above);
                q = fit(uv + off * (middle - focus));
                if (depth_at(q) >= middle) {
                    z = middle;
                } else {
                    above = middle;
                }
            }
            return fit(uv + off * (z - focus));
        }
    }
    return fit(uv - off * focus);
}

vec4 effect(vec2 uv) {
    // The black bars stay black.
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    float t = u_video_time * s_speed;
    float size = s_size / 100.0 * u_amount;
    float kick = 1.0 + 1.5 * u_beat * s_react / 100.0;
    if (s_style > 1.5) {
        vec2 off = vec2(0.03 * size, 0.0);
        return vec4(texture(u_video, look(uv, off)).r, texture(u_video, look(uv, -off)).gb, 1.0);
    }
    if (s_style > 0.5) {
        vec2 centre = 0.5 * (u_video_rect.xy + u_video_rect.zw);
        float zoom = 0.5 * size * kick * sin(t * 0.8);
        return vec4(texture(u_video, look(uv, (uv - centre) * zoom)).rgb, 1.0);
    }
    vec2 off = 0.08 * size * kick * vec2(sin(t * 0.9), 0.6 * cos(t * 0.65));
    return vec4(texture(u_video, look(uv, off)).rgb, 1.0);
}
