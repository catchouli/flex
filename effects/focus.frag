// Focus Pull: blurs each point by how far it is from the plane in focus, with bright points
// counting for more so they spread into discs, as they do through a real lens. Fading in, the blur
// grows from none.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(0.0, 0.0, 0.0, 1.0);
    float t = u_time * s_speed / 100.0;
    float focus = 0.5 + 0.42 * sin(t * 0.4);
    if (s_on > 2.5) focus = 0.1;
    else if (s_on > 1.5) focus = 0.95;
    else if (s_on > 0.5 && u_face_count > 0) focus = mix(focus, depth_at(face_point(0, 2)), u_face_seen[0]);
    float here = depth_at(uv);
    float radius = min(abs(here - focus) * 0.05 * s_blur / 100.0, 0.05) * u_amount;
    if (radius < 0.0006) return vec4(c, 1.0);
    vec2 shape = vec2(u_resolution.y / u_resolution.x, 1.0);
    vec3 sum = c;
    float total = 1.0;
    for (int i = 1; i <= 32; i++) {
        float a = float(i) * 2.3999632;
        vec2 at = uv + vec2(cos(a), sin(a)) * sqrt(float(i) / 32.0) * radius * shape;
        float there = depth_at(at);
        // Something sharp mustn't leak into the blur around it.
        float own = min(abs(there - focus) * 0.05 * s_blur / 100.0, 0.05) * u_amount;
        if (there > here && own < radius * 0.5) continue;
        vec3 s = texture(u_video, at).rgb;
        float weight = 1.0 + dot(s, s) * dot(s, s) * 4.0 * s_sparkle / 100.0;
        sum += s * weight;
        total += weight;
    }
    return vec4(sum / total, 1.0);
}
