//! YouTube in Flex's database: subscribed channels, the videos their feeds listed (kept
//! after they drop off the feed), and watch progress for any YouTube video played.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use super::feeds::FeedVideo;
use crate::Db;

#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub id: String,
    pub title: String,
    pub added_at: i64,
    /// When its feed was last read (0 = never).
    pub checked_at: i64,
    /// One you watch regularly: listed first, and what the focused filter shows.
    pub focused: bool,
    /// When `focused` last changed (0 = never), for syncing.
    pub focused_at: i64,
}

/// A video from a subscription.
#[derive(Clone, Debug, PartialEq)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel_id: String,
    pub channel: String,
    pub published: i64,
    pub short: bool,
    /// Seconds, once looked up (feeds don't say).
    pub duration: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WatchProgress {
    pub title: String,
    pub channel: String,
    /// Where to resume, if part-watched.
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub watched: bool,
    pub updated_at: i64,
}

fn video(r: &rusqlite::Row) -> rusqlite::Result<Video> {
    let duration = r.get::<_, Option<f64>>(6)?.filter(|d| *d > 0.0);
    Ok(Video { id: r.get(0)?, title: r.get(1)?, channel_id: r.get(2)?, channel: r.get(3)?, published: r.get(4)?, short: r.get(5)?, duration })
}

const VIDEO_COLUMNS: &str = "id, title, channel_id, channel, published, short, duration";

/// Part-watched videos resume unless they're nearly over (or barely started).
const MIN_RESUME_SECS: f64 = 10.0;

impl Db {
    pub fn yt_channels(&self) -> Result<Vec<Channel>> {
        let mut stmt = self.conn.prepare("SELECT id, title, added_at, checked_at, focused, focused_at FROM yt_channels ORDER BY title COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| {
            Ok(Channel { id: r.get(0)?, title: r.get(1)?, added_at: r.get(2)?, checked_at: r.get(3)?, focused: r.get(4)?, focused_at: r.get(5)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Subscribes (a channel already there keeps its date). Returns whether it was new.
    pub fn add_yt_channel(&self, id: &str, title: &str, now: i64) -> Result<bool> {
        let added = self.conn.execute(
            "INSERT OR IGNORE INTO yt_channels (id, title, added_at) VALUES (?1, ?2, ?3)",
            params![id, title, now],
        )?;
        self.conn.execute("DELETE FROM sync_removed WHERE kind = 'yt_channel' AND key = ?1", [id])?;
        Ok(added > 0)
    }

    /// Marks a subscription as focused (or not).
    pub fn set_yt_channel_focused(&self, id: &str, focused: bool, now: i64) -> Result<()> {
        self.conn.execute("UPDATE yt_channels SET focused = ?2, focused_at = ?3 WHERE id = ?1", params![id, focused, now])?;
        Ok(())
    }

    pub fn remove_yt_channel(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM yt_videos WHERE channel_id = ?1", [id])?;
        self.conn.execute("DELETE FROM yt_channels WHERE id = ?1", [id])?;
        self.removed("yt_channel", id)?;
        Ok(())
    }

    /// Saves what a channel's feed listed, and when it was read.
    pub fn save_yt_feed(&mut self, channel_id: &str, title: &str, videos: &[FeedVideo], now: i64) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO yt_videos (id, channel_id, title, channel, published, short) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET title = ?3, channel = ?4, short = ?6",
            )?;
            for v in videos {
                stmt.execute(params![v.id, channel_id, v.title, v.channel, v.published, v.short])?;
            }
            let title = if title.is_empty() { None } else { Some(title) };
            tx.execute(
                "UPDATE yt_channels SET checked_at = ?2, title = COALESCE(?3, title) WHERE id = ?1",
                params![channel_id, now, title],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The newest videos across subscriptions (only focused ones if `focused_only`).
    pub fn yt_latest(&self, limit: usize, shorts: bool, focused_only: bool) -> Result<Vec<Video>> {
        let sql = format!(
            "SELECT {VIDEO_COLUMNS} FROM yt_videos WHERE (?1 OR short = 0) AND channel_id IN (SELECT id FROM yt_channels WHERE focused OR NOT ?3)
             ORDER BY published DESC LIMIT ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![shorts, limit as i64, focused_only], video)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn yt_channel_videos(&self, channel_id: &str, limit: usize, shorts: bool) -> Result<Vec<Video>> {
        let sql = format!("SELECT {VIDEO_COLUMNS} FROM yt_videos WHERE channel_id = ?1 AND (?2 OR short = 0) ORDER BY published DESC LIMIT ?3");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![channel_id, shorts, limit as i64], video)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Saves how long videos are, for those that are stored (from any list that says).
    pub fn save_yt_durations(&mut self, durations: &[(String, f64)]) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare("UPDATE yt_videos SET duration = ?2 WHERE id = ?1 AND duration IS NOT ?2")?;
            for (id, secs) in durations {
                stmt.execute(params![id, secs])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Videos (not Shorts) whose length hasn't been looked up yet, by channel:
    /// (channel id, video ids).
    pub fn yt_missing_durations(&self) -> Result<Vec<(String, Vec<String>)>> {
        let mut stmt = self.conn.prepare("SELECT channel_id, id FROM yt_videos WHERE duration IS NULL AND short = 0 ORDER BY channel_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for row in rows {
            let (channel, id) = row?;
            match out.last_mut() {
                Some((c, ids)) if *c == channel => ids.push(id),
                _ => out.push((channel, vec![id])),
            }
        }
        Ok(out)
    }

    /// Marks videos whose length couldn't be found (e.g. live streams), so they aren't
    /// looked up again.
    pub fn mark_yt_durations_unknown(&self, ids: &[String]) -> Result<()> {
        for id in ids {
            self.conn.execute("UPDATE yt_videos SET duration = 0 WHERE id = ?1 AND duration IS NULL", [id])?;
        }
        Ok(())
    }

    pub fn yt_progress(&self, id: &str) -> Result<Option<WatchProgress>> {
        Ok(self
            .conn
            .query_row("SELECT title, channel, position, duration, watched, updated_at FROM yt_progress WHERE id = ?1", [id], |r| {
                Ok(WatchProgress { title: r.get(0)?, channel: r.get(1)?, position: r.get(2)?, duration: r.get(3)?, watched: r.get(4)?, updated_at: r.get(5)? })
            })
            .optional()?)
    }

    /// Progress for every YouTube video played, by id (to mark them in lists).
    pub fn yt_progress_all(&self) -> Result<HashMap<String, WatchProgress>> {
        let mut stmt = self.conn.prepare("SELECT id, title, channel, position, duration, watched, updated_at FROM yt_progress")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                WatchProgress { title: r.get(1)?, channel: r.get(2)?, position: r.get(3)?, duration: r.get(4)?, watched: r.get(5)?, updated_at: r.get(6)? },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Videos actually played (not only marked watched), most recently first: (id, progress).
    /// One entry per video, at the last time it was played.
    pub fn yt_history(&self, limit: usize) -> Result<Vec<(String, WatchProgress)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, channel, position, duration, watched, updated_at FROM yt_progress
             WHERE duration IS NOT NULL ORDER BY updated_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                WatchProgress { title: r.get(1)?, channel: r.get(2)?, position: r.get(3)?, duration: r.get(4)?, watched: r.get(5)?, updated_at: r.get(6)? },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Records where playback got to. Past `threshold` (e.g. 0.9) of the way through, the
    /// video counts as watched and won't resume. Returns the saved progress.
    #[allow(clippy::too_many_arguments)]
    pub fn record_yt_progress(&self, id: &str, title: &str, channel: &str, position: f64, duration: f64, threshold: f64, now: i64) -> Result<WatchProgress> {
        let old = self.yt_progress(id)?;
        let finished = duration > 0.0 && position >= duration * threshold;
        let watched = finished || old.as_ref().is_some_and(|o| o.watched);
        let resume = (!finished && position >= MIN_RESUME_SECS).then_some(position);
        self.conn.execute(
            "INSERT INTO yt_progress (id, title, channel, position, duration, watched, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET title = ?2, channel = ?3, position = ?4, duration = ?5, watched = ?6, updated_at = ?7",
            params![id, title, channel, resume, duration, watched, now],
        )?;
        Ok(WatchProgress { title: title.to_owned(), channel: channel.to_owned(), position: resume, duration: Some(duration), watched, updated_at: now })
    }

    pub fn set_yt_watched(&self, id: &str, title: &str, channel: &str, watched: bool, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO yt_progress (id, title, channel, position, watched, updated_at) VALUES (?1, ?2, ?3, NULL, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET position = NULL, watched = ?4, updated_at = ?5",
            params![id, title, channel, watched, now],
        )?;
        Ok(())
    }

    /// Part-watched videos for Continue Watching, most recent first: (id, progress).
    /// Leaves out ones removed from there (until they're played again).
    pub fn yt_continue(&self, limit: usize) -> Result<Vec<(String, WatchProgress)>> {
        let hidden = self.continue_hidden()?;
        let mut all: Vec<(String, WatchProgress)> = self
            .yt_progress_all()?
            .into_iter()
            .filter(|(id, p)| p.position.is_some() && hidden.get(&format!("yt:{id}")).is_none_or(|at| p.updated_at > *at))
            .collect();
        all.sort_by_key(|(_, p)| std::cmp::Reverse(p.updated_at));
        all.truncate(limit);
        Ok(all)
    }

    /// Keeps only the `keep` most recently played videos in Continue Watching: older ones
    /// are removed from there (keeping their resume point; playing one again brings it
    /// back). Returns how many were removed.
    pub fn trim_yt_continue(&self, keep: usize, now: i64) -> Result<usize> {
        let extra = self.yt_continue(usize::MAX)?.into_iter().skip(keep).collect::<Vec<_>>();
        for (id, _) in &extra {
            self.hide_continue(&format!("yt:{id}"), now)?;
        }
        Ok(extra.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(id: &str, channel_id: &str, published: i64, short: bool) -> FeedVideo {
        FeedVideo { id: id.into(), title: format!("Video {id}"), channel_id: channel_id.into(), channel: "Chan".into(), published, short }
    }

    #[test]
    fn subscriptions_and_feeds() {
        let mut db = Db::open_in_memory().unwrap();
        assert!(db.add_yt_channel("UCa", "A", 1).unwrap());
        assert!(!db.add_yt_channel("UCa", "A again", 2).unwrap(), "already there");
        db.add_yt_channel("UCb", "B", 3).unwrap();
        db.save_yt_feed("UCa", "A renamed", &[feed("v1", "UCa", 100, false), feed("v2", "UCa", 300, true)], 50).unwrap();
        db.save_yt_feed("UCb", "", &[feed("v3", "UCb", 200, false)], 60).unwrap();

        let channels = db.yt_channels().unwrap();
        assert_eq!((channels[0].title.as_str(), channels[0].checked_at), ("A renamed", 50));
        assert_eq!(channels[1].title, "B", "an empty feed title keeps the name");
        let ids = |v: Vec<Video>| v.into_iter().map(|v| v.id).collect::<Vec<_>>();
        assert_eq!(ids(db.yt_latest(10, true, false).unwrap()), ["v2", "v3", "v1"]);
        assert_eq!(ids(db.yt_latest(10, false, false).unwrap()), ["v3", "v1"], "no Shorts");
        assert_eq!(ids(db.yt_channel_videos("UCa", 10, true).unwrap()), ["v2", "v1"]);

        assert!(ids(db.yt_latest(10, true, true).unwrap()).is_empty(), "none focused yet");
        db.set_yt_channel_focused("UCb", true, 70).unwrap();
        assert_eq!(ids(db.yt_latest(10, true, true).unwrap()), ["v3"], "only focused channels");
        let b = db.yt_channels().unwrap().into_iter().find(|c| c.id == "UCb").unwrap();
        assert_eq!((b.focused, b.focused_at), (true, 70));

        db.remove_yt_channel("UCa").unwrap();
        assert_eq!(ids(db.yt_latest(10, true, false).unwrap()), ["v3"]);
    }

    #[test]
    fn durations_are_looked_up_once() {
        let mut db = Db::open_in_memory().unwrap();
        db.add_yt_channel("UCa", "A", 1).unwrap();
        db.save_yt_feed("UCa", "A", &[feed("v1", "UCa", 100, false), feed("v2", "UCa", 200, false), feed("s", "UCa", 300, true)], 5).unwrap();
        assert_eq!(db.yt_missing_durations().unwrap(), vec![("UCa".to_owned(), vec!["v1".to_owned(), "v2".to_owned()])], "not Shorts");
        db.save_yt_durations(&[("v1".into(), 61.0), ("elsewhere".into(), 5.0)]).unwrap();
        db.mark_yt_durations_unknown(&["v1".into(), "v2".into()]).unwrap();
        assert!(db.yt_missing_durations().unwrap().is_empty());
        let videos = db.yt_channel_videos("UCa", 10, false).unwrap();
        assert_eq!(videos.iter().map(|v| v.duration).collect::<Vec<_>>(), [None, Some(61.0)], "v2 unknown, v1 kept");
        // A feed read again keeps the lengths.
        db.save_yt_feed("UCa", "A", &[feed("v1", "UCa", 100, false)], 6).unwrap();
        assert_eq!(db.yt_channel_videos("UCa", 10, false).unwrap()[1].duration, Some(61.0));
    }

    #[test]
    fn continue_watching_keeps_only_the_newest() {
        let db = Db::open_in_memory().unwrap();
        for (i, id) in ["a", "b", "c", "d"].iter().enumerate() {
            db.record_yt_progress(id, "T", "C", 60.0, 600.0, 0.9, 100 + i as i64).unwrap();
        }
        assert_eq!(db.trim_yt_continue(2, 200).unwrap(), 2);
        let ids = |db: &Db| db.yt_continue(10).unwrap().into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        assert_eq!(ids(&db), ["d", "c"], "the two most recent stay");
        assert_eq!(db.yt_progress("a").unwrap().unwrap().position, Some(60.0), "resume point kept");
        assert_eq!(db.trim_yt_continue(2, 201).unwrap(), 0, "nothing more to do");
        // Playing an old one again brings it back as the newest; the oldest then goes.
        db.record_yt_progress("a", "T", "C", 90.0, 600.0, 0.9, 300).unwrap();
        db.trim_yt_continue(2, 301).unwrap();
        assert_eq!(ids(&db), ["a", "d"]);
        // None at all.
        db.trim_yt_continue(0, 400).unwrap();
        assert!(ids(&db).is_empty());
    }

    #[test]
    fn progress_resumes_then_finishes() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.yt_progress("v").unwrap(), None);
        let p = db.record_yt_progress("v", "T", "C", 120.0, 600.0, 0.9, 10).unwrap();
        assert_eq!((p.position, p.watched), (Some(120.0), false));
        assert_eq!(db.yt_continue(5).unwrap().len(), 1);
        // Barely started: nothing to resume.
        assert_eq!(db.record_yt_progress("w", "T", "C", 3.0, 600.0, 0.9, 11).unwrap().position, None);
        let p = db.record_yt_progress("v", "T", "C", 580.0, 600.0, 0.9, 12).unwrap();
        assert_eq!((p.position, p.watched), (None, true));
        assert!(db.yt_continue(5).unwrap().is_empty());
        // Rewatching part of it doesn't make it unwatched.
        assert!(db.record_yt_progress("v", "T", "C", 60.0, 600.0, 0.9, 13).unwrap().watched);
        db.set_yt_watched("v", "T", "C", false, 14).unwrap();
        assert!(!db.yt_progress("v").unwrap().unwrap().watched);
        assert_eq!(db.yt_progress_all().unwrap().len(), 2);
    }

    #[test]
    fn history_lists_played_videos_newest_first() {
        let db = Db::open_in_memory().unwrap();
        db.record_yt_progress("a", "A", "C", 60.0, 600.0, 0.9, 10).unwrap();
        db.record_yt_progress("b", "B", "C", 590.0, 600.0, 0.9, 20).unwrap();
        // Marked watched from a list without being played: not history.
        db.set_yt_watched("c", "C", "C", true, 30).unwrap();
        let ids = |db: &Db| db.yt_history(10).unwrap().into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        assert_eq!(ids(&db), ["b", "a"]);
        db.record_yt_progress("a", "A", "C", 120.0, 600.0, 0.9, 40).unwrap();
        assert_eq!(ids(&db), ["a", "b"], "playing again moves it to the top");
        assert_eq!(db.yt_history(1).unwrap().len(), 1);
    }
}
