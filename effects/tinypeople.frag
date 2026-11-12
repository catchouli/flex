// Tiny People: the room behind people comes from what Flex has seen of it. Then each person with
// a face is drawn smaller, standing on the lowest point of their own body. Fading in, people
// shrink from their own size as the room shows through behind them.

float someone(vec2 uv) {
    float most = person_at(uv);
    for (int i = 0; i < 6; i++) {
        float a = float(i) * 1.0471976;
        most = max(most, person_at(uv + vec2(cos(a), sin(a)) * vec2(0.014 * u_resolution.y / u_resolution.x, 0.014)));
    }
    return most;
}

vec4 effect(vec2 uv) {
    vec3 video = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(video, 1.0);
    float clear = 1.0 - smoothstep(0.08, 0.3, someone(uv));
    if (u_amount < 1.0) clear = mix(1.0, clear, u_amount);
    if (u_has_plate < 0.5) return vec4(video, 1.0);
    vec3 c = mix(plate_at(uv), video, clear);
    float scale = s_size / 100.0 * (1.0 + 0.12 * u_beat * s_react / 100.0);
    for (int i = 0; i < u_face_count; i++) {
        if (u_face_seen[i] <= 0.0) continue;
        vec2 chin = vec2(0.5 * (u_faces[i].x + u_faces[i].z), u_faces[i].y);
        // Down from the chin to where this person ends.
        float bottom = chin.y;
        for (int s = 1; s <= 10; s++) {
            float y = chin.y - float(s) * 0.07;
            if (y < u_video_rect.y || person_at(vec2(chin.x, y)) < 0.4) break;
            bottom = y;
        }
        vec2 stand = vec2(chin.x, bottom);
        vec2 from = stand + (uv - stand) / mix(1.0, scale, u_face_seen[i] * u_amount);
        if (from != clamp(from, u_video_rect.xy, u_video_rect.zw)) continue;
        // Keep to this person, when there are several.
        bool theirs = true;
        for (int j = 0; j < u_face_count; j++) {
            if (j != i && abs(from.x - 0.5 * (u_faces[j].x + u_faces[j].z)) < abs(from.x - chin.x)) theirs = false;
        }
        float body = theirs ? smoothstep(0.4, 0.7, person_at(from)) : 0.0;
        if (body > 0.0) c = mix(c, texture(u_video, from).rgb, body);
    }
    return vec4(c, 1.0);
}
