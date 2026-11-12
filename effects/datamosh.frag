// Datamosh: moves last frame's output along with how each block of the picture is moving, instead
// of showing the new frame, so the wrong picture gets dragged about by the right movement. Fading
// in, less and less of the new frame comes through and the smear builds up.

float luma(vec3 c) {
    return dot(c, vec3(0.3, 0.59, 0.11));
}

vec4 effect(vec2 uv) {
    vec3 video = texture(u_video, uv).rgb;
    float now = luma(video);
    if (u_frame < 2 || uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(video, now);

    vec2 pixel = 1.0 / u_resolution;
    float block = s_block * u_resolution.y / 720.0;
    vec2 cell = floor(uv * u_resolution / block);
    vec2 middle = (cell + 0.5) * block * pixel;

    vec2 moved = flow_at(middle) * u_resolution * u_dt * s_smear / 100.0 * u_amount;

    float close = s_near / 100.0 * smoothstep(0.5, 0.85, depth_at(middle));
    moved *= 1.0 - close;
    vec3 c = texture(u_prev, uv - moved * pixel).rgb;
    float seed = hash(cell + floor(u_video_time * 8.0));
    float clean = step(seed, u_beat * s_clean / 100.0);
    float back = mix(0.04, 0.002, s_rot / 100.0);
    c = mix(c, video, max(max(max(clean, back), close * 0.25), 1.0 - u_amount));
    return vec4(c, now);
}
