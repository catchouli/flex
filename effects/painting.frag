// Painting: the picture repainted by fast neural style in the style picked (u_video, with
// restyle), over the real picture (u_original). Fading in, the paint comes up over the real
// picture.

vec4 effect(vec2 uv) {
    vec3 painted = texture(u_video, uv).rgb;
    // Until the first painting comes, u_video is the real picture.
    if (u_has_restyled < 0.5 || uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(painted, 1.0);
    vec3 real = texture(u_original, uv).rgb;
    float grey = dot(painted, vec3(0.299, 0.587, 0.114));
    vec3 c = max(mix(vec3(grey), painted, s_colour / 100.0), 0.0);
    return vec4(mix(real, c, s_strength / 100.0 * u_amount), 1.0);
}
