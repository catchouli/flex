// Moustache Everyone: a shape worked out in the face's own coordinates, with a curl at each end. Fading in,
// the moustache grows from a point under the nose.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    vec3 tones[4] = vec3[4](vec3(0.03), vec3(0.2, 0.1, 0.04), vec3(0.62, 0.27, 0.05), vec3(0.6));
    vec3 tone = tones[int(s_colour + 0.5)];
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i];
        if (seen <= 0.0 || u_amount <= 0.0) continue;
        Face f = face_frame(i, aspect);
        vec2 at = mix(f.nose, f.mouth, 0.52);
        float w = f.spread * 0.72 * s_size / 100.0 * u_amount;
        vec2 across = f.across, up = f.up;
        if (meshed(i)) {
            // Pinned between nose and lip, so it rides on the mouth and narrows as the head turns.
            vec2 under_nose = mesh_point(i, 2).xy * aspect, top_lip = mesh_point(i, 0).xy * aspect;
            vec2 corners = (mesh_point(i, 291).xy - mesh_point(i, 61).xy) * aspect;
            at = mix(under_nose, top_lip, 0.55);
            w = length(corners) * 1.1 * s_size / 100.0 * u_amount;
            across = normalize(corners + vec2(1e-6, 0.0));
            up = vec2(-across.y, across.x);
        }
        vec2 rel = p - at;
        float x = dot(rel, across) / w, y = dot(rel, up) / w;
        float side = abs(x);
        float middle = -0.14 + 0.42 * side * side;
        float thick = 0.24 * pow(max(1.0 - side, 0.0), 0.75) + 0.02;
        float hair = (1.0 - smoothstep(thick - 0.02, thick + 0.02, abs(y - middle))) * (1.0 - smoothstep(0.98, 1.02, side));
        // The curl, turning in on the beat.
        float twirl = 0.8 * u_beat * s_react / 100.0;
        vec2 from_curl = vec2(side - 1.0, y - 0.28 - 0.14);
        float r = length(from_curl);
        float a = atan(from_curl.y, from_curl.x) + twirl;
        float arc = (1.0 - smoothstep(0.03, 0.05, abs(r - (0.09 + 0.05 * a / 3.14159)))) * (1.0 - smoothstep(0.26, 0.3, r));
        float amount = max(hair, arc) * seen;
        if (amount > 0.0) {
            float strands = 0.75 + 0.5 * noise(vec2(x * 60.0, (y - middle) * 12.0));
            c = mix(c, tone * strands + 0.02, amount);
        }
    }
    return vec4(c, 1.0);
}
