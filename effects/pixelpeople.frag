// Pixel People: both the picture and the outline are read once per block, so even the edges of
// people are stepped. Fading in, the blocks grow from single pixels and the colours step down.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 1.0);
    float size = mix(1.0, s_size * (1.0 + 0.8 * u_beat * s_react / 100.0), u_amount) * u_resolution.y / 720.0;
    vec2 block = (floor(uv * u_resolution / size) + 0.5) * size / u_resolution;
    if (person_at(block) < 0.5 || u_amount <= 0.0) return vec4(c, 1.0);
    vec3 pixel = texture(u_video, block).rgb;
    if (s_colours >= 2.0) pixel = mix(pixel, floor(pixel * s_colours + 0.5) / s_colours, u_amount);
    return vec4(pixel, 1.0);
}
