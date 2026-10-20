//! GOG cloud saves (port of Heroic's `getSaveSyncLocation` and the GOG part
//! of `save_sync.ts`):
//!
//! - the game's Galaxy `clientId` comes from gogdl's v2 manifest or from
//!   `goggame-<id>.info`,
//! - GOG's remote config for that client says whether cloud saves are on and
//!   lists the save folders (`<?SAVED_GAMES?>/Studio/Game`…); none listed
//!   means Galaxy's default folder,
//! - `gogdl save-sync` prints the new sync timestamp on stdout; it is handed
//!   back with `--ts` next time (`saveTimestamps.json`).
//!
//! The variables resolve against [`Roots`]: the user's folders on Windows,
//! the game's Wine prefix on Linux.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::paths;
use crate::saves::Roots;
use crate::types::SaveLocation;

/// Cloud-save setup of one installed game, cached in `cloudSaves.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CloudConfig {
    pub client_id: String,
    pub enabled: bool,
    /// As GOG lists them, variables not filled in.
    pub locations: Vec<SaveLocation>,
}

fn cache_file() -> PathBuf {
    paths::gogdl_config_dir().join("cloudSaves.json")
}

pub fn load_cache() -> HashMap<String, CloudConfig> {
    std::fs::read_to_string(cache_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_cache(all: &HashMap<String, CloudConfig>) {
    let written = serde_json::to_vec_pretty(all)
        .map_err(std::io::Error::other)
        .and_then(|raw| std::fs::write(cache_file(), raw));
    if let Err(e) = written {
        tracing::warn!(error = %e, "could not save the GOG cloud-save cache");
    }
}

/// The Galaxy client id of an installed game.
pub fn client_id(app_name: &str, install_path: &Path) -> Option<String> {
    let from_manifest = std::fs::read_to_string(paths::gogdl_manifests_dir().join(app_name))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(|v| v.get("version").and_then(Value::as_u64) == Some(2))
        .and_then(|v| v.get("clientId").and_then(Value::as_str).map(str::to_owned));
    from_manifest.filter(|s| !s.is_empty()).or_else(|| {
        let raw =
            std::fs::read_to_string(install_path.join(format!("goggame-{app_name}.info"))).ok()?;
        let v: Value = serde_json::from_str(&raw).ok()?;
        v.get("clientId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    })
}

/// `remote-config.gog.com` for `client_id` (public, no token).
pub async fn fetch(http: &reqwest::Client, client_id: &str) -> Result<CloudConfig> {
    let safe: String = client_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let url = format!(
        "https://remote-config.gog.com/components/galaxy_client/clients/{safe}?component_version=2.0.45"
    );
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Other(format!("GOG remote config: {}", e.without_url())))?;
    if !resp.status().is_success() {
        return Err(Error::Other(format!(
            "GOG remote config: HTTP {}",
            resp.status()
        )));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| Error::Other(format!("GOG remote config: {}", e.without_url())))?;
    Ok(parse_config(&v, &safe))
}

fn parse_config(v: &Value, client_id: &str) -> CloudConfig {
    let storage = v.pointer("/content/Windows/cloudStorage");
    let enabled = storage
        .and_then(|s| s.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let locations = storage
        .and_then(|s| s.get("locations"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|l| {
                    Some(SaveLocation {
                        name: l.get("name")?.as_str()?.to_string(),
                        path: l.get("location")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    CloudConfig {
        client_id: client_id.to_string(),
        enabled,
        locations,
    }
}

/// The folders to sync, variables filled in for this PC (or the game's
/// prefix). A location that cannot be resolved keeps an empty path (the
/// user picks it by hand).
pub fn resolve(config: &CloudConfig, install_path: &str, roots: &Roots) -> Vec<SaveLocation> {
    let mut list = config.locations.clone();
    if list.is_empty() {
        // Galaxy's own storage folder when the game lists none
        let path = roots
            .local_app_data
            .join("GOG.com")
            .join("Galaxy")
            .join("Applications")
            .join(&config.client_id)
            .join("Storage")
            .join("Shared")
            .join("Files")
            .to_string_lossy()
            .into_owned();
        return vec![SaveLocation {
            name: "__default".into(),
            path,
        }];
    }
    for l in &mut list {
        l.path =
            resolve_path(&l.path, &|var| variable(var, install_path, roots)).unwrap_or_default();
    }
    list
}

fn variable(name: &str, install_path: &str, roots: &Roots) -> Option<PathBuf> {
    match name {
        "INSTALL" => Some(PathBuf::from(install_path)),
        "SAVED_GAMES" => Some(roots.saved_games.clone()),
        "APPLICATION_DATA_LOCAL" => Some(roots.local_app_data.clone()),
        "APPLICATION_DATA_LOCAL_LOW" => Some(roots.local_low.clone()),
        "APPLICATION_DATA_ROAMING" => Some(roots.roaming.clone()),
        "DOCUMENTS" => Some(roots.documents.clone()),
        _ => None,
    }
}

fn resolve_path(location: &str, lookup: &dyn Fn(&str) -> Option<PathBuf>) -> Option<String> {
    let re = Regex::new(r"<\?(\w+)\?>").ok()?;
    let mut unknown = false;
    let replaced = re.replace_all(location.trim(), |c: &regex::Captures| match lookup(&c[1]) {
        Some(p) => p.to_string_lossy().into_owned(),
        None => {
            tracing::warn!(variable = &c[1], "unknown GOG save path variable");
            unknown = true;
            String::new()
        }
    });
    if unknown {
        return None;
    }
    crate::saves::normalize(&expand_env(&replaced)?)
}

/// `%VAR%` → its value; `None` when a variable is not set.
fn expand_env(s: &str) -> Option<String> {
    let re = Regex::new(r"%(\w+)%").ok()?;
    let mut missing = false;
    let out = re.replace_all(s, |c: &regex::Captures| {
        std::env::var(&c[1]).unwrap_or_else(|_| {
            missing = true;
            String::new()
        })
    });
    (!missing).then(|| out.into_owned())
}

fn timestamps_file() -> PathBuf {
    paths::gogdl_config_dir().join("saveTimestamps.json")
}

type Timestamps = HashMap<String, HashMap<String, String>>;

fn load_timestamps() -> Timestamps {
    std::fs::read_to_string(timestamps_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Last sync of one location ("0": never).
pub fn timestamp(app_name: &str, location: &str) -> String {
    load_timestamps()
        .get(app_name)
        .and_then(|m| m.get(location))
        .cloned()
        .unwrap_or_else(|| "0".into())
}

pub fn set_timestamp(app_name: &str, location: &str, ts: &str) {
    let mut all = load_timestamps();
    all.entry(app_name.to_string())
        .or_default()
        .insert(location.to_string(), ts.to_string());
    if let Ok(raw) = serde_json::to_vec_pretty(&all)
        && let Err(e) = std::fs::write(timestamps_file(), raw)
    {
        tracing::warn!(error = %e, "could not save GOG save timestamps");
    }
}

pub fn forget(app_name: &str) {
    let mut all = load_timestamps();
    if all.remove(app_name).is_some()
        && let Ok(raw) = serde_json::to_vec_pretty(&all)
    {
        let _ = std::fs::write(timestamps_file(), raw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn lookup(var: &str) -> Option<PathBuf> {
        match var {
            "INSTALL" => Some(PathBuf::from("E:\\Games\\Witcher")),
            "SAVED_GAMES" => Some(PathBuf::from("C:\\Users\\a\\Saved Games")),
            _ => None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn resolves_into_the_wine_prefix() {
        let roots = Roots::prefix(Path::new("/home/deck/Prefixes/gog/1207658924"));
        let user = "/home/deck/Prefixes/gog/1207658924/drive_c/users/steamuser";
        let c = CloudConfig {
            client_id: "4567".into(),
            enabled: true,
            locations: vec![
                SaveLocation {
                    name: "saves".into(),
                    path: "<?SAVED_GAMES?>/CD Projekt Red/Witcher/".into(),
                },
                SaveLocation {
                    name: "cfg".into(),
                    path: "<?APPLICATION_DATA_LOCAL_LOW?>\\Studio\\Game".into(),
                },
                SaveLocation {
                    name: "inst".into(),
                    path: "<?INSTALL?>/saves".into(),
                },
                SaveLocation {
                    name: "odd".into(),
                    path: "<?APPLICATION_SUPPORT?>/x".into(),
                },
            ],
        };
        let l = resolve(&c, "/home/deck/Games/Witcher", &roots);
        assert_eq!(
            l[0].path,
            format!("{user}/Saved Games/CD Projekt Red/Witcher")
        );
        assert_eq!(l[1].path, format!("{user}/AppData/LocalLow/Studio/Game"));
        assert_eq!(l[2].path, "/home/deck/Games/Witcher/saves");
        assert_eq!(l[3].path, "", "unknown variable: left for the user");
        let none = CloudConfig {
            client_id: "4567".into(),
            enabled: true,
            locations: vec![],
        };
        assert_eq!(
            resolve(&none, "/g", &roots)[0].path,
            format!("{user}/AppData/Local/GOG.com/Galaxy/Applications/4567/Storage/Shared/Files")
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolves_gog_variables() {
        assert_eq!(
            resolve_path("<?SAVED_GAMES?>/CD Projekt Red/Witcher/", &lookup).as_deref(),
            Some("C:\\Users\\a\\Saved Games\\CD Projekt Red\\Witcher")
        );
        assert_eq!(
            resolve_path("<?INSTALL?>\\saves", &lookup).as_deref(),
            Some("E:\\Games\\Witcher\\saves")
        );
        assert_eq!(resolve_path("<?APPLICATION_SUPPORT?>/x", &lookup), None);
    }

    #[test]
    fn reads_the_windows_cloud_storage() {
        let v = serde_json::json!({
            "version": "2.0",
            "content": {
                "Windows": { "cloudStorage": { "enabled": true, "locations": [
                    { "name": "saves", "location": "<?DOCUMENTS?>/Game" }
                ] } },
                "MacOS": { "cloudStorage": { "enabled": false, "locations": [] } }
            }
        });
        let c = parse_config(&v, "123");
        assert!(c.enabled);
        assert_eq!(
            c.locations,
            [SaveLocation {
                name: "saves".into(),
                path: "<?DOCUMENTS?>/Game".into()
            }]
        );
        let none = parse_config(&serde_json::json!({ "content": {} }), "123");
        assert!(!none.enabled);
    }

    #[cfg(windows)]
    #[test]
    fn default_location_without_listed_folders() {
        let c = CloudConfig {
            client_id: "4567".into(),
            enabled: true,
            locations: vec![],
        };
        let roots = Roots::host().expect("home");
        let l = resolve(&c, "E:\\G", &roots);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].name, "__default");
        assert!(
            l[0].path
                .ends_with("GOG.com\\Galaxy\\Applications\\4567\\Storage\\Shared\\Files")
        );
    }
}
