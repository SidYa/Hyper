use std::sync::Arc;

use hyper_core::compat::Compat;
use hyper_core::config::AppSettings;
use hyper_core::downloads::DownloadManager;
use hyper_core::events::Events;
use hyper_core::games::GameRunner;
use hyper_core::secrets::Vault;
use hyper_core::stores::{StoreContext, Stores};
use hyper_core::{paths, platform};
use tauri::AppHandle;
use tokio::sync::RwLock;

use crate::events::TauriSink;

/// Global application state managed by Tauri (`app.state::<AppState>()`).
pub struct AppState {
    pub app: AppHandle,
    pub settings: Arc<RwLock<AppSettings>>,
    pub vault: Arc<Vault>,
    pub stores: Arc<Stores>,
    pub downloads: Arc<DownloadManager>,
    pub games: Arc<GameRunner>,
    /// `--console` was passed on the command line.
    pub start_in_console_cli: bool,
}

impl AppState {
    pub fn new(app: AppHandle, settings: AppSettings, start_in_console_cli: bool) -> Self {
        let settings = Arc::new(RwLock::new(settings));
        let events: Events = Arc::new(TauriSink(app.clone()));
        let vault = Arc::new(Vault::new(paths::vault_dir()));
        let games = GameRunner::new(events.clone());
        if platform::current().needs_wine() {
            games.enable_compat(Compat::new(settings.clone()));
        }
        let stores = Arc::new(Stores::new(StoreContext {
            settings: settings.clone(),
            vault: vault.clone(),
            events: events.clone(),
            games: games.clone(),
        }));
        let downloads = DownloadManager::new(stores.clone(), events);
        Self {
            app,
            settings,
            vault,
            stores,
            downloads,
            games,
            start_in_console_cli,
        }
    }
}
