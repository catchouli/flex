//! Pieces of plain signal processing the effects share: filters, a delay line, noise, and a
//! reverb.

use std::f32::consts::PI;

/// A second-order filter (Robert Bristow-Johnson's cookbook).
#[derive(Clone, Copy, Default)]
pub struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Biquad {
    fn make(b: [f32; 3], a: [f32; 3]) -> Self {
        Self { b: b.map(|v| v / a[0]), a: [a[1] / a[0], a[2] / a[0]], ..Default::default() }
    }

    /// The angle and the width a frequency `hz` and a `q` make, at `rate`.
    fn angle(rate: f32, hz: f32, q: f32) -> (f32, f32) {
        let w = 2.0 * PI * hz.clamp(1.0, rate * 0.49) / rate;
        (w, w.sin() / (2.0 * q))
    }

    pub fn lowpass(rate: f32, hz: f32, q: f32) -> Self {
        let (w, alpha) = Self::angle(rate, hz, q);
        let c = w.cos();
        Self::make([(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0], [1.0 + alpha, -2.0 * c, 1.0 - alpha])
    }

    pub fn highpass(rate: f32, hz: f32, q: f32) -> Self {
        let (w, alpha) = Self::angle(rate, hz, q);
        let c = w.cos();
        Self::make([(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0], [1.0 + alpha, -2.0 * c, 1.0 - alpha])
    }

    /// A band around `hz`, as loud at `hz` as what came in.
    pub fn bandpass(rate: f32, hz: f32, q: f32) -> Self {
        let (w, alpha) = Self::angle(rate, hz, q);
        Self::make([alpha, 0.0, -alpha], [1.0 + alpha, -2.0 * w.cos(), 1.0 - alpha])
    }

    /// `db` louder (or quieter) around `hz`.
    pub fn peak(rate: f32, hz: f32, q: f32, db: f32) -> Self {
        let (w, alpha) = Self::angle(rate, hz, q);
        let a = 10f32.powf(db / 40.0);
        let c = w.cos();
        Self::make([1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a], [1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a])
    }

    /// The same filter with new settings, carrying on from where this one was.
    pub fn retune(&mut self, to: Self) {
        self.b = to.b;
        self.a = to.a;
    }

    pub fn next(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1] - self.a[0] * self.y[0] - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        // Kept from running down into denormals once the sound stops.
        self.y = [if y.abs() < 1e-20 { 0.0 } else { y }, self.y[0]];
        y
    }
}

/// How far a one-pole smoother moves a sample to take `secs` to get most of the way.
pub fn follow(secs: f32, rate: f32) -> f32 {
    1.0 - (-1.0 / (secs * rate).max(1e-3)).exp()
}

/// The sound as it came in, lately.
pub struct Line {
    samples: Vec<f32>,
    /// Where the newest sample is.
    at: usize,
}

impl Line {
    pub fn new(len: usize) -> Self {
        Self { samples: vec![0.0; (len + 2).next_power_of_two()], at: 0 }
    }

    pub fn push(&mut self, x: f32) {
        self.at = (self.at + 1) & (self.samples.len() - 1);
        self.samples[self.at] = x;
    }

    /// The sound `delay` samples ago (0 is the newest), between samples by straight lines.
    pub fn at(&self, delay: f32) -> f32 {
        let mask = self.samples.len() - 1;
        let delay = delay.clamp(0.0, (mask - 1) as f32);
        let whole = delay.floor();
        let part = delay - whole;
        let i = self.at.wrapping_sub(whole as usize) & mask;
        let a = self.samples[i];
        let b = self.samples[i.wrapping_sub(1) & mask];
        a + (b - a) * part
    }
}

/// Cheap noise (xorshift).
#[derive(Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(seed.max(1))
    }

    fn bits(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// −1 to 1.
    pub fn noise(&mut self) -> f32 {
        self.bits() as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    /// 0 to 1.
    pub fn unit(&mut self) -> f32 {
        self.bits() as f32 / u32::MAX as f32
    }
}

/// An allpass that smears sound out in time without colouring it (for a reverb's start).
struct Diffuser {
    line: Vec<f32>,
    at: usize,
    gain: f32,
}

impl Diffuser {
    fn new(len: usize, gain: f32) -> Self {
        Self { line: vec![0.0; len.max(1)], at: 0, gain }
    }

    fn next(&mut self, x: f32) -> f32 {
        let delayed = self.line[self.at];
        let v = x + self.gain * delayed;
        self.line[self.at] = v;
        self.at = (self.at + 1) % self.line.len();
        delayed - self.gain * v
    }
}

/// What kind of space a reverb is.
#[derive(Clone, Copy, Debug)]
pub struct Space {
    /// How long it rings on for, to a thousandth (60 dB down), in seconds.
    pub seconds: f32,
    /// How big it is, 1 for a concert hall.
    pub size: f32,
    /// How much quicker the highs die away than the lows, 0–1.
    pub damping: f32,
    /// How long before the first reflections arrive, in seconds.
    pub gap: f32,
}

/// The lines' lengths for a concert hall, in seconds: far enough apart not to ring together.
const LINES: [f32; 8] = [0.0297, 0.0371, 0.0411, 0.0437, 0.0530, 0.0599, 0.0677, 0.0731];

/// A reverb: eight delay lines fed back into each other through a mixing matrix (a feedback
/// delay network), each line losing what it should for how long it is, and its highs faster.
pub struct Reverb {
    gap: Line,
    gap_len: f32,
    diffusers: Vec<Diffuser>,
    lines: Vec<Line>,
    lengths: [f32; 8],
    gains: [f32; 8],
    damp: f32,
    lows: [f32; 8],
}

impl Reverb {
    pub fn new(rate: u32, space: Space) -> Self {
        let rate = rate as f32;
        let lengths = LINES.map(|s| (s * space.size.clamp(0.1, 3.0) * rate).round().max(8.0));
        let gains = lengths.map(|l| 10f32.powf(-3.0 * l / (space.seconds.max(0.05) * rate)));
        let diffusers = [0.0047, 0.0036, 0.0127, 0.0093].iter().map(|s| Diffuser::new((s * space.size.clamp(0.3, 1.5) * rate) as usize, 0.7)).collect();
        let gap_len = (space.gap * rate).max(1.0);
        Self {
            gap: Line::new(gap_len as usize + 1),
            gap_len,
            diffusers,
            lines: lengths.iter().map(|&l| Line::new(l as usize + 1)).collect(),
            lengths,
            gains,
            damp: space.damping.clamp(0.0, 0.95),
            lows: [0.0; 8],
        }
    }

    /// One sample in, and what the space gives back into each of `out`'s channels (each
    /// hears the lines mixed differently, so they're spread out).
    pub fn next(&mut self, x: f32, out: &mut [f32]) {
        self.gap.push(x);
        let mut x = self.gap.at(self.gap_len);
        for d in &mut self.diffusers {
            x = d.next(x);
        }
        let mut taps = [0.0; 8];
        for (i, t) in taps.iter_mut().enumerate() {
            let y = self.lines[i].at(self.lengths[i] - 1.0);
            // The highs lost a little more each time round.
            self.lows[i] += (y - self.lows[i]) * (1.0 - self.damp);
            *t = self.lows[i] * self.gains[i];
        }
        // Mixed by an 8×8 Hadamard matrix (energy kept), then back in with the new sound.
        let mut m = taps;
        let mut h = 1;
        while h < 8 {
            for i in (0..8).step_by(h * 2) {
                for j in i..i + h {
                    let (a, b) = (m[j], m[j + h]);
                    m[j] = a + b;
                    m[j + h] = a - b;
                }
            }
            h *= 2;
        }
        let scale = 1.0 / 8f32.sqrt();
        for (i, line) in self.lines.iter_mut().enumerate() {
            let v = m[i] * scale + x;
            line.push(if v.abs() < 1e-20 { 0.0 } else { v });
        }
        for (c, o) in out.iter_mut().enumerate() {
            // Each channel's own signs over the lines.
            let row = (c * 3 + 1) % 8;
            *o = taps.iter().enumerate().map(|(i, t)| if (row & i).count_ones() % 2 == 0 { *t } else { -*t }).sum::<f32>() * 0.35;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reverb_rings_on_and_dies_away() {
        let rate = 48_000;
        let mut reverb = Reverb::new(rate, Space { seconds: 1.0, size: 1.0, damping: 0.2, gap: 0.01 });
        let mut out = [0.0; 2];
        let mut energy = [0.0f32; 30];
        for i in 0..rate as usize * 3 / 2 {
            reverb.next(if i == 0 { 1.0 } else { 0.0 }, &mut out);
            energy[i / (rate as usize / 20)] += out[0] * out[0];
        }
        // Sound after a tenth of a second, and down by a long way after the second it's to ring.
        assert!(energy[2] > 0.0);
        let db = 10.0 * (energy[22] / energy[2]).log10();
        assert!(db < -30.0 && db > -120.0, "{db} dB");
    }

    #[test]
    fn the_filters_pass_what_they_should() {
        let rate = 48_000.0;
        let rms = |mut f: Biquad, hz: f32| {
            let n = 24_000;
            (0..n).map(|i| f.next((2.0 * PI * hz * i as f32 / rate).sin())).skip(n / 2).map(|y| y * y).sum::<f32>().sqrt()
        };
        assert!(rms(Biquad::lowpass(rate, 500.0, 0.707), 5000.0) < rms(Biquad::lowpass(rate, 500.0, 0.707), 100.0) * 0.05);
        assert!(rms(Biquad::highpass(rate, 500.0, 0.707), 50.0) < rms(Biquad::highpass(rate, 500.0, 0.707), 5000.0) * 0.05);
        let band = Biquad::bandpass(rate, 1000.0, 2.0);
        assert!((rms(band, 1000.0) / rms(Biquad::peak(rate, 1000.0, 1.0, 0.0), 1000.0) - 1.0).abs() < 0.05);
    }
}
