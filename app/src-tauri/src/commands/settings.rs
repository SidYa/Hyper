use hyper_core::config::AppSettings;
use hyper_core::config::game::GameSettings;
use hyper_core::error::Result;
use hyper_core::types::Runner;
use tauri::{AppHandle, State};

use crate::state::AppState;

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<AppSettings> {
    Ok(state.settings.read().await.clone())
}

/// Launch options of one game (`GamesConfig/<store>-<game>.json`).
#[tauri::command]
pub async fn get_game_settings(runner: Runner, app_name: String) -> Result<GameSettings> {
    Ok(GameSettings::load(runner, &app_name))
}

#[tauri::command]
pub async fn save_game_settings(
    runner: Runner,
    app_name: String,
    settings: GameSettings,
) -> Result<()> {
    settings.save(runner, &app_name)
}

#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<AppSettings> {
    settings.save()?;
    let mut current = state.settings.write().await;
    *current = settings;
    tracing::info!("settings saved");
    crate::tray::apply(&app, &current);
    Ok(current.clone())
}
