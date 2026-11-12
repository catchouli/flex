// Big Hands: hands (or feet) swell up where they are, each a lens over the picture centred on
// it, from MoGe-2's who's who. They throb with the bass. Fading in, they swell from their own
// size.
//
// s_which: 0 hands, 1 feet, 2 both.

vec2 swell(vec2 p, vec4 spot, float grow, float seen) {
    if (seen <= 0.0 || spot.w <= 0.0) return p;
    vec2 centre = spot.xy * vec2(u_resolution.x / u_resolution.y, 1.0);
    // Wider than the hand itself, so all of it grows and the arm stretches into it.
    float reach = max(spot.z, 0.015) * 4.5;
    vec2 v = p - centre;
    float d = length(v) / reach;
    if (d >= 1.0) return p;
    float amount = mix(1.0 / grow, 1.0, smoothstep(0.0, 1.0, d));
    return centre + v * mix(1.0, amount, seen * smoothstep(0.2, 0.6, spot.w));
}

vec4 effect(vec2 uv) {
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float grow = 1.0 + s_grow / 100.0 * (1.0 + 0.4 * u_bass * s_react / 100.0) * u_amount;
    for (int i = 0; i < u_person_count; i++) {
        float seen = u_person_seen[i];
        for (int n = 0; n < 2; n++) {
            if (s_which < 0.5 || s_which > 1.5) p = swell(p, u_person_hands[i * 2 + n], grow, seen);
            if (s_which > 0.5) p = swell(p, u_person_feet[i * 2 + n], grow, seen);
        }
    }
    return vec4(texture(u_video, p / aspect).rgb, 1.0);
}
