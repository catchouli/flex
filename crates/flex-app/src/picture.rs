//! Picture enhancements, both off until switched on in Settings → Picture: Anime4K
//! upscaling (shaders bundled with Flex) and 60fps motion smoothing through SVP, when it's
//! installed. Once one is on, every library uses it unless switched off in that library.

use std::path::{Path, PathBuf};

/// Anime4K: "off", "fast" or "hq".
pub const UPSCALE_SETTING: &str = "picture_upscale";
/// 60fps smoothing through SVP.
pub const SMOOTH_SETTING: &str = "picture_smooth";

/// A library's own switches ("false" = not in this library).
pub fn upscale_key(library_id: i64) -> String {
    format!("{UPSCALE_SETTING}:{library_id}")
}

pub fn smooth_key(library_id: i64) -> String {
    format!("{SMOOTH_SETTING}:{library_id}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upscale {
    Off,
    Fast,
    High,
}

impl Upscale {
    pub const ALL: [Upscale; 3] = [Upscale::Off, Upscale::Fast, Upscale::High];

    pub fn from_setting(s: Option<&str>) -> Self {
        match s {
            Some("fast") => Self::Fast,
            Some("hq") => Self::High,
            _ => Self::Off,
        }
    }

    pub fn setting(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Fast => "fast",
            Self::High => "hq",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Fast => "fast",
            Self::High => "high quality",
        }
    }

    pub fn next(self) -> Self {
        Self::ALL[(Self::ALL.iter().position(|u| *u == self).unwrap_or(0) + 1) % Self::ALL.len()]
    }

    /// Anime4K's "Mode A" presets: restore lines, then upscale. "fast" is for most GPUs,
    /// "high quality" for strong ones. (The upscalers skip themselves when the video
    /// already fills the screen.)
    fn shaders(self) -> &'static [&'static str] {
        match self {
            Self::Off => &[],
            Self::Fast => &[
                "Anime4K_Clamp_Highlights.glsl",
                "Anime4K_Restore_CNN_M.glsl",
                "Anime4K_Upscale_CNN_x2_M.glsl",
                "Anime4K_AutoDownscalePre_x2.glsl",
                "Anime4K_AutoDownscalePre_x4.glsl",
                "Anime4K_Upscale_CNN_x2_S.glsl",
            ],
            Self::High => &[
                "Anime4K_Clamp_Highlights.glsl",
                "Anime4K_Restore_CNN_VL.glsl",
                "Anime4K_Upscale_CNN_x2_VL.glsl",
                "Anime4K_AutoDownscalePre_x2.glsl",
                "Anime4K_AutoDownscalePre_x4.glsl",
                "Anime4K_Upscale_CNN_x2_M.glsl",
            ],
        }
    }
}

/// The shaders, from <https://github.com/bloc97/Anime4K> (MIT, see shaders/anime4k/LICENSE).
const SHADERS: &[(&str, &str)] = &[
    ("Anime4K_AutoDownscalePre_x2.glsl", include_str!("../shaders/anime4k/Anime4K_AutoDownscalePre_x2.glsl")),
    ("Anime4K_AutoDownscalePre_x4.glsl", include_str!("../shaders/anime4k/Anime4K_AutoDownscalePre_x4.glsl")),
    ("Anime4K_Clamp_Highlights.glsl", include_str!("../shaders/anime4k/Anime4K_Clamp_Highlights.glsl")),
    ("Anime4K_Restore_CNN_M.glsl", include_str!("../shaders/anime4k/Anime4K_Restore_CNN_M.glsl")),
    ("Anime4K_Restore_CNN_VL.glsl", include_str!("../shaders/anime4k/Anime4K_Restore_CNN_VL.glsl")),
    ("Anime4K_Upscale_CNN_x2_M.glsl", include_str!("../shaders/anime4k/Anime4K_Upscale_CNN_x2_M.glsl")),
    ("Anime4K_Upscale_CNN_x2_S.glsl", include_str!("../shaders/anime4k/Anime4K_Upscale_CNN_x2_S.glsl")),
    ("Anime4K_Upscale_CNN_x2_VL.glsl", include_str!("../shaders/anime4k/Anime4K_Upscale_CNN_x2_VL.glsl")),
];

/// mpv takes shaders as files, so they're written to the data folder (again whenever
/// they differ, e.g. after an update).
fn shader_files(upscale: Upscale) -> std::io::Result<Vec<PathBuf>> {
    let dir = flex_core::data_dir().join("shaders").join("anime4k");
    std::fs::create_dir_all(&dir)?;
    let mut out = Vec::new();
    for name in upscale.shaders() {
        let (_, text) = SHADERS.iter().find(|(n, _)| n == name).expect("bundled shader");
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(*text) {
            std::fs::write(&path, text)?;
        }
        out.push(path);
    }
    Ok(out)
}

/// What a video gets, from the global settings and its library's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Picture {
    pub upscale: Option<Upscale>,
    pub smooth: bool,
}

impl Picture {
    /// Extra mpv options (set before mpv starts).
    pub fn mpv_options(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(upscale) = self.upscale {
            match shader_files(upscale) {
                Ok(files) => {
                    let sep = if cfg!(windows) { ";" } else { ":" };
                    let list: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
                    out.push(("glsl-shaders", list.join(sep)));
                }
                Err(e) => log::warn!("writing the Anime4K shaders: {e}"),
            }
        }
        if self.smooth {
            // SVP's manager watches this pipe and adds its VapourSynth filter. VapourSynth
            // needs decoded frames in memory, so hardware decoding must copy them back.
            out.push(("input-ipc-server", svp::PIPE.to_owned()));
            out.push(("hwdec", "auto-copy-safe".to_owned()));
            out.push(("hr-seek-framedrop", "no".to_owned()));
        }
        out
    }

    /// For the stats overlay.
    pub fn describe(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(upscale) = self.upscale {
            parts.push(format!("Anime4K ({})", upscale.label()));
        }
        if self.smooth {
            parts.push("SVP 60fps".to_owned());
        }
        (!parts.is_empty()).then(|| parts.join(" + "))
    }
}

/// SVP (SmoothVideo Project, <https://www.svp-team.com>), a paid app that adds frames to
/// video in real time. Flex doesn't drive it: SVP's manager notices mpv through its IPC
/// pipe and filters the video itself, so SVP must be running.
pub mod svp {
    use super::*;

    /// The pipe SVP's manager looks for by default.
    #[cfg(windows)]
    pub const PIPE: &str = "mpvpipe";
    #[cfg(not(windows))]
    pub const PIPE: &str = "/tmp/mpvsocket";

    /// Overrides where SVP is looked for.
    const DIR_ENV: &str = "FLEX_SVP_DIR";

    pub struct Svp {
        pub dir: PathBuf,
        /// SVP's copy of libmpv (built with VapourSynth, which SVP needs), if it has one.
        pub libmpv_dir: Option<PathBuf>,
    }

    fn places() -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::env::var_os(DIR_ENV).map(PathBuf::from).into_iter().collect();
        #[cfg(windows)]
        for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
            if let Some(dir) = std::env::var_os(var) {
                out.push(Path::new(&dir).join("SVP 4"));
            }
        }
        #[cfg(target_os = "macos")]
        out.push(PathBuf::from("/Applications/SVP 4 Mac.app"));
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if let Some(home) = std::env::var_os("HOME") {
                out.push(Path::new(&home).join("SVP 4"));
            }
            out.push(PathBuf::from("/opt/svp"));
        }
        out
    }

    /// Whether `dir` looks like an SVP install.
    fn is_svp(dir: &Path) -> bool {
        if cfg!(target_os = "macos") {
            dir.join("Contents").is_dir()
        } else {
            ["SVPManager.exe", "SVPManager"].iter().any(|f| dir.join(f).is_file())
        }
    }

    /// The installed SVP, if any.
    pub fn find() -> Option<Svp> {
        let dir = places().into_iter().find(|d| is_svp(d))?;
        let mpv = dir.join("mpv64");
        let libmpv_dir = crate::mpv::ffi::NAMES.iter().any(|n| mpv.join(n).is_file()).then_some(mpv);
        Some(Svp { dir, libmpv_dir })
    }

    /// Whether Flex is playing through SVP's libmpv (or SVP doesn't come with one).
    pub fn using_its_libmpv(svp: &Svp) -> bool {
        match (&svp.libmpv_dir, crate::mpv::ffi::loaded_from()) {
            (Some(dir), Some(loaded)) => loaded.parent() == Some(dir.as_path()),
            (Some(_), None) => false,
            (None, _) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_use_bundled_shaders() {
        for upscale in Upscale::ALL {
            for name in upscale.shaders() {
                assert!(SHADERS.iter().any(|(n, _)| n == name), "{name} isn't bundled");
            }
            assert_eq!(Upscale::from_setting(Some(upscale.setting())), upscale);
        }
        assert_eq!(Upscale::from_setting(None), Upscale::Off);
    }
}
