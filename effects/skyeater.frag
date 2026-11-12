// Sky Eater: the sky shows the film again in log-polar space, a spiral falling forever.
// Without sky in the shot, nothing happens, until there's a window. Fading in, the hole in the
// sky opens out from the spiral's middle.

vec4 effect(vec2 uv) {
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    vec3 now = texture(u_video, uv).rgb;
    float sky = smoothstep(0.35, 0.7, sky_at(uv));
    if (sky <= 0.0) {
        return vec4(now, 1.0);
    }
    vec2 size = u_video_rect.zw - u_video_rect.xy;
    vec2 centre = 0.5 * (u_video_rect.xy + u_video_rect.zw) + vec2(0.0, 0.25 * size.y);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = (uv - centre) * aspect;
    float r = max(length(p), 1e-4);
    float a = atan(p.y, p.x);
    float t = u_video_time * s_speed;
    float fall = fract(log(r) * 0.8 - t * 0.35 + floor(s_arms) * a / 6.2831853);
    vec2 back = vec2(fall, fract(a * floor(s_arms) / 6.2831853 + t * 0.1));
    vec2 sample_at = u_video_rect.xy + size * vec2(abs(back.x * 2.0 - 1.0), abs(back.y * 2.0 - 1.0));
    vec3 spiral = texture(u_video, sample_at).rgb;
    spiral *= 0.75 + 0.6 * hue(fall + t * 0.2 + u_bass * 0.5);
    spiral *= smoothstep(0.0, 0.08, r) * (1.0 + 2.0 * u_beat * smoothstep(0.1, 0.0, abs(fract(fall + 0.5) - 0.5)));
    float reach = u_amount * 1.4;
    float eaten = u_amount >= 1.0 ? 1.0 : 1.0 - smoothstep(reach - 0.1, reach, r);
    return vec4(mix(now, spiral, sky * eaten), 1.0);
}
