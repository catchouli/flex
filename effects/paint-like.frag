// Paint Like: the picture painted by Magenta's stylization in the style picked (u_video, with
// restyle), over the real picture (u_original), keeping as much of the film's own colours (its
// colour, the painting's lightness) as asked. The network paints far smaller than the screen, so
// the film's own fine detail (finer than the painting's pixels) is laid back over it. Fading in,
// the paint comes up over the real picture.

vec4 effect(vec2 uv) {
    vec3 painted = texture(u_video, uv).rgb;
    // Until the first painting comes, u_video is the real picture.
    if (u_has_restyled < 0.5 || uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(painted, 1.0);
    vec3 real = texture(u_original, uv).rgb;
    const vec3 luma = vec3(0.299, 0.587, 0.114);
    // The film blurred over about one of the painting's pixels: what's left is the detail the
    // painting can't have.
    vec2 rect = u_video_rect.zw - u_video_rect.xy;
    vec2 cell = rect / max(u_restyled_size, vec2(1.0));
    float blurred = 0.0;
    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            blurred += dot(texture(u_original, uv + vec2(x, y) * cell * 0.6).rgb, luma);
        }
    }
    float detail = (dot(real, luma) - blurred / 9.0) * step(1.0, u_restyled_size.x);
    painted = clamp(painted + detail * s_detail / 100.0, 0.0, 1.0);
    vec3 recoloured = clamp(real + vec3(dot(painted, luma) - dot(real, luma)), 0.0, 1.0);
    vec3 c = mix(painted, recoloured, s_own_colours / 100.0);
    return vec4(mix(real, c, s_strength / 100.0 * u_amount), 1.0);
}
