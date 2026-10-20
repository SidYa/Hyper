//! Hyper — Tauri shell.
//!
//! All business logic lives in `hyper-core`. This crate only:
//! - wires Tauri plugins and the main window,
//! - exposes `#[tauri::command]`s (`commands/`),
//! - pushes events to the webview (`events.rs`),
//! - sets up logging (`logger.rs`).
//!
//! The main window is built in code (not in `tauri.conf.json`) because it
//! hosts several webviews: the Svelte UI plus the embedded store pages
//! (see `commands/storeview.rs`).

pub mod close;
pub mod commands;
pub mod events;
pub mod gamepad_watch;
#[cfg(target_os = "linux")]
pub mod gtk_layout;
pub mod logger;
pub mod state;
pub mod tray;

use hyper_core::{config, paths, platform};
use tauri::webview::WebviewBuilder;
use tauri::{LogicalPosition, LogicalSize, Manager, WebviewUrl, Window};

pub const MAIN_WINDOW: &str = "main";

/// Steam Deck edition (`--features deck`, with `HYPER_EDITION=deck` for
/// the UI build): console mode only, no way out to the desktop UI.
pub const DECK: bool = cfg!(feature = "deck");

/// DevTools (F12 / right-click → Inspect) are off in every webview. For
/// debugging, a development build can opt in with HYPER_DEVTOOLS=1.
pub fn devtools_enabled() -> bool {
    cfg!(debug_assertions) && std::env::var("HYPER_DEVTOOLS").is_ok_and(|v| v == "1")
}

/// The configured 1280×800 window is larger than a small or high-DPI
/// screen (e.g. 2048×1152 at 150 % = 1365×768 logical). Shrink it to 90 % of
/// the monitor so nothing is cut off on first launch.
fn fit_to_monitor(win: &Window) {
    let (Ok(Some(monitor)), Ok(outer)) = (win.current_monitor(), win.outer_size()) else {
        return;
    };
    let scale = monitor.scale_factor();
    let avail = monitor.size().to_logical::<f64>(scale);
    let cur = outer.to_logical::<f64>(scale);
    if cur.width > avail.width || cur.height > avail.height {
        let w = (avail.width * 0.9).max(960.0);
        let h = (avail.height * 0.9).max(600.0);
        let _ = win.set_size(LogicalSize::new(w, h));
        let _ = win.center();
    }
}

/// No system frame: the UI draws its own title bar and window buttons.
fn build_main_window(app: &tauri::AppHandle) -> tauri::Result<Window> {
    let mut builder = tauri::window::WindowBuilder::new(app, MAIN_WINDOW)
        .title("Hyper")
        .decorations(false)
        .inner_size(1280.0, 800.0)
        .min_inner_size(960.0, 600.0)
        .center()
        .visible(false);
    // The low-level builder does not pick up the bundle icon by itself; without
    // it the taskbar shows the generic one.
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon)?;
    }
    let window = builder.build()?;
    // close → tray or the quit flow; minimize → tray (close.rs)
    window.on_window_event({
        let window = window.clone();
        move |event| close::on_window_event(&window, event)
    });
    let scale = window.scale_factor()?;
    let size = window.inner_size()?.to_logical::<f64>(scale);
    window.add_child(
        WebviewBuilder::new(MAIN_WINDOW, WebviewUrl::App("index.html".into()))
            .auto_resize()
            .devtools(devtools_enabled())
            // the launcher UI never leaves the app's own pages: a stray link
            // or redirect must not load a remote site in its place
            .on_navigation(|url| {
                let own = matches!(url.scheme(), "tauri")
                    || url.host_str() == Some("tauri.localhost")
                    || (cfg!(debug_assertions) && url.host_str() == Some("localhost"));
                if !own {
                    tracing::warn!(scheme = url.scheme(), host = ?url.host_str(), "main view navigation refused");
                }
                own
            })
            // wry's defaults, plus: console-mode UI sounds play without a
            // click first (a controller press is no "user gesture" to Chromium)
            .additional_browser_args(
                "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required",
            ),
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(size.width, size.height),
    )?;
    Ok(window)
}

pub fn run() {
    platform::current().init_process();
    let args: Vec<String> = std::env::args().collect();
    let start_in_console_cli = args.iter().any(|a| a == "--console")
        || std::env::var("HYPER_START_CONSOLE").is_ok_and(|v| v == "1")
        || DECK;

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // A second instance was launched (the tray is one per process).
            // Bring the existing main window to the front.
            if let Some(win) = app.get_window(MAIN_WINDOW) {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }))
        // The main window always opens maximized, so it is excluded from state
        // restore (which brought back whatever size it had when closed); the
        // login windows are sized for their page and stay out as well.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_filter(|label| {
                    label != MAIN_WINDOW
                        && !label.starts_with(commands::store::LOGIN_WINDOW_PREFIX)
                        && !label.starts_with(commands::storeview::POPUP_PREFIX)
                })
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            paths::ensure_dirs()?;
            let log_guard = logger::init(paths::logs_dir());
            // Keep the guard alive for the whole app lifetime.
            app.manage(log_guard);

            let settings = config::AppSettings::load_or_default();
            tracing::info!(
                version = env!("CARGO_PKG_VERSION"),
                platform = platform::current().name(),
                config_dir = %paths::config_dir().display(),
                vault = %paths::vault_dir().display(),
                "Hyper starting"
            );

            let app_state = state::AppState::new(
                app.handle().clone(),
                settings.clone(),
                start_in_console_cli,
            );
            // the queue of the last session comes back paused, then the worker starts
            let downloads = app_state.downloads.clone();
            tauri::async_runtime::spawn(async move {
                downloads.restore().await;
                downloads.run().await;
            });
            // saves played offline go up once the stores are reachable again
            tauri::async_runtime::spawn(app_state.stores.clone().watch_pending());
            gamepad_watch::spawn(app.handle().clone(), app_state.games.clone());
            app.manage(app_state);
            app.manage(commands::library::Refreshing::default());
            if let Err(e) = tray::build(app.handle(), &settings) {
                // no tray: the "Minimize to tray" option then closes normally
                tracing::warn!(error = %e, "tray icon not available");
            }

            // Refresh every store from the network in the background; the UI
            // already shows the on-disk library and updates on this event.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<state::AppState>();
                for store in state.stores.all() {
                    if let Err(e) = commands::library::refresh_store(&handle, store.runner()).await {
                        tracing::warn!(runner = ?store.runner(), error = %e, "background library refresh failed");
                    }
                }
                let _ = tauri::Emitter::emit(&handle, events::LIBRARY_UPDATED, serde_json::json!({ "runner": null }));
            });

            let window = build_main_window(app.handle())?;
            #[cfg(target_os = "linux")]
            gtk_layout::install(app.handle());
            // fit_to_monitor keeps the restore-down size sensible on small screens
            fit_to_monitor(&window);
            let _ = window.maximize();
            let _ = window.show();
            let _ = window.set_focus();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::get_app_info,
            commands::app::log_gamepad,
            commands::app::set_fullscreen,
            commands::app::open_external,
            commands::app::open_app_folder,
            commands::app::quit,
            commands::app::get_close_blockers,
            commands::app::confirm_close_request,
            commands::app::list_folders,
            commands::app::list_folder_places,
            commands::app::create_folder,
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::get_game_settings,
            commands::settings::save_game_settings,
            commands::library::get_library,
            commands::library::get_refreshing_stores,
            commands::library::get_store_status,
            commands::store::store_login_start,
            commands::store::store_logout,
            commands::storeview::store_webview_show,
            commands::storeview::store_webview_bounds,
            commands::storeview::store_webview_hide,
            commands::storeview::store_webview_command,
            commands::storeview::store_webview_release_focus,
            commands::storeview::store_webview_scrollbar,
            commands::games::get_install_info,
            commands::games::get_dlcs,
            commands::games::move_game,
            commands::games::get_achievements,
            commands::games::install_game,
            commands::games::update_game,
            commands::games::repair_game,
            commands::games::get_game_extra,
            commands::games::get_play_stats,
            commands::games::cancel_download,
            commands::games::get_downloads,
            commands::games::resume_download,
            commands::games::uninstall_game,
            commands::games::launch_game,
            commands::games::kill_game,
            commands::games::open_game_folder,
            commands::games::find_importable_games,
            commands::games::import_game,
            commands::games::get_cloud_saves,
            commands::games::sync_saves,
            commands::games::open_save_folder,
            commands::sideload::sideload_add,
            commands::sideload::sideload_edit,
            commands::sideload::sideload_get,
            commands::sideload::exe_icon,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Hyper")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event
                && let Some(state) = app.try_state::<state::AppState>()
            {
                // no plaintext credentials outlive the app (a helper still
                // running dies with it: see process.rs `job`)
                state.stores.seal_leftovers();
            }
        });
}
