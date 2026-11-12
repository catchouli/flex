// Thermal Vision: heat made up from brightness, nearness, people and faces, through a false-colour
// palette, with three red dots hunting each face. With MoGe-2's head, bodies aren't evenly warm
// (heads and middles hottest, hands and feet cooler, clothes cooler than skin), animals are as
// warm as people, vehicles a little warm, and sky and water cold. Fading in, the palette comes
// over the film and the dots come up.

vec3 ramp(float t, vec3 a, vec3 b, vec3 c, vec3 d, vec3 e) {
    t = clamp(t, 0.0, 1.0) * 4.0;
    if (t < 1.0) return mix(a, b, t);
    if (t < 2.0) return mix(b, c, t - 1.0);
    if (t < 3.0) return mix(c, d, t - 2.0);
    return mix(d, e, t - 3.0);
}

vec3 colours(float h) {
    if (s_palette < 0.5) return ramp(h, vec3(0.02, 0.0, 0.12), vec3(0.42, 0.0, 0.55), vec3(0.9, 0.2, 0.2), vec3(1.0, 0.72, 0.05), vec3(1.0, 1.0, 0.9));
    if (s_palette < 1.5) return ramp(h, vec3(0.0, 0.0, 0.5), vec3(0.0, 0.75, 0.85), vec3(0.1, 0.9, 0.1), vec3(1.0, 0.95, 0.0), vec3(1.0, 0.1, 0.0));
    return vec3(h * h * 0.9 + h * 0.1);
}

vec4 effect(vec2 uv) {
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(0.0, 0.0, 0.0, 1.0);
    vec2 pixel = 2.0 / u_resolution;
    vec3 soft = (texture(u_video, uv + pixel).rgb + texture(u_video, uv - pixel).rgb + texture(u_video, uv + pixel * vec2(1.0, -1.0)).rgb + texture(u_video, uv - pixel * vec2(1.0, -1.0)).rgb) * 0.25;
    float luma = dot(soft, vec3(0.3, 0.59, 0.11));
    float people = s_people / 100.0;
    float person = smoothstep(0.3, 0.7, person_at(uv));
    float face = face_at(uv);
    float h = luma * 0.3 + depth_at(uv) * 0.14;
    float warmth = 0.5;
    if (u_has_more_masks > 0.5) {
        vec4 upper = more_masks_at(uv, 3);
        vec4 lower = more_masks_at(uv, 4);
        float body = upper.r + upper.g + upper.b + upper.a + lower.r + lower.g;
        if (body > 0.05) {
            // Head, torso, arm, hand, leg, foot.
            warmth = (upper.r * 0.62 + upper.g * 0.56 + upper.b * 0.46 + upper.a * 0.4 + lower.r * 0.44 + lower.g * 0.3) / body;
        }
        warmth += (body_skin_at(uv) - clothes_at(uv)) * 0.06;
        float animal = smoothstep(0.3, 0.7, animal_at(uv));
        h = mix(h, 0.52 + luma * 0.3, animal * people);
        h = mix(h, 0.36 + luma * 0.3, smoothstep(0.3, 0.7, vehicle_at(uv)) * people * 0.7);
        h = mix(h, h * 0.4, smoothstep(0.3, 0.7, sky_at(uv) + water_at(uv)));
    }
    h = mix(h, warmth + luma * 0.3, person * people);
    h += face * 0.2 * people;
    // Real faces aren't evenly warm: noses run cold, and eyes and open mouths run hot.
    h += (eye_at(uv) * 0.1 + mouth_at(uv) * 0.2 + lips_at(uv) * 0.05 - nose_at(uv) * 0.16) * people;
    h += u_bass * 0.04 + (noise(uv * u_resolution * 0.4 + u_time * 37.0) - 0.5) * 0.02;
    vec3 c = colours(h);
    if (u_amount < 1.0) c = mix(texture(u_video, uv).rgb, c, u_amount);
    if (s_sights < 0.5) {
        vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
        vec2 p = uv * aspect;
        float dots = 0.0;
        for (int i = 0; i < u_face_count; i++) {
            vec2 centre = 0.5 * (u_faces[i].xy + u_faces[i].zw) * aspect;
            float size = (u_faces[i].w - u_faces[i].y) * 0.14;
            for (int n = 0; n < 3; n++) {
                float a = float(n) * 2.0943951 + 1.5707963;
                dots = max(dots, u_face_seen[i] * (1.0 - smoothstep(0.006, 0.012, length(p - centre - vec2(cos(a), sin(a)) * size))));
            }
        }
        c = mix(c, vec3(1.0, 0.0, 0.0), dots * u_amount);
    }
    return vec4(c, 1.0);
}
