//! Picking the audio track in a show or film's original language (TMDB's
//! `original_language`, an ISO 639-1 code like "ja") from the tracks a file has. Tracks
//! are matched by their language tag first, then by a language name in their title, for
//! files whose tags are missing or wrong.

/// An audio track, as mpv lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub id: i64,
    /// The track's language tag, e.g. "jpn" or "ja" (often missing).
    pub lang: String,
    pub title: String,
    /// The file marks it as the default.
    pub default: bool,
}

/// Language tags (ISO 639-1, 639-2/B and /T) and names (English and native) a track in
/// each language might have.
const LANGUAGES: &[(&str, &[&str], &[&str])] = &[
    ("ja", &["ja", "jp", "jpn"], &["japanese", "日本語"]),
    ("en", &["en", "eng"], &["english"]),
    ("ko", &["ko", "kor"], &["korean", "한국어"]),
    ("zh", &["zh", "chi", "zho", "cmn"], &["chinese", "mandarin", "中文", "國語", "国语", "普通话"]),
    // TMDB's code for Cantonese.
    ("cn", &["yue", "chi", "zho"], &["cantonese", "粵語", "粤语"]),
    ("fr", &["fr", "fre", "fra"], &["french", "français", "francais"]),
    ("de", &["de", "ger", "deu"], &["german", "deutsch"]),
    ("es", &["es", "spa"], &["spanish", "español", "espanol", "castellano"]),
    ("it", &["it", "ita"], &["italian", "italiano"]),
    ("pt", &["pt", "por"], &["portuguese", "português", "portugues"]),
    ("ru", &["ru", "rus"], &["russian", "русский"]),
    ("hi", &["hi", "hin"], &["hindi", "हिन्दी"]),
    ("ta", &["ta", "tam"], &["tamil"]),
    ("te", &["te", "tel"], &["telugu"]),
    ("th", &["th", "tha"], &["thai", "ไทย"]),
    ("nl", &["nl", "dut", "nld"], &["dutch", "nederlands"]),
    ("sv", &["sv", "swe"], &["swedish", "svenska"]),
    ("da", &["da", "dan"], &["danish", "dansk"]),
    ("no", &["no", "nor", "nob", "nno"], &["norwegian", "norsk"]),
    ("fi", &["fi", "fin"], &["finnish", "suomi"]),
    ("pl", &["pl", "pol"], &["polish", "polski"]),
    ("tr", &["tr", "tur"], &["turkish", "türkçe"]),
    ("ar", &["ar", "ara"], &["arabic", "العربية"]),
    ("he", &["he", "heb"], &["hebrew", "עברית"]),
    ("id", &["id", "ind"], &["indonesian"]),
    ("vi", &["vi", "vie"], &["vietnamese", "tiếng việt"]),
];

/// The audio track in `language` (ISO 639-1), if one can be told apart. Among several,
/// the file's default wins, then the first; commentaries are left out.
pub fn pick(tracks: &[Track], language: &str) -> Option<i64> {
    let language = language.trim().to_lowercase();
    if language.is_empty() {
        return None;
    }
    let (tags, names): (Vec<&str>, &[&str]) = match LANGUAGES.iter().find(|(code, _, _)| *code == language) {
        Some((_, tags, names)) => (tags.to_vec(), names),
        None => (vec![language.as_str()], &[]),
    };
    let usable: Vec<&Track> = tracks.iter().filter(|t| !t.title.to_lowercase().contains("commentary")).collect();
    let by_tag: Vec<&Track> = usable.iter().copied().filter(|t| tags.contains(&t.lang.trim().to_lowercase().as_str())).collect();
    let matches = if by_tag.is_empty() {
        // A name in the title, but only for tracks without a (different) language tag.
        usable
            .iter()
            .copied()
            .filter(|t| t.lang.is_empty() || t.lang == "und")
            .filter(|t| {
                let title = t.title.to_lowercase();
                names.iter().any(|n| title.contains(n))
            })
            .collect()
    } else {
        by_tag
    };
    matches.iter().find(|t| t.default).or(matches.first()).map(|t| t.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64, lang: &str, title: &str, default: bool) -> Track {
        Track { id, lang: lang.into(), title: title.into(), default }
    }

    #[test]
    fn picks_by_language_tag() {
        let tracks = [track(1, "eng", "English 5.1", true), track(2, "jpn", "", false)];
        assert_eq!(pick(&tracks, "ja"), Some(2));
        assert_eq!(pick(&tracks, "en"), Some(1));
        assert_eq!(pick(&tracks, "JA"), Some(2), "case doesn't matter");
    }

    #[test]
    fn falls_back_to_the_title() {
        let tracks = [track(1, "", "English dub", true), track(2, "und", "Japanese (original)", false)];
        assert_eq!(pick(&tracks, "ja"), Some(2));
        // A track tagged as another language isn't picked for its title.
        let tracks = [track(1, "eng", "English (Japanese songs)", true)];
        assert_eq!(pick(&tracks, "ja"), None);
    }

    #[test]
    fn prefers_the_default_and_skips_commentary() {
        let tracks = [
            track(1, "jpn", "Commentary", false),
            track(2, "jpn", "Stereo", false),
            track(3, "jpn", "5.1", true),
        ];
        assert_eq!(pick(&tracks, "ja"), Some(3));
        let tracks = [track(1, "jpn", "Director's commentary", true), track(2, "jpn", "", false)];
        assert_eq!(pick(&tracks, "ja"), Some(2));
    }

    #[test]
    fn unknown_or_missing() {
        let tracks = [track(1, "eng", "", true)];
        assert_eq!(pick(&tracks, "ja"), None, "no Japanese track: leave it");
        assert_eq!(pick(&tracks, ""), None);
        assert_eq!(pick(&[track(1, "xx", "", false)], "xx"), Some(1), "codes not in the table still match tags");
    }
}
