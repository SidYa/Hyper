//! `legendary list --third-party`, done by us: refresh `assets.json` from
//! Epic's asset list, fetch catalog metadata for titles that are new or have
//! a new build (`metadata/<app>.json`), drop metadata of titles that left the
//! account, and add the library items without assets (titles handled by
//! Ubisoft Connect / the EA app). Port of `get_game_and_dlc_list` and
//! `get_non_asset_library_items` in legendary's `core.py`, minus the
//! achievements (unused here and the main source of 429s) and aliases.
//!
//! The files are what `legendary::library::load_library` reads.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::api::{Epic, asset_record};
use super::{metadata_path, read_json, write_json};
use crate::error::Result;
use crate::stores::legendary::library::{list_metadata_apps, read_installed};

/// Metadata requests in flight at once (legendary uses 16 threads, which
/// trips Epic's rate limit on big libraries).
const PARALLEL_METADATA: usize = 8;

#[derive(Debug, Default)]
pub struct SyncReport {
    pub assets_changed: bool,
    pub metadata_fetched: usize,
    pub metadata_failed: usize,
    pub pruned: usize,
    pub third_party_added: usize,
}

struct MetaJob {
    app: String,
    namespace: String,
    catalog_id: String,
    /// platform → asset record, as stored in `assets.json`
    asset_infos: Map<String, Value>,
    existing: Option<Value>,
    fetch_sidecar: bool,
}

pub async fn sync(epic: Arc<Epic>) -> Result<SyncReport> {
    let dir = epic.config_dir.clone();
    epic.refresh_launcher_config().await;
    let installed = read_installed(&dir);
    let mut platforms: BTreeSet<String> = BTreeSet::from(["Windows".to_string()]);
    platforms.extend(
        installed
            .values()
            .map(|i| i.platform.clone())
            .filter(|p| !p.is_empty()),
    );

    let assets_path = dir.join("assets.json");
    let mut assets: Map<String, Value> = read_json(&assets_path)
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let before = assets.clone();
    for platform in &platforms {
        let list = epic.assets(platform).await?;
        let records: Vec<Value> = list
            .iter()
            .map(|a| Value::Object(asset_record(a)))
            .collect();
        assets.insert(platform.clone(), Value::Array(records));
    }
    let mut report = SyncReport {
        assets_changed: assets != before,
        ..SyncReport::default()
    };
    if report.assets_changed {
        write_json(&assets_path, &Value::Object(assets.clone()))?;
    }

    // app → platform → asset record
    let mut by_app: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for (platform, list) in &assets {
        for a in list.as_array().into_iter().flatten() {
            if let Some(app) = a.get("app_name").and_then(Value::as_str) {
                by_app
                    .entry(app.to_string())
                    .or_default()
                    .insert(platform.clone(), a.clone());
            }
        }
    }

    let mut jobs = Vec::new();
    for (app, infos) in &by_app {
        let str_of = |a: &Value, k: &str| {
            a.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        // Unreal Engine marketplace assets are not games
        if infos.values().any(|a| str_of(a, "namespace") == "ue") {
            continue;
        }
        let Some(first) = infos.values().next() else {
            continue;
        };
        let existing = read_json(&metadata_path(&dir, app));
        // sidecar data is the same for every platform; Windows is the baseline
        let win_rev = infos
            .get("Windows")
            .and_then(|a| a.get("sidecar_rev"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let (needs_fetch, fetch_sidecar) = match &existing {
            None => (true, win_rev > 0),
            Some(g) => {
                let asset_updated = infos.iter().any(|(p, a)| {
                    g.pointer(&format!("/asset_infos/{p}/build_version")) != a.get("build_version")
                });
                let sidecar_updated = win_rev > 0
                    && g.pointer("/sidecar/rev").and_then(Value::as_i64) != Some(win_rev);
                (asset_updated || sidecar_updated, sidecar_updated)
            }
        };
        if needs_fetch {
            jobs.push(MetaJob {
                app: app.clone(),
                namespace: str_of(first, "namespace"),
                catalog_id: str_of(first, "catalog_item_id"),
                asset_infos: infos.clone(),
                existing,
                fetch_sidecar,
            });
        }
    }

    if !jobs.is_empty() {
        tracing::info!(count = jobs.len(), "fetching Epic metadata");
        let sem = Arc::new(Semaphore::new(PARALLEL_METADATA));
        let mut set = JoinSet::new();
        for job in jobs {
            let (epic, sem, dir) = (epic.clone(), sem.clone(), dir.clone());
            set.spawn(async move {
                let _slot = sem.acquire().await;
                let app = job.app.clone();
                (app, fetch_metadata(&epic, &dir, job).await)
            });
        }
        let mut last_err = None;
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((_, Ok(()))) => report.metadata_fetched += 1,
                Ok((app, Err(e))) => {
                    report.metadata_failed += 1;
                    tracing::warn!(app, error = %e, "Epic metadata not fetched");
                    last_err = Some(e);
                }
                Err(e) => {
                    report.metadata_failed += 1;
                    tracing::warn!(error = %e, "Epic metadata task failed");
                }
            }
        }
        // nothing came through (offline, rate-limited for good): report it
        if report.metadata_fetched == 0
            && let Some(e) = last_err
        {
            return Err(e);
        }
    }

    // titles in the library without an installable build (third-party
    // launchers, other platforms): their metadata stays
    let library_items = match epic.library_items().await {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!(error = %e, "Epic library items not fetched");
            Vec::new()
        }
    };
    let in_library: BTreeSet<&str> = library_items
        .iter()
        .filter_map(|i| i.get("appName").and_then(Value::as_str))
        .collect();

    // metadata of titles that left the account: gone, unless installed
    for app in list_metadata_apps(&dir) {
        if by_app.contains_key(&app)
            || installed.contains_key(&app)
            || in_library.contains(app.as_str())
        {
            continue;
        }
        let path = metadata_path(&dir, &app);
        let Some(game) = read_json(&path) else {
            continue;
        };
        if is_third_party(&game) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            tracing::debug!(app, "unused Epic metadata removed");
            report.pruned += 1;
        }
    }

    // ... and the ones not seen yet get their metadata (shown as owned,
    // not installable)
    for item in &library_items {
        let s = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or_default();
        let app = s("appName");
        if app.is_empty() || app == "1" || s("namespace") == "ue" || s("sandboxType") == "PRIVATE" {
            continue;
        }
        if by_app.contains_key(app) || metadata_path(&dir, app).is_file() {
            continue;
        }
        match epic.game_info(s("namespace"), s("catalogItemId")).await {
            Ok(Some(info)) => {
                let game = json!({
                    "app_name": app,
                    "app_title": info.get("title").cloned().unwrap_or(Value::from("Unknown")),
                    "metadata": info,
                    "asset_infos": {},
                    "base_urls": [],
                    "sidecar": Value::Null,
                    "achievements": Value::Null,
                });
                if let Err(e) = write_json(&metadata_path(&dir, app), &game) {
                    tracing::warn!(app, error = %e, "could not save Epic metadata");
                } else {
                    report.third_party_added += 1;
                }
            }
            Ok(None) => tracing::debug!(app, "library item without catalog entry"),
            Err(e) => tracing::warn!(app, error = %e, "Epic metadata not fetched"),
        }
    }

    tracing::info!(?report, "Epic library synced");
    Ok(report)
}

/// `metadata/<app>.json` as legendary's `Game.__dict__` writes it.
async fn fetch_metadata(epic: &Epic, dir: &Path, job: MetaJob) -> Result<()> {
    let info = epic
        .game_info(&job.namespace, &job.catalog_id)
        .await?
        .unwrap_or_else(|| {
            tracing::warn!(app = job.app, "Epic has no metadata for this title");
            json!({ "title": "Unknown" })
        });
    let existing = |k: &str| job.existing.as_ref().and_then(|g| g.get(k)).cloned();
    let sidecar = if job.fetch_sidecar {
        let record = epic
            .manifest_record(&job.namespace, &job.catalog_id, &job.app, "Windows")
            .await?;
        match record.get("sidecar") {
            // the config is a JSON object stored as a string
            Some(s) => {
                let config: Value =
                    serde_json::from_str(s.get("config").and_then(Value::as_str).unwrap_or("{}"))
                        .unwrap_or(json!({}));
                json!({ "config": config, "rev": s.get("rvn").cloned().unwrap_or(Value::from(0)) })
            }
            None => Value::Null,
        }
    } else {
        existing("sidecar").unwrap_or(Value::Null)
    };
    let game = json!({
        "app_name": job.app,
        "app_title": info.get("title").cloned().unwrap_or(Value::from("Unknown")),
        "metadata": info,
        "asset_infos": Value::Object(job.asset_infos),
        "base_urls": existing("base_urls").unwrap_or_else(|| json!([])),
        "sidecar": sidecar,
        "achievements": existing("achievements").unwrap_or(Value::Null),
    });
    write_json(&metadata_path(dir, &job.app), &game)
}

/// Titles another launcher installs (Ubisoft Connect, the EA app): no
/// assets of their own, kept in the library all the same.
pub(crate) fn is_third_party(game: &Value) -> bool {
    ["ThirdPartyManagedApp", "ThirdPartyManagedProvider"]
        .iter()
        .any(|k| {
            game.pointer(&format!("/metadata/customAttributes/{k}/value"))
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty())
        })
}

/// Catalog id and namespace of a title's metadata file.
pub(crate) fn catalog_ids(game: &Value) -> (String, String) {
    let s = |p: &str| {
        game.pointer(p)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    (s("/metadata/namespace"), s("/metadata/id"))
}

/// App names of the DLCs of the game with `catalog_id` that have a build for
/// `platform` (legendary's `get_dlc_for_game`).
pub(crate) fn dlcs_of(dir: &Path, catalog_id: &str, platform: &str) -> Vec<String> {
    if catalog_id.is_empty() {
        return Vec::new();
    }
    list_metadata_apps(dir)
        .into_iter()
        .filter(|app| {
            read_json(&metadata_path(dir, app)).is_some_and(|g| {
                g.pointer("/metadata/mainGameItem/id")
                    .and_then(Value::as_str)
                    == Some(catalog_id)
                    && g.pointer(&format!("/asset_infos/{platform}")).is_some()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn third_party_and_dlc_detection() {
        let dir = std::env::temp_dir().join(format!("hyper-epic-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let base = json!({ "app_name": "Base", "metadata": { "id": "c-base", "namespace": "ns" }, "asset_infos": { "Windows": {} } });
        let dlc = json!({ "app_name": "Dlc1", "metadata": { "id": "c-dlc", "namespace": "ns", "mainGameItem": { "id": "c-base" } }, "asset_infos": { "Windows": {} } });
        let mac_dlc = json!({ "app_name": "Dlc2", "metadata": { "id": "c-dlc2", "mainGameItem": { "id": "c-base" } }, "asset_infos": { "Mac": {} } });
        let ubi = json!({ "app_name": "Ubi", "metadata": { "customAttributes": { "ThirdPartyManagedApp": { "value": "UbisoftConnect" } } } });
        for g in [&base, &dlc, &mac_dlc, &ubi] {
            write_json(&metadata_path(&dir, g["app_name"].as_str().unwrap()), g).unwrap();
        }
        assert_eq!(dlcs_of(&dir, "c-base", "Windows"), ["Dlc1"]);
        assert!(dlcs_of(&dir, "", "Windows").is_empty());
        assert!(is_third_party(&ubi));
        assert!(!is_third_party(&base));
        assert_eq!(catalog_ids(&base), ("ns".into(), "c-base".into()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
