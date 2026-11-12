// Living Static: inside people the picture is thresholded against fresh noise every frame, so
// they keep their shape but are made of snow, with rolling bars and faces coming through. Fading
// in, people turn to snow a grain at a time.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 1.0);
    float person = smoothstep(0.4, 0.7, person_at(uv));
    vec2 grain = floor(uv * u_resolution / 2.0);
    float turned = hash(grain + 41.0);
    person *= smoothstep(turned, turned + 0.2, u_amount * 1.2);
    if (person <= 0.0) return vec4(c, 1.0);
    float t = u_time;
    float n = hash(grain + fract(t) * 137.0);
    float luma = dot(c, vec3(0.3, 0.59, 0.11));
    float bars = 0.75 + 0.25 * sin(uv.y * 26.0 + t * 6.0) * (1.0 + u_bass * s_react / 50.0);
    float snow = step(n, pow(luma, 0.8) * 1.4 + 0.05) * bars;
    vec3 tv = mix(vec3(snow), vec3(snow) * vec3(hash(grain + 3.1 + fract(t)), hash(grain + 7.7 + fract(t)), hash(grain + 1.3 + fract(t))) * 1.8, s_colour / 100.0);
    float flicker = 0.5 + 0.5 * sin(t * 13.0 + 8.0 * noise(vec2(t * 2.0, 1.0)));
    tv = mix(tv, c * (0.8 + 0.3 * snow), face_at(uv) * s_faces / 100.0 * (0.4 + 0.6 * flicker));
    return vec4(mix(c, tv, person), 1.0);
}
