//! One Flex at a time. The running copy listens on a localhost port (saved as `flex.port`
//! in the data folder); starting Flex again asks it to show its window, then exits.
//! `flex --quit` asks it to quit (e.g. before replacing flex.exe).

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use eframe::egui;

/// Requests to the app from outside its window: another launch, or the tray icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Show,
    Quit,
    /// Check for an update and install it now (tray menu).
    UpdateNow,
}

fn port_file() -> std::path::PathBuf {
    flex_core::data_dir().join("flex.port")
}

/// Starts listening for later launches. Each connection sends one command line.
pub fn listen(commands: Sender<Command>, ctx: egui::Context) {
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("couldn't listen for other launches: {e}");
            return;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or_default();
    if let Err(e) = std::fs::write(port_file(), port.to_string()) {
        log::warn!("couldn't save the instance port: {e}");
        return;
    }
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            if BufReader::new(&stream).read_line(&mut line).is_err() {
                continue;
            }
            let command = match line.trim() {
                "show" => Command::Show,
                "quit" => Command::Quit,
                "update" => Command::UpdateNow,
                other => {
                    log::warn!("unknown instance command {other:?}");
                    continue;
                }
            };
            log::info!("another launch asked to {}", line.trim());
            let _ = (&stream).write_all(b"ok\n");
            if commands.send(command).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    });
}

/// Asks the running Flex to show its window (or quit). Retries for a few seconds in case
/// it's still starting up and hasn't saved its port yet.
pub fn send(command: Command) -> anyhow::Result<()> {
    // Let the running copy bring its window to the front: Windows only allows that for
    // the process the user last interacted with, which is this one.
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
            windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY,
        );
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match try_send(command) {
            Ok(()) => return Ok(()),
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

fn try_send(command: Command) -> anyhow::Result<()> {
    let port: u16 = std::fs::read_to_string(port_file())?.trim().parse()?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(1))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(match command {
        Command::Show => b"show\n",
        Command::Quit => b"quit\n",
        Command::UpdateNow => b"update\n",
    })?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;
    anyhow::ensure!(reply.trim() == "ok", "unexpected reply {reply:?}");
    Ok(())
}
