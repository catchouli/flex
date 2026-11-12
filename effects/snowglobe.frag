// Snow Globe: flakes in layers at different distances, hidden by anything nearer. Where the top
// of something is, snow builds up in the alpha of u_prev, and a beat knocks it off again. The falling flakes drift on
// the air, blown sideways by wind heard in the film, and bangs and explosions knock them about.
// Fading in, the snow starts falling more thickly, settles faster and the glass bends into place.

// How the air's moving here: slow eddies that carry nearby flakes along together, and change.
vec2 eddy(vec2 p, float t) {
    return vec2(noise(p * 1.7 + vec2(t * 0.13, 0.0)), noise(p * 1.7 + vec2(3.1, -t * 0.11))) - 0.5;
}

// One layer of flakes, each drifting on the air and turning slowly around its own path.
float flakes(vec2 p, float scale, float t, float layer, float wind, float knock) {
    // Sheared, so that falling straight down here carries them sideways on the screen.
    vec2 q = p * scale - vec2((p.y - 0.5) * wind * scale, 0.0);
    q += (eddy(p + layer * 4.3, t) + 0.4 * eddy(p * 2.6 + layer, t * 1.7)) * scale * 0.2 * (1.0 + 1.0 * knock);
    q.y += t * (0.55 + 0.35 * hash(vec2(scale)));
    vec2 cell = floor(q);
    if (hash(cell + 2.0) > s_snow / 100.0 * u_amount || u_amount <= 0.0) return 0.0;
    float rate = 0.5 + hash(cell + 7.0);
    float phase = 6.2831853 * hash(cell + 9.0);
    vec2 place = vec2(hash(cell), hash(cell + 5.0)) * 0.5 + 0.25 + 0.18 * vec2(sin(t * rate + phase), cos(t * rate * 0.7 + phase * 1.3));
    float radius = 0.05 + 0.06 * hash(cell + 13.0);
    float d = length(fract(q) - place);
    // The nearest are out of focus.
    return (1.0 - smoothstep(radius * (0.6 - 0.12 * layer), radius * (1.0 + 0.3 * layer), d)) * (0.6 + 0.4 * hash(cell + 17.0));
}

vec4 effect(vec2 uv) {
    vec2 at = uv;
    float glass = 0.0;
    if (s_glass < 0.5) {
        vec2 off = uv - 0.5;
        at = 0.5 + off * mix(1.0, 0.93 + 0.12 * dot(off, off), u_amount);
        glass = dot(off, off) * u_amount;
    }
    vec3 c = texture(u_video, at).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 1.0);
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = at * aspect;
    // Wrapped, so the sums below don't run out of precision over a long film.
    float t = mod(u_time, 900.0);
    float near = depth_at(at);
    // Bangs and explosions knock the snow off things as a beat does.
    float knock = max(u_sound_explosions, u_sound_impacts) * s_shake / 100.0;
    float kick = max(u_beat * s_shake / 100.0, 0.6 * knock);
    // Wind heard in the film blows the flakes sideways, in gusts.
    float wind = u_sound_wind * 1.1 * (0.6 + 0.7 * noise(vec2(t * 0.23, 5.0)));

    // The top of something: nearer here than just above.
    float step_up = 3.0 / u_resolution.y;
    float ledge = smoothstep(0.03, 0.07, near - depth_at(at + vec2(0.0, step_up * 3.0)));
    vec4 before = texture(u_prev, uv);
    float lying = u_frame < 2 ? 0.0 : before.a;
    if (lying > 1.0) lying = 0.0;
    float rate = s_settle / 100.0 * u_amount * 0.025 * smoothstep(0.35, 0.75, fbm(p * 26.0));
    lying = clamp(lying + ledge * rate - 0.0012 - kick * 0.25, 0.0, 1.0);
    // Falling away from places that aren't ledges any more.
    lying *= mix(0.97, 1.0, ledge);
    c = mix(c, vec3(0.95, 0.97, 1.0), smoothstep(0.1, 0.8, lying) * u_amount);

    float fall = 0.0;
    for (int i = 0; i < 4; i++) {
        float f = float(i);
        float layer = 0.2 + f * 0.25;
        // Each layer stays put in the air as the camera moves.
        vec2 at_layer = p - (world_pin(vec2(0.5), world_distance(layer)) - vec2(0.5)) * vec2(u_resolution.x / u_resolution.y, 1.0);
        fall += flakes(at_layer, 46.0 - f * 11.0, t, f, wind, knock) * step(near, layer + 0.1) * (0.5 + f * 0.17);
    }
    c += vec3(0.95, 0.97, 1.0) * fall;
    if (s_glass < 0.5) {
        c *= 1.0 - smoothstep(0.2, 0.55, glass) * 0.55 * u_amount;
        vec2 shine = (uv - vec2(0.27, 0.74)) * vec2(1.0, 1.7);
        c += vec3(0.8, 0.9, 1.0) * (1.0 - smoothstep(0.0, 0.17, length(shine))) * 0.13 * u_amount;
    }
    return vec4(c, lying);
}
