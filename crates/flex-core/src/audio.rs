//! A file's audio tracks, described the way a person would: "Dolby TrueHD Atmos 7.1",
//! "AAC stereo". Built from what mpv (or ffprobe) says: the ffmpeg codec name, its profile
//! ("DTS-HD MA", "Dolby TrueHD + Dolby Atmos") and the channel count or layout.

use serde::{Deserialize, Serialize};

/// One audio track, as stored in the `media_audio` table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTrack {
    /// Language tag, e.g. "jpn" or "en" ('' when missing).
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub title: String,
    /// ffmpeg's codec name: "truehd", "eac3", "dts", "aac"…
    pub codec: String,
    /// ffmpeg's profile name, e.g. "DTS-HD MA" ('' when none).
    #[serde(default)]
    pub profile: String,
    /// 0 when unknown.
    #[serde(default)]
    pub channels: u32,
    /// Channel layout, e.g. "5.1(side)" ('' when unknown).
    #[serde(default)]
    pub layout: String,
    #[serde(default)]
    pub default: bool,
}

impl AudioTrack {
    /// "Dolby TrueHD Atmos 7.1".
    pub fn format(&self) -> String {
        let name = format_name(&self.codec, &self.profile);
        match channels_label(self.channels, &self.layout) {
            Some(ch) => format!("{name} {ch}"),
            None => name,
        }
    }

    /// "English · Dolby TrueHD Atmos 7.1", with "(commentary)" for commentaries.
    pub fn describe(&self) -> String {
        let mut text = match crate::language::name(&self.lang) {
            Some(lang) => format!("{lang} · {}", self.format()),
            None => self.format(),
        };
        if self.title.to_lowercase().contains("commentary") {
            text += " (commentary)";
        }
        text
    }
}

/// The codec's everyday name, with the profile where it matters (DTS-HD, Atmos).
pub fn format_name(codec: &str, profile: &str) -> String {
    let codec = codec.to_lowercase();
    let profile = profile.to_lowercase();
    let atmos = profile.contains("atmos");
    let name = match codec.as_str() {
        "truehd" if atmos => "Dolby TrueHD Atmos",
        "truehd" => "Dolby TrueHD",
        "mlp" => "MLP",
        "eac3" if atmos => "Dolby Digital Plus Atmos",
        "eac3" => "Dolby Digital Plus",
        "ac3" => "Dolby Digital",
        "ac4" => "Dolby AC-4",
        // FFmpeg's profiles: "DTS", "DTS-ES", "DTS 96/24", "DTS-HD HRA", "DTS-HD MA",
        // "DTS-HD MA + DTS:X", "DTS-HD MA + DTS:X IMAX", "DTS Express".
        "dts" if profile.contains("dts:x") => "DTS:X",
        "dts" if profile.contains("hd ma") => "DTS-HD MA",
        "dts" if profile.contains("hra") => "DTS-HD HRA",
        "dts" if profile.contains("express") => "DTS Express",
        "dts" if profile.contains("dts-es") => "DTS-ES",
        "dts" => "DTS",
        "aac" | "aac_latm" if profile.starts_with("he") => "HE-AAC",
        "aac" | "aac_latm" => "AAC",
        "flac" => "FLAC",
        "opus" => "Opus",
        "vorbis" => "Vorbis",
        "mp3" | "mp3float" => "MP3",
        "mp2" | "mp2float" => "MP2",
        "alac" => "ALAC",
        "wmav1" | "wmav2" | "wmapro" | "wmalossless" => "WMA",
        c if c.starts_with("pcm_") => "PCM",
        "" => "Unknown",
        c => return c.to_uppercase(),
    };
    name.into()
}

/// "5.1" from a layout like "5.1(side)", else from the channel count.
pub fn channels_label(channels: u32, layout: &str) -> Option<String> {
    let layout = layout.trim().to_lowercase();
    match layout.as_str() {
        "mono" => return Some("mono".into()),
        "stereo" | "downmix" => return Some("stereo".into()),
        "quad" | "quad(side)" => return Some("4.0".into()),
        _ => {}
    }
    // "5.1(side)", "7.1(wide)", "2.1"
    let numeric = layout.split('(').next().unwrap_or("");
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if numeric
        .split_once('.')
        .is_some_and(|(front, lfe)| digits(front) && digits(lfe))
    {
        return Some(numeric.to_owned());
    }
    Some(match channels {
        0 => return None,
        1 => "mono".into(),
        2 => "stereo".into(),
        3 => "2.1".into(),
        4 => "4.0".into(),
        5 => "5.0".into(),
        6 => "5.1".into(),
        7 => "6.1".into(),
        8 => "7.1".into(),
        n => format!("{n} ch"),
    })
}

/// One line for a details page: each track described, tracks that read the same once
/// with a count ("×2"), at most `max` of them then "+N more".
pub fn summary(tracks: &[AudioTrack], max: usize) -> String {
    let mut parts: Vec<(String, usize)> = Vec::new();
    for t in tracks {
        let d = t.describe();
        match parts.iter_mut().find(|(p, _)| *p == d) {
            Some((_, n)) => *n += 1,
            None => parts.push((d, 1)),
        }
    }
    let more = parts.len().saturating_sub(max);
    let mut out: Vec<String> = parts
        .into_iter()
        .take(max)
        .map(|(d, n)| if n > 1 { format!("{d} ×{n}") } else { d })
        .collect();
    if more > 0 {
        out.push(format!("+{more} more"));
    }
    out.join("   ·   ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(codec: &str, profile: &str, channels: u32, layout: &str) -> AudioTrack {
        AudioTrack {
            codec: codec.into(),
            profile: profile.into(),
            channels,
            layout: layout.into(),
            ..Default::default()
        }
    }

    #[test]
    fn names_formats() {
        assert_eq!(
            track("truehd", "Dolby TrueHD + Dolby Atmos", 8, "7.1").format(),
            "Dolby TrueHD Atmos 7.1"
        );
        assert_eq!(
            track("truehd", "", 6, "5.1(side)").format(),
            "Dolby TrueHD 5.1"
        );
        assert_eq!(
            track("eac3", "Dolby Digital Plus + Dolby Atmos", 6, "5.1(side)").format(),
            "Dolby Digital Plus Atmos 5.1"
        );
        assert_eq!(track("ac3", "", 6, "").format(), "Dolby Digital 5.1");
        assert_eq!(
            track("dts", "DTS-HD MA", 8, "7.1").format(),
            "DTS-HD MA 7.1"
        );
        assert_eq!(
            track("dts", "DTS-HD MA + DTS:X", 8, "7.1").format(),
            "DTS:X 7.1"
        );
        assert_eq!(track("dts", "DTS-ES", 7, "6.1").format(), "DTS-ES 6.1");
        assert_eq!(track("dts", "DTS", 6, "5.1(side)").format(), "DTS 5.1");
        assert_eq!(track("aac", "LC", 2, "stereo").format(), "AAC stereo");
        assert_eq!(track("aac", "HE-AACv2", 2, "").format(), "HE-AAC stereo");
        assert_eq!(track("flac", "", 1, "mono").format(), "FLAC mono");
        assert_eq!(track("pcm_s24le", "", 2, "").format(), "PCM stereo");
        assert_eq!(track("opus", "", 0, "").format(), "Opus");
        assert_eq!(track("weird", "", 10, "").format(), "WEIRD 10 ch");
    }

    #[test]
    fn describes_and_summarises() {
        let en = AudioTrack {
            lang: "eng".into(),
            ..track("truehd", "", 8, "7.1")
        };
        let ja = AudioTrack {
            lang: "jpn".into(),
            ..track("aac", "", 2, "")
        };
        let commentary = AudioTrack {
            lang: "en".into(),
            title: "Director's Commentary".into(),
            ..track("ac3", "", 2, "")
        };
        assert_eq!(en.describe(), "English · Dolby TrueHD 7.1");
        assert_eq!(
            commentary.describe(),
            "English · Dolby Digital stereo (commentary)"
        );
        assert_eq!(
            summary(&[en.clone(), ja.clone(), ja.clone()], 4),
            "English · Dolby TrueHD 7.1   ·   Japanese · AAC stereo ×2"
        );
        assert_eq!(
            summary(&[en, ja, commentary], 1),
            "English · Dolby TrueHD 7.1   ·   +2 more"
        );
    }
}
