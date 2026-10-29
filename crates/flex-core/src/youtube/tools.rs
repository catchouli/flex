//! Keeps yt-dlp and Deno in the data folder's `tools`, downloaded from their GitHub
//! releases and checked against GitHub's SHA-256. YouTube changes often and yt-dlp keeps
//! up, so it's checked for updates daily, and straight away when playback fails.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::tools_dir;
use crate::update::download_verified;

const USER_AGENT: &str = concat!("flex/", env!("CARGO_PKG_VERSION"));
/// How often to look for new versions (unless asked).
const CHECK_EVERY_SECS: i64 = 24 * 3600;

/// A program Flex keeps up to date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    YtDlp,
    Deno,
}

impl Tool {
    pub const ALL: [Tool; 2] = [Tool::YtDlp, Tool::Deno];

    pub fn name(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp",
            Tool::Deno => "deno",
        }
    }

    fn repo(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp/yt-dlp",
            Tool::Deno => "denoland/deno",
        }
    }

    /// The release file for this platform, or `None` where there isn't one.
    fn asset(self) -> Option<&'static str> {
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        Some(match (self, os, arch) {
            (Tool::YtDlp, "windows", _) => "yt-dlp.exe",
            (Tool::YtDlp, "macos", _) => "yt-dlp_macos",
            (Tool::YtDlp, "linux", "aarch64") => "yt-dlp_linux_aarch64",
            (Tool::YtDlp, "linux", _) => "yt-dlp_linux",
            (Tool::Deno, "windows", _) => "deno-x86_64-pc-windows-msvc.zip",
            (Tool::Deno, "macos", "aarch64") => "deno-aarch64-apple-darwin.zip",
            (Tool::Deno, "macos", _) => "deno-x86_64-apple-darwin.zip",
            (Tool::Deno, "linux", "aarch64") => "deno-aarch64-unknown-linux-gnu.zip",
            (Tool::Deno, "linux", _) => "deno-x86_64-unknown-linux-gnu.zip",
            _ => return None,
        })
    }

    /// Where it's installed.
    pub fn path(self) -> PathBuf {
        tools_dir().join(super::exe(self.name()))
    }
}

/// What's installed: release tags, and when updates were last looked for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Installed {
    #[serde(default)]
    pub yt_dlp: Option<String>,
    #[serde(default)]
    pub deno: Option<String>,
    #[serde(default)]
    pub checked_at: i64,
}

impl Installed {
    fn file() -> PathBuf {
        tools_dir().join("versions.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self) -> Result<()> {
        std::fs::write(Self::file(), serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn version(&self, tool: Tool) -> Option<&str> {
        match tool {
            Tool::YtDlp => self.yt_dlp.as_deref(),
            Tool::Deno => self.deno.as_deref(),
        }
    }

    fn set_version(&mut self, tool: Tool, tag: String) {
        match tool {
            Tool::YtDlp => self.yt_dlp = Some(tag),
            Tool::Deno => self.deno = Some(tag),
        }
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
    digest: Option<String>,
}

fn latest_release(repo: &str) -> Result<Release> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).build().into();
    agent
        .get(&format!("https://api.github.com/repos/{repo}/releases/latest"))
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()
        .with_context(|| format!("asking GitHub about {repo}"))?
        .body_mut()
        .read_json()
        .with_context(|| format!("reading GitHub's answer about {repo}"))
}

/// What [`update`] did.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// Tools installed or updated, with their new versions.
    pub updated: Vec<(Tool, String)>,
    /// Whether it looked at all (it only does once a day unless forced).
    pub checked: bool,
}

/// Installs missing tools and updates old ones. Without `force`, only looks once a day
/// (missing tools are always installed). `now` is a Unix time.
pub fn update(force: bool, now: i64) -> Result<Report> {
    let dir = tools_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut installed = Installed::load();
    let missing = Tool::ALL.iter().any(|t| t.asset().is_some() && !t.path().is_file());
    if !force && !missing && now - installed.checked_at < CHECK_EVERY_SECS {
        return Ok(Report::default());
    }
    let mut report = Report { checked: true, ..Report::default() };
    let mut first_error = None;
    for tool in Tool::ALL {
        let Some(asset_name) = tool.asset() else { continue };
        let result = (|| -> Result<()> {
            let release = latest_release(tool.repo())?;
            if tool.path().is_file() && installed.version(tool) == Some(release.tag_name.as_str()) {
                return Ok(());
            }
            let asset = release
                .assets
                .iter()
                .find(|a| a.name == asset_name)
                .with_context(|| format!("{} {} has no {asset_name}", tool.name(), release.tag_name))?;
            let sha256 = asset
                .digest
                .as_deref()
                .and_then(|d| d.strip_prefix("sha256:"))
                .with_context(|| format!("GitHub has no SHA-256 for {asset_name}"))?
                .to_ascii_lowercase();
            let download = dir.join(format!("{asset_name}.download"));
            log::info!("youtube: downloading {} {}", tool.name(), release.tag_name);
            download_verified(&asset.browser_download_url, asset_name, asset.size, &sha256, &download, |_| {})?;
            install(tool, &download)?;
            let _ = std::fs::remove_file(&download);
            installed.set_version(tool, release.tag_name.clone());
            report.updated.push((tool, release.tag_name));
            Ok(())
        })();
        if let Err(e) = result {
            log::warn!("youtube: updating {}: {e:#}", tool.name());
            first_error.get_or_insert(e);
        }
    }
    installed.checked_at = now;
    installed.save()?;
    match first_error {
        // yt-dlp is the one that matters; a missing Deno only costs some formats.
        Some(e) if !Tool::YtDlp.path().is_file() => Err(e),
        _ => Ok(report),
    }
}

/// Puts a downloaded release file in place: the program itself, or out of its zip.
fn install(tool: Tool, download: &Path) -> Result<()> {
    let dest = tool.path();
    let staged = dest.with_extension("new");
    if tool.asset().is_some_and(|a| a.ends_with(".zip")) {
        let file = std::fs::File::open(download)?;
        let mut zip = zip::ZipArchive::new(file).context("opening the zip")?;
        let wanted = super::exe(tool.name());
        let mut entry = zip.by_name(&wanted).with_context(|| format!("no {wanted} in the zip"))?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        std::fs::write(&staged, bytes)?;
    } else {
        std::fs::copy(download, &staged)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    // Only while yt-dlp isn't running (see `InUse`): a channel page keeps it going for as
    // long as it's open, so wait a while for that.
    let mut result = Ok(());
    let swapped = super::when_unused(SWAP_PATIENCE, || result = std::fs::rename(&staged, &dest));
    if !swapped {
        bail!("yt-dlp stayed in use; trying again next time");
    }
    result.with_context(|| format!("replacing {}", dest.display()))
}

/// How long an update waits for yt-dlp to stop being used before giving up until next time.
const SWAP_PATIENCE: Duration = Duration::from_secs(30 * 60);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_desktop_platform_has_files() {
        // This platform at least: the ones Flex runs on all have both.
        assert!(Tool::YtDlp.asset().is_some());
        assert!(Tool::Deno.asset().is_some_and(|a| a.ends_with(".zip")));
    }

    #[test]
    fn swaps_only_when_unused() {
        let in_use = super::super::InUse::new();
        let mut swapped = false;
        assert!(!super::super::when_unused(Duration::from_millis(600), || swapped = true), "gives up");
        assert!(!swapped);
        drop(in_use);
        assert!(super::super::when_unused(Duration::ZERO, || swapped = true));
        assert!(swapped);
    }

    #[test]
    fn installed_versions_round_trip() {
        let mut v = Installed::default();
        v.set_version(Tool::YtDlp, "2026.08.19".into());
        let json = serde_json::to_vec(&v).unwrap();
        let back: Installed = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.version(Tool::YtDlp), Some("2026.08.19"));
        assert_eq!(back.version(Tool::Deno), None);
        // Older or unknown fields don't break loading.
        let old: Installed = serde_json::from_str(r#"{"yt_dlp":"x","something":1}"#).unwrap();
        assert_eq!(old.checked_at, 0);
    }

    /// Downloads both tools for real into a scratch data folder:
    /// `cargo test -p flex-core installs_tools -- --ignored`.
    #[test]
    #[ignore]
    fn installs_tools() {
        let dir = std::env::temp_dir().join(format!("flex-tools-{}", std::process::id()));
        // SAFETY: tests that read FLEX_DATA_DIR don't run alongside this ignored one.
        unsafe { std::env::set_var("FLEX_DATA_DIR", &dir) };
        let report = update(false, 1000).unwrap();
        assert_eq!(report.updated.len(), 2, "{report:?}");
        let tools = super::super::Tools::find().unwrap();
        assert!(tools.yt_dlp.starts_with(&dir) && tools.deno.as_ref().is_some_and(|d| d.starts_with(&dir)));
        assert!(tools.yt_dlp_version().is_some());
        // Up to date and checked recently: nothing to do.
        assert_eq!(update(false, 2000).unwrap(), Report::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
