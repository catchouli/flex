// Smoke People: noise eats holes in people from the outside in, showing what's probably behind
// them, and what's eaten goes into a smoke layer in the alpha of u_prev that rises and curls.
// Fading in, the holes eat further in and the smoke thickens from none.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 0.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float t = u_time;
    float mask = person_at(uv);
    float person = smoothstep(0.35, 0.7, mask);
    // How far inside someone this is, roughly.
    float inner = 0.25 * (person_at(uv + vec2(0.05, 0.0) / aspect) + person_at(uv - vec2(0.05, 0.0) / aspect) + person_at(uv + vec2(0.0, 0.05)) + person_at(uv - vec2(0.0, 0.05)));
    float grain = fbm(p * 9.0 + vec2(0.0, -t * 0.5));
    float eaten = person * smoothstep(0.32, 0.62, grain + (1.0 - inner) * 0.5 * s_holes / 50.0 - 0.28 + u_bass * s_react / 400.0 - (1.0 - u_amount) * 0.6) * step(0.001, s_holes) * u_amount;

    vec2 curl = vec2(fbm(p * 6.0 + vec2(0.0, -t * 0.4)), fbm(p * 6.0 + vec2(5.2, -t * 0.3))) - 0.47;
    vec2 from = uv - vec2(s_wind / 100.0 * 0.0025, 0.0028) - curl * 0.006;
    float smoke = u_frame < 2 ? 0.0 : texture(u_prev, from).a * 0.975;
    smoke = max(smoke, (eaten + person * (1.0 - inner) * 0.8) * s_smoke / 100.0 * u_amount);

    vec3 shown = c;
    if (eaten > 0.0) shown = mix(c, plate_at(uv), eaten);
    float solid = person * (1.0 - eaten);
    vec3 ash = mix(vec3(0.62, 0.6, 0.58), vec3(0.9, 0.86, 0.8), grain);
    shown = mix(shown, ash, smoothstep(0.03, 0.8, smoke) * (1.0 - solid) * 0.85 * u_amount);
    return vec4(shown, smoke);
}
