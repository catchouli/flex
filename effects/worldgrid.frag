// World Grid: a test effect for camera = true (see worldgrid.toml). world_at(uv) says where a point
// on screen is in the shot's world; a lattice drawn there should stay stuck to the walls and floor
// as the camera moves, and leave people alone. Fading in, the lattice comes up.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (u_world_ready <= 0.0) return vec4(c, 1.0);
    vec3 at = world_at(uv) / 0.2;
    vec3 lines = abs(fract(at - 0.5) - 0.5) / max(fwidth(at), vec3(1e-4));
    float line = 1.0 - clamp(min(lines.x, min(lines.y, lines.z)), 0.0, 1.0);
    // Far away the lines crowd together and depth is less sure of itself.
    line *= 1.0 - smoothstep(0.15, 0.5, length(fwidth(at)));
    vec3 colour = 0.5 + 0.5 * cos(floor(at) * 1.3 + vec3(0.0, 2.1, 4.2));
    return vec4(mix(c, colour, line * u_world_ready * u_amount * (1.0 - person_at(uv)) * 0.9), 1.0);
}
