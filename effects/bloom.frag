// Mind Bloom: the picture folded into a kaleidoscope, added to a zoomed copy of the last frame.
// Fading in, the picture bends into its folds and the copies start to build up.

vec4 effect(vec2 uv) {
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    vec2 size = u_video_rect.zw - u_video_rect.xy;
    vec2 centre = u_video_rect.xy + 0.5 * size;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = (uv - centre) * aspect;
    float arms = floor(s_arms + 4.0 * u_bass);
    float wedge = 6.2831853 / max(arms, 1.0);
    float a = atan(p.y, p.x) + u_video_time * 0.15;
    a = abs(mod(a, wedge) - 0.5 * wedge);
    float r = length(p) * (1.0 - 0.25 * u_beat) * (0.8 + 0.15 * sin(u_video_time * 0.4));
    vec2 fold = vec2(cos(a), sin(a)) * r / aspect;
    vec2 q = abs(mod(fold / size + 0.5, 2.0));
    q = u_video_rect.xy + size * min(q, 2.0 - q);
    // Near things bulge out of the pattern.
    fold *= 1.0 - (depth_at(q) - 0.5) * s_relief / 100.0 * 0.6;
    q = abs(mod(fold / size + 0.5, 2.0));
    q = u_video_rect.xy + size * min(q, 2.0 - q);
    q = q * u_amount + uv * (1.0 - u_amount);
    vec3 scope = texture(u_video, q).rgb;
    vec2 inward = centre + (uv - centre) * (0.985 - 0.02 * u_beat);
    vec3 past = hue_rotate(texture(u_prev, inward).rgb, 0.05 + 0.2 * u_beat);
    float keep = (0.6 + 0.37 * s_bloom / 100.0) * u_amount;
    return vec4(max(scope * (1.05 - 0.05 * (1.0 - u_amount) - keep * 0.55), past * keep), 1.0);
}
