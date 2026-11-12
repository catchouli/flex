// Masks: tints the picture by what's where, for checking it and for working on an effect. Fading
// in, the tints and marks come up over the picture.
//
// s_show: 0 what's where, 1 people's skin, hair and clothes, 2 body parts, 3 who's who.

vec3 who_colour(int who) {
    return hue(float(who) * 0.37 + 0.05);
}

// A ring around a spot (xy, radius, how sure), in screen heights.
float ring(vec2 uv, vec4 spot) {
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    float d = length((uv - spot.xy) * aspect);
    float r = max(spot.z, 0.006);
    return spot.w * (1.0 - smoothstep(0.0, 0.003, abs(d - r)));
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    vec3 tint;
    float amount;
    if (s_show > 2.5) {
        int who = person_there(uv);
        tint = who >= 0 ? who_colour(who) : vec3(0.0);
        amount = who >= 0 ? 0.55 : 0.0;
        vec3 shown = mix(c * 0.6, tint, amount);
        vec2 px = 1.5 / u_resolution;
        for (int i = 0; i < u_person_count; i++) {
            if (u_person_seen[i] <= 0.0) continue;
            vec3 col = who_colour(i);
            vec4 b = u_person_box[i];
            vec2 inside = step(b.xy - px, uv) * step(uv, b.zw + px);
            vec2 inner = step(b.xy + px, uv) * step(uv, b.zw - px);
            float edge = inside.x * inside.y * (1.0 - inner.x * inner.y);
            shown = mix(shown, col, edge * u_person_seen[i]);
            shown = mix(shown, vec3(1.0), ring(uv, vec4(u_person_centre[i], 0.006, u_person_seen[i])));
            shown = mix(shown, vec3(1.0, 1.0, 0.3), ring(uv, u_person_head[i]));
            for (int n = 0; n < 2; n++) {
                shown = mix(shown, vec3(0.3, 1.0, 1.0), ring(uv, u_person_hands[i * 2 + n]));
                shown = mix(shown, vec3(1.0, 0.4, 1.0), ring(uv, u_person_feet[i * 2 + n]));
            }
        }
        return vec4(mix(c, shown, u_amount), 1.0);
    }
    if (s_show > 1.5) {
        vec4 upper = more_masks_at(uv, 3);
        float leg = leg_at(uv), foot = foot_at(uv);
        tint = upper.r * vec3(1.0, 0.9, 0.2) + upper.g * vec3(0.9, 0.2, 0.2) + upper.b * vec3(0.2, 0.8, 0.3)
            + upper.a * vec3(0.2, 0.9, 1.0) + leg * vec3(0.3, 0.3, 1.0) + foot * vec3(0.9, 0.3, 0.9);
        amount = clamp(upper.r + upper.g + upper.b + upper.a + leg + foot, 0.0, 1.0) * 0.7;
    } else if (s_show > 0.5) {
        float skin = body_skin_at(uv), hair = hair_at(uv), clothes = clothes_at(uv);
        tint = skin * vec3(1.0, 0.75, 0.6) + hair * vec3(0.55, 0.3, 0.1) + clothes * vec3(0.7, 0.25, 0.9);
        amount = clamp(skin + hair + clothes, 0.0, 1.0) * 0.7;
    } else {
        vec4 m = masks_at(uv);
        vec4 more = more_masks_at(uv, 0);
        vec4 things = more_masks_at(uv, 2);
        float window = window_at(uv);
        tint = m.r * vec3(1.0, 0.2, 0.2) + m.g * vec3(0.25, 0.45, 1.0) + m.b * vec3(0.25, 0.9, 0.9)
            + more.r * vec3(0.6, 0.45, 0.25) + more.g * vec3(0.8, 0.8, 0.5) + more.b * vec3(0.1, 0.3, 1.0)
            + more.a * vec3(0.25, 0.85, 0.25) + window * vec3(1.0, 0.85, 0.3)
            + things.r * vec3(1.0, 0.5, 0.0) + things.g * vec3(0.9, 0.3, 0.7) + things.b * vec3(0.55, 0.55, 0.65)
            + things.a * vec3(0.55, 0.3, 0.15);
        amount = clamp(m.r + m.g + m.b + more.r + more.g + more.b + more.a + window + things.r + things.g + things.b + things.a, 0.0, 1.0) * 0.55;
    }
    return vec4(mix(c, tint, amount * u_amount), 1.0);
}
