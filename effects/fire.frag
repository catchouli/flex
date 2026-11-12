// Fire People: heat in the alpha of u_prev. People's bodies feed it below a line that climbs the
// screen, it rises and curls, cooling as it goes, and it's drawn through a fire colour ramp.
// Fading in, the heat that's drawn rises, so the flames grow from nothing to their full height.

vec3 flame(float h) {
    return vec3(smoothstep(0.08, 0.5, h) * 1.25, smoothstep(0.32, 0.8, h) * 0.95, smoothstep(0.72, 1.05, h) * 0.8);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 0.0);
    vec2 p = uv * vec2(u_resolution.x / u_resolution.y, 1.0);
    float t = u_time;
    float height = (uv.y - u_video_rect.y) / max(u_video_rect.w - u_video_rect.y, 1e-3);
    float front = s_climb < 0.5 ? 2.0 : t / s_climb * 1.15;
    float person = smoothstep(0.4, 0.7, person_at(uv));
    float burning = person * (1.0 - smoothstep(front - 0.08, front + 0.02, height + (fbm(p * 7.0) - 0.5) * 0.2));

    vec2 curl = vec2(fbm(p * 8.0 + vec2(0.0, -t * 1.6)) - 0.47, 0.0);
    vec2 from = uv - vec2(curl.x * 0.012, 0.0045 + 0.0015 * s_height / 100.0);
    float heat = u_frame < 2 ? 0.0 : texture(u_prev, from).a;
    // The sound of fire, explosions and angry or exciting music make the flames flare up higher.
    float roar = clamp(u_sound_fire + u_sound_explosions + 0.7 * (u_sound_angry_music + u_sound_exciting_music), 0.0, 1.0) * s_react / 100.0;
    float cooling = mix(0.93, 0.975, clamp(s_height / 200.0, 0.0, 1.0)) * (0.985 + 0.012 * u_bass * s_react / 100.0 + 0.012 * roar);
    heat *= min(cooling, 0.99);
    float tongues = smoothstep(0.42 - 0.1 * roar, 0.7, fbm(p * 14.0 + vec2(0.0, -t * 2.5)));
    heat = max(heat, burning * tongues * (1.0 + 0.5 * u_beat * s_react / 100.0 + 0.4 * roar));

    float char = burning * s_char / 100.0 * u_amount;
    vec3 shown = c * (1.0 - char * 0.75) + vec3(0.25, 0.05, 0.0) * char * smoothstep(0.55, 0.8, fbm(p * 30.0 + t));
    shown += flame(heat * u_amount) * 1.15;
    // Firelight on whatever's at about the same distance as the flames.
    if (s_light > 0.0 && u_frame > 2) {
        float here = depth_at(uv), spill = 0.0;
        for (int i = 0; i < 8; i++) {
            float a = float(i) * 0.7853982;
            vec2 at = uv + vec2(cos(a) * u_resolution.y / u_resolution.x, sin(a)) * 0.13;
            spill += texture(u_prev, at).a * (1.0 - smoothstep(0.05, 0.35, abs(depth_at(at) - here)));
        }
        shown += c * vec3(1.0, 0.5, 0.15) * spill / 8.0 * s_light / 100.0 * 4.0 * u_amount;
    }
    return vec4(shown, heat);
}
