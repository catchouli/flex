// Relight: treats depth as a surface. Normals come from the depth network (or depth's slopes),
// so the picture can be lit again from new positions (and shadows found by walking towards each
// light). Fading in, the film's own light gives way to the new lights.
//
// s_style: 0 coloured lights, 1 clay, 2 oil slick, 3 show the surface.

const float RELIEF = 0.6;

// The surface's direction, with its tilt made more or less of by s_bump.
vec3 bumped_normal(vec2 uv) {
    vec3 n = normal_at(uv);
    return normalize(vec3(n.xy * s_bump / 50.0, max(n.z, 0.02)));
}

float shadow(vec3 p, vec3 light, float aspect) {
    float lit = 1.0;
    for (int i = 1; i <= 14; i++) {
        float a = float(i) / 15.0;
        vec3 q = mix(p, light, a * a);
        float above = depth_at(q.xy / vec2(aspect, 1.0) + 0.5) * RELIEF - q.z - 0.015;
        lit = min(lit, 1.0 - smoothstep(0.0, 0.05, above));
    }
    return lit;
}

vec4 effect(vec2 uv) {
    // The black bars stay black.
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    vec3 video = texture(u_video, uv).rgb;
    float d = depth_at(uv);
    float t = u_video_time * s_speed;
    vec3 n = bumped_normal(uv);
    if (s_style > 2.5) {
        return vec4(mix(video, n * 0.5 + 0.5, u_amount), 1.0);
    }
    if (s_style > 1.5) {
        // A reflection of a slowly turning room of colour.
        vec3 r = reflect(vec3(0.0, 0.0, -1.0), n);
        float bands = sin(r.x * 7.0 + t) + sin(r.y * 9.0 - 0.7 * t) + 3.0 * d;
        vec3 chrome = hue(0.18 * bands + 0.03 * t) * (0.45 + 0.55 * pow(1.0 - n.z, 1.5)) + pow(max(r.y, 0.0), 12.0);
        return vec4(mix(video * mix(1.0, 0.4, u_amount), chrome, 0.85 * u_amount), 1.0);
    }

    float aspect = u_resolution.x / u_resolution.y;
    vec3 p = vec3((uv - 0.5) * vec2(aspect, 1.0), d * RELIEF);
    vec3 view = vec3(0.0, 0.0, 1.0);
    vec3 base = s_style > 0.5 ? vec3(0.3) : video * video;
    vec3 light = vec3(s_style > 0.5 ? 0.12 : s_ambient / 100.0);
    vec3 shine = vec3(0.0);
    float flash = 1.0 + 1.5 * u_beat * s_react / 100.0;
    for (int i = 0; i < int(s_lights); i++) {
        float f = float(i);
        vec3 at = vec3(
            0.55 * aspect * sin(t * (0.5 + 0.13 * f) + f * 2.4),
            0.42 * cos(t * (0.37 + 0.09 * f) + f * 1.3),
            0.95 + 0.25 * sin(t * 0.6 + f)
        );
        // Lights stay where they are in the room when the camera moves.
        at.xy = (world_pin_nearby(at.xy / vec2(aspect, 1.0) + 0.5, 1.2) - 0.5) * vec2(aspect, 1.0);
        vec3 colour = hue(f * 0.31 + 0.1 + 0.03 * t) * 1.4;
        vec3 to = at - p;
        float dist2 = dot(to, to);
        vec3 l = to / sqrt(dist2);
        float seen = s_shadows > 0.5 ? shadow(p, at, aspect) : 1.0;
        float reach = flash * seen / (0.25 + 1.6 * dist2);
        light += colour * max(dot(n, l), 0.0) * reach;
        shine += colour * pow(max(dot(n, normalize(l + view)), 0.0), 40.0) * reach * 0.4;
    }
    // At none, the film's own light: sqrt(video * video * 1.4 / 1.4).
    base = mix(video * video, base, u_amount);
    light = mix(vec3(1.0 / 1.4), light, u_amount);
    return vec4(sqrt(base * light * 1.4) + shine * u_amount, 1.0);
}
