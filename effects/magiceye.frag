// Magic Eye: a pattern repeats across the screen, each repeat shifted by how near that point is.
// Each pixel walks back across the repeats to the first strip to find what it should copy. Fading
// in, the pattern comes over the picture and its depth grows from flat.

vec4 effect(vec2 uv) {
    vec3 film = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(0.0, 0.0, 0.0, 1.0);
    float width = u_resolution.x;
    float strip = floor(width / max(s_strips, 2.0));
    float x = floor(uv.x * width);
    for (int i = 0; i < 40 && x >= strip; i++) {
        float d = depth_at(vec2(x / width, uv.y));
        x -= strip - floor(d * strip * 0.28 * s_depth / 100.0 * u_amount);
    }
    if (s_pattern < 0.5) {
        vec2 cell = vec2(max(x, 0.0), floor(uv.y * u_resolution.y));
        float shimmer = floor(u_time * s_speed / 10.0);
        vec3 c = vec3(hash(cell + shimmer), hash(cell + 3.1 + shimmer), hash(cell + 7.7 + shimmer));
        return vec4(mix(film, mix(c, vec3(dot(c, vec3(0.33))), 0.3), u_amount), 1.0);
    }
    return vec4(mix(film, texture(u_video, vec2(u_video_rect.x + x / strip * (u_video_rect.z - u_video_rect.x), uv.y)).rgb, u_amount), 1.0);
}
