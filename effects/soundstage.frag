// Soundstage: reads u_spectrum with depth as its time axis. Near is now and far is a moment
// ago, so sounds appear on whatever's closest and roll back through the scene in shells.
// Bass is red, the middle green and treble blue, turning slowly with s_speed. Fading in, the
// colours, the push and the dimming all grow from nothing.

vec4 effect(vec2 uv) {
    // The black bars stay black.
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    float d = depth_at(uv);
    float age = (1.0 - d) * s_reach / 100.0;
    vec3 v = vec3(
        texture(u_spectrum, vec2(0.08, age)).r,
        texture(u_spectrum, vec2(0.4, age)).r,
        texture(u_spectrum, vec2(0.75, age)).r
    );
    float amount = s_amount / 100.0 * u_amount;
    float loud = max(v.r, max(v.g, v.b));
    vec2 push = (uv - 0.5) * 0.04 * loud * amount;
    vec3 video = texture(u_video, uv + push).rgb * (1.0 - s_dim / 100.0 * u_amount * (1.0 - loud));
    return vec4(video + hue_rotate(v * v, 0.1 * u_time * s_speed) * amount * 1.2, 1.0);
}
