// No console window for release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod input;
mod logging;
mod mpv;
mod player;
mod screens;
mod shared;
mod theme;
mod topbar;
mod widgets;

use std::path::PathBuf;

use eframe::egui;

/// Initialises COM on the UI thread for the life of the app (never uninitialised).
///
/// winit creates a COM taskbar object the first time the window goes fullscreen and reuses
/// it afterwards. If COM on this thread is torn down in between (libmpv initialises and
/// uninitialises COM as players come and go), that object is freed and leaving fullscreen
/// crashes with an access violation. Holding our own reference keeps COM, and the object,
/// alive.
#[cfg(windows)]
fn keep_com_alive() {
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
    let hr = unsafe { CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) };
    if hr < 0 {
        log::warn!("couldn't initialise COM on the UI thread: {hr:#x}");
    }
}

fn main() -> eframe::Result {
    logging::init();
    #[cfg(windows)]
    keep_com_alive();

    let mut windowed = false;
    let mut start = None;
    for arg in std::env::args_os().skip(1) {
        if arg == "--windowed" {
            windowed = true;
        } else {
            start = Some(PathBuf::from(arg));
        }
    }

    // Two copies would both write to the same database (and fight over the screen).
    let lock = flex_core::single_instance_lock();
    let already_running = matches!(lock, Ok(None));
    if let Err(e) = &lock {
        log::warn!("couldn't take the single-instance lock: {e}");
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Flex")
            .with_icon(std::sync::Arc::new(
                eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon-256.png"))
                    .expect("the bundled icon is a valid PNG"),
            ))
            .with_inner_size([1280.0, 720.0]),
        renderer: eframe::Renderer::Glow,
        event_loop_builder: Some(Box::new(|_builder| {
            #[cfg(windows)]
            {
                use winit::platform::windows::EventLoopBuilderExtWindows;
                _builder.with_msg_hook(input::os::hook);
            }
        })),
        ..Default::default()
    };
    eframe::run_native(
        "Flex",
        options,
        Box::new(move |cc| {
            if already_running {
                return Ok(Box::new(app::StartupError("Flex is already running.".into())) as Box<dyn eframe::App>);
            }
            Ok(match app::FlexApp::new(cc, start, windowed) {
                Ok(app) => Box::new(app) as Box<dyn eframe::App>,
                Err(e) => {
                    log::error!("{e:#}");
                    Box::new(app::StartupError(format!("{e:#}")))
                }
            })
        }),
    )
}
