// Depth Anything V2 Small's forward pass as compute shaders (see lib.rs).
//
// Based on Maisy's kernels for neurofield (github.com/bombpersons/neurofield,
// engine/src/neural/gpu_net.wgsl), used with her permission: the tiled matrix multiply,
// layer norm, softmax and 3x3 convolution are hers, adapted to keep the weights in their
// own buffer and with a few more options; the strided convolution, transposed convolution,
// resize and copy are new.
//
// Weights live in `w` and everything else (inputs and activations) in `mem`, all f32; each
// dispatch gets its sizes and offsets (in floats) from its own slot of a uniform buffer.
// The field names below are for the matrix multiply; other kernels say how they use them.

struct Params {
    m: u32, k: u32, n: u32, batch: u32,
    a_off: u32, a_bs: u32, a_rs: u32, a_cs: u32,
    b_off: u32, b_bs: u32, b_rs: u32, b_cs: u32,
    c_off: u32, c_bs: u32, c_rs: u32, c_cs: u32,
    bias_off: u32, flags: u32, x0: u32, x1: u32,
    f0: f32, f1: f32, f2: f32, f3: f32,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read_write> mem: array<f32>;
@group(0) @binding(2) var<storage, read> w: array<f32>;

const NONE: u32 = 0xffffffffu;
const GELU: u32 = 1u;
const ACCUMULATE: u32 = 2u;
const RELU: u32 = 4u;
/// Apply ReLU to the convolution's input as it's read (pre-activation residual units).
const RELU_IN: u32 = 8u;
/// The matrix multiply's B is in `mem` (else in `w`).
const B_MEM: u32 = 16u;

// erf by Abramowitz and Stegun 7.1.26 (absolute error under 1.5e-7): WGSL has no erf.
fn erf(x: f32) -> f32 {
    let t = 1.0 / (1.0 + 0.3275911 * abs(x));
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * exp(-x * x);
    return select(-y, y, x >= 0.0);
}

// The exact (erf) GELU, as nn.GELU().
fn gelu(x: f32) -> f32 {
    return 0.5 * x * (1.0 + erf(x * 0.70710678118654752));
}

fn bias(at: u32) -> f32 {
    if (p.bias_off == NONE) { return 0.0; }
    return w[p.bias_off + at];
}

// ---- Matrix multiply ----------------------------------------------------------------------
// C[z][i][j] (=, or += with ACCUMULATE) act(sum_t A[z][i][t] * B[z][t][j] + bias[j]) for
// i < m, j < n, t < k, z < batch. A[z][i][t] = mem[a_off + z*a_bs + i*a_rs + t*a_cs]; B is
// in `w` (or `mem` with B_MEM) likewise; C[z][i][j] = mem[c_off + z*c_bs + i*c_rs + j*c_cs].
// bias_off = NONE for no bias. A 32x32 tile of C per workgroup of 8x8 threads, 4x4 per
// thread, k in steps of 16 with the next step prefetched. Loads are branch-free: addresses
// are clamped into the matrix, and what lies outside is zeroed.

var<workgroup> ta: array<vec4<f32>, 128>; // [t 16][rows / 4: 8]
var<workgroup> tb: array<vec4<f32>, 128>; // [t 16][cols / 4: 8]

fn load_a(a_off: u32, kl: u32, row: u32, t: u32) -> vec4<f32> {
    let base = a_off + min(t, kl - 1u) * p.a_cs;
    let last = p.m - 1u;
    let v = vec4<f32>(mem[base + min(row, last) * p.a_rs], mem[base + min(row + 1u, last) * p.a_rs],
                      mem[base + min(row + 2u, last) * p.a_rs], mem[base + min(row + 3u, last) * p.a_rs]);
    let inside = vec4<bool>(row < p.m, row + 1u < p.m, row + 2u < p.m, row + 3u < p.m) & vec4<bool>(t < kl);
    return select(vec4<f32>(0.0), v, inside);
}

fn b_at(i: u32) -> f32 {
    if ((p.flags & B_MEM) != 0u) { return mem[i]; }
    return w[i];
}

fn load_b(b_off: u32, kl: u32, t: u32, col: u32) -> vec4<f32> {
    let base = b_off + min(t, kl - 1u) * p.b_rs;
    let last = p.n - 1u;
    let v = vec4<f32>(b_at(base + min(col, last) * p.b_cs), b_at(base + min(col + 1u, last) * p.b_cs),
                      b_at(base + min(col + 2u, last) * p.b_cs), b_at(base + min(col + 3u, last) * p.b_cs));
    let inside = vec4<bool>(col < p.n, col + 1u < p.n, col + 2u < p.n, col + 3u < p.n) & vec4<bool>(t < kl);
    return select(vec4<f32>(0.0), v, inside);
}

fn finish(v: f32, o: u32, col: u32) -> f32 {
    var x = v + bias(col);
    if ((p.flags & GELU) != 0u) { x = gelu(x); }
    if ((p.flags & RELU) != 0u) { x = max(x, 0.0); }
    if ((p.flags & ACCUMULATE) != 0u) { x += mem[o]; }
    return x;
}

@compute @workgroup_size(8, 8)
fn matmul(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let z = wg.z;
    let kl = p.k;
    let a_off = p.a_off + z * p.a_bs;
    let b_off = p.b_off + z * p.b_bs;
    let row0 = wg.y * 32u;
    let col0 = wg.x * 32u;
    let tid = lid.y * 8u + lid.x;
    // 64 threads load 16 t x 8 row-quads of A and 16 t x 8 column-quads of B, two of each.
    let q = tid % 8u;
    let tk = tid / 8u;
    var c0 = vec4<f32>(0.0); var c1 = vec4<f32>(0.0); var c2 = vec4<f32>(0.0); var c3 = vec4<f32>(0.0);
    var na0 = load_a(a_off, kl, row0 + q * 4u, tk); var na1 = load_a(a_off, kl, row0 + q * 4u, tk + 8u);
    var nb0 = load_b(b_off, kl, tk, col0 + q * 4u); var nb1 = load_b(b_off, kl, tk + 8u, col0 + q * 4u);
    for (var t0 = 0u; t0 < kl; t0 += 16u) {
        ta[tk * 8u + q] = na0; ta[(tk + 8u) * 8u + q] = na1;
        tb[tk * 8u + q] = nb0; tb[(tk + 8u) * 8u + q] = nb1;
        workgroupBarrier();
        na0 = load_a(a_off, kl, row0 + q * 4u, t0 + 16u + tk); na1 = load_a(a_off, kl, row0 + q * 4u, t0 + 24u + tk);
        nb0 = load_b(b_off, kl, t0 + 16u + tk, col0 + q * 4u); nb1 = load_b(b_off, kl, t0 + 24u + tk, col0 + q * 4u);
        // Unrolled by hand (WGSL has no unroll hint); a rolled loop waits on each read in turn.
        let x0 = ta[0u + lid.y]; let y0 = tb[0u + lid.x]; let x1 = ta[8u + lid.y]; let y1 = tb[8u + lid.x];
        let x2 = ta[16u + lid.y]; let y2 = tb[16u + lid.x]; let x3 = ta[24u + lid.y]; let y3 = tb[24u + lid.x];
        let x4 = ta[32u + lid.y]; let y4 = tb[32u + lid.x]; let x5 = ta[40u + lid.y]; let y5 = tb[40u + lid.x];
        let x6 = ta[48u + lid.y]; let y6 = tb[48u + lid.x]; let x7 = ta[56u + lid.y]; let y7 = tb[56u + lid.x];
        let x8 = ta[64u + lid.y]; let y8 = tb[64u + lid.x]; let x9 = ta[72u + lid.y]; let y9 = tb[72u + lid.x];
        let xa = ta[80u + lid.y]; let ya = tb[80u + lid.x]; let xb = ta[88u + lid.y]; let yb = tb[88u + lid.x];
        let xc = ta[96u + lid.y]; let yc = tb[96u + lid.x]; let xd = ta[104u + lid.y]; let yd = tb[104u + lid.x];
        let xe = ta[112u + lid.y]; let ye = tb[112u + lid.x]; let xf = ta[120u + lid.y]; let yf = tb[120u + lid.x];
        c0 += x0.x * y0; c1 += x0.y * y0; c2 += x0.z * y0; c3 += x0.w * y0;
        c0 += x1.x * y1; c1 += x1.y * y1; c2 += x1.z * y1; c3 += x1.w * y1;
        c0 += x2.x * y2; c1 += x2.y * y2; c2 += x2.z * y2; c3 += x2.w * y2;
        c0 += x3.x * y3; c1 += x3.y * y3; c2 += x3.z * y3; c3 += x3.w * y3;
        c0 += x4.x * y4; c1 += x4.y * y4; c2 += x4.z * y4; c3 += x4.w * y4;
        c0 += x5.x * y5; c1 += x5.y * y5; c2 += x5.z * y5; c3 += x5.w * y5;
        c0 += x6.x * y6; c1 += x6.y * y6; c2 += x6.z * y6; c3 += x6.w * y6;
        c0 += x7.x * y7; c1 += x7.y * y7; c2 += x7.z * y7; c3 += x7.w * y7;
        c0 += x8.x * y8; c1 += x8.y * y8; c2 += x8.z * y8; c3 += x8.w * y8;
        c0 += x9.x * y9; c1 += x9.y * y9; c2 += x9.z * y9; c3 += x9.w * y9;
        c0 += xa.x * ya; c1 += xa.y * ya; c2 += xa.z * ya; c3 += xa.w * ya;
        c0 += xb.x * yb; c1 += xb.y * yb; c2 += xb.z * yb; c3 += xb.w * yb;
        c0 += xc.x * yc; c1 += xc.y * yc; c2 += xc.z * yc; c3 += xc.w * yc;
        c0 += xd.x * yd; c1 += xd.y * yd; c2 += xd.z * yd; c3 += xd.w * yd;
        c0 += xe.x * ye; c1 += xe.y * ye; c2 += xe.z * ye; c3 += xe.w * ye;
        c0 += xf.x * yf; c1 += xf.y * yf; c2 += xf.z * yf; c3 += xf.w * yf;
        workgroupBarrier();
    }
    let col = col0 + lid.x * 4u;
    let rows = array<vec4<f32>, 4>(c0, c1, c2, c3);
    for (var i = 0u; i < 4u; i++) {
        let row = row0 + lid.y * 4u + i;
        if (row < p.m) {
            let r = rows[i];
            let o = p.c_off + z * p.c_bs + row * p.c_rs + col * p.c_cs;
            if (col < p.n) { mem[o] = finish(r.x, o, col); }
            if (col + 1u < p.n) { mem[o + p.c_cs] = finish(r.y, o + p.c_cs, col + 1u); }
            if (col + 2u < p.n) { mem[o + 2u * p.c_cs] = finish(r.z, o + 2u * p.c_cs, col + 2u); }
            if (col + 3u < p.n) { mem[o + 3u * p.c_cs] = finish(r.w, o + 3u * p.c_cs, col + 3u); }
        }
    }
}

// ---- Row reductions -------------------------------------------------------------------------

var<workgroup> red: array<f32, 64>;

// Sum (or max, with `is_max`) of each thread's `v` across the 64-thread workgroup.
fn reduce(v: f32, lid: u32, is_max: bool) -> f32 {
    red[lid] = v;
    workgroupBarrier();
    for (var s = 32u; s > 0u; s >>= 1u) {
        if (lid < s) {
            red[lid] = select(red[lid] + red[lid + s], max(red[lid], red[lid + s]), is_max);
        }
        workgroupBarrier();
    }
    let r = red[0];
    workgroupBarrier();
    return r;
}

// Layer norm (eps f0) of rows: m rows of n; in at a_off (row stride a_rs), out at c_off (row
// stride c_rs), weight at b_off and bias at bias_off in `w`. One workgroup per row.
@compute @workgroup_size(64)
fn layer_norm(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid3: vec3<u32>) {
    let row = wg.x;
    let lid = lid3.x;
    let x = p.a_off + row * p.a_rs;
    var s = 0.0;
    for (var j = lid; j < p.n; j += 64u) { s += mem[x + j]; }
    let mean = reduce(s, lid, false) / f32(p.n);
    var v = 0.0;
    for (var j = lid; j < p.n; j += 64u) { let d = mem[x + j] - mean; v += d * d; }
    let inv = 1.0 / sqrt(reduce(v, lid, false) / f32(p.n) + p.f0);
    let o = p.c_off + row * p.c_rs;
    for (var j = lid; j < p.n; j += 64u) {
        mem[o + j] = (mem[x + j] - mean) * inv * w[p.b_off + j] + w[p.bias_off + j];
    }
}

// Attention softmax in place over scores [batch (heads)][m][n] at a_off, each score scaled by
// f0 first. One workgroup per row.
@compute @workgroup_size(64)
fn softmax(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid3: vec3<u32>) {
    let head = wg.y;
    let row = wg.x;
    let lid = lid3.x;
    let x = p.a_off + (head * p.m + row) * p.n;
    var mx = -3.0e38;
    for (var j = lid; j < p.n; j += 64u) {
        let s = mem[x + j] * p.f0;
        mem[x + j] = s;
        mx = max(mx, s);
    }
    mx = reduce(mx, lid, true);
    var sum = 0.0;
    for (var j = lid; j < p.n; j += 64u) { let e = exp(mem[x + j] - mx); mem[x + j] = e; sum += e; }
    let inv = 1.0 / reduce(sum, lid, false);
    for (var j = lid; j < p.n; j += 64u) { mem[x + j] *= inv; }
}

// ---- Images (channels first: [channels][height][width]) -------------------------------------

// 3x3 convolution, padding 1, stride x1 (1 or 2): in [k channels][H = m][W = n] at a_off,
// weights [x0][k][3][3] at b_off, bias at bias_off (or NONE), out [x0][H'][W'] at c_off where
// H' = (H - 1) / stride + 1. Flags: RELU_IN, RELU, ACCUMULATE. Each thread makes 8 output
// channels (group wg.z) of one pixel from one read of its 3x3 neighbourhood, with the group's
// weights for 32 input channels at a time in workgroup memory.

// [ci < 32][tap 9][8 output channels]. Plain floats: writing one component of a vec4 in
// workgroup memory (cw[i][o % 4] = v) lost three of every four writes on Metal.
var<workgroup> cw: array<f32, 2304>;

@compute @workgroup_size(16, 16)
fn conv3x3(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>) {
    let co0 = wg.z * 8u;
    let cout = p.x0;
    let stride = max(p.x1, 1u);
    let oh = (p.m - 1u) / stride + 1u;
    let ow = (p.n - 1u) / stride + 1u;
    let x = gid.x;
    let y = gid.y;
    let live = x < ow && y < oh;
    let plane = p.m * p.n;
    let relu_in = (p.flags & RELU_IN) != 0u;
    var acc0 = vec4<f32>(0.0);
    var acc1 = vec4<f32>(0.0);
    for (var c0 = 0u; c0 < p.k; c0 += 32u) {
        let kc = min(32u, p.k - c0);
        workgroupBarrier();
        // This chunk's weights: w[co0 + o][c0 + ci][tap] -> cw[(ci * 9 + tap) * 8 + o].
        for (var e = li; e < kc * 9u * 8u; e += 256u) {
            let o = e % 8u;
            let ct = e / 8u; // ci * 9 + tap
            let co = min(co0 + o, cout - 1u);
            let v = select(0.0, w[p.b_off + co * p.k * 9u + c0 * 9u + ct], co0 + o < cout);
            cw[ct * 8u + o] = v;
        }
        workgroupBarrier();
        if (live) {
            for (var ci = 0u; ci < kc; ci++) {
                let src = p.a_off + (c0 + ci) * plane;
                for (var tap = 0u; tap < 9u; tap++) {
                    let yy = i32(y * stride) + i32(tap / 3u) - 1;
                    let xx = i32(x * stride) + i32(tap % 3u) - 1;
                    let inside = yy >= 0 && yy < i32(p.m) && xx >= 0 && xx < i32(p.n);
                    let at = u32(clamp(yy, 0, i32(p.m) - 1)) * p.n + u32(clamp(xx, 0, i32(p.n) - 1));
                    var v = select(0.0, mem[src + at], inside);
                    if (relu_in) { v = max(v, 0.0); }
                    let wi = (ci * 9u + tap) * 8u;
                    acc0 += v * vec4<f32>(cw[wi], cw[wi + 1u], cw[wi + 2u], cw[wi + 3u]);
                    acc1 += v * vec4<f32>(cw[wi + 4u], cw[wi + 5u], cw[wi + 6u], cw[wi + 7u]);
                }
            }
        }
    }
    if (!live) { return; }
    let outs = array<f32, 8>(acc0.x, acc0.y, acc0.z, acc0.w, acc1.x, acc1.y, acc1.z, acc1.w);
    for (var o = 0u; o < 8u; o++) {
        let co = co0 + o;
        if (co >= cout) { break; }
        var s = outs[o] + bias(co);
        if ((p.flags & RELU) != 0u) { s = max(s, 0.0); }
        let at = p.c_off + co * oh * ow + y * ow + x;
        if ((p.flags & ACCUMULATE) != 0u) { s += mem[at]; }
        mem[at] = s;
    }
}

// Transposed convolution with kernel = stride = x1 (no overlap): in [k][m][n] at a_off,
// weights [k][x0][s][s] at b_off (PyTorch's layout), bias at bias_off, out [x0][m*s][n*s] at
// c_off. One thread per output value.
@compute @workgroup_size(16, 16)
fn conv_transpose(@builtin(global_invocation_id) gid: vec3<u32>) {
    let s = p.x1;
    let ow = p.n * s;
    let oh = p.m * s;
    let x = gid.x;
    let y = gid.y;
    let co = gid.z;
    if (x >= ow || y >= oh || co >= p.x0) { return; }
    let ix = x / s;
    let iy = y / s;
    let tap = (y % s) * s + (x % s);
    let plane = p.m * p.n;
    var acc = bias(co);
    for (var ci = 0u; ci < p.k; ci++) {
        acc += mem[p.a_off + ci * plane + iy * p.n + ix] * w[p.b_off + (ci * p.x0 + co) * s * s + tap];
    }
    mem[p.c_off + co * oh * ow + y * ow + x] = acc;
}

// Bilinear resize, align_corners = true: [k channels][m][n] at a_off to [k][x0][x1] at c_off.
@compute @workgroup_size(16, 16)
fn resize(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    let c = gid.z;
    if (x >= p.x1 || y >= p.x0 || c >= p.k) { return; }
    let sy = select(0.0, f32(p.m - 1u) / f32(p.x0 - 1u), p.x0 > 1u);
    let sx = select(0.0, f32(p.n - 1u) / f32(p.x1 - 1u), p.x1 > 1u);
    let fy = f32(y) * sy;
    let fx = f32(x) * sx;
    let y0 = min(u32(fy), p.m - 1u);
    let x0 = min(u32(fx), p.n - 1u);
    let y1 = min(y0 + 1u, p.m - 1u);
    let x1 = min(x0 + 1u, p.n - 1u);
    let ty = fy - f32(y0);
    let tx = fx - f32(x0);
    let src = p.a_off + c * p.m * p.n;
    let top = mix(mem[src + y0 * p.n + x0], mem[src + y0 * p.n + x1], tx);
    let bottom = mix(mem[src + y1 * p.n + x0], mem[src + y1 * p.n + x1], tx);
    mem[p.c_off + c * p.x0 * p.x1 + y * p.x1 + x] = mix(top, bottom, ty);
}

// C = A over m floats (A at a_off in `w`, C at c_off); with B_MEM, A is in `mem` too; with
// ACCUMULATE, C += A in `mem`.
@compute @workgroup_size(64)
fn copy(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.y * 65535u * 64u + gid.x;
    if (i >= p.m) { return; }
    if ((p.flags & ACCUMULATE) != 0u) {
        mem[p.c_off + i] += mem[p.a_off + i];
    } else if ((p.flags & B_MEM) != 0u) {
        mem[p.c_off + i] = mem[p.a_off + i];
    } else {
        mem[p.c_off + i] = w[p.a_off + i];
    }
}
