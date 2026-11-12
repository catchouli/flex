// The face networks' layers as compute shaders (see face.rs). Pictures are channels first, [c][h][w], in
// `mem`; weights are in `w`, all f32. One thread per output value, the outputs counted through
// in order (channel, row, pixel) 64 to a workgroup, so that small pictures with many channels
// keep the GPU as busy as big ones.

struct Params {
    h: u32, w: u32, cin: u32, cout: u32,
    a: u32, b: u32, c: u32, weights: u32,
    bias: u32, kernel: u32, stride: u32, groups: u32,
    act: u32, oh: u32, ow: u32, pad: u32,
    alpha: u32, skip_c: u32, parts: u32, part_at: u32,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read_write> mem: array<f32>;
@group(0) @binding(2) var<storage, read> w: array<f32>;

// Workgroups in a row of the dispatch (see face.rs).
const ROW: u32 = 65535u;

// The output (x, y, channel) this thread makes, out of `channels` of oh x ow, and whether there is one.
fn place(gid: vec3<u32>, channels: u32) -> vec4<u32> {
    let i = gid.y * ROW * 64u + gid.x;
    let plane = p.oh * p.ow;
    let z = i / plane;
    let r = i % plane;
    return vec4<u32>(r % p.ow, r / p.ow, z, u32(z < channels));
}

// Adds the residual (from `b`, for the first `skip_c` channels) and applies act: 1 is ReLU, 2
// PReLU with weights at `alpha`, 3 ReLU6. Then stores output channel `co`'s value at `at`.
fn finish(v: f32, co: u32, at: u32) {
    var acc = v;
    if (co < p.skip_c) { acc += mem[p.b + at]; }
    if (p.act == 1u) {
        acc = max(acc, 0.0);
    } else if (p.act == 2u) {
        if (acc < 0.0) { acc *= w[p.alpha + co]; }
    } else if (p.act == 3u) {
        acc = clamp(acc, 0.0, 6.0);
    }
    mem[p.c + at] = acc;
}

// Taps tap0 to taps1 of one input channel's part of a convolution's output: the plane at `src`
// around (cy, cx), with that channel's weights at `weights`.
fn taps_of(src: u32, weights: u32, cy: i32, cx: i32, tap0: u32, taps1: u32) -> f32 {
    var acc = 0.0;
    let radius = i32(p.pad);
    for (var t = tap0; t < taps1; t++) {
        let yy = cy + i32(t / p.kernel) - radius;
        let xx = cx + i32(t % p.kernel) - radius;
        let inside = yy >= 0 && yy < i32(p.h) && xx >= 0 && xx < i32(p.w);
        let tap = u32(clamp(yy, 0, i32(p.h) - 1)) * p.w + u32(clamp(xx, 0, i32(p.w) - 1));
        acc += select(0.0, mem[src + tap], inside) * w[weights + t];
    }
    return acc;
}

// A convolution starting `pad` before each output, either full (groups 1) or depthwise (groups =
// channels). Weights are [cout][cin / groups][kernel][kernel]. Then `finish`.
@compute @workgroup_size(64)
fn conv(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = place(gid, p.cout);
    if (id.w == 0u) { return; }
    let taps = p.kernel * p.kernel;
    let plane = p.h * p.w;
    let inputs = p.cin / p.groups;
    let first = select(0u, id.z, p.groups > 1u);
    let cy = i32(id.y * p.stride);
    let cx = i32(id.x * p.stride);
    var acc = w[p.bias + id.z];
    for (var ci = 0u; ci < inputs; ci++) {
        acc += taps_of(p.a + (first + ci) * plane, p.weights + (id.z * inputs + ci) * taps, cy, cx, 0u, taps);
    }
    finish(acc, id.z, id.z * p.oh * p.ow + id.y * p.ow + id.x);
}

var<workgroup> partial: array<f32, 64>;

// The same (groups 1) for few outputs with long sums: a workgroup for each output, its 64
// threads taking turns through the inputs and adding up at the end.
@compute @workgroup_size(64)
fn conv_reduce(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let o = wg.y * ROW + wg.x;
    let plane_out = p.oh * p.ow;
    let co = o / plane_out;
    let r = o % plane_out;
    let x = r % p.ow;
    let y = r / p.ow;
    let taps = p.kernel * p.kernel;
    let plane = p.h * p.w;
    let cy = i32(y * p.stride);
    let cx = i32(x * p.stride);
    var acc = 0.0;
    if (co < p.cout) {
        for (var e = li; e < p.cin * taps; e += 64u) {
            let ci = e / taps;
            let t = e % taps;
            acc += taps_of(p.a + ci * plane, p.weights + (co * p.cin + ci) * taps, cy, cx, t, t + 1u);
        }
    }
    partial[li] = acc;
    workgroupBarrier();
    for (var s = 32u; s > 0u; s >>= 1u) {
        if (li < s) { partial[li] += partial[li + s]; }
        workgroupBarrier();
    }
    if (li == 0u && co < p.cout) {
        finish(partial[0] + w[p.bias + co], co, co * plane_out + r);
    }
}

// 2 x 2 max pooling.
@compute @workgroup_size(64)
fn pool(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = place(gid, p.cout);
    if (id.w == 0u) { return; }
    let src = p.a + id.z * p.h * p.w + id.y * 2u * p.w + id.x * 2u;
    let v = max(max(mem[src], mem[src + 1u]), max(mem[src + p.w], mem[src + p.w + 1u]));
    mem[p.c + id.z * p.oh * p.ow + id.y * p.ow + id.x] = v;
}

// A plus B doubled in size by repeating its pixels: B is [cin][h][w], A and the result
// twice that size.
@compute @workgroup_size(64)
fn add_up(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = place(gid, p.cout);
    if (id.w == 0u) { return; }
    let at = id.z * p.oh * p.ow + id.y * p.ow + id.x;
    let up = p.b + id.z * p.h * p.w + (id.y / 2u) * p.w + id.x / 2u;
    mem[p.c + at] = mem[p.a + at] + mem[up];
}

// The average of each channel.
@compute @workgroup_size(64)
fn gap(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = place(gid, p.cout);
    if (id.w == 0u) { return; }
    let plane = p.h * p.w;
    let src = p.a + id.z * plane;
    var sum = 0.0;
    for (var i = 0u; i < plane; i++) { sum += mem[src + i]; }
    mem[p.c + id.z] = sum / f32(plane);
}

// ---- 1x1 convolutions as a tiled matrix multiply ----------------------------------------------
// C [pixel][cout] = X^T [pixel][cin] W^T [cin][cout], with the input X [cin][pixels] in `mem` and
// the weights transposed to [cin][cout] when loaded (face.rs). A tile of 32 pixels by 32 output
// channels per workgroup of 8 x 8 threads, 4 x 4 a thread, cin in steps of 16 with the next
// step read ahead (as depth.wgsl's matmul). With `parts` > 1 the sum over cin is split into
// that many parts side by side (workgroup z), each left at `part_at` ([part][cout][pixels]) for
// `sum_parts` to add up: for layers with few pixels, which would have too few tiles.

var<workgroup> ta: array<vec4<f32>, 128>; // [ci 16][pixels / 4: 8]
var<workgroup> tb: array<vec4<f32>, 128>; // [ci 16][cout / 4: 8]

fn load_x(pixels: u32, t_end: u32, px: u32, t: u32) -> vec4<f32> {
    let base = p.a + min(t, p.cin - 1u) * pixels;
    let last = pixels - 1u;
    let v = vec4<f32>(mem[base + min(px, last)], mem[base + min(px + 1u, last)], mem[base + min(px + 2u, last)], mem[base + min(px + 3u, last)]);
    let inside = vec4<bool>(px < pixels, px + 1u < pixels, px + 2u < pixels, px + 3u < pixels) & vec4<bool>(t < t_end);
    return select(vec4<f32>(0.0), v, inside);
}

fn load_wt(t_end: u32, t: u32, co: u32) -> vec4<f32> {
    let base = p.weights + min(t, p.cin - 1u) * p.cout;
    let last = p.cout - 1u;
    let v = vec4<f32>(w[base + min(co, last)], w[base + min(co + 1u, last)], w[base + min(co + 2u, last)], w[base + min(co + 3u, last)]);
    let inside = vec4<bool>(co < p.cout, co + 1u < p.cout, co + 2u < p.cout, co + 3u < p.cout) & vec4<bool>(t < t_end);
    return select(vec4<f32>(0.0), v, inside);
}

@compute @workgroup_size(8, 8)
fn conv1_tile(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let pixels = p.h * p.w;
    let parts = max(p.parts, 1u);
    let each = ((p.cin + parts - 1u) / parts + 15u) / 16u * 16u;
    let t_start = wg.z * each;
    let t_end = min(p.cin, t_start + each);
    let row0 = wg.y * 32u;
    let col0 = wg.x * 32u;
    let tid = lid.y * 8u + lid.x;
    let q = tid % 8u;
    let tk = tid / 8u;
    var c0 = vec4<f32>(0.0); var c1 = vec4<f32>(0.0); var c2 = vec4<f32>(0.0); var c3 = vec4<f32>(0.0);
    var na0 = load_x(pixels, t_end, row0 + q * 4u, t_start + tk); var na1 = load_x(pixels, t_end, row0 + q * 4u, t_start + tk + 8u);
    var nb0 = load_wt(t_end, t_start + tk, col0 + q * 4u); var nb1 = load_wt(t_end, t_start + tk + 8u, col0 + q * 4u);
    for (var t0 = t_start; t0 < t_end; t0 += 16u) {
        ta[tk * 8u + q] = na0; ta[(tk + 8u) * 8u + q] = na1;
        tb[tk * 8u + q] = nb0; tb[(tk + 8u) * 8u + q] = nb1;
        workgroupBarrier();
        na0 = load_x(pixels, t_end, row0 + q * 4u, t0 + 16u + tk); na1 = load_x(pixels, t_end, row0 + q * 4u, t0 + 24u + tk);
        nb0 = load_wt(t_end, t0 + 16u + tk, col0 + q * 4u); nb1 = load_wt(t_end, t0 + 24u + tk, col0 + q * 4u);
        for (var k = 0u; k < 16u; k++) {
            let x = ta[k * 8u + lid.y];
            let y = tb[k * 8u + lid.x];
            c0 += x.x * y; c1 += x.y * y; c2 += x.z * y; c3 += x.w * y;
        }
        workgroupBarrier();
    }
    let rows = array<vec4<f32>, 4>(c0, c1, c2, c3);
    for (var i = 0u; i < 4u; i++) {
        let px = row0 + lid.y * 4u + i;
        if (px >= pixels) { continue; }
        for (var j = 0u; j < 4u; j++) {
            let co = col0 + lid.x * 4u + j;
            if (co >= p.cout) { continue; }
            if (parts > 1u) {
                mem[p.part_at + (wg.z * p.cout + co) * pixels + px] = rows[i][j];
            } else {
                finish(rows[i][j] + w[p.bias + co], co, co * pixels + px);
            }
        }
    }
}

// The parts of a split `conv1_tile` added up and finished: one thread per output.
@compute @workgroup_size(64)
fn sum_parts(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.y * ROW * 64u + gid.x;
    let pixels = p.h * p.w;
    if (i >= p.cout * pixels) { return; }
    let co = i / pixels;
    var v = w[p.bias + co];
    for (var part = 0u; part < p.parts; part++) {
        v += mem[p.part_at + part * p.cout * pixels + i];
    }
    finish(v, co, i);
}
