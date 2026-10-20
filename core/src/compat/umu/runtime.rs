//! The Steam Linux Runtime (pressure-vessel + a container platform) Proton
//! runs inside. Port of umu's `umu_runtime.py`: which runtime a Proton
//! needs comes from its `toolmanifest.vdf` (`require_tool_appid`); the
//! runtime is fetched from `repo.steampowered.com` into `~/.local/share/umu/
//! <variant>` and kept at the latest public beta, same folder and files as
//! umu (`VERSIONS.txt`, `.installed.ok`, the `umu` entry-point link), so an
//! installation made by umu is used as it is and vice versa.

use std::path::{Path, PathBuf};

use super::fetch::{self, Download, HashKind};
use super::fsutil::{self, TempDir};
use super::{Cancel, Dirs, Preparing, Progress};
use crate::error::{Error, Result};

/// A Steam Linux Runtime generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Runtime {
    /// Valve's codename (`sniper`); names the platform folder and the archive.
    pub codename: &'static str,
    /// umu's folder name below `~/.local/share/umu`, also the repo path.
    pub variant: &'static str,
    /// Steam app id of the runtime (`require_tool_appid` in Proton's manifest).
    pub appid: &'static str,
}

/// x86-64 runtimes only: Proton runs x86 games (umu also knows an arm64 steamrt4).
pub const RUNTIMES: &[Runtime] = &[
    Runtime {
        codename: "soldier",
        variant: "steamrt2",
        appid: "1391110",
    },
    Runtime {
        codename: "sniper",
        variant: "steamrt3",
        appid: "1628350",
    },
    Runtime {
        codename: "steamrt4",
        variant: "steamrt4",
        appid: "4183110",
    },
];

const HOST: &str = "https://repo.steampowered.com";
const INSTALL_MARKER: &str = ".installed.ok";
/// Written (with the runtime's version) when `pv-verify` failed after an
/// install: the runtime is used anyway, like umu does, and not fetched again
/// on every launch for a marker it will never get. A newer version still
/// replaces it, and the stamp goes with the old files.
const UNVERIFIED_STAMP: &str = ".hyper-unverified";
const TEMP_PREFIX: &str = ".hyper-";

pub fn by_appid(appid: &str) -> Option<&'static Runtime> {
    RUNTIMES.iter().find(|r| r.appid == appid)
}

impl Runtime {
    /// `SteamLinuxRuntime_sniper.tar.xz`; numbered generations drop the
    /// `steamrt` prefix: `SteamLinuxRuntime_4.tar.xz`.
    pub fn archive(&self) -> String {
        let short = self
            .codename
            .strip_prefix("steamrt")
            .filter(|s| s.chars().all(|c| c.is_ascii_digit()));
        format!(
            "SteamLinuxRuntime_{}.tar.xz",
            short.unwrap_or(self.codename)
        )
    }

    /// Top folder inside the archive.
    pub fn unpacked_dir(&self) -> String {
        self.archive().trim_end_matches(".tar.xz").to_string()
    }

    pub fn images_url(&self) -> String {
        format!("{HOST}/{}/images", self.variant)
    }

    pub fn local_dir(&self, dirs: &Dirs) -> PathBuf {
        dirs.local.join(self.variant)
    }

    /// `<codename>_platform_<version>` folder inside the runtime (the newest
    /// when there are several).
    pub fn platform_dir(&self, local: &Path) -> Option<PathBuf> {
        let prefix = format!("{}_platform_", self.codename);
        std::fs::read_dir(local)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_dir()
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(&prefix))
            })
            .max()
    }
}

/// The runtime a compatibility tool asks for in its `toolmanifest.vdf`;
/// `None` when it runs on the host (no `require_tool_appid`).
pub fn required_by(tool: &Path) -> Result<Option<&'static Runtime>> {
    let manifest =
        super::vdf::load_manifest(&tool.join("toolmanifest.vdf")).map_err(Error::Other)?;
    match manifest
        .str("require_tool_appid")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => Ok(None),
        Some(appid) => by_appid(appid).map(Some).ok_or_else(|| {
            Error::Other(format!(
                "{} needs an unknown Steam runtime (app id {appid})",
                tool.display()
            ))
        }),
    }
}

/// `VERSIONS.txt`: tab-separated `name<TAB>version<TAB>…` rows, `#` comments.
pub fn versions(text: &str) -> std::collections::HashMap<String, String> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut cols = l.split('\t');
            Some((
                cols.next()?.trim().to_string(),
                cols.next()?.trim().to_string(),
            ))
        })
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

/// `3.0.20261007.263965` → numbers to compare; anything odd counts as 0.
pub fn version_key(v: &str) -> Vec<u64> {
    v.trim()
        .split('.')
        .map(|s| s.parse().unwrap_or(0))
        .collect()
}

fn installed_version(local: &Path, rt: &Runtime) -> Option<String> {
    let text = std::fs::read_to_string(local.join("VERSIONS.txt")).ok()?;
    versions(&text).remove(rt.codename)
}

fn has_marker(local: &Path) -> bool {
    local.join(INSTALL_MARKER).is_file()
}

fn write_marker(local: &Path) {
    let tmp = local.join(format!("{INSTALL_MARKER}.tmp"));
    if std::fs::write(&tmp, "ok\n").is_ok() {
        let _ = std::fs::rename(&tmp, local.join(INSTALL_MARKER));
    }
    let _ = std::fs::remove_file(local.join(UNVERIFIED_STAMP));
}

/// Is the installed version the one whose verification failed earlier?
fn unverified_matches(local: &Path, rt: &Runtime) -> bool {
    let Some(current) = installed_version(local, rt) else {
        return false;
    };
    std::fs::read_to_string(local.join(UNVERIFIED_STAMP)).is_ok_and(|stamp| stamp.trim() == current)
}

/// Remember that this version failed `pv-verify` (warned about once, here).
fn write_unverified(local: &Path, rt: &Runtime) {
    let Some(current) = installed_version(local, rt) else {
        // no VERSIONS.txt: counted as broken and restored on the next launch
        return;
    };
    tracing::warn!(
        runtime = rt.variant,
        version = %current,
        "runtime verification failed; it is used anyway (stamped {UNVERIFIED_STAMP}, not fetched again until a newer build is out)"
    );
    let tmp = local.join(format!("{UNVERIFIED_STAMP}.tmp"));
    if std::fs::write(&tmp, format!("{current}\n")).is_ok() {
        let _ = std::fs::rename(&tmp, local.join(UNVERIFIED_STAMP));
    }
}

/// `umu` → `_v2-entry-point`: the name umu starts the runtime by.
fn ensure_entry_link(local: &Path) {
    let link = local.join("umu");
    if std::fs::symlink_metadata(&link).is_err()
        && local.join("_v2-entry-point").is_file()
        && let Err(e) = fsutil::symlink("_v2-entry-point", &link)
    {
        tracing::warn!(error = %e, "could not create the runtime's umu link");
    }
}

/// Make sure the runtime is installed and current; returns its folder.
pub async fn ensure(
    http: &reqwest::Client,
    dirs: &Dirs,
    rt: &'static Runtime,
    progress: Progress<'_>,
    cancelled: Cancel<'_>,
) -> Result<PathBuf> {
    let local = rt.local_dir(dirs);
    tokio::fs::create_dir_all(&local).await?;

    let mut installed = has_marker(&local);
    if !installed && rt.platform_dir(&local).is_some() {
        if unverified_matches(&local, rt) {
            // failed pv-verify before and nothing changed since: used as it is
            tracing::debug!(runtime = rt.variant, "runtime stamped unverified; using it");
            installed = true;
        } else {
            // an install made before the marker existed (older umu): check it once
            progress(Preparing::message(format!(
                "Verifying the Steam Linux Runtime ({})",
                rt.codename
            )));
            if verify(&local, rt).await {
                write_marker(&local);
                installed = true;
            } else if local.join("VERSIONS.txt").is_file() {
                // complete by its files but failing the check: the version on
                // disk decides below whether a newer one replaces it; a missing
                // marker alone does not fetch the same build again
                write_unverified(&local, rt);
                installed = true;
            }
        }
    }

    let remote = match fetch::text(http, &format!("{}/latest-public-beta.txt", rt.images_url()))
        .await
    {
        Ok(v) => v.trim().to_string(),
        Err(e) if installed => {
            tracing::info!(error = %e, "runtime repository not reachable; using the installed runtime");
            ensure_entry_link(&local);
            return Ok(local);
        }
        Err(e) => {
            tracing::warn!(error = %e, "runtime repository not reachable");
            return Err(Error::Other(
                "The Steam Linux Runtime is not set up yet and could not be downloaded. An internet connection is needed for the first start of a Windows game.".into(),
            ));
        }
    };
    if remote.is_empty() {
        return Err(Error::Other(format!(
            "{} published an empty latest-public-beta.txt",
            rt.images_url()
        )));
    }

    if !installed {
        tracing::info!(runtime = rt.variant, version = %remote, "setting up the Steam Linux Runtime");
        install(http, dirs, rt, &local, &remote, false, progress, cancelled).await?;
        return Ok(local);
    }

    let broken = rt.platform_dir(&local).is_none()
        || !local.join("pressure-vessel").is_dir()
        || !local.join("VERSIONS.txt").is_file();
    if broken {
        tracing::warn!(runtime = rt.variant, "runtime files missing; restoring");
        install(http, dirs, rt, &local, &remote, true, progress, cancelled).await?;
        return Ok(local);
    }

    let current = installed_version(&local, rt);
    let newer = current
        .as_deref()
        .is_none_or(|cur| version_key(&remote) > version_key(cur));
    if newer {
        tracing::info!(runtime = rt.variant, from = ?current, to = %remote, "updating the Steam Linux Runtime");
        install(http, dirs, rt, &local, &remote, false, progress, cancelled).await?;
    } else {
        tracing::debug!(runtime = rt.variant, version = ?current, "runtime is up to date");
    }
    ensure_entry_link(&local);
    Ok(local)
}

/// Download `version` and swap it in. `force` replaces whatever is there
/// (the restore of a broken runtime); otherwise a complete install of that
/// version made by another launcher while we waited for the lock is kept.
#[allow(clippy::too_many_arguments)]
async fn install(
    http: &reqwest::Client,
    dirs: &Dirs,
    rt: &Runtime,
    local: &Path,
    version: &str,
    force: bool,
    progress: Progress<'_>,
    cancelled: Cancel<'_>,
) -> Result<()> {
    let label = format!(
        "Downloading the Steam Linux Runtime ({} {version})",
        rt.codename
    );
    progress(Preparing::message(label.clone()));
    // umu's runtime lock
    let _lock = super::lock(dirs.local.join("umu.lock"), progress, cancelled).await?;
    // another launcher may have finished it while we waited
    if !force
        && has_marker(local)
        && installed_version(local, rt).as_deref() == Some(version)
        && rt.platform_dir(local).is_some()
        && local.join("pressure-vessel").is_dir()
    {
        return Ok(());
    }
    // unpack folders of an earlier run that was interrupted, under the lock
    let unpack_root = dirs.local.clone();
    let _ = tokio::task::spawn_blocking(move || {
        fsutil::remove_stale_temp_dirs(&unpack_root, TEMP_PREFIX)
    })
    .await;

    let base = format!("{}/{version}", rt.images_url());
    let archive = rt.archive();
    let sums = fetch::text(http, &format!("{base}/SHA256SUMS")).await?;
    let digest = fetch::digest_for(&sums, &archive, HashKind::Sha256)
        .ok_or_else(|| Error::Other(format!("{base}/SHA256SUMS lists no digest for {archive}")))?;
    let build_id = fetch::text(http, &format!("{base}/BUILD_ID.txt"))
        .await?
        .trim()
        .to_string();
    let dl = Download {
        url: format!("{base}/{archive}"),
        // the build id in the name: a resume never continues a different build's file
        parts: dirs.cache.join(format!("{archive}.{build_id}.parts")),
        expected: digest,
        kind: HashKind::Sha256,
        label,
    };
    let file = fetch::download(http, &dl, progress, cancelled).await?;

    progress(Preparing::message(format!(
        "Unpacking the Steam Linux Runtime ({})",
        rt.codename
    )));
    let local_owned = local.to_path_buf();
    let unpacked_name = rt.unpacked_dir();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let parent = local_owned
            .parent()
            .ok_or_else(|| Error::Other("runtime folder has no parent".into()))?;
        let tmp = TempDir::new_in(parent, TEMP_PREFIX)?;
        fsutil::extract_tar(&file, &tmp.path)?;
        let unpacked = tmp.path.join(&unpacked_name);
        if !unpacked.join("_v2-entry-point").is_file() {
            // not what the digest promised: a resume would only unpack it again
            let _ = std::fs::remove_file(&file);
            return Err(Error::Other(format!(
                "{} does not contain {unpacked_name}/_v2-entry-point",
                file.display()
            )));
        }
        std::fs::create_dir_all(&local_owned)?;
        // swap the new runtime in; the old one lands in `tmp` and goes with it
        fsutil::exchange(&unpacked, &local_owned)?;
        drop(tmp);
        let _ = std::fs::remove_file(&file);
        Ok(())
    })
    .await
    .map_err(|e| Error::Other(format!("unpacking failed: {e}")))??;

    progress(Preparing::message(format!(
        "Verifying the Steam Linux Runtime ({})",
        rt.codename
    )));
    if verify(local, rt).await {
        write_marker(local);
    } else {
        write_unverified(local, rt);
    }
    ensure_entry_link(local);
    tracing::info!(
        runtime = rt.variant,
        version,
        "Steam Linux Runtime installed"
    );
    Ok(())
}

/// `pv-verify` checks the platform against its mtree manifest.
async fn verify(local: &Path, rt: &Runtime) -> bool {
    let pv_verify = local.join("pressure-vessel").join("bin").join("pv-verify");
    let Some(platform) = rt.platform_dir(local) else {
        tracing::warn!(dir = %local.display(), "no {}_platform_* folder", rt.codename);
        return false;
    };
    if !pv_verify.is_file() {
        tracing::warn!(file = %pv_verify.display(), "pv-verify missing");
        return false;
    }
    let mut verify_cmd = tokio::process::Command::new(&pv_verify);
    crate::platform::child_env::apply(&mut verify_cmd);
    let status = verify_cmd
        .args(["--quiet", "--minimized-runtime"])
        .arg(platform.join("files"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
    match status {
        Ok(s) if s.success() => true,
        Ok(s) => {
            tracing::warn!(code = ?s.code(), "pv-verify failed");
            false
        }
        Err(e) => {
            tracing::warn!(error = %e, "pv-verify could not run");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names_follow_valves_repo() {
        let sniper = by_appid("1628350").expect("sniper");
        assert_eq!(sniper.archive(), "SteamLinuxRuntime_sniper.tar.xz");
        assert_eq!(sniper.unpacked_dir(), "SteamLinuxRuntime_sniper");
        assert_eq!(
            sniper.images_url(),
            "https://repo.steampowered.com/steamrt3/images"
        );
        let rt4 = by_appid("4183110").expect("steamrt4");
        assert_eq!(rt4.archive(), "SteamLinuxRuntime_4.tar.xz");
        assert_eq!(
            rt4.images_url(),
            "https://repo.steampowered.com/steamrt4/images"
        );
        assert!(by_appid("1").is_none());
    }

    #[test]
    fn versions_txt_and_version_order() {
        let text = "#Name\tVersion\t\tRuntime\tRuntime_Version\tComment\n\
                    depot\t0.20240530.90143\t\t\t# Overall version number\n\
                    sniper\t3.0.20240530.90143\tsniper\t3.0.20240530.90143\t\n\
                    pressure-vessel\t0.20240530.0\tscout\t0.20240530.0\t# from pressure-vessel-bin.tar.gz\n";
        let v = versions(text);
        assert_eq!(
            v.get("sniper").map(String::as_str),
            Some("3.0.20240530.90143")
        );
        assert!(!v.contains_key("#Name"));
        assert!(version_key("3.0.20261007.263965") > version_key("3.0.20240530.90143"));
        assert!(version_key("3.0.20240530.90143") >= version_key("3.0.20240530.90143 "));
    }

    #[test]
    fn required_runtime_comes_from_the_tool_manifest() {
        let base = std::env::temp_dir().join(format!("hyper-runtime-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let proton = base.join("UMU-Proton-10.0-4");
        std::fs::create_dir_all(&proton).expect("dir");
        std::fs::write(
            proton.join("toolmanifest.vdf"),
            "\"manifest\"\n{\n  \"version\" \"2\"\n  \"commandline\" \"/proton %verb%\"\n  \"require_tool_appid\" \"1628350\"\n  \"compatmanager_layer_name\" \"proton\"\n}\n",
        )
        .expect("write");
        assert_eq!(
            required_by(&proton).expect("ok").map(|r| r.codename),
            Some("sniper")
        );

        let host = base.join("host-tool");
        std::fs::create_dir_all(&host).expect("dir");
        std::fs::write(
            host.join("toolmanifest.vdf"),
            "\"manifest\" { \"commandline\" \"/run\" }",
        )
        .expect("write");
        assert_eq!(required_by(&host).expect("ok"), None);

        let odd = base.join("odd");
        std::fs::create_dir_all(&odd).expect("dir");
        std::fs::write(
            odd.join("toolmanifest.vdf"),
            "\"manifest\" { \"require_tool_appid\" \"42\" }",
        )
        .expect("write");
        assert!(required_by(&odd).is_err());
        assert!(required_by(&base.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unverified_stamp_follows_the_installed_version() {
        let base = std::env::temp_dir().join(format!("hyper-unverified-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("dir");
        let rt = by_appid("1628350").expect("sniper");
        assert!(!unverified_matches(&base, rt));
        // no VERSIONS.txt: nothing to stamp (counted as broken instead)
        write_unverified(&base, rt);
        assert!(!base.join(UNVERIFIED_STAMP).exists());

        let versions = |v: &str| {
            std::fs::write(
                base.join("VERSIONS.txt"),
                format!("sniper\t{v}\tsniper\t{v}\t\n"),
            )
            .expect("versions")
        };
        versions("3.0.1");
        write_unverified(&base, rt);
        assert!(unverified_matches(&base, rt));
        // another version on disk (an update by umu): the stamp no longer counts
        versions("3.0.2");
        assert!(!unverified_matches(&base, rt));
        // a passing check replaces the stamp with the marker
        write_marker(&base);
        assert!(has_marker(&base) && !base.join(UNVERIFIED_STAMP).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn newest_platform_folder_is_picked() {
        let base = std::env::temp_dir().join(format!("hyper-platform-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for d in [
            "sniper_platform_0.20240530.90143",
            "sniper_platform_3.0.20261007.263965",
            "pressure-vessel",
        ] {
            std::fs::create_dir_all(base.join(d)).expect("dir");
        }
        std::fs::write(base.join("sniper_platform_9"), "a file, not a folder").expect("file");
        let rt = by_appid("1628350").expect("sniper");
        assert_eq!(
            rt.platform_dir(&base),
            Some(base.join("sniper_platform_3.0.20261007.263965"))
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
