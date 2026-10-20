//! System tray: Hyper's icon with "Open" / "Quit". It is visible only while
//! the "Minimize to tray" setting (`exit_to_tray`) is on; then closing the
//! main window hides it instead and downloads carry on in the background
//! (`close.rs`; minimizing still goes to the taskbar). A left click on the
//! icon toggles the window. Not built in the Deck edition (gamescope has
//! no tray).

use hyper_core::config::AppSettings;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::MAIN_WINDOW;

pub const TRAY_ID: &str = "main";

/// The menu entries, kept to relabel them when the language changes.
pub struct TrayMenu {
    open: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

fn labels(language: &str) -> (&'static str, &'static str) {
    match language {
        "uk" => ("Відкрити Hyper", "Вийти"),
        _ => ("Open Hyper", "Quit"),
    }
}

/// Create the (hidden) tray icon once at startup; `apply` shows it.
pub fn build(app: &AppHandle, settings: &AppSettings) -> tauri::Result<()> {
    if crate::DECK {
        return Ok(());
    }
    let (open_label, quit_label) = labels(&settings.language);
    let open = MenuItem::with_id(app, "open", open_label, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", quit_label, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Hyper")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "quit" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { crate::close::quit(&app, false).await });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    let tray = builder.build(app)?;
    tray.set_visible(settings.exit_to_tray)?;
    app.manage(TrayMenu { open, quit });
    Ok(())
}

/// The tray exists (it may be missing on a Linux desktop without an
/// app-indicator library, and in the Deck edition).
pub fn available(app: &AppHandle) -> bool {
    app.tray_by_id(TRAY_ID).is_some()
}

/// Closing the window hides it to the tray.
pub fn hides_window(app: &AppHandle, settings: &AppSettings) -> bool {
    settings.exit_to_tray && available(app)
}

/// Follow the settings: icon shown while the option is on, menu in the
/// app's language.
pub fn apply(app: &AppHandle, settings: &AppSettings) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    if let Err(e) = tray.set_visible(settings.exit_to_tray) {
        tracing::warn!(error = %e, "tray icon visibility");
    }
    if let Some(menu) = app.try_state::<TrayMenu>() {
        let (open_label, quit_label) = labels(&settings.language);
        let _ = menu.open.set_text(open_label);
        let _ = menu.quit.set_text(quit_label);
    }
}

/// Bring the main window back from the tray (or the taskbar).
pub fn show_main(app: &AppHandle) {
    if let Some(win) = app.get_window(MAIN_WINDOW) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// A click on the tray icon: hidden or minimized → show; shown → hide to
/// the tray.
pub fn toggle_main(app: &AppHandle) {
    let Some(win) = app.get_window(MAIN_WINDOW) else {
        return;
    };
    let shown = win.is_visible().unwrap_or(false) && !win.is_minimized().unwrap_or(false);
    if shown {
        let _ = win.hide();
    } else {
        show_main(app);
    }
}
