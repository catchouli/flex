//! TV ("10-foot") look: large type, high-contrast focus, and a fixed 1080-point-tall
//! layout that scales to whatever resolution the screen actually is.

use eframe::egui::{self, Color32, FontFamily, FontId, TextStyle};

/// Layouts are designed for a screen this many points tall.
pub const DESIGN_HEIGHT: f32 = 1080.0;
/// Fraction of each edge kept clear in case the TV overscans.
pub const SAFE_MARGIN: f32 = 0.04;

pub const BG: Color32 = Color32::from_rgb(14, 15, 18);
pub const ROW: Color32 = Color32::from_rgb(28, 30, 36);
pub const FOCUS: Color32 = Color32::from_rgb(235, 170, 40);
pub const TEXT: Color32 = Color32::from_rgb(235, 235, 235);
pub const DIM: Color32 = Color32::from_rgb(150, 150, 160);
pub const OSD_BG: Color32 = Color32::from_black_alpha(190);

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

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for path in FALLBACK_FONTS {
        let Ok(bytes) = std::fs::read(path) else { continue };
        fonts.font_data.insert(path.to_string(), egui::FontData::from_owned(bytes).into());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(path.to_string());
        }
    }
    ctx.set_fonts(fonts);
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
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
        style.visuals = egui::Visuals::dark();
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.panel_fill = BG;
        style.interaction.selectable_labels = false;
    });
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
