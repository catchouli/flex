// No console window for release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod ambient;
mod app;
mod awake;
#[cfg(windows)]
mod autostart;
mod images;
mod input;
mod instance;
mod logging;
mod mpv;
mod music_player;
mod picture;
mod player;
mod scenery;
mod screens;
mod shared;
mod sync;
mod shots;
mod theme;
mod topbar;
mod tray;
mod update;
mod volume;
mod youtube;
mod widgets;

use std::path::PathBuf;

use eframe::egui;

/// Initialises COM on the UI thread for the life of the app (never uninitialised), and
/// keeps the taskbar's COM library loaded for good.
///
/// winit creates a COM taskbar object (from explorerframe.dll) the first time the window
/// goes fullscreen and reuses it afterwards. Crash dumps from the TV PC showed that
/// explorerframe.dll still gets unloaded in between, even with COM kept alive here, and
/// the next fullscreen change then crashes calling into it. Pinning the DLL means it can't
/// be unloaded, whatever asks for that.
#[cfg(windows)]
fn keep_com_alive() {
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
    use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW, LoadLibraryW};
    let hr = unsafe { CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) };
    if hr < 0 {
        log::warn!("couldn't initialise COM on the UI thread: {hr:#x}");
    }
    let name: Vec<u16> = "explorerframe.dll".encode_utf16().chain([0]).collect();
    let mut module = std::ptr::null_mut();
    // Safety: `name` is a NUL-terminated wide string that outlives both calls.
    let pinned = unsafe { !LoadLibraryW(name.as_ptr()).is_null() && GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_PIN, name.as_ptr(), &mut module) != 0 };
    if !pinned {
        log::warn!("couldn't keep explorerframe.dll loaded: leaving fullscreen may crash");
    }
}

/// `--test-crash`: crashes a few seconds after starting (once settings are loaded), to
/// check crash reports work. A native crash on Windows, a panic elsewhere.
fn start_test_crash() {
    std::thread::Builder::new()
        .name("test-crash".into())
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(5));
            log::warn!("crashing on purpose (--test-crash)");
            if cfg!(windows) {
                // Safety: none; this is the point.
                unsafe { std::ptr::null_mut::<u8>().write_volatile(1) };
            }
            // A panic on this thread alone wouldn't stop Flex: report it, then abort.
            let _ = std::panic::catch_unwind(|| panic!("crashing on purpose (--test-crash)"));
            std::process::abort();
        })
        .expect("spawning a thread");
}

fn main() -> eframe::Result {
    let mut windowed = false;
    let mut hidden = false;
    let mut quit = false;
    let mut test_crash = false;
    let mut start = None;
    for arg in std::env::args_os().skip(1) {
        if arg == "--windowed" {
            windowed = true;
        } else if arg == "--tray" {
            hidden = true;
        } else if arg == "--quit" {
            quit = true;
        } else if arg == "--test-crash" {
            test_crash = true;
        } else {
            start = Some(PathBuf::from(arg));
        }
    }

    // Two copies would both write to the same database (and fight over the screen), so a
    // second launch shows the running copy's window instead.
    // Taken before logging starts, which would otherwise rotate the running copy's log.
    let lock = flex_core::single_instance_lock();
    let already_running = matches!(lock, Ok(None));
    if quit {
        if already_running && let Err(e) = instance::send(instance::Command::Quit) {
            eprintln!("couldn't reach the running Flex: {e:#}");
        }
        return Ok(());
    }
    if already_running {
        if hidden {
            return Ok(()); // started with Windows, but Flex is already up
        }
        match instance::send(instance::Command::Show) {
            Ok(()) => return Ok(()),
            Err(e) => eprintln!("couldn't reach the running Flex: {e:#}"),
        }
    } else {
        logging::init();
    }
    if let Err(e) = &lock {
        log::warn!("couldn't take the single-instance lock: {e}");
    }
    #[cfg(windows)]
    keep_com_alive();
    if test_crash {
        start_test_crash();
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
            Ok(match app::FlexApp::new(cc, start, windowed, hidden) {
                Ok(app) => Box::new(app) as Box<dyn eframe::App>,
                Err(e) => {
                    log::error!("{e:#}");
                    Box::new(app::StartupError(format!("{e:#}")))
                }
            })
        }),
    )
}
