// Depth: a test effect for depth (depth = true in depth.toml). depth_at(uv) gives how near
// each point is, from 0 (far) to 1 (near), from Depth Anything V2 running on the GPU; it
// covers the picture (not the black bars) and arrives a frame or two after the picture.
//
// s_mode: 0 contours, 1 fog, 2 show the depth.

vec3 rainbow(float t) {
    return 0.5 + 0.5 * cos(6.2831853 * (t + vec3(0.0, 0.33, 0.67)));
}

vec4 effect(vec2 uv) {
    vec3 video = texture(u_video, uv).rgb;
    float d = depth_at(uv);
    float amount = s_amount / 100.0;
    float t = u_video_time * s_speed;

    if (s_mode > 1.5) {
        return vec4(vec3(d), 1.0);
    }
    if (s_mode > 0.5) {
        // Fog: far away fades into a slowly drifting colour.
        vec3 fog = rainbow(0.1 * t + 0.3 * uv.x) * 0.6 + 0.2;
        float thick = smoothstep(0.75, 0.0, d) * amount;
        return vec4(mix(video, fog, thick), 1.0);
    }
    // Contours: bands of colour at equal steps of depth, moving outwards over time.
    float pulse = u_beat * s_react / 100.0;
    float band = d * s_bands - t * 0.5 - pulse;
    float line = 1.0 - smoothstep(0.0, 0.12, abs(fract(band) - 0.5) - 0.38);
    vec3 colour = rainbow(floor(band) / s_bands + 0.05 * t);
    vec3 tinted = mix(video, video * colour * 1.6, amount * 0.6);
    return vec4(mix(tinted, colour, line * amount), 1.0);
}
