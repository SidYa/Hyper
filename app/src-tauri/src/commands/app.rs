use std::sync::atomic::{AtomicBool, Ordering};

use hyper_core::error::{Error, Result};
use hyper_core::{paths, platform};
use serde::Serialize;
use tauri::{AppHandle, State, Window};
use tauri_plugin_opener::OpenerExt;

use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub platform: &'static str,
    pub config_dir: String,
    pub logs_dir: String,
    pub start_in_console_cli: bool,
    pub always_console: bool,
}

/// The UI's first call: from here on it listens for events (`close.rs`).
#[tauri::command]
pub fn get_app_info(state: State<'_, AppState>) -> AppInfo {
    crate::close::mark_ui_ready();
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: platform::current().name(),
        config_dir: paths::config_dir().to_string_lossy().into_owned(),
        logs_dir: paths::logs_dir().to_string_lossy().into_owned(),
        start_in_console_cli: state.start_in_console_cli,
        always_console: crate::DECK,
    }
}

/// The window was maximized before console mode made it fullscreen.
static WAS_MAXIMIZED: AtomicBool = AtomicBool::new(false);

/// A controller as the webview sees it (`Gamepad.id` / `mapping`), for the
/// log: on Linux (WebKitGTK) the id is a bare device name, unlike Windows.
#[tauri::command]
pub fn log_gamepad(index: u32, id: String, mapping: String, buttons: u32, axes: u32) -> Result<()> {
    let id: String = id.chars().take(200).collect();
    let mapping: String = mapping.chars().take(20).collect();
    tracing::info!(index, id, mapping, buttons, axes, "webview controller");
    Ok(())
}

#[tauri::command]
pub fn set_fullscreen(window: Window, fullscreen: bool) -> Result<()> {
    // An undecorated window that is *maximized* keeps its client area cut to
    // the monitor's work area (tao leaves the taskbar visible) even after it
    // goes fullscreen, so console mode ended 60 px short with the frame
    // showing below. Leave the maximized state first and restore it after.
    if fullscreen {
        let maximized = window.is_maximized().unwrap_or(false);
        WAS_MAXIMIZED.store(maximized, Ordering::Relaxed);
        if maximized {
            let _ = window.unmaximize();
        }
    }
    window
        .set_fullscreen(fullscreen)
        .map_err(|e| Error::Other(e.to_string()))?;
    if !fullscreen && WAS_MAXIMIZED.swap(false, Ordering::Relaxed) {
        let _ = window.maximize();
    }
    Ok(())
}

#[tauri::command]
pub async fn open_external(app: AppHandle, url: String) -> Result<()> {
    // Web pages only: links come from store data, and other schemes (file:,
    // custom protocol handlers) would open local files or programs.
    let parsed =
        tauri::Url::parse(url.trim()).map_err(|e| Error::Other(format!("invalid link: {e}")))?;
    if !matches!(parsed.scheme(), "https" | "http") {
        return Err(Error::Other(format!(
            "refusing to open a `{}:` link",
            parsed.scheme()
        )));
    }
    // the opener blocks on the shell: off the async runtime's threads
    tokio::task::spawn_blocking(move || app.opener().open_url(parsed.as_str(), None::<&str>))
        .await
        .map_err(|e| Error::Other(e.to_string()))?
        .map_err(|e| Error::Other(e.to_string()))
}

/// Open one of Hyper's own folders in the file manager. Takes a name, not
/// a path, so the UI cannot be used to open arbitrary locations.
#[tauri::command]
pub fn open_app_folder(app: AppHandle, which: String) -> Result<()> {
    let dir = match which.as_str() {
        "config" => paths::config_dir(),
        "logs" => paths::logs_dir(),
        other => return Err(Error::Other(format!("unknown folder `{other}`"))),
    };
    std::fs::create_dir_all(&dir)?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| Error::Other(e.to_string()))
}

/// Leave the app (see `close.rs`). Without `force`, a running download or
/// save sync makes the UI ask first (`app:close-requested`).
#[tauri::command]
pub async fn quit(app: AppHandle, force: Option<bool>) -> Result<()> {
    crate::close::quit(&app, force.unwrap_or(false)).await;
    Ok(())
}

/// The UI got `app:close-requested` and is asking the user; without this
/// within a few seconds the app quits by itself (see `close.rs`).
#[tauri::command]
pub async fn confirm_close_request() -> Result<()> {
    crate::close::confirm_request();
    Ok(())
}

/// What quitting now would interrupt (for the console menu's confirmation).
#[tauri::command]
pub async fn get_close_blockers(state: State<'_, AppState>) -> Result<crate::close::CloseBlockers> {
    Ok(crate::close::blockers(&state).await)
}

/// Folders inside `path` (the drives when empty), for the console-mode
/// folder picker.
#[tauri::command]
pub async fn list_folders(path: Option<String>) -> Result<hyper_core::browse::FolderListing> {
    tokio::task::spawn_blocking(move || hyper_core::browse::list(path.as_deref()))
        .await
        .map_err(|e| Error::Other(e.to_string()))?
}

/// Places the folder picker jumps to (games library, home, documents…).
#[tauri::command]
pub async fn list_folder_places(
    state: State<'_, AppState>,
) -> Result<Vec<hyper_core::browse::PlaceEntry>> {
    let games = state.settings.read().await.default_install_path.clone();
    tokio::task::spawn_blocking(move || hyper_core::browse::places(&games))
        .await
        .map_err(|e| Error::Other(e.to_string()))
}

/// A new folder inside `parent` (the picker's "New folder"); returns its path.
#[tauri::command]
pub async fn create_folder(parent: String, name: String) -> Result<String> {
    tokio::task::spawn_blocking(move || hyper_core::browse::create(&parent, &name))
        .await
        .map_err(|e| Error::Other(e.to_string()))?
}
