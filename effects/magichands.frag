// Magic Hands: a ball of crackling energy in everyone's hands, sparks spinning round it, and
// lightning arcing between a person's two hands when they're near enough. It flares on the beat.
// From MoGe-2's who's who and hands. Fading in, the balls swell up from nothing as all of it
// brightens.

// Lightning from a to b: a wiggling line, re-struck several times a second.
float bolt(vec2 p, vec2 a, vec2 b, float seed) {
    vec2 ab = b - a;
    float len = length(ab);
    if (len < 1e-4) return 0.0;
    vec2 dir = ab / len;
    vec2 side = vec2(-dir.y, dir.x);
    float t = dot(p - a, dir) / len;
    if (t < 0.0 || t > 1.0) return 0.0;
    float strike = floor(u_time * 12.0);
    float wiggle = (fbm(vec2(t * 6.0, seed + strike)) - 0.5) * len * 0.35 * sin(t * 3.14159);
    float d = abs(dot(p - a, side) - wiggle);
    return exp(-d * u_resolution.y * 0.35);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float power = s_power / 100.0 * (1.0 + 1.5 * u_beat * s_react / 100.0) * u_amount;
    vec3 glow = vec3(0.0);
    for (int i = 0; i < u_person_count; i++) {
        float seen = u_person_seen[i];
        if (seen <= 0.0) continue;
        vec3 colour = s_colours < 0.5 ? hue(s_hue / 360.0) : hue(float(i) * 0.37 + s_hue / 360.0);
        vec2 at[2];
        float sure[2];
        for (int n = 0; n < 2; n++) {
            vec4 hand = u_person_hands[i * 2 + n];
            at[n] = hand.xy * aspect;
            sure[n] = seen * smoothstep(0.2, 0.6, hand.w);
            if (sure[n] <= 0.0) continue;
            float r = max(hand.z, 0.01) * 1.4 * max(u_amount, 1e-3);
            vec2 rel = p - at[n];
            float d = length(rel);
            // The ball: a hot core in a soft glow, its edge boiling.
            float edge = r * (0.75 + 0.25 * noise(vec2(atan(rel.y, rel.x) * 3.0, u_time * 6.0 + float(n * 7 + i))));
            float core = 1.0 - smoothstep(edge * 0.35, edge, d);
            float halo = exp(-d / (r * 0.9));
            // Sparks circling it.
            float sparks = 0.0;
            for (int k = 0; k < 6; k++) {
                float a = u_time * (2.0 + float(k) * 0.37) + float(k) * 1.047 + float(i);
                vec2 spark = at[n] + vec2(cos(a), sin(a) * 0.8) * r * (1.3 + 0.3 * sin(u_time * 3.0 + float(k)));
                sparks += exp(-length(p - spark) * u_resolution.y * 0.25);
            }
            glow += (colour * halo * 0.8 + vec3(1.0) * core * 0.9 + mix(colour, vec3(1.0), 0.5) * sparks) * sure[n] * power;
        }
        // Lightning between the hands, if they're both there and not too far apart.
        float apart = length(at[0] - at[1]);
        float tall = u_person_box[i].w - u_person_box[i].y;
        if (sure[0] > 0.0 && sure[1] > 0.0 && apart < tall * 0.8) {
            float arc = bolt(p, at[0], at[1], float(i) * 5.0) + 0.6 * bolt(p, at[0], at[1], float(i) * 5.0 + 2.5);
            glow += mix(colour, vec3(1.0), 0.6) * arc * min(sure[0], sure[1]) * power * (1.0 - apart / (tall * 0.8));
        }
        // The hands themselves lit by it.
        c += colour * hand_at(uv) * 0.35 * seen * power;
    }
    return vec4(c + glow, 1.0);
}
