// Cyborg: on one side of the face's own coordinates the skin gives way to plates and circuits,
// lit by which way the head is turned. Fading in, the metal spreads across the face from its side.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    float on = on_face(uv);
    if (on <= 0.0) return vec4(c, 1.0);
    vec2 a = face_uv(uv);
    // -1 at the very edge, where the map's blur reaches past the face's own pixels.
    int who = max(face_there(uv), 0);
    // How far into the metal side, ragged where the skin's torn.
    float x = s_side > 0.5 ? 1.0 - a.x : a.x;
    float tear = noise(a * vec2(9.0, 22.0)) * 0.07 + noise(a * 60.0) * 0.02;
    float edge = 1.0 - s_cover / 100.0 * u_amount + tear + 0.05 * (1.0 - u_amount);
    float metal = smoothstep(edge - 0.004, edge + 0.004, x);
    float wound = smoothstep(edge - 0.035, edge, x) * (1.0 - metal);
    c = mix(c, vec3(0.16, 0.01, 0.015), wound * on);
    if (metal <= 0.0) return vec4(c, 1.0);

    // Plates, in rows of different widths.
    vec2 cells = a * vec2(7.0, 11.0);
    cells.x += hash(vec2(floor(cells.y), 3.0)) * 3.0;
    vec2 cell = floor(cells), f = fract(cells);
    float gap = smoothstep(0.0, 0.06, min(min(f.x, 1.0 - f.x), min(f.y, 1.0 - f.y)));
    float grain = 0.9 + 0.1 * noise(vec2(a.x * 300.0, a.y * 12.0));
    vec3 turn = u_face_turn[who].xyz;
    // Each plate tilts a little, so they catch the light differently.
    vec2 tilt = (vec2(hash(cell), hash(cell + 7.0)) - 0.5) * 0.5;
    float light = clamp(0.35 + 0.65 * (tilt.x + turn.x * (x - 0.5) * 3.0 + (tilt.y + turn.y) * 0.5 + 0.5), 0.0, 1.0);
    vec3 steel = mix(vec3(0.08, 0.09, 0.11), vec3(0.72, 0.76, 0.82), light * light) * grain;
    steel *= 0.2 + 0.8 * gap;
    float rivet = 1.0 - smoothstep(0.05, 0.09, length((f - vec2(0.12, 0.2)) * vec2(1.0, 0.65)));
    steel = mix(steel, vec3(0.95), rivet * 0.7 * gap);

    // Some plates are missing: circuits underneath, with light running along them.
    if (hash(cell + 13.0) > 0.68) {
        vec2 q = fract(a * vec2(32.0, 48.0));
        float trace = 1.0 - smoothstep(0.0, 0.14, min(abs(q.x - 0.5), abs(q.y - 0.5)));
        float pulse = 0.5 + 0.5 * sin(a.y * 40.0 + a.x * 24.0 - u_time * 5.0 + 6.0 * hash(cell));
        steel = vec3(0.01, 0.03, 0.05) + vec3(0.1, 0.95, 0.75) * trace * (0.15 + pulse * pulse * (1.0 + u_bass * 2.0));
    }

    // The eye on this side becomes a lens. The socket's dark and the iris burns red.
    float eye = eye_at(uv);
    steel = mix(steel, vec3(0.02), eye);
    float iris = iris_at(uv);
    float lens = u_face_open[who].x * 0.5 + u_face_open[who].y * 0.5;
    steel += vec3(1.3, 0.05, 0.03) * iris * (0.6 + 0.8 * lens + u_beat);
    steel = mix(steel, vec3(0.02), mouth_at(uv));
    c = mix(c, steel, metal * on);
    return vec4(c, 1.0);
}
