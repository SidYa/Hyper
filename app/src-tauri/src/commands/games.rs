//! Install / update / uninstall / launch.

use hyper_core::error::{Error, Result};
use hyper_core::types::{
    CloudSaves, DownloadItem, GameExtra, InstallInfo, InstallPlatform, PlayStats, Runner,
    SaveSyncMode,
};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::state::AppState;

/// A folder the UI picked (install / import / move target): must be a full
/// path, so nothing resolves against the process' working directory.
fn full_path(path: &str) -> Result<String> {
    let path = path.trim();
    if !std::path::Path::new(path).is_absolute() {
        return Err(Error::Other("the folder must be a full path".into()));
    }
    Ok(path.to_string())
}

#[tauri::command]
pub async fn get_install_info(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
    platform: Option<InstallPlatform>,
) -> Result<InstallInfo> {
    state
        .stores
        .get(runner)
        .install_info(&app_name, platform.unwrap_or_default())
        .await
}

/// Move an installed game into another folder (its own subfolder is kept).
#[tauri::command]
pub async fn move_game(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
    path: String,
) -> Result<()> {
    let path = full_path(&path)?;
    state.stores.move_game(runner, &app_name, &path).await
}

/// The game's achievements with the account's progress.
#[tauri::command]
pub async fn get_achievements(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
) -> Result<hyper_core::types::Achievements> {
    state.stores.achievements(runner, &app_name).await
}

/// The add-ons of a game (installed or not).
#[tauri::command]
pub async fn get_dlcs(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
) -> Result<Vec<hyper_core::types::DlcEntry>> {
    state.stores.get(runner).dlcs(&app_name).await
}

/// Queue an install. Returns immediately; progress arrives as events.
#[tauri::command]
pub async fn install_game(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
    path: Option<String>,
    platform: Option<InstallPlatform>,
    branch: Option<String>,
    build_id: Option<String>,
) -> Result<Vec<DownloadItem>> {
    let game = state.stores.find_game(runner, &app_name).await?;
    let path = match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => full_path(&p)?,
        None => state.settings.read().await.default_install_path.clone(),
    };
    state
        .downloads
        .enqueue(
            DownloadItem::install(
                runner,
                &app_name,
                &game.title,
                path,
                platform.unwrap_or_default(),
            )
            .with_build(branch, build_id),
        )
        .await?;
    Ok(state.downloads.items().await)
}

#[tauri::command]
pub async fn update_game(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
) -> Result<Vec<DownloadItem>> {
    let game = state.stores.find_game(runner, &app_name).await?;
    let platform = game
        .install
        .as_ref()
        .map(|i| i.platform)
        .unwrap_or_default();
    state
        .downloads
        .enqueue(DownloadItem::update(
            runner,
            &app_name,
            &game.title,
            platform,
        ))
        .await?;
    Ok(state.downloads.items().await)
}

#[tauri::command]
pub async fn repair_game(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
) -> Result<Vec<DownloadItem>> {
    let game = state.stores.find_game(runner, &app_name).await?;
    let platform = game
        .install
        .as_ref()
        .map(|i| i.platform)
        .unwrap_or_default();
    state
        .downloads
        .enqueue(DownloadItem::repair(
            runner,
            &app_name,
            &game.title,
            platform,
        ))
        .await?;
    Ok(state.downloads.items().await)
}

/// Store-page details (description, screenshots…) in the UI language.
#[tauri::command]
pub async fn get_game_extra(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
) -> Result<GameExtra> {
    let lang = state.settings.read().await.language.clone();
    state.stores.get(runner).extra_info(&app_name, &lang).await
}

#[tauri::command]
pub async fn get_play_stats(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
) -> Result<PlayStats> {
    Ok(state.games.play_stats(runner, &app_name))
}

#[tauri::command]
pub async fn cancel_download(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
) -> Result<Vec<DownloadItem>> {
    state.downloads.cancel(runner, &app_name).await?;
    Ok(state.downloads.items().await)
}

/// Continue a download left paused from the last session.
#[tauri::command]
pub async fn resume_download(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
) -> Result<Vec<DownloadItem>> {
    state.downloads.resume(runner, &app_name).await?;
    Ok(state.downloads.items().await)
}

#[tauri::command]
pub async fn get_downloads(state: State<'_, AppState>) -> Result<Vec<DownloadItem>> {
    Ok(state.downloads.items().await)
}

#[tauri::command]
pub async fn uninstall_game(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
) -> Result<()> {
    state.stores.get(runner).uninstall(&app_name).await
}

#[tauri::command]
pub async fn launch_game(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
) -> Result<()> {
    // through `Stores` so auto-sync of cloud saves wraps the launch
    state.stores.launch(runner, &app_name).await
}

#[tauri::command]
pub async fn kill_game(state: State<'_, AppState>, runner: Runner, app_name: String) -> Result<()> {
    state.games.kill(runner, &app_name).await
}

#[tauri::command]
pub async fn open_game_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
) -> Result<()> {
    let game = state.stores.find_game(runner, &app_name).await?;
    let path = game
        .install
        .map(|i| i.install_path)
        .ok_or_else(|| hyper_core::Error::Other("game is not installed".into()))?;
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| hyper_core::Error::Other(e.to_string()))
}

/// Scan `paths` (folders of games, or game folders) for owned titles that
/// are installed there but unknown to the launcher, across every store.
#[tauri::command]
pub async fn find_importable_games(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<Vec<hyper_core::types::ImportCandidate>> {
    let mut out: Vec<hyper_core::types::ImportCandidate> = Vec::new();
    for path in &paths {
        let dir = std::path::PathBuf::from(path);
        let is_dir = tokio::fs::metadata(&dir).await.is_ok_and(|m| m.is_dir());
        if !dir.is_absolute() || !is_dir {
            tracing::warn!(path, "import folder not found");
            continue;
        }
        for store in state.stores.all() {
            match store.find_importable(&dir).await {
                // the same game can be found through two overlapping folders
                Ok(found) => out.extend(
                    found
                        .into_iter()
                        .filter(|c| {
                            !out.iter()
                                .any(|o| o.runner == c.runner && o.app_name == c.app_name)
                        })
                        .collect::<Vec<_>>(),
                ),
                Err(e) => {
                    tracing::warn!(runner = ?store.runner(), error = %e, "import scan failed")
                }
            }
        }
    }
    out.sort_by_key(|c| c.title.to_lowercase());
    Ok(out)
}

/// Register an existing installation of one game.
#[tauri::command]
pub async fn import_game(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
    path: String,
) -> Result<()> {
    let path = full_path(&path)?;
    state.stores.get(runner).import_game(&app_name, &path).await
}

/// Cloud-save support, settings and folders of an installed game.
#[tauri::command]
pub async fn get_cloud_saves(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
) -> Result<CloudSaves> {
    state.stores.cloud_saves(runner, &app_name).await
}

/// Sync a game's saves now; returns what the helper reported.
#[tauri::command]
pub async fn sync_saves(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
    mode: SaveSyncMode,
) -> Result<String> {
    state.stores.sync_saves(runner, &app_name, mode).await
}

/// Open one of the game's save folders (only the ones the settings name).
#[tauri::command]
pub async fn open_save_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
    name: String,
) -> Result<()> {
    let saves = state.stores.cloud_saves(runner, &app_name).await?;
    let path = saves
        .locations
        .into_iter()
        .find(|l| l.name == name && l.exists)
        .map(|l| l.path)
        .ok_or_else(|| hyper_core::Error::Other("the save folder does not exist yet".into()))?;
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| hyper_core::Error::Other(e.to_string()))
}
