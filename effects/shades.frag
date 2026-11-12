// Deal With It: chunky pixel sunglasses, on a grid that turns with the face. When each face turned
// up is kept in the alpha of the bottom row, to time the drop. Fading in, the shades drop in from
// above the picture; the timing's remembered as it is.

float recalled(int x) {
    return texelFetch(u_prev, ivec2(x, 0), 0).a;
}

float remember(float t, int x) {
    if (x == 0) return mod(floor(t), 1024.0);
    if (x == 1) return floor(t / 1024.0);
    return fract(t);
}

// 0 clear, 1 black, 2 a white glint.
int shades(vec2 at) {
    int x = int(floor(at.x)), y = int(floor(at.y));
    int side = x < 0 ? -1 - x : x;
    if (y == 1) return side <= 12 ? 1 : 0;
    if (y > 1 || y < -3) return 0;
    if (side < 2) return y == 0 ? 1 : 0;
    int low = y == -3 ? 4 : (y == -2 ? 3 : 2);
    int high = y == -3 ? 9 : (y == -2 ? 11 : 12);
    if (side < low || side > high) return 0;
    int across_lens = x < 0 ? x + 12 : x - 2;
    return mod(float(across_lens + y * 1), 7.0) < 2.0 && y > -3 && y < 1 ? 2 : 1;
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float t = u_video_time;
    float alpha = 1.0;
    ivec2 pixel = ivec2(gl_FragCoord.xy);
    for (int i = 0; i < MAX_FACES; i++) {
        float born = recalled(i * 3) + recalled(i * 3 + 1) * 1024.0 + recalled(i * 3 + 2);
        bool here = i < u_face_count && u_face_seen[i] > 0.02;
        if (!here || born > t) born = t;
        if (pixel.y == 0 && pixel.x >= i * 3 && pixel.x < i * 3 + 3) alpha = remember(born, pixel.x - i * 3);
        if (!here || u_amount <= 0.0) continue;
        Face f = face_frame(i, aspect);
        float landed = clamp((t - born) / (s_drop / 10.0), 0.0, 1.0);
        float fall = mix(1.0, 1.0 - landed * landed * (3.0 - 2.0 * landed), u_amount);
        vec2 rel = p - f.eyes - f.up * (fall * 1.5 + 0.02 * u_beat * s_react / 100.0);
        float cell = f.spread / 10.5 * s_size / 100.0;
        int s = shades(vec2(dot(rel, f.across), dot(rel, f.up)) / cell);
        if (s == 1) c = vec3(0.02);
        if (s == 2) c = vec3(0.95);
    }
    return vec4(c, alpha);
}
