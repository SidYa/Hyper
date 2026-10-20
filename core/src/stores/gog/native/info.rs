//! What `gogdl info` reported for a game: the build to install, its sizes
//! per language, languages, owned DLCs (with their sizes), the install
//! folder and the redistributables it needs.

use std::collections::HashMap;

use serde_json::Value;

use super::manifest::{Dlc, Manifest};
use super::{Client, id_string, target_build};
use crate::error::{Error, Result};

/// (download, disk) bytes per language code, `*` being the shared part.
pub type Sizes = HashMap<String, (u64, u64)>;

#[derive(Debug, Clone, Default)]
pub struct DlcInfo {
    pub id: String,
    pub title: String,
    pub size: Sizes,
}

#[derive(Debug, Clone, Default)]
pub struct Info {
    pub folder_name: String,
    pub version_name: String,
    pub build_id: String,
    /// Other branches of the game (beta…).
    pub branches: Vec<String>,
    /// Every build the account may install, newest first.
    pub builds: Vec<crate::types::BuildEntry>,
    /// Language codes the build comes in.
    pub languages: Vec<String>,
    pub size: Sizes,
    /// DLCs the account owns.
    pub dlcs: Vec<DlcInfo>,
    pub dependencies: Vec<String>,
    pub depot_version: u8,
}

/// The DLC products of a meta the account owns (`None` = all of them).
pub async fn owned_dlcs(
    client: &Client,
    manifest_meta: &Value,
    game_id: &str,
    requested: Option<&[String]>,
) -> Result<Vec<Dlc>> {
    let mut out = Vec::new();
    let v2 = manifest_meta.get("products").and_then(Value::as_array);
    let v1 = manifest_meta
        .pointer("/product/gameIDs")
        .and_then(Value::as_array);
    for p in v2.or(v1).into_iter().flatten() {
        let Some(id) = p
            .get("productId")
            .or_else(|| p.get("gameID"))
            .and_then(id_string)
        else {
            continue;
        };
        if id == game_id {
            continue;
        }
        if let Some(want) = requested
            && !want.contains(&id)
        {
            continue;
        }
        if !client.owns(&id).await? {
            continue;
        }
        let title = p
            .get("name")
            .map(|n| match n {
                Value::String(s) => s.clone(),
                other => other
                    .get("en")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
            .unwrap_or_default();
        out.push(Dlc { id, title });
    }
    Ok(out)
}

/// Sizes of the game-folder redistributables a build needs (the ones
/// installed system-wide, under `__redist`, are not part of the download).
fn add_dependency_sizes(repo: &Value, deps: &[String], into: &mut Sizes) {
    for depot in repo
        .get("depots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = depot
            .get("dependencyId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !deps.iter().any(|d| d == id) {
            continue;
        }
        if depot
            .pointer("/executable/path")
            .and_then(Value::as_str)
            .is_some_and(|p| p.starts_with("__redist"))
        {
            continue;
        }
        let e = into.entry("*".into()).or_default();
        e.0 += depot
            .get("compressedSize")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        e.1 += depot.get("size").and_then(Value::as_u64).unwrap_or(0);
    }
}

pub async fn fetch(
    client: &Client,
    app_name: &str,
    platform: &str,
    branch: Option<&str>,
    build_id: Option<&str>,
    password: Option<&str>,
) -> Result<Info> {
    let builds = client
        .builds_with_password(app_name, platform, password)
        .await?;
    let build = target_build(&builds, build_id, branch)
        .ok_or_else(|| Error::Other(format!("GOG has no {platform} build of {app_name}")))?;
    let link = super::meta_url(build)
        .ok_or_else(|| Error::Other("the build has no manifest link".into()))?;
    let meta = client.get_zlib_json(link, false).await?;
    let dlcs = owned_dlcs(client, &meta, app_name, None).await?;
    let manifest = Manifest::new(meta, platform, "en-US", dlcs.clone())?;
    let mut sizes = manifest.sizes();
    if !manifest.dependencies.is_empty() {
        match client.dependencies_repo().await {
            Ok(repo) => add_dependency_sizes(
                &repo,
                &manifest.dependencies,
                sizes.entry(app_name.to_string()).or_default(),
            ),
            Err(e) => {
                tracing::warn!(app = app_name, error = %e, "GOG dependencies repository unavailable; sizes without them")
            }
        }
    }
    let build_id = if manifest.version == 1 {
        build
            .get("legacy_build_id")
            .and_then(id_string)
            .unwrap_or_default()
    } else {
        build
            .get("build_id")
            .and_then(id_string)
            .unwrap_or_default()
    };
    Ok(Info {
        folder_name: manifest.install_directory.clone(),
        version_name: build
            .get("version_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        build_id,
        branches: super::branch_names(&builds),
        builds: super::build_entries(&builds),
        languages: manifest.languages(),
        size: sizes.remove(app_name).unwrap_or_default(),
        dlcs: dlcs
            .into_iter()
            .map(|d| DlcInfo {
                size: sizes.remove(&d.id).unwrap_or_default(),
                id: d.id,
                title: d.title,
            })
            .collect(),
        dependencies: manifest.dependencies.clone(),
        depot_version: manifest.version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_sizes_skip_system_redists() {
        let repo = serde_json::json!({ "depots": [
            { "dependencyId": "DirectX", "executable": { "path": "__redist/DirectX/setup.exe" }, "size": 100, "compressedSize": 50 },
            { "dependencyId": "ISI", "executable": { "path": "scriptinterpreter.exe" }, "size": 30, "compressedSize": 20 },
            { "dependencyId": "Other", "executable": { "path": "x.exe" }, "size": 1, "compressedSize": 1 }
        ] });
        let mut sizes = Sizes::new();
        add_dependency_sizes(&repo, &["DirectX".into(), "ISI".into()], &mut sizes);
        assert_eq!(sizes["*"], (20, 30));
    }
}
