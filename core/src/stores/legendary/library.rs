//! Builds [`GameInfo`]s from the files `legendary` keeps in its config dir:
//! `metadata/<app>.json` (one per owned title), `installed.json` and
//! `assets.json` (latest build versions, used for update detection).
//! Port of `loadFile` / `refreshInstalled` / `listUpdateableGames` from the
//! original `library.ts`.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::stores::human_size;
use crate::types::{GameInfo, InstallPlatform, InstalledInfo, Runner};

/// One entry of `installed.json`: the fields the library and the launch
/// read (serde skips the rest of what legendary keeps there; the whole
/// entry is written by `epic::download` / `epic::installed` as JSON).
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct InstalledEntry {
    pub executable: String,
    pub install_path: String,
    #[serde(default)]
    pub install_size: u64,
    #[serde(default)]
    pub is_dlc: bool,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub can_run_offline: bool,
    /// The build's own command line (manifest `launch_command`).
    #[serde(default)]
    pub launch_parameters: String,
    /// The game checks an ownership token at start (`-epicovt=`).
    #[serde(default)]
    pub requires_ot: bool,
}

pub fn read_installed(config_dir: &Path) -> HashMap<String, InstalledEntry> {
    let path = config_dir.join("installed.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    match serde_json::from_str::<HashMap<String, InstalledEntry>>(&raw) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "corrupted installed.json");
            HashMap::new()
        }
    }
}

/// app_name → latest build version, per platform, from `assets.json`.
pub fn read_latest_versions(config_dir: &Path) -> HashMap<(String, String), String> {
    let path = config_dir.join("assets.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    if let Some(platforms) = v.as_object() {
        for (platform, assets) in platforms {
            if let Some(list) = assets.as_array() {
                for a in list {
                    if let (Some(app), Some(ver)) = (
                        a.get("app_name").and_then(Value::as_str),
                        a.get("build_version").and_then(Value::as_str),
                    ) {
                        out.insert((platform.clone(), app.to_string()), ver.to_string());
                    }
                }
            }
        }
    }
    out
}

/// App names that have a metadata file.
pub fn list_metadata_apps(config_dir: &Path) -> Vec<String> {
    let dir = config_dir.join("metadata");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".json").map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

/// Parse one `metadata/<app>.json`. Returns `None` for titles Heroic also
/// hides: Unreal Engine assets, mods, mobile-only games.
pub fn game_from_metadata(
    raw: &str,
    installed: Option<&InstalledEntry>,
    latest_version: Option<&str>,
) -> Option<GameInfo> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let app_name = v.get("app_name")?.as_str()?.to_string();
    let md = v.get("metadata")?;

    let namespace = md.get("namespace").and_then(Value::as_str).unwrap_or("");
    let categories: Vec<&str> = md
        .get("categories")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| c.get("path").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    let ue_categories = ["assets", "asset-format", "plugins", "projects"];
    if namespace == "ue" || categories.iter().any(|c| ue_categories.contains(c)) {
        return None;
    }
    if categories.contains(&"mods") {
        return None;
    }
    let release_info = md.get("releaseInfo").and_then(Value::as_array);
    let mobile_only = release_info.is_some_and(|ri| {
        !ri.is_empty()
            && ri.iter().all(|info| {
                info.get("platform")
                    .and_then(Value::as_array)
                    .is_some_and(|p| {
                        p.iter()
                            .all(|x| matches!(x.as_str(), Some("Android") | Some("iOS")))
                    })
            })
    });
    if mobile_only {
        return None;
    }

    let title = md
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(&app_name)
        .to_string();
    let developer = md
        .get("developer")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let description = md
        .get("description")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| md.get("shortDescription").and_then(Value::as_str))
        .map(str::to_owned);

    let attr = |name: &str| -> Option<String> {
        md.get("customAttributes")?
            .get(name)?
            .get("value")?
            .as_str()
            .map(str::to_owned)
    };
    let folder_name = attr("FolderName").unwrap_or_else(|| app_name.clone());
    let can_run_offline = attr("CanRunOffline").as_deref() == Some("true");
    let third_party = attr("ThirdPartyManagedApp").or_else(|| attr("ThirdPartyManagedProvider"));
    let save_folder = attr("CloudSaveFolder").filter(|s| !s.is_empty());

    let key_images = md.get("keyImages").and_then(Value::as_array);
    let image = |types: &[&str]| -> Option<String> {
        key_images?
            .iter()
            .find(|img| {
                img.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| types.contains(&t))
            })
            .and_then(|img| img.get("url").and_then(Value::as_str))
            .map(str::to_owned)
    };
    let wide = image(&["DieselGameBox", "OfferImageWide"]);
    let tall = image(&["DieselGameBoxTall", "OfferImageTall"]);
    let store_front = image(&["DieselStoreFrontTall"]);
    let logo = image(&["DieselGameBoxLogo"]);

    let is_dlc = md.get("mainGameItem").is_some();

    let install = installed.map(|i| InstalledInfo {
        executable: i.executable.clone(),
        install_path: i.install_path.clone(),
        install_size: human_size(i.install_size),
        install_size_bytes: i.install_size,
        is_dlc: i.is_dlc,
        version: i.version.clone(),
        platform: InstallPlatform::from_legendary(&i.platform),
        language: None,
        build_id: None,
        branch: None,
        installed_dlcs: None,
        pinned_version: false,
    });
    let update_available = match (installed, latest_version) {
        (Some(i), Some(latest)) => !i.version.is_empty() && i.version != latest,
        _ => false,
    };

    Some(GameInfo {
        runner: Some(Runner::Legendary),
        app_name,
        title: title.clone(),
        art_cover: wide.clone().or_else(|| tall.clone()).unwrap_or_default(),
        art_square: tall.or(store_front).or(wide).unwrap_or_default(),
        art_logo: logo,
        art_background: None,
        art_icon: None,
        developer,
        description,
        store_url: Some(epic_store_url(&title)),
        folder_name: Some(folder_name),
        is_installed: installed.is_some(),
        // EA / Ubisoft titles install through their own launcher (`third_party`)
        installable: third_party
            .as_deref()
            .is_none_or(|t| super::third_party::Launcher::from_app(t).is_some()),
        is_dlc,
        can_run_offline,
        cloud_save_enabled: save_folder.is_some(),
        save_folder,
        version: installed.map(|i| i.version.clone()),
        third_party_managed_app: third_party,
        update_available,
        install,
    })
}

fn epic_store_url(title: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    format!("https://store.epicgames.com/browse?q={slug}&sortBy=relevancy")
}

/// Full library snapshot from the config dir.
pub fn load_library(config_dir: &Path) -> Vec<GameInfo> {
    let mut installed = read_installed(config_dir);
    // games another launcher installed for us: no files, only the record
    for (app, platform) in super::third_party::read_installed(config_dir) {
        installed.entry(app).or_insert(InstalledEntry {
            platform,
            ..InstalledEntry::default()
        });
    }
    let latest = read_latest_versions(config_dir);
    let mut games = Vec::new();
    for app in list_metadata_apps(config_dir) {
        let path = config_dir.join("metadata").join(format!("{app}.json"));
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let inst = installed.get(&app);
        let platform = inst
            .map(|i| i.platform.clone())
            .unwrap_or_else(|| "Windows".into());
        let latest_ver = latest.get(&(platform, app.clone())).map(String::as_str);
        match game_from_metadata(&raw, inst, latest_ver) {
            Some(g) => games.push(g),
            None => tracing::trace!(app, "skipped (UE asset / mod / mobile)"),
        }
    }
    games
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: &str = r#"{
      "app_name": "Fortnite",
      "metadata": {
        "namespace": "fn",
        "title": "Fortnite",
        "developer": "Epic Games",
        "description": "Battle royale",
        "categories": [{"path": "games"}],
        "releaseInfo": [{"appId": "Fortnite", "platform": ["Windows"]}],
        "customAttributes": {
          "FolderName": {"type": "STRING", "value": "FortniteGame"},
          "CanRunOffline": {"type": "STRING", "value": "false"}
        },
        "keyImages": [
          {"type": "DieselGameBoxTall", "url": "https://cdn/tall.jpg"},
          {"type": "DieselGameBox", "url": "https://cdn/wide.jpg"}
        ]
      }
    }"#;

    #[test]
    fn maps_metadata_to_game_info() {
        let g = game_from_metadata(META, None, None).unwrap();
        assert_eq!(g.app_name, "Fortnite");
        assert_eq!(g.title, "Fortnite");
        assert_eq!(g.art_cover, "https://cdn/wide.jpg");
        assert_eq!(g.art_square, "https://cdn/tall.jpg");
        assert_eq!(g.folder_name.as_deref(), Some("FortniteGame"));
        assert!(!g.is_installed);
        assert!(g.installable);
        assert!(!g.can_run_offline);
    }

    #[test]
    fn installed_and_update_detection() {
        let inst = InstalledEntry {
            executable: "FortniteClient.exe".into(),
            install_path: "C:\\Games\\Fortnite".into(),
            install_size: 1024 * 1024 * 1024,
            is_dlc: false,
            platform: "Windows".into(),
            version: "1.0".into(),
            can_run_offline: false,
            launch_parameters: String::new(),
            requires_ot: false,
        };
        let g = game_from_metadata(META, Some(&inst), Some("1.1")).unwrap();
        assert!(g.is_installed);
        assert!(g.update_available);
        assert_eq!(g.install.as_ref().unwrap().install_size, "1.0 GB");
        let g = game_from_metadata(META, Some(&inst), Some("1.0")).unwrap();
        assert!(!g.update_available);
    }

    #[test]
    fn skips_unreal_assets_and_mods() {
        let ue = META.replace("\"namespace\": \"fn\"", "\"namespace\": \"ue\"");
        assert!(game_from_metadata(&ue, None, None).is_none());
        let modded = META.replace("{\"path\": \"games\"}", "{\"path\": \"mods\"}");
        assert!(game_from_metadata(&modded, None, None).is_none());
    }
}
