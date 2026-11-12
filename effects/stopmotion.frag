// Stop Motion People: people are held on one frame for a moment, then jump to the next pose,
// while the room plays on smoothly. The held people are kept in u_prev, with how much of each
// pixel is them in its alpha. Where someone has moved on from, the room shows through, from what's
// been seen behind them (plate_at), so they don't leave a copy of themselves. Fading in, the poses
// are held for longer and longer, from every frame to the set rate.

float hash(float n) {
    return fract(sin(n * 12.9898) * 43758.5453);
}

float person(vec2 uv) {
    return smoothstep(0.3, 0.65, person_at(uv));
}

vec4 effect(vec2 uv) {
    vec3 video = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(video, 0.0);
    float fps = s_fps / max(u_amount, 0.01);
    float pose = floor(u_video_time * fps);
    bool next_pose = pose != floor((u_video_time - u_dt) * fps) || u_frame < 2 || u_cut > 0.9 || u_amount <= 0.0;
    // The room as it is now, and behind wherever someone is now.
    vec3 room = mix(video, plate_at(uv), person(uv) * smoothstep(0.2, 0.7, plate_known(uv)) * u_amount);
    if (next_pose) {
        // Each pose sits a little differently, and the light flickers, as in handmade animation.
        float wobble = s_wobble / 100.0 * u_amount;
        vec2 nudge = (vec2(hash(pose), hash(pose + 7.0)) - 0.5) * 0.004 * wobble;
        float flicker = 1.0 + (hash(pose + 3.0) - 0.5) * 0.08 * wobble;
        float held = person(uv - nudge);
        return vec4(mix(room, texture(u_video, uv - nudge).rgb * flicker, held), held);
    }
    vec4 before = texture(u_prev, uv);
    return vec4(mix(room, before.rgb, before.a), before.a);
}
