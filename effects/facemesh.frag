// Face Mesh: draws what effects get with mesh = true, for checking it. Fading in, the view comes up
// over the picture.

float dot_at(vec2 uv, vec2 at, vec2 aspect, float size) {
    return 1.0 - smoothstep(size, size * 1.6, length((uv - at) * aspect));
}

float line_at(vec2 uv, vec2 a, vec2 b, vec2 aspect) {
    vec2 pa = (uv - a) * aspect, ba = (b - a) * aspect;
    float along = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-8), 0.0, 1.0);
    return 1.0 - smoothstep(0.002, 0.004, length(pa - ba * along));
}

vec3 parts_view(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb * 0.6;
    c = mix(c, vec3(0.75, 0.55, 0.4), skin_at(uv));
    c = mix(c, vec3(0.95, 0.1, 0.3), lips_at(uv));
    c = mix(c, vec3(0.2, 0.8, 0.3), nose_at(uv));
    c = mix(c, vec3(0.3, 0.2, 0.1), brows_at(uv));
    c = mix(c, vec3(1.0), eye_at(uv));
    c = mix(c, vec3(0.1, 0.4, 1.0), iris_at(uv));
    c = mix(c, vec3(0.1), mouth_at(uv));
    return c;
}

vec3 where_view(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb * 0.6;
    vec2 at = face_uv(uv);
    vec2 grid = abs(fract(at * 16.0) - 0.5);
    float lines = 1.0 - smoothstep(0.4, 0.5, max(grid.x, grid.y));
    return mix(c, mix(vec3(at, 0.3), vec3(1.0), 1.0 - lines), on_face(uv));
}

vec4 view(vec2 uv) {
    if (s_view > 2.5) {
        vec2 at = uv * vec2(4.0, 2.0);
        vec4 f = face_texture(int(at.y) * 4 + int(at.x), (fract(at) - 0.02) / 0.96);
        return vec4(f.rgb * f.a, 1.0);
    }
    if (s_view > 1.5) return vec4(where_view(uv), 1.0);
    if (s_view > 0.5) return vec4(parts_view(uv), 1.0);
    vec3 c = texture(u_video, uv).rgb * 0.55;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    for (int i = 0; i < u_face_count; i++) {
        vec4 b = u_faces[i];
        vec2 size = b.zw - b.xy;
        vec2 grown = size * 0.35;
        // The head's direction, from the nose: works side-on too.
        if (u_face_turn[i].w > 0.5) {
            vec2 nose = face_point(i, 2);
            vec2 tip = nose + u_face_turn[i].xy / aspect * size.y * 1.2;
            c = mix(c, vec3(1.0, 0.25, 0.2), line_at(uv, nose, tip, aspect) * u_face_seen[i]);
        }
        // How open the eyes and mouth are, as bars beside the face.
        vec2 bar = vec2(b.z + 0.006, b.y);
        for (int n = 0; n < 3; n++) {
            vec2 here = (uv - bar) / vec2(0.006, size.y);
            vec3 open = u_face_open[i];
            float amount = n == 0 ? open.x : (n == 1 ? open.y : open.z);
            if (here.x >= 0.0 && here.x <= 1.0 && here.y >= 0.0 && here.y <= amount) {
                c = mix(c, n < 2 ? vec3(0.3, 0.8, 1.0) : vec3(1.0, 0.7, 0.2), u_face_mesh[i]);
            }
            bar.x += 0.009;
        }
        if (u_face_mesh[i] <= 0.01 || uv.x < b.x - grown.x || uv.x > b.z + grown.x || uv.y < b.y - grown.y || uv.y > b.w + grown.y) {
            continue;
        }
        float dot_size = 0.0007 + 0.006 * size.y;
        for (int n = 0; n < 478; n++) {
            vec3 p = mesh_point(i, n);
            float near = clamp(0.5 - p.z / max(size.x, 1e-4), 0.0, 1.0);
            c = mix(c, mix(vec3(0.1, 0.5, 1.0), vec3(0.3, 1.0, 0.5), near), dot_at(uv, p.xy, aspect, dot_size) * u_face_mesh[i]);
        }
    }
    return vec4(c, 1.0);
}

vec4 effect(vec2 uv) {
    vec3 c = view(uv).rgb;
    return vec4(c * u_amount + texture(u_video, uv).rgb * (1.0 - u_amount), 1.0);
}
