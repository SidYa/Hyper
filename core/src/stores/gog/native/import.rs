//! What `gogdl import` read from an installed game's folder: the product,
//! build, language and installed DLCs from `goggame-<id>.info` /
//! `goggame-<id>.id`, and the build's version name from GOG.

use std::path::Path;

use serde_json::Value;

use super::{Client, id_string};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Imported {
    pub app_name: String,
    pub build_id: String,
    pub title: String,
    /// Language code of the install.
    pub language: String,
    /// Product ids of the DLCs installed alongside.
    pub dlcs: Vec<String>,
    pub version_name: String,
}

/// The `goggame-*.info` files directly in `dir` (Windows installs; gogdl
/// also looks in `Contents/Resources` and `game/` for macOS / Linux
/// installers, which we do not import).
fn info_files(dir: &Path) -> Vec<(String, Value)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, Value)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.strip_prefix("goggame-")?.strip_suffix(".info")?;
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let raw = std::fs::read_to_string(e.path()).ok()?;
            Some((id.to_string(), serde_json::from_str(&raw).ok()?))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Everything that can be read without the network.
pub fn read_folder(dir: &Path) -> Result<Imported> {
    let infos = info_files(dir);
    if infos.is_empty() {
        return Err(Error::Other(format!(
            "{} has no goggame-*.info file",
            dir.display()
        )));
    }
    let root_id = infos
        .iter()
        .find_map(|(_, v)| v.get("rootGameId").and_then(id_string))
        .unwrap_or_else(|| infos[0].0.clone());
    let (_, info) = infos.iter().find(|(id, _)| *id == root_id).ok_or_else(|| {
        Error::Other(format!(
            "{} is missing goggame-{root_id}.info",
            dir.display()
        ))
    })?;
    let dlcs = infos
        .iter()
        .filter(|(id, _)| *id != root_id)
        .map(|(id, _)| id.clone())
        .collect();
    let mut build_id = info.get("buildId").and_then(id_string).unwrap_or_default();
    if let Ok(raw) = std::fs::read_to_string(dir.join(format!("goggame-{root_id}.id")))
        && let Ok(v) = serde_json::from_str::<Value>(&raw)
        && let Some(b) = v.get("buildId").and_then(id_string)
    {
        build_id = b;
    }
    let language = info
        .get("languages")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .or_else(|| info.get("language").and_then(Value::as_str))
        .unwrap_or("en-US");
    Ok(Imported {
        app_name: root_id,
        build_id: build_id.clone(),
        title: info
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        language: super::languages::code(language),
        dlcs,
        version_name: build_id,
    })
}

/// `read_folder` plus the build's version name from GOG.
pub async fn scan(client: &Client, dir: &Path, platform: &str) -> Result<Imported> {
    let mut out = read_folder(dir)?;
    if !out.build_id.is_empty() {
        match client.builds(&out.app_name, platform).await {
            Ok(builds) => {
                if let Some(b) = super::target_build(&builds, Some(&out.build_id), None)
                    && let Some(name) = b.get("version_name").and_then(Value::as_str)
                {
                    out.version_name = name.to_string();
                }
            }
            Err(e) => {
                tracing::warn!(app = out.app_name, error = %e, "GOG builds unavailable; version unknown")
            }
        }
    }
    Ok(out)
}

/// Save the gogdl manifest of `build_id` (or of the latest public build when
/// it is not listed any more) with the install's language and DLCs, so an
/// imported game can be updated and repaired like one installed here.
pub async fn restore_manifest(
    client: &Client,
    app_name: &str,
    platform: &str,
    build_id: &str,
    language: &str,
    dlcs: &[String],
) -> Result<()> {
    let builds = client.builds(app_name, platform).await?;
    let build = super::target_build(&builds, Some(build_id), None)
        .ok_or_else(|| Error::Other(format!("no build of {app_name} found")))?;
    let link = super::meta_url(build)
        .ok_or_else(|| Error::Other("the build has no manifest link".into()))?;
    let meta = client.get_zlib_json(link, false).await?;
    let dlcs = dlcs
        .iter()
        .map(|id| super::manifest::Dlc {
            id: id.clone(),
            title: String::new(),
        })
        .collect();
    let manifest = super::manifest::Manifest::new(meta, platform, language, dlcs)?;
    let dir = crate::paths::gogdl_manifests_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(app_name), manifest.to_json().to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_game_and_dlc_info_files() {
        let dir = std::env::temp_dir().join(format!("hyper-gog-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("goggame-100.info"), r#"{ "gameId": "100", "rootGameId": "100", "name": "Game", "buildId": "old", "language": "English" }"#).unwrap();
        std::fs::write(
            dir.join("goggame-200.info"),
            r#"{ "gameId": "200", "rootGameId": "100", "name": "DLC" }"#,
        )
        .unwrap();
        std::fs::write(dir.join("goggame-100.id"), r#"{ "buildId": "b42" }"#).unwrap();
        let i = read_folder(&dir).unwrap();
        assert_eq!(i.app_name, "100");
        assert_eq!(i.title, "Game");
        assert_eq!(i.build_id, "b42");
        assert_eq!(i.language, "en-US");
        assert_eq!(i.dlcs, ["200"]);
        assert!(read_folder(&dir.join("nope")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
