// Love Beams: hearts leave each eye along the way the face is pointing, growing and fading as they
// go. Straight at the camera, they float upwards instead. Singing, laughter and sweet music send
// out more and bigger hearts; crying and sad music turn them blue, and they crack and sink.
// Fading in, the hearts grow from nothing.

// Which way a face points on screen: length 0 straight at the camera, nearing 1 side on.
vec2 facing(Face f) {
    vec2 aim = vec2(dot(f.nose - 0.5 * (f.eyes + f.mouth), f.across) / f.height, 0.53 - dot(f.eyes - f.nose, f.up) / f.height) / 0.5;
    aim /= max(length(aim), 1.0);
    return f.across * aim.x + f.up * aim.y;
}

float heart(vec2 q) {
    q *= 0.85;
    q.y += 0.1;
    float a = q.x * q.x + q.y * q.y - 1.0;
    return a * a * a - q.x * q.x * q.y * q.y * q.y;
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    vec3 light = vec3(0.0);
    float t = u_time * s_speed / 100.0;
    float count = max(s_hearts, 1.0);
    float react = s_react / 50.0;
    float love = clamp((u_sound_singing + u_sound_laughter + u_sound_tender_music + u_sound_happy_music) * react, 0.0, 1.0);
    float sad = clamp((u_sound_crying + u_sound_sad_music) * react, 0.0, 1.0);
    vec3 colour = mix(vec3(1.0, 0.16, 0.42), vec3(0.25, 0.45, 1.0), sad);
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i] * step(1e-3, u_amount);
        if (seen <= 0.0) continue;
        Face f = face_frame(i, aspect);
        vec2 aim = u_face_turn[i].w > 0.5 ? u_face_turn[i].xy : facing(f);
        vec2 d = normalize(mix(f.up, normalize(aim + vec2(0.0, 1e-5)), smoothstep(0.1, 0.3, length(aim))));
        for (int e = 0; e < 2; e++) {
            vec2 eye = meshed(i) ? iris_of(i, e) * aspect : (e == 0 ? f.right : f.left);
            // The second half are the extra hearts, in between the others, faded in with the love.
            int n = int(count);
            for (int h = 0; h < n * 2; h++) {
                bool extra = h >= n;
                if (extra && love <= 0.0) break;
                float slot = extra ? float(h - n) + 0.5 : float(h);
                float along = fract(t * 0.3 + slot / count + float(i) * 0.37 + float(e) * 0.5 / count);
                vec2 side = vec2(-d.y, d.x);
                vec2 at = eye + d * along * 1.1 + side * sin(along * 14.0 + slot * 2.0) * 0.03;
                // Sad hearts sink as they go.
                at.y -= sad * along * along * 0.35;
                float size = f.spread * (0.14 + 0.5 * along) * s_size / 100.0 * (1.0 + 0.4 * u_beat * s_react / 100.0) * (1.0 + 0.35 * love) * max(u_amount, 1e-3);
                vec2 q = (p - at) / size;
                if (dot(q, q) > 3.0) continue;
                // A sad heart breaks down a zigzag, its halves drifting apart. Unbroken, the two halves
                // add up to the whole heart.
                float crack = 0.12 * (abs(fract(q.y * 2.2) * 2.0 - 1.0) - 0.5);
                float gap = 0.3 * sad * along;
                vec2 ql = q + vec2(gap, 0.0), qr = q - vec2(gap, 0.0);
                float halves = (1.0 - smoothstep(-0.03, 0.0, heart(ql))) * (1.0 - smoothstep(-0.02, 0.02, ql.x - crack))
                    + (1.0 - smoothstep(-0.03, 0.0, heart(qr))) * smoothstep(-0.02, 0.02, qr.x - crack);
                float inside = min(halves, 1.0) * (1.0 - smoothstep(0.04, 0.12, depth_at(uv) - depth_at(eye / aspect)));
                if (extra) inside *= love;
                light = max(light, colour * inside * seen * (1.0 - smoothstep(0.55, 1.0, along)));
            }
        }
    }
    float amount = max(light.r, max(light.g, light.b));
    return vec4(mix(c, light / max(amount, 1e-3), amount), 1.0);
}
