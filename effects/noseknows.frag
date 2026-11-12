// Follow Your Nose: a cone of light from each nose, along the way the head points. Fading in, the
// room dims and the beams reach out from the noses.

vec3 soft_hue(float h) {
    return 0.55 + 0.45 * cos(6.2832 * (h + vec3(0.0, 0.33, 0.67)));
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb * mix(1.0, 0.8, u_amount);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    for (int i = 0; i < u_face_count; i++) {
        if (u_face_turn[i].w < 0.5) continue;
        vec2 size = (u_faces[i].zw - u_faces[i].xy) * aspect;
        vec2 from = face_point(i, 2) * aspect;
        // Facing the viewer there's nowhere for the beam to go on screen but a bright spot.
        vec2 way = u_face_turn[i].xy;
        float sideways = length(way);
        vec2 along = way / max(sideways, 1e-3);
        vec2 p = uv * aspect - from;
        float x = dot(p, along), y = dot(p, vec2(-along.y, along.x));
        float reach = (0.25 + 1.5 * sideways) * s_length * u_amount;
        float spread = (size.y * 0.08 + x * 0.16) * s_width;
        float beam = x > 0.0 && reach > 0.0 ? (1.0 - smoothstep(spread * 0.5, spread, abs(y))) * (1.0 - smoothstep(reach * 0.5, reach, x)) : 0.0;
        float spot = exp(-dot(p, p) / max(size.y * size.y * 0.02, 1e-6));
        c += soft_hue(float(i) * 0.31 + u_time * 0.05) * (beam * 0.7 + spot * 0.6 * u_amount) * u_face_seen[i];
    }
    return vec4(c, 1.0);
}
