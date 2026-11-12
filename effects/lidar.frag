// Lidar Sweep: a front moves out through the depth of the scene. Whatever it's passed glows, and
// fades, as a set of scan dots. Fading in, the picture darkens as the scan comes up.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(0.0, 0.0, 0.0, 1.0);
    vec3 tints[4] = vec3[4](vec3(0.2, 1.0, 0.35), vec3(0.15, 0.85, 1.0), vec3(1.0, 0.15, 0.1), vec3(1.0, 0.65, 0.15));
    vec3 tint = tints[int(s_colour + 0.5)];
    float far = 1.0 - depth_at(uv);
    float front = fract(u_time * 0.22 * s_speed / 100.0) * 1.3 - 0.15;
    float behind = fract(front - far + 2.0);
    float line = 1.0 - smoothstep(0.0, 0.012, min(behind, 1.0 - behind));
    float after = exp(-behind * 11.0 * (1.2 - s_glow / 100.0));
    vec2 grid = fract(uv * u_resolution / (3.0 * u_resolution.y / 720.0));
    float dots = 1.0 - smoothstep(0.25, 0.45, length(grid - 0.5));
    float luma = dot(c, vec3(0.3, 0.59, 0.11));
    vec3 lit = tint * (luma * 1.6 + 0.08) * after * (0.5 + 0.5 * dots) * (1.0 + u_beat * s_react / 100.0);
    lit += mix(tint, vec3(1.0), 0.6) * line * 1.5;
    return vec4(lit * u_amount + c * mix(vec3(1.0), tint * after * s_film / 100.0, u_amount), 1.0);
}
