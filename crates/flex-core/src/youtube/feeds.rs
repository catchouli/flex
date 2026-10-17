//! Subscriptions without YouTube's API: each channel has an RSS (Atom) feed of its latest
//! ~15 videos, which needs no key and has hardly changed in years. Plus importing
//! subscription lists from Google Takeout, FreeTube, NewPipe or OPML, and search through
//! yt-dlp.

use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;

use super::Tools;

const USER_AGENT: &str = concat!("flex/", env!("CARGO_PKG_VERSION"));

/// A video from a channel's feed.
#[derive(Clone, Debug, PartialEq)]
pub struct FeedVideo {
    pub id: String,
    pub title: String,
    pub channel_id: String,
    pub channel: String,
    /// Unix time.
    pub published: i64,
    /// A YouTube Short (its link goes to /shorts/).
    pub short: bool,
}

/// A small 16:9 thumbnail (320×180) for a video; these addresses are stable.
pub fn thumbnail(video_id: &str) -> String {
    format!("https://i.ytimg.com/vi/{video_id}/mqdefault.jpg")
}

static CHANNEL_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"UC[A-Za-z0-9_-]{22}").expect("valid regex"));

/// A channel id ("UC" + 22 characters) in a URL or on its own.
pub fn channel_id(s: &str) -> Option<String> {
    CHANNEL_ID.find(s).map(|m| m.as_str().to_owned())
}

/// Fetches a channel's feed: its title and latest videos.
pub fn fetch_feed(channel_id: &str) -> Result<(String, Vec<FeedVideo>)> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).build().into();
    let url = format!("https://www.youtube.com/feeds/videos.xml?channel_id={channel_id}");
    // The feeds occasionally fail for no reason; one retry covers most of it.
    let mut last_error = None;
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2));
        }
        match agent.get(&url).header("User-Agent", USER_AGENT).call() {
            Ok(mut response) => {
                let xml = response.body_mut().read_to_string().context("reading the feed")?;
                return parse_feed(&xml, channel_id);
            }
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.expect("tried at least once")).with_context(|| format!("fetching the feed for {channel_id}"))
}

fn parse_feed(xml: &str, channel_id: &str) -> Result<(String, Vec<FeedVideo>)> {
    let doc = roxmltree::Document::parse(xml).context("reading the feed")?;
    let root = doc.root_element();
    let channel = text(root, "title");
    let videos = root
        .children()
        .filter(|c| c.tag_name().name() == "entry")
        .filter_map(|entry| {
            let id = text(entry, "videoId");
            if id.is_empty() {
                return None;
            }
            let link = entry
                .children()
                .find(|c| c.tag_name().name() == "link" && c.attribute("rel") == Some("alternate"))
                .and_then(|l| l.attribute("href"))
                .unwrap_or_default();
            let author = child(entry, "author").map(|a| text(a, "name")).unwrap_or_default();
            Some(FeedVideo {
                title: text(entry, "title"),
                channel_id: channel_id.to_owned(),
                channel: if author.is_empty() { channel.clone() } else { author },
                published: parse_time(&text(entry, "published")).unwrap_or(0),
                short: link.contains("/shorts/"),
                id,
            })
        })
        .collect();
    Ok((channel, videos))
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children().find(|c| c.tag_name().name() == name)
}

/// The trimmed text of `node`'s child `name`, or "".
fn text(node: roxmltree::Node, name: &str) -> String {
    child(node, name).and_then(|c| c.text()).unwrap_or_default().trim().to_owned()
}

/// "2026-10-01T12:30:00+00:00" (or with "Z") as Unix time.
fn parse_time(s: &str) -> Option<i64> {
    let num = |range: std::ops::Range<usize>| s.get(range)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut t = days * 86_400 + h * 3600 + mi * 60 + se;
    // A "+01:00" style offset.
    let tz = s.get(19..).unwrap_or("").trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let (Some(sign), Some(oh), Some(om)) = (tz.chars().next(), tz.get(1..3).and_then(|v| v.parse::<i64>().ok()), tz.get(4..6).and_then(|v| v.parse::<i64>().ok())) {
        let offset = oh * 3600 + om * 60;
        t -= if sign == '-' { -offset } else { offset };
    }
    Some(t)
}

/// Channels in an exported subscription list, as (id, title): Google Takeout's
/// subscriptions.csv, FreeTube's profiles/subscriptions export, NewPipe's JSON, or OPML.
pub fn parse_subscriptions(text: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut add = |id: Option<String>, title: &str| {
        if let Some(id) = id
            && !found.iter().any(|(f, _)| *f == id)
        {
            found.push((id, title.trim().to_owned()));
        }
    };
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    // JSON (NewPipe, FreeTube), possibly one object per line (FreeTube's .db files).
    let values: Vec<serde_json::Value> = match serde_json::from_str(trimmed) {
        Ok(v) => vec![v],
        Err(_) => trimmed.lines().filter_map(|l| serde_json::from_str(l.trim()).ok()).collect(),
    };
    for value in &values {
        let lists = match value {
            serde_json::Value::Array(items) => vec![items],
            other => other.get("subscriptions").and_then(|s| s.as_array()).into_iter().collect(),
        };
        for item in lists.into_iter().flatten() {
            let id = ["id", "url", "channelId"].iter().find_map(|k| item.get(*k)?.as_str()).and_then(channel_id);
            let title = ["name", "title"].iter().find_map(|k| item.get(*k)?.as_str()).unwrap_or_default();
            add(id, title);
        }
    }
    if !values.is_empty() {
        return found;
    }
    // OPML: <outline xmlUrl="...channel_id=UC..." title="..."/>.
    if trimmed.starts_with('<')
        && let Ok(doc) = roxmltree::Document::parse(trimmed)
    {
        for outline in doc.descendants().filter(|n| n.tag_name().name() == "outline") {
            let id = outline.attribute("xmlUrl").and_then(channel_id);
            add(id, outline.attribute("title").or(outline.attribute("text")).unwrap_or_default());
        }
        return found;
    }
    // Takeout CSV: "Channel Id,Channel Url,Channel Title".
    for line in trimmed.lines().skip_while(|l| !l.contains("UC") && l.to_ascii_lowercase().starts_with("channel")) {
        let fields = csv_fields(line);
        let id = fields.first().and_then(|f| channel_id(f));
        add(id, fields.get(2).map_or("", String::as_str));
    }
    found
}

/// One CSV line's fields, with "quoted, fields" and "" escapes.
fn csv_fields(line: &str) -> Vec<String> {
    let (mut fields, mut field, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            c => field.push(c),
        }
    }
    fields.push(field);
    fields
}

/// A search result or a channel's video, from yt-dlp's flat listings.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub channel_id: String,
    pub duration: Option<f64>,
}

#[derive(Deserialize)]
struct Flat {
    #[serde(default)]
    entries: Vec<FlatEntry>,
    channel_id: Option<String>,
    channel: Option<String>,
    uploader: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize)]
struct FlatEntry {
    id: Option<String>,
    title: Option<String>,
    channel: Option<String>,
    uploader: Option<String>,
    channel_id: Option<String>,
    duration: Option<f64>,
    ie_key: Option<String>,
}

fn listed(flat: Flat) -> Vec<Listed> {
    flat.entries
        .into_iter()
        .filter(|e| e.ie_key.as_deref().is_none_or(|k| k == "Youtube"))
        .filter_map(|e| {
            let id = e.id.filter(|id| super::video_id(id).is_some())?;
            Some(Listed {
                id,
                title: e.title.unwrap_or_default(),
                channel: e.channel.or(e.uploader).or_else(|| flat.channel.clone()).unwrap_or_default(),
                channel_id: e.channel_id.or_else(|| flat.channel_id.clone()).unwrap_or_default(),
                duration: e.duration,
            })
        })
        .collect()
}

impl Tools {
    fn flat(&self, target: &str, items: Option<usize>) -> Result<Flat> {
        let mut cmd = self.command();
        cmd.args(["--flat-playlist", "--dump-single-json"]);
        if let Some(n) = items {
            cmd.arg("--playlist-items").arg(format!("1:{n}"));
        }
        let out = cmd.arg(target).output().with_context(|| format!("running {}", self.yt_dlp.display()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let line = err.lines().rev().find(|l| l.starts_with("ERROR")).unwrap_or(err.trim());
            bail!("yt-dlp: {}", line.trim_start_matches("ERROR: "));
        }
        serde_json::from_slice(&out.stdout).context("reading yt-dlp's output")
    }

    /// Searches YouTube for videos.
    pub fn search(&self, query: &str, count: usize) -> Result<Vec<Listed>> {
        Ok(listed(self.flat(&format!("ytsearch{count}:{query}"), None)?))
    }

    /// A channel's id and name from any link to it (an @handle, /channel/, /c/ or /user/
    /// URL, or a video of theirs).
    pub fn channel(&self, url: &str) -> Result<(String, String)> {
        let url = url.trim();
        let url = if url.starts_with('@') { format!("https://www.youtube.com/{url}") } else { url.to_owned() };
        if let Some(id) = super::video_id(&url) {
            let video = self.flat(&format!("https://www.youtube.com/watch?v={id}"), None)?;
            let channel_id = video.channel_id.context("yt-dlp didn't say whose video that is")?;
            return Ok((channel_id, video.channel.or(video.uploader).unwrap_or_default()));
        }
        let flat = self.flat(&url, Some(1))?;
        let id = flat.channel_id.or_else(|| channel_id(&url)).context("that doesn't look like a YouTube channel")?;
        let name = flat.channel.or(flat.uploader).or(flat.title).unwrap_or_default();
        Ok((id, name.trim_end_matches(" - Videos").to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns:yt="http://www.youtube.com/xml/schemas/2015" xmlns:media="http://search.yahoo.com/mrss/" xmlns="http://www.w3.org/2005/Atom">
 <title>Blender</title>
 <entry>
  <id>yt:video:aqz-KE-bpKQ</id>
  <yt:videoId>aqz-KE-bpKQ</yt:videoId>
  <title>Big Buck Bunny</title>
  <link rel="alternate" href="https://www.youtube.com/watch?v=aqz-KE-bpKQ"/>
  <author><name>Blender</name></author>
  <published>2014-11-10T14:05:55+00:00</published>
 </entry>
 <entry>
  <yt:videoId>abcdefghijk</yt:videoId>
  <title>A short</title>
  <link rel="alternate" href="https://www.youtube.com/shorts/abcdefghijk"/>
  <published>2026-10-01T12:00:00+01:00</published>
 </entry>
</feed>"#;

    #[test]
    fn reads_feeds() {
        let (channel, videos) = parse_feed(FEED, "UCSMOQeBJ2RAnuFungnQOxLg").unwrap();
        assert_eq!(channel, "Blender");
        assert_eq!(videos.len(), 2);
        assert_eq!((videos[0].id.as_str(), videos[0].title.as_str(), videos[0].short), ("aqz-KE-bpKQ", "Big Buck Bunny", false));
        assert_eq!(videos[0].published, 1_415_628_355);
        assert!(videos[1].short);
        assert_eq!(videos[1].channel, "Blender", "falls back to the feed's title");
        assert_eq!(videos[1].published, 1_790_852_400, "offset applied");
    }

    #[test]
    fn parses_times() {
        assert_eq!(parse_time("1970-01-01T00:00:00+00:00"), Some(0));
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_time("2026-10-01T11:00:00.123-01:00"), Some(1_790_856_000));
        assert_eq!(parse_time("nonsense"), None);
    }

    const A: &str = "UCSMOQeBJ2RAnuFungnQOxLg";
    const B: &str = "UC_x5XG1OV2P6uZZ5FSM9Ttw";

    #[test]
    fn imports_takeout_csv() {
        let csv = format!("Channel Id,Channel Url,Channel Title\n{A},http://www.youtube.com/channel/{A},Blender\n{B},http://www.youtube.com/channel/{B},\"Google, for Developers\"\n");
        assert_eq!(parse_subscriptions(&csv), vec![(A.into(), "Blender".into()), (B.into(), "Google, for Developers".into())]);
    }

    #[test]
    fn imports_newpipe_and_freetube() {
        let newpipe = format!(r#"{{"app_version":"0.27","subscriptions":[{{"service_id":0,"url":"https://www.youtube.com/channel/{A}","name":"Blender"}}]}}"#);
        assert_eq!(parse_subscriptions(&newpipe), vec![(A.into(), "Blender".into())]);
        // FreeTube's profiles file: one JSON object per line; duplicates across profiles.
        let freetube = format!(
            "{{\"name\":\"All Channels\",\"subscriptions\":[{{\"id\":\"{A}\",\"name\":\"Blender\"}},{{\"id\":\"{B}\",\"name\":\"Google\"}}],\"_id\":\"allChannels\"}}\n\
             {{\"name\":\"Art\",\"subscriptions\":[{{\"id\":\"{A}\",\"name\":\"Blender\"}}],\"_id\":\"x\"}}\n"
        );
        assert_eq!(parse_subscriptions(&freetube), vec![(A.into(), "Blender".into()), (B.into(), "Google".into())]);
    }

    #[test]
    fn imports_opml() {
        let opml = format!(
            r#"<opml version="1.1"><body><outline text="YouTube Subscriptions"><outline text="Blender" title="Blender" type="rss" xmlUrl="https://www.youtube.com/feeds/videos.xml?channel_id={A}"/></outline></body></opml>"#
        );
        assert_eq!(parse_subscriptions(&opml), vec![(A.into(), "Blender".into())]);
    }

    #[test]
    fn lists_only_videos() {
        let flat: Flat = serde_json::from_str(&format!(
            r#"{{"entries":[{{"id":"aqz-KE-bpKQ","title":"Bunny","channel":"Blender","channel_id":"{A}","duration":635.0,"ie_key":"Youtube"}},
                {{"id":"{A}","title":"A channel","ie_key":"YoutubeTab"}}]}}"#
        ))
        .unwrap();
        let got = listed(flat);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].id.as_str(), got[0].duration), ("aqz-KE-bpKQ", Some(635.0)));
    }

    /// Real network: `cargo test -p flex-core youtube_online -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn youtube_online() {
        let (channel, videos) = fetch_feed(A).unwrap();
        eprintln!("feed: {channel}, {} videos, newest {:?}", videos.len(), videos.first().map(|v| &v.title));
        assert!(!videos.is_empty());
        let tools = Tools::find().expect("yt-dlp");
        let results = tools.search("big buck bunny blender", 5).unwrap();
        eprintln!("search: {:?}", results.iter().map(|r| (&r.title, &r.channel)).collect::<Vec<_>>());
        assert!(!results.is_empty());
        let (id, name) = tools.channel("@BlenderOfficial").unwrap();
        eprintln!("channel: {id} {name}");
        assert_eq!(id, A);
    }
}
