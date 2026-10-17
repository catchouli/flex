//! Finding a newer Flex release on GitHub and downloading its Windows installer.
//!
//! This only talks to GitHub; the app decides when to check and when to install.

use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const LATEST_PAGE: &str = "https://github.com/catchouli/flex/releases/latest";
const LATEST_RELEASE: &str = "https://api.github.com/repos/catchouli/flex/releases/latest";
const USER_AGENT: &str = concat!("flex/", env!("CARGO_PKG_VERSION"));

/// A release version, like 0.3.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// Parses "0.3.1" or "v0.3.1".
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.strip_prefix('v').unwrap_or(s).split('.').map(|p| p.parse().ok());
        let version = Self(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }

    /// This build's version.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is X.Y.Z")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// A release's Windows installer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installer {
    pub version: Version,
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex, as GitHub reports it for the uploaded file.
    pub sha256: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
    digest: Option<String>,
}

/// The latest release's version, from where GitHub's "latest release" page redirects
/// (".../releases/tag/v0.3.2"). Unlike the API (60 requests an hour per address without
/// a login), this page is fine to ask every few minutes.
pub fn latest_version() -> Result<Version> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .max_redirects(0)
        .max_redirects_will_error(false)
        .build()
        .into();
    let response = agent.head(LATEST_PAGE).header("User-Agent", USER_AGENT).call().context("asking GitHub for the latest release")?;
    let location = response.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or_default();
    let tag = location.rsplit('/').next().unwrap_or_default();
    Version::parse(tag).with_context(|| format!("GitHub's latest release isn't a version (it points to {location:?})"))
}

/// The latest release's installer, if that release is newer than `current`.
pub fn newer_installer(current: Version) -> Result<Option<Installer>> {
    if latest_version()? <= current {
        return Ok(None);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).build().into();
    let release: Release = agent
        .get(LATEST_RELEASE)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("asking GitHub about the latest release")?
        .body_mut()
        .read_json()
        .context("reading GitHub's answer")?;
    pick(release, current)
}

fn pick(release: Release, current: Version) -> Result<Option<Installer>> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let Some(version) = Version::parse(&release.tag_name) else {
        bail!("the latest release's tag ({}) isn't a version", release.tag_name);
    };
    if version <= current {
        return Ok(None);
    }
    let name = format!("flex-v{version}-windows-x64-setup.exe");
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name == name)
        .with_context(|| format!("release {} has no {name}", release.tag_name))?;
    // The download is checked against GitHub's checksum, so one without it isn't used.
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .with_context(|| format!("GitHub has no SHA-256 for {name}"))?
        .to_ascii_lowercase();
    Ok(Some(Installer { version, name, url: asset.browser_download_url, size: asset.size, sha256 }))
}

/// Downloads the installer to `dest`, checking its size and SHA-256. A matching file
/// already there (from an earlier try) is kept. `progress` gets the bytes downloaded so
/// far as they arrive.
pub fn download(installer: &Installer, dest: &Path, progress: impl FnMut(u64)) -> Result<()> {
    download_verified(&installer.url, &installer.name, installer.size, &installer.sha256, dest, progress)
}

/// Downloads `url` to `dest`, checking its size and SHA-256 (lowercase hex); `name` is
/// for messages. A matching file already there is kept. `progress` gets the bytes so far.
pub fn download_verified(url: &str, name: &str, expected_size: u64, sha256: &str, dest: &Path, mut progress: impl FnMut(u64)) -> Result<()> {
    if dest.is_file() && file_sha256(dest).is_ok_and(|h| h == sha256) {
        progress(expected_size);
        return Ok(());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(Some(Duration::from_secs(30 * 60)))
        .build()
        .into();
    let mut response = agent.get(url).header("User-Agent", USER_AGENT).call().with_context(|| format!("downloading {name}"))?;
    let mut body = response.body_mut().as_reader();
    let partial = dest.with_extension("part");
    let mut file = File::create(&partial).with_context(|| format!("creating {}", partial.display()))?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buf = vec![0; 256 * 1024];
    loop {
        let n = body.read(&mut buf).with_context(|| format!("downloading {name}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])?;
        size += n as u64;
        progress(size);
    }
    file.sync_all()?;
    drop(file);
    let check = || -> Result<()> {
        ensure!(size == expected_size, "got {size} bytes of {name}, expected {expected_size}");
        ensure!(hex(&hasher.finalize()) == sha256, "{name} doesn't match its SHA-256");
        Ok(())
    };
    if let Err(e) = check() {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    std::fs::rename(&partial, dest).with_context(|| format!("saving {}", dest.display()))
}

fn file_sha256(path: &Path) -> Result<String> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    std::io::copy(&mut reader, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, assets: serde_json::Value) -> Release {
        serde_json::from_value(serde_json::json!({ "tag_name": tag, "assets": assets })).unwrap()
    }

    fn setup_asset(version: &str) -> serde_json::Value {
        serde_json::json!({
            "name": format!("flex-v{version}-windows-x64-setup.exe"),
            "size": 123,
            "browser_download_url": "https://example.com/setup.exe",
            "digest": format!("sha256:{}", "AB".repeat(32)),
        })
    }

    #[test]
    fn parses_and_orders_versions() {
        assert_eq!(Version::parse("v0.3.1"), Some(Version(0, 3, 1)));
        assert_eq!(Version::parse("1.10.0"), Some(Version(1, 10, 0)));
        assert_eq!(Version::parse("0.3"), None);
        assert_eq!(Version::parse("0.3.1.2"), None);
        assert_eq!(Version::parse("0.3.1-beta"), None);
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert_eq!(Version(0, 3, 1).to_string(), "0.3.1");
    }

    #[test]
    fn picks_the_installer_of_a_newer_release() {
        let found = pick(release("v0.4.0", serde_json::json!([setup_asset("0.4.0")])), Version(0, 3, 0)).unwrap().unwrap();
        assert_eq!(found.version, Version(0, 4, 0));
        assert_eq!(found.name, "flex-v0.4.0-windows-x64-setup.exe");
        assert_eq!(found.sha256, "ab".repeat(32));
    }

    #[test]
    fn ignores_releases_that_are_not_newer() {
        assert_eq!(pick(release("v0.3.0", serde_json::json!([setup_asset("0.3.0")])), Version(0, 3, 0)).unwrap(), None);
        assert_eq!(pick(release("v0.2.0", serde_json::json!([setup_asset("0.2.0")])), Version(0, 3, 0)).unwrap(), None);
        let mut pre = release("v0.4.0", serde_json::json!([setup_asset("0.4.0")]));
        pre.prerelease = true;
        assert_eq!(pick(pre, Version(0, 3, 0)).unwrap(), None);
    }

    #[test]
    fn needs_an_installer_with_a_checksum() {
        assert!(pick(release("v0.4.0", serde_json::json!([])), Version(0, 3, 0)).is_err());
        let mut asset = setup_asset("0.4.0");
        asset["digest"] = serde_json::Value::Null;
        assert!(pick(release("v0.4.0", serde_json::json!([asset])), Version(0, 3, 0)).is_err());
    }

    /// Asks GitHub's "latest release" page which version that is.
    #[test]
    #[ignore]
    fn finds_the_latest_version() {
        assert!(latest_version().unwrap() >= Version(0, 3, 1));
        assert_eq!(newer_installer(Version(999, 0, 0)).unwrap(), None);
    }

    /// Downloads the real latest installer (about 100 MB): `cargo test -p flex-core -- --ignored`.
    #[test]
    #[ignore]
    fn downloads_the_latest_installer() {
        let installer = newer_installer(Version(0, 0, 0)).unwrap().unwrap();
        let dir = std::env::temp_dir().join("flex-update-test");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join(&installer.name);
        let _ = std::fs::remove_file(&dest);
        let mut last = 0;
        download(&installer, &dest, |done| last = done).unwrap();
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), installer.size);
        assert_eq!(last, installer.size);
        download(&installer, &dest, |_| {}).unwrap(); // already there: kept
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
