// Living Textures: the texture grown over the film (u_video, with restyle: a living texture
// picked as the style), shaded by the film's own light (u_original) so its shapes still show.
// Fading in, the texture grows over the real picture with the strength.

vec4 effect(vec2 uv) {
    vec3 grown = texture(u_video, uv).rgb;
    // Until the textures are here, u_video is the real picture.
    if (u_has_restyled < 0.5 || uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(grown, 1.0);
    vec3 real = texture(u_original, uv).rgb;
    float light = dot(real, vec3(0.299, 0.587, 0.114));
    vec3 c = grown * mix(1.0, 0.25 + 1.6 * light, s_shading / 100.0);
    return vec4(mix(real, c, s_strength / 100.0 * u_amount), 1.0);
}
