// Fog Layer: a level of mist in the shot's world, a little above the ground that depth finds. Each pixel's line of
// sight is followed down into it, so whatever's deep in the fog is hidden by more of it, and what
// stands above it isn't. Fading in, the mist rises from the ground and thickens.

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
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw) || u_world_ready <= 0.0) return vec4(film, 1.0);
    vec3 origin, direction;
    world_ray(uv, origin, direction);
    float scene = world_distance(depth_at(uv));
    // The ground, from depth, gives which way is up, and how high the camera is. A shot's world
    // has no real scale, so the fog's height goes by the camera's.
    if (u_ground.w < 0.3) return vec4(film, 1.0);
    vec2 size = u_video_rect.zw - u_video_rect.xy;
    vec2 a = vec2(0.3, 0.12), b = vec2(0.7, 0.12), m = vec2(0.5, 0.3);
    vec3 g0 = world_point(u_video_rect.xy + a * size, dot(vec3(a, 1.0), u_ground.xyz));
    vec3 g1 = world_point(u_video_rect.xy + b * size, dot(vec3(b, 1.0), u_ground.xyz));
    vec3 g2 = world_point(u_video_rect.xy + m * size, dot(vec3(m, 1.0), u_ground.xyz));
    vec3 up = normalize(cross(g1 - g0, g2 - g0));
    if (dot(up, origin - g0) < 0.0) up = -up;
    float eye = dot(origin - g0, up);
    if (eye <= 1e-3) return vec4(film, 1.0);
    float top = eye * s_height / 100.0 * u_amount;
    // How much of the line of sight, up to what's there, is below the top.
    float h0 = eye, h1 = dot(origin + direction * scene - g0, up);
    float inside = 0.0;
    if (h0 < top && h1 < top) {
        inside = scene;
    } else if (h0 < top || h1 < top) {
        float cross_at = clamp((top - h0) / (h1 - h0) * scene, 0.0, scene);
        inside = h1 < top ? scene - cross_at : cross_at;
    }
    inside *= length(direction) / eye;
    vec3 at = origin + direction * scene;
    float t = u_video_time * s_drift / 100.0;
    // Along the ground, for the billows.
    vec3 across = normalize(cross(up, vec3(0.0, 0.0, 1.0)) + vec3(1e-4));
    vec2 flat_at = vec2(dot(at, across), dot(at, cross(up, across))) / eye;
    // Wind heard in the film churns it, and blows a quicker layer of it through.
    float wind = u_sound_wind;
    vec2 churn = vec2(fbm_h3(flat_at * 1.6 + vec2(t * 0.21, 3.7)), fbm_h3(flat_at * 1.6 + vec2(8.3, -t * 0.17))) - 0.5;
    float slow = fbm_h3(flat_at * 0.7 + vec2(t * 0.06, t * 0.025) + churn * 1.2 * wind);
    float quick = fbm_h3(flat_at * 0.9 + vec2(t * 0.42, t * 0.12) + churn * 1.5);
    float billow = 0.55 + 0.9 * mix(slow, quick, 0.6 * wind);
    // Thicker the further down into it.
    float depth_in = clamp(1.0 - dot(at - g0, up) / max(top, 1e-3), 0.0, 1.0);
    // It thickens a little in silence, and in frightening music.
    float settle = 1.0 + 0.3 * u_sound_silence + 0.4 * u_sound_scary_music;
    float fog = 1.0 - exp(-inside * 2.0 * s_thick / 100.0 * settle * billow * (0.5 + depth_in) * u_amount);
    float light = dot(film, vec3(0.3, 0.59, 0.11));
    vec3 mist = mix(vec3(0.62, 0.66, 0.72), vec3(0.85, 0.88, 0.92), billow - 0.55) * (0.6 + 0.5 * light);
    return vec4(mix(film, mist, fog * u_world_ready), 1.0);
}
