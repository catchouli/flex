//! Taking sound at 48 kHz down to 16 kHz and back, for the networks that hear and play at that.

/// A third of the rate.
pub const FACTOR: usize = 3;
/// How late the way down and back up again is, in samples at 48 kHz.
#[cfg(test)]
const ROUND_TRIP: usize = TAPS - 3;
const TAPS: usize = 48;

/// Low-pass taps: a windowed sinc cut off just under 8 kHz, at 48 kHz.
fn taps() -> Vec<f32> {
    let cut = 7_200.0 / 48_000.0;
    let taps: Vec<f64> = (0..TAPS)
        .map(|i| {
            let t = i as f64 - (TAPS - 1) as f64 / 2.0;
            let sinc = if t == 0.0 { 2.0 * cut } else { (2.0 * std::f64::consts::PI * cut * t).sin() / (std::f64::consts::PI * t) };
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (TAPS - 1) as f64).cos();
            sinc * w
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    taps.iter().map(|t| (t / sum) as f32).collect()
}

/// 48 kHz in, 16 kHz out.
pub struct Down {
    taps: Vec<f32>,
    recent: Vec<f32>,
    phase: usize,
}

impl Default for Down {
    fn default() -> Self {
        Self { taps: taps(), recent: vec![0.0; TAPS], phase: 0 }
    }
}

impl Down {
    /// Takes a sample, and every third gives one back.
    pub fn push(&mut self, x: f32) -> Option<f32> {
        self.recent.rotate_left(1);
        self.recent[TAPS - 1] = x;
        self.phase += 1;
        if self.phase < FACTOR {
            return None;
        }
        self.phase = 0;
        Some(self.recent.iter().zip(&self.taps).map(|(a, b)| a * b).sum())
    }
}

/// 16 kHz in, 48 kHz out.
pub struct Up {
    taps: Vec<f32>,
    recent: Vec<f32>,
}

impl Default for Up {
    fn default() -> Self {
        Self { taps: taps().iter().map(|t| t * FACTOR as f32).collect(), recent: vec![0.0; TAPS] }
    }
}

impl Up {
    /// Takes a sample and gives back three.
    pub fn push(&mut self, x: f32, out: &mut Vec<f32>) {
        for k in 0..FACTOR {
            self.recent.rotate_left(1);
            self.recent[TAPS - 1] = if k == 0 { x } else { 0.0 };
            out.push(self.recent.iter().zip(&self.taps).map(|(a, b)| a * b).sum());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain(taps: &[f32], hz: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, t) in taps.iter().enumerate() {
            let p = 2.0 * std::f64::consts::PI * hz / 48_000.0 * i as f64;
            re += *t as f64 * p.cos();
            im += *t as f64 * p.sin();
        }
        (re * re + im * im).sqrt()
    }

    #[test]
    fn the_low_pass_keeps_speech_and_drops_what_would_fold_back() {
        let taps = taps();
        assert!((gain(&taps, 1_000.0) - 1.0).abs() < 0.02);
        assert!(gain(&taps, 10_000.0) < 0.01);
    }

    #[test]
    fn down_and_up_again_keeps_a_tone() {
        let (mut down, mut up) = (Down::default(), Up::default());
        let tone: Vec<f32> = (0..4800).map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 48_000.0).sin()).collect();
        let mut out = Vec::new();
        for &x in &tone {
            if let Some(y) = down.push(x) {
                up.push(y, &mut out);
            }
        }
        // Late by both filters' middles, less the two samples a third is taken early by.
        let late = ROUND_TRIP;
        let worst = (1000..4000).map(|i| (out[i] - tone[i - late]).abs()).fold(0.0, f32::max);
        assert!(worst < 0.03, "worst {worst}");
    }
}
