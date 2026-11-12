// Warp Speed: smears everything beyond a certain distance outwards from the middle, by more the
// further it is, with streaks of starlight coming out of the vanishing point. Fading in, the
// smear stretches out from nothing and the stars and tint come up.

vec4 effect(vec2 uv) {
    vec3 video = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(0.0, 0.0, 0.0, 1.0);
    vec2 middle = 0.5 * (u_video_rect.xy + u_video_rect.zw);
    vec2 d = uv - middle;
    float near = depth_at(uv);
    float away = smoothstep(s_from / 100.0, 1.0, 1.0 - near);
    float stretch = (0.04 + 0.1 * u_bass * s_react / 100.0) * s_amount / 100.0 * u_amount * away;
    vec3 sum = vec3(0.0);
    float total = 0.0;
    for (int i = 0; i < 24; i++) {
        float f = float(i) / 24.0;
        vec2 at = uv - d * stretch * f * 6.0;
        float weight = 1.0 - f * 0.6;
        sum += texture(u_video, at).rgb * weight;
        total += weight;
    }
    vec3 c = sum / total;
    c = mix(c, c * vec3(0.55, 0.75, 1.3), away * s_tint / 100.0 * u_amount);
    float a = atan(d.y * u_resolution.y, d.x * u_resolution.x);
    float r = length(d * vec2(u_resolution.x / u_resolution.y, 1.0));
    float spoke = floor(a * 70.0);
    float position = fract(r * 2.0 - u_time * (0.6 + hash(vec2(spoke, 1.0))) + hash(vec2(spoke, 2.0)));
    float streak = step(0.6, hash(vec2(spoke, 3.0))) * smoothstep(0.75, 1.0, position) * smoothstep(0.04, 0.3, r);
    c += vec3(0.85, 0.92, 1.0) * streak * away * s_stars / 60.0 * u_amount;
    return vec4(c, 1.0);
}
