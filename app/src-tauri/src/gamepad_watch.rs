//! System-level controller shortcut that works while a game has focus.
//!
//! The UI reads controllers through the browser Gamepad API, which only
//! delivers input to the focused window, so it is blind once a game is in
//! front. This thread reads controllers natively (XInput) and, only while a
//! game launched by Hyper is running, watches for:
//!
//!   hold  View/Back + Start  (≈ Select + Start)  for 2 seconds
//!
//! which force-closes the game and brings Hyper back to the front (or
//! closes an open store login page, see `commands::store`). A
//! two-button chord is used on purpose: games use B (and most single
//! buttons) for their own actions, so holding B would close games by accident.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gilrs::{Button, EventType, Gamepad, Gilrs};
use hyper_core::games::GameRunner;
use tauri::{AppHandle, Emitter, Manager};

use crate::MAIN_WINDOW;
use crate::commands::store;
use crate::events::GAMEPAD_QUIT_COMBO as QUIT_COMBO_PROGRESS;

const HOLD: Duration = Duration::from_millis(2000);
const TICK: Duration = Duration::from_millis(50);

pub fn spawn(app: AppHandle, games: Arc<GameRunner>) {
    std::thread::Builder::new()
        .name("gamepad-watch".into())
        .spawn(move || run(app, games))
        .map_err(|e| tracing::warn!(error = %e, "could not start gamepad watcher"))
        .ok();
}

fn run(app: AppHandle, games: Arc<GameRunner>) {
    let mut gilrs = match Gilrs::new() {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!(error = %e, "native gamepad input unavailable; in-game close shortcut disabled");
            return;
        }
    };
    tracing::info!("gamepad watcher started");
    for (_, gp) in gilrs.gamepads() {
        log_pad("controller found", &gp);
    }
    let mut held_since: Option<Instant> = None;
    let mut fired = false;
    loop {
        // drain events so the cached button state stays current
        while let Some(ev) = gilrs.next_event() {
            match ev.event {
                EventType::Connected => log_pad("controller connected", &gilrs.gamepad(ev.id)),
                EventType::Disconnected => tracing::info!(pad = %ev.id, "controller disconnected"),
                _ => {}
            }
        }

        // the same chord cancels a store login page (no other way out of it
        // with a controller: the page gets no gamepad input from us)
        let chord = gilrs
            .gamepads()
            .any(|(_, gp)| gp.is_pressed(Button::Select) && gp.is_pressed(Button::Start))
            && (games.any_running() || store::login_window_open(&app));

        match (chord, held_since) {
            (true, None) => {
                held_since = Some(Instant::now());
                fired = false;
            }
            (true, Some(t)) if !fired && t.elapsed() >= HOLD => {
                fired = true;
                if store::close_login_windows(&app) {
                    tracing::info!("controller quit chord held; closed the login page");
                    // no "closing the game" ring over the console UI
                    let _ = app.emit(QUIT_COMBO_PROGRESS, 0.0_f32);
                    std::thread::sleep(TICK);
                    continue;
                }
                tracing::info!("controller quit chord held; closing running game");
                let handle = app.clone();
                let games = games.clone();
                tauri::async_runtime::spawn(async move {
                    games.kill_all().await;
                    if let Some(w) = handle.get_window(MAIN_WINDOW) {
                        let _ = w.unminimize();
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                });
                let _ = app.emit(QUIT_COMBO_PROGRESS, 1.0_f32);
            }
            (true, Some(t)) if !fired => {
                let _ = app.emit(
                    QUIT_COMBO_PROGRESS,
                    (t.elapsed().as_secs_f32() / HOLD.as_secs_f32()).min(1.0),
                );
            }
            (false, Some(_)) => {
                // released (or the game is gone): hide the progress ring
                held_since = None;
                let _ = app.emit(QUIT_COMBO_PROGRESS, 0.0_f32);
            }
            _ => {}
        }
        std::thread::sleep(TICK);
    }
}

/// What the OS reports for a controller; on Linux the UI only sees its name,
/// so this line is how a remote tester's log tells which pad it was.
fn log_pad(what: &str, gp: &Gamepad<'_>) {
    tracing::info!(
        pad = %gp.id(),
        name = gp.name(),
        os_name = gp.os_name(),
        vendor = format!("{:04x}", gp.vendor_id().unwrap_or(0)),
        product = format!("{:04x}", gp.product_id().unwrap_or(0)),
        "{what}"
    );
}
