//! A video's related videos, the "up next" list beside it on YouTube. yt-dlp doesn't list
//! these, so they come from the endpoint YouTube's own web page uses. Its layout changes
//! now and then; everything here is optional, so a change means fewer or no results
//! rather than wrong ones.

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::feeds::Listed;

const URL: &str = "https://www.youtube.com/youtubei/v1/next?prettyPrint=false";
/// The web client YouTube's page identifies as. Old versions keep working for years.
const CLIENT_VERSION: &str = "2.20251001.00.00";

/// Videos YouTube suggests after `video_id`.
pub fn related(video_id: &str) -> Result<Vec<Listed>> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).build().into();
    let body = json!({
        "context": { "client": { "clientName": "WEB", "clientVersion": CLIENT_VERSION, "hl": "en" } },
        "videoId": video_id,
    });
    let json: Value = agent
        .post(URL)
        .send_json(&body)
        .context("asking YouTube for related videos")?
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_json()
        .context("reading YouTube's related videos")?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    Ok(parse(&json, video_id, now))
}

/// The videos in YouTube's response; `now` (Unix time) turns "2d ago" into a date.
fn parse(json: &Value, video_id: &str, now: i64) -> Vec<Listed> {
    let results = json.pointer("/contents/twoColumnWatchNextResults/secondaryResults/secondaryResults/results");
    let mut out: Vec<Listed> = Vec::new();
    for item in results.and_then(Value::as_array).into_iter().flatten() {
        let Some(lockup) = item.get("lockupViewModel") else { continue };
        if lockup["contentType"] != "LOCKUP_CONTENT_TYPE_VIDEO" {
            continue; // playlists, courses, mixes
        }
        let Some(id) = lockup["contentId"].as_str().filter(|id| super::video_id(id).is_some() && *id != video_id) else { continue };
        if out.iter().any(|l| l.id == id) {
            continue;
        }
        let meta = &lockup["metadata"]["lockupMetadataViewModel"];
        let channel = meta.pointer("/metadata/contentMetadataViewModel/metadataRows/0/metadataParts/0/text/content");
        let channel_id = find_str(&meta["image"], "browseId").filter(|b| b.starts_with("UC"));
        let badge = find(&lockup["contentImage"], "thumbnailBadgeViewModel");
        let duration = badge.and_then(|b| b["text"].as_str()).and_then(parse_duration);
        // The second row: views, then how long ago ("131K", "13h ago").
        let second: Vec<&str> = meta
            .pointer("/metadata/contentMetadataViewModel/metadataRows/1/metadataParts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|p| p.pointer("/text/content").and_then(Value::as_str))
            .collect();
        let views = second.iter().find_map(|t| parse_views(t));
        let published = second.iter().find_map(|t| parse_ago(t)).map(|secs| now - secs);
        out.push(Listed {
            id: id.to_owned(),
            title: meta["title"]["content"].as_str().unwrap_or_default().to_owned(),
            channel: channel.and_then(Value::as_str).unwrap_or_default().to_owned(),
            channel_id: channel_id.unwrap_or_default().to_owned(),
            duration,
            views,
            published,
        });
    }
    out
}

/// The first value under `key` anywhere in `value`.
fn find<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map.get(key).or_else(|| map.values().find_map(|v| find(v, key))),
        Value::Array(items) => items.iter().find_map(|v| find(v, key)),
        _ => None,
    }
}

fn find_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    find(value, key)?.as_str()
}

/// "950", "131K", "1.1M views" as a number (not "13h ago").
fn parse_views(s: &str) -> Option<u64> {
    let s = s.trim().trim_end_matches(" views").trim_end_matches(" view");
    let (number, scale) = match s.char_indices().last()? {
        (i, 'K') => (&s[..i], 1e3),
        (i, 'M') => (&s[..i], 1e6),
        (i, 'B') => (&s[..i], 1e9),
        _ => (s, 1.0),
    };
    let n: f64 = number.replace(',', "").parse().ok()?;
    Some((n * scale).round() as u64)
}

/// "13h ago", "2mo ago", "Streamed 3 years ago" as roughly how many seconds ago.
fn parse_ago(s: &str) -> Option<i64> {
    let s = s.trim().strip_suffix(" ago")?;
    // The number and unit at the end: "13h", "3 years".
    let digits_end = s.rfind(|c: char| c.is_ascii_digit())? + 1;
    let digits_start = s[..digits_end].rfind(|c: char| !c.is_ascii_digit()).map_or(0, |i| i + 1);
    let n: i64 = s[digits_start..digits_end].parse().ok()?;
    let unit = s[digits_end..].trim();
    let secs = match unit.trim_end_matches('s') {
        "" | "sec" | "second" => 1,
        "m" | "min" | "minute" => 60,
        "h" | "hr" | "hour" => 3600,
        "d" | "day" => 86_400,
        "w" | "wk" | "week" => 7 * 86_400,
        "mo" | "month" => 30 * 86_400,
        "y" | "yr" | "year" => 365 * 86_400,
        _ => return None,
    };
    Some(n * secs)
}

/// "12:45" or "1:02:03" in seconds.
fn parse_duration(s: &str) -> Option<f64> {
    let mut secs = 0.0;
    for part in s.trim().split(':') {
        secs = secs * 60.0 + part.parse::<u32>().ok()? as f64;
    }
    Some(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lockup(id: &str, kind: &str, title: &str, channel: &str, channel_id: &str, length: &str) -> Value {
        json!({ "lockupViewModel": {
            "contentId": id,
            "contentType": kind,
            "contentImage": { "thumbnailViewModel": { "overlays": [{ "thumbnailBottomOverlayViewModel": {
                "badges": [{ "thumbnailBadgeViewModel": { "text": length } }] } }] } },
            "metadata": { "lockupMetadataViewModel": {
                "title": { "content": title },
                "image": { "decoratedAvatarViewModel": { "rendererContext": { "commandContext": { "onTap": {
                    "innertubeCommand": { "browseEndpoint": { "browseId": channel_id } } } } } } },
                "metadata": { "contentMetadataViewModel": { "metadataRows": [
                    { "metadataParts": [{ "text": { "content": channel } }] },
                    { "metadataParts": [{ "text": { "content": "31K" } }, { "text": { "content": "2d ago" } }] },
                ] } },
            } },
        } })
    }

    #[test]
    fn reads_related_videos() {
        let video = "LOCKUP_CONTENT_TYPE_VIDEO";
        let results = json!([
            lockup("583N5TMBeVw", video, "The Ultimate Blender Quickstart Guide", "zac in 3D", "UC44unSnzdTRE8gpL5nOFflw", "36:31"),
            lockup("PLjEaoINr3zgGUwGwXlj9kBe7TrVWNjkyv", "LOCKUP_CONTENT_TYPE_COURSE", "A course", "", "", ""),
            { "reelShelfRenderer": {} },
            lockup("aqz-KE-bpKQ", video, "The video itself", "Blender", "UCSMOQeBJ2RAnuFungnQOxLg", "10:35"),
            lockup("-tbSCMbJA6o", video, "Blender Donut Tutorial", "Blender Guru", "UCOKHwx1VCdgnxwbjyb9Iu1g", "1:02:03"),
            { "continuationItemRenderer": {} },
        ]);
        let json = json!({ "contents": { "twoColumnWatchNextResults": { "secondaryResults": { "secondaryResults": { "results": results } } } } });
        let related = parse(&json, "aqz-KE-bpKQ", 1_000_000);
        let ids: Vec<&str> = related.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["583N5TMBeVw", "-tbSCMbJA6o"]);
        assert_eq!(related[0].title, "The Ultimate Blender Quickstart Guide");
        assert_eq!(related[0].channel, "zac in 3D");
        assert_eq!(related[0].channel_id, "UC44unSnzdTRE8gpL5nOFflw");
        assert_eq!(related[0].duration, Some(36.0 * 60.0 + 31.0));
        assert_eq!(related[1].duration, Some(3723.0));
        assert_eq!((related[0].views, related[0].published), (Some(31_000), Some(1_000_000 - 2 * 86_400)));
    }

    #[test]
    fn reads_views_and_ages() {
        assert_eq!(parse_views("131K"), Some(131_000));
        assert_eq!(parse_views("1.1M views"), Some(1_100_000));
        assert_eq!(parse_views("950"), Some(950));
        assert_eq!(parse_views("13h ago"), None);
        assert_eq!(parse_ago("13h ago"), Some(13 * 3600));
        assert_eq!(parse_ago("2mo ago"), Some(60 * 86_400));
        assert_eq!(parse_ago("Streamed 3 years ago"), Some(3 * 365 * 86_400));
        assert_eq!(parse_ago("1 day ago"), Some(86_400));
        assert_eq!(parse_ago("131K"), None);
    }

    #[test]
    fn survives_a_different_layout() {
        assert!(parse(&json!({ "contents": {} }), "aqz-KE-bpKQ", 0).is_empty());
    }

    /// Real network: `cargo test -p flex-core related_online -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn related_online() {
        let related = related("aqz-KE-bpKQ").unwrap();
        eprintln!("{:#?}", related.iter().take(5).collect::<Vec<_>>());
        assert!(related.len() >= 5);
        assert!(related.iter().all(|l| !l.title.is_empty() && !l.channel.is_empty()));
    }
}
