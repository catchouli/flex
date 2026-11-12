// Drive-In: the picture so far, wrapped around each face using the face's own coordinates, so it
// bends over the nose and holds on as the head turns. It's drawn from the last frame shown, so
// faces in the picture on the face have pictures on them too. With MoGe-2's head the rest of
// people's skin shows it too, fitted to each person. Fading in, the screen brightens up on them.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    float face = on_face(uv);
    float on = face * (1.0 - eye_at(uv)) * (1.0 - lips_at(uv)) * (1.0 - mouth_at(uv));
    // Arms, hands and the rest, with the film fitted to the whole person.
    int who = face > 0.5 ? -1 : person_there(uv);
    float body = who >= 0 ? smoothstep(0.35, 0.65, body_skin_at(uv)) * (1.0 - face) : 0.0;
    on = max(on, body);
    if (on <= 0.0) return vec4(c, 1.0);
    vec2 a = face_uv(uv);
    if (body > 0.0) {
        vec4 b = u_person_box[who];
        a = (uv - b.xy) / max(b.zw - b.xy, vec2(1e-4));
    }
    // Fitted to the face, film-shaped, with its centre between the eyes and nose.
    vec2 q = (a - vec2(0.5, 0.48)) / vec2(0.5, 0.46);
    float edge = max(abs(q.x), abs(q.y));
    vec2 at = q * 0.4 + 0.5;
    vec3 screen = texture(u_prev, mix(u_video_rect.xy, u_video_rect.zw, clamp(at, 0.0, 1.0))).rgb;
    float luma = dot(c, vec3(0.3, 0.59, 0.11));
    // It's still lit by the room, and it's a cheap old screen.
    screen = pow(screen, vec3(0.75)) * (0.95 + luma * 1.1 + 0.25 * u_beat);
    screen *= 0.86 + 0.14 * sin(a.y * 500.0);
    float within = 1.0 - smoothstep(0.97, 1.05, edge);
    c = mix(c, mix(vec3(0.03), screen, within), on * s_bright / 100.0 * u_amount);
    return vec4(c, 1.0);
}
