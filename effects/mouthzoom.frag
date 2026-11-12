// Mouth Cam: the screen is filled by one mouth, and cuts to another when someone else starts
// talking. What's remembered between frames is kept in the alpha of the bottom row of u_prev:
// how much each face has been talking lately (0 to 7), how open its mouth was (8 to 15), whose
// mouth is shown (16) and how long it's been shown (17). Fading in, the picture zooms and turns in
// from the whole frame to the mouth; what's remembered goes on as it is.

const int OPEN_AT = 8;
const int SHOWN_AT = 16;
const int SINCE_AT = 17;
const float LONGEST = 4.0;

float kept(int x) {
    return texelFetch(u_prev, ivec2(x, 0), 0).a;
}

bool here(int i) {
    return i >= 0 && i < u_face_count && u_face_seen[i] > 0.5;
}

float openness(int i) {
    return meshed(i) ? u_face_open[i].z : 0.0;
}

// How much face `i` has been talking, this frame: mouths that keep moving while a voice is heard.
float talk(int i) {
    if (!here(i)) return 0.0;
    float before = kept(i);
    float moved = abs(openness(i) - kept(OPEN_AT + i));
    float voice = clamp(u_sound_speech + u_sound_shouting + u_sound_singing + u_sound_laughter, 0.0, 1.0);
    float heard = mix(0.25, 1.0, voice);
    return clamp(before * pow(0.35, u_dt) + moved * 5.0 * heard * s_eager / 100.0, 0.0, 1.0);
}

float face_size(int i) {
    return u_faces[i].w - u_faces[i].y;
}

// Whose mouth was shown, and whose is now: the one shown stays until it's lost, or someone else
// has been talking clearly more for a moment and it's been shown for long enough.
int shown_before() {
    return int(kept(SHOWN_AT) * 16.0 + 0.5) - 1;
}

int choose() {
    int shown = shown_before();
    float since = kept(SINCE_AT) * LONGEST;
    int best = -1;
    float most = 0.0;
    int biggest = -1;
    for (int i = 0; i < u_face_count; i++) {
        if (!here(i)) continue;
        float t = talk(i);
        if (t > most) { most = t; best = i; }
        if (biggest < 0 || face_size(i) > face_size(biggest)) biggest = i;
    }
    if (!here(shown)) return best >= 0 && most > 0.15 ? best : biggest;
    if (best >= 0 && best != shown && most > 0.3 && most > talk(shown) + 0.2 && since >= s_hold) return best;
    return shown;
}

float remembered(int chosen) {
    ivec2 pixel = ivec2(gl_FragCoord.xy);
    if (pixel.y != 0 || pixel.x > SINCE_AT) return 1.0;
    int x = pixel.x;
    if (x < MAX_FACES) return talk(x);
    if (x < OPEN_AT + MAX_FACES) return here(x - OPEN_AT) ? openness(x - OPEN_AT) : 0.0;
    if (x == SHOWN_AT) return float(chosen + 1) / 16.0;
    if (x == SINCE_AT) return chosen == shown_before() ? min(kept(SINCE_AT) + u_dt / LONGEST, 1.0) : 0.0;
    return 1.0;
}

vec4 effect(vec2 uv) {
    int chosen = choose();
    float memory = remembered(chosen);
    if (chosen < 0) {
        return vec4(texture(u_video, uv).rgb, memory);
    }
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 right, left, middle;
    if (meshed(chosen)) {
        right = mesh_point(chosen, 61).xy;
        left = mesh_point(chosen, 291).xy;
        middle = 0.5 * (mesh_point(chosen, 13).xy + mesh_point(chosen, 14).xy);
        middle = mix(0.5 * (right + left), middle, 0.5);
    } else {
        right = face_point(chosen, 3);
        left = face_point(chosen, 4);
        middle = 0.5 * (right + left);
    }
    vec2 across = (left - right) * aspect;
    // Lips reach a little past their corners; a mouth half the face's width is a guess for
    // faces too side-on to tell.
    float wide = max(length(across) * 1.25, face_size(chosen) * 0.35);
    float angle = s_level > 0.5 ? atan(across.y, across.x) : 0.0;
    if (across.x < 0.0 && s_level > 0.5) angle += 3.14159265;
    float since = kept(SINCE_AT) * LONGEST;
    if (chosen != shown_before()) since = 0.0;
    // A little push in as each new mouth is cut to.
    float punch = 1.0 + 0.18 * exp(-since * 6.0) * s_punch / 100.0;
    float scale = pow(wide / (aspect.x * s_fill / 100.0) / punch, u_amount);
    middle = mix(vec2(0.5), middle, u_amount);
    angle *= u_amount;
    vec2 p = (uv - 0.5) * aspect;
    float c = cos(angle), s = sin(angle);
    vec2 from = middle * aspect + vec2(c * p.x - s * p.y, s * p.x + c * p.y) * scale;
    vec2 at = from / aspect;
    vec3 colour = texture(u_video, at).rgb;
    if (at != clamp(at, u_video_rect.xy, u_video_rect.zw)) colour = vec3(0.0);
    // A flash of white on the cut.
    colour = mix(colour, vec3(1.0), exp(-since * 20.0) * s_flash / 100.0 * u_amount);
    return vec4(colour, memory);
}
