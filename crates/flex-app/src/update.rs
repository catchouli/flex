//! Automatic updates: opt-in, and only for copies put there by the Windows installer.
//!
//! A background thread checks GitHub now and then and downloads a newer installer. Once
//! one is ready and nothing is loaded in the player, the app runs it silently: it asks
//! Flex to quit (saving progress), installs, and starts Flex again. "Update now" in the
//! tray menu checks straight away and installs as soon as it's downloaded. Progress
//! shows along the bottom of the screen.
//!
//! `FLEX_UPDATE_AS=0.1.0` makes Flex act as an installed copy of that version, for
//! trying this out with a zip or dev build.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
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
/// A check is one small request (see [`update::latest_version`]).
const CHECK_EVERY: Duration = Duration::from_secs(5 * 60);
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

/// Where an update has got to, for the messages along the bottom of the screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Progress {
    #[default]
    Idle,
    /// Only shown for "Update now".
    Checking,
    Downloading { version: Version, done: u64, total: u64 },
    Ready { version: Version, installer: PathBuf },
    Installing { version: Version },
    /// After "Update now": nothing newer, or it failed. Shown once, then back to idle.
    UpToDate,
    Failed(String),
}

#[derive(Default)]
struct Shared {
    enabled: AtomicBool,
    progress: Mutex<Progress>,
}

#[derive(Default)]
pub struct Updater {
    shared: Arc<Shared>,
    /// Wakes the checking thread for "Update now"; set once it's running.
    wake: Option<Sender<()>>,
    /// "Update now" was chosen: install as soon as it's downloaded, even mid-video.
    now: bool,
    launched: bool,
}

impl Updater {
    /// Starts or stops checking for updates.
    pub fn set_enabled(&mut self, on: bool, ctx: &egui::Context) {
        self.shared.enabled.store(on, Ordering::Relaxed);
        if on {
            self.start(ctx);
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        if self.wake.is_none() {
            let (tx, rx) = channel();
            self.wake = Some(tx);
            let (shared, ctx) = (self.shared.clone(), ctx.clone());
            std::thread::spawn(move || check_loop(&shared, &rx, &ctx));
        }
    }

    /// "Update now" (from the tray): checks right away, then installs whatever it finds
    /// as soon as it's downloaded, whether or not automatic updates are on.
    pub fn update_now(&mut self, ctx: &egui::Context) {
        if self.launched {
            return;
        }
        self.now = true;
        self.start(ctx);
        let downloading = matches!(self.progress(), Progress::Downloading { .. } | Progress::Ready { .. });
        if !downloading {
            self.set_progress(Progress::Checking);
            if let Some(wake) = &self.wake {
                let _ = wake.send(());
            }
        }
        ctx.request_repaint();
    }

    pub fn progress(&self) -> Progress {
        self.shared.progress.lock().map(|p| p.clone()).unwrap_or_default()
    }

    fn set_progress(&self, progress: Progress) {
        if let Ok(mut p) = self.shared.progress.lock() {
            *p = progress;
        }
    }

    /// Whether this update was asked for with "Update now".
    pub fn asked(&self) -> bool {
        self.now
    }

    /// A one-off message ("up to date", "failed") after "Update now", once.
    pub fn take_message(&mut self) -> Option<String> {
        let message = match self.progress() {
            Progress::UpToDate => format!("Flex {} is up to date", current_version()),
            Progress::Failed(e) => format!("Couldn't update: {e}"),
            _ => return None,
        };
        self.now = false;
        self.set_progress(Progress::Idle);
        Some(message)
    }

    /// The installer to run and its version, once one has been downloaded (and updates
    /// are on, or "Update now" was chosen).
    pub fn ready(&self) -> Option<(Version, PathBuf)> {
        if self.launched || !(self.now || self.shared.enabled.load(Ordering::Relaxed)) {
            return None;
        }
        match self.progress() {
            Progress::Ready { version, installer } => Some((version, installer)),
            _ => None,
        }
    }

    /// Runs the installer silently. It stops Flex, so this is the last thing Flex does.
    /// `hidden`: Flex is in the tray, so it's started there again afterwards.
    pub fn install(&mut self, version: Version, installer: &Path, hidden: bool) -> Result<()> {
        self.launched = true;
        let log = download_dir().join("install.log");
        let mut command = std::process::Command::new(installer);
        command.args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]).arg(format!("/LOG={}", log.display()));
        if hidden {
            // Passed on to Flex when the installer starts it again (see flex.iss).
            command.arg("/flexargs=--tray");
        }
        log::info!("updating: running {} (log: {})", installer.display(), log.display());
        if let Err(e) = command.spawn() {
            self.launched = false;
            self.now = false;
            self.set_progress(Progress::Idle);
            return Err(e).with_context(|| format!("couldn't run {}", installer.display()));
        }
        self.set_progress(Progress::Installing { version });
        Ok(())
    }
}

fn check_loop(shared: &Shared, wake: &Receiver<()>, ctx: &egui::Context) {
    let current = current_version();
    let mut next = Instant::now() + FIRST_CHECK;
    loop {
        let asked = match wake.recv_timeout(TICK) {
            Ok(()) => true,
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        // Once one is downloaded there's nothing more to do (it's installed next).
        if matches!(*shared.progress.lock().unwrap_or_else(|e| e.into_inner()), Progress::Ready { .. }) {
            continue;
        }
        if !asked {
            if !shared.enabled.load(Ordering::Relaxed) {
                // Switched back on later: check soon after.
                next = next.max(Instant::now() + FIRST_CHECK);
                continue;
            }
            if Instant::now() < next {
                continue;
            }
        }
        let set = |progress: Progress| {
            *shared.progress.lock().unwrap_or_else(|e| e.into_inner()) = progress;
            ctx.request_repaint();
        };
        next = Instant::now() + CHECK_EVERY;
        match fetch(current, &set) {
            Ok(Some((version, installer))) => set(Progress::Ready { version, installer }),
            Ok(None) => set(if asked { Progress::UpToDate } else { Progress::Idle }),
            Err(e) => {
                log::warn!("update check failed: {e:#}");
                set(if asked { Progress::Failed(format!("{e:#}")) } else { Progress::Idle });
            }
        }
    }
}

/// Downloads a newer release's installer, if there is one, reporting progress.
fn fetch(current: Version, set: &dyn Fn(Progress)) -> Result<Option<(Version, PathBuf)>> {
    let Some(installer) = update::newer_installer(current)? else {
        log::debug!("no update: {current} is the latest");
        return Ok(None);
    };
    log::info!("update found: Flex {} ({} bytes)", installer.version, installer.size);
    let (version, total) = (installer.version, installer.size);
    set(Progress::Downloading { version, done: 0, total });
    let dir = download_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    // Installers for older updates aren't needed any more.
    for entry in std::fs::read_dir(&dir)?.flatten() {
        if entry.file_name() != *installer.name {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let dest = dir.join(&installer.name);
    // Repaint for each whole percent, not every chunk.
    let mut shown = 0;
    update::download(&installer, &dest, |done| {
        let percent = done * 100 / total.max(1);
        if percent != shown {
            shown = percent;
            set(Progress::Downloading { version, done, total });
        }
    })?;
    log::info!("update ready: {}", dest.display());
    Ok(Some((version, dest)))
}
