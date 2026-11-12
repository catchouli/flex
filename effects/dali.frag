// Melting Clocks: looks upwards from each point for something nearer than what's here. If it's in
// reach of this column's drip, the near thing hangs down over this point. People are left solid.
// Fading in, the drips lengthen from nothing and the sky turns gold.

vec4 effect(vec2 uv) {
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(0.0, 0.0, 0.0, 1.0);
    float t = u_video_time;
    float x = uv.x * u_resolution.x / u_resolution.y;
    float sway = sin(uv.y * 9.0 + t * 0.6) * 0.004 * s_sway / 50.0 * u_amount;
    vec2 at = uv + vec2(sway, 0.0);
    // Everything but people sags in slow lobes.
    float column = noise(vec2(x * 9.0, t * 0.07)) * noise(vec2(x * 3.0 + 4.0, t * 0.04));
    at.y = min(at.y + column * 0.18 * s_length / 100.0 * u_amount * (1.0 - smoothstep(0.3, 0.6, person_at(uv))), u_video_rect.w);
    float here = depth_at(at);
    float reach = 0.33 * s_length / 100.0 * u_amount * (0.25 + 0.75 * pow(noise(vec2(x * 26.0, t * 0.09)), 2.0) + 0.5 * noise(vec2(x * 6.0, 2.0 + t * 0.05)));
    vec2 from = at;
    float hanging = 0.0;
    float before = 0.0;
    for (int i = 1; i <= 14; i++) {
        float up = float(i) / 14.0 * reach;
        if (at.y + up > u_video_rect.w) break;
        if (depth_at(vec2(at.x, at.y + up)) > here + 0.03 && person_at(vec2(at.x, at.y + up)) < 0.4) {
            // Pin down where the edge is, so the drip doesn't come out in steps.
            float a = before, b = up;
            for (int n = 0; n < 5; n++) {
                float m = 0.5 * (a + b);
                if (depth_at(vec2(at.x, at.y + m)) > here + 0.03) b = m; else a = m;
            }
            // Stretched: most of the drip is the last sliver of the surface above it.
            from = vec2(at.x, at.y + b + b * 0.06);
            hanging = 1.0 - b / reach;
            break;
        }
        before = up;
    }
    vec3 c = texture(u_video, from).rgb;
    c *= 1.0 - 0.18 * hanging;
    float sky = smoothstep(0.35, 0.7, sky_at(uv)) * s_sky / 100.0 * u_amount;
    if (sky > 0.0) {
        float height = (uv.y - u_video_rect.y) / max(u_video_rect.w - u_video_rect.y, 1e-3);
        vec3 gold = mix(vec3(1.0, 0.75, 0.35), vec3(0.3, 0.55, 0.85), smoothstep(0.45, 1.0, height));
        c = mix(c, gold * (0.7 + 0.5 * dot(c, vec3(0.33))), sky);
    }
    return vec4(c, 1.0);
}
