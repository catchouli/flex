// Reaction-Diffusion Skin: living animal markings, kept in the alpha of u_prev. Each point is
// pushed up by what's close around it and down by what's further out, which settles by itself
// into stripes and spots. Depth shapes them: they're bigger up close, squashed on surfaces that
// turn away, don't cross the edges of things, and line up along the depth of the room. They ride
// along on whatever's moving, and are lit as if raised from the surface. Fading in, the markings
// come up on the picture; they go on growing underneath at every amount.

// An average of the pattern around `uv`, flattened along `slant` where the surface turns away,
// and counting only what's on the same surface. `along` pulls the samples towards a line, which
// makes stripes form across it.
float around(vec2 uv, float radius, float turn, vec2 slant, float near, vec3 along) {
    float sum = 0.0, total = 0.0;
    for (int i = 0; i < 8; i++) {
        float a = float(i) * 0.7853982 + turn;
        vec2 v = vec2(cos(a), sin(a));
        v -= slant * dot(v, slant);
        v -= (v - along.xy * dot(v, along.xy)) * along.z;
        vec2 at = uv + v * radius / u_resolution;
        float same = exp(-abs(depth_at(at) - near) * 28.0) + 0.02;
        sum += texture(u_prev, at).a * same;
        total += same;
    }
    return sum / total;
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    float near = depth_at(uv);
    float pattern;
    if (u_frame < 3 || u_cut > 0.85) {
        pattern = noise(uv * u_resolution / (s_size * u_resolution.y / 720.0 * 1.7));
    } else {
        vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
        float e = 0.008;
        vec2 slope = vec2(depth_at(uv + vec2(e / aspect.x, 0.0)) - depth_at(uv - vec2(e / aspect.x, 0.0)), depth_at(uv + vec2(0.0, e)) - depth_at(uv - vec2(0.0, e))) / (2.0 * e);
        // How far the surface turns away, as the direction to squash the pattern along.
        vec2 slant = normalize(slope + 1e-5) * clamp(length(slope) * 0.45, 0.0, 0.8);

        float size = s_size * u_resolution.y / 720.0 * mix(1.0, 0.5 + 1.25 * near, s_scale / 100.0);
        vec2 from = uv - flow_at(uv) * u_dt;
        float here = texture(u_prev, from).a;
        // Holding back only up and down the depth lays the stripes along it, like contour lines.
        vec3 along = vec3(normalize(slope + 1e-5), s_depth / 100.0 * 0.97 * smoothstep(0.02, 0.3, length(slope)));
        float close = 0.5 * (here + around(from, size * 0.6, 0.3, slant, near, vec3(1.0, 0.0, 0.0)));
        float far = 0.5 * (around(from, size * 1.5, 0.0, slant, near, along) + around(from, size * 2.4, 0.4, slant, near, along));
        float luma = dot(c, vec3(0.3, 0.59, 0.11));
        float contour = sin(near * 6.2831853 * s_bands);
        float push = (close - far) * 7.0 + (luma - 0.45) * 0.45 * s_follow / 100.0 + contour * 1.3 * s_depth / 100.0 + (u_bass - 0.3) * 0.03;
        pattern = clamp(here + clamp(push, -1.0, 1.0) * 0.07, 0.0, 1.0);
    }
    float a = smoothstep(0.35, 0.65, pattern);
    vec3 ink;
    if (s_colours < 0.5) {
        ink = mix(c * 0.2, c * 1.5 + 0.05, a);
    } else if (s_colours < 1.5) {
        ink = mix(vec3(0.7, 0.12, 0.25), vec3(1.0, 0.72, 0.45), a) * (0.5 + 0.8 * c);
    } else {
        ink = vec3(a) * (0.75 + 0.3 * c);
    }
    // Lit from above and to the left, as if the markings stood up off the surface.
    vec2 px = 1.5 / u_resolution;
    vec2 rise = vec2(texture(u_prev, uv + vec2(px.x, 0.0)).a - texture(u_prev, uv - vec2(px.x, 0.0)).a, texture(u_prev, uv + vec2(0.0, px.y)).a - texture(u_prev, uv - vec2(0.0, px.y)).a);
    ink *= 1.0 + dot(rise, vec2(0.6, -0.8)) * 1.6 * s_relief / 100.0;
    vec3 shown = mix(c, ink, s_strength / 100.0 * u_amount);
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) shown = vec3(0.0);
    return vec4(shown, pattern);
}
