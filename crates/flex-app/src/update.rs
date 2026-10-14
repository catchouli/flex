//! Automatic updates: opt-in, and only for copies put there by the Windows installer.
//!
//! A background thread checks GitHub now and then and downloads a newer installer. Once
//! one is ready and nothing is loaded in the player, the app runs it silently: it asks
//! Flex to quit (saving progress), installs, and starts Flex again.
//!
//! `FLEX_UPDATE_AS=0.1.0` makes Flex act as an installed copy of that version, for
//! trying this out with a zip or dev build.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use eframe::egui;
use flex_core::update::{self, Version};

pub const SETTING: &str = "auto_update";
const PRETEND_ENV: &str = "FLEX_UPDATE_AS";
/// The first check comes a little after starting (or switching updates on), so it
/// doesn't compete with startup.
const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
const TICK: Duration = Duration::from_secs(15);

/// The version to compare releases against.
pub fn current_version() -> Version {
    std::env::var(PRETEND_ENV).ok().and_then(|v| Version::parse(&v)).unwrap_or_else(Version::current)
}

/// Whether this copy was put here by the installer (its uninstaller sits next to it), so
/// the installer can update it.
pub fn installed() -> bool {
    if std::env::var_os(PRETEND_ENV).is_some() {
        return true;
    }
    cfg!(windows)
        && std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("unins000.exe").is_file()))
            .unwrap_or(false)
}

fn download_dir() -> PathBuf {
    std::env::temp_dir().join("flex-update")
}

#[derive(Default)]
pub struct Updater {
    enabled: Arc<AtomicBool>,
    /// The downloaded installer, once there is one.
    ready: Arc<Mutex<Option<PathBuf>>>,
    started: bool,
    launched: bool,
}

impl Updater {
    /// Starts or stops checking for updates.
    pub fn set_enabled(&mut self, on: bool, ctx: &egui::Context) {
        self.enabled.store(on, Ordering::Relaxed);
        if on && !self.started {
            self.started = true;
            let (enabled, ready, ctx) = (self.enabled.clone(), self.ready.clone(), ctx.clone());
            std::thread::spawn(move || check_loop(&enabled, &ready, &ctx));
        }
    }

    /// The installer to run, while updates are on and one has been downloaded.
    pub fn ready(&self) -> Option<PathBuf> {
        if self.launched || !self.enabled.load(Ordering::Relaxed) {
            return None;
        }
        self.ready.lock().ok()?.clone()
    }

    /// Runs the installer silently. It stops Flex, so this is the last thing Flex does.
    /// `hidden`: Flex is in the tray, so it's started there again afterwards.
    pub fn install(&mut self, installer: &Path, hidden: bool) -> Result<()> {
        self.launched = true;
        let log = download_dir().join("install.log");
        let mut command = std::process::Command::new(installer);
        command.args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]).arg(format!("/LOG={}", log.display()));
        if hidden {
            // Passed on to Flex when the installer starts it again (see flex.iss).
            command.arg("/flexargs=--tray");
        }
        log::info!("updating: running {} (log: {})", installer.display(), log.display());
        command.spawn().with_context(|| format!("couldn't run {}", installer.display()))?;
        Ok(())
    }
}

fn check_loop(enabled: &AtomicBool, ready: &Mutex<Option<PathBuf>>, ctx: &egui::Context) {
    let current = current_version();
    let mut next = Instant::now() + FIRST_CHECK;
    loop {
        std::thread::sleep(TICK);
        if !enabled.load(Ordering::Relaxed) {
            // Switched back on later: check soon after.
            next = next.max(Instant::now() + FIRST_CHECK);
            continue;
        }
        if Instant::now() < next {
            continue;
        }
        match fetch(current) {
            Ok(Some(path)) => {
                if let Ok(mut r) = ready.lock() {
                    *r = Some(path);
                }
                ctx.request_repaint();
                return;
            }
            Ok(None) => next = Instant::now() + CHECK_EVERY,
            Err(e) => {
                log::warn!("update check failed: {e:#}");
                next = Instant::now() + RETRY_AFTER;
            }
        }
    }
}

/// Downloads a newer release's installer, if there is one.
fn fetch(current: Version) -> Result<Option<PathBuf>> {
    let Some(installer) = update::newer_installer(current)? else {
        log::info!("no update: {current} is the latest");
        return Ok(None);
    };
    log::info!("update found: Flex {} ({} bytes)", installer.version, installer.size);
    let dir = download_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    // Installers for older updates aren't needed any more.
    for entry in std::fs::read_dir(&dir)?.flatten() {
        if entry.file_name() != *installer.name {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let dest = dir.join(&installer.name);
    update::download(&installer, &dest)?;
    log::info!("update ready: {}", dest.display());
    Ok(Some(dest))
}
