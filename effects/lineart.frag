// Line Art: the network's line drawing (u_video, with restyle: dark lines on white) as ink, over
// paper or the real picture's colours (u_original) in flat bands. Fading in, the paper comes over
// the picture as the ink darkens in.

vec4 effect(vec2 uv) {
    vec3 real = texture(u_original, uv).rgb;
    // Until the first drawing comes, u_video is the real picture.
    if (u_has_restyled < 0.5 || uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(real, 1.0);
    float ink = clamp((1.0 - texture(u_video, uv).r) * s_ink / 100.0, 0.0, 1.0) * u_amount;
    // Flat bands: each channel in fewer steps the more it's flattened, lifted a little like
    // printed colour.
    float steps = mix(24.0, 3.0, s_flatten / 100.0);
    vec3 banded = floor(real * steps + 0.5) / steps;
    banded = mix(banded, vec3(1.0), 0.15 * s_flatten / 100.0);
    vec3 paper = vec3(0.97, 0.95, 0.9);
    vec3 base = mix(real, mix(paper, banded, s_colour / 100.0), u_amount);
    return vec4(mix(base, vec3(0.06, 0.05, 0.07), ink), 1.0);
}
