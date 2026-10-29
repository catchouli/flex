//! "For you" without an account: videos related to what was played lately and to
//! subscriptions' new videos, ranked by how strongly those suggest them.

use std::collections::{HashMap, HashSet};

use super::feeds::Listed;
use super::store::{Video, WatchProgress};

/// How many recently played videos, and subscriptions' new videos, to start from.
const FROM_HISTORY: usize = 6;
const FROM_SUBSCRIPTIONS: usize = 4;
/// Each older played video counts this much less than the one after it.
const RECENCY_DECAY: f32 = 0.85;
/// At most this many videos from one channel, so one channel doesn't fill the page.
const PER_CHANNEL: usize = 3;
/// Videos from subscribed channels count this much more.
const SUBSCRIBED_BONUS: f32 = 1.25;
/// Shorter than this (seconds) counts as a Short.
const SHORT_SECS: f64 = 60.0;

/// The videos to ask for related videos of, with how much each counts: recently played
/// ones (more if watched through, less if given up on early), then subscriptions' newest
/// unwatched videos. `history` is most recent first, `latest` newest first.
pub fn seeds(history: &[(String, WatchProgress)], latest: &[Video], progress: &HashMap<String, WatchProgress>) -> Vec<(String, f32)> {
    let mut out: Vec<(String, f32)> = Vec::new();
    for (i, (id, p)) in history.iter().take(FROM_HISTORY).enumerate() {
        let fraction = match (p.position, p.duration) {
            (Some(pos), Some(dur)) if dur > 0.0 => pos / dur,
            _ => 0.0,
        };
        let interest = if p.watched { 1.0 } else if fraction > 0.3 { 0.8 } else { 0.4 };
        out.push((id.clone(), interest * RECENCY_DECAY.powi(i as i32)));
    }
    let new: Vec<(String, f32)> = latest
        .iter()
        .filter(|v| !v.short && !progress.get(&v.id).is_some_and(|p| p.watched))
        .filter(|v| !out.iter().any(|(id, _)| *id == v.id))
        .take(FROM_SUBSCRIPTIONS)
        .map(|v| (v.id.clone(), 0.6))
        .collect();
    out.extend(new);
    out
}

/// Merges each seed's related videos (weight, then the list in YouTube's order) into one
/// list. A video scores for every seed that suggests it, more near the top of that list.
/// Leaves out `exclude` (e.g. watched videos and the seeds), Shorts unless `shorts`, and
/// more than a few videos from one channel.
pub fn rank(lists: &[(f32, Vec<Listed>)], exclude: &HashSet<String>, subscribed: &HashSet<String>, shorts: bool) -> Vec<Listed> {
    let mut scores: HashMap<&str, (f32, usize, &Listed)> = HashMap::new();
    let mut order = 0;
    for (weight, list) in lists {
        for (rank, video) in list.iter().enumerate() {
            if exclude.contains(&video.id) || (!shorts && video.duration.is_some_and(|d| d <= SHORT_SECS)) {
                continue;
            }
            let bonus = if subscribed.contains(&video.channel_id) { SUBSCRIBED_BONUS } else { 1.0 };
            let score = weight * bonus / (1.0 + rank as f32 / 4.0);
            let entry = scores.entry(&video.id).or_insert_with(|| {
                order += 1;
                (0.0, order, video)
            });
            entry.0 += score;
        }
    }
    let mut ranked: Vec<(f32, usize, &Listed)> = scores.into_values().collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut per_channel: HashMap<&str, usize> = HashMap::new();
    ranked
        .into_iter()
        .filter(|(_, _, v)| {
            let key = if v.channel_id.is_empty() { v.channel.as_str() } else { v.channel_id.as_str() };
            let n = per_channel.entry(key).or_default();
            *n += 1;
            *n <= PER_CHANNEL
        })
        .map(|(_, _, v)| v.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(id: &str, channel: &str) -> Listed {
        Listed { id: id.into(), title: id.into(), channel: channel.into(), channel_id: format!("UC{channel}"), duration: Some(600.0), views: None, published: None }
    }

    fn played(id: &str, position: f64, watched: bool) -> (String, WatchProgress) {
        let p = WatchProgress { title: id.into(), channel: "c".into(), position: Some(position), duration: Some(100.0), watched, updated_at: 0 };
        (id.into(), p)
    }

    #[test]
    fn suggested_by_more_seeds_ranks_higher() {
        let lists = vec![
            (1.0, vec![listed("a", "x"), listed("b", "y")]),
            (1.0, vec![listed("c", "z"), listed("b", "y")]),
        ];
        let ids: Vec<String> = rank(&lists, &HashSet::new(), &HashSet::new(), true).into_iter().map(|v| v.id).collect();
        assert_eq!(ids, ["b", "a", "c"]);
    }

    #[test]
    fn leaves_out_excluded_shorts_and_too_much_of_one_channel() {
        let mut short = listed("s", "y");
        short.duration = Some(30.0);
        let list = vec![listed("w", "y"), short, listed("1", "x"), listed("2", "x"), listed("3", "x"), listed("4", "x")];
        let exclude = HashSet::from(["w".to_owned()]);
        let ids: Vec<String> = rank(&[(1.0, list)], &exclude, &HashSet::new(), false).into_iter().map(|v| v.id).collect();
        assert_eq!(ids, ["1", "2", "3"]);
    }

    #[test]
    fn seeds_weigh_recent_and_finished_videos_more() {
        let history = vec![played("given-up", 5.0, false), played("finished", 100.0, true)];
        let latest = vec![
            Video { id: "new".into(), title: "t".into(), channel_id: "UCx".into(), channel: "x".into(), published: 0, short: false, duration: None },
            Video { id: "finished".into(), title: "t".into(), channel_id: "UCx".into(), channel: "x".into(), published: 0, short: false, duration: None },
        ];
        let seeds = seeds(&history, &latest, &HashMap::new());
        let ids: Vec<&str> = seeds.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["given-up", "finished", "new"]);
        assert!(seeds[1].1 > seeds[0].1);
    }

    /// Asks YouTube for real: `cargo test -p flex-core suggests_online -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn suggests_online() {
        let lists: Vec<(f32, Vec<Listed>)> =
            ["aqz-KE-bpKQ", "eRsGyueVLvQ"].iter().map(|id| (1.0, super::super::related::related(id).unwrap())).collect();
        let ranked = rank(&lists, &HashSet::new(), &HashSet::new(), false);
        for v in ranked.iter().take(10) {
            eprintln!("{} · {} · {}", v.id, v.channel, v.title);
        }
        assert!(ranked.len() >= 10);
    }
}
