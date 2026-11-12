// Laser Eyes: a beam from each eye, drawn as light added to the picture. Beams go the way the
// face is pointing, worked out from where the nose sits between the eyes and mouth. Someone
// looking straight at the camera is firing at you, so their beams shrink into a flare. Fading in,
// the beams shoot out from the eyes as they brighten.

// Where the nose tip sits between the eyes and the mouth when the head is level.
const float LEVEL_NOSE = 0.53;
// How far the nose tip stands out from the face, as a fraction of the distance from eyes to mouth.
const float NOSE_DEPTH = 0.5;

vec3 tint() {
    if (s_colour < 0.5) return vec3(1.0, 0.12, 0.08);
    if (s_colour < 1.5) return vec3(0.15, 1.0, 0.2);
    return vec3(0.15, 0.4, 1.0);
}

// How close `p` is to the ray from `a` along `d`, as a bright core and a soft glow, dying away
// at `reach`.
float beam(vec2 p, vec2 a, vec2 d, float width, float reach) {
    float along = max(dot(p - a, d), 0.0);
    float away = length(p - a - d * along);
    return (1.0 - smoothstep(reach * 0.5, reach + 1e-4, along)) / (1.0 + pow(away / width, 2.0));
}

// Which way face `i` points on screen: its length is 0 straight at the camera, nearing 1 side on.
vec2 facing(int i, vec2 aspect) {
    vec2 right = face_point(i, 0) * aspect;
    vec2 left = face_point(i, 1) * aspect;
    vec2 eyes = 0.5 * (right + left);
    vec2 mouth = 0.5 * (face_point(i, 3) + face_point(i, 4)) * aspect;
    vec2 nose = face_point(i, 2) * aspect;
    // Measured against the line from mouth to eyes: on a turned head the far eye is a poor
    // guess, so the line between the eyes tilts all over the place.
    float height = max(length(eyes - mouth), 0.004);
    vec2 up = (eyes - mouth) / height;
    vec2 across = vec2(up.y, -up.x);
    vec2 aim = vec2(dot(nose - 0.5 * (eyes + mouth), across) / height, LEVEL_NOSE - dot(eyes - nose, up) / height) / NOSE_DEPTH;
    aim /= max(length(aim), 1.0);
    return across * aim.x + up * aim.y;
}

vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    if (uv != clamp(uv, u_video_rect.xy, u_video_rect.zw)) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    float t = u_video_time;
    float flare = 1.0 + 1.5 * u_beat * s_react / 100.0;
    float power = s_power / 100.0;
    vec3 light = vec3(0.0);
    for (int i = 0; i < u_face_count; i++) {
        vec2 right = face_point(i, 0) * aspect;
        vec2 left = face_point(i, 1) * aspect;
        float spread = max(length(left - right), 0.004);
        vec2 aim = u_face_turn[i].w > 0.5 ? u_face_turn[i].xy : facing(i, aspect);
        // Close to head-on the direction means little, so the beam is short or not there at all.
        float out_of_screen = smoothstep(0.08, 0.3, length(aim));
        float reach = 3.0 * out_of_screen * out_of_screen * u_amount;
        float wander = 0.25 * s_sweep / 100.0 * sin(t * 1.3 + float(i) * 2.1);
        vec2 d = normalize(aim + vec2(0.0, 1e-5));
        d = vec2(d.x * cos(wander) - d.y * sin(wander), d.x * sin(wander) + d.y * cos(wander));
        float width = spread * 0.05 * (0.6 + power) * flare;
        for (int e = 0; e < 2; e++) {
            vec2 eye = e == 0 ? right : left;
            float lit = 1.0;
            if (meshed(i)) {
                eye = iris_of(i, e) * aspect;
                // Beams only come out of open eyes.
                lit = smoothstep(0.05, 0.35, e == 0 ? u_face_open[i].x : u_face_open[i].y);
            }
            // A beam starts at the face, so anything nearer than that hides it.
            float b = beam(p, eye, d, width, reach) * (1.0 - smoothstep(0.04, 0.12, depth_at(uv) - depth_at(0.5 * (right + left) / aspect)));
            float flash = 1.0 / (1.0 + pow(length(p - eye) / (spread * 0.14 * flare * (2.2 - 1.2 * out_of_screen)), 2.0));
            light += (b * power + flash * 0.9 * power) * u_face_seen[i] * lit * u_amount;
        }
    }
    vec3 colour = tint();
    return vec4(c + colour * light * 1.6 * flare + vec3(pow(min(light.r, 1.0), 4.0)) * 0.9, 1.0);
}
