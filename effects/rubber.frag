// Rubber Sheet: a mesh made in pixel space. There's a grid of vertices over the picture, each
// raised by the depth under it and moved by its own wobble; between them heights blend, as
// across a mesh's faces. Each pixel walks along its view ray until it goes under that surface,
// then shows the part of the film pinned there. Where depth jumps, the sheet stretches. Fading
// in, the wobble, the view's circling and the zoom past the edge all grow from none.
//
// s_style: 0 waves, 1 ripples, 2 facets, 3 wireframe.

const int STEPS = 40;

// How much of the picture's edge to keep out of view regardless, as a fraction of its size.
const float EDGE = 0.012;

// The furthest the wobble and the view's circling can move a point on screen.
float reach() {
    float push = s_style < 0.5 ? 0.0625 : 0.018;
    return push * s_wobble / 100.0 * (1.0 + 2.0 * s_react / 100.0) + 0.1 * s_sway / 100.0;
}

vec2 cells() {
    vec2 size = u_video_rect.zw - u_video_rect.xy;
    float across = floor(s_cells);
    return vec2(across, max(1.0, floor(across * size.y * u_resolution.y / (size.x * u_resolution.x))));
}

vec2 to_picture(vec2 q) {
    return (q - u_video_rect.xy) / (u_video_rect.zw - u_video_rect.xy);
}

float hash(vec2 v, float k) {
    return fract(sin(dot(v, vec2(127.1 + k, 311.7 - k))) * 43758.5453);
}

vec2 vertex_at(vec2 v, vec2 grid) {
    return u_video_rect.xy + (v / grid) * (u_video_rect.zw - u_video_rect.xy);
}

// How high a vertex is: the depth under it, plus a wave in the styles that have one.
float height(vec2 v, vec2 grid, float t, float amount) {
    float wave = 0.0;
    if (s_style > 1.5 && s_style < 2.5) {
        wave = 0.6 * sin(t * (1.5 + 2.0 * hash(v, 3.0)) + 6.2831853 * hash(v, 0.0));
    } else if (s_style > 0.5 && s_style < 1.5) {
        wave = sin(length((v / grid - 0.5) * vec2(grid.x / grid.y, 1.0)) * 26.0 - t * 4.0);
    }
    return depth_at(vertex_at(v, grid)) + wave * 0.35 * amount;
}

// Depth averaged over a patch, so the waves bend around people instead of tearing them
// away from what's behind them.
float soft_depth(vec2 at) {
    float sum = 0.0;
    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            sum += depth_at(at + vec2(float(x), float(y)) * 0.08);
        }
    }
    return sum / 9.0;
}

// How far a vertex moves sideways.
vec2 sway(vec2 v, vec2 grid, float t, float amount) {
    if (s_style > 1.5 && s_style < 2.5) {
        float phase = 6.2831853 * hash(v, 0.0);
        return vec2(cos(t * 1.7 + phase), sin(t * 1.3 + 2.0 * phase)) * 0.018 * amount;
    }
    if (s_style > 0.5 && s_style < 1.5) {
        vec2 p = (v / grid - 0.5) * vec2(grid.x / grid.y, 1.0);
        float r = length(p);
        return p / max(r, 0.001) * cos(r * 26.0 - t * 4.0) * 0.018 * amount;
    }
    // Sine waves rolling through the world, so neighbouring vertices move together and the
    // scene ripples rather than cracks. The world position comes from depth.
    vec2 p = v / grid - 0.5;
    // Depth is a little different every frame, so it only nudges the waves or they'd shiver.
    float dist = 1.6 - 0.8 * soft_depth(vertex_at(v, grid));
    vec3 w = vec3(p.x * (grid.x / grid.y) * dist, p.y * dist, dist);
    return vec2(
        0.6 * sin(1.3 * t + 2.1 * w.x + 1.7 * w.z) + 0.4 * sin(0.9 * t - 1.4 * w.y + 2.6 * w.x),
        0.6 * sin(1.1 * t + 1.9 * w.z - 1.5 * w.y) + 0.4 * sin(1.7 * t + 2.3 * w.y + 1.2 * w.x)
    ) * 0.0625 * amount;
}

float height_at(vec2 q, vec2 grid, float t, float amount) {
    vec2 g = to_picture(q) * grid;
    vec2 v = floor(g);
    vec2 f = g - v;
    return mix(
        mix(height(v, grid, t, amount), height(v + vec2(1.0, 0.0), grid, t, amount), f.x),
        mix(height(v + vec2(0.0, 1.0), grid, t, amount), height(v + vec2(1.0, 1.0), grid, t, amount), f.x),
        f.y
    );
}

vec2 sway_at(vec2 q, vec2 grid, float t, float amount) {
    vec2 g = to_picture(q) * grid;
    vec2 v = floor(g);
    vec2 f = g - v;
    return mix(
        mix(sway(v, grid, t, amount), sway(v + vec2(1.0, 0.0), grid, t, amount), f.x),
        mix(sway(v + vec2(0.0, 1.0), grid, t, amount), sway(v + vec2(1.0, 1.0), grid, t, amount), f.x),
        f.y
    );
}

// Mirrors past the picture's edge, so a tilted view never shows a hole.
vec2 fit(vec2 q) {
    vec2 size = u_video_rect.zw - u_video_rect.xy;
    vec2 a = mod((q - u_video_rect.xy) / size, 2.0);
    return u_video_rect.xy + size * (1.0 - abs(1.0 - a));
}

vec4 effect(vec2 uv) {
    // The black bars stay black.
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }

    float t = u_video_time * s_speed;
    float amount = s_wobble / 100.0 * (1.0 + (u_beat + u_bass) * s_react / 100.0) * u_amount;
    vec2 grid = cells();
    vec2 centre = 0.5 * (u_video_rect.xy + u_video_rect.zw);

    // Zoomed in just past as far as the wobble can reach, so the picture's edge is never pulled
    // into view: it's often dark, from the film's frame or a sliver of black bar.
    vec2 size = u_video_rect.zw - u_video_rect.xy;
    uv = centre + (uv - centre) * (1.0 - 2.0 * (reach() / size + EDGE) * u_amount);

    // The view circles, so there's always some angle on the mesh, and leans in a little.
    float angle = 0.45 * t;
    vec2 tilt = s_sway / 100.0 * u_amount * vec2(0.16 * cos(angle), 0.11 * sin(angle));
    float lean = 0.22 * s_sway / 100.0 * u_amount;
    float top = 1.0 + 0.35 * amount;
    float bottom = -0.35 * amount;
    float span = top - bottom;

    float z = top;
    vec2 q = uv;
    bool hit = false;
    float above = top;
    for (int i = 0; i <= STEPS; i++) {
        z = top - span * float(i) / float(STEPS);
        vec2 c = fit(uv + (tilt + (uv - centre) * lean) * (z - 0.5));
        if (height_at(c, grid, t, amount) >= z) {
            hit = true;
            break;
        }
        above = z;
    }
    if (!hit) {
        z = bottom;
        above = bottom;
    }
    for (int j = 0; j < 5; j++) {
        float middle = 0.5 * (z + above);
        vec2 c = fit(uv + (tilt + (uv - centre) * lean) * (middle - 0.5));
        if (height_at(c, grid, t, amount) >= middle) {
            z = middle;
        } else {
            above = middle;
        }
    }
    q = fit(uv + (tilt + (uv - centre) * lean) * (z - 0.5));

    vec2 moved = fit(q + sway_at(q, grid, t, amount));
    vec3 video = texture(u_video, moved).rgb;

    // Light from the upper left, so the wobble reads on flat walls too.
    vec2 e = (u_video_rect.zw - u_video_rect.xy) / grid;
    float here = height_at(q, grid, t, amount);
    float slope = height_at(q + vec2(e.x, 0.0), grid, t, amount) - here + height_at(q + vec2(0.0, e.y), grid, t, amount) - here;
    // Waves stay unlit: they only move sideways, so all the light could show is depth's hard edges.
    vec3 c = s_style < 0.5 ? video : video * (1.0 + clamp(slope * 2.5, -0.3, 0.35) * u_amount);

    if (s_style > 2.5) {
        vec2 g = to_picture(moved) * grid;
        vec2 w = fwidth(g) * 1.2;
        vec2 near_line = min(fract(g), 1.0 - fract(g));
        float line = 1.0 - min(smoothstep(0.0, w.x, near_line.x), smoothstep(0.0, w.y, near_line.y));
        vec3 glow = 0.5 + 0.5 * cos(6.2831853 * (z * 1.5 + 0.05 * t + vec3(0.0, 0.33, 0.67)));
        c = mix(c * mix(1.0, 0.45, u_amount), glow * 1.3, line * u_amount);
    }
    return vec4(c, 1.0);
}
