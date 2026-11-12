// Fish Shoal: shoals swim loops through the air of the shot's world, one or none to each cell of a
// grid around where the camera is looking, so there are always some about wherever it goes.
// Whatever's nearer than a fish stands in front of it. Fading in, the fish grow from nothing.

// Value noise on hash(vec3) rather than hash(vec2), and four octaves of it.
float noise_h3(vec2 p) {
    vec2 i = floor(p), f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    float a = hash(vec3(i, 0.0)), b = hash(vec3(i + vec2(1.0, 0.0), 0.0));
    float c = hash(vec3(i + vec2(0.0, 1.0), 0.0)), d = hash(vec3(i + vec2(1.0, 1.0), 0.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

float fbm_h3(vec2 p) {
    float sum = 0.0, amount = 0.5;
    for (int i = 0; i < 4; i++) {
        sum += amount * noise_h3(p);
        p = p * 2.03 + vec2(17.1, 9.2);
        amount *= 0.5;
    }
    return sum;
}

// How tall something looks on screen `away` from the camera, if it's `size` of the picture's height
// 1 away. A shot's world has no real scale, so sizes go by the picture instead.
float picture_size(float size, float away) {
    return size / max(away, 1e-3) * (u_video_rect.w - u_video_rect.y);
}

// Most of a room is further away than this, in a shot's world.
const float LOOKING = 0.5;

vec4 effect(vec2 uv) {
    vec3 film = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw) || u_world_ready <= 0.0 || u_amount <= 0.0) return vec4(film, 1.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    float shot = mod(u_shot_start * 100.0, 971.0);
    float t = u_video_time * s_speed / 100.0;
    float scene_distance = world_distance(depth_at(uv));
    vec3 origin, ahead;
    world_ray(vec2(0.5 * (u_video_rect.xy + u_video_rect.zw)), origin, ahead);
    // In front of the room: never further than most of it, nor than what's in the middle of the
    // picture. Only which cells are near changes with this; where they are doesn't.
    vec2 size = u_video_rect.zw - u_video_rect.xy;
    float room = 0.0;
    for (int i = 0; i < 5; i++) {
        vec2 at = vec2(0.5) + 0.25 * vec2(i == 1 ? -1.0 : i == 2 ? 1.0 : 0.0, i == 3 ? -1.0 : i == 4 ? 1.0 : 0.0);
        room += world_distance(depth_at(u_video_rect.xy + at * size)) / 5.0;
    }
    vec3 looking = origin + normalize(ahead) * min(LOOKING, 0.6 * room);
    // A shoal or none in each cell of a grid in the world around where the camera's looking.
    float spacing = LOOKING * u_world_lens.y / u_world_lens.x / 1.4;
    vec3 middle = floor(looking / spacing);
    vec3 glow = vec3(0.0);
    vec3 fish_colour = vec3(0.0);
    float covered = 0.0;
    float react = s_react / 100.0;
    float pulse = 1.0 + 0.6 * u_beat * react;
    // Bangs, explosions and shouting startle them: they dart on and scatter apart, and gather
    // again as it passes. In silence they close up and swim calmly.
    float startle = min(max(max(u_sound_impacts, u_sound_explosions), u_sound_shouting) * react, 1.0);
    float calm = u_sound_silence * min(react, 1.0) * (1.0 - min(startle, 1.0));
    for (int x = -1; x <= 1; x++) {
        for (int z = -1; z <= 1; z++) {
            vec3 cell = middle + vec3(float(x), 0.0, float(z));
            vec3 shoal = cell + vec3(shot, 0.0, shot * 0.5);
            if (hash(shoal) > 0.55) continue;
            vec3 centre = (cell + 0.5) * spacing;
            vec3 from_middle = abs(centre - looking) / spacing;
            float kept = 1.0 - smoothstep(0.9, 1.5, max(from_middle.x, from_middle.z));
            if (kept <= 0.0) continue;
            float turning = hash(shoal + 4.0) > 0.5 ? 1.0 : -1.0;
            for (int i = 0; i < 40; i++) {
                if (i >= int(s_count)) break;
                float k = float(i);
                vec3 seed = shoal + vec3(k * 1.7, 2.0, 0.0);
                // Round a loop in the cell, each fish a little behind the one before.
                float phase = turning * (t * 0.5 - k * 0.12 + 0.5 * startle * (0.6 + 0.8 * hash(seed + 6.0))) + 0.3 * hash(seed);
                vec3 spread = (vec3(hash(seed + 1.0), hash(seed + 2.0), hash(seed + 3.0)) - 0.5) * spacing * vec3(0.25, 0.15, 0.25);
                spread *= 1.0 + 2.2 * startle - 0.35 * calm;
                vec3 world = centre + spacing * vec3(0.38 * cos(phase), 0.08 * sin(2.0 * phase), 0.3 * sin(phase)) + spread;
                vec3 next = centre + spacing * vec3(0.38 * cos(phase + turning * 0.05), 0.08 * sin(2.0 * phase + turning * 0.1), 0.3 * sin(phase + turning * 0.05)) + spread;
                vec3 place = world_to_screen(world);
                if (place.z < 0.1) continue;
                vec2 heading = (world_to_screen(next).xy - place.xy) * aspect;
                heading = length(heading) > 1e-6 ? normalize(heading) : vec2(1.0, 0.0);
                float long_ = picture_size(0.035 * s_size / 100.0 * (0.8 + 0.4 * hash(seed + 5.0)), place.z) * u_amount;
                vec2 d = (uv - place.xy) * aspect;
                vec2 q = vec2(dot(d, heading), dot(d, vec2(-heading.y, heading.x))) / long_;
                if (abs(q.x) > 1.2 || abs(q.y) > 0.6) continue;
                float seen = smoothstep(place.z * 0.93, place.z * 1.07, scene_distance) * kept;
                // The tail swishes, and the body with it.
                q.y -= 0.06 * (1.0 + 0.8 * startle - 0.5 * calm) * sin(t * 11.0 + k) * clamp(-q.x, 0.0, 1.0);
                float body = 1.0 - smoothstep(0.85, 1.0, length(q / vec2(0.5, 0.17)));
                float tail = step(q.x, -0.42) * step(-0.78, q.x) * (1.0 - smoothstep(0.0, 0.02, abs(q.y) - (-q.x - 0.42) * 0.75));
                float shape = max(body, tail) * seen;
                vec3 colour = hue(0.5 + 0.25 * hash(shoal + 9.0) + 0.08 * hash(seed + 9.0) + 0.1 * sin(t * 0.3 + k)) * 1.3 * pulse;
                // A darker eye near the front.
                colour *= 1.0 - 0.8 * (1.0 - smoothstep(0.03, 0.05, length(q - vec2(0.3, 0.04))));
                fish_colour = mix(fish_colour, colour * (0.75 + 0.35 * (1.0 - abs(q.y) / 0.17)), shape);
                covered = max(covered, shape);
                glow += colour * 0.02 / (dot(q, q) * 4.0 + 1.0) * seen;
            }
        }
    }
    vec3 c = film + film * glow * 2.0 * u_world_ready;
    return vec4(mix(c, fish_colour, covered * u_world_ready), 1.0);
}
