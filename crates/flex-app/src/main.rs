// No console window for release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod browser;
mod input;
mod mpv;
mod player;
mod theme;

use std::path::PathBuf;

use eframe::egui;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut windowed = false;
    let mut start = None;
    for arg in std::env::args_os().skip(1) {
        if arg == "--windowed" {
            windowed = true;
        } else {
            start = Some(PathBuf::from(arg));
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Flex")
            .with_inner_size([1280.0, 720.0])
            .with_fullscreen(!windowed),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native("Flex", options, Box::new(|cc| Ok(Box::new(app::FlexApp::new(cc, start)))))
}
