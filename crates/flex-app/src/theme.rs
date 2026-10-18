//! TV ("10-foot") look: large type, high-contrast focus, and a fixed 1080-point-tall
//! layout that scales to whatever resolution the screen actually is.
//!
//! The look itself comes from a theme: colours, how panels (rows, buttons, cards) are
//! drawn, how the focused one stands out, the background, titles and text shadows.
//! Built-in themes live in `crates/flex-app/themes/*.toml`; custom ones are folders in
//! the data folder's `themes`, each with a `theme.toml` (same format) and any images or
//! font it names. See THEMES.md.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use eframe::egui::{self, Align2, Color32, CornerRadius, FontFamily, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, TextStyle, TextureHandle, pos2, vec2};
use serde::Deserialize;

/// Layouts are designed for a screen this many points tall.
pub const DESIGN_HEIGHT: f32 = 1080.0;
/// Fraction of each edge kept clear in case the TV overscans.
pub const SAFE_MARGIN: f32 = 0.04;

/// System fonts used as fallbacks for CJK titles, which egui's bundled fonts don't cover.
/// All that exist are loaded, in order.
#[cfg(windows)]
const FALLBACK_FONTS: &[&str] =
    &[r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\YuGothM.ttc", r"C:\Windows\Fonts\malgun.ttf"];
#[cfg(target_os = "macos")]
const FALLBACK_FONTS: &[&str] = &[
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
];
#[cfg(not(any(windows, target_os = "macos")))]
const FALLBACK_FONTS: &[&str] = &["/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"];

// ---------------------------------------------------------------------------------------
// Theme files

/// A colour in a theme file: "#rrggbb" or "#rrggbbaa".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colour(pub Color32);

impl<'de> Deserialize<'de> for Colour {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        parse_colour(&s).map(Colour).ok_or_else(|| serde::de::Error::custom(format!("not a colour: {s:?} (use \"#rrggbb\" or \"#rrggbbaa\")")))
    }
}

fn parse_colour(s: &str) -> Option<Color32> {
    let hex = s.trim().strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        6 => Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => None,
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Colours {
    /// Behind everything.
    pub background: Colour,
    /// Titles and text drawn on the background.
    pub text: Colour,
    pub dim: Colour,
    /// Rows, buttons and cards.
    pub panel: Colour,
    /// Text on panels (defaults to `text`), and its quieter version (defaults to `dim`).
    pub panel_text: Option<Colour>,
    pub panel_dim: Option<Colour>,
    /// Focus, progress bars and highlights.
    pub accent: Colour,
    /// Text on an accent-filled (focused) panel.
    pub on_accent: Colour,
    /// Focused text and pictures on the background, e.g. a focused poster and its title
    /// (defaults to `accent`).
    pub highlight: Option<Colour>,
    /// Progress bars (defaults to `accent`).
    pub progress: Option<Colour>,
    /// Behind messages and the player's controls.
    pub overlay: Colour,
    pub overlay_text: Option<Colour>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Line {
    pub width: f32,
    pub colour: Colour,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PanelStyle {
    #[serde(default = "default_radius")]
    pub radius: f32,
    /// Fades the panel from `panel` at the top to this at the bottom.
    pub gradient: Option<Colour>,
    pub border: Option<Line>,
    /// A second border inside the first (with a gap of `inner_gap`).
    pub inner_border: Option<Line>,
    #[serde(default)]
    pub inner_gap: f32,
    /// Raised 3D edges: light top/left, dark bottom/right.
    pub bevel: Option<Bevel>,
    /// A glossy sheen over the top half, fading out (use a see-through white).
    pub gloss: Option<Colour>,
}

fn default_radius() -> f32 {
    8.0
}

#[derive(Clone, Debug, Deserialize)]
pub struct Bevel {
    pub light: Colour,
    pub dark: Colour,
    #[serde(default = "two")]
    pub width: f32,
}

fn two() -> f32 {
    2.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusKind {
    /// The focused panel is filled with the accent colour.
    #[default]
    Fill,
    /// It keeps its colour, with an accent border and accent text.
    Outline,
    /// It keeps its look; a pointer points at it from the left.
    Pointer,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct FocusStyle {
    #[serde(default)]
    pub style: FocusKind,
    /// For `fill`: fades the fill from `accent` at the top to this at the bottom.
    pub gradient: Option<Colour>,
    /// A ring drawn inside the focused panel.
    pub ring: Option<Line>,
    /// For `pointer`: "hand", "triangle", or an image file in the theme's folder.
    pub pointer: Option<String>,
    /// For `pointer`: what the pointer is drawn in (defaults to the panel text colour).
    pub pointer_colour: Option<Colour>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderKind {
    #[default]
    Plain,
    /// A title bar behind the screen's title.
    Bar,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct HeaderStyle {
    #[serde(default)]
    pub style: HeaderKind,
    pub colour: Option<Colour>,
    /// For a bar: fades to this colour at the bottom.
    pub gradient: Option<Colour>,
    /// For a bar: its corner radius.
    #[serde(default)]
    pub radius: f32,
    pub text: Option<Colour>,
    /// Text before and after every title, e.g. "> " and "_".
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct BackgroundStyle {
    /// Fades from `background` at the top to this at the bottom.
    pub gradient: Option<Colour>,
    /// A built-in pattern drawn over it: "stone", "pinstripes", "rain" (falling code)
    /// or "hills" (rolling hills, in `hill` fading to `hill_bottom`).
    pub pattern: Option<String>,
    pub hill: Option<Colour>,
    pub hill_bottom: Option<Colour>,
    /// An image in the theme's folder, covering the screen.
    pub image: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct TextStyleConfig {
    /// A shadow behind text, offset down and right by `shadow_offset`.
    pub shadow: Option<Colour>,
    #[serde(default = "two")]
    pub shadow_offset: f32,
    /// Whether text on panels gets the shadow too.
    #[serde(default = "yes")]
    pub panel_shadow: bool,
    /// A font file in the theme's folder used for all text, or "monospace" for Flex's
    /// own fixed-width font.
    pub font: Option<String>,
}

/// A theme, as written in a `theme.toml`.
#[derive(Clone, Debug, Deserialize)]
pub struct Theme {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Dark themes get egui's dark widgets (text boxes); light ones its light widgets.
    #[serde(default = "yes")]
    pub dark: bool,
    pub colours: Colours,
    #[serde(default)]
    pub panel: PanelStyle,
    #[serde(default)]
    pub focus: FocusStyle,
    #[serde(default)]
    pub header: HeaderStyle,
    #[serde(default)]
    pub background: BackgroundStyle,
    #[serde(default)]
    pub text: TextStyleConfig,
    /// Its id (folder name, or built-in name) and folder (for images), set when loaded.
    #[serde(skip)]
    pub id: String,
    #[serde(skip)]
    pub dir: Option<PathBuf>,
}

fn yes() -> bool {
    true
}

impl Theme {
    pub fn panel_text(&self) -> Color32 {
        self.colours.panel_text.unwrap_or(self.colours.text).0
    }

    pub fn panel_dim(&self) -> Color32 {
        self.colours.panel_dim.unwrap_or(self.colours.dim).0
    }
}

/// The themes that come with Flex, in the order Settings lists them.
const BUILT_IN: [(&str, &str); 10] = [
    ("dark", include_str!("../themes/dark.toml")),
    ("ff7", include_str!("../themes/ff7.toml")),
    ("osrs", include_str!("../themes/osrs.toml")),
    ("pokemon", include_str!("../themes/pokemon.toml")),
    ("win95", include_str!("../themes/win95.toml")),
    ("winxp", include_str!("../themes/winxp.toml")),
    ("aqua", include_str!("../themes/aqua.toml")),
    ("terminal", include_str!("../themes/terminal.toml")),
    ("templeos", include_str!("../themes/templeos.toml")),
    ("farmyard", include_str!("../themes/farmyard.toml")),
];

pub const DEFAULT_THEME: &str = "dark";

pub fn parse(id: &str, toml_text: &str, dir: Option<&Path>) -> Result<Theme, String> {
    let mut theme: Theme = toml::from_str(toml_text).map_err(|e| format!("{e}"))?;
    theme.id = id.to_owned();
    theme.dir = dir.map(Path::to_path_buf);
    Ok(theme)
}

/// Where custom themes go.
pub fn themes_dir() -> PathBuf {
    flex_core::data_dir().join("themes")
}

/// Every theme: built-in ones, then custom ones from the themes folder (by folder name).
pub fn all() -> Vec<Theme> {
    let mut out: Vec<Theme> = BUILT_IN.iter().map(|(id, text)| parse(id, text, None).expect("built-in themes parse")).collect();
    if let Ok(entries) = std::fs::read_dir(themes_dir()) {
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.join("theme.toml").is_file()).collect();
        dirs.sort();
        for dir in dirs {
            let id = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
            match std::fs::read_to_string(dir.join("theme.toml")).map_err(|e| e.to_string()).and_then(|t| parse(&id, &t, Some(&dir))) {
                Ok(theme) => {
                    out.retain(|t| t.id != id);
                    out.push(theme);
                }
                Err(e) => log::warn!("theme {}: {e}", dir.display()),
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// The current theme

static CURRENT: RwLock<Option<Arc<Theme>>> = RwLock::new(None);
/// Textures for the current theme's images and patterns, by name.
static TEXTURES: RwLock<Option<HashMap<String, TextureHandle>>> = RwLock::new(None);

pub fn current() -> Arc<Theme> {
    if let Some(theme) = CURRENT.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return theme.clone();
    }
    let dark = Arc::new(parse(DEFAULT_THEME, BUILT_IN[0].1, None).expect("built-in themes parse"));
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(dark.clone());
    dark
}

pub fn bg() -> Color32 {
    current().colours.background.0
}
pub fn text() -> Color32 {
    current().colours.text.0
}
pub fn dim() -> Color32 {
    current().colours.dim.0
}
pub fn panel() -> Color32 {
    current().colours.panel.0
}
pub fn panel_dim() -> Color32 {
    current().panel_dim()
}
pub fn accent() -> Color32 {
    current().colours.accent.0
}
pub fn on_accent() -> Color32 {
    current().colours.on_accent.0
}
pub fn highlight() -> Color32 {
    let t = current();
    t.colours.highlight.unwrap_or(t.colours.accent).0
}
pub fn progress() -> Color32 {
    let t = current();
    t.colours.progress.unwrap_or(t.colours.accent).0
}
pub fn overlay() -> Color32 {
    current().colours.overlay.0
}
pub fn overlay_text() -> Color32 {
    let t = current();
    t.colours.overlay_text.unwrap_or(t.colours.text).0
}

/// Switches to `theme`: colours, egui's widget style, fonts and textures.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    let mut textures = HashMap::new();
    let load = |name: &str, image: egui::ColorImage, repeat: bool| {
        let wrap = if repeat { egui::TextureWrapMode::Repeat } else { egui::TextureWrapMode::ClampToEdge };
        // Patterns keep their crisp pixels.
        let filter = if repeat { egui::TextureOptions::NEAREST } else { egui::TextureOptions::LINEAR };
        let options = egui::TextureOptions { wrap_mode: wrap, ..filter };
        ctx.load_texture(format!("theme-{}-{name}", theme.id), image, options)
    };
    if let Some(pattern) = &theme.background.pattern
        && let Some(image) = crate::scenery::make_pattern(pattern, theme.colours.background.0) {
            textures.insert("pattern".to_owned(), load("pattern", image, true));
        }
    for (key, file) in [("background", &theme.background.image), ("pointer", &theme.focus.pointer)] {
        let Some(file) = file.as_ref().filter(|f| f.contains('.')) else { continue };
        let Some(dir) = &theme.dir else { continue };
        match read_image(&dir.join(file)) {
            Ok(image) => {
                textures.insert(key.to_owned(), load(key, image, false));
            }
            Err(e) => log::warn!("theme {}: {file}: {e}", theme.id),
        }
    }
    let font = match theme.text.font.as_deref() {
        Some("monospace") => ThemeFont::Monospace,
        Some(f) => match theme.dir.as_ref().map(|dir| std::fs::read(dir.join(f))) {
            Some(Ok(bytes)) => ThemeFont::File(bytes),
            Some(Err(e)) => {
                log::warn!("theme font {f}: {e}");
                ThemeFont::Default
            }
            None => ThemeFont::Default,
        },
        None => ThemeFont::Default,
    };
    install_fonts(ctx, font);
    // egui draws only the text boxes itself; they look like panels.
    let (bg, dark) = (theme.colours.background.0, theme.dark);
    let (panel, panel_text, panel_dim, accent) = (theme.colours.panel.0, theme.panel_text(), theme.panel_dim(), theme.colours.accent.0);
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        *v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.override_text_color = Some(panel_text);
        v.weak_text_color = Some(panel_dim);
        v.panel_fill = bg;
        v.extreme_bg_color = panel;
        v.text_cursor.stroke.color = panel_text;
        v.selection.bg_fill = accent.gamma_multiply(0.5);
    });
    log::info!("theme: {}", theme.name);
    *TEXTURES.write().unwrap_or_else(|e| e.into_inner()) = Some(textures);
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(theme));
    ctx.request_repaint();
}

fn read_image(path: &Path) -> Result<egui::ColorImage, String> {
    let image = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(size, &image))
}

fn texture(name: &str) -> Option<TextureHandle> {
    TEXTURES.read().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(name).cloned()
}

// ---------------------------------------------------------------------------------------
// Drawing in the current theme

/// A vertical gradient (or flat fill when `bottom` is `None`).
fn gradient(painter: &Painter, rect: Rect, top: Color32, bottom: Option<Color32>) {
    rounded_gradient(painter, rect, 0.0, top, bottom);
}

/// A rounded rectangle fading from `top` to `bottom` (flat when `bottom` is `None`).
fn rounded_gradient(painter: &Painter, rect: Rect, radius: f32, top: Color32, bottom: Option<Color32>) {
    let Some(bottom) = bottom else {
        painter.rect_filled(rect, CornerRadius::same(radius.round().clamp(0.0, 255.0) as u8), top);
        return;
    };
    let colour_at = |y: f32| crate::scenery::lerp(top, bottom, (y - rect.top()) / rect.height().max(1.0));
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0).max(0.0);
    // The outline, corner arcs clockwise from the top left, as a fan around the centre.
    let mut outline = Vec::new();
    const SEGMENTS: usize = 6;
    for (corner, start) in [(rect.left_top() + vec2(r, r), 180.0_f32), (rect.right_top() + vec2(-r, r), 270.0), (rect.right_bottom() - vec2(r, r), 0.0), (rect.left_bottom() + vec2(r, -r), 90.0)] {
        for i in 0..=SEGMENTS {
            let a = (start + 90.0 * i as f32 / SEGMENTS as f32).to_radians();
            outline.push(corner + vec2(a.cos(), a.sin()) * r);
        }
    }
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.center(), colour_at(rect.center().y));
    for p in &outline {
        mesh.colored_vertex(*p, colour_at(p.y));
    }
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(mesh);
}

/// The background, filling `rect`.
pub fn paint_background(painter: &Painter, rect: Rect) {
    let t = current();
    let b = &t.background;
    gradient(painter, rect, t.colours.background.0, b.gradient.map(|c| c.0));
    if let Some(pattern) = &b.pattern {
        if let Some(tex) = texture("pattern") {
            let s = crate::scenery::pattern_scale(pattern);
            let [w, h] = tex.size();
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(rect.width() / (s * w as f32), rect.height() / (s * h as f32)));
            painter.image(tex.id(), rect, uv, Color32::WHITE);
        } else {
            let hill = b.hill.map_or(Color32::from_rgb(0x6d, 0xbb, 0x3a), |c| c.0);
            let hill_bottom = b.hill_bottom.map_or(Color32::from_rgb(0x2f, 0x7d, 0x1c), |c| c.0);
            crate::scenery::paint_scene(painter, rect, pattern, hill, hill_bottom);
        }
    }
    if let Some(tex) = texture("background") {
        let [w, h] = tex.size();
        // Cover the screen, cropping the image's overflow.
        let (sx, sy) = (rect.width() / w as f32, rect.height() / h as f32);
        let scale = sx.max(sy);
        let (cw, ch) = (rect.width() / scale / w as f32, rect.height() / scale / h as f32);
        let uv = Rect::from_center_size(pos2(0.5, 0.5), vec2(cw, ch));
        painter.image(tex.id(), rect, uv, Color32::WHITE);
    }
}

/// The part of the theme's scene in front of everything (if it has one).
pub fn paint_foreground(painter: &Painter, rect: Rect) {
    if let Some(pattern) = &current().background.pattern {
        crate::scenery::paint_scene_front(painter, rect, pattern);
    }
}

/// How much of the bottom of the screen to keep clear for the foreground.
pub fn foreground_height() -> f32 {
    current().background.pattern.as_deref().map_or(0.0, crate::scenery::front_height)
}

/// A panel (row, button, card) in its state; returns the colour for its text.
pub fn paint_panel(painter: &Painter, rect: Rect, focused: bool, selected: bool) -> Color32 {
    let t = current();
    let p = &t.panel;
    let radius = CornerRadius::same(p.radius.round().clamp(0.0, 255.0) as u8);
    if focused && t.focus.style == FocusKind::Fill {
        rounded_gradient(painter, rect, p.radius, t.colours.accent.0, t.focus.gradient.map(|c| c.0));
    } else {
        rounded_gradient(painter, rect, p.radius, t.colours.panel.0, p.gradient.map(|c| c.0));
    }
    if let Some(gloss) = p.gloss {
        // A sheen over the top half, fading out towards the middle.
        let inset = p.border.as_ref().map_or(0.0, |b| b.width) + 1.0;
        let sheen = Rect::from_min_max(rect.min + vec2(inset, inset), pos2(rect.right() - inset, rect.center().y));
        rounded_gradient(painter, sheen, p.radius - inset, gloss.0, Some(gloss.0.gamma_multiply(0.15)));
    }
    if let Some(b) = &p.bevel {
        let w = b.width;
        let (tl, br) = (rect.left_top(), rect.right_bottom());
        let light = Stroke::new(w, b.light.0);
        let dark = Stroke::new(w, b.dark.0);
        let h = w / 2.0;
        painter.line_segment([pos2(tl.x, tl.y + h), pos2(br.x - w, tl.y + h)], light);
        painter.line_segment([pos2(tl.x + h, tl.y), pos2(tl.x + h, br.y - w)], light);
        painter.line_segment([pos2(tl.x, br.y - h), pos2(br.x, br.y - h)], dark);
        painter.line_segment([pos2(br.x - h, tl.y), pos2(br.x - h, br.y)], dark);
    }
    if let Some(b) = &p.border {
        painter.rect_stroke(rect, radius, Stroke::new(b.width, b.colour.0), StrokeKind::Inside);
    }
    if let Some(b) = &p.inner_border {
        let inset = p.border.as_ref().map_or(0.0, |o| o.width) + p.inner_gap;
        let inner_radius = CornerRadius::same((p.radius - inset).max(0.0).round() as u8);
        painter.rect_stroke(rect.shrink(inset), inner_radius, Stroke::new(b.width, b.colour.0), StrokeKind::Inside);
    }
    if focused {
        if t.focus.style == FocusKind::Outline {
            painter.rect_stroke(rect, radius, Stroke::new(3.0, t.colours.accent.0), StrokeKind::Inside);
        }
        if t.focus.style == FocusKind::Pointer || t.focus.pointer.is_some() {
            paint_pointer(painter, rect);
        }
        if let Some(ring) = &t.focus.ring {
            painter.rect_stroke(rect, radius, Stroke::new(ring.width, ring.colour.0), StrokeKind::Inside);
        }
    } else if selected && t.focus.style != FocusKind::Pointer {
        // Selected but the list isn't focused (e.g. the top bar has focus).
        painter.rect_stroke(rect, radius, Stroke::new(2.0, t.colours.accent.0), StrokeKind::Inside);
    }
    match (focused, t.focus.style) {
        (true, FocusKind::Fill) => t.colours.on_accent.0,
        (true, FocusKind::Outline) => t.colours.accent.0,
        _ => t.panel_text(),
    }
}

/// The theme's frame around a picture (poster, thumbnail) and its focus: a ring in the
/// highlight colour, and the pointer if the theme has one.
pub fn paint_picture_frame(painter: &Painter, rect: Rect, focused: bool) {
    let t = current();
    let radius = CornerRadius::same(8);
    if let Some(b) = &t.panel.border {
        painter.rect_stroke(rect, radius, Stroke::new(b.width, b.colour.0), StrokeKind::Inside);
    }
    if !focused {
        return;
    }
    if t.focus.style != FocusKind::Pointer {
        painter.rect_stroke(rect, radius, Stroke::new(5.0, highlight()), StrokeKind::Inside);
    }
    if t.focus.style == FocusKind::Pointer || t.focus.pointer.is_some() {
        paint_pointer(painter, rect);
    }
}

/// A progress bar on a panel: it has to show on the accent fill of a focused one.
pub fn progress_on_panel(focused: bool) -> Color32 {
    if focused && current().focus.style == FocusKind::Fill { on_accent() } else { progress() }
}

/// The pointer, pointing at `rect` from its left. Drawn on a layer above lists, so it
/// isn't clipped to them.
pub fn paint_pointer(painter: &Painter, rect: Rect) {
    // Not for rows scrolled out of their list.
    if !painter.clip_rect().contains(rect.center()) {
        return;
    }
    let t = current();
    let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("theme-pointer"));
    let p = painter.ctx().layer_painter(layer).with_clip_rect(painter.ctx().content_rect());
    let colour = t.focus.pointer_colour.map_or_else(|| t.panel_text(), |c| c.0);
    let tip = pos2(rect.left() - 6.0, rect.center().y);
    if let Some(tex) = texture("pointer") {
        let [w, h] = tex.size();
        let height = 40.0;
        let size = vec2(height * w as f32 / h as f32, height);
        p.image(tex.id(), Rect::from_min_size(tip - vec2(size.x, size.y / 2.0), size), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        return;
    }
    match t.focus.pointer.as_deref() {
        Some("triangle") => {
            let points = vec![tip - vec2(22.0, 14.0), tip, tip - vec2(22.0, -14.0)];
            p.add(egui::Shape::convex_polygon(points, colour, Stroke::NONE));
        }
        Some("hen") => crate::scenery::hen_pointer(&p, tip),
        _ => paint_hand(&p, tip, colour),
    }
}
/// A gloved hand pointing right, its fingertip at `tip` (an original drawing).
fn paint_hand(p: &Painter, tip: Pos2, fill: Color32) {
    const S: f32 = 1.35;
    let at = |x: f32, y: f32| tip + vec2(x * S, y * S);
    let rect = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(at(x, y), vec2(w * S, h * S));
    let outline = Stroke::new(2.5, Color32::from_rgb(40, 40, 48));
    let shade = Color32::from_rgb((fill.r() as f32 * 0.72) as u8, (fill.g() as f32 * 0.72) as u8, (fill.b() as f32 * 0.8) as u8);
    // Cuff, palm (with curled fingers), thumb and the pointing finger.
    let cuff = rect(-58.0, -11.0, 10.0, 22.0);
    let palm = rect(-50.0, -12.0, 26.0, 26.0);
    let thumb = rect(-36.0, -18.0, 14.0, 9.0);
    let finger = rect(-32.0, -12.0, 32.0, 10.0);
    for (r, radius, colour) in [(cuff, 3.0, shade), (palm, 9.0, fill), (thumb, 4.0, fill), (finger, 5.0, fill)] {
        p.rect(r, CornerRadius::same((radius * S) as u8), colour, outline, StrokeKind::Outside);
    }
    // The curled fingers' edges.
    for y in [1.5, 7.5] {
        p.line_segment([at(-40.0, y), at(-27.0, y)], Stroke::new(2.0, outline.color));
    }
}

/// Text with the theme's shadow (if it has one).
pub fn shadow_text(painter: &Painter, pos: Pos2, align: Align2, s: &str, font: FontId, colour: Color32) -> Rect {
    if let Some(shadow) = current().text.shadow {
        let offset = current().text.shadow_offset;
        painter.text(pos + vec2(offset, offset), align, s, font.clone(), shadow.0);
    }
    painter.text(pos, align, s, font, colour)
}

/// Text on a panel: with the theme's shadow if it shadows panel text too.
pub fn panel_shadow_text(painter: &Painter, pos: Pos2, align: Align2, s: &str, font: FontId, colour: Color32) -> Rect {
    if current().text.panel_shadow { shadow_text(painter, pos, align, s, font, colour) } else { painter.text(pos, align, s, font, colour) }
}

/// A screen's title: plain text, or on a title bar. Returns where the title went.
pub fn paint_title(painter: &Painter, rect: Rect, title: &str, font: FontId) -> Rect {
    let t = current();
    let title = &format!("{}{title}{}", t.header.prefix, t.header.suffix);
    match t.header.style {
        HeaderKind::Plain => {
            let colour = t.header.text.unwrap_or(t.colours.text).0;
            shadow_text(painter, rect.left_top(), Align2::LEFT_TOP, title, font, colour)
        }
        HeaderKind::Bar => {
            let bar = Rect::from_min_size(rect.left_top() - vec2(0.0, 4.0), vec2(rect.width(), font.size + 16.0));
            let top = t.header.colour.unwrap_or(t.colours.accent).0;
            rounded_gradient(painter, bar, t.header.radius, top, t.header.gradient.map(|c| c.0));
            let colour = t.header.text.unwrap_or(t.colours.on_accent).0;
            shadow_text(painter, bar.left_center() + vec2(14.0, 0.0), Align2::LEFT_CENTER, title, font, colour)
        }
    }
}

// ---------------------------------------------------------------------------------------
// Fonts, scale and layout

enum ThemeFont {
    Default,
    Monospace,
    File(Vec<u8>),
}

fn install_fonts(ctx: &egui::Context, theme_font: ThemeFont) {
    let mut fonts = egui::FontDefinitions::default();
    match theme_font {
        ThemeFont::Default => {}
        ThemeFont::Monospace => {
            let mono = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
            fonts.families.insert(FontFamily::Proportional, mono);
        }
        ThemeFont::File(bytes) => {
            fonts.font_data.insert("theme".to_owned(), egui::FontData::from_owned(bytes).into());
            fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "theme".to_owned());
        }
    }
    for path in FALLBACK_FONTS {
        let Ok(bytes) = std::fs::read(path) else { continue };
        fonts.font_data.insert(path.to_string(), egui::FontData::from_owned(bytes).into());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(path.to_string());
        }
    }
    ctx.set_fonts(fonts);
}

/// Sets up text sizes and the theme saved in settings (`theme_id`, or the default).
pub fn install(ctx: &egui::Context, theme_id: Option<&str>) {
    // Ctrl +/- is ours (fullscreen); the layout already scales itself to the screen.
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::new(22.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(30.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(30.0, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(52.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(24.0, FontFamily::Monospace)),
        ]
        .into();
        style.interaction.selectable_labels = false;
    });
    // FLEX_THEME=<id> tries a theme without saving it (handy while making one).
    let env = std::env::var("FLEX_THEME").ok();
    let id = env.as_deref().or(theme_id).unwrap_or(DEFAULT_THEME);
    let theme = all().into_iter().find(|t| t.id == id).unwrap_or_else(|| {
        log::warn!("theme {id:?} not found; using the default");
        parse(DEFAULT_THEME, BUILT_IN[0].1, None).expect("built-in themes parse")
    });
    apply(ctx, theme);
}

/// Keeps the logical screen height at `DESIGN_HEIGHT` points regardless of resolution or DPI.
pub fn update_scale(ctx: &egui::Context) {
    let physical_h = ctx.content_rect().height() * ctx.pixels_per_point();
    let native = ctx.native_pixels_per_point().unwrap_or(1.0);
    let zoom = (physical_h / DESIGN_HEIGHT / native).max(0.25);
    if (ctx.zoom_factor() - zoom).abs() > 0.005 {
        ctx.set_zoom_factor(zoom);
    }
}

/// The screen rect shrunk by the overscan-safe margin.
pub fn safe_rect(ctx: &egui::Context) -> egui::Rect {
    let r = ctx.content_rect();
    r.shrink2(r.size() * SAFE_MARGIN)
}

/// "12 min left", "1 h 5 min left".
pub fn format_remaining(secs: f64) -> String {
    let mins = (secs / 60.0).ceil().max(1.0) as u64;
    if mins >= 60 { format!("{} h {} min left", mins / 60, mins % 60) } else { format!("{mins} min left") }
}

pub fn format_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_themes_parse() {
        for (id, text) in BUILT_IN {
            let theme = parse(id, text, None).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(!theme.name.is_empty(), "{id}");
        }
    }

    #[test]
    fn colours_and_errors() {
        assert_eq!(parse_colour("#ff8000"), Some(Color32::from_rgb(255, 128, 0)));
        assert_eq!(parse_colour("#00000080"), Some(Color32::from_rgba_unmultiplied(0, 0, 0, 128)));
        assert_eq!(parse_colour("ff8000"), None);
        assert_eq!(parse_colour("#ff80"), None);
        let bad = "name = \"x\"\n[colours]\nbackground = \"blue\"";
        assert!(parse("x", bad, None).unwrap_err().contains("not a colour"));
    }

}
