// Night of the Living: drains people to grey-green with veins under the skin, blacks out their
// eyes and mouths, and grades everything else like an old horror film. The flicker stays gentle.
// With MoGe-2's head only skin rots; clothes turn to grimy rags and hair goes lank and grey.
// Fading in, the grade comes over the film and the rot, eyes and flicker grow with it.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 1.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float t = u_time;
    float luma = dot(c, vec3(0.3, 0.59, 0.11));
    float person = smoothstep(0.35, 0.7, person_at(uv));

    vec3 world = mix(vec3(luma), c, 0.25) * vec3(0.72, 0.8, 1.0);
    world *= 1.0 - s_gloom / 100.0 * 0.8 * (1.0 - depth_at(uv));
    if (u_amount < 1.0) world = mix(c, world, u_amount);
    // On a mesh the veins are in the skin, rather than sliding over it.
    float on = on_face(uv);
    vec2 flesh = on > 0.5 ? face_uv(uv) * 7.0 + 40.0 : p * 22.0;
    float ridge = abs(fbm(flesh) - 0.5);
    float veins = (1.0 - smoothstep(0.0, 0.03, ridge)) * s_rot / 100.0;
    float blotch = fbm((on > 0.5 ? face_uv(uv) * 2.4 + 40.0 : p * 7.0) + 3.0);
    vec3 skin = pow(luma, 1.25) * vec3(0.72, 0.9, 0.62) + vec3(0.03, 0.06, 0.02);
    skin *= mix(1.0, 0.45 + 1.0 * blotch, s_rot / 100.0);
    skin = mix(skin, vec3(0.1, 0.04, 0.12), veins * 0.75);
    float rotting = person;
    if (u_has_more_masks > 0.5) {
        float bare = body_skin_at(uv), hair = hair_at(uv), cloth = clothes_at(uv);
        float all_of = max(bare + hair + cloth, 1e-3);
        rotting = person * bare / all_of;
        vec3 rags = mix(vec3(luma), c, 0.3) * vec3(0.82, 0.8, 0.68) * mix(1.0, 0.55 + 0.7 * fbm(p * 11.0 + 7.0), s_rot / 100.0);
        vec3 lank = vec3(pow(luma, 1.1)) * vec3(0.8, 0.82, 0.78);
        world = mix(world, rags, person * cloth / all_of * u_amount);
        world = mix(world, lank, person * hair / all_of * s_rot / 100.0 * u_amount);
    }
    c = mix(world, skin, rotting * mix(0.5, 1.0, s_rot / 100.0) * u_amount);

    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i] * u_amount;
        if (seen <= 0.0) continue;
        vec2 right = face_point(i, 0) * aspect, left = face_point(i, 1) * aspect;
        vec2 mouth = 0.5 * (face_point(i, 3) + face_point(i, 4)) * aspect;
        float spread = max(length(left - right), 0.25 * (u_faces[i].w - u_faces[i].y));
        for (int e = 0; e < 2; e++) {
            vec2 eye = meshed(i) ? iris_of(i, e) * aspect : (e == 0 ? right : left);
            float d = length((p - eye) / vec2(1.0, 0.8)) / spread;
            c = mix(c, vec3(0.0), (1.0 - smoothstep(0.18, 0.42, d)) * seen);
            float awake = meshed(i) ? smoothstep(0.05, 0.3, e == 0 ? u_face_open[i].x : u_face_open[i].y) : 1.0;
            c += vec3(0.7, 0.75, 0.45) * (1.0 - smoothstep(0.02, 0.08, d)) * seen * 0.8 * awake;
        }
        float m = length((p - mouth) / vec2(1.2, 0.55)) / spread;
        if (meshed(i)) {
            c = mix(c, c * vec3(0.3, 0.2, 0.3), lips_at(uv) * u_amount);
            c = mix(c, vec3(0.02, 0.0, 0.0), mouth_at(uv) * u_amount);
        } else {
            c = mix(c, vec3(0.02, 0.0, 0.0), (1.0 - smoothstep(0.3, 0.55, m)) * seen * 0.85);
        }
    }

    float flick = noise(vec2(t * 9.0, 1.0)) * 0.6 + noise(vec2(t * 23.0, 4.0)) * 0.4;
    // Thunder, screams, alarms and scary music make the light stutter harder and more often.
    float dread = clamp(u_sound_thunder + u_sound_shouting + u_sound_alarms + 0.7 * u_sound_scary_music, 0.0, 1.0);
    c *= 1.0 - s_flicker / 100.0 * u_amount * 0.45 * (1.0 + 0.75 * dread) * smoothstep(0.35 - 0.2 * dread, 0.8 - 0.25 * dread, flick);
    // Thunder brings flashes of lightning too.
    c *= 1.0 + s_flicker / 100.0 * u_amount * 1.2 * u_sound_thunder * smoothstep(0.62, 0.8, noise(vec2(t * 14.0, 7.0)));
    vec2 off = uv - 0.5;
    c *= 1.0 - dot(off, off) * 1.3 * u_amount;
    c += (hash(uv * 311.0 + fract(t) * 91.0) - 0.5) * s_grain / 100.0 * u_amount * 0.22;
    return vec4(c, 1.0);
}
