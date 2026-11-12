// Googly Eyes: each pupil is a little physics simulation. It stays put as the head moves, falls,
// and bounces off the rim, and bangs and explosions rattle it. Its state is six numbers an eye in the
// alpha of the bottom row of u_prev. Fading in, the eyes swell up from nothing; the pupils' physics
// carries on as it is.

vec2 kept(int at) {
    return vec2(texelFetch(u_prev, ivec2(at, 0), 0).a, texelFetch(u_prev, ivec2(at + 1, 0), 0).a);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    ivec2 pixel = ivec2(gl_FragCoord.xy);
    float alpha = 1.0;
    float rattle = clamp(u_sound_impacts + u_sound_explosions, 0.0, 1.0);
    for (int i = 0; i < MAX_FACES; i++) {
        for (int e = 0; e < 2; e++) {
            int base = (i * 2 + e) * 6;
            bool here = i < u_face_count && u_face_seen[i] > 0.5 && u_frame > 2;
            vec2 at = vec2(0.0);
            float radius = 0.01;
            Face f;
            if (i < u_face_count) {
                f = face_frame(i, aspect);
                at = e == 0 ? f.right : f.left;
                radius = max(f.spread * 0.36 * s_size / 100.0, 0.002);
            }
            vec2 offset = kept(base), speed = kept(base + 2), last = kept(base + 4);
            if (!here || length(at - last) > radius * 4.0) { offset = vec2(0.0, -0.5); speed = vec2(0.0); last = at; }
            if (pixel.y == 0 && pixel.x >= base && pixel.x < base + 6) {
                // The eye moved but the pupil's still where it was.
                vec2 o = offset - (at - last) / radius * 0.8;
                vec2 v = speed * 0.985 + vec2(0.0, -0.02);
                v += vec2(sin(u_time * 31.0 + float(base)), cos(u_time * 23.0 + float(base))) * u_beat * s_react / 100.0 * 0.05;
                // Bangs shake them faster and harder than the beat does.
                v += vec2(sin(u_time * 67.0 + float(base) * 1.3), cos(u_time * 53.0 + float(base) * 0.7)) * rattle * s_react / 100.0 * 0.08;
                o += v;
                float out_by = length(o);
                if (out_by > 0.62) {
                    vec2 n = o / out_by;
                    o = n * 0.62;
                    v -= n * dot(v, n) * (1.0 + s_bounce / 100.0 * 0.8);
                }
                int k = pixel.x - base;
                if (k < 2) alpha = o[k]; else if (k < 4) alpha = v[k - 2]; else alpha = at[k - 4];
            }
            if (!here || u_amount <= 0.0) continue;
            // Drawn at their size by u_amount, though the pupil moves as it would at full size.
            float drawn = radius * u_amount;
            float d = length(p - at) / drawn;
            if (d > 1.15) continue;
            vec3 eye = mix(vec3(0.0), vec3(0.97, 0.97, 0.95) * (1.0 - 0.12 * d * d), 1.0 - smoothstep(0.93, 0.98, d));
            float pupil = length((p - at) / drawn - offset);
            eye = mix(eye, vec3(0.02), 1.0 - smoothstep(0.34, 0.38, pupil));
            eye += vec3(0.9) * (1.0 - smoothstep(0.08, 0.12, length((p - at) / drawn - vec2(-0.32, 0.38))));
            // Blinks are a skin-coloured lid coming down over the plastic.
            if (meshed(i)) {
                float open = e == 0 ? u_face_open[i].x : u_face_open[i].y;
                float lid_at = mix(-1.05, 1.1, smoothstep(0.03, 0.45, open));
                if (dot(p - at, f.up) / drawn > lid_at) {
                    vec3 skin = texture(u_video, (at + f.up * drawn * 1.7) / aspect).rgb;
                    eye = mix(skin, skin * 0.55, smoothstep(0.75, 1.0, d));
                }
            }
            c = mix(c, eye, 1.0 - smoothstep(1.0, 1.08, d));
        }
    }
    return vec4(c, alpha);
}
