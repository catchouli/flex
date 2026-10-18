//! Keeps the computer from sleeping while something plays. mpv would do this for its own
//! window, but Flex draws mpv's picture itself, so it has to ask for itself.

/// What playback needs kept on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Need {
    /// A video: the screen and the computer.
    Screen,
    /// Music: only the computer; the screen may go off.
    System,
}

/// Holds the request while playback needs it. Dropping it lets the computer sleep again.
#[derive(Default)]
pub struct KeepAwake {
    held: Option<(Need, imp::Hold)>,
    failed: bool,
}

impl KeepAwake {
    /// Asks for `need` (or lets go with `None`); only does anything when it changes.
    pub fn set(&mut self, need: Option<Need>) {
        if self.held.as_ref().map(|(n, _)| *n) == need {
            return;
        }
        // Let go of the old one first: on Windows they're the same switch.
        self.held = None;
        let Some(need) = need else { return };
        match imp::Hold::new(need) {
            Ok(hold) => self.held = Some((need, hold)),
            // Once is enough in the log; this runs at every play and pause.
            Err(e) if !self.failed => {
                self.failed = true;
                log::warn!("couldn't keep the computer awake while playing: {e}");
            }
            Err(_) => {}
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::Need;
    use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState};

    /// Windows' own setting for media players. It belongs to the thread that set it, which
    /// is always Flex's UI thread.
    pub struct Hold;

    impl Hold {
        pub fn new(need: Need) -> Result<Self, String> {
            let display = if need == Need::Screen { ES_DISPLAY_REQUIRED } else { 0 };
            let previous = unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | display) };
            if previous == 0 { Err("Windows refused".into()) } else { Ok(Hold) }
        }
    }

    impl Drop for Hold {
        fn drop(&mut self) {
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::Need;
    use std::process::{Child, Command, Stdio};

    /// A helper that holds the request for as long as it runs: macOS's `caffeinate`, or
    /// systemd's `systemd-inhibit` elsewhere.
    pub struct Hold(Child);

    impl Hold {
        pub fn new(need: Need) -> Result<Self, String> {
            let mut cmd = command(need);
            // If Flex dies without stopping it, the helper goes too: `caffeinate -w` watches
            // Flex, and `cat` ends when Flex's end of its stdin closes.
            cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
            let name = cmd.get_program().to_string_lossy().into_owned();
            cmd.spawn().map(Hold).map_err(|e| format!("{name}: {e}"))
        }
    }

    #[cfg(target_os = "macos")]
    fn command(need: Need) -> Command {
        let mut cmd = Command::new("caffeinate");
        cmd.arg(if need == Need::Screen { "-di" } else { "-i" }).args(["-w", &std::process::id().to_string()]);
        cmd
    }

    #[cfg(not(target_os = "macos"))]
    fn command(need: Need) -> Command {
        // "idle" also stops logind blanking the screen; music only needs "sleep".
        let what = if need == Need::Screen { "idle:sleep" } else { "sleep" };
        let mut cmd = Command::new("systemd-inhibit");
        cmd.args([&format!("--what={what}"), "--who=Flex", "--why=Playing", "--mode=block", "cat"]);
        cmd
    }

    impl Drop for Hold {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
