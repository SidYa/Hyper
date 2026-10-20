//! Epic titles another launcher owns: EA (Origin / the EA app) and Ubisoft
//! Connect games bought on the Epic store. Epic ships no files for them;
//! the other launcher installs and starts the game, Epic only hands over
//! the login. Port of legendary's `launch --origin` / `--ubisoft`
//! (`get_origin_uri` / `get_ubisoft_uri`) and Heroic's `installEA` /
//! `installUbisoft` + `thirdParty.ts`.
//!
//! "Installing" such a game means: make sure the launcher itself is
//! installed (its installer is downloaded and run once), then remember the
//! game in `third-party-installed.json` (`[[app_name, platform], …]`, the
//! file Heroic keeps too). "Launching" opens the launcher's URI with Epic's
//! exchange code; the launcher downloads the game on first start.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{Error, Result};

const INSTALLED_FILE: &str = "third-party-installed.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launcher {
    Ea,
    Ubisoft,
}

impl Launcher {
    /// The metadata's `ThirdPartyManagedApp` / `ThirdPartyManagedProvider`;
    /// `None` for launchers nobody supports (legendary knows these two).
    pub fn from_app(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "origin" | "the ea app" => Some(Self::Ea),
            "ubisoftconnect" => Some(Self::Ubisoft),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Ea => "the EA app",
            Self::Ubisoft => "Ubisoft Connect",
        }
    }

    pub fn installer_url(self) -> &'static str {
        match self {
            Self::Ea => {
                "https://origin-a.akamaihd.net/EA-Desktop-Client-Download/installer-releases/EAappInstaller.exe"
            }
            Self::Ubisoft => {
                "https://static3.cdn.ubi.com/orbit/launcher_installer/UbisoftConnectInstaller.exe"
            }
        }
    }

    pub fn installer_file(self) -> &'static str {
        match self {
            Self::Ea => "EAappInstaller.exe",
            Self::Ubisoft => "UbisoftConnectInstaller.exe",
        }
    }

    /// Heroic's arguments: the EA installer without starting the client
    /// afterwards and without complaining when it is there already; Ubisoft
    /// Connect silent.
    pub fn installer_args(self) -> &'static [&'static str] {
        match self {
            Self::Ea => &["EAX_LAUNCH_CLIENT=0", "IGNORE_INSTALLED=1"],
            Self::Ubisoft => &["/S"],
        }
    }
}

fn installed_file(config_dir: &Path) -> PathBuf {
    config_dir.join(INSTALLED_FILE)
}

/// `(app_name, platform)` of every third-party game marked installed.
pub fn read_installed(config_dir: &Path) -> Vec<(String, String)> {
    let Ok(raw) = std::fs::read_to_string(installed_file(config_dir)) else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<(String, String)>>(&raw) {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(error = %e, "corrupted {INSTALLED_FILE}");
            Vec::new()
        }
    }
}

fn write_installed(config_dir: &Path, list: &[(String, String)]) -> Result<()> {
    std::fs::create_dir_all(config_dir)?;
    std::fs::write(installed_file(config_dir), serde_json::to_vec(list)?)?;
    Ok(())
}

pub fn add_installed(config_dir: &Path, app_name: &str, platform: &str) -> Result<()> {
    let mut list = read_installed(config_dir);
    list.retain(|(a, _)| a != app_name);
    list.push((app_name.to_string(), platform.to_string()));
    write_installed(config_dir, &list)
}

pub fn remove_installed(config_dir: &Path, app_name: &str) -> Result<()> {
    let mut list = read_installed(config_dir);
    let before = list.len();
    list.retain(|(a, _)| a != app_name);
    if list.len() == before {
        return Ok(());
    }
    write_installed(config_dir, &list)
}

/// The launcher's installer in `dir`, downloaded when it is not there yet.
pub async fn fetch_installer(
    http: &reqwest::Client,
    launcher: Launcher,
    dir: &Path,
) -> Result<PathBuf> {
    use tokio::io::AsyncWriteExt;
    let path = dir.join(launcher.installer_file());
    if is_fresh_installer(&path) {
        return Ok(path);
    }
    tokio::fs::create_dir_all(dir).await?;
    tracing::info!(
        url = launcher.installer_url(),
        "downloading the {} installer",
        launcher.display_name()
    );
    let fail = |e: &dyn std::fmt::Display| {
        Error::Other(format!("{} installer: {e}", launcher.display_name()))
    };
    // tens of MB: the shared client's 20 s deadline is far too short for that
    let mut resp = http
        .get(launcher.installer_url())
        .timeout(INSTALLER_TIMEOUT)
        .send()
        .await
        .map_err(|e| fail(&e.without_url()))?;
    if !resp.status().is_success() {
        return Err(fail(&format!("server answered {}", resp.status())));
    }
    let part = path.with_extension("exe.part");
    let streamed = async {
        let mut file = tokio::fs::File::create(&part).await?;
        let (mut len, mut head) = (0u64, Vec::<u8>::with_capacity(2));
        while let Some(chunk) = resp.chunk().await.map_err(|e| fail(&e.without_url()))? {
            if head.len() < 2 {
                head.extend(chunk.iter().take(2 - head.len()));
            }
            len += chunk.len() as u64;
            if len > MAX_INSTALLER {
                return Err(fail(&"larger than any launcher installer"));
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        if len < 1024 || head != b"MZ" {
            return Err(fail(&"not a Windows program"));
        }
        Ok(())
    }
    .await;
    if let Err(e) = streamed {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    tokio::fs::rename(&part, &path).await?;
    Ok(path)
}

/// The whole installer download may take this long (slow links).
const INSTALLER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(900);
/// An installer bigger than this is not one.
const MAX_INSTALLER: u64 = 1 << 30;
/// A downloaded installer is reused for a week, then fetched again (the
/// launchers update theirs).
const INSTALLER_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

/// A cached installer that is recent and still a Windows program.
fn is_fresh_installer(path: &Path) -> bool {
    use std::io::Read;
    let recent = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < INSTALLER_MAX_AGE);
    let mut head = [0u8; 2];
    recent
        && std::fs::File::open(path)
            .and_then(|mut f| f.read_exact(&mut head))
            .is_ok()
        && &head == b"MZ"
}

/// The URI the launcher is opened with (`get_origin_uri` / `get_ubisoft_uri`):
/// Epic's exchange code (`"0"` offline), the account, the locale and the
/// game's `AdditionalCommandline` parameters. `metadata` is the game's
/// `metadata/<app>.json`.
pub fn launch_uri(
    launcher: Launcher,
    app_name: &str,
    metadata: &Value,
    token: &str,
    account_id: &str,
    display_name: &str,
    language: &str,
) -> Result<String> {
    let attr = |k: &str| {
        metadata
            .pointer(&format!("/metadata/customAttributes/{k}/value"))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
    };
    let base = match launcher {
        Launcher::Ea => format!("link2ea://launchgame/{app_name}"),
        Launcher::Ubisoft => format!("uplay://launch/{}", attr("GameID").unwrap_or(app_name)),
    };
    let mut url =
        reqwest::Url::parse(&base).map_err(|e| Error::Other(format!("launch URI: {e}")))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("AUTH_PASSWORD", token)
            .append_pair("AUTH_TYPE", "exchangecode")
            .append_pair("epicusername", display_name)
            .append_pair("epicuserid", account_id)
            .append_pair("epiclocale", language);
        if let Some(extra) = attr("AdditionalCommandline")
            && let Ok(parsed) =
                reqwest::Url::parse(&format!("x://x?{}", extra.trim_start_matches('?')))
        {
            for (k, v) in parsed.query_pairs() {
                q.append_pair(&k, &v);
            }
        }
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_legendarys_launchers() {
        assert_eq!(Launcher::from_app("Origin"), Some(Launcher::Ea));
        assert_eq!(Launcher::from_app("the EA app"), Some(Launcher::Ea));
        assert_eq!(
            Launcher::from_app("UbisoftConnect"),
            Some(Launcher::Ubisoft)
        );
        assert_eq!(Launcher::from_app("Rockstar"), None);
    }

    #[test]
    fn builds_the_launch_uris() {
        let meta = serde_json::json!({ "metadata": { "customAttributes": {
            "AdditionalCommandline": { "value": "foo=bar&baz=1" },
            "GameID": { "value": "4242" }
        } } });
        let ea = launch_uri(
            Launcher::Ea,
            "Bf6App",
            &meta,
            "c0de",
            "acc",
            "Player One",
            "uk",
        )
        .expect("uri");
        assert!(ea.starts_with("link2ea://launchgame/Bf6App?AUTH_PASSWORD=c0de&AUTH_TYPE=exchangecode&epicusername=Player+One&epicuserid=acc&epiclocale=uk"), "{ea}");
        assert!(ea.ends_with("&foo=bar&baz=1"), "{ea}");
        let ubi =
            launch_uri(Launcher::Ubisoft, "UbiApp", &meta, "0", "acc", "P", "en").expect("uri");
        assert!(
            ubi.starts_with("uplay://launch/4242?AUTH_PASSWORD=0&"),
            "{ubi}"
        );
        let plain = launch_uri(
            Launcher::Ubisoft,
            "UbiApp",
            &serde_json::json!({}),
            "0",
            "acc",
            "P",
            "en",
        )
        .expect("uri");
        assert!(plain.starts_with("uplay://launch/UbiApp?"), "{plain}");
    }

    #[test]
    fn cached_installers_must_be_recent_windows_programs() {
        let dir =
            std::env::temp_dir().join(format!("hyper-thirdparty-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("x.exe");
        assert!(!is_fresh_installer(&exe), "missing");
        std::fs::write(&exe, b"<html>").unwrap();
        assert!(!is_fresh_installer(&exe), "not MZ");
        std::fs::write(&exe, b"MZ....").unwrap();
        assert!(is_fresh_installer(&exe));
        let old =
            std::time::SystemTime::now() - INSTALLER_MAX_AGE - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&exe)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(!is_fresh_installer(&exe), "older than a week");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installed_list_round_trips() {
        let dir = std::env::temp_dir().join(format!("hyper-thirdparty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(read_installed(&dir).is_empty());
        add_installed(&dir, "Bf6", "Windows").expect("add");
        add_installed(&dir, "Ubi", "Windows").expect("add");
        add_installed(&dir, "Bf6", "Windows").expect("add again");
        assert_eq!(
            read_installed(&dir),
            [
                ("Ubi".to_string(), "Windows".to_string()),
                ("Bf6".to_string(), "Windows".to_string())
            ]
        );
        remove_installed(&dir, "Ubi").expect("remove");
        remove_installed(&dir, "nope").expect("remove missing");
        assert_eq!(
            read_installed(&dir),
            [("Bf6".to_string(), "Windows".to_string())]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
