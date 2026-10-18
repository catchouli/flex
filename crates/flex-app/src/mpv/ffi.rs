//! Minimal libmpv bindings, loaded at runtime so the app builds without mpv installed.
//! Constants and layouts mirror mpv's `client.h`, `render.h` and `render_gl.h` (client API 2.x).

use std::ffi::{c_char, c_double, c_int, c_void};
use std::path::PathBuf;
use std::sync::OnceLock;

use libloading::Library;

pub type Handle = c_void;
pub type RenderContext = c_void;

pub const FORMAT_STRING: c_int = 1;
pub const FORMAT_FLAG: c_int = 3;
pub const FORMAT_INT64: c_int = 4;
pub const FORMAT_DOUBLE: c_int = 5;

pub const EVENT_NONE: c_int = 0;
pub const EVENT_SHUTDOWN: c_int = 1;
pub const EVENT_LOG_MESSAGE: c_int = 2;
pub const EVENT_END_FILE: c_int = 7;
pub const EVENT_FILE_LOADED: c_int = 8;

pub const END_FILE_REASON_EOF: c_int = 0;
pub const END_FILE_REASON_ERROR: c_int = 4;

pub const RENDER_PARAM_INVALID: c_int = 0;
pub const RENDER_PARAM_API_TYPE: c_int = 1;
pub const RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
pub const RENDER_PARAM_OPENGL_FBO: c_int = 3;
pub const RENDER_PARAM_FLIP_Y: c_int = 4;

#[repr(C)]
pub struct Event {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct EventLogMessage {
    pub prefix: *const c_char,
    pub level: *const c_char,
    pub text: *const c_char,
    pub log_level: c_int,
}

/// Only the leading field is read; the rest of `mpv_event_end_file` is left undeclared.
#[repr(C)]
pub struct EventEndFile {
    pub reason: c_int,
    pub error: c_int,
}

#[repr(C)]
pub struct RenderParam {
    pub kind: c_int,
    pub data: *mut c_void,
}

pub type GetProcAddressFn = unsafe extern "C" fn(ctx: *mut c_void, name: *const c_char) -> *mut c_void;

#[repr(C)]
pub struct OpenGlInitParams {
    pub get_proc_address: Option<GetProcAddressFn>,
    pub get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
pub struct OpenGlFbo {
    pub fbo: c_int,
    pub w: c_int,
    pub h: c_int,
    pub internal_format: c_int,
}

pub type Callback = unsafe extern "C" fn(ctx: *mut c_void);

macro_rules! api {
    ($($name:ident : fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        #[allow(non_snake_case)]
        pub struct Api {
            $(pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
            _lib: Library,
        }

        impl Api {
            unsafe fn load(lib: Library) -> Result<Self, libloading::Error> {
                unsafe {
                    Ok(Self {
                        $($name: *lib.get(concat!(stringify!($name), "\0").as_bytes())?,)*
                        _lib: lib,
                    })
                }
            }
        }
    };
}

api! {
    mpv_client_api_version: fn() -> u64;
    mpv_error_string: fn(c_int) -> *const c_char;
    mpv_free: fn(*mut c_void);
    mpv_create: fn() -> *mut Handle;
    mpv_initialize: fn(*mut Handle) -> c_int;
    mpv_terminate_destroy: fn(*mut Handle);
    mpv_set_option_string: fn(*mut Handle, *const c_char, *const c_char) -> c_int;
    mpv_command: fn(*mut Handle, *mut *const c_char) -> c_int;
    mpv_set_property: fn(*mut Handle, *const c_char, c_int, *mut c_void) -> c_int;
    mpv_get_property: fn(*mut Handle, *const c_char, c_int, *mut c_void) -> c_int;
    mpv_request_log_messages: fn(*mut Handle, *const c_char) -> c_int;
    mpv_wait_event: fn(*mut Handle, c_double) -> *mut Event;
    mpv_set_wakeup_callback: fn(*mut Handle, Option<Callback>, *mut c_void);
    mpv_render_context_create: fn(*mut *mut RenderContext, *mut Handle, *mut RenderParam) -> c_int;
    mpv_render_context_set_update_callback: fn(*mut RenderContext, Option<Callback>, *mut c_void);
    mpv_render_context_update: fn(*mut RenderContext) -> u64;
    mpv_render_context_render: fn(*mut RenderContext, *mut RenderParam) -> c_int;
    mpv_render_context_report_swap: fn(*mut RenderContext);
    mpv_render_context_free: fn(*mut RenderContext);
}

/// Library file names to try, in order. Each is looked up next to the executable first,
/// then via the platform's normal library search.
#[cfg(target_os = "windows")]
pub const NAMES: &[&str] = &["libmpv-2.dll", "mpv-2.dll", "mpv-1.dll"];
#[cfg(target_os = "macos")]
pub const NAMES: &[&str] = &["libmpv.2.dylib", "libmpv.dylib"];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const NAMES: &[&str] = &["libmpv.so.2", "libmpv.so"];

/// Extra absolute locations checked after the executable's directory.
#[cfg(target_os = "macos")]
const EXTRA_DIRS: &[&str] = &[
    "/opt/homebrew/lib",
    "/usr/local/lib",
    "/opt/local/lib",
    // Development convenience: IINA bundles a complete libmpv.
    "/Applications/IINA.app/Contents/Frameworks",
];
#[cfg(not(target_os = "macos"))]
const EXTRA_DIRS: &[&str] = &[];

/// Environment variable that overrides the libmpv path entirely.
pub const PATH_ENV: &str = "FLEX_LIBMPV";

/// A folder to try before all the others (SVP's libmpv, see [`crate::picture`]).
static PREFERRED: OnceLock<PathBuf> = OnceLock::new();
/// Where libmpv was loaded from.
static LOADED: OnceLock<PathBuf> = OnceLock::new();

/// Tries libmpv in `dir` first. Only has an effect before libmpv is first loaded.
pub fn prefer_dir(dir: PathBuf) {
    let _ = PREFERRED.set(dir);
}

/// Where libmpv was loaded from, once it has been.
pub fn loaded_from() -> Option<&'static std::path::Path> {
    LOADED.get().map(PathBuf::as_path)
}

fn candidates() -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os(PATH_ENV) {
        return vec![PathBuf::from(path)];
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from));
    let mut out = Vec::new();
    for dir in PREFERRED.get().into_iter().cloned().chain(exe_dir).chain(EXTRA_DIRS.iter().map(PathBuf::from)) {
        out.extend(NAMES.iter().map(|n| dir.join(n)));
    }
    // Bare names last, so the OS search path (PATH / DYLD paths) gets a chance.
    out.extend(NAMES.iter().map(PathBuf::from));
    out
}

/// Loads libmpv once. Returns a human-readable error listing what was tried on failure.
pub fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| {
        let mut errors = Vec::new();
        for path in candidates() {
            // Safety: loading libmpv runs its (trusted) initialisers only.
            match unsafe { Library::new(&path) } {
                Ok(lib) => {
                    log::info!("libmpv: {}", path.display());
                    let _ = LOADED.set(path.clone());
                    return unsafe { Api::load(lib) }
                        .map_err(|e| format!("{} is missing symbols: {e}", path.display()));
                }
                Err(e) => errors.push(format!("  {}: {e}", path.display())),
            }
        }
        Err(format!("Could not load libmpv. Tried:\n{}", errors.join("\n")))
    })
    .as_ref()
    .map_err(Clone::clone)
}
