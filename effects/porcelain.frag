// Porcelain Dolls: skin turned to glazed china, in the face's own coordinates, with cracks that
// spread the longer a shot goes on. Fading in, the china creeps up the face from the chin.

vec2 random2(vec2 p) {
    return fract(sin(vec2(dot(p, vec2(127.1, 311.7)), dot(p, vec2(269.5, 183.3)))) * 43758.5453);
}

// How near a point is to a crack: the edges between cells.
float crack_distance(vec2 p) {
    vec2 n = floor(p), f = fract(p), nearest = vec2(0.0), nearest_cell = vec2(0.0);
    float best = 8.0;
    for (int j = -1; j <= 1; j++) {
        for (int i = -1; i <= 1; i++) {
            vec2 g = vec2(float(i), float(j)), o = random2(n + g), r = g + o - f;
            float d = dot(r, r);
            if (d < best) { best = d; nearest = r; nearest_cell = g; }
        }
    }
    best = 8.0;
    for (int j = -2; j <= 2; j++) {
        for (int i = -2; i <= 2; i++) {
            vec2 g = nearest_cell + vec2(float(i), float(j)), o = random2(n + g), r = g + o - f;
            if (dot(nearest - r, nearest - r) > 1e-5) best = min(best, dot(0.5 * (nearest + r), normalize(r - nearest)));
        }
    }
    return best;
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    float on = on_face(uv);
    if (on <= 0.0) return vec4(c, 1.0);
    vec2 a = face_uv(uv);
    // -1 at the very edge, where the map's blur reaches past the face's own pixels.
    int who = max(face_there(uv), 0);
    float luma = dot(c, vec3(0.3, 0.59, 0.11));
    vec3 turn = u_face_turn[who].xyz;

    // White glaze that keeps the film's light, with a shine that slides as the head turns.
    vec3 glaze = vec3(0.96, 0.93, 0.9) * (0.4 + 1.1 * pow(luma, 0.7));
    vec2 from_shine = (a - vec2(0.5 - turn.x * 0.3, 0.7 + turn.y * 0.2)) * vec2(1.0, 0.7);
    glaze += vec3(0.5) * exp(-dot(from_shine, from_shine) * 40.0);
    // Painted cheeks.
    vec2 cheek = vec2(abs(a.x - 0.5) - 0.225, a.y - 0.45);
    glaze = mix(glaze, vec3(0.95, 0.4, 0.45), 0.6 * exp(-dot(cheek, cheek) * 140.0));

    float grown = min(u_since_cut * 0.05 * s_cracks / 50.0 + u_bass * 0.04, 0.5);
    float d = crack_distance(a * 9.0 + float(who) * 3.7);
    // Cracks start from the chin and creep up the face.
    float reach = smoothstep(a.y - 0.25, a.y + 0.05, grown * 2.2);
    float crack = (1.0 - smoothstep(0.0, 0.035, d)) * reach;
    glaze = mix(glaze, vec3(0.05, 0.04, 0.05), crack);

    // How far up the face the china has crept, fading in.
    float spread = u_amount >= 1.0 ? 1.0 : smoothstep(a.y - 0.15, a.y, u_amount * 1.4 - 0.2);
    float keep = (1.0 - lips_at(uv)) * (1.0 - eye_at(uv)) * (1.0 - mouth_at(uv));
    c = mix(c, glaze, keep * on * spread);
    // Small painted lips, in a bow.
    vec2 lip = vec2(abs(a.x - 0.5), a.y - 0.305);
    float bow = 1.0 - smoothstep(0.5, 1.0, length(lip / vec2(0.075, 0.042)));
    c = mix(c, vec3(0.72, 0.03, 0.1) * (0.5 + luma), max(lips_at(uv), bow * on) * (1.0 - mouth_at(uv)) * spread);
    // Glass eyes: dark, wet, and the brows painted on thin.
    float iris = iris_at(uv) * spread;
    c = mix(c, vec3(0.03, 0.12, 0.3) + c * 0.5, iris);
    c = mix(c, c * vec3(1.15), eye_at(uv) * spread * (1.0 - iris));
    c = mix(c, vec3(0.2, 0.1, 0.08), brows_at(uv) * 0.7 * spread);
    return vec4(c, 1.0);
}
