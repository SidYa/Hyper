//! Games added by hand (not from a store): add / edit / read back.
//! Launch, remove and the library listing go through the generic game
//! commands like every other store.

use hyper_core::error::Result;
use hyper_core::platform;
use hyper_core::types::{GameInfo, SideloadGame, SideloadInput};
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub async fn sideload_add(state: State<'_, AppState>, input: SideloadInput) -> Result<GameInfo> {
    state.stores.sideload.add(input).await
}

#[tauri::command]
pub async fn sideload_edit(
    state: State<'_, AppState>,
    app_name: String,
    input: SideloadInput,
) -> Result<GameInfo> {
    state.stores.sideload.edit(&app_name, input).await
}

#[tauri::command]
pub async fn sideload_get(state: State<'_, AppState>, app_name: String) -> Result<SideloadGame> {
    state.stores.sideload.get(&app_name).await
}

/// PNG bytes of an executable's icon, for the card preview in the add form.
#[tauri::command]
pub async fn exe_icon(path: String) -> Result<Option<Vec<u8>>> {
    Ok(tauri::async_runtime::spawn_blocking(move || {
        platform::current().exe_icon_png(std::path::Path::new(&path))
    })
    .await
    .ok()
    .flatten())
}
