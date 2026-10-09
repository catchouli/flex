//! Logging to a file (Windows release builds have no console) and crash reports.
//!
//! - `logs/flex.log` in the data folder holds this run's log; the previous run's is kept as
//!   `flex.previous.log`.
//! - A panic, or on Windows a native crash (e.g. inside mpv or the graphics driver), writes
//!   `logs/crash-<time>.txt` with what happened and where.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

pub fn logs_dir() -> PathBuf {
    flex_core::data_dir().join("logs")
}

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();

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

/// Writes a crash report and returns its path.
fn write_crash_report(what: &str) -> Option<PathBuf> {
    let dir = logs_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let path = dir.join(format!("crash-{stamp}.txt"));
    let report = format!(
        "Flex {} crashed on {}.\n\n{what}\n\nThe log up to the crash is in flex.log (or flex.previous.log once Flex has been restarted).\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
    );
    std::fs::write(&path, report).ok()?;
    Some(path)
}

fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_owned();
        let what = format!("Panic in thread '{thread}': {info}\n\nBacktrace:\n{backtrace}");
        log::error!("{what}");
        log::logger().flush();
        if let Some(path) = write_crash_report(&what) {
            eprintln!("crash report written to {}", path.display());
        }
        default(info);
    }));
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter};
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
        GetModuleHandleExW,
    };

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
        let _ = super::write_crash_report(&what);
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
    let program = if cfg!(windows) { "explorer" } else { "open" };
    if let Err(e) = std::process::Command::new(program).arg(&dir).spawn() {
        log::warn!("opening {}: {e}", dir.display());
    }
}
