//! Display names for files: show and episode titles from online metadata where available,
//! the file name otherwise. Shared by the app and the web UI.

use std::collections::HashMap;

use crate::catalog::Catalog;
use crate::db::{EpisodeMeta, SeriesMeta};
use crate::tree::Tree;

/// Display names for a file, from online metadata where available.
#[derive(Clone, Debug)]
pub struct Titles {
    /// Show or film title (None if the file isn't organised yet).
    pub show: Option<String>,
    /// For episodes: "S1E5 · Title", "S1E5", or the file name.
    pub episode: Option<String>,
    /// The best single name for the file itself: episode title, film title, else file name.
    pub name: String,
}

impl Titles {
    /// "Show · S1E5 · Title" for episodes, the title for films.
    pub fn full(&self) -> String {
        match (&self.show, &self.episode) {
            (Some(show), Some(ep)) => format!("{show} · {ep}"),
            (Some(show), None) => show.clone(),
            (None, _) => self.name.clone(),
        }
    }
}

/// Titles for a file in a library (`catalog` is None while it's being organised).
pub fn titles(tree: &Tree, catalog: Option<&Catalog>, series: &HashMap<String, SeriesMeta>, file_id: i64) -> Option<Titles> {
    let file = tree.file(file_id)?;
    let file_name = crate::library::display_name(file.file_name(), true);
    let Some(entry) = catalog.and_then(|c| c.show_of_file(file_id)) else {
        return Some(Titles { show: None, episode: None, name: file_name });
    };
    let show = entry.show.title.clone();
    if !entry.is_series() {
        return Some(Titles { name: show.clone(), show: Some(show), episode: None });
    }
    let ep = entry.episodes.iter().find(|e| e.file_id == file_id)?;
    let title = episode_meta(catalog?, series, file_id).map(|m| m.title.clone()).filter(|t| !t.is_empty());
    let code = match (ep.season, ep.number) {
        (0, _) => None,
        (s, Some(n)) => Some(format!("S{s}E{n}")),
        _ => None,
    };
    let episode = match (code, &title) {
        (Some(c), Some(t)) => format!("{c} · {t}"),
        (Some(c), None) => c,
        (None, Some(t)) => t.clone(),
        (None, None) => file_name.clone(),
    };
    Some(Titles { show: Some(show), episode: Some(episode), name: title.unwrap_or(file_name) })
}

/// TMDB metadata (title, description, still) for an episode, if known. Specials' numbering
/// rarely lines up with TMDB's, so they get none.
pub fn episode_meta<'a>(catalog: &Catalog, series: &'a HashMap<String, SeriesMeta>, file_id: i64) -> Option<&'a EpisodeMeta> {
    let entry = catalog.show_of_file(file_id)?;
    let ep = entry.episodes.iter().find(|e| e.file_id == file_id)?;
    let pid = entry.show.provider_id.as_ref()?;
    (ep.season > 0).then_some(())?;
    series.get(pid)?.episode(ep.season, ep.number?)
}
