//! Names of events emitted from Rust to the webview, and the bridge that
//! forwards `hyper_core` events to them.
//! Keep in sync with `app/src/lib/api/events.ts`.

use hyper_core::events::{CoreEvent, EventSink};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub const LIBRARY_UPDATED: &str = "library:updated";
/// Stores whose library is being fetched (`Vec<Runner>`, empty when done).
pub const LIBRARY_REFRESHING: &str = "library:refreshing";
pub const GAME_STATUS: &str = "game:status";
pub const DOWNLOAD_QUEUE: &str = "download:queue";
pub const STORE_LOGIN_CHANGED: &str = "store:login-changed";
pub const STORE_LOGIN_ERROR: &str = "store:login-error";
pub const GAMEPAD_QUIT_COMBO: &str = "gamepad:quit-combo";
/// A game's saves wait for an upload (played offline) or were uploaded.
pub const SAVES_PENDING: &str = "saves:pending";
/// Closing was asked while downloads / save syncs run (`close::CloseBlockers`):
/// the UI asks the user and calls `quit` with `force`.
pub const APP_CLOSE_REQUESTED: &str = "app:close-requested";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SavesPending {
    runner: hyper_core::types::Runner,
    app_name: String,
    pending: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LibraryUpdated {
    runner: Option<hyper_core::types::Runner>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LoginError {
    pub runner: hyper_core::types::Runner,
    pub message: String,
}

pub struct TauriSink(pub AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, event: CoreEvent) {
        let result = match event {
            CoreEvent::GameStatus(s) => self.0.emit(GAME_STATUS, s),
            CoreEvent::LibraryUpdated(runner) => {
                self.0.emit(LIBRARY_UPDATED, LibraryUpdated { runner })
            }
            CoreEvent::StoreLogin(s) => self.0.emit(STORE_LOGIN_CHANGED, s),
            CoreEvent::DownloadQueue(items) => self.0.emit(DOWNLOAD_QUEUE, items),
            CoreEvent::SavesPending {
                runner,
                app_name,
                pending,
            } => self.0.emit(
                SAVES_PENDING,
                SavesPending {
                    runner,
                    app_name,
                    pending,
                },
            ),
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, "failed to emit event to webview");
        }
    }
}
