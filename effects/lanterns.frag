// Paper Lanterns: hung through the air of the shot's world, a few to each cell of a grid around
// where the camera is looking, so there are always some in view wherever it goes. Whatever's
// nearer than a lantern stands in front of it. They wait for the camera to be known, and light up
// as it is. Fading in, the lanterns swell from nothing as the picture dims and their light comes up.

const float LOOKING = 0.75;

vec4 effect(vec2 uv) {
    vec3 film = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw) || u_world_ready <= 0.0 || u_amount <= 0.0) return vec4(film, 1.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    // The same lanterns for all of one shot, and different ones in the next.
    float shot = mod(u_shot_start * 100.0, 971.0);
    float scene_distance = world_distance(depth_at(uv));
    vec3 tint = s_colour > 40.0 ? hue(s_colour / 360.0) * 1.3 : vec3(1.0, 0.72, 0.38);
    float react = s_react / 100.0;
    // Singing or tender music warms them and turns them up.
    float warm = min(u_sound_singing + u_sound_tender_music, 1.0) * react;
    tint = mix(tint, tint * vec3(1.15, 0.9, 0.65), min(0.5 * warm, 1.0)) * (1.0 + 0.3 * warm);
    // Wind heard in the film sways them further, and bangs and explosions jolt them.
    float wind = u_sound_wind * react;
    float jolt = max(u_sound_explosions, u_sound_impacts) * react;
    vec3 glow = vec3(0.0);
    vec3 paper = vec3(0.0);
    float covered = 0.0;
    float pulse = 1.0 + 0.5 * u_beat * react;
    // Where the camera is and which way it's looking, in the world.
    mat3 to_world = transpose(u_world_rotation);
    vec3 camera = -(to_world * u_world_offset);
    vec3 looking = camera + to_world * vec3(0.0, 0.0, LOOKING);
    // A grid that fills the view whatever the lens: it's the same all through a shot.
    float spacing = LOOKING * u_world_lens.y / u_world_lens.x / 3.0;
    vec3 middle = floor(looking / spacing);
    for (int x = -2; x <= 2; x++) {
        for (int y = -1; y <= 1; y++) {
            for (int z = -2; z <= 2; z++) {
                vec3 cell = middle + vec3(float(x), float(y), float(z));
                vec3 seed = cell + vec3(shot, 0.0, shot * 0.5);
                if (hash(seed) > s_count / 100.0) continue;
                vec3 world = (cell + vec3(hash(seed + 1.3), hash(seed + 2.7), hash(seed + 4.1))) * spacing;
                // Lanterns coming into or leaving the part of the grid being looked at fade.
                vec3 from_middle = abs(world - looking) / spacing;
                float kept = 1.0 - smoothstep(1.4, 2.3, max(from_middle.x, max(from_middle.y * 1.4, from_middle.z)));
                if (kept <= 0.0) continue;
                vec3 place = world_to_screen(world);
                float away = place.z;
                if (away < 0.15) continue;
                vec2 at = place.xy;
                // They sway a little on their strings.
                at.x += sin(u_time * (0.6 + 0.5 * hash(seed + 8.0)) + seed.x * 3.0) * (0.006 + 0.012 * wind) / away;
                at.x += (0.5 + 0.5 * sin(u_time * 0.4 + seed.y)) * 0.01 * wind / away;
                at += vec2(sin(u_time * 13.0 + seed.z * 5.0), sin(u_time * 17.0 + seed.x * 7.0)) * 0.005 * jolt / away;
                vec2 d = (uv - at) * aspect;
                float radius = 0.03 * s_size / 100.0 / away * u_amount;
                // Something nearer than the lantern is in front of it.
                float seen = smoothstep(away * 0.92, away * 1.08, scene_distance) * kept;
                float flicker = 0.85 + 0.15 * sin(u_time * (5.0 + 3.0 * hash(seed + 2.0)) + seed.z * 4.0);
                float r = length(d * vec2(1.0, 0.82)) / radius;
                // The light it casts falls on whatever's near it, even behind it.
                glow += tint * flicker * pulse * 0.016 / (r * r + 2.5) * (0.35 + 0.65 * seen) * kept * u_amount;
                if (r < 1.0) {
                    // Ribs of the paper, and brighter towards the flame.
                    float ribs = 0.82 + 0.18 * cos(asin(clamp(d.x / radius, -1.0, 1.0)) * 16.0);
                    float body = (1.0 - smoothstep(0.92, 1.0, r)) * seen;
                    paper = mix(paper, tint * flicker * pulse * mix(1.5, 0.75, r * r) * ribs, body);
                    covered = max(covered, body);
                }
            }
        }
    }
    vec3 c = film * (1.0 - s_dim / 100.0 * u_world_ready * u_amount) + (film * glow * 14.0 + glow * 1.8) * u_world_ready;
    return vec4(mix(c, paper, covered * u_world_ready), 1.0);
}
