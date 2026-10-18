//! The system volume, for the phone remote's volume buttons. Flex has no volume of its
//! own: this is the same volume the keyboard's volume keys change.

/// Turns the system volume up or down a step. Returns the new level (0–100) where the
/// platform says, for showing on screen; Windows shows its own.
pub fn change(up: bool) -> Result<Option<u8>, String> {
    imp::change(up)
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, SendInput, VK_VOLUME_DOWN, VK_VOLUME_UP,
    };

    /// Presses the volume key, as a keyboard's would: Windows changes the volume (2% a
    /// press) and shows its volume display.
    pub fn change(up: bool) -> Result<Option<u8>, String> {
        let vk = if up { VK_VOLUME_UP } else { VK_VOLUME_DOWN };
        let key = |flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: KEYEVENTF_EXTENDEDKEY | flags, time: 0, dwExtraInfo: 0 } },
        };
        // Three presses: one is too small a step to notice from the sofa.
        let inputs: Vec<INPUT> = (0..3).flat_map(|_| [key(0), key(KEYEVENTF_KEYUP)]).collect();
        let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
        if sent as usize == inputs.len() { Ok(None) } else { Err("Windows didn't take the volume key".into()) }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::process::Command;

    pub fn change(up: bool) -> Result<Option<u8>, String> {
        let step = if up { "+ 6" } else { "- 6" };
        let script = format!(
            "set v to (output volume of (get volume settings)) {step}\n\
             if v < 0 then set v to 0\n\
             if v > 100 then set v to 100\n\
             set volume output volume v\n\
             return v"
        );
        let out = Command::new("osascript").args(["-e", &script]).output().map_err(|e| format!("osascript: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().parse().ok())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::process::Command;

    /// PipeWire's `wpctl`, else PulseAudio's `pactl` (PipeWire answers that too).
    pub fn change(up: bool) -> Result<Option<u8>, String> {
        let sign = if up { "+" } else { "-" };
        let wpctl = Command::new("wpctl").args(["set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", &format!("5%{sign}")]).status();
        if wpctl.is_ok_and(|s| s.success()) {
            return Ok(level(Command::new("wpctl").args(["get-volume", "@DEFAULT_AUDIO_SINK@"])));
        }
        let pactl = Command::new("pactl").args(["set-sink-volume", "@DEFAULT_SINK@", &format!("{sign}5%")]).status();
        match pactl {
            Ok(s) if s.success() => Ok(None),
            _ => Err("couldn't change the volume (needs wpctl or pactl)".into()),
        }
    }

    /// "Volume: 0.45" → 45.
    fn level(cmd: &mut Command) -> Option<u8> {
        let out = cmd.output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let v: f64 = text.split_whitespace().nth(1)?.parse().ok()?;
        Some((v * 100.0).round().clamp(0.0, 100.0) as u8)
    }
}
