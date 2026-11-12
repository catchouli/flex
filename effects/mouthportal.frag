// Mouth Portal: the inside of each mouth, from its mesh, shows somewhere else entirely. Fading
// in, the glow comes up and the portal opens as if the mouth were opening wider.

const int LIP_POINTS = 20;

int lip(int n) {
    int inner[LIP_POINTS] = int[LIP_POINTS](78, 191, 80, 81, 82, 13, 312, 311, 310, 415, 308, 324, 318, 402, 317, 14, 87, 178, 88, 95);
    return inner[n];
}

// Whether a point's inside the lips, by counting edges crossed on the way out.
bool inside_lips(int face, vec2 uv) {
    bool inside = false;
    vec2 last = mesh_point(face, lip(LIP_POINTS - 1)).xy;
    for (int n = 0; n < LIP_POINTS; n++) {
        vec2 here = mesh_point(face, lip(n)).xy;
        if ((here.y > uv.y) != (last.y > uv.y) && uv.x < (last.x - here.x) * (uv.y - here.y) / (last.y - here.y) + here.x) {
            inside = !inside;
        }
        last = here;
    }
    return inside;
}

vec3 galaxy(vec2 p) {
    float r = length(p), a = atan(p.y, p.x) + u_time * 0.8 + 3.0 / (r + 0.3);
    float arms = pow(0.5 + 0.5 * sin(a * 3.0 + r * 8.0), 3.0);
    vec2 cell = floor(p * 24.0);
    float star = step(0.985, fract(sin(dot(cell, vec2(12.9898, 78.233))) * 43758.5453));
    return vec3(0.2, 0.05, 0.5) * arms * 2.0 + vec3(1.0, 0.5, 0.9) * exp(-r * 6.0) * (0.5 + u_bass) + star;
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    for (int i = 0; i < u_face_count; i++) {
        float open = u_face_open[i].z * u_face_mesh[i] * u_face_seen[i] * u_amount;
        if (open <= 0.01) continue;
        vec2 top = mesh_point(i, 13).xy, bottom = mesh_point(i, 14).xy;
        vec2 left = mesh_point(i, 61).xy, right = mesh_point(i, 291).xy;
        vec2 centre = 0.25 * (top + bottom + left + right);
        float width = max(length((right - left) * aspect), 1e-4);
        vec2 p = (uv - centre) * aspect / width;
        float glow = exp(-length(p) * 2.2) * open * s_glow * 0.6;
        c += vec3(0.55, 0.3, 1.0) * glow * (0.7 + u_bass);
        if (length(p) < 1.2 && inside_lips(i, uv)) {
            c = mix(c, galaxy(p * 2.5), smoothstep(0.02, 0.15, open));
        }
    }
    return vec4(c, 1.0);
}
