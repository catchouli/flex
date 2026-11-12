// Undertow: feedback weighed by depth. Each frame the last one is pushed along a swirling
// flow, shifted in colour and blended under the video, but only where depth lets it. Fading in,
// more and more of the last frame is kept.

float noise_sin(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    vec2 k = vec2(127.1, 311.7);
    float a = fract(sin(dot(i, k)) * 43758.5453);
    float b = fract(sin(dot(i + vec2(1.0, 0.0), k)) * 43758.5453);
    float c = fract(sin(dot(i + vec2(0.0, 1.0), k)) * 43758.5453);
    float d = fract(sin(dot(i + vec2(1.0, 1.0), k)) * 43758.5453);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

vec4 effect(vec2 uv) {
    // The black bars stay black.
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    float d = depth_at(uv);
    float m = smoothstep(s_edge / 100.0, min(s_edge / 100.0 + 0.3, 1.0), s_where < 0.5 ? 1.0 - d : d);
    float kick = 1.0 + 2.0 * u_beat * s_react / 100.0;
    float t = u_time;
    float a = 6.2831853 * 2.0 * noise_sin(uv * vec2(3.0, 2.0) + vec2(0.05 * t, -0.03 * t));
    vec2 swirl = vec2(cos(a), sin(a)) * 0.006 * s_flow / 50.0 * kick;
    vec2 inward = (vec2(0.5) - uv) * 0.006 * s_flow / 50.0;
    vec2 q = uv + (swirl + inward) * m;
    // A touch of blur, or the small steps build up into a combed pattern.
    vec2 e = 0.75 / u_resolution;
    vec3 prev = 0.25 * (texture(u_prev, q + e).rgb + texture(u_prev, q - e).rgb
        + texture(u_prev, q + vec2(e.x, -e.y)).rgb + texture(u_prev, q - vec2(e.x, -e.y)).rgb);
    prev = hue_rotate(prev, 0.08 * s_colour / 50.0);
    vec3 video = texture(u_video, uv).rgb;
    return vec4(mix(video, prev, m * s_trails / 100.0 * u_amount), 1.0);
}
