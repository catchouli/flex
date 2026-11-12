// Tiny World: real miniatures look the way they do because so little is in focus. This keeps one
// band of distance sharp, and turns up the colour. Fading in, the blur and the colour come up
// from none.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(0.0, 0.0, 0.0, 1.0);
    float off = abs(depth_at(uv) - s_at / 100.0);
    float radius = smoothstep(s_band / 200.0, s_band / 200.0 + 0.35, off) * 0.016 * s_blur / 100.0 * u_amount;
    vec2 shape = vec2(u_resolution.y / u_resolution.x, 1.0);
    vec3 sum = c;
    if (radius > 0.0002) {
        for (int i = 1; i <= 20; i++) {
            float a = float(i) * 2.3999632;
            sum += texture(u_video, uv + vec2(cos(a), sin(a)) * sqrt(float(i) / 20.0) * radius * shape).rgb;
        }
        sum /= 21.0;
    }
    float grey = dot(sum, vec3(0.3, 0.59, 0.11));
    vec3 toy = vec3(grey) + (sum - grey) * mix(1.0, s_colour / 100.0, u_amount);
    toy = (toy - 0.5) * mix(1.0, 1.12, u_amount) + mix(0.5, 0.56, u_amount);
    return vec4(toy, 1.0);
}
