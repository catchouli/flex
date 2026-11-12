
// ---- NeuFlow v2 (flow.rs), added after depth.wgsl's kernels ------------------------------------

fn activate(v: f32, flags: u32) -> f32 {
    if ((flags & LEAKY) != 0u) { return select(v * 0.1, v, v >= 0.0); }
    if ((flags & GELU) != 0u) { return gelu(v); }
    return v;
}

// Any other convolution, leaky ReLU after: in [k][m][n] at a_off, weights [x0][k][a_bs][a_bs] at
// b_off, bias at bias_off, stride x1, padding a_rs, out [x0][b_bs][b_rs] at c_off. Each thread
// makes 8 output channels of one pixel, with the group's weights for as many input channels as
// fit in workgroup memory at a time.
var<workgroup> cwide: array<f32, 4608>;

@compute @workgroup_size(16, 16)
fn flow_conv(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wg: vec3<u32>) {
    let co0 = wg.z * 8u;
    let size = p.a_bs;
    let taps = size * size;
    let step = 4608u / (taps * 8u);
    let live = gid.x < p.b_rs && gid.y < p.b_bs;
    let plane = p.m * p.n;
    var acc0 = vec4<f32>(0.0);
    var acc1 = vec4<f32>(0.0);
    for (var c0 = 0u; c0 < p.k; c0 += step) {
        let kc = min(step, p.k - c0);
        workgroupBarrier();
        for (var e = li; e < kc * taps * 8u; e += 256u) {
            let o = e % 8u;
            let ct = e / 8u;
            let co = min(co0 + o, p.x0 - 1u);
            cwide[ct * 8u + o] = select(0.0, w[p.b_off + (co * p.k + c0) * taps + ct], co0 + o < p.x0);
        }
        workgroupBarrier();
        if (live) {
            for (var ci = 0u; ci < kc; ci++) {
                let src = p.a_off + (c0 + ci) * plane;
                for (var tap = 0u; tap < taps; tap++) {
                    let yy = i32(gid.y * p.x1) + i32(tap / size) - i32(p.a_rs);
                    let xx = i32(gid.x * p.x1) + i32(tap % size) - i32(p.a_rs);
                    let inside = yy >= 0 && yy < i32(p.m) && xx >= 0 && xx < i32(p.n);
                    let at = u32(clamp(yy, 0, i32(p.m) - 1)) * p.n + u32(clamp(xx, 0, i32(p.n) - 1));
                    let v = select(0.0, mem[src + at], inside);
                    let wi = (ci * taps + tap) * 8u;
                    acc0 += v * vec4<f32>(cwide[wi], cwide[wi + 1u], cwide[wi + 2u], cwide[wi + 3u]);
                    acc1 += v * vec4<f32>(cwide[wi + 4u], cwide[wi + 5u], cwide[wi + 6u], cwide[wi + 7u]);
                }
            }
        }
    }
    if (!live) { return; }
    let outs = array<f32, 8>(acc0.x, acc0.y, acc0.z, acc0.w, acc1.x, acc1.y, acc1.z, acc1.w);
    for (var o = 0u; o < 8u; o++) {
        let co = co0 + o;
        if (co >= p.x0) { break; }
        mem[p.c_off + co * p.b_bs * p.b_rs + gid.y * p.b_rs + gid.x] = activate(outs[o] + bias(co), p.flags);
    }
}

// Halves a picture, each output the average of four: [k][m][n] at a_off to [k][m/2][n/2] at c_off.
@compute @workgroup_size(16, 16)
fn flow_half(@builtin(global_invocation_id) id: vec3<u32>) {
    let oh = p.m / 2u;
    let ow = p.n / 2u;
    if (id.x >= ow || id.y >= oh) { return; }
    let top = p.a_off + id.z * p.m * p.n + id.y * 2u * p.n + id.x * 2u;
    let sum = mem[top] + mem[top + 1u] + mem[top + p.n] + mem[top + p.n + 1u];
    mem[p.c_off + id.z * oh * ow + id.y * ow + id.x] = sum * 0.25;
}

// Doubles a picture, nearest neighbour, scaling the values by f0: [k][m][n] at a_off to
// [k][2m][2n] at c_off.
@compute @workgroup_size(16, 16)
fn flow_double(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.n * 2u || id.y >= p.m * 2u) { return; }
    let v = mem[p.a_off + id.z * p.m * p.n + (id.y / 2u) * p.n + id.x / 2u];
    mem[p.c_off + id.z * p.m * p.n * 4u + id.y * p.n * 2u + id.x] = v * p.f0;
}

// For each of m values at a_off: clamp(v * f0 + f1, f2, f3), with GELU if asked, to c_off, or
// added to it with ACCUMULATE.
@compute @workgroup_size(64)
fn flow_map(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.y * 65535u * 64u + gid.x;
    if (i >= p.m) { return; }
    let v = activate(clamp(mem[p.a_off + i] * p.f0 + p.f1, p.f2, p.f3), p.flags);
    if ((p.flags & ACCUMULATE) != 0u) { mem[p.c_off + i] += v; } else { mem[p.c_off + i] = v; }
}

// [m][n] at a_off to [n][m] at c_off.
@compute @workgroup_size(64)
fn flow_transpose(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.y * 65535u * 64u + gid.x;
    if (i >= p.m * p.n) { return; }
    let row = i / p.n;
    let col = i % p.n;
    mem[p.c_off + col * p.m + row] = mem[p.a_off + i];
}

// Each channel of [k][m] at a_off, times w[b_off + channel], plus w[bias_off + channel], to c_off.
@compute @workgroup_size(64)
fn flow_channels(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.y * 65535u * 64u + gid.x;
    if (i >= p.k * p.m) { return; }
    let channel = i / p.m;
    mem[p.c_off + i] = mem[p.a_off + i] * w[p.b_off + channel] + w[p.bias_off + channel];
}

fn corr_at(volume: u32, x: i32, y: i32) -> f32 {
    if (x < 0 || y < 0 || x >= i32(p.n) || y >= i32(p.m)) { return 0.0; }
    return mem[volume + u32(y) * p.n + u32(x)];
}

// For each pixel of [m][n], the correlations with the 9 x 9 places around where its flow points,
// each bilinear, nothing outside the picture. Correlation volume [m*n][m][n] at a_off (scaled
// by f0 as it's read), flow [2][m][n] at b_off, out [81][m][n] at c_off, x offset slowest.
@compute @workgroup_size(16, 16)
fn flow_correlate(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.n || id.y >= p.m) { return; }
    let plane = p.m * p.n;
    let pixel = id.y * p.n + id.x;
    let volume = p.a_off + pixel * plane;
    let px = f32(id.x) + mem[p.b_off + pixel];
    let py = f32(id.y) + mem[p.b_off + plane + pixel];
    for (var i = 0u; i < 9u; i++) {
        for (var j = 0u; j < 9u; j++) {
            let x = px + f32(i) - 4.0;
            let y = py + f32(j) - 4.0;
            let fx = floor(x);
            let fy = floor(y);
            let ax = x - fx;
            let ay = y - fy;
            let x0 = i32(fx);
            let y0 = i32(fy);
            let top = mix(corr_at(volume, x0, y0), corr_at(volume, x0 + 1, y0), ax);
            let low = mix(corr_at(volume, x0, y0 + 1), corr_at(volume, x0 + 1, y0 + 1), ax);
            mem[p.c_off + (i * 9u + j) * plane + pixel] = mix(top, low, ay) * p.f0;
        }
    }
}

// Where each pixel of [m][n] matches, on average, less where it is: probabilities [m*n][m*n] at
// a_off, flow [2][m][n] at c_off.
@compute @workgroup_size(64)
fn flow_expect(@builtin(global_invocation_id) gid: vec3<u32>) {
    let plane = p.m * p.n;
    let row = gid.x;
    if (row >= plane) { return; }
    var sx = 0.0;
    var sy = 0.0;
    for (var j = 0u; j < plane; j++) {
        let v = mem[p.a_off + row * plane + j];
        sx += v * f32(j % p.n);
        sy += v * f32(j / p.n);
    }
    mem[p.c_off + row] = sx - f32(row % p.n);
    mem[p.c_off + plane + row] = sy - f32(row / p.n);
}

// A convolution's inputs laid out for a matrix multiply ("im2col"): for each input channel ci and
// tap (ky, kx) of a size x size kernel, a row of what each output pixel sees there, zero outside.
// In [k][m][n] at a_off, stride x1, padding a_rs, size a_bs, out [k * size * size][b_bs * b_rs] at
// c_off.
@compute @workgroup_size(64)
fn flow_im2col(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixels = p.b_bs * p.b_rs;
    let i = gid.y * 65535u * 64u + gid.x;
    let taps = p.a_bs * p.a_bs;
    if (i >= p.k * taps * pixels) { return; }
    let row = i / pixels;
    let px = i % pixels;
    let ci = row / taps;
    let tap = row % taps;
    let yy = i32((px / p.b_rs) * p.x1 + tap / p.a_bs) - i32(p.a_rs);
    let xx = i32((px % p.b_rs) * p.x1 + tap % p.a_bs) - i32(p.a_rs);
    let inside = yy >= 0 && yy < i32(p.m) && xx >= 0 && xx < i32(p.n);
    let at = p.a_off + ci * p.m * p.n + u32(clamp(yy, 0, i32(p.m) - 1)) * p.n + u32(clamp(xx, 0, i32(p.n) - 1));
    mem[p.c_off + i] = select(0.0, mem[at], inside);
}

// The parts of a split sum added up, with the bias and activation: x0 parts of [k][m] at a_off,
// one after the other, to [k][m] at c_off.
@compute @workgroup_size(64)
fn flow_sum(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.y * 65535u * 64u + gid.x;
    let count = p.k * p.m;
    if (i >= count) { return; }
    var v = bias(i / p.m);
    for (var part = 0u; part < p.x0; part++) {
        v += mem[p.a_off + part * count + i];
    }
    mem[p.c_off + i] = activate(v, p.flags);
}
