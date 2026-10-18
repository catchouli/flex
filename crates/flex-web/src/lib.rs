//! Flex's web UI: browse the libraries from other devices on the home network.
//!
//! The app owns the library state and hands the server read-only [`Snapshot`]s whenever
//! it changes; the server never organises or matches anything itself. Changes made from
//! the web (marking things watched) are written to the database, applied to the server's
//! snapshot straight away, and reported to the app through the `on_change` callback, as
//! are the phone remote's commands (see [`remote`]).

mod api;
mod images;
mod media;
mod music;
mod play;
pub mod remote;

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result};
use flex_core::catalog::Catalog;
use flex_core::db::SeriesMeta;
use flex_core::{Library, Tree};

/// One library as the app currently has it.
#[derive(Clone)]
pub struct LibrarySnapshot {
    pub lib: Library,
    pub tree: Arc<Tree>,
    /// None while the library is still being organised into shows.
    pub catalog: Option<Arc<Catalog>>,
    /// Music libraries: albums and artists, once the tags are read.
    pub music: Option<Arc<flex_core::music::MusicCatalog>>,
    /// Music: each album's cover by album key, as the app shows it: `file:<path>` for art
    /// next to the tracks, or a Cover Art Archive URL.
    pub covers: Arc<HashMap<String, String>>,
}

/// Everything the web UI shows. Cheap to build: the big parts are shared `Arc`s.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub libraries: Vec<LibrarySnapshot>,
    /// Episode titles and stills, by provider id.
    pub series: Arc<HashMap<String, SeriesMeta>>,
}

/// A change made from the web, for the app to apply to its own state (the database is
/// already updated).
#[derive(Clone, Debug)]
pub enum Change {
    Watched { library_id: i64, file_ids: Vec<i64>, watched: bool },
    /// Played in the browser.
    Progress { library_id: i64, file_id: i64, progress: flex_core::progress::Progress },
    /// From the phone remote.
    Remote(remote::Remote),
    /// The remote's search: the app answers through `reply`.
    Search { query: String, reply: std::sync::mpsc::SyncSender<Vec<remote::Hit>> },
}

pub struct Config {
    pub port: u16,
    pub db_path: PathBuf,
    /// The app's image cache (TMDB posters and stills, `images/<size>/<name>`).
    pub images_dir: PathBuf,
    /// The app's screenshots (`shots/<file id>-<size>/NN.jpg`), used for episodes
    /// without a TMDB still.
    pub shots_dir: PathBuf,
    /// Where the web UI keeps extracted subtitles and fonts.
    pub cache_dir: PathBuf,
}

pub(crate) struct State {
    snapshot: RwLock<Arc<Snapshot>>,
    /// Writes from the web (the app has its own connection).
    db: Mutex<flex_core::Db>,
    images_dir: PathBuf,
    shots_dir: PathBuf,
    pub(crate) cache_dir: PathBuf,
    on_change: Box<dyn Fn(Change) + Send + Sync>,
    agent: ureq::Agent,
    pub(crate) tools: Arc<media::Tools>,
    /// What's playing in the app, and whether YouTube is on.
    pub(crate) status: RwLock<remote::Status>,
}

impl State {
    pub(crate) fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Changes files in the snapshot straight away (the app publishes its own copy once
    /// it has caught up).
    pub(crate) fn patch_files(&self, library_id: i64, ids: &[i64], f: impl Fn(&mut flex_core::MediaFile)) {
        let mut current = self.snapshot.write().unwrap_or_else(|e| e.into_inner());
        let mut updated = (**current).clone();
        if let Some(l) = updated.libraries.iter_mut().find(|l| l.lib.id == library_id) {
            let tree = Arc::make_mut(&mut l.tree);
            for id in ids {
                if let Some(file) = tree.file_mut(*id) {
                    f(file);
                }
            }
        }
        *current = Arc::new(updated);
    }
}

/// The running web server. Stops when dropped.
pub struct Server {
    state: Arc<State>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    port: u16,
}

impl Server {
    /// Starts serving on all interfaces (so other devices can reach it).
    pub fn start(config: Config, snapshot: Snapshot, on_change: impl Fn(Change) + Send + Sync + 'static) -> Result<Self> {
        // Bound here, not on the server thread, so a port that's taken is reported now.
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, config.port))
            .with_context(|| format!("listening on port {}", config.port))?;
        listener.set_nonblocking(true)?;
        // The port actually bound (0 in the config picks any free one).
        let port = listener.local_addr()?.port();
        let db = flex_core::Db::open(&config.db_path)?;
        let agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(30))).build().into();
        let state = Arc::new(State {
            snapshot: RwLock::new(Arc::new(snapshot)),
            db: Mutex::new(db),
            images_dir: config.images_dir,
            shots_dir: config.shots_dir,
            cache_dir: config.cache_dir,
            on_change: Box::new(on_change),
            agent,
            tools: Arc::new(media::Tools::find()),
            status: RwLock::default(),
        });
        let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("flex-web")
            .enable_all()
            .build()?;
        let router = api::router(state.clone());
        let thread = std::thread::Builder::new().name("flex-web".into()).spawn(move || {
            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::from_std(listener) {
                    Ok(l) => l,
                    Err(e) => return log::error!("web server: {e}"),
                };
                let serve = axum::serve(listener, router).with_graceful_shutdown(async {
                    let _ = stopped.await;
                });
                if let Err(e) = serve.await {
                    log::error!("web server: {e}");
                }
            });
        })?;
        log::info!("web UI on port {port}");
        Ok(Self { state, shutdown: Some(shutdown), thread: Some(thread), port })
    }

    /// Replaces what the web UI shows (call whenever the app's libraries change).
    pub fn publish(&self, snapshot: Snapshot) {
        *self.state.snapshot.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(snapshot);
    }

    /// Keeps the remote's status up to date (cheap to call every frame).
    pub fn set_status(&self, status: remote::Status) {
        let current = self.state.status.read().unwrap_or_else(|e| e.into_inner());
        if *current == status {
            return;
        }
        drop(current);
        *self.state.status.write().unwrap_or_else(|e| e.into_inner()) = status;
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        // Open connections (a long download) shouldn't hold up the app, so don't wait
        // for them: the thread finishes on its own.
        drop(self.thread.take());
        log::info!("web UI stopped");
    }
}

/// This machine's address on the local network, for showing the web UI's URL. Uses the
/// route to a public address without sending anything.
pub fn lan_address() -> Option<std::net::IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(SocketAddr::from(([192, 0, 2, 1], 9))).ok()?;
    Some(socket.local_addr().ok()?.ip()).filter(|ip| !ip.is_unspecified())
}
