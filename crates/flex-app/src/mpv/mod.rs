//! Safe wrapper around libmpv: a player handle plus an OpenGL render context
//! that draws video into whatever framebuffer egui is painting.

pub mod ffi;

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use eframe::egui;

pub enum Event {
    FileLoaded,
    /// Playback finished; `error` is set if the file failed to play.
    EndFile { eof: bool, error: Option<String> },
    Shutdown,
}

pub struct Mpv {
    api: &'static ffi::Api,
    handle: *mut ffi::Handle,
    wakeup_ctx: *mut egui::Context,
}

// libmpv handles are thread-safe (client.h: "all functions are thread-safe").
unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

unsafe extern "C" fn request_repaint(ctx: *mut c_void) {
    // Safety: ctx is the boxed egui::Context owned by Mpv/Renderer, which outlives the callback.
    unsafe { (*(ctx as *const egui::Context)).request_repaint() };
}

fn cstr(s: &str) -> CString {
    CString::new(s).expect("mpv strings must not contain NUL")
}

impl Mpv {
    /// Creates and initialises a player. `options` are applied before initialisation and may
    /// override the defaults below.
    pub fn new(egui_ctx: &egui::Context, options: &[(&str, &str)]) -> Result<Self> {
        let defaults = [
            ("vo", "libmpv"),
            // Windows: decoding on the GPU but copying frames back avoids the D3D11→OpenGL
            // hand-over, which glitches on some GPUs (seen as blocky artefacts on motion).
            ("hwdec", if cfg!(windows) { "auto-copy-safe" } else { "auto-safe" }),
            // Time video to the display's refresh, resampling audio slightly to match:
            // smoother motion than timing by the audio clock alone.
            ("video-sync", "display-resample"),
            ("config", "no"),
            ("terminal", "no"),
            ("input-default-bindings", "no"),
            ("input-vo-keyboard", "no"),
            ("osc", "no"),
            ("keep-open", "no"),
            ("idle", "yes"),
            ("tone-mapping", "auto"),
        ];
        let options: Vec<(&str, &str)> = defaults.iter().chain(options).copied().collect();
        let mut mpv = Self::create(&options)?;
        mpv.wakeup_ctx = Box::into_raw(Box::new(egui_ctx.clone()));
        unsafe { (mpv.api.mpv_set_wakeup_callback)(mpv.handle, Some(request_repaint), mpv.wakeup_ctx.cast()) };
        Ok(mpv)
    }

    /// A player with no window or wakeups, for background work (screenshots): events are
    /// read with [`Mpv::wait_event`].
    pub fn headless(options: &[(&str, &str)]) -> Result<Self> {
        let defaults = [("config", "no"), ("terminal", "no"), ("idle", "yes"), ("input-default-bindings", "no")];
        let options: Vec<(&str, &str)> = defaults.iter().chain(options).copied().collect();
        Self::create(&options)
    }

    fn create(options: &[(&str, &str)]) -> Result<Self> {
        let api = ffi::api().map_err(|e| anyhow!(e))?;
        let version = unsafe { (api.mpv_client_api_version)() };
        if version >> 16 != 2 {
            bail!("unsupported libmpv client API version {}.{}", version >> 16, version & 0xffff);
        }
        let handle = unsafe { (api.mpv_create)() };
        if handle.is_null() {
            bail!("mpv_create failed");
        }
        let mpv = Self { api, handle, wakeup_ctx: ptr::null_mut() };
        for (k, v) in options {
            mpv.set_option(k, v)?;
        }
        mpv.check(unsafe { (api.mpv_initialize)(handle) }, "initialize")?;
        // FLEX_MPV_LOG=v (or debug, ...) shows more of mpv's log, for debugging.
        let level = cstr(&std::env::var("FLEX_MPV_LOG").unwrap_or_else(|_| "warn".to_owned()));
        unsafe { (api.mpv_request_log_messages)(handle, level.as_ptr()) };
        Ok(mpv)
    }

    fn check(&self, code: c_int, what: &str) -> Result<()> {
        if code >= 0 {
            return Ok(());
        }
        let msg = unsafe { CStr::from_ptr((self.api.mpv_error_string)(code)) };
        bail!("mpv {what}: {}", msg.to_string_lossy())
    }

    pub fn set_option(&self, name: &str, value: &str) -> Result<()> {
        let code = unsafe {
            (self.api.mpv_set_option_string)(self.handle, cstr(name).as_ptr(), cstr(value).as_ptr())
        };
        self.check(code, &format!("option {name}={value}"))
    }

    pub fn command(&self, args: &[&str]) -> Result<()> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).collect();
        ptrs.push(ptr::null());
        let code = unsafe { (self.api.mpv_command)(self.handle, ptrs.as_mut_ptr()) };
        self.check(code, &format!("command {args:?}"))
    }

    /// Starts playing `path`: a video file, a disc image (`.iso`), or a disc folder
    /// (`VIDEO_TS` or a folder of `.VOB`s, or `BDMV`), which plays through the disc URL with
    /// the folder or image set as the device. Without DVD support in libmpv, a DVD image
    /// plays its main feature's VOBs directly (no menus or chapters).
    pub fn load(&self, path: &std::path::Path) -> Result<()> {
        use flex_core::disc::{DiscImage, disc_image, dvd_main_title};
        let disc = if path.is_dir() {
            let bluray = path.file_name().is_some_and(|n| n.eq_ignore_ascii_case("BDMV"));
            // libbluray wants the folder that contains `BDMV`.
            Some(if bluray { (DiscImage::Bluray, path.parent().unwrap_or(path)) } else { (DiscImage::Dvd, path) })
        } else {
            disc_image(path).map(|kind| (kind, path))
        };
        let url = match disc {
            None => path.to_string_lossy().into_owned(),
            Some((DiscImage::Bluray, device)) => {
                self.set_string("bluray-device", &device.to_string_lossy())?;
                "bd://".to_owned()
            }
            Some((DiscImage::Dvd, device)) if path.is_dir() || self.has_protocol("dvd") => {
                self.set_string("dvd-device", &device.to_string_lossy())?;
                "dvd://".to_owned()
            }
            Some((DiscImage::Dvd, image)) => match dvd_main_title(image) {
                Some(range) => format!("slice://{}-{}@{}", range.start, range.end, image.to_string_lossy()),
                None => image.to_string_lossy().into_owned(),
            },
        };
        self.command(&["loadfile", &url])
    }

    fn has_protocol(&self, name: &str) -> bool {
        self.get_string("protocol-list").is_some_and(|list| list.split(',').any(|p| p == name))
    }

    pub fn set_string(&self, name: &str, value: &str) -> Result<()> {
        let value = cstr(value);
        let mut ptr = value.as_ptr();
        let code = unsafe {
            (self.api.mpv_set_property)(self.handle, cstr(name).as_ptr(), ffi::FORMAT_STRING, (&raw mut ptr).cast())
        };
        self.check(code, &format!("set {name}"))
    }

    pub fn set_flag(&self, name: &str, value: bool) -> Result<()> {
        let mut v: c_int = value.into();
        let code = unsafe {
            (self.api.mpv_set_property)(self.handle, cstr(name).as_ptr(), ffi::FORMAT_FLAG, (&raw mut v).cast())
        };
        self.check(code, &format!("set {name}"))
    }

    fn get_raw<T: Default>(&self, name: &str, format: c_int) -> Option<T> {
        let mut out = T::default();
        let code = unsafe {
            (self.api.mpv_get_property)(self.handle, cstr(name).as_ptr(), format, (&raw mut out).cast())
        };
        (code >= 0).then_some(out)
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        self.get_raw(name, ffi::FORMAT_DOUBLE)
    }

    pub fn get_i64(&self, name: &str) -> Option<i64> {
        self.get_raw(name, ffi::FORMAT_INT64)
    }

    pub fn get_flag(&self, name: &str) -> Option<bool> {
        self.get_raw::<c_int>(name, ffi::FORMAT_FLAG).map(|v| v != 0)
    }

    pub fn get_string(&self, name: &str) -> Option<String> {
        let mut out: *mut c_char = ptr::null_mut();
        let code = unsafe {
            (self.api.mpv_get_property)(self.handle, cstr(name).as_ptr(), ffi::FORMAT_STRING, (&raw mut out).cast())
        };
        if code < 0 || out.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(out) }.to_string_lossy().into_owned();
        unsafe { (self.api.mpv_free)(out.cast()) };
        Some(s)
    }

    /// Drains pending events without blocking. Log messages are forwarded to `log`.
    pub fn poll_events(&self) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            match self.next_event(0.0) {
                Some(Some(e)) => events.push(e),
                Some(None) => {}
                None => break,
            }
        }
        events
    }

    /// Waits up to `timeout` seconds for an event of interest; `None` on timeout.
    pub fn wait_event(&self, timeout: f64) -> Option<Event> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f64(timeout);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now()).as_secs_f64();
            match self.next_event(left)? {
                Some(e) => return Some(e),
                None if left <= 0.0 => return None,
                None => {}
            }
        }
    }

    /// The next event: `None` if there was none within `timeout`, `Some(None)` for one
    /// that's of no interest (log messages are forwarded to `log`).
    fn next_event(&self, timeout: f64) -> Option<Option<Event>> {
        let ev = unsafe { &*(self.api.mpv_wait_event)(self.handle, timeout) };
        Some(match ev.event_id {
            ffi::EVENT_NONE => return None,
            ffi::EVENT_SHUTDOWN => Some(Event::Shutdown),
            ffi::EVENT_FILE_LOADED => Some(Event::FileLoaded),
            ffi::EVENT_END_FILE => {
                let data = unsafe { &*(ev.data as *const ffi::EventEndFile) };
                let error = (data.reason == ffi::END_FILE_REASON_ERROR).then(|| {
                    let msg = unsafe { CStr::from_ptr((self.api.mpv_error_string)(data.error)) };
                    msg.to_string_lossy().into_owned()
                });
                Some(Event::EndFile { eof: data.reason == ffi::END_FILE_REASON_EOF, error })
            }
            ffi::EVENT_LOG_MESSAGE => {
                let m = unsafe { &*(ev.data as *const ffi::EventLogMessage) };
                let (prefix, text) = unsafe { (CStr::from_ptr(m.prefix), CStr::from_ptr(m.text)) };
                log::warn!("mpv[{}] {}", prefix.to_string_lossy(), text.to_string_lossy().trim_end());
                None
            }
            _ => None,
        })
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        unsafe {
            (self.api.mpv_set_wakeup_callback)(self.handle, None, ptr::null_mut());
            (self.api.mpv_terminate_destroy)(self.handle);
            if !self.wakeup_ctx.is_null() {
                drop(Box::from_raw(self.wakeup_ctx));
            }
        }
    }
}

pub type GetProcAddress = Arc<dyn Fn(&CStr) -> *const c_void + Send + Sync>;

unsafe extern "C" fn get_proc_address(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    // Safety: ctx points at the boxed GetProcAddress owned by the Renderer.
    unsafe {
        let f = &*(ctx as *const GetProcAddress);
        f(CStr::from_ptr(name)) as *mut c_void
    }
}

/// An mpv OpenGL render context. Must be created, used and dropped on the GL thread
/// with the context current; it keeps its `Mpv` alive so it is always freed first.
pub struct Renderer {
    ctx: *mut ffi::RenderContext,
    mpv: Arc<Mpv>,
    _proc_address: Box<GetProcAddress>,
    update_ctx: *mut egui::Context,
}

// Only ever touched from egui's paint callback on the main (GL) thread.
unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

impl Renderer {
    pub fn new(mpv: Arc<Mpv>, proc_address: GetProcAddress, egui_ctx: &egui::Context) -> Result<Self> {
        let api = mpv.api;
        let proc_address = Box::new(proc_address);
        let mut init = ffi::OpenGlInitParams {
            get_proc_address: Some(get_proc_address),
            get_proc_address_ctx: (&raw const *proc_address).cast_mut().cast(),
        };
        let mut params = [
            ffi::RenderParam { kind: ffi::RENDER_PARAM_API_TYPE, data: c"opengl".as_ptr().cast_mut().cast() },
            ffi::RenderParam { kind: ffi::RENDER_PARAM_OPENGL_INIT_PARAMS, data: (&raw mut init).cast() },
            ffi::RenderParam { kind: ffi::RENDER_PARAM_INVALID, data: ptr::null_mut() },
        ];
        let mut ctx = ptr::null_mut();
        let code = unsafe { (api.mpv_render_context_create)(&mut ctx, mpv.handle, params.as_mut_ptr()) };
        mpv.check(code, "render context create")?;

        let update_ctx = Box::into_raw(Box::new(egui_ctx.clone()));
        unsafe { (api.mpv_render_context_set_update_callback)(ctx, Some(request_repaint), update_ctx.cast()) };
        Ok(Self { ctx, mpv, _proc_address: proc_address, update_ctx })
    }

    /// Renders the current video frame to framebuffer `fbo` (0 = default) of size `w`×`h` pixels.
    ///
    /// Also reports the previous frame as shown: egui swaps buffers right after painting,
    /// so by the next paint the last frame is on screen. Without this mpv can only guess
    /// when frames appear, which shows up as stutter on fast motion.
    pub fn render(&self, fbo: i32, w: i32, h: i32, flip_y: bool) {
        let api = self.mpv.api;
        unsafe { (api.mpv_render_context_report_swap)(self.ctx) };
        let mut target = ffi::OpenGlFbo { fbo, w, h, internal_format: 0 };
        let mut flip: c_int = flip_y.into();
        let mut params = [
            ffi::RenderParam { kind: ffi::RENDER_PARAM_OPENGL_FBO, data: (&raw mut target).cast() },
            ffi::RenderParam { kind: ffi::RENDER_PARAM_FLIP_Y, data: (&raw mut flip).cast() },
            ffi::RenderParam { kind: ffi::RENDER_PARAM_INVALID, data: ptr::null_mut() },
        ];
        unsafe {
            (api.mpv_render_context_update)(self.ctx);
            (api.mpv_render_context_render)(self.ctx, params.as_mut_ptr());
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let api = self.mpv.api;
        unsafe {
            (api.mpv_render_context_set_update_callback)(self.ctx, None, ptr::null_mut());
            (api.mpv_render_context_free)(self.ctx);
            drop(Box::from_raw(self.update_ctx));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Event, Mpv};

    /// Reads a file's chapters the way the player does and finds its opening and ending:
    /// `FLEX_CHAPTER_VIDEO=… cargo test -p flex reads_chapters -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn reads_chapters() {
        let video = std::env::var("FLEX_CHAPTER_VIDEO").expect("FLEX_CHAPTER_VIDEO");
        let mpv = Mpv::headless(&[("vo", "null"), ("ao", "null"), ("pause", "yes")]).unwrap();
        mpv.command(&["loadfile", &video]).unwrap();
        assert!(matches!(mpv.wait_event(10.0), Some(Event::FileLoaded)));
        let count = mpv.get_i64("chapter-list/count").unwrap_or(0);
        let chapters: Vec<(f64, String)> = (0..count)
            .filter_map(|i| Some((mpv.get_f64(&format!("chapter-list/{i}/time"))?, mpv.get_string(&format!("chapter-list/{i}/title")).unwrap_or_default())))
            .collect();
        eprintln!("chapters: {chapters:?}");
        let skips = flex_core::skip::find(&chapters, mpv.get_f64("duration"));
        eprintln!("skips: {skips:?}");
        assert_eq!(skips.len(), 2);
        assert_eq!((skips[0].start, skips[0].end), (20.0, 110.0));
        assert_eq!((skips[1].start, skips[1].end), (500.0, 590.0));
    }

    /// Plays a real YouTube video in a windowless mpv (needs yt-dlp and network):
    /// `cargo test -p flex plays_youtube -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn plays_youtube() {
        let _ = env_logger::builder().is_test(true).try_init();
        let tools = flex_core::youtube::Tools::find().expect("yt-dlp in the tools folder or on the PATH");
        let started = Instant::now();
        let video = tools.resolve("aqz-KE-bpKQ", 1080, false).unwrap();
        eprintln!("resolved in {:?}: {} ({:?}p)", started.elapsed(), video.title, video.height);

        let mpv = Mpv::headless(&[("vo", "null"), ("ao", "null"), ("hwdec", "no")]).unwrap();
        let loading = Instant::now();
        // Through the relay, as the player does.
        mpv.command(&["loadfile", &video.relayed_mpv_url()]).unwrap();
        match mpv.wait_event(30.0) {
            Some(Event::FileLoaded) => {}
            Some(Event::EndFile { error, .. }) => panic!("didn't play: {error:?}"),
            Some(Event::Shutdown) | None => panic!("no FileLoaded within 30s"),
        }
        eprintln!("loaded in {:?}", loading.elapsed());
        let count = mpv.get_i64("track-list/count").unwrap_or(0);
        let kinds: Vec<String> = (0..count).filter_map(|i| mpv.get_string(&format!("track-list/{i}/type"))).collect();
        eprintln!("tracks: {kinds:?}, duration {:?}", mpv.get_f64("duration"));
        assert!(kinds.iter().any(|k| k == "video") && kinds.iter().any(|k| k == "audio"), "{kinds:?}");

        let wait_for = |what: &str, done: &dyn Fn(f64) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let _ = mpv.poll_events();
                if let Some(pos) = mpv.get_f64("time-pos").filter(|p| done(*p)) {
                    return pos;
                }
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        let t = Instant::now();
        let pos = wait_for("playback", &|p| p >= 2.0);
        eprintln!("playing: {pos:.1}s after {:?}", t.elapsed());
        // It keeps up: 15 more seconds play in about 15 seconds, without pausing to buffer.
        let t = Instant::now();
        let mut stalls = 0;
        let mut was_waiting = false;
        let pos = loop {
            let _ = mpv.poll_events();
            let waiting = mpv.get_flag("paused-for-cache").unwrap_or(false);
            stalls += usize::from(waiting && !was_waiting);
            was_waiting = waiting;
            let pos = mpv.get_f64("time-pos").unwrap_or(0.0);
            if pos >= 17.0 || t.elapsed() > Duration::from_secs(40) {
                break pos;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        eprintln!("played to {pos:.1}s in {:?} with {stalls} stalls", t.elapsed());
        assert!(pos >= 17.0 && stalls == 0, "buffering: {stalls} stalls, reached {pos:.1}s");
        let t = Instant::now();
        mpv.command(&["seek", "300", "absolute"]).unwrap();
        let pos = wait_for("seek", &|p| p >= 300.0);
        eprintln!("seeked to {pos:.1}s in {:?}", t.elapsed());
    }
}
