//! SponsorBlock (sponsor.ajay.app): community-marked sponsor reads, "like and subscribe"
//! reminders and so on, skipped during playback.
//!
//! Videos are looked up by the first 4 characters of their id's SHA-256, so the server
//! doesn't learn which video is being watched (the list for that prefix is filtered here).

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const API: &str = "https://sponsor.ajay.app/api/skipSegments";
const USER_AGENT: &str = concat!("flex/", env!("CARGO_PKG_VERSION"));

/// SponsorBlock's categories: (id, name shown in Settings, skipped by default).
pub const CATEGORIES: [(&str, &str, bool); 8] = [
    ("sponsor", "Sponsors", true),
    ("selfpromo", "Self-promotion", true),
    ("interaction", "Like/subscribe reminders", true),
    ("intro", "Intros", false),
    ("outro", "End cards and credits", false),
    ("preview", "Previews and recaps", false),
    ("filler", "Filler tangents", false),
    ("music_offtopic", "Non-music parts of music videos", false),
];

/// The categories skipped unless the user changes them.
pub fn default_categories() -> Vec<String> {
    CATEGORIES.iter().filter(|c| c.2).map(|c| c.0.to_owned()).collect()
}

/// A category's colour on the seek bar (SponsorBlock's own defaults).
pub fn category_colour(id: &str) -> [u8; 3] {
    match id {
        "sponsor" => [0x00, 0xd4, 0x00],
        "selfpromo" => [0xff, 0xff, 0x00],
        "interaction" => [0xcc, 0x00, 0xff],
        "intro" => [0x00, 0xff, 0xff],
        "outro" => [0x02, 0x02, 0xed],
        "preview" => [0x00, 0x8f, 0xd6],
        "filler" => [0x73, 0x00, 0xff],
        "music_offtopic" => [0xff, 0x99, 0x00],
        _ => [0xcc, 0xcc, 0xcc],
    }
}

/// The name of a category, e.g. "Sponsors".
pub fn category_name(id: &str) -> &str {
    CATEGORIES.iter().find(|c| c.0 == id).map_or(id, |c| c.1)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub category: String,
}

#[derive(Deserialize)]
struct Video {
    #[serde(rename = "videoID")]
    video_id: String,
    segments: Vec<RawSegment>,
}

#[derive(Deserialize)]
struct RawSegment {
    segment: [f64; 2],
    category: String,
    #[serde(rename = "actionType", default)]
    action_type: String,
}

/// Segments to skip in `video_id`, from `categories`, in order.
pub fn segments(video_id: &str, categories: &[String]) -> Result<Vec<Segment>> {
    if categories.is_empty() {
        return Ok(Vec::new());
    }
    let hash = Sha256::digest(video_id.as_bytes());
    let prefix: String = hash.iter().take(2).map(|b| format!("{b:02x}")).collect();
    let categories = serde_json::to_string(categories)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .get(&format!("{API}/{prefix}"))
        .query("categories", &categories)
        .query("actionTypes", r#"["skip"]"#)
        .header("User-Agent", USER_AGENT)
        .call()
        .context("asking SponsorBlock")?;
    // 404: nothing marked for any video with this prefix.
    if response.status() == 404 {
        return Ok(Vec::new());
    }
    anyhow::ensure!(response.status().is_success(), "SponsorBlock answered {}", response.status());
    let body = response.body_mut().read_to_string().context("reading SponsorBlock's answer")?;
    parse(&body, video_id)
}

fn parse(json: &str, video_id: &str) -> Result<Vec<Segment>> {
    let videos: Vec<Video> = serde_json::from_str(json).context("reading SponsorBlock's answer")?;
    let mut segments: Vec<Segment> = videos
        .into_iter()
        .filter(|v| v.video_id == video_id)
        .flat_map(|v| v.segments)
        .filter(|s| s.action_type.is_empty() || s.action_type == "skip")
        .filter(|s| s.segment[1] > s.segment[0])
        .map(|s| Segment { start: s.segment[0], end: s.segment[1], category: s.category })
        .collect();
    segments.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(segments)
}

/// The segment playback just ran into, going from `before` to `now` seconds: skip to its
/// end. Seeking into the middle of a segment on purpose doesn't count (`before` is far
/// from it), so it can still be watched.
pub fn entered(segments: &[Segment], before: f64, now: f64) -> Option<&Segment> {
    // Playing normally moves a fraction of a second per check; a seek jumps further.
    const PLAYING_STEP: f64 = 1.5;
    if !(0.0..=PLAYING_STEP).contains(&(now - before)) {
        return None;
    }
    segments.iter().find(|s| before <= s.start + 0.05 && now >= s.start && now < s.end - 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_this_videos_skips() {
        let json = r#"[
            {"videoID":"other","segments":[{"segment":[1,2],"category":"sponsor","actionType":"skip"}]},
            {"videoID":"aqz-KE-bpKQ","segments":[
                {"segment":[60.5,90],"category":"sponsor","actionType":"skip"},
                {"segment":[5,10],"category":"selfpromo","actionType":"skip"},
                {"segment":[100,110],"category":"sponsor","actionType":"mute"}]}]"#;
        let segs = parse(json, "aqz-KE-bpKQ").unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!((segs[0].start, segs[0].category.as_str()), (5.0, "selfpromo"), "sorted");
    }

    #[test]
    fn skips_when_playback_runs_in_not_when_seeking_in() {
        let segs = vec![Segment { start: 60.0, end: 90.0, category: "sponsor".into() }];
        assert!(entered(&segs, 59.8, 60.1).is_some(), "played into it");
        assert!(entered(&segs, 10.0, 70.0).is_none(), "seeked into the middle");
        assert!(entered(&segs, 70.0, 70.2).is_none(), "already inside, watching on purpose");
        assert!(entered(&segs, 89.7, 89.8).is_none(), "nearly over");
        assert!(entered(&segs, 50.0, 50.2).is_none());
    }

    #[test]
    fn default_categories_are_the_annoying_ones() {
        assert_eq!(default_categories(), ["sponsor", "selfpromo", "interaction"]);
        assert_eq!(category_name("interaction"), "Like/subscribe reminders");
        // Every category has its own colour.
        let colours: std::collections::HashSet<[u8; 3]> = CATEGORIES.iter().map(|c| category_colour(c.0)).collect();
        assert_eq!(colours.len(), CATEGORIES.len());
    }

    /// Real network: `cargo test -p flex-core sponsorblock_online -- --ignored --nocapture`.
    /// aQp-iM0bCs4 had a sponsor segment marked at the start (October 2026).
    #[test]
    #[ignore]
    fn sponsorblock_online() {
        let segs = segments("aQp-iM0bCs4", &default_categories()).unwrap();
        eprintln!("{segs:?}");
        assert!(segs.iter().any(|s| s.category == "sponsor"));
        // A video with nothing marked in these categories (others share its prefix).
        assert!(segments("dQw4w9WgXcQ", &default_categories()).unwrap().is_empty());
    }
}
