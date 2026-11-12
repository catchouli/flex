// Kaleidoface: the same fold as Kaleidoscope, centred on the biggest face and lined up with it,
// so the face itself is what repeats. Fading in, each point slides from where it is to where the
// fold puts it.

const float TAU = 6.2831853;

vec2 bounce(vec2 x) {
    return 1.0 - abs(1.0 - mod(x, 2.0));
}

vec3 folded(vec2 uv, vec2 rel, float arms, Face f, float reach, vec2 aspect) {
    float wedge = TAU / max(arms, 1.0);
    float a = abs(mod(atan(rel.y, rel.x), wedge) - wedge * 0.5);
    vec2 k = length(rel) * vec2(cos(a), sin(a)) * reach;
    // The slice runs from the chin up through the face, so eyes, nose and mouth all repeat.
    vec2 q = f.nose - f.up * f.height * 1.25 + f.up * k.x + f.across * k.y;
    vec2 size = u_video_rect.zw - u_video_rect.xy;
    return texture(u_video, mix(uv, u_video_rect.xy + bounce((q / aspect - u_video_rect.xy) / size) * size, u_amount)).rgb;
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    int biggest = -1;
    float most = 0.0;
    for (int i = 0; i < u_face_count; i++) {
        float h = u_faces[i].w - u_faces[i].y;
        if (u_face_seen[i] > 0.5 && h > most) { most = h; biggest = i; }
    }
    if (biggest < 0) return vec4(c, 1.0);
    Face f = face_frame(biggest, aspect);
    vec2 rel = rotate(p - f.nose, u_video_time * 0.25 * s_spin / 100.0);
    float reach = f.height * 6.0 / (s_zoom / 100.0) * (1.0 - 0.25 * u_bass * s_react / 100.0);
    float whole = floor(s_arms);
    float between = smoothstep(0.3, 0.7, s_arms - whole);
    vec3 k = folded(uv, rel, whole, f, reach, aspect);
    if (between > 0.0) k = mix(k, folded(uv, rel, whole + 1.0, f, reach, aspect), between);
    return vec4(mix(k, c, s_film / 100.0) * u_face_seen[biggest] + c * (1.0 - u_face_seen[biggest]), 1.0);
}
