// Strobe Sculpture: alpha holds the statues, with which half-second each was made in the whole
// part and its brightness in the fraction. It has to be alpha, as the colours are in use showing
// the film, and people are drawn over their own statues. Fading in, the statues come up out of
// the picture.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (u_frame < 2 || uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 0.0);
    float t = u_video_time;
    float now = mod(floor(t * 2.0), 30.0);
    float kept = texelFetch(u_prev, ivec2(gl_FragCoord.xy) - ivec2(0, int(s_rise + 0.5)), 0).a;
    float person = person_at(uv);
    float rate = s_every / 100.0;
    bool stamp = u_beat > 0.9 || (rate > 0.0 && fract(t * rate) < 0.03);
    if (stamp && person > 0.55) {
        kept = now + 1.0 + 0.98 * clamp(dot(c, vec3(0.3, 0.59, 0.11)), 0.0, 1.0);
    }
    if (kept >= 1.0) {
        float made = floor(kept) - 1.0;
        float age = mod(now - made, 30.0) * 0.5;
        float left = 1.0 - smoothstep(s_fade * 0.5, s_fade, age);
        if (left <= 0.0) {
            kept = 0.0;
        } else {
            float bright = fract(kept) / 0.98;
            vec3 statue = mix(vec3(bright), hue(made * 0.13 + bright * 0.4) * (0.25 + bright * 1.5), s_colour / 100.0);
            c = mix(c, statue, left * 0.9 * u_amount);
        }
    }
    vec3 live = texture(u_video, uv).rgb;
    return vec4(mix(c, live, smoothstep(0.45, 0.75, person)), kept);
}
