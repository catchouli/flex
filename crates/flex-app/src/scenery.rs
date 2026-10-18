//! Backgrounds and pictures for themes, all drawn in code: tiled patterns (stone,
//! pinstripes, falling code), painted scenes (rolling hills, a farmyard) and pixel-art
//! sprites. Nothing here is taken from a game or website.

use eframe::egui::{self, Color32, CornerRadius, Painter, Pos2, Rect, pos2, vec2};

/// A repeatable hash: 0..1 for (x, y, seed).
pub fn noise(x: usize, y: usize, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263) ^ seed.wrapping_mul(2_246_822_519);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xff) as f32 / 255.0
}

fn scale(c: Color32, f: f32) -> Color32 {
    let s = |v: u8| (v as f32 * f).clamp(0.0, 255.0) as u8;
    Color32::from_rgb(s(c.r()), s(c.g()), s(c.b()))
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let (a, b) = (a.to_srgba_unmultiplied(), b.to_srgba_unmultiplied());
    let m = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(m(0), m(1), m(2), m(3))
}

// ---------------------------------------------------------------------------------------
// Tiled patterns

/// Points per pattern pixel when tiled.
pub fn pattern_scale(name: &str) -> f32 {
    match name {
        "pinstripes" => 1.0,
        "rain" => 3.0,
        _ => 2.0,
    }
}

/// A tile for a built-in pattern, tinted from the background colour; `None` for unknown
/// patterns and the painted scenes.
pub fn make_pattern(name: &str, base: Color32) -> Option<egui::ColorImage> {
    match name {
        "stone" => Some(stone(base)),
        "pinstripes" => {
            // Two pixels light, two a shade darker.
            let pixels = (0..8 * 8).map(|i| if (i / 8) % 4 < 2 { base } else { scale(base, 0.955) }).collect();
            Some(egui::ColorImage::new([8, 8], pixels))
        }
        "rain" => Some(rain(base)),
        _ => None,
    }
}

/// Uneven stone blocks with speckle.
fn stone(base: Color32) -> egui::ColorImage {
    const SIZE: usize = 256;
    const COURSE: usize = 32;
    // Each course is cut into blocks of uneven width (the cuts repeat with the tile).
    let cuts: Vec<Vec<usize>> = (0..SIZE / COURSE)
        .map(|c| {
            let mut x = (noise(c, 0, 3) * 40.0) as usize;
            let mut cuts = Vec::new();
            while x < SIZE {
                cuts.push(x);
                x += 36 + (noise(c, x, 5) * 44.0) as usize;
            }
            cuts
        })
        .collect();
    let mut pixels = Vec::with_capacity(SIZE * SIZE);
    for y in 0..SIZE {
        let course = y / COURSE;
        let row = &cuts[course];
        for x in 0..SIZE {
            // Before the first cut is the end of the course's last block (wrapped round).
            let block = row.iter().rposition(|&c| c <= x).unwrap_or(row.len() - 1);
            let shade = 0.78 + noise(block, course, 7) * 0.32;
            let grain = 0.92 + noise(x / 2, y / 2, 1) * 0.16;
            let (dy, dx) = (y % COURSE, row.iter().map(|&c| x.abs_diff(c)).min().unwrap_or(99));
            let f = if dy < 2 || dx < 2 {
                0.5
            } else if dy == 2 || dx == 2 {
                shade * 1.18 // a lit top-left edge
            } else {
                shade * grain
            };
            pixels.push(scale(base, f));
        }
    }
    egui::ColorImage::new([SIZE, SIZE], pixels)
}

/// 3×5 glyphs for falling code: digits and a few katakana-like shapes, a row per 3 bits.
const GLYPHS: [u16; 16] = [
    0b111_101_101_101_111,
    0b010_110_010_010_111,
    0b111_001_111_100_111,
    0b111_001_011_001_111,
    0b101_101_111_001_001,
    0b111_100_111_001_111,
    0b111_100_111_101_111,
    0b111_001_010_010_010,
    0b111_101_111_101_111,
    0b111_101_111_001_111,
    0b111_001_010_010_100,
    0b010_101_101_101_001,
    0b110_001_110_001_110,
    0b101_101_001_001_110,
    0b111_000_000_000_111,
    0b001_101_010_101_100,
];

/// Columns of falling green code fading behind their bright heads.
fn rain(base: Color32) -> egui::ColorImage {
    const SIZE: usize = 256;
    const CELL_W: usize = 4;
    const CELL_H: usize = 6;
    let (cols, rows) = (SIZE / CELL_W, SIZE / CELL_H + 1);
    let mut pixels = vec![base; SIZE * SIZE];
    for col in 0..cols {
        // Two trails per column, wrapping round the tile.
        for trail in 0..2 {
            let head = (noise(col, trail, 11) * rows as f32) as usize;
            let len = 6 + (noise(col, trail, 13) * 22.0) as usize;
            if noise(col, trail, 17) < 0.3 {
                continue;
            }
            for d in 0..len {
                let row = (head + rows - d) % rows;
                let glyph = GLYPHS[(noise(col, row, 19) * 16.0) as usize % 16];
                let fade = 1.0 - d as f32 / len as f32;
                let colour = if d == 0 {
                    Color32::from_rgb(90, 200, 115)
                } else {
                    lerp(base, Color32::from_rgb(40, 255, 100), 0.06 + fade * 0.22)
                };
                for gy in 0..5 {
                    for gx in 0..3 {
                        if glyph >> (14 - (gy * 3 + gx)) & 1 == 1 {
                            let (x, y) = (col * CELL_W + gx, row * CELL_H + gy);
                            if y < SIZE {
                                pixels[y * SIZE + x] = colour;
                            }
                        }
                    }
                }
            }
        }
    }
    egui::ColorImage::new([SIZE, SIZE], pixels)
}

// ---------------------------------------------------------------------------------------
// Painted scenes

/// Draws a painted scene over the background's colours; false if `name` isn't one.
pub fn paint_scene(painter: &Painter, rect: Rect, name: &str, hill: Color32, hill_bottom: Color32) -> bool {
    match name {
        "hills" => paint_hills(painter, rect, hill, hill_bottom),
        "farmyard" => paint_farmyard(painter, rect),
        _ => return false,
    }
    true
}

/// A soft cloud of overlapping puffs.
fn soft_cloud(painter: &Painter, c: Pos2, size: f32) {
    let white = Color32::from_white_alpha(70);
    for (dx, dy, r) in [(-0.9, 0.15, 0.5), (-0.35, -0.2, 0.7), (0.35, -0.1, 0.65), (0.9, 0.2, 0.45), (0.0, 0.25, 0.6)] {
        painter.circle_filled(c + vec2(dx, dy) * size, r * size, white);
    }
}

/// Two rolling green hills under a sky with soft clouds.
fn paint_hills(painter: &Painter, rect: Rect, hill: Color32, hill_bottom: Color32) {
    let (w, h) = (rect.width(), rect.height());
    for (x, y, s) in [(0.18, 0.16, 70.0), (0.55, 0.09, 50.0), (0.8, 0.22, 85.0), (0.38, 0.3, 40.0)] {
        soft_cloud(painter, rect.min + vec2(x * w, y * h), s);
    }
    // The far hill paler (hazier), the near one in full colour.
    let far = (lerp(hill, Color32::from_rgb(200, 230, 255), 0.25), lerp(hill_bottom, hill, 0.4));
    let hills = [(0.6, 0.07, 1.3, 0.8, far), (0.7, 0.09, 0.9, 2.6, (hill, hill_bottom))];
    for (base, amp, freq, phase, (top, bottom)) in hills {
        const STEPS: usize = 96;
        let mut mesh = egui::Mesh::default();
        for i in 0..=STEPS {
            let t = i as f32 / STEPS as f32;
            let y = rect.top() + h * (base - amp * (t * std::f32::consts::PI * freq + phase).sin());
            let x = rect.left() + t * w;
            mesh.colored_vertex(pos2(x, y), top);
            mesh.colored_vertex(pos2(x, rect.bottom()), bottom);
            if i > 0 {
                let k = (i * 2) as u32;
                mesh.add_triangle(k - 2, k - 1, k);
                mesh.add_triangle(k - 1, k + 1, k);
            }
        }
        painter.add(mesh);
    }
}

/// Pixel art: each string is a row, each character a pixel coloured by `palette`
/// ('.' and unknown characters are clear). `flip` mirrors it left to right.
fn sprite(painter: &Painter, top_left: Pos2, px: f32, rows: &[&str], palette: &dyn Fn(char) -> Option<Color32>, flip: bool) {
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    for (y, row) in rows.iter().enumerate() {
        let chars: Vec<char> = row.chars().collect();
        // Runs of one colour become one rectangle.
        let mut x = 0;
        while x < chars.len() {
            let c = chars[x];
            let run = chars[x..].iter().take_while(|&&d| d == c).count();
            if let Some(colour) = palette(c) {
                let left = if flip { width - x - run } else { x };
                let r = Rect::from_min_size(top_left + vec2(left as f32 * px, y as f32 * px), vec2(run as f32 * px, px));
                painter.rect_filled(r, CornerRadius::ZERO, colour);
            }
            x += run;
        }
    }
}

const HEN: [&str; 12] = [
    "........R.R...",
    ".......RRRR...",
    "......WWWWW...",
    "......WWWEWYY.",
    "S.....WWWWWY..",
    "SS....WWWWR...",
    "SWS..WWWWWR...",
    "SWWWWWWWWW....",
    ".SWWSSSWWW....",
    "..SWWWWWW.....",
    "....Y..Y......",
    "...YY.YY......",
];

/// An upright runner duck.
const DUCK: [&str; 16] = [
    "...WWW...",
    "..WWEWYY.",
    "..WWWW...",
    "...WW....",
    "...WW....",
    "...WW....",
    "..WWW....",
    "..WWWW...",
    ".WWSWW...",
    ".WWSSW...",
    ".WWSSW...",
    ".WWWWW...",
    "..WWWW...",
    "...WW....",
    "...Y.Y...",
    "..YY.YY..",
];

/// Body, shade and eye colours of hens and ducks: white, buff and black.
const FEATHERS: [(Color32, Color32, Color32); 3] = [
    (Color32::from_rgb(0xfb, 0xf7, 0xec), Color32::from_rgb(0xe4, 0xd9, 0xc4), Color32::from_rgb(0x2b, 0x2b, 0x2b)),
    (Color32::from_rgb(0xd8, 0xa9, 0x6c), Color32::from_rgb(0xb5, 0x85, 0x4e), Color32::from_rgb(0x2b, 0x2b, 0x2b)),
    (Color32::from_rgb(0x46, 0x41, 0x3c), Color32::from_rgb(0x2e, 0x2b, 0x28), Color32::from_rgb(0xef, 0xe6, 0xc0)),
];

fn bird_palette(feathers: usize) -> impl Fn(char) -> Option<Color32> {
    let (body, shade, eye) = FEATHERS[feathers % 3];
    move |c| match c {
        'W' => Some(body),
        'S' => Some(shade),
        'E' => Some(eye),
        'R' => Some(Color32::from_rgb(0xd2, 0x40, 0x2e)),
        'Y' => Some(Color32::from_rgb(0xe8, 0xa3, 0x3a)),
        _ => None,
    }
}

/// A hen standing with her feet on `ground` at `x` (her left edge).
pub fn hen(painter: &Painter, x: f32, ground: f32, px: f32, feathers: usize, facing_left: bool) {
    sprite(painter, pos2(x, ground - HEN.len() as f32 * px), px, &HEN, &bird_palette(feathers), facing_left);
}

fn duck(painter: &Painter, x: f32, ground: f32, px: f32, feathers: usize, facing_left: bool) {
    sprite(painter, pos2(x, ground - DUCK.len() as f32 * px), px, &DUCK, &bird_palette(feathers), facing_left);
}

/// The hen pointer: a small hen whose beak is at `tip`.
pub fn hen_pointer(painter: &Painter, tip: Pos2) {
    let px = 4.5;
    hen(painter, tip.x - 13.0 * px, tip.y + 8.0 * px, px, 0, false);
}

const LEAF: [&str; 6] = ["..LL...", ".LMML..", "LMMDML.", ".LMDMML", "..LMML.", "...LL.."];
const TOMATO: [&str; 5] = ["..G..", ".RRR.", "RRHRR", "RRRRR", ".RRR."];
const PUMPKIN: [&str; 5] = ["...G...", ".OoOoO.", "OOoOoOO", "OOoOoOO", ".OoOoO."];
const FLOWER: [&str; 3] = [".P.", "PCP", ".P."];
const COOP: [&str; 10] = [
    "......RR......",
    "....RRRRRR....",
    "..RRRRRRRRRR..",
    "RRRRRRRRRRRRRR",
    ".WWWWWWWWWWWW.",
    ".WWWWWDDDWWWW.",
    ".WWWWWDDDWWWW.",
    ".WWWWWDDDWWWW.",
    ".WWWWWDDDWWWW.",
    ".WWWWWDDDWWWW.",
];

fn garden_palette(c: char) -> Option<Color32> {
    Some(match c {
        'L' => Color32::from_rgb(0x7f, 0xb0, 0x69),
        'M' => Color32::from_rgb(0x5e, 0x8c, 0x5a),
        'D' => Color32::from_rgb(0x3e, 0x6b, 0x45),
        'G' => Color32::from_rgb(0x3e, 0x6b, 0x45),
        'R' => Color32::from_rgb(0xd6, 0x45, 0x32),
        'H' => Color32::from_rgb(0xf0, 0x80, 0x70),
        'O' => Color32::from_rgb(0xe8, 0x75, 0x2a),
        'o' => Color32::from_rgb(0xc7, 0x5b, 0x1e),
        'P' => Color32::from_rgb(0xe8, 0xa0, 0xb4),
        'C' => Color32::from_rgb(0xfb, 0xe3, 0x8a),
        _ => return None,
    })
}

fn coop_palette(c: char) -> Option<Color32> {
    Some(match c {
        'R' => Color32::from_rgb(0x6e, 0x4e, 0x33),
        'W' => Color32::from_rgb(0xa3, 0x82, 0x5a),
        'D' => Color32::from_rgb(0x3a, 0x2a, 0x1e),
        _ => return None,
    })
}

/// A blocky cloud `blocks` wide, `b` points per block.
fn pixel_cloud(painter: &Painter, at: Pos2, b: f32) {
    let body = Color32::from_rgba_unmultiplied(0xff, 0xfb, 0xed, 210);
    let shade = Color32::from_rgba_unmultiplied(0xbf, 0xd9, 0xe7, 210);
    for (row, (from, to, colour)) in [(5, 10, body), (2, 14, body), (0, 19, body), (1, 18, shade)].into_iter().enumerate() {
        let r = Rect::from_min_max(at + vec2(from as f32 * b, row as f32 * b), at + vec2(to as f32 * b, (row + 1) as f32 * b));
        painter.rect_filled(r, CornerRadius::ZERO, colour);
    }
}

/// A climbing plant up one side of the screen, leaves and fruit along it.
fn side_plant(painter: &Painter, rect: Rect, right: bool, px: f32) {
    let stem = Color32::from_rgb(0x4b, 0x70, 0x44);
    let edge = |x: f32| if right { rect.right() - x } else { rect.left() + x };
    let mut y = rect.top() - 10.0;
    let mut i = 0;
    while y < rect.bottom() {
        // A wavy stem, a pixel column at a time.
        let x = 26.0 + (y / 90.0 + if right { 1.7 } else { 0.0 }).sin() * 10.0;
        painter.rect_filled(Rect::from_min_size(pos2(edge(x) - px / 2.0, y), vec2(px, 8.0)), CornerRadius::ZERO, stem);
        if i % 6 == 0 {
            // Leaves alternate sides; fruit hangs between some of them.
            let out = (i / 6) % 2 == 0;
            let lx = if out == right { edge(x) } else { edge(x) - 7.0 * px };
            sprite(painter, pos2(lx, y - 3.0 * px), px, &LEAF, &garden_palette, out == right);
            let pick = noise(i, right as usize, 23);
            let fruit: Option<&[&str]> = if pick < 0.22 {
                Some(if right { &PUMPKIN } else { &TOMATO })
            } else if pick < 0.36 {
                Some(&FLOWER)
            } else {
                None
            };
            if let Some(fruit) = fruit {
                let fx = edge(x + if right { -px * 2.0 } else { px * 2.0 }) - fruit[0].len() as f32 * px / 2.0;
                sprite(painter, pos2(fx, y + 4.0 * px), px, fruit, &garden_palette, false);
            }
        }
        y += 8.0;
        i += 1;
    }
}

/// The farmyard's ground: in the overscan margin, so it covers little.
const GROUND_H: f32 = 44.0;

/// How much of the bottom of the screen a scene's front part needs kept clear.
pub fn front_height(name: &str) -> f32 {
    match name {
        "farmyard" => GROUND_H + 16.0 * 3.5,
        _ => 0.0,
    }
}

/// Draws the part of a painted scene that goes in front of everything (except while a
/// video plays); false if `name` has none.
pub fn paint_scene_front(painter: &Painter, rect: Rect, name: &str) -> bool {
    match name {
        "farmyard" => farmyard_front(painter, rect),
        _ => return false,
    }
    true
}

/// A strip of turf and soil along the bottom, with a coop, a pond, and a flock of hens and
/// runner ducks.
fn farmyard_front(painter: &Painter, rect: Rect) {
    let (w, px) = (rect.width(), 4.0);
    let ground_h = GROUND_H;
    let ground = rect.bottom() - ground_h;
    let soil = Rect::from_min_max(pos2(rect.left(), ground), rect.max);
    painter.rect_filled(soil, CornerRadius::ZERO, Color32::from_rgb(0x7a, 0x5c, 0x43));
    let speck = Color32::from_rgb(0x5c, 0x42, 0x32);
    for i in 0..(w / 18.0) as usize {
        let x = rect.left() + i as f32 * 18.0 + noise(i, 0, 29) * 10.0;
        let y = ground + 20.0 + noise(i, 1, 29) * (ground_h - 24.0);
        painter.rect_filled(Rect::from_min_size(pos2(x, y), vec2(px * (1.0 + noise(i, 2, 29).round()), px)), CornerRadius::ZERO, speck);
    }
    for (offset, height, colour) in [(0.0, px, (0x88, 0xad, 0x5b)), (px, px * 2.0, (0x63, 0x8d, 0x48)), (px * 3.0, px, (0x3f, 0x65, 0x3a))] {
        let (r, g, b) = colour;
        painter.rect_filled(Rect::from_min_size(pos2(rect.left(), ground + offset), vec2(w, height)), CornerRadius::ZERO, Color32::from_rgb(r, g, b));
    }
    // Grass tufts poking up.
    for i in 0..(w / 26.0) as usize {
        let x = rect.left() + i as f32 * 26.0 + noise(i, 3, 31) * 14.0;
        let colour = if i % 2 == 0 { Color32::from_rgb(0x63, 0x8d, 0x48) } else { Color32::from_rgb(0x88, 0xad, 0x5b) };
        let tall = px * (1.0 + (noise(i, 4, 31) * 2.0).floor());
        painter.rect_filled(Rect::from_min_size(pos2(x, ground - tall), vec2(px * 0.75, tall)), CornerRadius::ZERO, colour);
    }

    // The pond on the right: a stepped basin with water in it.
    let pond_w = (w * 0.2).clamp(130.0, 320.0);
    let pond_l = rect.right() - w * 0.04 - pond_w;
    let bed = Color32::from_rgb(0x5c, 0x42, 0x32);
    let water = Color32::from_rgb(0x7f, 0xb4, 0xc7);
    const BASIN: [(f32, f32); 4] = [(0.0, 0.62), (0.04, 0.78), (0.11, 0.91), (0.22, 1.0)];
    for (inset, depth) in BASIN {
        let basin = Rect::from_min_max(pos2(pond_l + pond_w * inset, ground), pos2(pond_l + pond_w * (1.0 - inset), ground + 40.0 * depth));
        painter.rect_filled(basin.expand(px), CornerRadius::ZERO, bed);
    }
    for (inset, depth) in BASIN {
        let basin = Rect::from_min_max(pos2(pond_l + pond_w * inset, ground + px * 2.0), pos2(pond_l + pond_w * (1.0 - inset), ground + 40.0 * depth));
        painter.rect_filled(basin, CornerRadius::ZERO, water);
    }
    let shine = Color32::from_rgb(0xa5, 0xcd, 0xd9);
    painter.rect_filled(Rect::from_min_size(pos2(pond_l, ground + px * 2.0), vec2(pond_w, px * 0.75)), CornerRadius::ZERO, shine);
    painter.rect_filled(Rect::from_min_size(pos2(pond_l + pond_w * 0.15, ground + px * 4.0), vec2(pond_w * 0.25, px * 0.5)), CornerRadius::ZERO, shine);

    // The coop, and the flock: hens, then runner ducks, then more hens heading home.
    let coop_x = rect.left() + w * 0.03 + 70.0;
    sprite(painter, pos2(coop_x, ground - COOP.len() as f32 * px * 1.2 + px), px * 1.2, &COOP, &coop_palette, false);
    let feet = ground + px;
    let px = 3.5;
    for (i, x) in [0.17, 0.25, 0.33].into_iter().enumerate() {
        hen(painter, rect.left() + x * w, feet, px, i, false);
    }
    for (i, x) in [0.42, 0.47, 0.52].into_iter().enumerate() {
        duck(painter, rect.left() + x * w, feet, px, i, false);
    }
    for (i, x) in [0.6, 0.67].into_iter().enumerate() {
        hen(painter, rect.left() + x * w, feet, px, i + 1, true);
    }
    // A duck having a swim.
    let swim = pos2(pond_l + pond_w * 0.45, ground + px * 2.0);
    sprite(painter, swim - vec2(0.0, 9.0 * px), px, &DUCK[..9], &bird_palette(0), true);

}

/// Blue sky with blocky clouds and plants climbing both sides (the ground and birds are
/// in front: see [`farmyard_front`]).
fn paint_farmyard(painter: &Painter, rect: Rect) {
    let (w, h) = (rect.width(), rect.height());
    let px = 4.0;
    // Sky: zenith, then middle at 56%, then the horizon.
    let (top, middle, horizon) = (Color32::from_rgb(0x77, 0xb8, 0xe3), Color32::from_rgb(0xb6, 0xdd, 0xee), Color32::from_rgb(0xe3, 0xef, 0xdf));
    let split = rect.top() + h * 0.56;
    for (r, a, b) in [(Rect::from_min_max(rect.min, pos2(rect.right(), split)), top, middle), (Rect::from_min_max(pos2(rect.left(), split), rect.max), middle, horizon)] {
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(r.left_top(), a);
        mesh.colored_vertex(r.right_top(), a);
        mesh.colored_vertex(r.right_bottom(), b);
        mesh.colored_vertex(r.left_bottom(), b);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(mesh);
    }
    for (x, y, b) in [(0.10, 0.07, 11.0), (0.70, 0.22, 7.5), (0.36, 0.40, 13.0), (0.84, 0.60, 8.5), (0.18, 0.76, 6.5)] {
        pixel_cloud(painter, rect.min + vec2(x * w, y * h), b);
    }

    let ground = rect.bottom() - GROUND_H;
    side_plant(painter, Rect::from_min_max(rect.min, pos2(rect.right(), ground)), false, px);
    side_plant(painter, Rect::from_min_max(rect.min, pos2(rect.right(), ground)), true, px);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_are_made() {
        for name in ["stone", "pinstripes", "rain"] {
            assert!(make_pattern(name, Color32::from_rgb(60, 50, 40)).is_some(), "{name}");
        }
        assert!(make_pattern("farmyard", Color32::BLACK).is_none(), "scenes are painted, not tiled");
    }

    #[test]
    fn sprites_are_rectangular() {
        for rows in [&HEN[..], &DUCK, &COOP, &LEAF, &TOMATO, &PUMPKIN, &FLOWER] {
            assert!(rows.iter().all(|r| r.len() == rows[0].len()), "{rows:?}");
        }
    }
}
