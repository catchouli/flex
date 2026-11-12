//! Splitting the sound into the voice and everything else. For now from where films put
//! dialogue: the centre channel when there is one, otherwise the middle of the stereo picture,
//! until a network does it properly. Either way, voice and background add back up to exactly
//! the sound that came in.

use super::Stems;

/// Where the voice is in the sound's channels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Split {
    /// What's the same in the first two channels (front left and right), or all of a single one.
    #[default]
    Middle,
    /// The channel with this number (a film's centre channel).
    Channel(usize),
}

impl Split {
    /// Where the voice is in sound with mpv's channel layout `layout` (`audio-params/channels`)
    /// of `channels` channels: libavfilter puts the centre third whenever there is one.
    pub fn for_layout(layout: &str, channels: usize) -> Self {
        const NO_CENTRE: [&str; 6] = ["mono", "stereo", "2.1", "quad", "quad(side)", "downmix"];
        if channels >= 3 && !NO_CENTRE.contains(&layout) && !layout.starts_with("2.") { Self::Channel(2) } else { Self::Middle }
    }

    /// Splits `channels` blocks of `frames` frames into `stems`, and says how much of the voice
    /// goes back into each channel.
    pub fn split(&self, samples: &[f32], channels: usize, frames: usize, stems: &mut Stems) -> Vec<f32> {
        stems.background.resize_with(channels, Vec::new);
        for (b, c) in stems.background.iter_mut().zip(samples.chunks(frames)) {
            b.clear();
            b.extend_from_slice(c);
        }
        stems.voice.clear();
        stems.voice.resize(frames, 0.0);
        let mut weights = vec![0.0; channels];
        match *self {
            Self::Channel(c) if c < channels => {
                std::mem::swap(&mut stems.voice, &mut stems.background[c]);
                stems.background[c].clear();
                stems.background[c].resize(frames, 0.0);
                weights[c] = 1.0;
            }
            _ if channels >= 2 => {
                let (left, rest) = stems.background.split_at_mut(1);
                for ((v, l), r) in stems.voice.iter_mut().zip(left[0].iter_mut()).zip(rest[0].iter_mut()) {
                    *v = (*l + *r) / 2.0;
                    *l -= *v;
                    *r -= *v;
                }
                weights[0] = 1.0;
                weights[1] = 1.0;
            }
            _ => {
                if let Some(only) = stems.background.first_mut() {
                    std::mem::swap(&mut stems.voice, only);
                    only.clear();
                    only.resize(frames, 0.0);
                    weights[0] = 1.0;
                }
            }
        }
        weights
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(split: Split, channels: usize) {
        let frames = 64;
        let samples: Vec<f32> = (0..channels * frames).map(|i| ((i * 37) % 11) as f32 - 5.0).collect();
        let mut stems = Stems::default();
        let weights = split.split(&samples, channels, frames, &mut stems);
        for c in 0..channels {
            for i in 0..frames {
                let back = stems.background[c][i] + weights[c] * stems.voice[i];
                assert!((back - samples[c * frames + i]).abs() < 1e-5, "{split:?} {channels} channels: channel {c} at {i}");
            }
        }
    }

    #[test]
    fn voice_and_background_add_back_up_to_the_sound() {
        for channels in 1..=8 {
            round_trip(Split::Middle, channels);
            round_trip(Split::Channel(2), channels);
        }
    }

    #[test]
    fn the_centre_is_found_where_there_is_one() {
        assert_eq!(Split::for_layout("5.1(side)", 6), Split::Channel(2));
        assert_eq!(Split::for_layout("7.1", 8), Split::Channel(2));
        assert_eq!(Split::for_layout("stereo", 2), Split::Middle);
        assert_eq!(Split::for_layout("quad", 4), Split::Middle);
        assert_eq!(Split::for_layout("2.1", 3), Split::Middle);
    }
}
