//! The system tray icon: Flex keeps running there when its window is closed, so it opens
//! again instantly. Clicking the icon shows the window; its menu has Open, Update now (in
//! installed copies) and Quit.

use std::sync::mpsc::Sender;

use eframe::egui;

use crate::instance::Command;

/// Keeps the icon in the tray while alive.
pub struct Tray {
    #[cfg(any(windows, target_os = "macos"))]
    _icon: tray_icon::TrayIcon,
}

/// Adds the tray icon, or returns None where there's no tray (closing the window then
/// quits as before). Call on the UI thread once the event loop is running.
pub fn create(commands: Sender<Command>, ctx: &egui::Context) -> Option<Tray> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        match build(commands, ctx) {
            Ok(icon) => return Some(Tray { _icon: icon }),
            Err(e) => log::warn!("couldn't add the tray icon: {e:#}"),
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = (commands, ctx);
    None
}

#[cfg(any(windows, target_os = "macos"))]
fn build(commands: Sender<Command>, ctx: &egui::Context) -> anyhow::Result<tray_icon::TrayIcon> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let image = image::load_from_memory(include_bytes!("../../../assets/icon-256.png"))?.thumbnail(64, 64).to_rgba8();
    let icon = tray_icon::Icon::from_rgba(image.to_vec(), image.width(), image.height())?;

    let menu = Menu::new();
    menu.append(&MenuItem::with_id("open", "Open Flex", true, None))?;
    if crate::update::installed() {
        menu.append(&MenuItem::with_id("update", "Update now…", true, None))?;
    }
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&MenuItem::with_id("quit", "Quit Flex", true, None))?;

    let send = move |tx: &Sender<Command>, ctx: &egui::Context, command| {
        let _ = tx.send(command);
        ctx.request_repaint();
    };
    let (tx, c) = (commands.clone(), ctx.clone());
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id.0.as_str() {
        "open" => send(&tx, &c, Command::Show),
        "update" => send(&tx, &c, Command::UpdateNow),
        "quit" => send(&tx, &c, Command::Quit),
        _ => {}
    }));
    let (tx, c) = (commands, ctx.clone());
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
            send(&tx, &c, Command::Show);
        }
    }));

    Ok(TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("Flex")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?)
}
