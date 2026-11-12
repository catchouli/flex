// Voxel World: looks down the column from the nearest row of blocks up to this one. The first block
// tall enough to cover this pixel is what's seen, either its side or its top. Fading in, the
// blocks grow from single pixels and rise out of the flat picture.

vec4 effect(vec2 uv) {
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(0.0, 0.0, 0.0, 1.0);
    float size = s_size * u_resolution.y / 720.0;
    if (u_amount < 1.0) size = mix(1.0, size, u_amount);
    vec2 pixels = uv * u_resolution;
    vec2 cell = floor(pixels / size);
    vec2 inside = fract(pixels / size);
    float tallest = 10.0 * s_height / 100.0 * u_amount * (1.0 + 0.4 * u_bass * s_react / 100.0);
    int reach = int(ceil(tallest)) + 1;
    for (int k = reach; k >= 0; k--) {
        vec2 block = vec2(cell.x, cell.y - float(k));
        vec2 at = (block + 0.5) * size / u_resolution;
        if (at != clamp(at, u_video_rect.xy, u_video_rect.zw)) continue;
        float rise = floor(depth_at(at) * tallest);
        float y = float(k) + inside.y;
        if (y > rise + 1.0) continue;
        vec3 c = texture(u_video, at).rgb;
        if (y >= rise) {
            // The top, lit.
            vec2 top = vec2(inside.x, y - rise);
            c *= mix(1.0, 1.08 - 0.25 * (1.0 - smoothstep(0.0, 0.08, min(min(top.x, 1.0 - top.x), min(top.y, 1.0 - top.y)))), u_amount);
            if (s_studs > 0.0) {
                float d = length(top - 0.5);
                float stud = (1.0 - smoothstep(0.26, 0.3, d)) * s_studs / 100.0 * u_amount;
                c *= 1.0 + stud * (0.25 * (top.y - top.x) + 0.1);
            }
        } else {
            // A side, darker, in courses.
            c *= 0.32 + 0.16 * step(0.1, fract(y));
        }
        return vec4(c, 1.0);
    }
    return vec4(0.0, 0.0, 0.0, 1.0);
}
