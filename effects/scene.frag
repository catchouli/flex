// Shot and Scene: a test effect for scene = true (see scene.toml). Bars at the top left, from the
// top: shot size, handheld, pace, then action, dialogue, quiet and montage. The white dot in the
// middle follows the camera's pan and tilt. Fading in, the bars and the dot grow from nothing.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    const vec3 colours[7] = vec3[7](vec3(1.0, 1.0, 0.3), vec3(0.6, 0.6, 1.0), vec3(1.0), vec3(1.0, 0.3, 0.2), vec3(0.3, 1.0, 0.5), vec3(0.3, 0.6, 1.0), vec3(1.0, 0.4, 1.0));
    float bars[7] = float[7](u_shot_size, u_handheld, clamp(u_pace / 60.0, 0.0, 1.0), u_scene.x, u_scene.y, u_scene.z, u_scene.w);
    for (int i = 0; i < 7; i++) {
        float y = 0.9 - float(i) * 0.05;
        if (uv.x > 0.03 && uv.x < 0.03 + bars[i] * 0.25 * u_amount && abs(uv.y - y) < 0.018) c = mix(c, colours[i], 0.85);
    }
    if (distance(uv, vec2(0.5) + u_camera.xy * 0.25) < 0.012 * u_amount) c = vec3(1.0);
    return vec4(c, 1.0);
}
