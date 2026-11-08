// Melt: a test video effect for Flex. Folds the picture into a slowly turning
// kaleidoscope, shifts its colours, and leaves trails that spiral outwards and change
// colour as they fade. Beats in the sound kick the picture outwards and jolt the colours.
//
// Flex declares these for you (see EFFECTS.md):
//   u_video       the video frame          u_prev        this effect's last output
//   u_resolution  size in pixels           u_time        seconds since it started
//   u_frame       frame number             u_video_rect  the picture without black bars
//                                                        (x0, y0, x1, y1), 0-1 from bottom left
//   u_video_time  the playback position (stops when paused)
//   u_level, u_bass, u_mid, u_treble, u_beat             the sound, 0-1 (audio = true in melt.toml)
//   u_spectrum    x: frequency, y: age (0 = now)
//   s_speed, s_segments, s_trails, s_colours, s_react, s_spill   the settings in melt.toml
//
// Edit this file while a video plays: Flex reloads it as soon as it's saved.

const float TAU = 6.2831853;

vec2 rotate(vec2 p, float a) {
    float c = cos(a), s = sin(a);
    return vec2(c * p.x - s * p.y, s * p.x + c * p.y);
}

// Turns a colour around the grey axis.
vec3 hue_shift(vec3 c, float a) {
    const vec3 k = vec3(0.57735);
    float ca = cos(a);
    return c * ca + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - ca);
}

// Mirrors coordinates back into 0-1, so nothing samples off the picture.
vec2 mirror(vec2 x) {
    return 1.0 - abs(1.0 - mod(x, 2.0));
}

// Folds p (centred) into `n` mirrored slices.
vec2 kaleidoscope(vec2 p, float n) {
    if (n < 1.5) return p;
    float slice = TAU / n;
    float a = mod(atan(p.y, p.x), slice);
    a = abs(a - slice * 0.5);
    return length(p) * vec2(cos(a), sin(a));
}

vec4 effect(vec2 uv) {
    // Video time, so pausing freezes it.
    float t = u_video_time * s_speed;
    float react = s_react / 100.0;
    vec2 lo = u_video_rect.xy, size = u_video_rect.zw - u_video_rect.xy;
    float aspect = u_resolution.x / u_resolution.y;

    // Centred, square coordinates (so the kaleidoscope's slices aren't squashed).
    vec2 p = (uv - 0.5) * vec2(aspect, 1.0);
    vec2 k = kaleidoscope(rotate(p, t * 0.1), s_segments);
    // Back to the picture, breathing in and out a little.
    // Breathing in and out a little, and pulled in on each beat.
    float zoom = 1.0 + 0.08 * sin(t * 0.7) - 0.15 * u_beat * react;
    vec2 pic = k / vec2(aspect, 1.0) * zoom + 0.5;
    pic = (pic - lo) / size;
    bool inside = all(greaterThanEqual(uv, lo)) && all(lessThanEqual(uv, lo + size));
    vec3 video = texture(u_video, lo + mirror(pic) * size).rgb;
    if (s_spill < 0.5 && !inside) video = vec3(0.0);
    video = hue_shift(video, (t * 0.3 + u_beat * react * 1.5) * s_colours / 100.0);

    // Last frame, zoomed in a touch and turned, so trails spiral outwards.
    vec2 q = rotate(p, 0.004 * sin(t * 0.5)) * 0.992;
    vec3 prev = texture(u_prev, q / vec2(aspect, 1.0) + 0.5).rgb;
    // Bass makes the trails last longer.
    float keep = min(s_trails / 100.0 + 0.08 * u_bass * react, 0.97);
    prev = hue_shift(prev, 0.06 * s_colours / 100.0) * keep;

    // Bright things leave glowing trails.
    return vec4(max(video, prev), 1.0);
}
