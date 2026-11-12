//! Old sound: an old radio, a gramophone, a walkie-talkie, worn tape, or 8-bit.

use std::f32::consts::PI;

use crate::dsp::{Biquad, Line, Rng, follow};
use crate::{Scene, Settings, SoundEffect, Stems};

/// The styles, by id: what the "style" setting's options are.
pub const STYLES: [&str; 5] = ["radio", "gramophone", "walkie", "tape", "bits"];

/// One channel's filters and state; mono styles use only the first.
struct Channel {
    filters: Vec<Biquad>,
    line: Line,
    /// The level the radio's squashing follows.
    level: f32,
    /// For 8-bit: the sample held, and how far to the next.
    held: f32,
    hold: f32,
}

/// "style" picks the style, "amount" how much of it.
pub struct OldSound {
    rate: f32,
    style: String,
    channels: Vec<Channel>,
    rng: Rng,
    t: f32,
    /// A gramophone's click ringing on, and the hiss's colour.
    click: f32,
    hiss: Biquad,
    /// The walkie-talkie: how open it is, whether it was, and the squelch when it closes.
    open: f32,
    was_open: bool,
    squelch: f32,
    mono: Vec<f32>,
}

impl OldSound {
    pub fn new(rate: u32) -> Self {
        Self { rate: rate as f32, style: String::new(), channels: Vec::new(), rng: Rng::new(0x01d), t: 0.0, click: 0.0, hiss: Biquad::default(), open: 0.0, was_open: false, squelch: 0.0, mono: Vec::new() }
    }

    fn filters(&self, style: &str) -> Vec<Biquad> {
        let r = self.rate;
        match style {
            "radio" => vec![Biquad::highpass(r, 300.0, 0.7), Biquad::highpass(r, 300.0, 0.7), Biquad::lowpass(r, 3500.0, 0.7), Biquad::lowpass(r, 3500.0, 0.7), Biquad::peak(r, 1200.0, 1.0, 4.0)],
            "gramophone" => vec![Biquad::highpass(r, 250.0, 0.7), Biquad::lowpass(r, 4000.0, 0.7), Biquad::lowpass(r, 4500.0, 0.7), Biquad::peak(r, 1500.0, 1.5, 7.0), Biquad::peak(r, 700.0, 2.0, 3.0)],
            "walkie" => vec![Biquad::highpass(r, 500.0, 0.7), Biquad::highpass(r, 500.0, 0.7), Biquad::lowpass(r, 2600.0, 0.7), Biquad::lowpass(r, 2600.0, 0.7)],
            "tape" => vec![Biquad::lowpass(r, 7000.0, 0.7), Biquad::peak(r, 90.0, 0.9, 2.5)],
            _ => Vec::new(),
        }
    }
}

impl SoundEffect for OldSound {
    fn process(&mut self, stems: &mut Stems, settings: &Settings, _: &Scene) {
        let style = settings.choice("style").unwrap_or("radio");
        let amount = settings.number("amount").map_or(1.0, |a| a / 100.0).clamp(0.0, 1.0);
        let rate = self.rate;
        let frames = stems.voice.len();
        if style != self.style {
            self.style = style.to_string();
            self.channels.clear();
            self.hiss = Biquad::bandpass(rate, if style == "gramophone" { 3000.0 } else { 5000.0 }, 0.5);
        }
        let start = self.t;
        self.t += frames as f32 / rate;
        let mono = matches!(style, "radio" | "gramophone" | "walkie");
        let count = if mono { 1 } else { stems.background.len() };
        while self.channels.len() < count {
            let filters = self.filters(style);
            self.channels.push(Channel { filters, line: Line::new((0.01 * rate) as usize), level: 0.0, held: 0.0, hold: 0.0 });
        }
        if mono {
            stems.mono(&mut self.mono);
            // The walkie-talkie's only open while someone's talking, and hisses as it closes.
            let open = stems.speech > 0.5;
            if self.was_open && !open && style == "walkie" {
                self.squelch = 0.15;
            }
            self.was_open = open;
            let attack = follow(0.01, rate);
            let release = follow(0.25, rate);
            let ch = &mut self.channels[0];
            for (i, x) in self.mono.iter_mut().enumerate() {
                let dry = *x;
                let mut y = match style {
                    "gramophone" => {
                        // Wow: the record not quite round.
                        let t = start + i as f32 / rate;
                        ch.line.push(dry);
                        ch.line.at((0.0012 + 0.0006 * (2.0 * PI * 0.55 * t).sin()) * rate)
                    }
                    _ => dry,
                };
                y = ch.filters.iter_mut().fold(y, |y, f| f.next(y));
                // Squashed, so quiet and loud come out much the same.
                ch.level += (y.abs() - ch.level) * if y.abs() > ch.level { attack } else { release };
                let squash = (0.3 / (ch.level + 0.005)).min(20.0);
                y = match style {
                    "walkie" => (y * squash * 3.0).tanh() * 0.06,
                    "radio" => (y * squash * 1.2).tanh() * 0.12,
                    _ => (y * 1.4).tanh() * 0.8,
                };
                let hiss = self.hiss.next(self.rng.noise());
                match style {
                    "gramophone" => {
                        // Crackle: clicks now and then, a few loud ones, ringing a moment.
                        if self.rng.unit() < 40.0 / rate {
                            self.click += self.rng.unit().powi(4) * 0.12 * if self.rng.unit() < 0.5 { 1.0 } else { -1.0 };
                        }
                        y += self.click + hiss * 0.012;
                        self.click *= 0.6;
                    }
                    "walkie" => {
                        self.open += ((if open { 1.0 } else { 0.0 }) - self.open) * if open { attack } else { follow(0.05, rate) };
                        y = y * self.open + hiss * (0.004 + self.squelch);
                        self.squelch = (self.squelch - 1.0 / rate).max(0.0);
                    }
                    _ => y += hiss * 0.006,
                }
                *x = y;
            }
            for (v, (e, w)) in stems.voice.iter_mut().zip(stems.echo.iter_mut().zip(&self.mono)) {
                *v = (*v + *e) * (1.0 - amount) + w * amount;
                *e = 0.0;
            }
            for c in &mut stems.background {
                c.iter_mut().for_each(|x| *x *= 1.0 - amount);
            }
        } else {
            stems.mix_down();
            let step = 11_025.0 / rate;
            for (c, ch) in stems.background.iter_mut().zip(&mut self.channels) {
                for (i, x) in c.iter_mut().enumerate() {
                    let dry = *x;
                    let y = if style == "tape" {
                        // Wow and flutter: the tape not running quite steadily.
                        let t = start + i as f32 / rate;
                        let wow = 0.003 + 0.0010 * (2.0 * PI * 0.7 * t).sin() + 0.00012 * (2.0 * PI * 9.0 * t).sin();
                        ch.line.push(dry);
                        let y = ch.filters.iter_mut().fold(ch.line.at(wow * rate), |y, f| f.next(y));
                        (y * 1.3).tanh() / 1.3f32.tanh() + self.hiss.next(self.rng.noise()) * 0.004
                    } else {
                        // 8-bit: held at 11 kHz, in 32 steps, brought up to a steady level first
                        // so quiet sound still fills them.
                        ch.level += (dry.abs() - ch.level) * if dry.abs() > ch.level { 0.01 } else { 0.0001 };
                        let gain = 0.3 / (ch.level + 0.02);
                        ch.hold += step;
                        if ch.hold >= 1.0 {
                            ch.hold -= 1.0;
                            ch.held = ((dry * gain).clamp(-1.0, 1.0) * 16.0).round() / 16.0 / gain;
                        }
                        ch.held
                    };
                    *x = dry + (y - dry) * amount;
                }
            }
        }
    }
}
