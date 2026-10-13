//! "Start with Windows": a per-user Run entry that opens Flex hidden in the tray.

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "Flex";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn is_enabled() -> bool {
    let (key, value) = (wide(RUN_KEY), wide(VALUE));
    let mut size = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    status == ERROR_SUCCESS
}

pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    let (key, value) = (wide(RUN_KEY), wide(VALUE));
    let status = if enabled {
        let exe = std::env::current_exe()?;
        let command = wide(&format!("\"{}\" --tray", exe.display()));
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) }
    };
    anyhow::ensure!(status == ERROR_SUCCESS, "registry error {status}");
    Ok(())
}

/// Points an existing entry at this copy of flex.exe, in case it was moved or updated.
pub fn refresh() {
    if is_enabled()
        && let Err(e) = set_enabled(true)
    {
        log::warn!("couldn't update the startup entry: {e:#}");
    }
}
