// Depth: a test effect for depth (depth = true in depth.toml). depth_at(uv) gives how near
// each point is, from 0 (far) to 1 (near), from Depth Anything V2 running on the GPU; it
// covers the picture (not the black bars) and arrives a frame or two after the picture. Fading
// in, the overlay comes up over the picture.
//
// s_mode: 0 contours, 1 fog, 2 show the depth.

// A fog colour setting: the first choice is the drifting rainbow, the rest are fixed.
vec3 fog_colour(float choice, vec3 moving) {
    const vec3 named[11] = vec3[11](
        vec3(0.92, 0.94, 0.97), vec3(0.5, 0.52, 0.56), vec3(0.02, 0.02, 0.04), vec3(0.85, 0.1, 0.08), vec3(0.95, 0.5, 0.08),
        vec3(0.95, 0.85, 0.2), vec3(0.15, 0.75, 0.2), vec3(0.08, 0.7, 0.7), vec3(0.1, 0.3, 0.9), vec3(0.5, 0.15, 0.85), vec3(0.95, 0.3, 0.65)
    );
    return choice < 0.5 ? moving : named[int(choice + 0.5) - 1];
}

vec4 effect(vec2 uv) {
    vec3 video = texture(u_video, uv).rgb;
    float d = depth_at(uv);
    float amount = s_amount / 100.0 * u_amount;
    float t = u_video_time * s_speed;

    if (s_mode > 1.5) {
        return vec4(vec3(d) * u_amount + video * (1.0 - u_amount), 1.0);
    }
    if (s_mode > 0.5) {
        // Fog: far away fades into a slowly drifting colour.
        vec3 moving = hue(0.1 * t + 0.3 * uv.x) * 0.6 + 0.2;
        float far = 1.0 - smoothstep(0.0, 0.75, d);
        vec3 fog = mix(fog_colour(s_fog_start, moving), fog_colour(s_fog_end, moving), far);
        return vec4(mix(video, fog, far * amount), 1.0);
    }
    // Contours: bands of colour at equal steps of depth, moving outwards over time.
    float pulse = u_beat * s_react / 100.0;
    float band = d * s_bands - t * 0.5 - pulse;
    float line = 1.0 - smoothstep(0.0, 0.12, abs(fract(band) - 0.5) - 0.38);
    vec3 colour = hue(floor(band) / s_bands + 0.05 * t);
    vec3 tinted = mix(video, video * colour * 1.6, amount * 0.6);
    return vec4(mix(tinted, colour, line * amount), 1.0);
}
