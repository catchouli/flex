// Wallpaper Crawl: the walls flow along a moving noise field, and a grid of eyes hides in them.
// Each eye opens now and then and looks at the first face it can find. Fading in, the walls start to
// writhe and colour, and the eyes open from shut.

// An eye at `q` (roughly -1 to 1 inside it), looking along `look`, opened by `open`.
vec4 eye(vec2 q, vec2 look, float open) {
    float lid = 0.05 + 0.5 * open;
    float top = lid * (1.0 - q.x * q.x);
    float shape = (1.0 - smoothstep(-0.03, 0.03, abs(q.y) - top)) * step(abs(q.x), 1.0);
    if (shape <= 0.0) return vec4(0.0);
    vec2 iris_centre = look * vec2(0.3, 0.12);
    float d = length(q - iris_centre);
    vec3 c = vec3(0.92, 0.88, 0.8) * (0.75 + 0.25 * smoothstep(0.0, top, top - abs(q.y)));
    c = mix(c, vec3(0.5, 0.25, 0.06) + 0.25 * sin(atan(q.y - iris_centre.y, q.x - iris_centre.x) * 18.0), (1.0 - smoothstep(0.3, 0.34, d)));
    c = mix(c, vec3(0.01), (1.0 - smoothstep(0.12, 0.15, d)));
    c += vec3(0.9) * (1.0 - smoothstep(0.03, 0.05, length(q - iris_centre - vec2(0.07, 0.07))));
    return vec4(c, shape);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    float wall = smoothstep(0.35, 0.7, wall_at(uv));
    if (wall <= 0.0) return vec4(c, 1.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    // On the wall itself, so the writhing and the eyes stay put when the camera moves.
    vec2 p = shot_uv(uv) * aspect;
    float t = u_video_time;
    vec2 flow = vec2(fbm(p * 4.0 + vec2(t * 0.25, 0.0)), fbm(p * 4.0 + vec2(5.2, -t * 0.2))) - 0.47;
    vec2 from = uv + flow * 0.06 * s_crawl / 100.0 * u_amount * (1.0 + u_bass);
    vec3 moved = texture(u_video, clamp(from, u_video_rect.xy, u_video_rect.zw)).rgb;
    moved = mix(moved, moved * hue(flow.x * 2.0 + t * 0.05) * 1.8, s_colour / 100.0 * u_amount);

    float size = 0.14;
    vec2 cell = floor(p / size);
    float chosen = hash(cell + 11.0);
    // Quiet and scary music wake more of the walls, and a scream snaps every eye open.
    float uneasy = max(u_sound_silence, u_sound_scary_music);
    float scream = u_sound_shouting;
    float eyes = s_eyes / 100.0;
    float more = eyes * 0.8 * max(uneasy, scream);
    if (chosen < eyes + more) {
        // The extra eyes fade in as the sound grows, rather than all appearing at once.
        float woken = chosen < eyes ? 1.0 : clamp((eyes + more - chosen) / 0.1, 0.0, 1.0);
        float rhythm = sin(t * (0.4 + 0.5 * hash(cell)) + 30.0 * hash(cell + 2.0));
        float open = smoothstep(0.1 - 0.9 * uneasy, 0.6 - 0.5 * uneasy, rhythm);
        // A blink now and then.
        open *= 1.0 - smoothstep(0.96, 1.0, sin(t * 2.3 + 9.0 * hash(cell + 4.0))) * (1.0 - scream);
        open = mix(open, 1.0, scream) * woken * u_amount;
        if (open > 0.0) {
            vec2 centre = (cell + 0.5) * size;
            vec2 target = u_face_count > 0 ? face_point(0, 2) * aspect : vec2(aspect.x * 0.5 + 0.3 * sin(t * 0.7), 0.4);
            // Further away, smaller.
            vec2 q = (p - centre) / (size * 0.42 * (0.55 + depth_at(centre / aspect))) * vec2(1.0, 1.4);
            vec4 e = eye(q, normalize(target - centre), open);
            moved = mix(moved, e.rgb, e.a);
        }
    }
    return vec4(mix(c, moved, wall), 1.0);
}
