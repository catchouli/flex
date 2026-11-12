// Slow Burn: everything's driven by how long the frame on screen has been in its shot. Fading in,
// it builds as if the shot had gone on longer.

vec3 turn(vec3 c, float a) {
    const vec3 k = vec3(0.57735);
    return c * cos(a) + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - cos(a));
}

vec4 effect(vec2 uv) {
    float built = u_since_cut > 1e5 ? min(u_time / s_time, 1.0) : min(u_since_cut / s_time, 1.0);
    built = built * built * (3.0 - 2.0 * built) * u_amount;
    vec2 p = uv * vec2(u_resolution.x / u_resolution.y, 1.0);
    float t = u_video_time;
    vec2 flow = vec2(fbm(p * 3.0 + vec2(t * 0.2, 0.0)), fbm(p * 3.0 + vec2(5.0, t * 0.17))) - 0.47;
    vec2 at = clamp(uv + flow * 0.14 * built * s_warp / 100.0, u_video_rect.xy, u_video_rect.zw);
    vec3 c = texture(u_video, at).rgb;
    float grey = dot(c, vec3(0.3, 0.59, 0.11));
    c = mix(c, vec3(grey) + (c - grey) * 2.4, built * s_colour / 100.0);
    c = turn(c, built * s_colour / 100.0 * 2.5 * (fbm(p * 2.0 + t * 0.1) - 0.2));
    return vec4(c + vec3(1.0) * u_cut * s_flash / 100.0 * u_amount, 1.0);
}
