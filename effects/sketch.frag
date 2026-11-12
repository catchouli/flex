// Ink Drawing People: finds edges in the picture inside people and draws them as ink, with
// diagonal hatching for the shadows, redrawn a few times a second like hand-drawn animation.
// Fading in, the drawing spreads over people in patches.

// Features drawn crisply from the mesh, where the picture alone would be mush: an inked line at
// the edge of the lips, eyes and brows.
float features(vec2 uv) {
    return lips_at(uv) + eye_at(uv) + brows_at(uv) + mouth_at(uv) * 0.5 + iris_at(uv) * 0.8;
}

float feature_line(vec2 uv, float wide) {
    vec2 e = wide / u_resolution;
    return clamp((abs(features(uv + vec2(e.x, 0.0)) - features(uv - vec2(e.x, 0.0))) + abs(features(uv + vec2(0.0, e.y)) - features(uv - vec2(0.0, e.y)))) * 1.5, 0.0, 1.0);
}

float luma(vec2 uv) {
    return dot(texture(u_video, uv).rgb, vec3(0.3, 0.59, 0.11));
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 1.0);
    float person = smoothstep(0.42, 0.62, person_at(uv));
    if (person <= 0.0) return vec4(c, 1.0);
    float step_time = floor(u_time * 8.0);
    vec2 p = uv * u_resolution;
    vec2 wobble = (vec2(noise(p * 0.03 + step_time), noise(p * 0.03 - step_time + 9.0)) - 0.5) * s_boil / 100.0 * 4.0;
    vec2 at = uv + wobble / u_resolution;
    vec2 e = 1.5 / u_resolution * u_resolution.y / 720.0;
    float dx = luma(at + vec2(e.x, 0.0)) - luma(at - vec2(e.x, 0.0));
    float dy = luma(at + vec2(0.0, e.y)) - luma(at - vec2(0.0, e.y));
    float edge = smoothstep(0.05, 0.16, length(vec2(dx, dy)) * s_lines / 100.0);
    float tone = luma(at);
    float scale = u_resolution.y / 720.0;
    float hatch1 = step(0.5, fract((p.x + p.y + wobble.x * 3.0) / (7.0 * scale))) * (1.0 - smoothstep(0.2, 0.5, tone));
    float hatch2 = step(0.5, fract((p.x - p.y + wobble.y * 3.0) / (7.0 * scale))) * (1.0 - smoothstep(0.05, 0.28, tone));
    float ink = clamp(edge + (hatch1 + hatch2) * s_hatch / 100.0 * 0.8 + 0.7 * feature_line(at, 1.1 * u_resolution.y / 720.0), 0.0, 1.0);
    vec3 paper = vec3(0.95, 0.91, 0.82), pen = vec3(0.07, 0.06, 0.1);
    if (s_paper > 2.5) { paper = vec3(0.09, 0.12, 0.11); pen = vec3(0.92); }
    else if (s_paper > 1.5) { paper = vec3(0.08, 0.22, 0.55); pen = vec3(0.85, 0.92, 1.0); }
    else if (s_paper > 0.5) { paper = vec3(0.98); }
    paper *= 0.96 + 0.08 * noise(p * 0.6);
    // Fading in, patches drawn first and last.
    float n = noise(p * 0.012 / scale);
    float drawn = smoothstep(n - 0.1, n, u_amount * 1.2 - 0.1);
    return vec4(mix(c, mix(paper, pen, ink), person * drawn), 1.0);
}
