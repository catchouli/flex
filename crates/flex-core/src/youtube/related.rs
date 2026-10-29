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
    Ok(parse(&json, video_id))
}

fn parse(json: &Value, video_id: &str) -> Vec<Listed> {
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
        out.push(Listed {
            id: id.to_owned(),
            title: meta["title"]["content"].as_str().unwrap_or_default().to_owned(),
            channel: channel.and_then(Value::as_str).unwrap_or_default().to_owned(),
            channel_id: channel_id.unwrap_or_default().to_owned(),
            duration,
            views: None,
            published: None,
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
                    { "metadataParts": [{ "text": { "content": "31K" } }, { "text": { "content": "2mo ago" } }] },
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
        let related = parse(&json, "aqz-KE-bpKQ");
        let ids: Vec<&str> = related.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["583N5TMBeVw", "-tbSCMbJA6o"]);
        assert_eq!(related[0].title, "The Ultimate Blender Quickstart Guide");
        assert_eq!(related[0].channel, "zac in 3D");
        assert_eq!(related[0].channel_id, "UC44unSnzdTRE8gpL5nOFflw");
        assert_eq!(related[0].duration, Some(36.0 * 60.0 + 31.0));
        assert_eq!(related[1].duration, Some(3723.0));
    }

    #[test]
    fn survives_a_different_layout() {
        assert!(parse(&json!({ "contents": {} }), "aqz-KE-bpKQ").is_empty());
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
