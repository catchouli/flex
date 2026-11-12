// Black Hole: a real one, hanging in the room. Each line of sight is bent by its gravity on the
// way past and either falls in, crosses the glowing disc, or comes out to land on whatever's
// behind, so what you see around it is the room lensed, and by more the further back it is.
// Anything nearer than the hole stands in front of it, and the disc lights up the room. Fading in,
// the hole grows from a point and the people it hides go out of the room.

float fbm4(vec2 p) {
    float sum = 0.0, amount = 0.5;
    for (int i = 0; i < 4; i++) {
        sum += amount * noise(p);
        p = p * 2.03 + vec2(17.1, 9.2);
        amount *= 0.5;
    }
    return sum;
}

// In units of the hole's own radius: the camera's distance, and how far out to follow rays.
const float CAMERA = 16.0;
const float BOUND = 12.0;
// How big the hole's shadow looks compared with its radius.
const float SHADOW = 2.6;

float distance_of(float near) {
    return 1.0 / (0.1 + near * 1.2);
}

// The room behind, without anyone in it where that's known.
vec3 room_at(vec2 uv) {
    vec2 size = u_video_rect.zw - u_video_rect.xy;
    vec2 p = (uv - u_video_rect.xy) / size;
    // Mirrored at the edges, as there's nothing past them.
    p = 1.0 - abs(mod(p, 2.0) - 1.0);
    vec2 inside = u_video_rect.xy + p * size;
    float gone = smoothstep(0.3, 0.6, person_at(inside)) * smoothstep(0.2, 0.7, plate_known(inside)) * u_amount;
    vec3 c = texture(u_video, inside).rgb;
    return gone > 0.0 ? mix(c, plate_at(inside), gone) : c;
}

vec3 disc_colour(vec3 at, vec3 axis, vec3 towards_eye, vec3 u, vec3 v) {
    float r = length(at);
    float heat = pow(3.0 / r, 0.75);
    float spin = u_time * (1.0 + 2.0 * u_beat * s_react / 100.0);
    float angle = atan(dot(at, v), dot(at, u)) + spin * 2.6 / pow(r, 1.5);
    vec2 ring = vec2(cos(angle), sin(angle));
    float cloud = fbm4(ring * 2.6 + vec2(r * 1.4, -r * 0.7));
    cloud = 0.25 + 1.5 * cloud * cloud;
    // The side coming towards you is brighter and bluer.
    float beta = 0.55 / sqrt(r);
    float doppler = 1.0 / max(1.0 - beta * dot(normalize(cross(axis, at)), towards_eye), 0.15);
    float boost = doppler * doppler * doppler;
    vec3 tint = mix(vec3(1.0, 0.32, 0.07), vec3(1.0, 0.86, 0.68), clamp(heat * doppler - 0.25, 0.0, 1.0));
    return tint * cloud * pow(heat, 1.5) * boost * 3.4;
}

vec4 effect(vec2 uv) {
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(0.0, 0.0, 0.0, 1.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float t = u_time * s_drift / 100.0;
    vec2 centre_uv = u_video_rect.xy + (u_video_rect.zw - u_video_rect.xy) * (vec2(0.5, 0.56) + vec2(0.27 * sin(t * 0.31), 0.14 * sin(t * 0.43 + 1.0)));
    // It hangs in the room, so it stays there when the camera moves.
    centre_uv = world_pin_nearby(centre_uv, 1.5);
    vec2 centre = centre_uv * aspect;
    float radius = 0.06 * s_size / 100.0 * (1.0 + 0.12 * u_bass * s_react / 100.0) * max(u_amount, 1e-4);
    vec2 rel = p - centre;

    // It hangs just in front of what's behind it, leaving out anyone nearby, then as far
    // forward as it's told, so people and whatever else is closer pass in front.
    float behind_it = 0.0, counted = 0.0;
    for (int i = 0; i < 8; i++) {
        float a = float(i) * 0.7853982;
        vec2 around = centre_uv + vec2(cos(a), sin(a)) / aspect * (0.09 + radius * 2.0);
        float clear = 1.0 - smoothstep(0.2, 0.5, person_at(around));
        behind_it += depth_at(around) * clear;
        counted += clear;
    }
    behind_it = behind_it / max(counted, 0.5) + 0.05;
    float hole_near = mix(behind_it, 0.85, s_distance / 100.0);
    float hole_far = distance_of(hole_near);

    float near = depth_at(uv);
    float person = smoothstep(0.35, 0.65, person_at(uv));
    float front = mix(smoothstep(hole_near - 0.015, hole_near + 0.035, near), smoothstep(hole_near - 0.12, hole_near - 0.04, near), person);
    float scene_far = distance_of(near);

    vec3 c = texture(u_video, uv).rgb;
    vec3 lensed = c;
    vec3 glow = vec3(0.0);
    float glow_alpha = 0.0;

    float bend = s_pull / 100.0;
    vec2 s = rel / radius * SHADOW;
    float b = length(s);
    if (front < 0.999) {
        // A disc tipped back, leaning a little.
        float tip = radians(s_tilt);
        vec3 axis = normalize(vec3(0.16, cos(tip), sin(tip)));
        vec3 u = normalize(cross(axis, vec3(0.0, 0.0, 1.0)));
        vec3 v = cross(axis, u);

        vec2 landed;
        vec3 ray_at = vec3(0.0), ray = vec3(0.0, 0.0, 1.0);
        bool traced = b < BOUND;
        bool fell = false;
        if (traced) {
            vec3 dir = normalize(vec3(s, CAMERA));
            vec3 eye = vec3(0.0, 0.0, -CAMERA);
            // Straight up to the edge of where it matters.
            float along = -dot(eye, dir);
            float off = BOUND * BOUND - (dot(eye, eye) - along * along);
            vec3 pos = eye + dir * (along - sqrt(max(off, 0.0)));
            vec3 vel = dir;
            vec3 spin = cross(pos, vel);
            float h2 = dot(spin, spin);
            for (int i = 0; i < 110; i++) {
                float r2 = dot(pos, pos);
                if (r2 < 1.0) { fell = true; break; }
                if (r2 > BOUND * BOUND * 1.02 && dot(pos, vel) > 0.0) break;
                float r = sqrt(r2);
                float dt = clamp(0.1 * r, 0.04, 0.7);
                vel += -1.5 * h2 * pos / (r2 * r2 * r) * dt;
                vec3 before = pos;
                pos += vel * dt;
                float d0 = dot(before, axis), d1 = dot(pos, axis);
                if (d0 * d1 < 0.0 && glow_alpha < 0.98) {
                    vec3 at = mix(before, pos, d0 / (d0 - d1));
                    float R = length(at);
                    float there = smoothstep(2.4, 3.1, R) * (1.0 - smoothstep(6.5, 10.5, R));
                    if (there > 0.0) {
                        float alpha = there * 0.8;
                        glow += disc_colour(at, axis, -normalize(vel), u, v) * alpha * (1.0 - glow_alpha);
                        glow_alpha += alpha * (1.0 - glow_alpha);
                    }
                }
            }
            ray_at = pos;
            ray = normalize(vel);
        }

        // Where the bent ray lands, on the scene behind this point.
        vec2 from_uv = uv;
        for (int pass = 0; pass < 2; pass++) {
            float there = pass == 0 ? scene_far : distance_of(depth_at(clamp(from_uv, u_video_rect.xy, u_video_rect.zw)));
            float z = CAMERA * max(there / hole_far - 1.0, 0.02);
            vec2 at_plane;
            if (traced && !fell) {
                vec3 hit = ray.z > 0.04 ? ray_at + ray * (z - ray_at.z) / ray.z : ray_at + ray * 60.0;
                at_plane = hit.xy * CAMERA / (CAMERA + z);
            } else {
                at_plane = s - s / max(b * b, 1e-3) * 2.0 * z * CAMERA / (CAMERA + z);
            }
            at_plane = s + (at_plane - s) * bend;
            from_uv = (centre + at_plane / SHADOW * radius) / aspect;
        }
        lensed = fell ? vec3(0.0) : room_at(from_uv);
        lensed = lensed * (1.0 - glow_alpha) + glow * s_glow / 100.0 * (1.0 + 0.7 * u_bass * s_react / 100.0);
        // Dimmed close in, where most of the light has been pulled away.
        if (!fell && glow_alpha < 1.0) lensed *= mix(0.4, 1.0, smoothstep(2.6, 6.0, b));
        // Light that's been round the hole, along the edge of its shadow.
        float off_edge = (b - SHADOW) / 0.16;
        lensed += vec3(1.0, 0.82, 0.6) * exp(-off_edge * off_edge) * 1.6 * s_glow / 100.0;
    }

    vec3 shown = mix(lensed, c, front);

    // The disc's light falling on the room, and catching the edges of anyone in front of it.
    vec2 screen = (uv - 0.5) * aspect;
    vec3 surface = vec3(screen * scene_far, scene_far);
    vec2 hole_screen = (centre_uv - 0.5) * aspect;
    vec3 hole = vec3(hole_screen * hole_far, hole_far);
    vec3 to_hole = hole - surface;
    float away = length(to_hole);
    // The surface's direction, with z into the picture as `surface` has it.
    vec3 facing_us = normal_at(uv);
    vec3 normal = vec3(facing_us.xy, -facing_us.z);
    float facing = max(dot(normal, to_hole / max(away, 1e-3)), 0.0);
    float size_of = radius * hole_far * 4.0;
    float falling = size_of * size_of / (away * away + size_of * size_of * 0.25);
    float amount = s_light / 100.0 * (1.0 + 0.6 * u_bass * s_react / 100.0) * falling;
    vec3 light = vec3(1.0, 0.55, 0.26);
    shown += shown * light * facing * amount * 2.4;
    float edge = pow(1.0 - clamp(-normal.z, 0.0, 1.0), 2.0) * front * smoothstep(0.25, 1.4, falling);
    shown += light * edge * s_light / 100.0 * 0.55;
    return vec4(shown, 1.0);
}
