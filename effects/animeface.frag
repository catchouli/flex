// Anime Face: each face redrawn the way it's drawn in anime. With a mesh, the face is reshaped
// (a smaller nose and mouth, a narrower jaw and a smaller chin, pulled in the way a liquify tool
// does) and its skin evened into flat cel shading. Then an eye is drawn over each real one, the
// way they're inked: a tall iris shaded dark at the top and glowing at the bottom, a heavy
// winged upper lash and big painted highlights, in one of several styles. They narrow as a head
// turns, look where the head's facing, and blink now and then. Side on, the face is reshaped the
// way anime draws a profile (a small nose, with the mouth brought forward under it and a pointed
// chin) and the eye is drawn as it's seen from the side. Above them go thin arched brows in place of the real ones,
// which go up and down with the real ones and frown and worry with the sound. How high each real
// brow usually sits is learned as a face is watched, in the alpha of the bottom row of u_prev.
// Fading in, the faces are reshaped and evened more and more, and the drawn eyes, brows and blush
// swell in from half their size.

float hash(float n) {
    return fract(sin(n * 91.345) * 47453.5453);
}

// The colour of a face's eyes.
vec3 iris_colour(int face) {
    if (s_colour > 0.5) {
        if (s_colour < 1.5) return vec3(0.12, 0.42, 1.0);
        if (s_colour < 2.5) return vec3(0.62, 0.2, 1.0);
        if (s_colour < 3.5) return vec3(0.08, 0.8, 0.45);
        if (s_colour < 4.5) return vec3(1.0, 0.62, 0.08);
        return vec3(1.0, 0.13, 0.25);
    }
    return mix(hue(hash(float(face) + 2.0)), vec3(0.2, 0.45, 1.0), 0.25);
}

float inside_of(float distance_in, float px) {
    return clamp(distance_in / px + 0.5, 0.0, 1.0);
}

// A four-pointed glint.
float glint(vec2 q, float size) {
    q = abs(q) / max(size, 1e-4);
    return 1.0 - smoothstep(0.85, 1.0, sqrt(q.x) + sqrt(q.y));
}

// One eye, with x running outwards from the nose and y up, in units of the eye. `side` is which
// way outwards is on screen, so the highlights land on the same side of both eyes. `shine` adds
// twinkling stars to the highlights, and `shock` shrinks the iris and pupil to startled dots.
// How a style of eye is drawn: how tall it opens above and below the middle, how much its outer
// corner turns up, and how big its iris and pupil are, how much it shines and how heavy its lashes
// are, compared with the sparkly style.
struct Style {
    vec2 opening;
    float slant, iris, pupil, shine, lash;
};

// A style of eyes by number: 1 sparkly, 2 round, 3 narrow, 4 sleepy, 5 fierce.
Style style_by(int pick) {
    if (pick == 2) return Style(vec2(1.3, 1.05), 0.0, 1.12, 0.75, 1.15, 0.75); // round
    if (pick == 3) return Style(vec2(0.6, 0.45), 0.12, 0.8, 0.8, 0.5, 1.15); // narrow
    if (pick == 4) return Style(vec2(0.5, 0.75), -0.05, 1.0, 0.9, 0.55, 1.1); // sleepy
    if (pick == 5) return Style(vec2(0.62, 0.5), 0.32, 0.72, 0.45, 0.3, 1.35); // fierce
    return Style(vec2(1.08, 0.8), 0.0, 1.0, 1.0, 1.0, 1.0); // sparkly
}

// The style that suits a face of these proportions (how open its eyes are, how wide it is for its
// height, and how wide its jaw is for its cheeks): wide-open eyes are drawn round, hooded ones
// sleepy on a broad face and narrow on a long one, a square jaw with narrowish eyes fierce, and
// the rest sparkly.
int style_for(vec3 proportions) {
    float eye = proportions.x, wide = proportions.y, jaw = proportions.z;
    if (eye >= 0.44) return 2;
    if (eye <= 0.3) return wide >= 0.82 ? 4 : 3;
    if (jaw >= 0.86 && eye < 0.38) return 5;
    return 1;
}

// What's remembered from frame to frame, in the alpha of the bottom row of u_prev: for each face,
// first how high each brow usually sits, then the average of its proportions over the first
// frames it's seen, how many frames that is, and the style that was settled on.
const int PROPORTIONS_AT = MAX_FACES * 2;
const int LEARN_FRAMES = 8;
float kept(int x) {
    return u_frame > 2 ? texelFetch(u_prev, ivec2(x, 0), 0).a : 0.0;
}

// A face's style of eyes: s_style picks one for everyone. Otherwise each face's suits its
// proportions, settled once it's been seen a few frames; a face never seen closely enough has one
// of its own at random.
Style style_of(int face) {
    int pick = int(s_style + 0.5);
    if (pick > 0) return style_by(pick);
    int at = PROPORTIONS_AT + face * 5;
    if (kept(at + 4) > 0.5) return style_by(int(kept(at + 4) + 0.5));
    if (kept(at + 3) > 0.5) return style_by(style_for(vec3(kept(at), kept(at + 1), kept(at + 2))));
    return style_by(1 + int(hash(float(face) + 11.0) * 4.999));
}

vec4 draw_eye(vec2 l, vec2 look, float open, vec3 colour, float side, float pulse, float shine, float shock, float px, Style st) {
    l.y -= st.slant * l.x;
    vec2 tall = vec2(1.0, l.y > 0.0 ? max(st.opening.x * open, 0.03) : st.opening.y);
    float round_d = length(l / tall);
    float inside = inside_of(1.0 - round_d, px);

    vec3 white = vec3(0.98, 0.97, 1.0);
    white = mix(white, vec3(0.62, 0.66, 0.88), smoothstep(0.35, 1.0, l.y / max(tall.y, 0.05)) * step(0.0, l.y));

    vec2 centre = vec2(look.x, look.y + 0.06);
    // Everything in the iris is drawn in its own units, so a shocked iris shrinks as a piece.
    float shrink = 1.0 - 0.45 * shock;
    vec2 li = (l - centre) / shrink;
    float ipx = px / shrink;
    vec2 ir = li / (vec2(0.74, 1.1) * st.iris);
    float d = length(ir);
    vec3 dark = colour * 0.2, mid = colour * 0.8, bright = mix(colour, vec3(1.0), 0.4) * 1.25;
    vec3 iris = mix(bright, mid, smoothstep(-0.95, -0.1, ir.y));
    iris = mix(iris, dark, smoothstep(-0.2, 0.7, ir.y));
    float angle = atan(ir.y, ir.x);
    iris *= 0.86 + 0.28 * (0.5 + 0.5 * sin(angle * 23.0 + 1.3 * sin(angle * 5.0))) * smoothstep(0.25, 0.9, d);
    iris += bright * 0.55 * smoothstep(0.55, 0.92, d) * (1.0 - smoothstep(-0.8, -0.1, ir.y));
    iris = mix(iris, dark * 0.45, smoothstep(0.84, 0.98, d));
    float pupil = length((li - vec2(0.0, 0.1)) / (vec2(0.27, 0.44) * st.iris * st.pupil * (1.0 + 0.12 * (pulse - 1.0)) * (1.0 - 0.5 * shock)));
    iris = mix(iris, vec3(0.02, 0.01, 0.05), 1.0 - smoothstep(0.92, 1.08, pupil));

    vec3 eye = mix(white, iris, inside_of(1.0 - d, ipx / (0.74 * st.iris)));

    // A shocked eye keeps only a little of its shine.
    float amount = s_sparkle / 100.0 * (1.0 - 0.6 * shock) * min(st.shine, 1.0);
    vec2 q1 = li - vec2(-0.26 * side, 0.4);
    float highlight = inside_of(1.0 - length(q1 / (vec2(0.27, 0.34) * pulse)), ipx / 0.27);
    vec2 q2 = li - vec2(0.3 * side, -0.5);
    highlight = max(highlight, inside_of(1.0 - length(q2) / (0.14 * pulse), ipx / 0.14));
    vec2 q3 = li - vec2(-0.05 * side, -0.68);
    highlight = max(highlight, 0.8 * inside_of(1.0 - length(q3) / 0.07, ipx / 0.07));
    highlight = max(highlight, glint(q1 + vec2(0.1 * side, -0.1), 0.25 + 0.6 * (pulse - 1.0)));
    // Starry-eyed: more glints, twinkling in turn.
    if (shine > 0.0) {
        float twinkle = 0.5 + 0.5 * sin(u_time * 5.0 + side * 1.7);
        highlight = max(highlight, shine * glint(li - vec2(0.36 * side, -0.1), 0.1 + 0.16 * twinkle));
        highlight = max(highlight, shine * glint(li - vec2(-0.34 * side, -0.4), 0.08 + 0.12 * (1.0 - twinkle)));
        highlight = max(highlight, shine * glint(q1 + vec2(0.1 * side, -0.1), 0.4 + 0.25 * twinkle));
    }
    eye = mix(eye, vec3(1.0), highlight * amount);

    vec4 out_colour = vec4(eye, inside);

    // Inked lines, darkest at the outside corner.
    vec3 ink = vec3(0.07, 0.04, 0.07);
    float thick = mix(0.14, 0.42, smoothstep(-0.7, 0.85, l.x)) * st.lash;
    float upper = l.y > -0.06 ? inside_of(1.0 + thick - round_d, px) * inside_of(round_d - 0.98, px) : 0.0;
    float t = (l.x - 0.82) / 0.72;
    float wing = 0.0;
    if (t > 0.0 && t < 1.0) wing = inside_of(mix(0.17, 0.0, t) - abs(l.y - mix(0.12, 0.42, t)), px);
    float t2 = (l.x - 0.66) / 0.62;
    if (t2 > 0.0 && t2 < 1.0) wing = max(wing, inside_of(mix(0.08, 0.0, t2) - abs(l.y - mix(0.55, 0.92, t2)), px));
    float lower = l.y < 0.0 ? inside_of(1.06 - round_d, px) * inside_of(round_d - 0.985, px) * smoothstep(-0.6, 0.2, l.x) : 0.0;
    float crease = l.y > 0.0 ? 0.55 * inside_of(0.03 - abs(length(l / vec2(1.05, 1.5 * max(open, 0.05))) - 1.0), px) * smoothstep(-0.6, -0.1, l.x) * (1.0 - smoothstep(0.65, 1.0, l.x)) : 0.0;
    float lines = max(max(upper, wing), lower);
    out_colour.rgb = mix(out_colour.rgb, ink, lines);
    out_colour.a = max(out_colour.a, lines);
    out_colour.rgb = mix(out_colour.rgb, vec3(0.3, 0.17, 0.16), crease * (1.0 - out_colour.a));
    out_colour.a = max(out_colour.a, crease);
    return out_colour;
}

// An eye seen from the side, with x running forwards (the way the face looks) and y up, in units
// of the eye: a wedge open at the front, coming to a point at the back, with a narrow iris at the
// front and the lashes sweeping back past the point.
vec4 draw_side_eye(vec2 l, float look_up, float open, vec3 colour, float pulse, float shine, float shock, float px, Style st) {
    l.y += st.slant * l.x;
    float top = max(0.95 * st.opening.x * open, 0.03), bottom = -0.62 * st.opening.y;
    vec2 back = vec2(-0.6, -0.05);
    float rise = clamp(l.y / max(l.y > 0.0 ? top : -bottom, 0.03), -1.0, 1.0);
    float front = 0.32 - 0.1 * rise * rise;
    float t = clamp((l.x - back.x) / (front - back.x), 0.0, 1.0);
    float upper = mix(back.y, top, 1.0 - (1.0 - t) * (1.0 - t));
    float lower = mix(back.y, bottom, sqrt(t));
    float inside = inside_of(upper - l.y, px) * inside_of(l.y - lower, px) * inside_of(front - l.x, px) * inside_of(l.x - back.x, px);

    vec3 white = mix(vec3(0.98, 0.97, 1.0), vec3(0.62, 0.66, 0.88), smoothstep(0.3, 1.0, l.y / max(top, 0.05)));
    float shrink = 1.0 - 0.45 * shock;
    vec2 li = (l - vec2(front - 0.22 * st.iris, 0.04 + look_up)) / shrink;
    vec2 ir = li / (vec2(0.24, 0.66) * st.iris);
    float d = length(ir);
    vec3 dark = colour * 0.2, mid = colour * 0.8, bright = mix(colour, vec3(1.0), 0.4) * 1.25;
    vec3 iris = mix(bright, mid, smoothstep(-0.95, -0.1, ir.y));
    iris = mix(iris, dark, smoothstep(-0.2, 0.7, ir.y));
    iris = mix(iris, dark * 0.45, smoothstep(0.84, 0.98, d));
    float pupil = length((li - vec2(0.02, 0.08)) / (vec2(0.09, 0.28) * st.iris * st.pupil * (1.0 - 0.5 * shock)));
    iris = mix(iris, vec3(0.02, 0.01, 0.05), 1.0 - smoothstep(0.92, 1.08, pupil));
    vec3 eye = mix(white, iris, inside_of(1.0 - d, px / (0.24 * st.iris)));
    float amount = s_sparkle / 100.0 * (1.0 - 0.6 * shock) * min(st.shine, 1.0);
    float highlight = inside_of(1.0 - length((li - vec2(0.04, 0.3)) / (vec2(0.09, 0.17) * pulse)), px / 0.09);
    highlight = max(highlight, inside_of(1.0 - length(li - vec2(-0.05, -0.38)) / (0.06 * pulse), px / 0.06));
    if (shine > 0.0) highlight = max(highlight, shine * glint(li - vec2(0.0, 0.3), 0.18 + 0.1 * sin(u_time * 5.0)));
    eye = mix(eye, vec3(1.0), highlight * amount);
    vec4 out_colour = vec4(eye, inside);

    // A heavy lash along the top, flicking back past the corner, and a thin line under.
    vec3 ink = vec3(0.07, 0.04, 0.07);
    float thick = mix(0.1, 0.22, 1.0 - t) * st.lash;
    float lash = l.x > back.x && l.x < front + 0.04 ? inside_of(thick - abs(l.y - upper - 0.5 * thick + 0.02), px) : 0.0;
    float w = (back.x - l.x) / 0.4;
    if (w > 0.0 && w < 1.0) lash = max(lash, inside_of(mix(0.16, 0.0, w) * st.lash - abs(l.y - back.y - 0.05 - 0.28 * w), px));
    float under = l.x > back.x && t > 0.35 ? inside_of(0.035 - abs(l.y - lower + 0.01), px) : 0.0;
    float lines = max(lash, under);
    out_colour.rgb = mix(out_colour.rgb, ink, lines);
    out_colour.a = max(out_colour.a, lines);
    return out_colour;
}

// How side on a face is, from 0 (facing the camera) to 1 (in profile): from which way the head
// points where that's known, or else from how close together its eyes look.
float side_on(int face, vec2 left, vec2 right, float height) {
    if (u_face_turn[face].w > 0.5) return smoothstep(0.65, 0.9, abs(u_face_turn[face].x));
    return 1.0 - smoothstep(0.4, 0.7, length(left - right) / height);
}

// A face's mesh point, with the screen's shape taken out.
vec2 at_mesh(int face, int n, vec2 aspect) {
    return mesh_point(face, n).xy * aspect;
}

// How open a face's eyes are (height over width), how wide it is for its height (allowing for
// the head being turned), and how wide its jaw is for its cheeks, from the mesh.
vec3 proportions_of(int face, vec2 aspect) {
    float right = length(at_mesh(face, 159, aspect) - at_mesh(face, 145, aspect)) / max(length(at_mesh(face, 33, aspect) - at_mesh(face, 133, aspect)), 1e-4);
    float left = length(at_mesh(face, 386, aspect) - at_mesh(face, 374, aspect)) / max(length(at_mesh(face, 263, aspect) - at_mesh(face, 362, aspect)), 1e-4);
    float cheeks = length(at_mesh(face, 454, aspect) - at_mesh(face, 234, aspect));
    float turned = u_face_turn[face].w > 0.5 ? max(sqrt(1.0 - min(u_face_turn[face].x * u_face_turn[face].x, 1.0)), 0.5) : 1.0;
    float wide = cheeks / turned / max(length(at_mesh(face, 10, aspect) - at_mesh(face, 152, aspect)), 1e-4);
    return vec3(0.5 * (right + left), wide, length(at_mesh(face, 397, aspect) - at_mesh(face, 172, aspect)) / max(cheeks, 1e-4));
}

// Pulls what's in an oval around `centre` in towards it, so the thing there is drawn `size` times
// as big (above 0.45, so nothing folds over), easing back to untouched at the oval's edge. `q` is
// where on screen is being drawn; this says where to look for it. `half_size` is across and up.
vec2 resize(vec2 q, vec2 centre, vec2 across, vec2 up, vec2 half_size, float size) {
    vec2 d = q - centre;
    float x = length(vec2(dot(d, across), dot(d, up)) / half_size);
    if (x >= 1.0) return q;
    float fall = 1.0 - x * x;
    return centre + d * (1.0 + (1.0 / size - 1.0) * fall * fall);
}

// Moves what's around `from` by `by`, easing off over `reach`.
vec2 shift(vec2 q, vec2 from, vec2 by, float reach) {
    float x = length(q - from) / reach;
    if (x >= 1.0) return q;
    float fall = 1.0 - x * x;
    return q - by * fall * fall;
}

// Where to look in the picture for what's drawn at `q`, with face `i` reshaped by `k` (0 to 1).
vec2 reshaped(int i, vec2 q, float k, vec2 aspect) {
    vec2 chin = at_mesh(i, 152, aspect), top = at_mesh(i, 10, aspect);
    vec2 right_edge = at_mesh(i, 234, aspect), left_edge = at_mesh(i, 454, aspect);
    float wide = max(length(left_edge - right_edge), 1e-3);
    vec2 up = normalize(top - chin + vec2(0.0, 1e-5));
    vec2 across = vec2(up.y, -up.x);
    // Only near the face.
    vec2 middle = 0.5 * (chin + top);
    if (length(q - middle) > 1.2 * max(wide, length(top - chin))) return q;
    // The jaw comes in and the chin up, to a point.
    for (int side = 0; side < 2; side++) {
        vec2 jaw = at_mesh(i, side == 0 ? 172 : 397, aspect);
        vec2 lower = at_mesh(i, side == 0 ? 136 : 365, aspect);
        vec2 inwards = normalize(chin - jaw + up * wide * 0.3);
        q = shift(q, jaw, inwards * 0.07 * wide * k, 0.32 * wide);
        q = shift(q, lower, inwards * 0.05 * wide * k, 0.26 * wide);
    }
    q = shift(q, chin, up * 0.05 * wide * k, 0.3 * wide);
    // A small nose, and a small mouth.
    q = resize(q, at_mesh(i, 1, aspect), across, up, vec2(0.17, 0.2) * wide, 1.0 - 0.45 * k);
    vec2 mouth = 0.5 * (at_mesh(i, 13, aspect) + at_mesh(i, 14, aspect));
    q = resize(q, mouth, across, up, vec2(0.27, 0.2) * wide, 1.0 - 0.35 * k);
    return q;
}

// Where to look in the picture for what's drawn at `q`, with face `i` reshaped side on by `k` (0 to
// 1), the way anime draws a profile: the nose smaller and pulled back, the mouth brought forward
// to be almost under its tip, and the chin to a point. From the five points, as profiles mostly
// have no mesh.
vec2 profile_reshaped(int i, vec2 q, float k, vec2 aspect) {
    vec2 right = face_point(i, 0) * aspect, left = face_point(i, 1) * aspect;
    vec2 eyes = 0.5 * (right + left), nose = face_point(i, 2) * aspect;
    vec2 mouth = 0.5 * (face_point(i, 3) + face_point(i, 4)) * aspect;
    float height = max(length(eyes - mouth), 0.004);
    if (length(q - nose) > 2.5 * height) return q;
    vec2 up = (eyes - mouth) / height;
    vec2 across = vec2(up.y, -up.x);
    vec2 forward = across * (dot(nose - eyes, across) < 0.0 ? -1.0 : 1.0);
    q = resize(q, nose - forward * 0.1 * height, forward, up, vec2(0.32, 0.3) * height, 1.0 - 0.4 * k);
    q = shift(q, nose, -forward * 0.07 * height * k, 0.32 * height);
    q = shift(q, mouth + forward * 0.25 * height, forward * 0.16 * height * k, 0.42 * height);
    q = shift(q, mouth - up * 0.6 * height + forward * 0.1 * height, (forward * 0.06 + up * 0.04) * height * k, 0.32 * height);
    q = shift(q, mouth - up * 0.55 * height - forward * 0.45 * height, up * 0.08 * height * k, 0.4 * height);
    return q;
}

// A face's skin colour: what's on its cheeks, forehead and the bridge of its nose, as the atlas
// has it (not the chin, which may have a beard).
vec3 skin_tone(int face) {
    vec4 sum = vec4(0.0);
    vec2 spots[6] = vec2[6](vec2(0.3, 0.5), vec2(0.7, 0.5), vec2(0.5, 0.78), vec2(0.4, 0.76), vec2(0.6, 0.76), vec2(0.5, 0.56));
    for (int n = 0; n < 6; n++) {
        vec4 s = face_texture(face, spots[n]);
        sum += vec4(s.rgb * s.a, s.a);
    }
    return sum.rgb / max(sum.a, 1e-4);
}

float luma(vec3 c) {
    return dot(c, vec3(0.299, 0.587, 0.114));
}

// The picture at `at` (in the original frame), with skin evened into flat cel shading by
// `amount`, for face `face`: its blemishes and texture blurred away, its colour the face's own,
// and its light and shade in two soft steps. Only what looks like the face's skin is evened, so
// beards, glasses and hair over the face are left, and it fades out towards the face's edges.
vec3 even_skin(vec2 at, int face, float amount, float blur, vec3 tone) {
    vec3 c = texture(u_video, at).rgb;
    float skin = (skin_at(at) + nose_at(at)) * (1.0 - lips_at(at)) * (1.0 - eye_at(at)) * (1.0 - brows_at(at)) * (1.0 - mouth_at(at));
    if (skin <= 0.0 || amount <= 0.0) return c;
    vec2 a = face_uv(at);
    skin *= 1.0 - smoothstep(0.72, 0.95, length((a - vec2(0.5, 0.47)) / vec2(0.5, 0.43)));
    float near = luma(c) / max(luma(tone), 0.02);
    vec3 hue_of = c / max(luma(c), 0.02) - tone / max(luma(tone), 0.02);
    skin *= smoothstep(0.3, 0.55, near) * (1.0 - smoothstep(0.18, 0.4, length(hue_of)));
    if (skin <= 0.0) return c;
    vec3 soft = c;
    for (int n = 1; n <= 12; n++) {
        float a = float(n) * 2.39996;
        vec2 o = vec2(cos(a), sin(a)) * blur * sqrt(float(n) / 12.0);
        soft += texture(u_video, at + o / vec2(u_resolution.x / u_resolution.y, 1.0)).rgb;
    }
    soft /= 13.0;
    float light = luma(soft) / max(luma(tone), 0.02);
    float shade = mix(0.74, 1.0, smoothstep(0.76, 0.9, light));
    shade = mix(shade, 1.12, 0.7 * smoothstep(1.2, 1.32, light));
    vec3 flat_ = tone * shade;
    // A quarter of it is the face's own blurred skin, so the light in the shot still shows.
    vec3 evened = mix(soft, flat_, 0.75);
    return mix(c, evened, skin * amount);
}

// How far above its eye a real brow is, in eye widths, from the mesh. `e` 0 is the right eye.
float brow_height(int face, int e, vec2 up, vec2 aspect) {
    vec2 brow = at_mesh(face, e == 0 ? 105 : 334, aspect), lid = at_mesh(face, e == 0 ? 159 : 386, aspect);
    vec2 inner = at_mesh(face, e == 0 ? 133 : 362, aspect), outer = at_mesh(face, e == 0 ? 33 : 263, aspect);
    return dot(brow - lid, up) / max(length(outer - inner), 1e-4);
}

// How high a brow usually sits, as learned so far, or -1 before it's known.
float usual_height(int face, int e) {
    return u_frame > 2 ? texelFetch(u_prev, ivec2(face * 2 + e, 0), 0).a : -1.0;
}

// A drawn brow, with x running outwards from the nose and y up, in units of the eye, as
// draw_eye has them. `lift` raises it, `frown` brings its inner end down and `worry` up.
float draw_brow(vec2 l, float lift, float frown, float worry, float px) {
    if (l.x < -1.2 || l.x > 1.5 || l.y < 0.6 || l.y > 3.4) return 0.0;
    float inner_end = -0.8, outer_end = 1.2;
    float x = clamp(l.x, inner_end, outer_end);
    float along = (x - inner_end) / (outer_end - inner_end);
    float inner_part = 1.0 - smoothstep(0.0, 0.75, along);
    float arch = x - 0.25;
    float y = 1.75 + lift + 0.3 * (1.0 - arch * arch) - 0.1 * frown + (0.45 * worry - 0.55 * frown) * inner_part;
    float thick = mix(0.3, 0.04, smoothstep(0.0, 1.0, along));
    // Round at the inner end, and tapering to a point at the outer.
    float d = l.x < inner_end ? length(l - vec2(inner_end, y)) : abs(l.y - y);
    return inside_of(0.5 * thick - d, px) * inside_of(outer_end - l.x, px);
}

vec4 effect(vec2 uv) {
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);
    vec2 p = uv * aspect;
    // Reshape the faces with meshes, and even their skin.
    vec2 from = p;
    float shape = s_shape / 100.0 * u_amount;
    for (int i = 0; i < u_face_count; i++) {
        if (u_face_seen[i] <= 0.0 || shape <= 0.0) continue;
        vec2 right = face_point(i, 0) * aspect, left = face_point(i, 1) * aspect;
        float height = max(length(0.5 * (right + left) - 0.5 * (face_point(i, 3) + face_point(i, 4)) * aspect), 0.004);
        float side = side_on(i, left, right, height);
        if (meshed(i) && side < 1.0) from = mix(from, reshaped(i, from, shape, aspect), u_face_seen[i] * (1.0 - side));
        if (side > 0.0) from = mix(from, profile_reshaped(i, from, shape, aspect), u_face_seen[i] * side);
    }
    vec2 at = clamp(from / aspect, u_video_rect.xy, u_video_rect.zw);
    vec3 c = texture(u_video, at).rgb;
    int there = face_there(at);
    if (there >= 0 && meshed(there) && (s_skin > 0.0 || s_brows > 0.5)) {
        float wide = length(at_mesh(there, 454, aspect) - at_mesh(there, 234, aspect));
        vec3 tone = skin_tone(there);
        c = even_skin(at, there, s_skin / 100.0 * u_face_seen[there] * u_amount, 0.035 * wide, tone);
        // The real brows are covered with skin, for the drawn ones.
        if (s_brows > 0.5) {
            vec3 above = texture(u_video, at + vec2(0.0, 0.05 * wide)).rgb;
            vec3 skin = mix(above, tone * clamp(luma(above) / max(luma(tone), 0.02), 0.6, 1.1), 0.8);
            c = mix(c, skin, smoothstep(0.0, 0.5, brows_at(at)) * u_face_seen[there] * u_amount);
        }
    }

    // Keep learning how high each real brow usually sits, and each face's proportions.
    ivec2 pixel = ivec2(gl_FragCoord.xy);
    float alpha = 1.0;
    if (pixel.y == 0 && pixel.x < PROPORTIONS_AT) {
        int i = pixel.x / 2, e = pixel.x - i * 2;
        alpha = -1.0;
        if (i < u_face_count && u_face_seen[i] > 0.5 && meshed(i)) {
            vec2 up = normalize(at_mesh(i, 10, aspect) - at_mesh(i, 152, aspect) + vec2(0.0, 1e-5));
            float now = brow_height(i, e, up, aspect), usual = usual_height(i, e);
            alpha = usual < 0.0 ? now : mix(usual, now, 0.01);
        }
    } else if (pixel.y == 0 && pixel.x < PROPORTIONS_AT + MAX_FACES * 5) {
        int i = (pixel.x - PROPORTIONS_AT) / 5, k = pixel.x - PROPORTIONS_AT - i * 5;
        int at = PROPORTIONS_AT + i * 5;
        alpha = 0.0;
        if (i < u_face_count && u_face_seen[i] > 0.5) {
            vec3 average = vec3(kept(at), kept(at + 1), kept(at + 2));
            float count = kept(at + 3), settled = kept(at + 4);
            vec2 right = face_point(i, 0) * aspect, left = face_point(i, 1) * aspect;
            float height = max(length(0.5 * (right + left) - 0.5 * (face_point(i, 3) + face_point(i, 4)) * aspect), 0.004);
            // Learned only from a good look at it, facing the camera.
            if (settled < 0.5 && meshed(i) && side_on(i, left, right, height) < 0.4) {
                average = (average * count + proportions_of(i, aspect)) / (count + 1.0);
                count += 1.0;
                if (count >= float(LEARN_FRAMES)) settled = float(style_for(average));
            }
            alpha = k < 3 ? average[k] : k == 3 ? count : settled;
        }
    }
    float pulse = 1.0 + 0.45 * u_beat * s_react / 100.0;
    // What kind of sound it is: crying brings tears, singing and sweet music make eyes shine, and
    // shouts and bangs shock them.
    float react = s_react / 50.0;
    float cry = clamp(u_sound_crying * react, 0.0, 1.0);
    float shine = clamp((u_sound_singing + u_sound_tender_music + u_sound_happy_music) * react, 0.0, 1.0);
    float shock = clamp(max(u_sound_shouting, u_sound_explosions) * react, 0.0, 1.0);
    float frown = clamp((u_sound_shouting + u_sound_angry_music) * react, 0.0, 1.0);
    float worry = clamp((u_sound_crying + u_sound_sad_music + 0.5 * u_sound_scary_music) * react, 0.0, 1.0) * (1.0 - frown);
    for (int i = 0; i < u_face_count; i++) {
        float seen = u_face_seen[i] * u_amount;
        if (seen <= 0.0) continue;
        vec2 right = face_point(i, 0) * aspect, left = face_point(i, 1) * aspect;
        vec2 nose = face_point(i, 2) * aspect;
        vec2 eyes = 0.5 * (right + left);
        vec2 mouth = 0.5 * (face_point(i, 3) + face_point(i, 4)) * aspect;
        float height = max(length(eyes - mouth), 0.004);
        vec2 up = (eyes - mouth) / height;
        vec2 across = vec2(up.y, -up.x);
        float frontal = max(length(left - right), height * 0.95);
        float size = frontal * 0.17 * s_size / 100.0 * (0.5 + 0.5 * u_amount);
        float look = clamp(dot(nose - eyes, across) / frontal * 1.3, -0.32, 0.32);
        float side = side_on(i, left, right, height);
        vec2 forward = across * (dot(nose - eyes, across) < 0.0 ? -1.0 : 1.0);
        Style st = style_of(i);

        float open = 1.0;
        if (s_blink > 0.5) {
            float period = 3.2 + 2.6 * hash(float(i) + 7.0);
            float at = fract(u_time / period + hash(float(i))) * period;
            open = max(smoothstep(0.0, 0.07, abs(at - 0.09) - 0.015), 0.03);
        }

        for (int e = 0; e < 2; e++) {
            vec2 eye = e == 0 ? right : left;
            // Side on, only the near eye shows.
            if (length(left - right) < 0.55 * height && abs(dot(eye - nose, across)) < abs(dot((e == 0 ? left : right) - nose, across))) continue;
            float out_x = sign(dot(eye - eyes, across) + (e == 0 ? -1e-5 : 1e-5));
            vec2 outward = across * out_x;
            float squash = clamp(abs(dot(eye - nose, across)) / (0.5 * frontal), 0.35, 1.25);
            vec2 centre = eye + outward * frontal * 0.04 - up * frontal * 0.02;

            // Blush under the eye, hatched the way it's drawn.
            vec2 cheek = p - (centre - up * size * 1.7 + outward * size * 0.25);
            float blush = 1.0 - smoothstep(0.5, 1.0, length(vec2(dot(cheek, across), dot(cheek, up) * 2.1)) / (size * 1.05));
            if (blush > 0.0 && s_blush > 0.0) {
                float hatch = 0.75 + 0.25 * smoothstep(-0.2, 0.2, sin((dot(cheek, across) - dot(cheek, up) * 0.6) / size * 18.0));
                c = mix(c, c * vec3(1.05, 0.72, 0.72) + vec3(0.2, 0.03, 0.08), blush * hatch * seen * s_blush / 100.0);
            }

            // With a mesh, these eyes do what the real ones are doing.
            float eye_open = open;
            vec2 gaze = vec2(look * out_x, 0.0);
            if (meshed(i)) {
                if (s_blink > 0.5) eye_open = max(smoothstep(0.0, 0.7, e == 0 ? u_face_open[i].x : u_face_open[i].y), 0.03);
                vec2 inner = mesh_point(i, e == 0 ? 133 : 362).xy * aspect, outer = mesh_point(i, e == 0 ? 33 : 263).xy * aspect;
                float wide = max(length(outer - inner), 1e-4);
                vec2 from_middle = iris_of(i, e) * aspect - 0.5 * (inner + outer);
                gaze = vec2(clamp(dot(from_middle, outer - inner) / (wide * wide) * 2.4, -0.32, 0.32), clamp(dot(from_middle, up) / wide * 2.0, -0.2, 0.2));
            }

            vec2 rel = p - centre;
            vec2 l = vec2(dot(rel, outward) / (size * squash), dot(rel, up) / size);
            float px = 1.6 / (u_resolution.y * size);

            // Tears stream down from under the eye, running further the harder it sounds like crying.
            if (cry > 0.0) {
                float run = -l.y - 0.72;
                float reach = 2.6 * cry;
                if (run > -0.2 && run < reach + 0.3) {
                    float wobble = 0.06 * sin(run * 3.0 - u_time * 4.0 + float(e) * 2.0);
                    float x = (l.x - 0.4) * squash - wobble;
                    float width = 0.12 + 0.04 * run;
                    float stream = inside_of(width - abs(x), px) * smoothstep(-0.2, 0.05, run) * (1.0 - smoothstep(reach, reach + 0.3, run));
                    float flow = 0.8 + 0.2 * sin(run * 7.0 - u_time * 9.0);
                    vec3 tear = mix(vec3(0.55, 0.8, 1.0) * flow, vec3(0.95, 0.98, 1.0), 1.0 - smoothstep(0.0, 0.35 * width, abs(x + 0.4 * width)));
                    c = mix(c, tear, stream * 0.75 * smoothstep(0.0, 0.4, cry) * seen);
                }
            }

            // A brow above it, colouring like the real one but darker.
            if (s_brows > 0.5) {
                float lift = 0.35 * shock;
                vec3 colour = vec3(0.1, 0.06, 0.06);
                if (meshed(i)) {
                    float usual = usual_height(i, e);
                    vec2 wide_eye = at_mesh(i, e == 0 ? 33 : 263, aspect) - at_mesh(i, e == 0 ? 133 : 362, aspect);
                    if (usual >= 0.0) lift += clamp(1.6 * (brow_height(i, e, up, aspect) - usual) * length(wide_eye) / size, -0.4, 0.9);
                    colour = mix(colour, min(texture(u_video, mesh_point(i, e == 0 ? 105 : 334).xy).rgb, vec3(0.5)) * 0.6, 0.5);
                }
                float brow = draw_brow(l, lift, frown, worry, px);
                c = mix(c, colour, brow * seen);
            }

            // Side on, the eye's drawn as it's seen from the side, turning into it as the head turns.
            vec4 drawn = vec4(0.0);
            if (side < 1.0 && dot(l, l) < 6.0) drawn = draw_eye(l, gaze, eye_open, iris_colour(i), out_x * sign(across.x + 1e-5), pulse, shine, shock, px, st);
            if (side > 0.0) {
                vec2 ls = vec2(dot(rel, forward), dot(rel, up)) / size;
                vec4 profile = dot(ls, ls) < 6.0 ? draw_side_eye(ls, gaze.y, eye_open, iris_colour(i), pulse, shine, shock, px, st) : vec4(0.0);
                drawn = mix(drawn, profile, side);
            }
            c = mix(c, drawn.rgb, drawn.a * seen);
        }
    }
    return vec4(c, alpha);
}
