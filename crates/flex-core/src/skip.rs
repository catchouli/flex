//! "Skip intro": finds openings and endings from a file's chapter names. Many releases
//! (anime especially) mark them as chapters called "OP", "Opening", "ED", "Credits" and
//! so on. Files without such chapters get no skip button.

/// What a skippable part is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipKind {
    Opening,
    Ending,
}

impl SkipKind {
    /// The button's label.
    pub fn label(self) -> &'static str {
        match self {
            SkipKind::Opening => "Skip opening",
            SkipKind::Ending => "Skip credits",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Skip {
    pub kind: SkipKind,
    pub start: f64,
    /// Where skipping lands: the next chapter, or the end of the file.
    pub end: f64,
}

/// Shorter than this, a chapter called "Intro" is more likely a cold open's title card.
const MIN_SECS: f64 = 5.0;
/// Openings are ~90s; longer "intro" chapters are probably something else.
const MAX_OPENING_SECS: f64 = 4.0 * 60.0;
/// Film credits run long.
const MAX_ENDING_SECS: f64 = 15.0 * 60.0;

fn kind_of(title: &str) -> Option<SkipKind> {
    let t = title.trim().to_lowercase();
    // Words as separate tokens: "op", "op1", "op 2", "ed", but not "Operation" or "Edward".
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let numbered = |w: &str, prefix: &str| w.strip_prefix(prefix).is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()));
    let first = words.first().copied().unwrap_or_default();
    let has = |needle: &str| words.contains(&needle);
    if numbered(first, "op") || has("opening") || has("intro") || t.contains("オープニング") {
        return Some(SkipKind::Opening);
    }
    if numbered(first, "ed") || has("ending") || has("credits") || has("outro") || t.contains("エンディング") {
        return Some(SkipKind::Ending);
    }
    None
}

/// The skippable parts among `chapters` (start in seconds, title), in a file `duration`
/// seconds long.
pub fn find(chapters: &[(f64, String)], duration: Option<f64>) -> Vec<Skip> {
    chapters
        .iter()
        .enumerate()
        .filter_map(|(i, (start, title))| {
            let kind = kind_of(title)?;
            let end = chapters.get(i + 1).map(|c| c.0).or(duration)?;
            let length = end - start;
            let max = if kind == SkipKind::Opening { MAX_OPENING_SECS } else { MAX_ENDING_SECS };
            (MIN_SECS..=max).contains(&length).then_some(Skip { kind, start: *start, end })
        })
        .collect()
}

/// The part playing at `pos`, if any (not in its last second, so it doesn't flash up
/// just as it ends).
pub fn at(skips: &[Skip], pos: f64) -> Option<(usize, &Skip)> {
    skips.iter().enumerate().find(|(_, s)| pos >= s.start && pos < s.end - 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chapters(list: &[(f64, &str)]) -> Vec<(f64, String)> {
        list.iter().map(|(t, n)| (*t, (*n).to_owned())).collect()
    }

    #[test]
    fn recognises_chapter_names() {
        for name in ["OP", "Opening", "Opening Song", "OP1", "Op 2", "Intro", "オープニング"] {
            assert_eq!(kind_of(name), Some(SkipKind::Opening), "{name}");
        }
        for name in ["ED", "Ending", "ED2", "End Credits", "Credits", "Outro", "エンディング"] {
            assert_eq!(kind_of(name), Some(SkipKind::Ending), "{name}");
        }
        for name in ["Prologue", "Part A", "Operation Overlord", "Edward", "Chapter 01", "Preview", "Avant", ""] {
            assert_eq!(kind_of(name), None, "{name}");
        }
    }

    #[test]
    fn finds_an_anime_episodes_opening_and_ending() {
        let list = chapters(&[(0.0, "Prologue"), (95.0, "Opening"), (185.0, "Part A"), (720.0, "Part B"), (1300.0, "Ending"), (1390.0, "Preview")]);
        let skips = find(&list, Some(1420.0));
        assert_eq!(
            skips,
            vec![
                Skip { kind: SkipKind::Opening, start: 95.0, end: 185.0 },
                Skip { kind: SkipKind::Ending, start: 1300.0, end: 1390.0 },
            ]
        );
        assert_eq!(at(&skips, 100.0).map(|(i, _)| i), Some(0));
        assert_eq!(at(&skips, 184.5), None, "not in its last second");
        assert_eq!(at(&skips, 500.0), None);
    }

    #[test]
    fn last_chapter_runs_to_the_end_and_odd_lengths_are_ignored() {
        let film = chapters(&[(0.0, "Chapter 1"), (5400.0, "End Credits")]);
        assert_eq!(find(&film, Some(5880.0)), vec![Skip { kind: SkipKind::Ending, start: 5400.0, end: 5880.0 }]);
        assert!(find(&film, None).is_empty(), "no duration: no end for the last chapter");
        // A 2-second "Intro" card and a 10-minute "Opening" aren't skipped.
        let odd = chapters(&[(0.0, "Intro"), (2.0, "Part A"), (60.0, "Opening"), (660.0, "Part B")]);
        assert!(find(&odd, Some(1400.0)).is_empty());
    }
}
