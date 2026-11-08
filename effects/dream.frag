// Dream: a DeepDream-like test effect for Flex, without a neural network. Each frame
// starts from the last one and:
//   - pushes it along the outlines of shapes, so edges curl into tendrils (swirling)
//   - pushes it away from dark into bright, so bright patches bulge (blossoming)
//   - sharpens it, so small details get amplified frame after frame (detail)
//   - turns its colours along the swirls (colours)
// and blends a little of the real video back in, so the dream keeps following it.
// It keeps dreaming while paused. See melt.frag or EFFECTS.md for the uniforms.

float lum(vec3 c) {
    return dot(c, vec3(0.299, 0.587, 0.114));
}

vec3 hue_shift(vec3 c, float a) {
    const vec3 k = vec3(0.57735);
    float ca = cos(a);
    return c * ca + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - ca);
}

vec3 prev_at(vec2 uv) {
    return texture(u_prev, uv).rgb;
}

vec4 effect(vec2 uv) {
    vec3 video = texture(u_video, uv).rgb;
    if (u_frame < 2) return vec4(video, 1.0);

    // Pixel steps scale with the picture so it looks alike at any resolution.
    vec2 px = vec2(1.0) / u_resolution * (u_resolution.y / 540.0);
    float react = s_react / 100.0;
    // Sound nudges the swirling along; kept gentle, since big jumps flash.
    float surge = 1.0 + 0.3 * u_beat * react + 0.2 * u_bass * react;

    // Brightness slope of the last frame, a few pixels across (blurrier = bigger swirls).
    float r = 3.0;
    float l = lum(prev_at(uv - vec2(r, 0.0) * px)), rt = lum(prev_at(uv + vec2(r, 0.0) * px));
    float d = lum(prev_at(uv - vec2(0.0, r) * px)), up = lum(prev_at(uv + vec2(0.0, r) * px));
    vec2 grad = vec2(rt - l, up - d);
    vec2 along = vec2(grad.y, -grad.x);

    // Where this pixel's colour comes from: along edges, and from darker towards brighter.
    vec2 flow = (along * s_swirl / 100.0 * 1.5 - grad * s_grow / 100.0) * 12.0 * surge;
    vec2 src = uv - flow * px * 4.0;
    // Drifting very slowly inwards, as if falling into it.
    src = (src - 0.5) * 0.999 + 0.5;

    // Sharpen what's carried over: each pass makes details a little stronger.
    vec3 p = prev_at(src);
    vec3 blur = 0.25 * (prev_at(src + vec2(2.0, 0.0) * px) + prev_at(src - vec2(2.0, 0.0) * px)
                      + prev_at(src + vec2(0.0, 2.0) * px) + prev_at(src - vec2(0.0, 2.0) * px));
    p += (p - blur) * s_detail / 100.0 * 0.5;

    // Colour turns where the swirls are strongest, and saturates a touch.
    float swirl = clamp(length(grad) * 8.0, 0.0, 1.0);
    p = hue_shift(p, swirl * s_colours / 100.0 * 0.15);
    p = mix(vec3(lum(p)), p, 1.0 + 0.03 * s_colours / 100.0);

    float keep = clamp(s_dream / 100.0, 0.0, 0.97);
    return vec4(clamp(mix(video, p, keep), 0.0, 1.0), 1.0);
}
