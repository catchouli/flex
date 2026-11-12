// God Rays: real shafts of light through the air of the room. The light comes from the brightest
// place in the shot if there is one. Each point follows its line of sight through the air,
// checking at each step whether anything in the room is in the light's way, so beams only exist
// in the air in front of what you're looking at, and people and furniture cut dark shafts through
// them. Bright windows and sky pour out their own beams, surfaces the light lands on brighten,
// beams take their colour from the film, and dust glitters in them and swirls when things move.
// Fading in, the room dims as the beams and the dust brighten in; what's remembered of the rays
// isn't faded.

float fbm4(vec2 p) {
    float sum = 0.0, amount = 0.5;
    for (int i = 0; i < 4; i++) {
        sum += amount * noise(p);
        p = p * 2.03 + vec2(17.1, 9.2);
        amount *= 0.5;
    }
    return sum;
}

float distance_of(float near) {
    return 1.0 / (0.12 + near * 1.2);
}

const int AIR = 8;
const int TOWARDS_LIGHT = 5;

// The film's own brightest colour, as a light of about full strength.
vec3 film_light() {
    vec3 best = vec3(1.0);
    float top = -1.0;
    for (int i = 0; i < 4; i++) {
        float l = dot(u_palette[i], vec3(0.3, 0.59, 0.11)) * (0.4 + u_palette_share[i]);
        if (l > top) { top = l; best = u_palette[i]; }
    }
    best /= max(max(best.r, max(best.g, best.b)), 0.05);
    return mix(best, vec3(1.0, 0.86, 0.68) + vec3(0.1, 0.0, -0.1) * u_warmth, 0.45);
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(0.0);
    vec2 low = u_video_rect.xy, size = u_video_rect.zw - low;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    float t = u_time * s_speed / 100.0;
    float react = s_react / 100.0;
    // Singing, choirs above all, and tender or happy music brighten and widen the beams, and
    // thunder flashes them.
    float hymn = clamp(u_sound_singing + 0.7 * (u_sound_tender_music + u_sound_happy_music), 0.0, 1.0) * react;
    float flash = u_sound_thunder * react * smoothstep(0.66, 0.85, noise(vec2(u_time * 14.0, 3.0)));

    vec2 above = vec2(0.5 + 0.36 * sin(t * 0.5), 1.14);
    float found = clamp(u_light.z * 1.8, 0.0, 1.0) * s_find / 100.0;
    vec2 light_pic = mix(above, vec2(u_light.x, max(u_light.y, 0.45) + 0.04), found);
    vec2 light = low + light_pic * size;

    float near = depth_at(uv);
    float sky_here = smoothstep(0.4, 0.7, sky_at(uv));
    near = mix(near, 0.0, sky_here);
    float far = distance_of(near);
    // A different start at each pixel and frame, so few steps blend into a smooth result.
    float jitter = fract(52.9829189 * fract(dot(gl_FragCoord.xy, vec2(0.06711056, 0.00583715))) + float(u_frame % 16) * 0.618034);

    // Air along the line of sight, as far as the surface.
    float through = 0.0;
    float on_surface = 1.0;
    float target_far = distance_of(0.05);
    for (int a = 0; a < AIR; a++) {
        float z = far * (float(a) + jitter) / float(AIR);
        float sun = 1.0;
        for (int s = 0; s < TOWARDS_LIGHT; s++) {
            float f = (float(s) + 0.2 + 0.8 * jitter) / float(TOWARDS_LIGHT) * 0.6;
            vec2 at = clamp(mix(uv, light, f), low, u_video_rect.zw);
            vec4 masks = masks_at(at);
            float there = depth_at(at);
            float z_there = mix(z, target_far, f * found);
            float near_enough = smoothstep(0.2, 0.42, there);
            float in_way = max(near_enough, smoothstep(0.35, 0.65, masks.r)) * smoothstep(-0.08, 0.2, z_there - distance_of(there));
            sun *= 1.0 - 0.6 * in_way * (1.0 - smoothstep(0.4, 0.7, masks.b));
        }
        through += sun;
        if (a == AIR - 1) on_surface = sun;
    }
    through /= float(AIR);
    float air = 1.0 - exp(-far * far * 0.13);

    // Light pouring from whatever's actually bright between here and the light.
    float pours = 0.0, fade = 1.0;
    for (int i = 0; i < 8; i++) {
        vec2 at = clamp(mix(uv, light, (float(i) + jitter) / 8.0 * 0.85), low, u_video_rect.zw);
        vec3 there = texture(u_video, at).rgb;
        float glow = max(dot(there, vec3(0.3, 0.59, 0.11)) - 0.68, 0.0) / 0.32;
        pours += (glow + 0.5 * sky_at(at)) * fade;
        fade *= 0.87;
    }
    pours /= 8.0;

    vec2 from_light = (uv - light) * aspect;
    float angle = atan(from_light.x, -from_light.y);
    float wide = noise(vec2(angle * 4.0 + t * 0.09, t * 0.07));
    float fine = noise(vec2(angle * 13.0 - t * 0.14, t * 0.11 + 5.0));
    float shafts = 0.16 + 0.84 * smoothstep(0.28 - 0.2 * hymn, 0.72 - 0.1 * hymn, wide * 0.65 + fine * 0.35);
    // Close to the light they all run together, instead of meeting at a point.
    shafts = mix(1.0, shafts, smoothstep(0.03, 0.4, length(from_light)));

    vec2 moving = flow_at(uv);
    float stirred = clamp(length(moving * aspect) * 1.4, 0.0, 1.0);
    vec2 drift = vec2(u_time * 0.05, -u_time * 0.03);
    float haze = 0.62 + 0.7 * fbm4(uv * aspect * 5.0 - moving * 0.35 + drift) + 0.4 * stirred;

    float rays = through * through * air * shafts * haze * 1.15 + pours * 1.5 * mix(0.45, 1.0, through);
    rays *= smoothstep(0.0, 0.7, u_since_cut);
    // Blended with the last frame, faster when the shot's just changed.
    if (u_frame > 2 && u_cut <= 0.2) {
        vec2 px = 1.5 / u_resolution;
        float before = texture(u_prev, uv + px).a + texture(u_prev, uv - px).a + texture(u_prev, uv + vec2(px.x, -px.y)).a + texture(u_prev, uv + vec2(-px.x, px.y)).a;
        rays = mix(rays, before * 0.5, 0.82);
    }
    float stored = clamp(rays * 0.5, 0.0, 1.0);

    vec3 tint = mix(hue(s_colour / 360.0) * 0.5 + 0.55, film_light(), s_film / 100.0);
    tint = mix(tint, hue(uv.x * 2.0 - t * 0.1) * 1.1, s_rainbow / 100.0);

    float strength = s_strength / 100.0 * (1.0 + 0.9 * u_bass * react + 0.8 * hymn + 3.0 * flash) * mix(1.4, 0.85, clamp(u_brightness * 2.0, 0.0, 1.0)) * u_amount;

    // Where the light lands on things.
    float e = 0.006;
    vec2 slope = vec2(depth_at(uv + vec2(e / aspect.x, 0.0)) - depth_at(uv - vec2(e / aspect.x, 0.0)), depth_at(uv + vec2(0.0, e)) - depth_at(uv - vec2(0.0, e)));
    vec3 normal = normalize(vec3(slope * 16.0, 1.0));
    vec3 towards = normalize(vec3(from_light * -1.0, 0.5));
    float facing = max(dot(normal, towards), 0.0);
    float landed = on_surface * mix(1.0, shafts, 0.6) * (0.35 + 0.65 * facing);
    vec3 lit = c * mix(vec3(1.0), mix(0.66, 0.74, clamp(u_brightness * 2.0, 0.0, 1.0)) + tint * landed * 0.85 * s_land / 100.0, u_amount);
    lit += tint * face_at(uv) * on_surface * facing * 0.08 * s_land / 100.0 * u_amount;

    // Added as light through air is, which can't blow out what's already bright.
    vec3 shown = 1.0 - (1.0 - clamp(lit, 0.0, 1.0)) * exp(-tint * rays * strength * 0.8);

    // Motes of dust, in front of whatever's behind them, only where there's light.
    float amount = s_dust / 100.0 * u_amount;
    if (amount > 0.0) {
        vec2 p = uv * aspect - moving * 0.08;
        for (int i = 0; i < 3; i++) {
            float layer = float(i);
            float cell_size = 0.022 + layer * 0.014;
            float layer_near = 0.3 + layer * 0.22;
            if (near > layer_near) continue;
            vec2 q = (p + vec2(sin(u_time * 0.1 + layer) * 0.04, u_time * (0.008 + layer * 0.005))) / cell_size + layer * 19.0;
            vec2 cell = floor(q);
            if (hash(cell + layer) > 0.12 * (1.0 + stirred * 2.0)) continue;
            vec2 spot = vec2(hash(cell + 3.1), hash(cell + 7.3)) * 0.7 + 0.15;
            float twinkle = 0.5 + 0.5 * sin(u_time * (1.0 + 2.0 * hash(cell + 11.0)) + 6.3 * hash(cell));
            float mote = 1.0 - smoothstep(0.03, 0.09 + 0.03 * layer, length(fract(q) - spot));
            shown += tint * mote * twinkle * (0.5 + 1.2 * u_treble * react) * through * shafts * amount;
        }
    }
    return vec4(shown, stored);
}
