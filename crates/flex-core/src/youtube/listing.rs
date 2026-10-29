//! A channel's videos or playlists, or a playlist's videos, read from yt-dlp as it goes.
//! YouTube hands these out 30 at a time, so a channel with years of uploads takes a while
//! to list in full; a [`Listing`] gives each entry as soon as yt-dlp prints it. yt-dlp
//! waits while nobody reads (its output pipe fills up), so reading only as far as someone
//! scrolls keeps it from fetching the whole channel up front.

use std::io::{BufRead, BufReader, Lines, Read};
use std::process::{Child, ChildStderr, ChildStdout, Stdio};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::{InUse, Tools};
use super::feeds::Listed;

/// A channel's uploads, newest first (without Shorts or live streams, which YouTube lists
/// separately).
pub fn channel_videos_url(channel_id: &str) -> String {
    format!("https://www.youtube.com/channel/{channel_id}/videos")
}

/// A channel's playlists.
pub fn channel_playlists_url(channel_id: &str) -> String {
    format!("https://www.youtube.com/channel/{channel_id}/playlists")
}

pub fn playlist_url(playlist_id: &str) -> String {
    format!("https://www.youtube.com/playlist?list={playlist_id}")
}

/// A playlist in a channel's list of them.
#[derive(Clone, Debug, PartialEq)]
pub struct Playlist {
    pub id: String,
    pub title: String,
    /// Usually its first video's.
    pub thumbnail: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Video(Listed),
    Playlist(Playlist),
}

#[derive(Deserialize)]
struct Line {
    id: Option<String>,
    title: Option<String>,
    duration: Option<f64>,
    view_count: Option<u64>,
    /// Approximate, from YouTube's "3 years ago" (see `approximate_date` below).
    timestamp: Option<i64>,
    channel: Option<String>,
    channel_id: Option<String>,
    /// For a channel's own tabs, entries name the channel only here.
    playlist_channel: Option<String>,
    playlist_channel_id: Option<String>,
    ie_key: Option<String>,
    #[serde(default)]
    thumbnails: Vec<Thumbnail>,
}

#[derive(Deserialize)]
struct Thumbnail {
    url: String,
}

/// One line of yt-dlp's `--flat-playlist -j` output; `None` for anything else (channels,
/// unreadable lines).
fn parse_line(line: &str) -> Option<Entry> {
    let l: Line = serde_json::from_str(line).ok()?;
    let id = l.id?;
    match l.ie_key.as_deref() {
        Some("Youtube") | None if super::video_id(&id).is_some() => Some(Entry::Video(Listed {
            title: l.title.unwrap_or_default(),
            channel: l.channel.or(l.playlist_channel).unwrap_or_default(),
            channel_id: l.channel_id.or(l.playlist_channel_id).unwrap_or_default(),
            duration: l.duration,
            views: l.view_count,
            published: l.timestamp,
            id,
        })),
        // Playlists list as "tabs" too, as do channels ("UC…"), which aren't wanted here.
        Some("YoutubeTab") if !id.starts_with("UC") => Some(Entry::Playlist(Playlist {
            title: l.title.unwrap_or_default(),
            thumbnail: l.thumbnails.into_iter().last().map(|t| t.url),
            id,
        })),
        _ => None,
    }
}

/// yt-dlp listing something, one entry at a time. Dropping it stops yt-dlp.
pub struct Listing {
    child: Child,
    lines: Lines<BufReader<ChildStdout>>,
    stderr: Option<ChildStderr>,
    /// Entries given out, to tell "empty" from "failed".
    count: usize,
    /// Dropped after `Drop` has stopped yt-dlp.
    _in_use: InUse,
}

impl Listing {
    /// Waits for yt-dlp to finish (once the entries have run out); an error if it failed
    /// before listing anything.
    pub fn finish(mut self) -> Result<()> {
        let status = self.child.wait().context("waiting for yt-dlp")?;
        if status.success() || self.count > 0 {
            return Ok(());
        }
        let mut err = String::new();
        if let Some(mut stderr) = self.stderr.take() {
            let _ = stderr.read_to_string(&mut err);
        }
        let line = err.lines().rev().find(|l| l.starts_with("ERROR")).unwrap_or(err.trim());
        bail!("yt-dlp: {}", line.trim_start_matches("ERROR: "))
    }
}

impl Iterator for Listing {
    type Item = Entry;

    fn next(&mut self) -> Option<Entry> {
        loop {
            let line = self.lines.next()?.ok()?;
            if let Some(entry) = parse_line(&line) {
                self.count += 1;
                return Some(entry);
            }
        }
    }
}

impl Drop for Listing {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Tools {
    /// Starts listing a channel tab or playlist (see the `*_url` functions).
    pub fn listing(&self, url: &str) -> Result<Listing> {
        let in_use = InUse::new();
        let mut child = self
            .command()
            .args(["--flat-playlist", "--lazy-playlist", "--dump-json", url])
            // Upload dates, worked out from the "3 years ago" YouTube shows (listings
            // don't give exact ones).
            .args(["--extractor-args", "youtubetab:approximate_date"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("running {}", self.yt_dlp.display()))?;
        let stdout = child.stdout.take().context("yt-dlp's output")?;
        let stderr = child.stderr.take();
        Ok(Listing { child, lines: BufReader::new(stdout).lines(), stderr, count: 0, _in_use: in_use })
    }

    /// The lengths of the latest videos (about 30) on each channel, in one run of yt-dlp:
    /// (channel id, video id, seconds). Feeds don't say how long videos are.
    pub fn latest_durations(&self, channel_ids: &[String]) -> Result<Vec<(String, String, f64)>> {
        if channel_ids.is_empty() {
            return Ok(Vec::new());
        }
        let _in_use = InUse::new();
        let out = self
            .command()
            .args(["--flat-playlist", "--dump-json", "--ignore-errors", "--playlist-items", "1:30"])
            .args(channel_ids.iter().map(|id| channel_videos_url(id)))
            .output()
            .with_context(|| format!("running {}", self.yt_dlp.display()))?;
        let found: Vec<(String, String, f64)> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| match parse_line(line)? {
                Entry::Video(v) => Some((v.channel_id, v.id, v.duration.filter(|d| *d > 0.0)?)),
                Entry::Playlist(_) => None,
            })
            .collect();
        if found.is_empty() && !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let line = err.lines().rev().find(|l| l.starts_with("ERROR")).unwrap_or(err.trim());
            bail!("yt-dlp: {}", line.trim_start_matches("ERROR: "));
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_channel_videos() {
        let line = r#"{"title":"How Deep Is Your BASS","duration":193,"view_count":688000,"channel_url":null,"ie_key":"Youtube","id":"NdQbLw91KFk",
            "_type":"url","playlist_channel":"Davie504","playlist_channel_id":"UCgFvT6pUq9HLOvKBYERzXSQ","playlist_index":3,"timestamp":1603929600}"#;
        let Some(Entry::Video(v)) = parse_line(line) else { panic!("a video") };
        assert_eq!((v.id.as_str(), v.title.as_str(), v.duration, v.views), ("NdQbLw91KFk", "How Deep Is Your BASS", Some(193.0), Some(688_000)));
        assert_eq!((v.channel.as_str(), v.channel_id.as_str()), ("Davie504", "UCgFvT6pUq9HLOvKBYERzXSQ"));
        assert_eq!(v.published, Some(1_603_929_600));
    }

    #[test]
    fn reads_playlists() {
        let line = r#"{"title":"Anime on BASS","thumbnails":[{"url":"https://i.ytimg.com/vi/HbPnZsoLHyA/hqdefault.jpg?sqp=x","height":270,"width":480}],
            "channel":"Playlist","channel_id":"UCgFvT6pUq9HLOvKBYERzXSQ","ie_key":"YoutubeTab","id":"PL-k0k_f4UoimmXiNWbSpqMp-o_DVcNNVU"}"#;
        let Some(Entry::Playlist(p)) = parse_line(line) else { panic!("a playlist") };
        assert_eq!((p.id.as_str(), p.title.as_str()), ("PL-k0k_f4UoimmXiNWbSpqMp-o_DVcNNVU", "Anime on BASS"));
        assert_eq!(p.thumbnail.as_deref(), Some("https://i.ytimg.com/vi/HbPnZsoLHyA/hqdefault.jpg?sqp=x"));
        assert_eq!(parse_line(r#"{"id":"UCgFvT6pUq9HLOvKBYERzXSQ","ie_key":"YoutubeTab"}"#), None, "a channel");
        assert_eq!(parse_line("not json"), None);
    }

    /// Real network: `cargo test -p flex-core listing_online -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn listing_online() {
        let tools = Tools::find().expect("yt-dlp");
        let channel = "UCgFvT6pUq9HLOvKBYERzXSQ"; // Davie504: hundreds of videos
        let videos: Vec<Entry> = tools.listing(&channel_videos_url(channel)).unwrap().take(100).collect();
        assert_eq!(videos.len(), 100, "more than one page");
        assert!(videos.iter().all(|e| matches!(e, Entry::Video(v) if v.published.is_some())), "dates");
        let playlists: Vec<Entry> = tools.listing(&channel_playlists_url(channel)).unwrap().collect();
        eprintln!("{} playlists, first {:?}", playlists.len(), playlists.first());
        let Some(Entry::Playlist(p)) = playlists.first() else { panic!("a playlist") };
        let in_it: Vec<Entry> = tools.listing(&playlist_url(&p.id)).unwrap().take(5).collect();
        eprintln!("{:?}", in_it.first());
        assert!(matches!(in_it.first(), Some(Entry::Video(v)) if !v.channel.is_empty()));
        let durations = tools.latest_durations(&[channel.to_owned()]).unwrap();
        assert!(durations.len() >= 20 && durations.iter().all(|(c, _, _)| c == channel), "{durations:?}");
    }
}
