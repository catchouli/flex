// Distance: one effect, as strong as depth says. Everything is weighed by m, which runs
// from 0 where the effect stops to 1 at the far end (or the near end, with s_where). Fading in,
// the effect's strength grows from nothing.
//
// s_style: 0 fog, 1 prism, 2 heat haze, 3 blocks, 4 colour spin, 5 rush.

// A fog colour setting: the first choice is the drifting rainbow, the rest are fixed.
vec3 fog_colour(float choice, vec3 moving) {
    const vec3 named[11] = vec3[11](
        vec3(0.92, 0.94, 0.97), vec3(0.5, 0.52, 0.56), vec3(0.02, 0.02, 0.04), vec3(0.85, 0.1, 0.08), vec3(0.95, 0.5, 0.08),
        vec3(0.95, 0.85, 0.2), vec3(0.15, 0.75, 0.2), vec3(0.08, 0.7, 0.7), vec3(0.1, 0.3, 0.9), vec3(0.5, 0.15, 0.85), vec3(0.95, 0.3, 0.65)
    );
    return choice < 0.5 ? moving : named[int(choice + 0.5) - 1];
}

float noise_sin(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    vec2 k = vec2(127.1, 311.7);
    float a = fract(sin(dot(i, k)) * 43758.5453);
    float b = fract(sin(dot(i + vec2(1.0, 0.0), k)) * 43758.5453);
    float c = fract(sin(dot(i + vec2(0.0, 1.0), k)) * 43758.5453);
    float d = fract(sin(dot(i + vec2(1.0, 1.0), k)) * 43758.5453);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

float clouds(vec2 p) {
    return 0.55 * noise_sin(p) + 0.3 * noise_sin(p * 2.1 + 7.0) + 0.15 * noise_sin(p * 4.3 + 13.0);
}

vec4 effect(vec2 uv) {
    // The black bars stay black.
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    float d = depth_at(uv);
    float t = u_video_time * s_speed;
    float from = s_from / 100.0;
    float m = smoothstep(from, 1.0, s_where < 0.5 ? 1.0 - d : d);
    float kick = 1.0 + u_beat * s_react / 100.0;
    float amount = s_amount / 100.0 * kick * u_amount;
    float aspect = u_resolution.x / u_resolution.y;
    vec2 p = uv * vec2(aspect, 1.0);

    if (s_style < 0.5) {
        // Far layers of cloud move more slowly and look smaller, so the fog has some depth.
        float n = clouds(p * (2.0 + 5.0 * m) + vec2(0.12, 0.03) * t);
        vec3 moving = hue(0.04 * t + 0.25 * p.x + 0.3 * m) * 0.45 + 0.45;
        vec3 fog = mix(fog_colour(s_fog_start, moving), fog_colour(s_fog_end, moving), m);
        float thick = 1.0 - exp(-1.6 * m * m * amount * (0.3 + 1.4 * n));
        return vec4(mix(texture(u_video, uv).rgb, fog, thick), 1.0);
    }
    if (s_style < 1.5) {
        vec2 dir = (uv - 0.5) * 0.09 * m * amount;
        vec3 c = vec3(texture(u_video, uv + dir).r, texture(u_video, uv).g, texture(u_video, uv - dir).b);
        return vec4(mix(c, c * hue(0.1 * t + m), 0.25 * m * amount), 1.0);
    }
    if (s_style < 2.5) {
        vec2 wob = vec2(sin(p.y * 40.0 + t * 5.0 + 6.0 * noise_sin(p * 5.0)), cos(p.x * 31.0 + t * 4.0));
        return vec4(texture(u_video, uv + wob * 0.012 * m * amount).rgb, 1.0);
    }
    if (s_style < 3.5) {
        // Quantised depth keeps each band of the scene to one block size.
        float level = floor(m * 5.0 + 0.5) / 5.0;
        vec2 size = vec2(1.0 + level * amount * 70.0) / u_resolution;
        vec2 q = (floor(uv / size) + 0.5) * size;
        return vec4(texture(u_video, q).rgb, 1.0);
    }
    if (s_style < 4.5) {
        vec3 c = texture(u_video, uv).rgb;
        // Blended towards it: the turn grows with time, so scaling it would wander on the way in.
        return vec4(mix(c, hue_rotate(c, m * (amount * 12.0 + 0.6 * t)), u_amount), 1.0);
    }
    vec3 sum = vec3(0.0);
    for (int i = 0; i < 16; i++) {
        float a = float(i) / 16.0;
        sum += texture(u_video, mix(uv, vec2(0.5), a * 0.35 * m * amount)).rgb;
    }
    return vec4(sum / 16.0, 1.0);
}
