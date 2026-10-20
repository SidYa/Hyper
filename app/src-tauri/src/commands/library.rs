use std::sync::Mutex;

use hyper_core::error::Result;
use hyper_core::types::{GameInfo, Runner, StoreStatus};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::events::LIBRARY_REFRESHING;
use crate::state::AppState;

/// Stores whose library is being fetched from the network right now (at
/// start and after a login, which can take a while); the UI shows a spinner
/// while the list is not empty.
#[derive(Default)]
pub struct Refreshing(Mutex<Vec<Runner>>);

impl Refreshing {
    fn update(&self, f: impl FnOnce(&mut Vec<Runner>)) -> Vec<Runner> {
        let mut list = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut list);
        list.clone()
    }
}

#[tauri::command]
pub fn get_refreshing_stores(refreshing: State<'_, Refreshing>) -> Result<Vec<Runner>> {
    Ok(refreshing.update(|_| {}))
}

/// Fetch a store's library from the network, marked as refreshing meanwhile.
pub async fn refresh_store(app: &AppHandle, runner: Runner) -> Result<()> {
    let state = app.state::<AppState>();
    let refreshing = app.state::<Refreshing>();
    let mark = runner.is_store();
    if mark {
        let list = refreshing.update(|l| l.push(runner));
        let _ = app.emit(LIBRARY_REFRESHING, list);
    }
    let out = state.stores.get(runner).library(true).await.map(|_| ());
    if mark {
        let list = refreshing.update(|l| {
            if let Some(i) = l.iter().position(|r| *r == runner) {
                l.remove(i);
            }
        });
        let _ = app.emit(LIBRARY_REFRESHING, list);
    }
    out
}

#[tauri::command]
pub async fn get_library(
    state: State<'_, AppState>,
    runner: Option<Runner>,
) -> Result<Vec<GameInfo>> {
    collect(&state, runner, false).await
}

#[tauri::command]
pub async fn get_game_info(
    state: State<'_, AppState>,
    runner: Runner,
    app_name: String,
) -> Result<GameInfo> {
    state.stores.find_game(runner, &app_name).await
}

#[tauri::command]
pub async fn get_store_status(state: State<'_, AppState>) -> Result<Vec<StoreStatus>> {
    let mut out = Vec::new();
    for store in state.stores.all() {
        if store.runner().is_store() {
            out.push(store.status().await);
        }
    }
    Ok(out)
}

async fn collect(state: &AppState, runner: Option<Runner>, force: bool) -> Result<Vec<GameInfo>> {
    let managers = match runner {
        Some(r) => vec![state.stores.get(r)],
        None => state.stores.all(),
    };
    let mut games = Vec::new();
    for m in managers {
        match m.library(force).await {
            Ok(mut g) => games.append(&mut g),
            Err(e) => {
                tracing::warn!(runner = ?m.runner(), error = %e, "library fetch failed")
            }
        }
    }
    Ok(games)
}
