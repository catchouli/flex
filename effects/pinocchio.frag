// Pinocchio: draws a tapering nose out from each nose tip, the way the face is pointing. When each
// face turned up is remembered in the alpha of the bottom row of u_prev (alpha never reaches the
// screen), three numbers a face so that long films still count exactly. Fading in, the noses grow
// out from a point, longer and thicker; the remembering goes on as it is.

float recalled(int x) {
    return texelFetch(u_prev, ivec2(x, 0), 0).a;
}

float remember(float t, int x) {
    if (x == 0) return mod(floor(t), 1024.0);
    if (x == 1) return floor(t / 1024.0);
    return fract(t);
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
        vec2 aim = vec2(dot(f.nose - 0.5 * (f.eyes + f.mouth), f.across) / f.height, 0.53 - dot(f.eyes - f.nose, f.up) / f.height) * 2.0;
        // Head-on it still has to stick out somewhere.
        aim.x += aim.x < 0.0 ? -0.35 : 0.35;
        vec2 d = normalize(f.across * aim.x + f.up * aim.y);
        if (u_face_turn[i].w > 0.5) {
            // Head-on it still has to stick out somewhere.
            vec2 turn = u_face_turn[i].xy;
            float side_on = length(turn);
            d = normalize(turn + f.across * (turn.x * f.across.x + turn.y * f.across.y < 0.0 ? -0.35 : 0.35) * (1.0 - smoothstep(0.2, 0.5, side_on)));
        }
        float length_now = f.height * min(0.2 + (t - born) * 0.18 * s_speed / 100.0, s_longest / 100.0) * u_amount;

        vec2 base = f.nose - d * f.spread * 0.1;
        vec2 side = vec2(-d.y, d.x);
        float along = dot(p - base, d) / length_now;
        float away = dot(p - base, side);
        float root = f.spread * 0.2 * u_amount;
        float half_width = root * mix(1.0, 0.55, clamp(along, 0.0, 1.0));
        // A round end on the taper.
        float past = max(along - 1.0, 0.0) * length_now;
        float inside = (1.0 - smoothstep(0.75, 1.0, length(vec2(away, past)) / half_width)) * smoothstep(-0.05, 0.05, along);
        inside *= u_face_seen[i];
        if (inside > 0.0) {
            // Skin from the nose itself, stretched out, and lit from above.
            float tip = clamp(along, 0.0, 1.0);
            vec2 from = f.nose + side * away * 0.8 + d * (fract(tip * 3.0) - 0.5) * root * 0.25;
            vec3 skin = texture(u_video, from / aspect).rgb;
            float light = 0.72 + 0.45 * dot(side, vec2(0.0, 1.0)) * away / half_width + 0.12 * tip;
            c = mix(c, skin * light, inside);
        }
    }
    return vec4(c, alpha);
}
