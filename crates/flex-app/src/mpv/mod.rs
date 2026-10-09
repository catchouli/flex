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
        let api = ffi::api().map_err(|e| anyhow!(e))?;
        let version = unsafe { (api.mpv_client_api_version)() };
        if version >> 16 != 2 {
            bail!("unsupported libmpv client API version {}.{}", version >> 16, version & 0xffff);
        }

        let handle = unsafe { (api.mpv_create)() };
        if handle.is_null() {
            bail!("mpv_create failed");
        }
        let mut mpv = Self { api, handle, wakeup_ctx: ptr::null_mut() };

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
        for (k, v) in defaults.iter().chain(options) {
            mpv.set_option(k, v)?;
        }
        mpv.check(unsafe { (api.mpv_initialize)(handle) }, "initialize")?;
        unsafe { (api.mpv_request_log_messages)(handle, c"warn".as_ptr()) };

        mpv.wakeup_ctx = Box::into_raw(Box::new(egui_ctx.clone()));
        unsafe { (api.mpv_set_wakeup_callback)(handle, Some(request_repaint), mpv.wakeup_ctx.cast()) };
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
            let ev = unsafe { &*(self.api.mpv_wait_event)(self.handle, 0.0) };
            match ev.event_id {
                ffi::EVENT_NONE => break,
                ffi::EVENT_SHUTDOWN => events.push(Event::Shutdown),
                ffi::EVENT_FILE_LOADED => events.push(Event::FileLoaded),
                ffi::EVENT_END_FILE => {
                    let data = unsafe { &*(ev.data as *const ffi::EventEndFile) };
                    let error = (data.reason == ffi::END_FILE_REASON_ERROR).then(|| {
                        let msg = unsafe { CStr::from_ptr((self.api.mpv_error_string)(data.error)) };
                        msg.to_string_lossy().into_owned()
                    });
                    events.push(Event::EndFile { eof: data.reason == ffi::END_FILE_REASON_EOF, error });
                }
                ffi::EVENT_LOG_MESSAGE => {
                    let m = unsafe { &*(ev.data as *const ffi::EventLogMessage) };
                    let (prefix, text) = unsafe { (CStr::from_ptr(m.prefix), CStr::from_ptr(m.text)) };
                    log::warn!("mpv[{}] {}", prefix.to_string_lossy(), text.to_string_lossy().trim_end());
                }
                _ => {}
            }
        }
        events
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
