// Swarm: a grid of specks, each with a home inside someone, coloured from its home and pushed
// around it by noise. Each point checks the specks in the nine nearest cells. Fading in, the
// specks grow from nothing as the people under them fade away.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw) || u_amount <= 0.0) return vec4(c, 1.0);
    vec2 pixels = uv * u_resolution;
    float size = s_size * u_resolution.y / 720.0;
    float person = smoothstep(0.35, 0.7, person_at(uv)) * u_amount;
    float t = u_time;
    float reach = (s_buzz / 100.0 + (u_beat * 1.6 + u_bass * 0.5) * s_scatter / 100.0);
    vec2 cell = floor(pixels / size);
    vec3 speck = vec3(0.0);
    float covered = 0.0;
    int span = reach > 0.9 ? 2 : 1;
    for (int y = -span; y <= span; y++) {
        for (int x = -span; x <= span; x++) {
            vec2 c2 = cell + vec2(float(x), float(y));
            vec2 home = (c2 + 0.5) * size / u_resolution;
            if (home != clamp(home, u_video_rect.xy, u_video_rect.zw) || person_at(home) < 0.5) continue;
            float seed = hash(c2);
            vec2 buzz = vec2(sin(t * (3.0 + 5.0 * seed) + seed * 40.0), cos(t * (4.0 + 4.0 * seed) + seed * 70.0));
            vec2 at = (c2 + 0.5) * size + buzz * size * reach;
            float d = length(pixels - at) / (size * (0.3 + 0.25 * seed)) / u_amount;
            float dot_here = 1.0 - smoothstep(0.75, 1.0, d);
            if (dot_here > covered) {
                covered = dot_here;
                speck = texture(u_video, home).rgb * (0.85 + 0.45 * seed);
            }
        }
    }
    if (person <= 0.0 && covered <= 0.0) return vec4(c, 1.0);
    vec3 behind = person > 0.0 ? mix(plate_at(uv) * 0.75, c, s_solid / 100.0) : c;
    vec3 shown = mix(c, behind, person);
    return vec4(mix(shown, speck, covered), 1.0);
}
