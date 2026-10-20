//! Leaving the app. Every way out — the title-bar close button and Alt+F4
//! (`WindowEvent::CloseRequested`), the tray's Quit, the console menu's
//! "Quit Hyper" (the `quit` command) — ends in [`quit`]:
//!
//! - with "Minimize to tray" on, closing the window only hides it (a plain
//!   minimize still goes to the taskbar);
//! - otherwise, while a download runs or waits, or cloud saves are being
//!   synced, the UI is asked first (`app:close-requested` with what is
//!   busy) and quits with `force` once the user confirmed;
//! - a UI that does not confirm the request within [`ACK_TIMEOUT`] (a hung
//!   or crashed webview) cannot show the warning: the app quits anyway. The
//!   UI confirms on arrival, before its dialog, so a user still deciding is
//!   never overruled;
//! - the running download is stopped gently (`DownloadManager::shutdown`:
//!   its resume file stays, the queue comes back paused next start), then
//!   the process exits.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Window, WindowEvent};

use crate::state::AppState;
use crate::{events, tray};

/// What closing now would interrupt (titles).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseBlockers {
    pub downloads: Vec<String>,
    pub syncing: Vec<String>,
}

impl CloseBlockers {
    pub fn is_empty(&self) -> bool {
        self.downloads.is_empty() && self.syncing.is_empty()
    }
}

pub async fn blockers(state: &AppState) -> CloseBlockers {
    let downloads = state.downloads.busy_titles().await;
    let mut syncing = Vec::new();
    for (runner, app) in state.stores.syncing_games() {
        let title = state
            .stores
            .find_game(runner, &app)
            .await
            .map(|g| g.title)
            .unwrap_or(app);
        syncing.push(title);
    }
    CloseBlockers { downloads, syncing }
}

/// The UI has started and listens for `app:close-requested` (set by its
/// first `get_app_info` call). Before that, `emit` would succeed with no
/// listener and the close request would be lost, so quitting goes ahead.
static UI_READY: AtomicBool = AtomicBool::new(false);

pub fn mark_ui_ready() {
    UI_READY.store(true, Ordering::Relaxed);
}

/// How long the UI has to confirm `app:close-requested` before the app
/// quits without it.
const ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// Close requests sent to the UI, and the last one it confirmed.
static REQUESTED: AtomicU64 = AtomicU64::new(0);
static CONFIRMED: AtomicU64 = AtomicU64::new(0);

/// The UI received `app:close-requested` (it calls this before showing
/// its dialog).
pub fn confirm_request() {
    CONFIRMED.fetch_max(REQUESTED.load(Ordering::SeqCst), Ordering::SeqCst);
}

/// Quit, unless something is busy and `force` is not set: then the window
/// comes to the front and the UI shows the warning instead.
pub async fn quit(app: &AppHandle, force: bool) {
    let state = app.state::<AppState>();
    if !force {
        let busy = blockers(&state).await;
        if !busy.is_empty() {
            tracing::info!(
                downloads = busy.downloads.len(),
                syncing = busy.syncing.len(),
                "close asked while busy"
            );
            if !UI_READY.load(Ordering::Relaxed) {
                tracing::warn!("UI not up yet, nothing can show the close warning; quitting");
            } else {
                tray::show_main(app);
                let id = REQUESTED.fetch_add(1, Ordering::SeqCst) + 1;
                match app.emit(events::APP_CLOSE_REQUESTED, busy) {
                    Ok(()) => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            tokio::time::sleep(ACK_TIMEOUT).await;
                            if CONFIRMED.load(Ordering::SeqCst) < id {
                                tracing::warn!("the UI did not answer the close request; quitting");
                                exit_now(&app).await;
                            }
                        });
                        return;
                    }
                    Err(e) => tracing::warn!(error = %e, "close warning not delivered; quitting"),
                }
            }
        }
    }
    exit_now(app).await;
}

/// Stop the running download gently and leave.
async fn exit_now(app: &AppHandle) {
    app.state::<AppState>().downloads.shutdown().await;
    app.exit(0);
}

/// Window events of the main window: close → tray when the option is on,
/// otherwise [`quit`]. Minimize is left alone (taskbar, as usual).
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let app = window.app_handle().clone();
        let window = window.clone();
        tauri::async_runtime::spawn(async move {
            let state = app.state::<AppState>();
            let to_tray = tray::hides_window(&app, &*state.settings.read().await);
            if to_tray {
                let _ = window.hide();
            } else {
                quit(&app, false).await;
            }
        });
    }
}
