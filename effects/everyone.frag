// Everyone Is Them: finds the biggest face and copies it over all the others. Fading in, the copy
// spreads out over each face from its middle.

// Follows the mesh, so eyes land on eyes and the face turns the way the one it's on is turned.
vec3 copied_on_mesh(vec3 c, vec2 uv, int to, int from, float seen, vec3 match) {
    if (face_there(uv) != to) return c;
    vec2 a = face_uv(uv);
    vec4 skin = face_texture(from, a);
    float feather = 1.0 - smoothstep(0.82, 0.98, length((a - vec2(0.5, 0.47)) / vec2(0.5, 0.45)) / max(u_amount, 1e-4));
    return mix(c, skin.rgb * match, on_face(uv) * skin.a * feather * seen * step(1e-4, u_amount));
}

vec3 copied(vec3 c, vec2 p, Face to, Face from, float seen, vec2 aspect) {
    vec2 to_mid = 0.5 * (to.eyes + to.mouth), from_mid = 0.5 * (from.eyes + from.mouth);
    float amount = oval(p, to_mid, to, vec2(to.spread * 1.3, to.height * 1.5) * max(u_amount, 1e-4)) * seen * step(1e-4, u_amount);
    if (amount <= 0.0) return c;
    vec2 rel = p - to_mid;
    vec2 l = vec2(dot(rel, to.across) / to.spread, dot(rel, to.up) / to.height);
    vec2 src = from_mid + from.across * l.x * from.spread + from.up * l.y * from.height;
    // Matched to the light on the face it's going on.
    vec3 here = texture(u_video, to.nose / aspect).rgb, there = texture(u_video, from.nose / aspect).rgb;
    float match = clamp(dot(here, vec3(0.33)) / max(dot(there, vec3(0.33)), 0.02), 0.6, 1.6);
    return mix(c, texture(u_video, src / aspect).rgb * match, amount);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    int biggest = -1;
    float most = 0.0;
    for (int i = 0; i < u_face_count; i++) {
        float h = u_faces[i].w - u_faces[i].y;
        if (u_face_seen[i] > 0.02 && h > most) { most = h; biggest = i; }
    }
    if (biggest < 0) return vec4(c, 1.0);
    Face them = face_frame(biggest, aspect);
    for (int i = 0; i < u_face_count; i++) {
        if (i == biggest || u_face_seen[i] <= 0.02) continue;
        float seen = min(u_face_seen[i], u_face_seen[biggest]);
        if (meshed(i) && meshed(biggest)) {
            float light = dot(face_texture(i, vec2(0.5, 0.78)).rgb, vec3(0.33)) / max(dot(face_texture(biggest, vec2(0.5, 0.78)).rgb, vec3(0.33)), 0.02);
            c = copied_on_mesh(c, uv, i, biggest, seen, vec3(clamp(light, 0.6, 1.6)));
        } else {
            c = copied(c, p, face_frame(i, aspect), them, seen, aspect);
        }
    }
    return vec4(c, 1.0);
}
