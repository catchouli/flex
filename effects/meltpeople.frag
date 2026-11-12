// Melting People: bodies droop in lobes, and drops leave them to run down the screen. Alpha says
// how much of each point is molten person, which falls a little further each frame. Fading in,
// the bodies sag further and the drops start running; what's molten is still remembered.

vec4 effect(vec2 uv) {
    vec3 video = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(video, 0.0);
    float t = u_video_time;
    float x = uv.x * u_resolution.x / u_resolution.y;
    float person = smoothstep(0.4, 0.7, person_at(uv));
    float lobes = noise(vec2(x * 14.0, t * 0.25)) * (0.5 + 0.5 * sin(uv.y * 12.0 + t));
    vec2 from = uv + vec2(0.0, lobes * 0.09 * s_sag / 100.0 * (1.0 + 0.5 * u_bass) * u_amount);
    vec3 body = texture(u_video, clamp(from, u_video_rect.xy, u_video_rect.zw)).rgb;
    vec3 c = mix(video, body, person);

    float column = floor(uv.x * u_resolution.x / 5.0);
    float height = (uv.y - u_video_rect.y) / max(u_video_rect.w - u_video_rect.y, 1e-3);
    float pooling = 1.0 - smoothstep(s_pool / 100.0 - 0.01, s_pool / 100.0 + 0.01, height + (noise(vec2(x * 30.0, t)) - 0.5) * 0.02);
    // Drips also come to rest on the top of things.
    pooling = max(pooling, s_ledges / 100.0 * smoothstep(0.03, 0.08, depth_at(uv) - depth_at(uv + vec2(0.0, 8.0 / u_resolution.y))));
    float fall = (1.0 + 7.0 * pow(hash(vec2(column, 1.0)), 2.0)) * s_speed / 100.0 * 2.0 * (1.0 - pooling);
    vec4 above = u_frame < 2 ? vec4(0.0) : texture(u_prev, uv + vec2(0.0, fall / u_resolution.y));
    float molten = above.a * s_stay / 100.0;
    // Drops start from people's undersides and from random columns of them.
    float source = person * step(hash(vec2(column, 7.0)), s_drip / 100.0 * 0.5) * step(0.5, noise(vec2(column * 0.1, t * 0.4 + uv.y * 3.0))) * u_amount;
    if (source > molten) {
        molten = source;
        above.rgb = body;
    }
    c = mix(c, above.rgb * (0.82 + 0.25 * (1.0 - pooling)), molten * (1.0 - person) * u_amount);
    vec3 kept = mix(c, above.rgb, molten * u_amount);
    return vec4(person > 0.5 ? c : kept, molten);
}
