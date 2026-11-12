// Liquid Mercury: a surface from depth with travelling ripples on top, lit and reflecting the
// film through its own bends. Fading in, the bends grow from flat as the metal comes over.

float surface(vec2 uv, float t) {
    float h = depth_at(uv) * 1.3;
    h += s_slosh / 100.0 * 0.06 * (sin(uv.x * 19.0 + t * 1.8) + sin(uv.y * 23.0 - t * 1.4 + uv.x * 7.0) + sin((uv.x - uv.y) * 41.0 + t * 2.6) * 0.5);
    return h;
}

vec4 effect(vec2 uv) {
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    float t = u_video_time + 3.0 * u_bass;
    vec2 e = 2.5 / u_resolution;
    float here = surface(uv, t);
    vec3 n = normalize(vec3(surface(uv - vec2(e.x, 0.0), t) - surface(uv + vec2(e.x, 0.0), t), surface(uv - vec2(0.0, e.y), t) - surface(uv + vec2(0.0, e.y), t), 0.012));
    vec2 q = clamp(uv + n.xy * 0.16 * u_amount, u_video_rect.xy, u_video_rect.zw);
    vec3 film = texture(u_video, uv).rgb;
    vec3 seen = texture(u_video, q).rgb;
    float grey = dot(seen, vec3(0.3, 0.59, 0.11));
    vec3 l = normalize(vec3(-0.4, 0.7, 0.6));
    float spec = pow(max(dot(reflect(-l, n), vec3(0.0, 0.0, 1.0)), 0.0), 60.0);
    float rim = pow(1.0 - n.z, 2.0);
    vec3 chrome = mix(vec3(grey), seen, 0.25) * vec3(0.85, 0.93, 1.05) * (0.7 + 0.8 * rim) + vec3(1.0, 0.97, 0.9) * spec * (1.2 + 2.0 * u_beat);
    chrome += (here - 0.5) * vec3(0.02, 0.05, 0.1);
    return vec4(mix(film, chrome, s_metal / 100.0 * u_amount), 1.0);
}
