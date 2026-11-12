// Colour Thief: takes the colour out of everything but one kind of thing: people, everything but
// people, animals, vehicles, plants or sky. Fading in, the colour drains out of everything else
// as the kept colour grows stronger.
//
// s_who: 0 people, 1 the world (all but people), 2 animals, 3 vehicles, 4 plants, 5 sky.

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) return vec4(c, 1.0);
    float person = smoothstep(0.4, 0.65, person_at(uv));
    float colourful = s_who < 0.5 ? person : 1.0 - person;
    if (s_who > 1.5) {
        float kind = s_who < 2.5 ? animal_at(uv) : s_who < 3.5 ? vehicle_at(uv) : s_who < 4.5 ? plant_at(uv) : sky_at(uv);
        colourful = smoothstep(0.35, 0.65, kind);
    }
    float luma = dot(c, vec3(0.3, 0.59, 0.11));
    float boost = s_boost / 100.0 * (1.0 + u_bass * s_react / 100.0);
    vec3 vivid = clamp(vec3(luma) + (c - luma) * boost, 0.0, 1.0);
    return vec4(mix(mix(c, vec3(luma), s_grey / 100.0 * u_amount), vivid, colourful * u_amount), 1.0);
}
