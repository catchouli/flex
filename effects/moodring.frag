// Mood Ring: the film's colours follow how its music feels. Each mood has a grade of its own, and
// they mix in the shares Flex hears them in, so a score that turns from tender to scary takes the
// picture with it. Without music the film looks as it is. A ring in the corner shows the mood's
// colour, like the stone in a mood ring. Fading in, the grades come up to strength and the ring
// grows from a point.

float luma(vec3 c) {
    return dot(c, vec3(0.299, 0.587, 0.114));
}

vec3 saturate_by(vec3 c, float amount) {
    return mix(vec3(luma(c)), c, amount);
}

vec3 contrast(vec3 c, float amount, float pivot) {
    return (c - pivot) * amount + pivot;
}

vec4 effect(vec2 uv) {
    float k = s_strength / 100.0 * u_amount;
    float aspect = u_resolution.x / u_resolution.y;
    float t = u_time;
    vec2 centre = mix(u_video_rect.xy, u_video_rect.zw, 0.5);
    float r = length((uv - centre) * vec2(aspect, 1.0));

    float happy = u_sound_happy_music * k, sad = u_sound_sad_music * k, tender = u_sound_tender_music * k;
    float exciting = u_sound_exciting_music * k, angry = u_sound_angry_music * k, scary = u_sound_scary_music * k;

    // Exciting music punches in on the beat; angry music shakes.
    vec2 p = centre + (uv - centre) * (1.0 - 0.02 * exciting * u_beat);
    p += (vec2(hash(vec2(floor(t * 24.0), 1.0)), hash(vec2(2.0, floor(t * 24.0)))) - 0.5) * 0.006 * angry * u_level;
    p = clamp(p, u_video_rect.xy, u_video_rect.zw);
    vec3 c = texture(u_video, p).rgb;
    vec3 base = c;
    float y = luma(c);

    // Happy: warm, rich and lifted.
    vec3 g = saturate_by(c, 1.35) * vec3(1.1, 1.02, 0.85) + vec3(0.04, 0.03, 0.0);
    c = mix(c, g, clamp(happy, 0.0, 1.0));
    // Sad: drained, cold and flat.
    g = mix(vec3(y), base, 0.35) * vec3(0.82, 0.92, 1.12);
    g = contrast(g, 0.85, 0.4);
    c = mix(c, g, clamp(sad, 0.0, 1.0));
    // Tender: a soft pink glow around bright things.
    if (tender > 0.01) {
        vec3 soft = vec3(0.0);
        for (int i = 0; i < 8; i++) {
            float a = float(i) * 0.785398;
            soft += texture(u_video, p + vec2(cos(a) / aspect, sin(a)) * 0.012).rgb;
        }
        soft /= 8.0;
        g = mix(c, soft, 0.5) + soft * soft * vec3(0.35, 0.18, 0.22);
        g = saturate_by(g, 0.85) * vec3(1.06, 0.97, 1.0);
        c = mix(c, g, clamp(tender, 0.0, 1.0));
    }
    // Exciting: hard contrast and loud colour.
    g = saturate_by(contrast(c, 1.35, 0.45), 1.6);
    c = mix(c, g, clamp(exciting, 0.0, 1.0));
    // Angry: red, with the blacks crushed.
    g = contrast(mix(vec3(y), c, 0.6) * vec3(1.3, 0.75, 0.65), 1.3, 0.3);
    c = mix(c, g, clamp(angry, 0.0, 1.0));
    // Scary: cold green-cyan, dark round the edges, a failing light and grain.
    float flicker = 0.85 + 0.15 * step(0.35, hash(vec2(floor(t * 12.0), 5.0)));
    g = mix(vec3(y), c, 0.4) * vec3(0.8, 1.05, 0.95) * flicker * (1.0 - smoothstep(0.3, 0.95, r) * 0.7);
    g += (hash(uv * u_resolution + fract(t) * 61.0) - 0.5) * 0.06;
    c = mix(c, g, clamp(scary, 0.0, 1.0));

    // The ring: a glossy stone in the corner, in the mood's colour.
    if (s_ring > 0.5 && u_amount > 0.0) {
        float total = happy + sad + tender + exciting + angry + scary;
        vec3 mood = vec3(0.12, 0.1, 0.1);
        if (total > 0.02) {
            mood = (happy * vec3(1.0, 0.75, 0.1) + sad * vec3(0.15, 0.3, 0.9) + tender * vec3(1.0, 0.45, 0.65)
                + exciting * vec3(0.2, 0.9, 0.4) + angry * vec3(0.95, 0.1, 0.05) + scary * vec3(0.4, 0.1, 0.6)) / total;
            mood = mix(vec3(0.12, 0.1, 0.1), mood, smoothstep(0.02, 0.3, total));
        }
        vec2 at = vec2(u_video_rect.z * aspect - 0.09, u_video_rect.y + 0.09);
        vec2 q = (vec2(uv.x * aspect, uv.y) - at) / (0.05 * u_amount);
        float d = length(q);
        // A gold band round an oval stone, lit from the top left.
        float band = smoothstep(1.02, 0.98, d) * smoothstep(0.78, 0.82, d);
        float stone = smoothstep(0.82, 0.78, d);
        vec3 gold = vec3(0.85, 0.65, 0.25) * (0.7 + 0.4 * dot(normalize(q + 1e-4), normalize(vec2(-1.0, 1.0))));
        vec3 gem = mood * (0.55 + 0.6 * (1.0 - d)) + vec3(1.0) * smoothstep(0.25, 0.0, length(q - vec2(-0.3, 0.3))) * 0.8;
        float shade = smoothstep(1.2, 1.0, d) * (1.0 - band - stone) * 0.4;
        c *= 1.0 - shade;
        c = mix(c, gold, band);
        c = mix(c, gem, stone);
    }
    return vec4(clamp(c, 0.0, 1.0), 1.0);
}
