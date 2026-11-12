// Big Heads: each head is magnified about its chin, so it swells upwards and outwards while the
// neck stays where it is, and the picture around it is pushed back into place at the edge. Fading
// in, heads swell from their own size and start to bobble.

// The head compared with the box round the face: how wide and tall from its middle, and how far
// above the chin the middle is. Bigger than the box, to take in the hair and ears.
const vec2 HEAD = vec2(0.8, 0.75);
const float MIDDLE = 0.62;
// Where the magnifying starts to let go and where it stops, in head sizes from the middle.
const float SOLID = 1.05;
const float GONE = 1.9;

vec4 effect(vec2 uv) {
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    vec2 low = u_video_rect.xy * aspect, high = u_video_rect.zw * aspect;
    float t = u_video_time;
    vec2 sum = vec2(0.0);
    float weights = 0.0, most = 0.0;
    for (int i = 0; i < u_face_count; i++) {
        vec2 eyes = 0.5 * (face_point(i, 0) + face_point(i, 1)) * aspect;
        vec2 mouth = 0.5 * (face_point(i, 3) + face_point(i, 4)) * aspect;
        vec2 up = normalize(eyes - mouth + vec2(0.0, 1e-5));
        vec2 across = vec2(up.y, -up.x);
        vec2 size = (u_faces[i].zw - u_faces[i].xy) * aspect;
        vec2 centre = 0.5 * (u_faces[i].xy + u_faces[i].zw) * aspect;
        vec2 chin = centre - up * size.y * 0.5;

        float seen = smoothstep(0.0, 1.0, u_face_seen[i]);
        float grown = seen * u_amount;
        // Swells with the bass and kicks on each beat.
        float pulse = (0.5 * u_bass + 0.45 * u_beat) * s_react / 100.0;
        float zoom = 1.0 + (s_size / 100.0 - 1.0 + pulse) * grown;
        float angle = s_bobble / 100.0 * 0.3 * sin(t * 2.7 + float(i) * 1.9) * grown;
        vec2 from_chin = p - chin;
        from_chin = vec2(from_chin.x * cos(angle) - from_chin.y * sin(angle), from_chin.x * sin(angle) + from_chin.y * cos(angle));

        vec2 local = vec2(dot(from_chin, across), dot(from_chin, up) - size.y * MIDDLE * zoom);
        float away = length(local / (size * HEAD * zoom));
        float weight = (1.0 - smoothstep(SOLID, GONE, away)) * seen;

        // The nearest and biggest head goes in front where two overlap.
        float front = pow(weight, 6.0) * size.y + 1e-6;
        sum += clamp(chin + from_chin / zoom, low, high) * front;
        weights += front;
        most = max(most, weight);
    }
    vec2 q = u_face_count > 0 ? mix(p, sum / weights, most) : p;
    return vec4(texture(u_video, q / aspect).rgb, 1.0);
}
