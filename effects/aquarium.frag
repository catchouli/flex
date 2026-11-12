// Aquarium: a flat sheet of water in the room, level with the floor or table found from depth, so
// it tilts with the camera. A point is underwater if it's further away than where its line of
// sight meets the water, so the waterline runs along the table and around people on its own. What's seen is then refracted, reflected about where it
// meets the water, lit by caustics and fogged by how much water is in front of it. Fading in, the
// water rises from below the floor to its level.

// The waves on the surface, at a point on it.
float swell(vec2 at, float t) {
    float h = sin(at.x * 5.0 + t * 1.1) * 0.5 + sin(at.y * 7.0 - t * 1.4 + at.x * 2.0) * 0.35;
    h += sin((at.x + at.y) * 15.0 + t * 2.3) * 0.12 + (noise(at * 9.0 + t * 0.6) - 0.5) * 0.5;
    return h;
}

vec2 slope(vec2 at, float t) {
    float e = 0.02;
    return vec2(swell(at + vec2(e, 0.0), t) - swell(at - vec2(e, 0.0), t), swell(at + vec2(0.0, e), t) - swell(at - vec2(0.0, e), t)) / (2.0 * e);
}

// The usual web of light on the bottom of a pool, from a few rounds of folding space.
float caustics(vec2 p, float t) {
    vec2 q = mod(p * 8.0, 6.2831853) - 250.0;
    vec2 i = q;
    float sum = 1.0;
    for (int n = 0; n < 4; n++) {
        float at = t * 0.6 * (1.0 - 3.5 / float(n + 1));
        i = q + vec2(cos(at - i.x) + sin(at + i.y), sin(at - i.y) + cos(at + i.x));
        sum += 1.0 / length(vec2(q.x / (sin(i.x + at) / 0.005), q.y / (cos(i.y + at) / 0.005)));
    }
    sum = 1.17 - pow(sum / 4.0, 1.4);
    return min(pow(abs(sum), 8.0), 1.5);
}

// How far past everything in the picture the true horizon's depth is.
const float BEYOND = 0.1;

float ring_bubble(vec2 p, vec2 centre, float size) {
    float d = length(p - centre) / size;
    return (1.0 - smoothstep(0.85, 1.0, d)) * (0.2 + 0.8 * smoothstep(0.55, 0.95, d)) + 0.7 * (1.0 - smoothstep(0.0, 0.25, length(p - centre - vec2(-0.3, 0.3) * size) / size));
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 1.0);
    float aspect = u_resolution.x / u_resolution.y;
    vec2 low = u_video_rect.xy, size = u_video_rect.zw - low;
    vec2 pic = (uv - low) / size;
    vec2 p = uv * vec2(aspect, 1.0);
    float t = u_time;
    float react = s_react / 100.0;

    float sky = smoothstep(0.4, 0.7, sky_at(uv));
    float near = mix(depth_at(uv), 0.0, sky);
    float far = 1.0 / max(near + BEYOND, 0.02);
    float person = person_at(uv);

    float filled = s_rise < 0.5 ? 1.0 : smoothstep(0.0, 1.0, t / s_rise);
    // The level surface, as how far below the horizon each point of the picture is. With 1 for
    // the height of that surface below the camera, the water is at above_water.
    float found = s_find * u_ground.w;
    vec3 ground = mix(vec3(0.0, -1.7, 1.7 * s_horizon / 100.0), u_ground.xyz + vec3(0.0, 0.0, BEYOND), found);
    float above_water = mix(1.0, -0.4, s_level / 100.0 * filled) + (1.0 - sqrt(u_amount)) / max(sqrt(u_amount), 1e-3) * 1.5 - 0.012 * u_bass * react;
    float below = dot(ground, vec3(pic, 1.0));
    vec2 downhill = ground.xy / vec2(aspect, 1.0);
    vec2 along = normalize(vec2(-downhill.y, downhill.x));
    float across = dot((pic - 0.5) * vec2(aspect, 1.0), along * sign(along.x + 1e-5));
    float waves = s_waves / 100.0 * (1.0 + 1.2 * u_bass * react);

    // How far along the line of sight the water starts, if it does.
    float hit = 1e4;
    bool from_above = above_water > 0.0;
    if (from_above && below > 0.002) hit = above_water / below;
    if (!from_above && below < -0.002) hit = -above_water / -below;
    vec2 on_surface = vec2(across, 1.0) * min(hit, 12.0);
    if (hit < 1e3) {
        float lift = swell(on_surface, t) * 0.012 * waves;
        hit = from_above ? max(above_water + lift, 0.001) / below : max(-above_water + lift, 0.001) / -below;
        on_surface = vec2(across, 1.0) * min(hit, 12.0);
    }
    bool sees_surface = hit < far;
    bool under = from_above ? sees_surface : true;
    if (!under) return vec4(c, 1.0);

    float through = from_above ? far - hit : (sees_surface ? hit : far);
    vec2 tilt = hit < 1e3 ? slope(on_surface, t) * waves : vec2(0.0);

    // Rings spreading out from people where they stand in the water.
    float crowd = 0.0;
    for (int i = 0; i < 8; i++) {
        float a = float(i) * 0.7853982;
        crowd += person_at(uv + vec2(cos(a) / aspect, sin(a) * 0.4) * 0.06);
    }
    crowd *= 0.125 * s_wake / 100.0;
    float wake = sin(crowd * 30.0 - t * 5.0 - u_beat * react * 4.0) * crowd * (1.0 - smoothstep(0.4, 0.7, person));
    tilt += vec2(0.0, wake * 3.0);

    vec2 bend = (tilt * 0.0045 + vec2(sin(p.y * 40.0 + t * 1.6), cos(p.x * 34.0 - t * 1.3)) * 0.0012) * min(through + 0.3, 2.5) / vec2(aspect, 1.0);
    vec2 from = clamp(uv + bend, low, u_video_rect.zw);
    vec3 w = texture(u_video, from).rgb;
    float near_at = depth_at(from);

    // Light playing over whatever's underwater. Walls take it upright, everything else lying down.
    float wall = smoothstep(0.35, 0.7, wall_at(uv));
    vec2 lay_flat = vec2((pic.x - 0.5) * aspect, 1.0) * far;
    vec2 upright = vec2((pic.x - 0.5) * aspect * far, pic.y * 3.0);
    float light = caustics(mix(lay_flat, upright, wall) + near_at * 0.1, t);
    float shallow = exp(-through * 0.9);
    w *= vec3(0.62, 0.86, 1.0);
    w += vec3(0.75, 0.97, 1.0) * light * shallow * s_light / 100.0 * (0.35 + 0.5 * dot(w, vec3(0.5)));

    float fog = 1.0 - exp(-through * 1.5 * s_murk / 50.0);
    vec3 deep = mix(vec3(0.05, 0.42, 0.55), vec3(0.01, 0.1, 0.28), clamp(through * 0.3, 0.0, 1.0));

    float own = smoothstep(0.4, 0.7, person);
    for (int i = 0; i < 2; i++) {
        float layer = float(i);
        float scale = 0.11 + 0.1 * layer;
        vec2 q = vec2(p.x + t * (0.05 + 0.03 * layer) * (mod(layer, 2.0) < 0.5 ? 1.0 : -1.0), p.y + sin(t * 0.3 + layer) * 0.02) / scale;
        vec2 cell = floor(q);
        float wanted = s_fish / 100.0 * 0.55;
        float layer_near = 0.2 + 0.35 * layer;
        if (hash(cell + layer * 7.0) < wanted && near_at < layer_near) {
            vec2 f = fract(q) - vec2(0.5, 0.5 + 0.25 * (hash(cell + 3.0) - 0.5));
            f.x *= mod(layer, 2.0) < 0.5 ? 1.0 : -1.0;
            float wag = sin(t * 7.0 + hash(cell) * 30.0) * 0.05;
            float body = 1.0 - smoothstep(0.85, 1.0, length(vec2(f.x, f.y + wag * (f.x + 0.3)) / vec2(0.26, 0.11)));
            vec2 tail_at = f + vec2(0.33, wag * 2.0);
            float tail = (1.0 - smoothstep(0.08, 0.12, tail_at.x * 0.6 + abs(tail_at.y))) * smoothstep(-0.12, -0.08, tail_at.x);
            float fish = max(body, tail);
            vec3 skin = mix(vec3(1.0, 0.55, 0.1), vec3(0.3, 0.75, 0.95), hash(cell + 11.0)) * (0.7 + 0.5 * smoothstep(-0.1, 0.1, f.y));
            w = mix(w, skin, fish * (1.0 - own));
        }
    }
    w = mix(w, deep, clamp(fog, 0.0, 0.94));

    if (sees_surface && from_above) {
        // The reflection, flipped about where this point's own distance meets the water.
        vec2 flipped = pic * vec2(aspect, 1.0) + downhill * 2.0 * (above_water / far - below) / max(dot(downhill, downhill), 1e-4);
        vec2 mirrored = flipped / vec2(aspect, 1.0) + tilt * 0.012 * vec2(0.5, 1.0);
        vec3 seen = texture(u_video, low + clamp(mirrored, vec2(0.001), vec2(0.999)) * size).rgb;
        float glance = mix(0.12, 0.85, smoothstep(0.25, 3.0, hit));
        w = mix(w, seen * vec3(0.8, 0.92, 1.0), glance * s_mirror / 100.0);
        // White water where the surface meets something.
        float touching = 1.0 - smoothstep(0.0, 0.05 + 0.04 * noise(on_surface * 40.0 + t), far - hit);
        w += vec3(0.9, 0.97, 1.0) * touching * 0.55;
        w += vec3(0.85, 0.95, 1.0) * abs(wake) * 0.45;
    }
    if (sees_surface && !from_above) {
        // The underside of the surface, bright and broken up.
        float sheen = 0.4 + 0.6 * smoothstep(-0.8, 1.2, swell(on_surface, t));
        w = mix(w, mix(vec3(0.2, 0.6, 0.75), vec3(0.85, 0.97, 1.0), sheen), exp(-hit * 0.45));
    }

    float amount = s_bubbles / 100.0;
    for (int i = 0; i < 3; i++) {
        float f = float(i);
        float size_of = 0.03 + f * 0.018;
        vec2 q = vec2(p.x, p.y - t * (0.12 + 0.05 * f)) / size_of + f * 13.0;
        vec2 cell = floor(q);
        if (hash(cell) < 0.1 * amount && near_at < 0.25 + 0.3 * f) {
            vec2 centre = (vec2(0.5 + 0.28 * sin(t * 1.5 + 6.0 * hash(cell + 3.0)), 0.5) + cell - f * 13.0) * size_of;
            w += vec3(0.85, 0.95, 1.0) * ring_bubble(vec2(p.x, p.y - t * (0.12 + 0.05 * f)), centre, size_of * (0.15 + 0.2 * hash(cell + 1.0))) * 0.5;
        }
    }
    // People breathing out, if their mouths are under.
    for (int i = 0; i < u_face_count; i++) {
        if (u_face_seen[i] <= 0.0 || amount <= 0.0) continue;
        vec2 mouth = 0.5 * (face_point(i, 3) + face_point(i, 4));
        float mouth_below = dot(ground, vec3((mouth - low) / size, 1.0));
        float mouth_far = 1.0 / max(depth_at(mouth) + BEYOND, 0.02);
        if (from_above && (mouth_below <= 0.002 || above_water / mouth_below >= mouth_far)) continue;
        vec2 at = mouth * vec2(aspect, 1.0);
        float face_width = u_faces[i].z - u_faces[i].x;
        for (int b = 0; b < 7; b++) {
            float n = float(b);
            float life = fract(t * 0.35 + n / 7.0 + float(i) * 0.3);
            vec2 centre = at + vec2(sin(life * 9.0 + n * 2.0) * 0.015, life * 0.5);
            w += vec3(0.9, 0.97, 1.0) * ring_bubble(p, centre, face_width * (0.04 + 0.05 * fract(n * 0.37))) * u_face_seen[i] * 0.7 * (1.0 - life);
        }
    }
    return vec4(w, 1.0);
}
