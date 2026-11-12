// Kaleidoscope: every step is a setting. Each point on screen is moved about, folded into a slice,
// and looked up in the film; then the colour is worked on and trails are mixed in. A count of arms
// between two whole numbers blends between them, so it can change smoothly. Fading in, each point
// slides from where it is to where the folds put it, the colour work comes in, and trails build up.

const float TAU = 6.2831853;

vec3 hue_turn(vec3 c, float a) {
    const vec3 k = vec3(0.57735);
    return c * cos(a) + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - cos(a));
}

// Bounces coordinates back into 0-1, so nothing looks off the edge of the film.
vec2 bounce(vec2 x) {
    return 1.0 - abs(1.0 - mod(x, 2.0));
}

vec2 slice(vec2 p, float arms) {
    if (arms < 1.5) return s_reflect > 0.5 ? vec2(p.x, abs(p.y)) : p;
    float wedge = TAU / arms;
    float a = mod(atan(p.y, p.x), wedge);
    if (s_reflect > 0.5) a = abs(a - wedge * 0.5);
    return length(p) * vec2(cos(a), sin(a));
}

vec2 tiled(vec2 p) {
    float size = max(s_tile / 100.0, 0.02);
    if (s_layout > 1.5) {
        vec2 step_by = vec2(1.0, 1.7320508) * size;
        vec2 a = mod(p, step_by) - 0.5 * step_by;
        vec2 b = mod(p - 0.5 * step_by, step_by) - 0.5 * step_by;
        return dot(a, a) < dot(b, b) ? a : b;
    }
    if (s_layout > 0.5) {
        vec2 cell = floor(p / size + 0.5);
        vec2 q = p - cell * size;
        if (mod(cell.x, 2.0) > 0.5) q.x = -q.x;
        if (mod(cell.y, 2.0) > 0.5) q.y = -q.y;
        return q;
    }
    return p;
}

vec3 look(vec2 uv, vec2 p, float arms, float zoom, float roam, float aspect) {
    vec2 q = slice(p, arms);
    int folds = int(s_folds + 0.5);
    for (int i = 1; i < folds; i++) {
        q = rotate(q - vec2(s_fold_shift / 100.0 * 0.5, 0.0), radians(s_fold_turn));
        q = slice(q, arms);
    }
    // A slice starts at the middle of the film and points one way, so it would only ever see
    // that side. Moving it back by a bit centres it on the middle of the picture instead.
    q = rotate((q - vec2(0.32 * min(length(p) + 0.35, 1.0), 0.0)) / zoom, radians(s_look_turn));
    q += vec2(s_pan_x, s_pan_y) / 100.0 * 0.5;
    q += vec2(sin(roam * 0.31) * 0.14, sin(roam * 0.23 + 1.0) * 0.07 - 0.03);
    vec2 size = u_video_rect.zw - u_video_rect.xy;
    vec2 picture = bounce(q / vec2(aspect * size.y / size.x, 1.0) / size.y + 0.5);
    if (s_relief > 0.0) {
        // Near things bulge out of the pattern.
        q *= 1.0 - (depth_at(u_video_rect.xy + picture * size) - 0.5) * s_relief / 100.0 * 0.6;
        picture = bounce(q / vec2(aspect * size.y / size.x, 1.0) / size.y + 0.5);
    }
    return texture(u_video, mix(uv, u_video_rect.xy + picture * size, u_amount)).rgb;
}

vec4 effect(vec2 uv) {
    float t = u_video_time;
    float aspect = u_resolution.x / u_resolution.y;
    vec2 to_square = vec2(aspect, 1.0);
    vec2 middle = vec2(0.5) + vec2(s_centre_x, s_centre_y) / 100.0 * 0.5;
    middle += vec2(cos(t * 0.37), sin(t * 0.29)) * 0.3 * s_orbit / 100.0;
    vec2 p = (uv - middle) * to_square;

    float r = length(p);
    p = rotate(p, radians(s_turn) + t * 0.25 * s_spin / 100.0 + s_twist / 100.0 * r * 2.0);
    if (s_swirl > 0.0) {
        p = rotate(p, s_swirl / 100.0 * 0.6 * sin(r * 14.0 - t * 1.7));
    }
    if (s_rings > 0.0) {
        float along = log(r + 1e-4) / log(2.0) - t * 0.25 * s_rush / 100.0;
        float repeated = pow(2.0, abs(2.0 * fract(along) - 1.0)) * 0.5;
        p *= mix(1.0, repeated / max(r, 1e-4), s_rings / 100.0);
    }
    p = tiled(p);

    float kick = s_kick / 100.0, pulse = s_pulse / 100.0;
    float zoom = s_zoom / 100.0 * (1.0 + s_breathe / 100.0 * 0.35 * sin(t * 0.7));
    zoom *= 1.0 + 0.5 * u_bass * pulse + 0.35 * u_beat * kick;
    float roam = t * s_roam / 100.0;

    float arms = max(s_arms + s_bass_arms * u_bass, 1.0);
    float whole = floor(arms);
    float between = smoothstep(0.3, 0.7, arms - whole);
    vec3 c = look(uv, p, whole, zoom, roam, aspect);
    if (between > 0.0) c = mix(c, look(uv, p, whole + 1.0, zoom, roam, aspect), between);

    // Blended towards the turned colour: the angle keeps growing with time, so scaling it would
    // turn the colours somewhere arbitrary on the way in.
    c = mix(c, hue_turn(c, radians(s_hue) + t * 0.5 * s_flow / 100.0 + s_rainbow / 100.0 * r * 6.0 + u_beat * kick), u_amount);
    float grey = dot(c, vec3(0.3, 0.59, 0.11));
    c = mix(vec3(grey), c, mix(1.0, s_saturation / 100.0, u_amount));
    c = (c - 0.5) * mix(1.0, s_contrast / 100.0, u_amount) + 0.5;
    if (s_layers >= 2.0) c = mix(c, floor(clamp(c, 0.0, 1.0) * s_layers + 0.5) / s_layers, u_amount);
    c = clamp(c, 0.0, 4.0);
    c *= 1.0 - s_vignette / 100.0 * smoothstep(0.25, 0.9, r) * u_amount;

    bool inside = uv == clamp(uv, u_video_rect.xy, u_video_rect.zw);
    if (s_fill < 0.5 && !inside) c = vec3(0.0);

    if (s_trails > 0.0 && u_frame > 1) {
        vec2 q = (uv - 0.5) * to_square;
        q = rotate(q, 0.001 * s_trail_turn) * (1.0 - 0.001 * s_trail_zoom);
        vec3 past = texture(u_prev, q / to_square + 0.5).rgb;
        past = hue_turn(past, 0.002 * s_trail_colour) * s_trails / 100.0 * u_amount;
        c = max(c, past);
    }
    return vec4(c, 1.0);
}
