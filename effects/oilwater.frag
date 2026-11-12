// Oil on Water: colours from light interfering in a thin film whose thickness flows about and
// piles up around people, as an oil slick's bands bunch round a stone. Fading in, the slick's
// colours come up from nothing.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 1.0);
    vec2 p = uv * vec2(u_resolution.x / u_resolution.y, 1.0);
    float t = u_time * s_speed / 100.0;
    vec2 warp = vec2(fbm(p * 1.7 + vec2(t * 0.05, 0.0)), fbm(p * 1.7 + vec2(4.1, t * 0.04)));
    float film = fbm(p * 1.3 + warp * 2.4 + vec2(0.0, t * 0.02));
    float person = person_at(uv);
    film += person * 0.35;
    film += u_bass * 0.04;
    film += depth_at(uv) * s_surfaces / 100.0 * 1.6;

    float order = film * 7.0 * s_bands / 100.0;
    vec3 colour = 0.5 + 0.5 * cos(6.2831853 * order * vec3(1.0, 1.22, 1.45));
    float shine = 0.35 + 0.65 * (1.0 - dot(c, vec3(0.33)));
    float amount = s_strength / 100.0 * u_amount * shine * (1.0 - smoothstep(0.3, 0.7, person) * s_people / 100.0);
    return vec4(c + (colour - 0.5) * amount * (0.4 + dot(c, vec3(0.5))), 1.0);
}
