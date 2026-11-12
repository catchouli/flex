// Film Palette: each point takes the nearest of the picture's four main colours, like a
// screen print made specially for this shot. Fading in, the colours flatten into the palette and the
// print starts to wobble.

vec4 effect(vec2 uv) {
    vec2 p = uv * vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 bend = (vec2(noise(p * 40.0), noise(p * 40.0 + 9.0)) - 0.5) * 0.008 * s_wobble / 30.0 * u_amount;
    vec3 c = texture(u_video, uv + bend).rgb;
    int best = 0;
    float least = 1e9;
    for (int k = 0; k < 4; k++) {
        vec3 d = c - u_palette[k];
        float away = dot(d, d);
        if (away < least) { least = away; best = k; }
    }
    return vec4(mix(c, u_palette[best], s_flat / 100.0 * u_amount) * (s_bright / 100.0 * u_amount + (1.0 - u_amount)), 1.0);
}
