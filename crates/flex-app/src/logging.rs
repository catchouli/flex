//! Logging to a file (Windows release builds have no console) and crash reports.
//!
//! - `logs/flex.log` in the data folder holds this run's log; the previous run's is kept as
//!   `flex.previous.log`.
//! - A panic, or on Windows a native crash (e.g. inside mpv or the graphics driver), writes a
//!   `crash-<time>` folder with `crash.txt` (what happened and where), a copy of the log and,
//!   on Windows, a minidump (`flex.dmp`). It goes in the folder chosen in Settings → "Crash
//!   reports folder", or the logs folder.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

pub fn logs_dir() -> PathBuf {
    flex_core::data_dir().join("logs")
}

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();

/// Setting: the folder crash reports go in (empty = the logs folder).
pub const CRASH_DIR_SETTING: &str = "crash_dir";

/// The folder from [`CRASH_DIR_SETTING`], if one is set.
static CRASH_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_crash_dir(dir: Option<PathBuf>) {
    *CRASH_DIR.lock().unwrap_or_else(|e| e.into_inner()) = dir;
}

/// Where crash reports go.
pub fn crash_dir() -> PathBuf {
    // try_lock: never wait inside a crash.
    let chosen = CRASH_DIR.try_lock().ok().and_then(|d| d.clone());
    chosen.unwrap_or_else(logs_dir)
}

/// Sends log output to the log file and stderr.
struct Tee;

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(file) = LOG_FILE.get() {
            let _ = file.lock().unwrap_or_else(|e| e.into_inner()).write_all(buf);
        }
        let _ = std::io::stderr().write_all(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(file) = LOG_FILE.get() {
            let _ = file.lock().unwrap_or_else(|e| e.into_inner()).flush();
        }
        Ok(())
    }
}

fn open_log_file() -> std::io::Result<File> {
    let dir = logs_dir();
    std::fs::create_dir_all(&dir)?;
    let log = dir.join("flex.log");
    if log.exists() {
        let _ = std::fs::rename(&log, dir.join("flex.previous.log"));
    }
    File::create(log)
}

pub fn init() {
    match open_log_file() {
        Ok(file) => {
            let _ = LOG_FILE.set(Mutex::new(file));
        }
        Err(e) => eprintln!("couldn't create the log file: {e}"),
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Pipe(Box::new(Tee)))
        .init();
    log::info!("Flex {} on {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS);
    install_panic_hook();
    #[cfg(windows)]
    windows::install_crash_handler();
}

/// Writes a crash report folder and returns its path. `dump` writes the minidump into the
/// path it's given (Windows only).
fn write_crash_report(what: &str, dump: impl FnOnce(&std::path::Path) -> Result<(), String>) -> Option<PathBuf> {
    let name = format!("crash-{}", chrono::Local::now().format("%Y-%m-%d-%H%M%S"));
    // The chosen folder may be a share that's gone away: fall back to the logs folder.
    let dir = [crash_dir(), logs_dir()]
        .into_iter()
        .map(|d| d.join(&name))
        .find(|d| std::fs::create_dir_all(d).is_ok())?;
    let _ = std::fs::copy(logs_dir().join("flex.log"), dir.join("flex.log"));
    let dumped = if cfg!(windows) {
        match dump(&dir.join("flex.dmp")) {
            Ok(()) => " The minidump is flex.dmp.".to_owned(),
            Err(e) => format!(" Couldn't write a minidump: {e}"),
        }
    } else {
        String::new()
    };
    let report = format!(
        "Flex {} crashed on {}.\n\n{what}\n\nThe log up to the crash is flex.log in this folder.{dumped}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
    );
    std::fs::write(dir.join("crash.txt"), report).ok()?;
    Some(dir)
}

fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_owned();
        let what = format!("Panic in thread '{thread}': {info}\n\nBacktrace:\n{backtrace}");
        log::error!("{what}");
        log::logger().flush();
        #[cfg(windows)]
        let dump = |path: &std::path::Path| windows::write_minidump(path, std::ptr::null());
        #[cfg(not(windows))]
        let dump = |_: &std::path::Path| Ok(());
        if let Some(path) = write_crash_report(&what, dump) {
            eprintln!("crash report written to {}", path.display());
        }
        default(info);
    }));
}

#[cfg(windows)]
mod windows {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpWithFullMemoryInfo,
        MiniDumpWithIndirectlyReferencedMemory, MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump,
        SetUnhandledExceptionFilter,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
        GetModuleHandleExW,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId};

    /// Writes a minidump of this process to `path`. `info` is the crash's exception (null
    /// for a panic, where the dump just shows every thread's stack).
    pub fn write_minidump(path: &std::path::Path, info: *const EXCEPTION_POINTERS) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let exception = MINIDUMP_EXCEPTION_INFORMATION {
            ThreadId: unsafe { GetCurrentThreadId() },
            ExceptionPointers: info as *mut _,
            ClientPointers: 0,
        };
        // Stacks plus the memory they point at: enough for locals and a few levels of
        // heap objects, while staying a few MB.
        let kind = MiniDumpWithIndirectlyReferencedMemory
            | MiniDumpWithThreadInfo
            | MiniDumpWithUnloadedModules
            | MiniDumpWithFullMemoryInfo;
        // Safety: the handles are this process and a file we keep open for the call.
        let ok = unsafe {
            MiniDumpWriteDump(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                file.as_raw_handle(),
                kind,
                if info.is_null() { std::ptr::null() } else { &exception },
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if ok == 0 { Err(std::io::Error::last_os_error().to_string()) } else { Ok(()) }
    }

    /// "libmpv-2.dll+0x1a2b3c" for a code address.
    fn locate(address: usize) -> String {
        let mut module: HMODULE = std::ptr::null_mut();
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        // Safety: with FROM_ADDRESS the "name" argument is the address to look up.
        if unsafe { GetModuleHandleExW(flags, address as *const u16, &mut module) } == 0 {
            return format!("{address:#x} (unknown module)");
        }
        let mut name = [0u16; 512];
        let len = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) } as usize;
        let path = String::from_utf16_lossy(&name[..len]);
        let file = path.rsplit('\\').next().unwrap_or(&path).to_owned();
        format!("{file}+{:#x}", address - module as usize)
    }

    unsafe extern "system" fn on_crash(info: *const EXCEPTION_POINTERS) -> i32 {
        // Safety: Windows passes valid exception pointers to the filter.
        let what = unsafe {
            let record = &*(*info).ExceptionRecord;
            let code = record.ExceptionCode as u32;
            let name = match code {
                0xC0000005 => " (access violation)",
                0xC00000FD => " (stack overflow)",
                0xC000001D => " (illegal instruction)",
                0xC0000409 => " (stack buffer overrun / fail-fast)",
                _ => "",
            };
            format!(
                "Native crash: exception {code:#010x}{name} at {}",
                locate(record.ExceptionAddress as usize)
            )
        };
        log::error!("{what}");
        log::logger().flush();
        let _ = super::write_crash_report(&what, |path| write_minidump(path, info));
        0 // EXCEPTION_CONTINUE_SEARCH: let Windows handle it as usual
    }

    pub fn install_crash_handler() {
        unsafe { SetUnhandledExceptionFilter(Some(on_crash)) };
    }
}

/// Opens the logs folder in Explorer / Finder.
pub fn open_logs_folder() {
    let dir = logs_dir();
    let _ = std::fs::create_dir_all(&dir);
    open_folder(&dir);
}

/// Opens `dir` in Explorer / Finder.
pub fn open_folder(dir: &std::path::Path) {
    let program = if cfg!(windows) { "explorer" } else { "open" };
    if let Err(e) = std::process::Command::new(program).arg(dir).spawn() {
        log::warn!("opening {}: {e}", dir.display());
    }
}
