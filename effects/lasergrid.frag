// Laser Grid: straight beams across the shot's world, at heights and distances around where the
// camera is looking, so there are always some in view. Each pixel's line of sight is tested against
// each beam, so a beam shows only where it passes in front of what's there, and where it meets a
// surface it leaves a bright spot, brightest on people. Fading in, the beams and spots thicken from
// nothing as the room dims.

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

vec4 effect(vec2 uv) {
    vec3 film = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw) || u_world_ready <= 0.0 || u_amount <= 0.0) return vec4(film, 1.0);
    float shot = mod(u_shot_start * 100.0, 971.0);
    float t = u_video_time;
    vec3 origin, direction;
    world_ray(uv, origin, direction);
    float scene = world_distance(depth_at(uv));
    vec3 surface = origin + direction * scene;
    float person = smoothstep(0.3, 0.6, person_at(uv));
    vec3 tint = hue(s_colour / 360.0 + 0.02) * vec3(1.6, 0.9, 0.9);
    if (s_colour < 5.0) tint = vec3(1.6, 0.12, 0.08);
    float light = 0.0, spots = 0.0;
    float pulse = 1.0 + 0.6 * u_beat * s_react / 100.0;
    // A beam or none at each height and distance of a grid in the world around where the camera's
    // looking, each running across the room at an angle of its own.
    vec3 centre_origin, ahead;
    world_ray(vec2(0.5 * (u_video_rect.xy + u_video_rect.zw)), centre_origin, ahead);
    vec3 looking = origin + normalize(ahead) * 0.55;
    float spacing = 0.55 * u_world_lens.z / u_world_lens.x / 2.5;
    vec2 middle = floor(looking.yz / spacing);
    for (int y = -3; y <= 3; y++) {
        for (int z = -2; z <= 2; z++) {
            vec2 cell = middle + vec2(float(y), float(z));
            vec3 seed = vec3(cell, shot);
            if (hash(seed) > s_beams / 16.0) continue;
            vec3 at = vec3(looking.x, (cell + vec2(hash(seed + 1.0), hash(seed + 2.0))) * spacing);
            vec2 from_middle = abs(at.yz - looking.yz) / spacing;
            float kept = 1.0 - smoothstep(2.2, 3.2, max(from_middle.x, from_middle.y * 1.5));
            if (kept <= 0.0) continue;
            float yaw = (hash(seed + 3.0) - 0.5) * 1.2, slant = hash(seed + 4.0) > 0.7 ? (hash(seed + 5.0) - 0.5) * 0.9 : 0.0;
            vec3 along = normalize(vec3(cos(yaw), slant, sin(yaw)));
            // Where the line of sight comes closest to the beam.
            vec3 w0 = origin - at;
            float a = dot(direction, direction), b = dot(direction, along), d = dot(direction, w0), e = dot(along, w0);
            float den = max(a - b * b, 1e-6);
            float s = (b * e - d) / den;
            float u = (a * e - b * d) / den;
            float gap = length(w0 + direction * s - along * u);
            float width = (0.0006 + 0.0008 * max(s, 0.0)) * 640.0 / u_world_lens.x * u_amount;
            float in_front = step(0.05, s) * (1.0 - smoothstep(scene * 0.97, scene * 1.03, s)) * kept;
            float shimmer = 0.85 + 0.15 * noise_h3(vec2(u * 30.0 - t * 2.0, cell.x + cell.y * 7.0));
            light += in_front * shimmer * (exp(-gap * gap / (width * width)) + 0.25 * s_haze / 100.0 * exp(-gap / (width * 6.0)));
            // Where it meets whatever's there.
            float off = length(cross(surface - at, along));
            float spot = 0.004 * scene * 640.0 / u_world_lens.x * u_amount;
            spots += exp(-off * off / (spot * spot)) * (1.0 + 2.5 * person) * kept;
        }
    }
    vec3 c = film * (1.0 - 0.25 * u_world_ready * u_amount);
    c += tint * (light * pulse + spots * 0.8) * u_world_ready;
    c += film * tint * spots * 0.6 * u_world_ready;
    return vec4(c, 1.0);
}
