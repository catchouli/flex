// Spotlight: faces stay as they are while everything around them melts and dims. Where
// there are no faces, it's all melt. Fading in, the melt, the grey and the dimming grow from
// nothing.

vec4 effect(vec2 uv) {
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    vec2 size = u_video_rect.zw - u_video_rect.xy;
    vec2 p = (uv - u_video_rect.xy) / size;
    float t = u_video_time;
    float amount = s_melt / 100.0 * u_amount * (1.0 + u_bass * s_react / 100.0);
    float far = 1.0 - depth_at(uv);
    amount *= mix(1.0, 0.35 + 1.3 * far, s_distance / 100.0);
    vec2 flow = vec2(
        sin(p.y * 9.0 + t * 1.1) + 0.5 * sin(p.y * 23.0 - t * 1.7 + p.x * 6.0),
        sin(p.x * 7.0 - t * 0.9) + 0.5 * sin(p.x * 19.0 + t * 1.3 + p.y * 5.0)
    );
    vec2 q = clamp(uv + flow * 0.03 * amount * size, u_video_rect.xy, u_video_rect.zw);
    vec3 world = texture(u_video, q).rgb;
    float grey = dot(world, vec3(0.3, 0.59, 0.11));
    world = mix(world, vec3(grey) * vec3(0.75, 0.85, 1.1), 0.7 * u_amount);
    world *= 1.0 - s_dim / 100.0 * u_amount * (1.0 - 0.3 * u_beat * s_react / 100.0) * mix(1.0, 0.5 + far, s_distance / 100.0);
    vec3 face = texture(u_video, uv).rgb;
    return vec4(mix(world, face, face_at(uv)), 1.0);
}
