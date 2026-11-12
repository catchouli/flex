// Colour Time Split: one colour is now, and the other two are running averages of the past, so
// still things look normal and moving things split apart. Fading in, the two colours fall
// further and further behind.

vec4 effect(vec2 uv) {
    vec3 video = texture(u_video, uv).rgb;
    if (u_frame < 2) return vec4(video, 1.0);
    vec3 before = texture(u_prev, uv).rgb;
    float lag = s_lag / 100.0;
    float soon = mix(0.5, 0.06, lag), late = mix(0.2, 0.02, lag);
    vec3 rates;
    if (s_order < 0.5) rates = vec3(1.0, soon, late);
    else if (s_order < 1.5) rates = vec3(late, 1.0, soon);
    else rates = vec3(soon, late, 1.0);
    // Up close it's nearly in step; the distance falls further behind.
    rates = mix(rates, vec3(1.0), s_near / 100.0 * smoothstep(0.4, 0.85, depth_at(uv)));
    if (u_amount < 1.0) rates = mix(vec3(1.0), rates, u_amount);
    return vec4(mix(before, video, rates), 1.0);
}
