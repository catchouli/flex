//! Plays an album (or a shuffled one) as a playlist: tracks follow on by themselves, and
//! the player closes after the last one, back to where it was started from.
//!
//! The track list is a normal list: Up/Down move through it, OK plays the chosen track
//! (or pauses/plays the current one). Play/Pause pauses · Left/Right seek · the remote's
//! next/previous buttons skip tracks · Back stops. The mouse can click tracks and the bar.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use eframe::egui::{self, Align2, CornerRadius, Rect, TextStyle, Ui, pos2, vec2};

use crate::images::{Size, paint_cover};
use crate::input::{Action, Press};
use crate::mpv::{self, Mpv};
use crate::screens::{Nav, Screen};
use crate::shared::Shared;
use crate::theme;
use crate::widgets::{self, Icon, ListInput, ListState, Row};

const COVER: f32 = 520.0;
const SEEK_STEP: i64 = 10;
const BAR_GRAB: f32 = 20.0;

struct Entry {
    file_id: i64,
    path: PathBuf,
    title: String,
    artists: String,
    duration: f64,
}

pub struct MusicPlayer {
    library_id: i64,
    mpv: Arc<Mpv>,
    album: String,
    album_artist: String,
    year: Option<i32>,
    cover: Option<String>,
    /// In play order (the playlist).
    entries: Vec<Entry>,
    list: ListState,
    /// Playlist position last seen, to move the selection along with playback.
    current: Option<usize>,
    started: bool,
    /// While the seek bar is held: where to.
    drag: Option<f64>,
}

impl MusicPlayer {
    /// Plays album `key` of a music library, from track `start` (in album order), or all
    /// of it shuffled.
    pub fn open(sh: &Shared, library_id: i64, key: &str, start: usize, shuffle: bool) -> Result<Self> {
        let state = sh.lib(library_id).ok_or_else(|| anyhow::anyhow!("that library is gone"))?;
        let music = state.music.as_ref().ok_or_else(|| anyhow::anyhow!("the music is still being read"))?;
        let (_, album) = music.album(key).ok_or_else(|| anyhow::anyhow!("that album is gone"))?;
        let mut order: Vec<usize> = (0..album.tracks.len()).collect();
        let mut first = start.min(order.len().saturating_sub(1));
        if shuffle {
            shuffle_in_place(&mut order);
            first = 0;
        }
        let entries: Vec<Entry> = order
            .iter()
            .filter_map(|&i| {
                let track = &album.tracks[i];
                let file = state.tree.file(track.file_id)?;
                let artists = if track.artists.len() == 1 && track.artists[0].eq_ignore_ascii_case(&album.artist) {
                    String::new()
                } else {
                    track.artists.join(", ")
                };
                Some(Entry {
                    file_id: track.file_id,
                    path: file.abs_path(&state.lib.root),
                    title: track.title.clone(),
                    artists,
                    duration: track.duration,
                })
            })
            .collect();
        // Audio only: embedded cover art would otherwise count as a video to show.
        let mpv = Arc::new(Mpv::new(&sh.ctx, &[("vo", "null"), ("vid", "no"), ("video-sync", "audio")])?);
        // Leaving idle, mpv starts at `playlist-start` (it ignored jumps made before then).
        mpv.set_string("playlist-start", &first.to_string())?;
        for (i, e) in entries.iter().enumerate() {
            let mode = if i + 1 == entries.len() { "append-play" } else { "append" };
            mpv.command(&["loadfile", &e.path.to_string_lossy(), mode])?;
        }
        log::info!("playing {} — {} from track {}", album.artist, album.title, first + 1);
        let mut list = ListState::default();
        list.select(first);
        Ok(Self {
            library_id,
            mpv,
            album: album.title.clone(),
            album_artist: album.artist.clone(),
            year: album.year,
            cover: album.cover.as_ref().map(|c| format!("file:{c}")),
            entries,
            list,
            current: None,
            started: false,
            drag: None,
        })
    }

    fn command(&self, args: &[&str]) {
        if let Err(e) = self.mpv.command(args) {
            log::warn!("{e:#}");
        }
    }

    fn position(&self) -> Option<usize> {
        self.mpv.get_i64("playlist-pos").and_then(|p| usize::try_from(p).ok())
    }

    fn paused(&self) -> bool {
        self.mpv.get_flag("pause").unwrap_or(false)
    }

    fn play_index(&self, i: usize) {
        self.command(&["playlist-play-index", &i.to_string()]);
        let _ = self.mpv.set_flag("pause", false);
    }

    fn draw_now_playing(&mut self, ui: &Ui, sh: &mut Shared, rect: Rect) {
        let style = ui.style().clone();
        let painter = ui.painter();
        let cover = Rect::from_min_size(rect.min, vec2(COVER, COVER));
        match self.cover.as_deref().and_then(|c| sh.images.get(c, Size::Poster)) {
            Some(t) => paint_cover(painter, cover, &t),
            None => {
                painter.rect_filled(cover, CornerRadius::same(8), theme::panel());
                let galley = painter.layout(self.album.clone(), TextStyle::Body.resolve(&style), theme::dim(), COVER - 60.0);
                painter.galley(cover.center() - galley.size() / 2.0, galley, theme::dim());
            }
        }
        let entry = self.current.and_then(|i| self.entries.get(i));
        let text = Rect::from_min_max(pos2(rect.left(), cover.bottom() + 28.0), pos2(cover.right(), rect.bottom()));
        let clip = painter.with_clip_rect(text);
        let title = entry.map_or("", |e| e.title.as_str());
        clip.text(text.left_top(), Align2::LEFT_TOP, title, TextStyle::Body.resolve(&style), theme::text());
        let by = match entry {
            Some(e) if !e.artists.is_empty() => e.artists.clone(),
            _ => self.album_artist.clone(),
        };
        clip.text(text.left_top() + vec2(0.0, 44.0), Align2::LEFT_TOP, by, TextStyle::Small.resolve(&style), theme::text());
        let album = match self.year {
            Some(y) => format!("{} · {y}", self.album),
            None => self.album.clone(),
        };
        clip.text(text.left_top() + vec2(0.0, 78.0), Align2::LEFT_TOP, album, TextStyle::Small.resolve(&style), theme::dim());

        // Seek bar with times, clickable and draggable like the video player's.
        let bar = Rect::from_min_size(text.left_top() + vec2(0.0, 140.0), vec2(COVER, 10.0));
        let duration = self.mpv.get_f64("duration").or(entry.map(|e| e.duration)).unwrap_or(0.0);
        let response = ui.interact(bar.expand2(vec2(0.0, BAR_GRAB)), egui::Id::new("flex-music-seek"), egui::Sense::click_and_drag());
        let at = |x: f32| duration * ((x - bar.left()) / bar.width()).clamp(0.0, 1.0) as f64;
        if duration > 0.0 && response.is_pointer_button_down_on() {
            if let Some(p) = response.interact_pointer_pos() {
                self.drag = Some(at(p.x));
            }
        } else if let Some(target) = self.drag.take() {
            let target = ui.input(|i| i.pointer.latest_pos()).map_or(target, |p| at(p.x));
            self.command(&["seek", &format!("{target:.3}"), "absolute"]);
        }
        let pos = self.drag.or_else(|| self.mpv.get_f64("time-pos")).unwrap_or(0.0);
        painter.rect_filled(bar, CornerRadius::same(5), theme::text().gamma_multiply(0.22));
        if duration > 0.0 {
            let mut done = bar;
            done.set_width(bar.width() * (pos / duration).clamp(0.0, 1.0) as f32);
            painter.rect_filled(done, CornerRadius::same(5), theme::progress());
            if response.hovered() || self.drag.is_some() {
                painter.circle_filled(pos2(done.right(), bar.center().y), 12.0, theme::progress());
            }
        }
        let small = TextStyle::Small.resolve(&style);
        painter.text(bar.left_bottom() + vec2(0.0, 14.0), Align2::LEFT_TOP, theme::format_time(pos), small.clone(), theme::dim());
        painter.text(bar.right_bottom() + vec2(0.0, 14.0), Align2::RIGHT_TOP, theme::format_time(duration), small, theme::dim());

        // Play/pause button: shows what clicking it does.
        let c = bar.center() + vec2(0.0, 96.0);
        let button = ui.interact(Rect::from_center_size(c, vec2(84.0, 84.0)), egui::Id::new("flex-music-play"), egui::Sense::click());
        if button.clicked() {
            let _ = self.mpv.set_flag("pause", !self.paused());
        }
        let (fill, icon) = if button.hovered() { (theme::accent(), theme::on_accent()) } else { (theme::panel(), theme::text()) };
        painter.circle_filled(c, 42.0, fill);
        if self.paused() {
            let play = vec![c + vec2(-10.0, -16.0), c + vec2(18.0, 0.0), c + vec2(-10.0, 16.0)];
            painter.add(egui::Shape::convex_polygon(play, icon, egui::Stroke::NONE));
        } else {
            // Two bars (fonts don't reliably have a pause glyph).
            for x in [-12.0, 4.0] {
                painter.rect_filled(Rect::from_min_size(c + vec2(x, -16.0), vec2(8.0, 32.0)), CornerRadius::same(2), icon);
            }
        }
    }
}

/// A Fisher–Yates shuffle seeded from the clock (good enough for a playlist).
fn shuffle_in_place(items: &mut [usize]) {
    let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
    for i in (1..items.len()).rev() {
        // xorshift64
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        items.swap(i, (seed % (i as u64 + 1)) as usize);
    }
}

impl Screen for MusicPlayer {
    fn handle(&mut self, _sh: &mut Shared, press: Press) -> Nav {
        match press.action {
            Action::Back | Action::Stop if !press.repeat => return Nav::Pop,
            Action::PlayPause if !press.repeat => {
                let _ = self.mpv.set_flag("pause", !self.paused());
            }
            Action::Next if !press.repeat => self.command(&["playlist-next"]),
            Action::Previous if !press.repeat => self.command(&["playlist-prev"]),
            Action::Left | Action::Right => {
                let step = if press.repeat { 3 * SEEK_STEP } else { SEEK_STEP };
                let step = if press.action == Action::Left { -step } else { step };
                self.command(&["seek", &step.to_string(), "relative"]);
            }
            _ => match self.list.handle(press, self.entries.len()) {
                ListInput::Activate(i) if Some(i) == self.current => {
                    let _ = self.mpv.set_flag("pause", !self.paused());
                }
                ListInput::Activate(i) => self.play_index(i),
                ListInput::AtTop | ListInput::None => {}
            },
        }
        Nav::None
    }

    fn ui(&mut self, ui: &mut Ui, sh: &mut Shared, _rect: Rect, focused: bool) -> Nav {
        // Full screen without the top bar, so keep to the TV-safe area ourselves.
        let rect = theme::safe_rect(ui.ctx());
        for event in self.mpv.poll_events() {
            match event {
                mpv::Event::FileLoaded => self.started = true,
                mpv::Event::EndFile { error: Some(e), .. } => sh.toast(format!("Couldn't play a track: {e}")),
                mpv::Event::Shutdown => return Nav::Pop,
                mpv::Event::EndFile { .. } => {}
            }
        }
        // The playlist ran out: back to the album.
        if self.started && self.mpv.get_flag("idle-active").unwrap_or(false) {
            return Nav::Pop;
        }
        let position = self.position();
        if position != self.current {
            // Follow playback with the selection unless it was moved elsewhere.
            if position.is_some() && (self.current.is_none() || self.list.selected == self.current.unwrap_or(0)) {
                self.list.select(position.unwrap_or(0));
            }
            self.current = position;
            if let Some(entry) = position.and_then(|i| self.entries.get(i)) {
                sh.mark_played(self.library_id, entry.file_id);
            }
        }

        let left = Rect::from_min_size(rect.min, vec2(COVER, rect.height()));
        self.draw_now_playing(ui, sh, left);
        let right = Rect::from_min_max(pos2(left.right() + 60.0, rect.top()), rect.max);
        let rows: Vec<Row> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let title = format!("{}. {}", i + 1, e.title);
                let time = theme::format_time(e.duration);
                let detail = match (Some(i) == self.current, e.artists.is_empty()) {
                    (true, _) => format!("Playing · {time}"),
                    (false, true) => time,
                    (false, false) => format!("{} · {time}", e.artists),
                };
                let icon = if Some(i) == self.current { Icon::Video } else { Icon::None };
                Row::new(icon, title).detail(detail)
            })
            .collect();
        widgets::list(ui, right, &rows, &mut self.list, focused);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        Nav::None
    }

    fn shows_top_bar(&self) -> bool {
        false
    }

    /// Counts as the player: hiding to the tray stops it, and updates wait for it.
    fn is_player(&self) -> bool {
        true
    }

    fn playback(&self) -> Option<flex_plugin::Playback> {
        let entry = self.position().and_then(|i| self.entries.get(i)).filter(|_| self.started)?;
        let state = if self.paused() { flex_plugin::PlaybackState::Paused } else { flex_plugin::PlaybackState::Playing };
        let title = format!("{} · {}", entry.artists, entry.title);
        let position = self.mpv.get_f64("time-pos");
        Some(flex_plugin::Playback { state, title, kind: "music".into(), position, duration: Some(entry.duration), media: None })
    }

    /// Clicks act on the list and bar, never as the video player's click-to-pause.
    fn on_controls(&self, _pos: egui::Pos2) -> bool {
        true
    }
}
