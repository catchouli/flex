// Aura: light around people, kept in the alpha of u_prev and carried up and outwards each frame,
// so it flows off them. People themselves are left alone. With MoGe-2's head it can stream off
// animals instead, or both. Fading in, the people start giving off light and it builds up as it
// flows away from them.
//
// s_who: 0 people, 1 animals, 2 people and animals.

float glowing_at(vec2 uv) {
    if (s_who < 0.5) return person_at(uv);
    if (s_who < 1.5) return animal_at(uv);
    return max(person_at(uv), animal_at(uv));
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 0.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float t = u_time;
    float person = glowing_at(uv);
    float reach = 0.03 * s_size / 100.0 * (1.0 + u_bass * s_react / 50.0);
    float near = 0.0, source = 0.0;
    for (int i = 0; i < 12; i++) {
        float a = float(i) * 0.5235988;
        vec2 at = uv + vec2(cos(a), sin(a)) * reach / aspect;
        float there = glowing_at(at);
        near += there;
        if (there > 0.5) source = max(source, depth_at(at));
    }
    near /= 12.0;
    vec2 wander = vec2(fbm(p * 5.0 + vec2(0.0, -t * 0.7)), fbm(p * 5.0 + vec2(6.0, -t * 0.6))) - 0.47;
    vec2 from = uv - (vec2(0.0, 0.003) + wander * 0.008) * s_size / 100.0;
    float kept = u_frame < 2 ? 0.0 : texture(u_prev, from).a;
    float field = max(near * 1.1 * u_amount, kept * (0.955 + 0.03 * u_bass * s_react / 100.0));
    vec3 colour = hue(s_colours / 360.0 + s_spread / 100.0 * (field * 0.6 + wander.x * 1.5 + t * 0.05));
    float glow = smoothstep(0.05, 0.9, field) * (1.0 + 1.2 * u_beat * s_react / 100.0) * u_amount;
    // It's behind them, so anything nearer than the person hides it.
    if (source > 0.0) glow *= 1.0 - smoothstep(source + 0.02, source + 0.1, depth_at(uv));
    float solid = smoothstep(0.4, 0.7, person);
    return vec4(c + colour * glow * (1.0 - solid) * 0.95, field);
}
