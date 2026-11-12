// Third Eye: copies each face's right eye, turned upright, to the middle of the forehead, with
// rings of light rolling out from it. Bangs, explosions and screams make it fly wide open. Fading
// in, the eye grows from a point and the rings brighten.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    if (u_amount <= 0.0) return vec4(c, 1.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    vec3 light = vec3(0.0);
    float startle = clamp(u_sound_explosions + u_sound_impacts + u_sound_shouting, 0.0, 1.0);
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i];
        if (seen <= 0.0) continue;
        vec2 right = face_point(i, 0) * aspect;
        vec2 left = face_point(i, 1) * aspect;
        vec2 mouth = 0.5 * (face_point(i, 3) + face_point(i, 4)) * aspect;
        vec2 eyes = 0.5 * (right + left);
        vec2 up = eyes - mouth;
        vec2 brow = meshed(i) ? mesh_point(i, 151).xy * aspect : eyes + up * 0.72;
        float across = max(length(left - right), 1e-3);
        vec2 side = (left - right) / across;
        vec2 top = vec2(-side.y, side.x) * sign(dot(vec2(-side.y, side.x), up));
        vec2 rel = p - brow;
        // Upright: along the forehead is up and down in the source eye.
        vec2 local = vec2(dot(rel, top), dot(rel, side)) / (across * s_size / 100.0 * u_amount);
        float open = 0.45 + 0.55 * clamp(u_beat + startle + 0.5 + 0.5 * sin(u_time * 0.9 + float(i)), 0.0, 1.0) + 0.3 * startle;
        // It blinks when they blink, unless something's just startled it.
        if (meshed(i)) open = mix(0.06, open, max(smoothstep(0.05, 0.5, max(u_face_open[i].x, u_face_open[i].y)), startle));
        float shape = length(local / vec2(0.34, 0.2 * open));
        float mask = (1.0 - smoothstep(0.8, 1.0, shape)) * seen;
        if (mask > 0.0) {
            vec2 source = (right + side * local.x * across * 0.55 + top * local.y * across * 0.55 / open) / aspect;
            vec3 eye = texture(u_video, source).rgb * 1.25;
            c = mix(c, eye, mask);
        }
        float r = length(rel) / across;
        float rings = sin(r * 14.0 - u_time * 5.0 - 6.0 * u_bass) * 0.5 + 0.5;
        light += hue(r * 0.5 - u_time * 0.2 + float(i) * 0.3) * pow(rings, 3.0) * exp(-r * 0.75) * seen * s_aura / 100.0 * u_amount * (0.5 + 1.2 * max(u_beat, startle));
        float rim = shape - 1.0;
        light += vec3(1.0, 0.8, 0.5) * exp(-rim * rim * 22.0) * seen * 0.9;
    }
    return vec4(c + light, 1.0);
}
