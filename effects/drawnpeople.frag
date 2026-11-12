// Drawn People: people redrawn by AnimeGANv3 (u_video, with restyle), the rest real (u_original),
// with ink along people's outlines if wanted. Fading in, the drawing comes up over people and the
// ink darkens.

vec4 effect(vec2 uv) {
    vec3 drawn = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(drawn, 1.0);
    vec3 real = texture(u_original, uv).rgb;
    float m = person_at(uv);
    float soft = 0.03 + s_edge / 100.0 * 0.3;
    float person = smoothstep(0.5 - soft, 0.5 + soft, m);
    float amount = person * s_strength / 100.0 * u_amount;
    vec3 c = mix(real, drawn, amount);
    // Ink where the mask crosses a half, about 3 pixels wide.
    vec2 px = 3.0 / u_resolution;
    float across = abs(person_at(uv + vec2(px.x, 0.0)) - person_at(uv - vec2(px.x, 0.0)))
                 + abs(person_at(uv + vec2(0.0, px.y)) - person_at(uv - vec2(0.0, px.y)));
    float ink = smoothstep(0.25, 0.7, across) * (1.0 - smoothstep(0.15, 0.45, abs(m - 0.5))) * s_outline / 100.0 * u_amount;
    // Until the first drawing comes, just the real picture.
    if (u_has_restyled < 0.5) ink = 0.0;
    return vec4(mix(c, vec3(0.05, 0.04, 0.06), ink), 1.0);
}
