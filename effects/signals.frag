// Signals: draws each of the inputs on its own. See EFFECTS.md for what they are. Fading in, the
// drawing comes up over the picture.

vec4 signals(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (s_view < 0.5) {
        vec2 v = flow_at(uv) * vec2(u_resolution.x / u_resolution.y, 1.0);
        float speed = length(v);
        return vec4(hue(atan(v.y, v.x) / 6.2831853) * smoothstep(0.0, 0.35, speed), 1.0);
    }
    if (s_view < 1.5) return vec4(history_at(uv, 0.5), 1.0);
    if (s_view < 2.5) return vec4(plate_at(uv), 1.0);
    if (s_view < 3.5) return vec4(vec3(plate_known(uv)), 1.0);
    if (s_view < 4.5) {
        int who = who_at(uv);
        float person = smoothstep(0.4, 0.7, person_at(uv));
        vec3 tint = who < 0 ? vec3(0.5) : hue(float(who) * 0.17);
        return vec4(mix(c * 0.4, tint * (0.3 + dot(c, vec3(0.8))), person), 1.0);
    }
    if (s_view > 5.5) {
        // The ground in green where the depth agrees with it, and the horizon in red.
        vec2 size = max(u_video_rect.zw - u_video_rect.xy, vec2(1e-6));
        vec2 pic = (uv - u_video_rect.xy) / size;
        float plane = dot(vec3(pic, 1.0), u_ground.xyz);
        float on = 1.0 - smoothstep(0.015, 0.03, abs(plane - depth_at(uv)));
        vec3 shown = mix(c * 0.5, vec3(0.1, 1.0, 0.3), on * u_ground.w * 0.7);
        shown = mix(shown, vec3(1.0, 0.1, 0.1), (1.0 - smoothstep(0.0, 0.012, abs(plane))) * u_ground.w);
        float aspect = u_resolution.x / u_resolution.y;
        float d = length((pic - u_light.xy) * size * vec2(aspect, 1.0));
        shown = mix(shown, vec3(1.0, 0.9, 0.1), (1.0 - smoothstep(0.015, 0.02, d)) * (0.3 + 0.7 * u_light.z));
        if (pic.x > 0.97) shown = vec3(step(pic.y, u_ground.w));
        return vec4(shown, 1.0);
    }
    if (uv.y < 0.12) {
        int k = int(clamp(uv.x * 4.0, 0.0, 3.0));
        return vec4(u_palette[k], 1.0);
    }
    if (uv.x > 0.92) {
        float bar = uv.x > 0.973 ? u_brightness : (uv.x > 0.947 ? u_colourfulness : 0.5 + 0.5 * u_warmth);
        return vec4(vec3(step(uv.y, bar)), 1.0);
    }
    return vec4(mix(c, vec3(1.0, 0.1, 0.1), u_cut), 1.0);
}

vec4 effect(vec2 uv) {
    return vec4(mix(texture(u_video, uv).rgb, signals(uv).rgb, u_amount), 1.0);
}
