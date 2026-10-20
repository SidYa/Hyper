//! UMU-Proton: the one Proton build Hyper uses (umu's default when
//! `PROTONPATH` is unset). Port of umu's `umu_proton.py` for that case:
//! ask GitHub for the latest release, install it into Steam's
//! `compatibilitytools.d` if it is not there yet, otherwise use the newest
//! UMU-Proton already installed (by umu, Heroic or by hand). Offline, or
//! when GitHub is rate-limited, the installed one is used.
//!
//! Not ported: GE-Proton / `*-Latest` tokens with their signed delta
//! patches, user-chosen `PROTONPATH`s, the passthrough tools for native
//! Linux programs (owner decision: always UMU-Proton, no version picker).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::fetch::{self, Download, HashKind};
use super::fsutil::{self, TempDir};
use super::{Cancel, Dirs, Preparing, Progress};
use crate::error::{Error, Result};
use crate::paths;

const LATEST_RELEASE: &str =
    "https://api.github.com/repos/Open-Wine-Components/umu-proton/releases/latest";
const NAME: &str = "UMU-Proton";
/// GitHub answers 60 anonymous requests an hour per address; umu asks on
/// every launch, we remember the answer for a while.
const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);
const TEMP_PREFIX: &str = ".hyper-";

/// The two assets of a release that matter: the archive and its digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Folder name, `UMU-Proton-10.0-4`.
    pub name: String,
    /// `UMU-Proton-10.0-4.tar.gz`
    pub tarball: String,
    pub tar_url: String,
    /// `UMU-Proton-10.0-4.sha512sum`
    pub sum_url: String,
}

/// Pick the x86-64 archive and its checksum file out of a GitHub release.
pub fn parse_release(json: &serde_json::Value) -> Option<Release> {
    let assets = json.get("assets")?.as_array()?;
    let mut tar = None;
    let mut sum = None;
    for asset in assets {
        let (Some(name), Some(url)) = (
            asset.get("name").and_then(|v| v.as_str()),
            asset.get("browser_download_url").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        // builds for other architectures ship next to the x86-64 one; the
        // names become folder and file names here, so they must be plain
        if name.contains("-aarch64")
            || !url.starts_with("https://")
            || !is_plain_name(name)
            || !name.starts_with(NAME)
        {
            continue;
        }
        if name.ends_with("sum") {
            sum = Some((name.to_string(), url.to_string()));
        } else if name.ends_with(".tar.gz") || name.ends_with(".tar.xz") {
            tar = Some((name.to_string(), url.to_string()));
        }
    }
    let (tarball, tar_url) = tar?;
    let (_, sum_url) = sum?;
    Some(Release {
        name: strip_archive_suffix(&tarball),
        tarball,
        tar_url,
        sum_url,
    })
}

/// One path component and nothing else: no separators, no NUL, not `.` /
/// `..`, no leading dot (a hidden name would clash with the temp folders).
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0'])
        && name.chars().all(|c| !c.is_control())
}

/// `UMU-Proton-10.0-4.tar.gz` → `UMU-Proton-10.0-4` (the last two
/// dot-separated parts go, like umu does it).
pub fn strip_archive_suffix(tarball: &str) -> String {
    let parts: Vec<&str> = tarball.split('.').collect();
    if parts.len() > 2 {
        parts[..parts.len() - 2].join(".")
    } else {
        tarball.to_string()
    }
}

/// Make sure a UMU-Proton is installed and return its folder (the one
/// holding `proton` and `toolmanifest.vdf`).
pub async fn ensure(
    http: &reqwest::Client,
    dirs: &Dirs,
    progress: Progress<'_>,
    cancelled: Cancel<'_>,
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(&dirs.steam_compat).await?;
    tokio::fs::create_dir_all(&dirs.cache).await?;

    if let Some(recent) = recent_check() {
        tracing::debug!(proton = %recent.display(), "UMU-Proton checked recently");
        return Ok(recent);
    }

    match latest(http).await {
        Ok(Some(release)) => {
            let target = dirs.steam_compat.join(&release.name);
            if !target.is_dir() {
                match install(http, dirs, &release, &target, progress, cancelled).await {
                    Ok(()) => {}
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(e) => {
                        tracing::warn!(error = %e, "could not install the latest UMU-Proton; using what is installed")
                    }
                }
            }
            if target.is_dir() {
                tracing::info!(proton = %release.name, "UMU-Proton is up to date");
                remember_check(&target);
                return Ok(target);
            }
        }
        Ok(None) => tracing::warn!("the latest UMU-Proton release has no usable assets"),
        Err(e) => {
            tracing::info!(error = %e, "GitHub not reachable; using the installed UMU-Proton")
        }
    }

    if let Some(installed) = installed(dirs) {
        tracing::info!(proton = %installed.display(), "using installed UMU-Proton");
        return Ok(installed);
    }
    Err(Error::Other(
        "UMU-Proton is not installed and could not be downloaded. An internet connection is needed for the first start of a Windows game.".into(),
    ))
}

async fn latest(http: &reqwest::Client) -> Result<Option<Release>> {
    let resp = http
        .get(LATEST_RELEASE)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| Error::Other(format!("network: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::Other(format!("GitHub answered {}", resp.status())));
    }
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| Error::Other(format!("network: {e}")))?;
    Ok(parse_release(&json))
}

async fn install(
    http: &reqwest::Client,
    dirs: &Dirs,
    release: &Release,
    target: &Path,
    progress: Progress<'_>,
    cancelled: Cancel<'_>,
) -> Result<()> {
    progress(Preparing::message(format!("Downloading {}", release.name)));
    tokio::fs::create_dir_all(&dirs.local).await?;
    // umu's lock: another launcher may be installing the same build right now
    let _lock = super::lock(
        dirs.local.join("compatibilitytools.d.lock"),
        progress,
        cancelled,
    )
    .await?;
    if target.is_dir() {
        return Ok(());
    }
    let steam_compat = dirs.steam_compat.clone();
    let _ = tokio::task::spawn_blocking(move || {
        fsutil::remove_stale_temp_dirs(&steam_compat, TEMP_PREFIX)
    })
    .await;

    let sums = fetch::text(http, &release.sum_url).await?;
    let digest = fetch::digest_for(&sums, &release.tarball, HashKind::Sha512).ok_or_else(|| {
        Error::Other(format!(
            "{} lists no digest for {}",
            release.sum_url, release.tarball
        ))
    })?;
    let dl = Download {
        url: release.tar_url.clone(),
        parts: dirs.cache.join(format!("{}.parts", release.tarball)),
        expected: digest,
        kind: HashKind::Sha512,
        label: format!("Downloading {}", release.name),
    };
    let archive = fetch::download(http, &dl, progress, cancelled).await?;

    progress(Preparing::message(format!("Unpacking {}", release.name)));
    let name = release.name.clone();
    let steam_compat = dirs.steam_compat.clone();
    let target = target.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        // unpack next to the destination so the final step is one rename
        let tmp = TempDir::new_in(&steam_compat, TEMP_PREFIX)?;
        fsutil::extract_tar(&archive, &tmp.path)?;
        let unpacked = tmp.path.join(&name);
        if !unpacked.join("toolmanifest.vdf").is_file() {
            // not what the digest promised: a resume would only unpack it again
            let _ = std::fs::remove_file(&archive);
            return Err(Error::Other(format!(
                "{} does not contain a {name} folder with toolmanifest.vdf",
                archive.display()
            )));
        }
        std::fs::rename(&unpacked, &target)?;
        drop(tmp);
        let _ = std::fs::remove_file(&archive);
        Ok(())
    })
    .await
    .map_err(|e| Error::Other(format!("unpacking failed: {e}")))??;
    tracing::info!(proton = %release.name, dir = %dirs.steam_compat.display(), "UMU-Proton installed");
    Ok(())
}

/// The newest UMU-Proton in umu's own folder or Steam's `compatibilitytools.d`.
pub fn installed(dirs: &Dirs) -> Option<PathBuf> {
    [&dirs.umu_compat, &dirs.steam_compat]
        .into_iter()
        .find_map(|dir| newest_in(dir))
}

fn newest_in(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(NAME))
                && p.join("toolmanifest.vdf").is_file()
        })
        .max_by_key(|p| fsutil::natural_key(&p.file_name().unwrap_or_default().to_string_lossy()))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LastCheck {
    checked_at: u64,
    proton: PathBuf,
}

fn check_file() -> PathBuf {
    paths::cache_dir().join("umu-proton-check.json")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The Proton found less than [`CHECK_EVERY`] ago, if it is still there.
fn recent_check() -> Option<PathBuf> {
    let last: LastCheck = serde_json::from_slice(&std::fs::read(check_file()).ok()?).ok()?;
    let fresh = now().saturating_sub(last.checked_at) < CHECK_EVERY.as_secs();
    (fresh && last.proton.join("toolmanifest.vdf").is_file()).then_some(last.proton)
}

fn remember_check(proton: &Path) {
    let last = LastCheck {
        checked_at: now(),
        proton: proton.to_path_buf(),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&last) {
        let _ = std::fs::create_dir_all(paths::cache_dir());
        let _ = std::fs::write(check_file(), bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_archive_and_its_digest_from_a_release() {
        let json = serde_json::json!({
            "tag_name": "UMU-Proton-10.0-4",
            "assets": [
                { "name": "UMU-Proton-10.0-4.sha512sum", "browser_download_url": "https://github.com/x/UMU-Proton-10.0-4.sha512sum" },
                { "name": "UMU-Proton-10.0-4-aarch64.tar.gz", "browser_download_url": "https://github.com/x/UMU-Proton-10.0-4-aarch64.tar.gz" },
                { "name": "UMU-Proton-10.0-4.tar.gz", "browser_download_url": "https://github.com/x/UMU-Proton-10.0-4.tar.gz" }
            ]
        });
        let r = parse_release(&json).expect("release");
        assert_eq!(r.name, "UMU-Proton-10.0-4");
        assert_eq!(r.tarball, "UMU-Proton-10.0-4.tar.gz");
        assert!(r.tar_url.ends_with("/UMU-Proton-10.0-4.tar.gz"));
        assert!(r.sum_url.ends_with(".sha512sum"));
    }

    #[test]
    fn incomplete_or_insecure_releases_are_refused() {
        let no_sum = serde_json::json!({ "assets": [
            { "name": "UMU-Proton-10.0-4.tar.gz", "browser_download_url": "https://github.com/x/UMU-Proton-10.0-4.tar.gz" } ] });
        assert_eq!(parse_release(&no_sum), None);
        let http = serde_json::json!({ "assets": [
            { "name": "UMU-Proton-10.0-4.sha512sum", "browser_download_url": "http://github.com/x/UMU-Proton-10.0-4.sha512sum" },
            { "name": "UMU-Proton-10.0-4.tar.gz", "browser_download_url": "http://github.com/x/UMU-Proton-10.0-4.tar.gz" } ] });
        assert_eq!(parse_release(&http), None);
        assert_eq!(parse_release(&serde_json::json!({})), None);
    }

    #[test]
    fn asset_names_must_be_plain_file_names() {
        let release = |tar: &str, sum: &str| {
            parse_release(&serde_json::json!({ "assets": [
                { "name": sum, "browser_download_url": "https://github.com/x/s" },
                { "name": tar, "browser_download_url": "https://github.com/x/t" } ] }))
        };
        assert!(release("UMU-Proton-10.0-4.tar.gz", "UMU-Proton-10.0-4.sha512sum").is_some());
        // the archive name becomes a folder name next to the other Protons
        assert_eq!(
            release("../UMU-Proton-10.0-4.tar.gz", "UMU-Proton-10.0-4.sha512sum"),
            None
        );
        assert_eq!(
            release("UMU-Proton/x.tar.gz", "UMU-Proton-10.0-4.sha512sum"),
            None
        );
        assert_eq!(
            release("UMU-Proton\\x.tar.gz", "UMU-Proton-10.0-4.sha512sum"),
            None
        );
        assert_eq!(
            release("UMU-Proton-10\0.tar.gz", "UMU-Proton-10.0-4.sha512sum"),
            None
        );
        // the digest file must belong to UMU-Proton too
        assert_eq!(release("UMU-Proton-10.0-4.tar.gz", "other.sha512sum"), None);
        assert_eq!(
            release("UMU-Proton-10.0-4.tar.gz", "../UMU-Proton.sha512sum"),
            None
        );
        assert!(!is_plain_name(""));
        assert!(!is_plain_name("."));
        assert!(!is_plain_name(".."));
        assert!(!is_plain_name(".hyper-x"));
        assert!(is_plain_name("UMU-Proton-10.0-4.tar.xz"));
    }

    #[test]
    fn archive_suffix_handling() {
        assert_eq!(
            strip_archive_suffix("UMU-Proton-10.0-4.tar.gz"),
            "UMU-Proton-10.0-4"
        );
        assert_eq!(strip_archive_suffix("GE-Proton9-7.tar.xz"), "GE-Proton9-7");
        assert_eq!(strip_archive_suffix("plain"), "plain");
    }

    #[test]
    fn newest_installed_proton_wins() {
        let base = std::env::temp_dir().join(format!("hyper-proton-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for name in [
            "UMU-Proton-9.0-3",
            "UMU-Proton-10.0-4",
            "UMU-Proton-10.0-11",
            "GE-Proton10-1",
            "UMU-Proton-99-broken",
        ] {
            std::fs::create_dir_all(base.join("steam").join(name)).expect("dir");
            if name != "UMU-Proton-99-broken" {
                std::fs::write(
                    base.join("steam").join(name).join("toolmanifest.vdf"),
                    "\"manifest\" {}",
                )
                .expect("manifest");
            }
        }
        let dirs = Dirs {
            local: base.join("umu"),
            cache: base.join("cache"),
            steam_compat: base.join("steam"),
            umu_compat: base.join("umu").join("compatibilitytools"),
        };
        assert_eq!(
            installed(&dirs),
            Some(base.join("steam").join("UMU-Proton-10.0-11"))
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
