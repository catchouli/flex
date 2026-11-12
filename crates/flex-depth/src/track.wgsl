// Following points from one picture to the next by Lucas-Kanade on an image pyramid, there and
// back again. Each picture is its levels one after the other, and each level its brightness, x
// gradient and y gradient, which `track.rs` works out.

struct Level { w: u32, h: u32, at: u32, unused: u32 }
struct Params { levels: array<Level, 4>, count: u32, from_frame: u32, to_frame: u32, level_count: u32 }

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> pic: array<f32>;
// x, y, and a guess at how far it's moved.
@group(0) @binding(2) var<storage, read> points: array<vec4<f32>>;
// x and y now, how different the patch looks, and how far from the start coming back ended up.
@group(0) @binding(3) var<storage, read_write> found: array<vec4<f32>>;

const RADIUS: i32 = 7;
const PIXELS: u32 = 225u;
const MOST_STEPS: u32 = 9u;
const CLOSE_ENOUGH: f32 = 0.02;

fn sample(base: u32, w: u32, h: u32, x: f32, y: f32) -> f32 {
    let x0 = clamp(i32(floor(x)), 0, i32(w) - 2);
    let y0 = clamp(i32(floor(y)), 0, i32(h) - 2);
    let fx = clamp(x - f32(x0), 0.0, 1.0);
    let fy = clamp(y - f32(y0), 0.0, 1.0);
    let i = base + u32(y0) * w + u32(x0);
    let top = pic[i] * (1.0 - fx) + pic[i + 1u] * fx;
    let bottom = pic[i + w] * (1.0 - fx) + pic[i + w + 1u] * fx;
    return top * (1.0 - fy) + bottom * fy;
}

// Where `start` went from one frame to the other: x, y, and how different the patch looks.
fn follow(from_frame: u32, to_frame: u32, start: vec2<f32>, guess: vec2<f32>) -> vec3<f32> {
    var v = guess / f32(1u << (p.level_count - 1u));
    var before: array<f32, 225>;
    var gx: array<f32, 225>;
    var gy: array<f32, 225>;
    var seen: array<f32, 225>;
    var left = 0.0;
    var level = i32(p.level_count) - 1;
    loop {
        if level < 0 { break; }
        let l = p.levels[level];
        let w = l.w;
        let h = l.h;
        let plane = w * h;
        let centre = start / f32(1u << u32(level));
        let from_at = from_frame + l.at;
        let to_at = to_frame + l.at;
        var mean = 0.0;
        var a = 0.0;
        var b = 0.0;
        var c = 0.0;
        var n = 0u;
        for (var dy = -RADIUS; dy <= RADIUS; dy += 1) {
            for (var dx = -RADIUS; dx <= RADIUS; dx += 1) {
                let x = centre.x + f32(dx);
                let y = centre.y + f32(dy);
                let t = sample(from_at, w, h, x, y);
                let ix = sample(from_at + plane, w, h, x, y);
                let iy = sample(from_at + 2u * plane, w, h, x, y);
                before[n] = t;
                gx[n] = ix;
                gy[n] = iy;
                mean += t;
                a += ix * ix;
                b += ix * iy;
                c += iy * iy;
                n += 1u;
            }
        }
        mean /= f32(PIXELS);
        let d = a * c - b * b + 1e-9;
        for (var step = 0u; step < MOST_STEPS; step += 1u) {
            var seen_mean = 0.0;
            n = 0u;
            for (var dy = -RADIUS; dy <= RADIUS; dy += 1) {
                for (var dx = -RADIUS; dx <= RADIUS; dx += 1) {
                    let j = sample(to_at, w, h, centre.x + f32(dx) + v.x, centre.y + f32(dy) + v.y);
                    seen[n] = j;
                    seen_mean += j;
                    n += 1u;
                }
            }
            seen_mean /= f32(PIXELS);
            var bx = 0.0;
            var by = 0.0;
            left = 0.0;
            for (var i = 0u; i < PIXELS; i += 1u) {
                let e = (before[i] - mean) - (seen[i] - seen_mean);
                bx += e * gx[i];
                by += e * gy[i];
                left += abs(e);
            }
            left /= f32(PIXELS);
            let du = (c * bx - b * by) / d;
            let dv = (a * by - b * bx) / d;
            v += vec2<f32>(du, dv);
            if max(abs(du), abs(dv)) < CLOSE_ENOUGH { break; }
        }
        if level > 0 { v *= 2.0; }
        level -= 1;
    }
    return vec3<f32>(start + v, left);
}

@compute @workgroup_size(16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.count { return; }
    let point = points[id.x];
    let there = follow(p.from_frame, p.to_frame, point.xy, point.zw);
    let back = follow(p.to_frame, p.from_frame, there.xy, point.xy - there.xy);
    found[id.x] = vec4<f32>(there, distance(back.xy, point.xy));
}
