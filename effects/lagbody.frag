// Time-Lagged Body: each point of a person shows what was there a moment ago, longer ago the
// higher up it is. Alpha remembers where people were lately, so the laggy part can hang about
// after they've moved on. Fading in, the lag, ripple and colour split grow from none as the
// lagged body comes through; what's remembered isn't faded.

vec4 effect(vec2 uv) {
    vec3 video = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(video, 0.0);
    float height = (uv.y - u_video_rect.y) / max(u_video_rect.w - u_video_rect.y, 1e-3);
    float t = u_video_time;
    float wave = sin(height * 16.0 - t * 3.0) * s_ripple / 100.0 * 0.012 * (0.3 + height) * (1.0 + u_bass * s_react / 50.0) * u_amount;
    vec2 from = uv + vec2(wave, 0.0);
    float person = smoothstep(0.4, 0.7, person_at(from));
    vec3 now = texture(u_video, clamp(from, u_video_rect.xy, u_video_rect.zw)).rgb;
    if (u_frame < 2) return vec4(video, person);
    float lately = max(person, texture(u_prev, uv).a * 0.96);
    float ago = pow(clamp(height + 0.1, 0.0, 1.0), 1.4) * s_lag / 100.0 * u_amount;
    float split = s_colour / 100.0 * 0.12 * u_amount;
    vec3 lagged = vec3(history_at(from, ago).r, history_at(from, ago + split).g, history_at(from, ago + split * 2.0).b);
    return vec4(mix(video, lagged, smoothstep(0.02, 0.4, lately) * u_amount), lately);
}
