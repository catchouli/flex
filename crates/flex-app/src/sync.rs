//! Sync between devices (see `flex_core::sync`), on a background thread with its own
//! database connection: every so often (and when nudged, e.g. after watching something)
//! it applies other devices' snapshots, then rewrites this device's if anything changed.
//! What it applied comes back through [`Sync::poll`] so the app can reload it.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use eframe::egui;
use flex_core::sync::{self as core, Applied};

pub const DIR_SETTING: &str = "sync_dir";
pub const DEVICE_SETTING: &str = "sync_device";
pub const NAME_SETTING: &str = "sync_name";

/// How often to look for other devices' changes.
const EVERY: Duration = Duration::from_secs(30);

pub enum Msg {
    Applied(Applied),
    /// The other devices seen: (name, when their snapshot was written).
    Peers(Vec<(String, i64)>),
    /// This device's snapshot was written at this time.
    Wrote(i64),
    Failed(String),
}

pub struct Sync {
    wake: Sender<()>,
    rx: Receiver<Msg>,
    pub peers: Vec<(String, i64)>,
    pub last_written: Option<i64>,
    pub error: Option<String>,
}

impl Sync {
    pub fn start(db_path: PathBuf, dir: PathBuf, device: String, name: String, ctx: &egui::Context) -> Self {
        let (wake, woken) = channel();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("flex-sync".into())
            .spawn(move || run(&db_path, &dir, &device, &name, &woken, &tx, &ctx))
            .ok();
        Self { wake, rx, peers: Vec::new(), last_written: None, error: None }
    }

    /// Syncs soon (something changed here, or a video just finished).
    pub fn nudge(&self) {
        let _ = self.wake.send(());
    }

    /// What the thread applied since last time (the rest updates the status).
    pub fn poll(&mut self) -> Vec<Applied> {
        let mut applied = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Applied(a) => applied.push(a),
                Msg::Peers(p) => self.peers = p,
                Msg::Wrote(at) => {
                    self.last_written = Some(at);
                    self.error = None;
                }
                Msg::Failed(e) => self.error = Some(e),
            }
        }
        applied
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn run(db_path: &std::path::Path, dir: &std::path::Path, device: &str, name: &str, woken: &Receiver<()>, tx: &Sender<Msg>, ctx: &egui::Context) {
    let send = |msg| {
        let _ = tx.send(msg);
        ctx.request_repaint();
    };
    let mut db = match flex_core::Db::open(db_path) {
        Ok(db) => db,
        Err(e) => return send(Msg::Failed(format!("{e:#}"))),
    };
    // What was last written (without its time), to skip rewriting an unchanged snapshot.
    let mut written: Option<Vec<u8>> = None;
    loop {
        let others = core::read_others(dir, device);
        send(Msg::Peers(others.iter().map(|s| (s.name.clone(), s.written_at)).collect()));
        match core::apply(&mut db, &others) {
            Ok(applied) if !applied.is_empty() => {
                log::info!("sync: applied {applied:?}");
                send(Msg::Applied(applied));
            }
            Ok(_) => {}
            Err(e) => log::warn!("sync: applying: {e:#}"),
        }
        match core::export(&db, device, name, 0) {
            Ok(mut snapshot) => {
                let body = flex_plugin::serde_json::to_vec(&snapshot).unwrap_or_default();
                if written.as_ref() != Some(&body) {
                    snapshot.written_at = now();
                    match core::write(dir, &snapshot) {
                        Ok(()) => {
                            written = Some(body);
                            send(Msg::Wrote(snapshot.written_at));
                        }
                        Err(e) => {
                            log::warn!("sync: {e:#}");
                            send(Msg::Failed(format!("{e:#}")));
                        }
                    }
                }
            }
            Err(e) => log::warn!("sync: exporting: {e:#}"),
        }
        match woken.recv_timeout(EVERY) {
            // Several nudges at once only need one round.
            Ok(()) => while woken.try_recv().is_ok() {},
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// A name for this device to start with: its host name.
pub fn default_name() -> String {
    let from_env = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok();
    let from_command = || {
        let out = std::process::Command::new("hostname").output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().trim_end_matches(".local").to_owned())
    };
    from_env.or_else(from_command).filter(|n| !n.is_empty()).unwrap_or_else(|| "This device".to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use flex_core::scan::{ScanResult, ScannedFile};
    use flex_core::{Db, LibraryKind};

    use super::Sync;

    fn device(dir: &Path, name: &str) -> (std::path::PathBuf, i64) {
        let path = dir.join(format!("{name}.db"));
        let mut db = Db::open(&path).unwrap();
        let lib = db.add_library("Anime", &dir.join(format!("{name}-media")), LibraryKind::Anime).unwrap();
        let files = ["Show/01.mkv", "Show/02.mkv"].iter().map(|p| ScannedFile { rel_path: (*p).into(), size: 1, mtime: 1 }).collect();
        db.apply_scan(lib, &ScanResult { files, ..ScanResult::default() }).unwrap();
        (path, lib)
    }

    fn watched(db_path: &Path, lib: i64, rel: &str) -> bool {
        Db::open(db_path).unwrap().files(lib).unwrap().into_iter().any(|f| f.rel_path == rel && f.progress.watched)
    }

    /// Two devices with their own databases sync through a folder, both ways.
    #[test]
    fn two_devices_sync_through_a_folder() {
        let dir = std::env::temp_dir().join(format!("flex-sync-app-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shared = dir.join("shared");
        let (tv_db, tv_lib) = device(&dir, "tv");
        let (mac_db, mac_lib) = device(&dir, "mac");
        // Watched on the TV before syncing starts.
        let mut tv = Db::open(&tv_db).unwrap();
        let first = tv.files(tv_lib).unwrap().into_iter().find(|f| f.rel_path == "Show/01.mkv").unwrap().id;
        tv.set_watched(&[first], true).unwrap();

        let ctx = eframe::egui::Context::default();
        let mut tv_sync = Sync::start(tv_db.clone(), shared.clone(), "tv0000000000".into(), "TV".into(), &ctx);
        let mut mac_sync = Sync::start(mac_db.clone(), shared.clone(), "mac000000000".into(), "Mac".into(), &ctx);
        let wait = |what: &str, done: &mut dyn FnMut() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !done() {
                assert!(Instant::now() < deadline, "timed out: {what}");
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        // The TV writes its file at once; nudge the Mac to read it.
        wait("the TV's snapshot", &mut || shared.join("tv0000000000.json").exists());
        mac_sync.nudge();
        let mut reloaded = false;
        wait("the Mac to apply it", &mut || {
            reloaded |= mac_sync.poll().iter().any(|a| a.files.contains(&mac_lib));
            reloaded && watched(&mac_db, mac_lib, "Show/01.mkv")
        });

        // And back: marked watched on the Mac (a second later, so it's newer).
        std::thread::sleep(Duration::from_millis(1100));
        let mut mac = Db::open(&mac_db).unwrap();
        let second = mac.files(mac_lib).unwrap().into_iter().find(|f| f.rel_path == "Show/02.mkv").unwrap().id;
        mac.set_watched(&[second], true).unwrap();
        mac_sync.nudge();
        wait("the Mac's change to be written", &mut || {
            mac_sync.poll();
            std::fs::read_to_string(shared.join("mac000000000.json")).is_ok_and(|s| s.contains("Show/02.mkv"))
        });
        tv_sync.nudge();
        wait("the TV to apply it", &mut || {
            tv_sync.poll();
            watched(&tv_db, tv_lib, "Show/02.mkv")
        });
        // The status (sent just before applying) may land after the last check above.
        std::thread::sleep(Duration::from_millis(200));
        tv_sync.poll();
        assert!(tv_sync.peers.iter().any(|(name, _)| name == "Mac"), "{:?}", tv_sync.peers);
        drop((tv_sync, mac_sync));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
