// The restyle networks' kernels (see animegan.rs), next to depth.wgsl's: their 7x7 and 9x9
// convolutions, the statistics over the whole picture their norms use, the norms themselves,
// the softmax and normalising of AnimeGANv3's attention, doubling, and the packing of what comes
// out.
// Pictures are channels first, [channels][height][width], in `mem`.

/// Tanh after (the 7x7 convolution).
const TANH: u32 = 256u;
/// The norm adds `b` before its leaky ReLU (else after).
const ADD_BEFORE: u32 = 512u;
/// The norm's ReLU is a plain one (else leaky, 0.2).
const NORM_RELU: u32 = 4096u;
/// The norm has no ReLU at all.
const NORM_LINEAR: u32 = 8192u;
/// ag_join: the second's mean and spread are a scale and shift for each channel from the
/// weights instead (an instance norm's).
const AFFINE: u32 = 16384u;
/// ag_double: nearest, not bilinear.
const NEAREST: u32 = 32768u;
/// ag_pack: the values are already 0 to 255.
const BYTES: u32 = 65536u;
/// ag_pack: one channel, through a sigmoid (0 to 1), as grey.
const GREY: u32 = 131072u;
/// ag_join: with AFFINE, the scales and shifts are in `mem` (a style's), not the weights.
const GAMMA_MEM: u32 = 262144u;
/// ag_norm: through a sigmoid after (and no ReLU).
const NORM_SIGMOID: u32 = 524288u;
/// ag_pack: the values are 0 to 1.
const UNIT: u32 = 1048576u;

// k x k convolution (k = 3, 5, 7 or 9), stride 1, reflect padding k / 2: in [k][H = m][W = n] at
// a_off, weights [x0][k][k][k] at b_off, bias at bias_off (or NONE), out [x0][H][W] at c_off.
// Flags: TANH, LEAKY2, RELU, ACCUMULATE, LEAKY2_LAST. A 64 x 16 tile of pixels a workgroup: each thread makes 4 pixels side by
// side for 8 output channels (group wg.z), sliding along each row of its neighbourhood, with 2
// input channels at a time in workgroup memory (15 KB in all, under WebGPU's least 16; 9 x 9
// takes one at a time to fit).
var<workgroup> ckw: array<vec4<f32>, 196>; // [ci 2][tap k * k][2] x 4 outputs
var<workgroup> ckx: array<f32, 3080>; // [ci 2][16 + k - 1][64 + k - 1]

fn conv_k(ks: u32, lid: vec3<u32>, li: u32, wg: vec3<u32>) {
    let cout = p.x0;
    let co0 = wg.z * 8u;
    let x0 = wg.x * 64u + lid.x * 4u;
    let y = wg.y * 16u + lid.y;
    let plane = p.m * p.n;
    let taps = ks * ks;
    let rows = 15u + ks;
    let cols = 63u + ks;
    let area = rows * cols;
    let tx0 = i32(wg.x * 64u) - i32(ks / 2u);
    let ty0 = i32(wg.y * 16u) - i32(ks / 2u);
    var a0 = array<vec4<f32>, 4>();
    var a1 = array<vec4<f32>, 4>();
    let batch = select(2u, 1u, ks > 7u);
    for (var c0 = 0u; c0 < p.k; c0 += batch) {
        let kc = min(batch, p.k - c0);
        workgroupBarrier();
        for (var e = li; e < kc * taps * 2u; e += 256u) {
            let half = e % 2u;
            let tap = (e / 2u) % taps;
            let ci = e / (2u * taps);
            var v = vec4<f32>(0.0);
            for (var j = 0u; j < 4u; j++) {
                let o = co0 + half * 4u + j;
                v[j] = select(0.0, w[p.b_off + min(o, cout - 1u) * p.k * taps + (c0 + ci) * taps + tap], o < cout);
            }
            ckw[e] = v;
        }
        for (var e = li; e < kc * area; e += 256u) {
            let ci = e / area;
            let r = e % area;
            let yy = reflect(ty0 + i32(r / cols), i32(p.m));
            let xx = reflect(tx0 + i32(r % cols), i32(p.n));
            ckx[e] = mem[p.a_off + (c0 + ci) * plane + u32(clamp(yy, 0, i32(p.m) - 1)) * p.n + u32(clamp(xx, 0, i32(p.n) - 1))];
        }
        workgroupBarrier();
        for (var ci = 0u; ci < kc; ci++) {
            for (var dy = 0u; dy < ks; dy++) {
                let row = ci * area + (lid.y + dy) * cols + lid.x * 4u;
                var r: array<f32, 12>;
                for (var i = 0u; i < ks + 3u; i++) { r[i] = ckx[row + i]; }
                let wr = (ci * taps + dy * ks) * 2u;
                for (var dx = 0u; dx < ks; dx++) {
                    let w0 = ckw[wr + dx * 2u];
                    let w1 = ckw[wr + dx * 2u + 1u];
                    for (var j = 0u; j < 4u; j++) {
                        a0[j] += r[dx + j] * w0;
                        a1[j] += r[dx + j] * w1;
                    }
                }
            }
        }
    }
    if (y >= p.m) { return; }
    for (var j = 0u; j < 4u; j++) {
        let x = x0 + j;
        if (x >= p.n) { break; }
        for (var o = 0u; o < 8u; o++) {
            let co = co0 + o;
            if (co >= cout) { break; }
            var s = select(a1[j][o - 4u], a0[j][min(o, 3u)], o < 4u);
            s = leaky2(s + bias(co), p.flags, LEAKY2);
            if ((p.flags & RELU) != 0u) { s = max(s, 0.0); }
            if ((p.flags & TANH) != 0u) { s = tanh(s); }
            let at = p.c_off + co * plane + y * p.n + x;
            if ((p.flags & ACCUMULATE) != 0u) { s += mem[at]; }
            mem[at] = leaky2(s, p.flags, LEAKY2_LAST);
        }
    }
}

@compute @workgroup_size(16, 16)
fn ag_conv7(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>) {
    conv_k(7u, lid, li, wg);
}

@compute @workgroup_size(16, 16)
fn ag_conv9(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>) {
    conv_k(9u, lid, li, wg);
}

@compute @workgroup_size(16, 16)
fn ag_conv5(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>) {
    conv_k(5u, lid, li, wg);
}

@compute @workgroup_size(16, 16)
fn ag_conv3(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>) {
    conv_k(3u, lid, li, wg);
}

// Each channel's mean and spread over the whole picture, in two steps. First, for each channel of
// [k][m] at a_off (m values a channel) and each of x0 parts of it, the part's mean and sum of
// squared differences from that, to [k][x0][2] at c_off. Grid: (x0 parts, k channels).
// One read of each value: each thread keeps a running mean and sum of squared differences
// (Welford's), and the threads' are joined (Chan's).
var<workgroup> sn: array<f32, 64>;
var<workgroup> sm: array<f32, 64>;
var<workgroup> sq: array<f32, 64>;

@compute @workgroup_size(64)
fn ag_stats(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid3: vec3<u32>) {
    let part = wg.x;
    let c = wg.y;
    let lid = lid3.x;
    let per = (p.m + p.x0 - 1u) / p.x0;
    let start = part * per;
    let end = min(start + per, p.m);
    let src = p.a_off + c * p.m;
    var n = 0.0;
    var mean = 0.0;
    var m2 = 0.0;
    for (var i = start + lid; i < end; i += 64u) {
        let x = mem[src + i];
        n += 1.0;
        let d = x - mean;
        mean += d / n;
        m2 += d * (x - mean);
    }
    sn[lid] = n;
    sm[lid] = mean;
    sq[lid] = m2;
    workgroupBarrier();
    for (var s = 32u; s > 0u; s >>= 1u) {
        if (lid < s) {
            let na = sn[lid];
            let nb = sn[lid + s];
            let nn = max(na + nb, 1.0);
            let d = sm[lid + s] - sm[lid];
            sm[lid] += d * nb / nn;
            sq[lid] += sq[lid + s] + d * d * na * nb / nn;
            sn[lid] = na + nb;
        }
        workgroupBarrier();
    }
    if (lid == 0u) {
        let o = p.c_off + (c * p.x0 + part) * 2u;
        mem[o] = sm[0];
        mem[o + 1u] = sq[0];
    }
}

// Then, for each of x1 channels: the parts (from ag_stats over 2 x1 channels: a convolution's
// output, then the 1x1 convolution's of that) joined, and the norm's scale and shift written to
// [x1][2] at c_off: the first's values normalised, then given the second's mean and spread.
// eps is f0. With mem[bias_off] above 0 (bias_off not NONE), they're eased from what was there by
// that much. With AFFINE, the parts are only the first's, and the scale and shift those that
// normalise it and then scale and shift it by the weights' [x1] scales and [x1] shifts at b_off
// (an instance norm's), or with GAMMA_MEM, in `mem` at b_off (a style's).
@compute @workgroup_size(64)
fn ag_join(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = gid.x;
    if (c >= p.x1) { return; }
    let per = (p.m + p.x0 - 1u) / p.x0;
    var mv: array<vec2<f32>, 2>;
    let affine = (p.flags & AFFINE) != 0u;
    for (var which = 0u; which < select(2u, 1u, affine); which++) {
        let src = p.a_off + ((c + which * p.x1) * p.x0) * 2u;
        var mean = 0.0;
        for (var i = 0u; i < p.x0; i++) {
            let n = f32(min(per, p.m - min(p.m, i * per)));
            mean += mem[src + i * 2u] * n;
        }
        mean /= f32(p.m);
        var m2 = 0.0;
        for (var i = 0u; i < p.x0; i++) {
            let n = f32(min(per, p.m - min(p.m, i * per)));
            let d = mem[src + i * 2u] - mean;
            m2 += mem[src + i * 2u + 1u] + n * d * d;
        }
        mv[which] = vec2<f32>(mean, m2 / f32(p.m));
    }
    var scale = sqrt(mv[1].y + p.f0) / sqrt(mv[0].y + p.f0);
    var shift = mv[1].x - mv[0].x * scale;
    if (affine) {
        var gamma = w[p.b_off + c];
        var beta = w[p.b_off + p.x1 + c];
        if ((p.flags & GAMMA_MEM) != 0u) {
            gamma = mem[p.b_off + c];
            beta = mem[p.b_off + p.x1 + c];
        }
        scale = gamma / sqrt(mv[0].y + p.f0);
        shift = beta - mv[0].x * scale;
    }
    let o = p.c_off + c * 2u;
    var ease = 0.0;
    if (p.bias_off != NONE) { ease = clamp(mem[p.bias_off], 0.0, 1.0); }
    mem[o] = mix(scale, mem[o], ease);
    mem[o + 1u] = mix(shift, mem[o + 1u], ease);
}

// A layer norm over a whole [x1][m] picture, from ag_stats' parts for its x1 channels at a_off
// (x0 parts each): the scale and shift that make it mean 0 and spread 1, then gamma and beta
// (each [x1], side by side at b_off in the weights), for each channel to [x1][2] at c_off.
// eps is f0. One workgroup, x1 up to 256.
@compute @workgroup_size(256)
fn ag_layer(@builtin(local_invocation_id) lid3: vec3<u32>) {
    let c = lid3.x;
    let live = c < p.x1;
    let per = (p.m + p.x0 - 1u) / p.x0;
    let src = p.a_off + c * p.x0 * 2u;
    var mean = 0.0;
    var m2 = 0.0;
    if (live) {
        for (var i = 0u; i < p.x0; i++) {
            mean += mem[src + i * 2u] * f32(min(per, p.m - min(p.m, i * per)));
        }
        mean /= f32(p.m);
        for (var i = 0u; i < p.x0; i++) {
            let d = mem[src + i * 2u] - mean;
            m2 += mem[src + i * 2u + 1u] + f32(min(per, p.m - min(p.m, i * per))) * d * d;
        }
    }
    // Every channel has m values: the whole's mean is the channels' mean, and its squared
    // differences are theirs plus each channel's mean's from the whole's.
    let all = reduce256(select(0.0, mean, live), c, false) / f32(p.x1);
    let d = mean - all;
    let spread = reduce256(select(0.0, m2 + f32(p.m) * d * d, live), c, false) / (f32(p.m) * f32(p.x1));
    if (!live) { return; }
    let scale = w[p.b_off + c] / sqrt(spread + p.f0);
    mem[p.c_off + c * 2u] = scale;
    mem[p.c_off + c * 2u + 1u] = w[p.b_off + p.x1 + c] - all * scale;
}

// out[c][i] = leaky(in[c][i] * scale[c] + shift[c]), with in [k][m] at a_off, the scales and
// shifts [k][2] at x0 in `mem` (or NONE for none), out at c_off, and [k][m] at b_off (unless
// NONE) added after the leaky ReLU (or before it, with ADD_BEFORE). With NORM_RELU the ReLU is a
// plain one; with NORM_LINEAR there's none; with NORM_SIGMOID, a sigmoid instead. Grid: (m / 256, k).
@compute @workgroup_size(256)
fn ag_norm(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    let c = gid.y;
    if (i >= p.m) { return; }
    let at = c * p.m + i;
    var v = mem[p.a_off + at];
    if (p.x0 != NONE) { v = v * mem[p.x0 + c * 2u] + mem[p.x0 + c * 2u + 1u]; }
    let add = p.b_off != NONE;
    let before = (p.flags & ADD_BEFORE) != 0u;
    if (add && before) { v += mem[p.b_off + at]; }
    if ((p.flags & NORM_SIGMOID) != 0u) {
        v = 1.0 / (1.0 + exp(-v));
    } else if ((p.flags & NORM_LINEAR) == 0u) {
        v = select(v * select(0.2, 0.0, (p.flags & NORM_RELU) != 0u), v, v >= 0.0);
    }
    if (add && !before) { v += mem[p.b_off + at]; }
    mem[p.c_off + at] = v;
}

/// ag_softmax divides each row by f0 + its sum instead.
const SUMS: u32 = 1u;
/// ag_columns makes each column a softmax instead.
const SOFTMAX: u32 = 1u;

// Softmax in place along each of m rows of n at a_off (or, with SUMS, each divided by f0 + its
// sum). One workgroup a row.
@compute @workgroup_size(256)
fn ag_softmax(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid3: vec3<u32>) {
    let lid = lid3.x;
    let x = p.a_off + wg.x * p.n;
    if ((p.flags & SUMS) != 0u) {
        var total = 0.0;
        for (var j = lid; j < p.n; j += 256u) { total += mem[x + j]; }
        let inv = 1.0 / (p.f0 + reduce256(total, lid, false));
        for (var j = lid; j < p.n; j += 256u) { mem[x + j] *= inv; }
        return;
    }
    var mx = -3.0e38;
    for (var j = lid; j < p.n; j += 256u) { mx = max(mx, mem[x + j]); }
    mx = reduce256(mx, lid, true);
    var sum = 0.0;
    for (var j = lid; j < p.n; j += 256u) { sum += exp(mem[x + j] - mx); }
    let inv = 1.0 / reduce256(sum, lid, false);
    for (var j = lid; j < p.n; j += 256u) { mem[x + j] = exp(mem[x + j] - mx) * inv; }
}

var<workgroup> red256: array<f32, 256>;

fn reduce256(v: f32, lid: u32, is_max: bool) -> f32 {
    red256[lid] = v;
    workgroupBarrier();
    for (var s = 128u; s > 0u; s >>= 1u) {
        if (lid < s) {
            red256[lid] = select(red256[lid] + red256[lid + s], max(red256[lid], red256[lid + s]), is_max);
        }
        workgroupBarrier();
    }
    let r = red256[0];
    workgroupBarrier();
    return r;
}

// Each of n columns of [m][n] at a_off divided by (f0 + its sum), in place (or, with SOFTMAX,
// made a softmax).
@compute @workgroup_size(64)
fn ag_columns(@builtin(global_invocation_id) gid: vec3<u32>) {
    let j = gid.x;
    if (j >= p.n) { return; }
    if ((p.flags & SOFTMAX) != 0u) {
        var mx = -3.0e38;
        for (var i = 0u; i < p.m; i++) { mx = max(mx, mem[p.a_off + i * p.n + j]); }
        var total = 0.0;
        for (var i = 0u; i < p.m; i++) { total += exp(mem[p.a_off + i * p.n + j] - mx); }
        for (var i = 0u; i < p.m; i++) { mem[p.a_off + i * p.n + j] = exp(mem[p.a_off + i * p.n + j] - mx) / total; }
        return;
    }
    var s = 0.0;
    for (var i = 0u; i < p.m; i++) { s += mem[p.a_off + i * p.n + j]; }
    let inv = 1.0 / (p.f0 + s);
    for (var i = 0u; i < p.m; i++) { mem[p.a_off + i * p.n + j] *= inv; }
}

// Twice the size, bilinear, each output pixel at half its place in the input (ONNX's Upsample,
// TensorFlow's resize without half pixels): [k][m][n] at a_off to [k][2m][2n] at c_off. With
// NEAREST, each input pixel four times.
@compute @workgroup_size(16, 16)
fn ag_double(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    let c = gid.z;
    if (x >= 2u * p.n || y >= 2u * p.m) { return; }
    let y0 = y / 2u;
    let x0 = x / 2u;
    let y1 = min(y0 + (y & 1u), p.m - 1u);
    let x1 = min(x0 + (x & 1u), p.n - 1u);
    let src = p.a_off + c * p.m * p.n;
    let out = p.c_off + c * 4u * p.m * p.n + y * 2u * p.n + x;
    if ((p.flags & NEAREST) != 0u) {
        mem[out] = mem[src + y0 * p.n + x0];
        return;
    }
    let v = mem[src + y0 * p.n + x0] + mem[src + y0 * p.n + x1] + mem[src + y1 * p.n + x0] + mem[src + y1 * p.n + x1];
    mem[out] = v * 0.25;
}

// Four pictures [4][k][m][n] at a_off (the places in each 2x2: top left, top right, bottom left,
// bottom right) woven into one twice the size, [k][2m][2n] at c_off.
@compute @workgroup_size(16, 16)
fn ag_weave(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    let c = gid.z;
    if (x >= 2u * p.n || y >= 2u * p.m) { return; }
    let plane = p.m * p.n;
    let place = (y & 1u) * 2u + (x & 1u);
    mem[p.c_off + c * 4u * plane + y * 2u * p.n + x] = mem[p.a_off + (place * p.k + c) * plane + (y / 2u) * p.n + x / 2u];
}

// [3][m] at a_off, from -1 to 1 (or 0 to 255, with BYTES; 0 to 1, with UNIT; or with GREY, [1][m]
// through a sigmoid as grey), to m RGB bytes packed in words at c_off. The top byte is 0x40,
// which makes every word an ordinary float (from 2 to 8): never a NaN or a denormal, that
// something on the way might change.
@compute @workgroup_size(256)
fn ag_pack(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.y * 65535u * 256u + gid.x;
    if (i >= p.m) { return; }
    var bytes: vec3<f32>;
    if ((p.flags & GREY) != 0u) {
        bytes = vec3<f32>(255.0 / (1.0 + exp(-mem[p.a_off + i])));
    } else {
        let rgb = vec3<f32>(mem[p.a_off + i], mem[p.a_off + p.m + i], mem[p.a_off + 2u * p.m + i]);
        bytes = select((rgb + 1.0) * 127.5, rgb, (p.flags & BYTES) != 0u);
        if ((p.flags & UNIT) != 0u) { bytes = rgb * 255.0; }
    }
    let b = vec3<u32>(clamp(bytes + 0.5, vec3<f32>(0.0), vec3<f32>(255.0)));
    mem[p.c_off + i] = bitcast<f32>(b.x | (b.y << 8u) | (b.z << 16u) | (0x40u << 24u));
}
