// Anime: the picture redrawn by AnimeGANv3 in the style picked (u_video, with restyle), its
// edges inked darker if wanted, over the real picture (u_original). Fading in, the drawing comes up
// over the real picture.

float luma(vec3 c) {
    return dot(c, vec3(0.299, 0.587, 0.114));
}

vec4 effect(vec2 uv) {
    vec3 drawn = texture(u_video, uv).rgb;
    // Until the first drawing comes, u_video is the real picture.
    if (u_has_restyled < 0.5 || uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(drawn, 1.0);
    vec3 real = texture(u_original, uv).rgb;
    // Ink: where the drawing's brightness changes sharply (Sobel), darker the sharper.
    vec2 px = 1.0 / u_resolution;
    float tl = luma(texture(u_video, uv + px * vec2(-1.0, 1.0)).rgb), t = luma(texture(u_video, uv + px * vec2(0.0, 1.0)).rgb), tr = luma(texture(u_video, uv + px * vec2(1.0, 1.0)).rgb);
    float l = luma(texture(u_video, uv + px * vec2(-1.0, 0.0)).rgb), r = luma(texture(u_video, uv + px * vec2(1.0, 0.0)).rgb);
    float bl = luma(texture(u_video, uv + px * vec2(-1.0, -1.0)).rgb), b = luma(texture(u_video, uv + px * vec2(0.0, -1.0)).rgb), br = luma(texture(u_video, uv + px * vec2(1.0, -1.0)).rgb);
    vec2 g = vec2(tr + 2.0 * r + br - tl - 2.0 * l - bl, tl + 2.0 * t + tr - bl - 2.0 * b - br);
    float ink = smoothstep(0.15, 0.6, length(g)) * s_outlines / 100.0;
    vec3 c = mix(drawn, drawn * 0.2, ink);
    c = max(mix(vec3(luma(c)), c, s_colour / 100.0), 0.0);
    return vec4(mix(real, c, s_strength / 100.0 * u_amount), 1.0);
}
