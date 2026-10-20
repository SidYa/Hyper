//! Store login / logout.
//!
//! Each store's web login opens in its own window whose browser profile is
//! shared with the embedded Store tab (`storeview.rs`). When the store
//! redirects to its success URL we intercept the navigation, cancel it,
//! close the window and exchange the authorization code ourselves
//! (`Stores::login`, over HTTPS; the token goes to the vault).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use hyper_core::error::{Error, Result};
use hyper_core::paths;
use hyper_core::stores::{gog, legendary};
use hyper_core::types::Runner;
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, Manager, State, Url, WebviewUrl, WebviewWindowBuilder};

use crate::MAIN_WINDOW;
use crate::commands::storeview::{self, Page};
use crate::events::{LIBRARY_UPDATED, LoginError, STORE_LOGIN_ERROR};
use crate::state::AppState;

pub const LOGIN_WINDOW_PREFIX: &str = "login-";

/// A code exchange is running for the store (a page redirecting twice must
/// not start two); cleared when `finish_login` is done.
static EPIC_LOGIN_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static GOG_LOGIN_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

fn login_in_flight(runner: Runner) -> &'static AtomicBool {
    match runner {
        Runner::Gog => &GOG_LOGIN_IN_FLIGHT,
        Runner::Legendary | Runner::Sideload => &EPIC_LOGIN_IN_FLIGHT,
    }
}

fn login_window_label(runner: Runner) -> String {
    format!("{LOGIN_WINDOW_PREFIX}{}", runner.id())
}

/// Closes every open login window (the controller cancel chord) and brings
/// the main window back. Returns whether one was open.
pub fn close_login_windows(app: &AppHandle) -> bool {
    let open: Vec<_> = app
        .webview_windows()
        .into_iter()
        .filter(|(label, _)| label.starts_with(LOGIN_WINDOW_PREFIX))
        .map(|(_, w)| w)
        .collect();
    for w in &open {
        let _ = w.close();
    }
    if !open.is_empty() {
        focus_main(app);
    }
    !open.is_empty()
}

pub fn login_window_open(app: &AppHandle) -> bool {
    app.webview_windows()
        .keys()
        .any(|l| l.starts_with(LOGIN_WINDOW_PREFIX))
}

fn focus_main(app: &AppHandle) {
    if let Some(w) = app.get_window(MAIN_WINDOW) {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Returns the authorization code when `url` is the store's success redirect:
/// https on the default port, the exact host (and GOG's path). A page that
/// navigates to `http://localhost:1/?code=…` must not log the launcher into
/// someone else's account.
fn login_code(runner: Runner, url: &Url) -> Option<String> {
    if url.scheme() != "https" || url.port().is_some() {
        return None;
    }
    let host_ok = match runner {
        Runner::Legendary => url.host_str() == Some("localhost"),
        Runner::Gog => {
            url.host_str() == Some(gog::LOGIN_REDIRECT_HOST)
                && url.path().starts_with(gog::LOGIN_REDIRECT_PATH)
        }
        Runner::Sideload => false,
    };
    if !host_ok {
        return None;
    }
    url.query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
}

#[tauri::command]
pub async fn store_login_start(
    app: AppHandle,
    runner: Runner,
    console: Option<bool>,
) -> Result<()> {
    open_login_window(&app, runner, console.unwrap_or(false))
}

#[tauri::command]
pub async fn store_logout(
    app: AppHandle,
    state: State<'_, AppState>,
    runner: Runner,
) -> Result<()> {
    state.stores.get(runner).logout().await?;
    if let Some(w) = app.get_webview_window(&login_window_label(runner)) {
        let _ = w.close();
    }
    storeview::close(&app, runner);
    // Drop the browser profile so the next login asks for credentials and
    // the embedded store is logged out too.
    let dir = paths::webview_data_dir(runner.id());
    if dir.exists() {
        // the browser process releases the folder asynchronously (and only
        // once every window on this profile is gone); retry for a while, a
        // half-deleted profile breaks the next login's pages
        let mut last = Ok(());
        for _ in 0..40 {
            last = tokio::fs::remove_dir_all(&dir).await;
            if last.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        if let Err(e) = last {
            tracing::warn!(error = %e, "could not clear browser profile");
        }
    }
    Ok(())
}

/// `console`: the main window is fullscreen (console mode, maybe a TV or the
/// Steam Deck), so the page opens maximized and stays above it.
fn open_login_window(app: &AppHandle, runner: Runner, console: bool) -> Result<()> {
    let label = login_window_label(runner);
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }
    let (url, title, user_agent) = match runner {
        Runner::Legendary => (
            legendary::LOGIN_URL,
            "Epic Games — Hyper",
            legendary::LOGIN_USER_AGENT,
        ),
        Runner::Gog => (gog::LOGIN_URL, "GOG — Hyper", storeview::STORE_USER_AGENT),
        Runner::Sideload => return Err(Error::Other("sideloaded games have no login".into())),
    };
    let url = Url::parse(url).map_err(|e| Error::Other(e.to_string()))?;
    let handle = app.clone();
    // Host of the document the window shows: set once a navigation is
    // committed (`on_page_load` Started), not when it is merely asked for,
    // so a page the window was lured to cannot pose as the store by
    // starting a navigation there and redirecting to the success URL.
    let shown_host = Arc::new(Mutex::new(String::new()));
    let committed = shown_host.clone();
    WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .devtools(crate::devtools_enabled())
        .title(title)
        .inner_size(560.0, 760.0)
        .center()
        .maximized(console)
        .always_on_top(console)
        .user_agent(user_agent)
        .data_directory(paths::webview_data_dir(runner.id()))
        .on_page_load(move |_, payload| {
            if matches!(payload.event(), PageLoadEvent::Started) {
                let host = payload.url().host_str().unwrap_or_default().to_string();
                *committed.lock().unwrap_or_else(|e| e.into_inner()) = host;
            }
        })
        .on_navigation(move |url| {
            if let Some(code) = login_code(runner, url) {
                let last = shown_host.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if !is_store_host(runner, &last) {
                    tracing::warn!(?runner, from = %last, "login redirect not coming from the store; ignored");
                    return false;
                }
                if login_in_flight(runner).swap(true, Ordering::SeqCst) {
                    tracing::warn!(?runner, "login redirect while a code exchange runs; ignored");
                    return false;
                }
                let app = handle.clone();
                tauri::async_runtime::spawn(async move { finish_login(app, runner, code).await });
                return false;
            }
            // the same pages the Store tab may show in place; the rest goes
            // to the user's browser
            match storeview::page_policy(url) {
                Page::Store => true,
                Page::Elsewhere => {
                    tracing::info!(?runner, url = %storeview::redact(url), "opening off-store link in the browser");
                    storeview::open_in_browser(&handle, url);
                    false
                }
                Page::Refused => {
                    tracing::warn!(?runner, scheme = url.scheme(), "login page navigation refused");
                    false
                }
            }
        })
        .on_download(|_, event| storeview::refuse_download(event))
        .build()
        .map_err(|e| Error::Other(format!("could not open login window: {e}")))?;
    Ok(())
}

/// The store's own sign-in pages: a success redirect only counts when the
/// window was on one of them right before.
fn is_store_host(runner: Runner, host: &str) -> bool {
    let domain = match runner {
        Runner::Legendary => "epicgames.com",
        Runner::Gog => "gog.com",
        Runner::Sideload => return false,
    };
    host == domain || host.ends_with(&format!(".{domain}"))
}

async fn finish_login(app: AppHandle, runner: Runner, code: String) {
    exchange_code(app, runner, code).await;
    login_in_flight(runner).store(false, Ordering::SeqCst);
}

async fn exchange_code(app: AppHandle, runner: Runner, code: String) {
    let state = app.state::<AppState>();
    // The Store tab must hold the shared browser profile before the login
    // window lets go of it (see storeview::ensure_created).
    let language = state.settings.read().await.language.clone();
    storeview::ensure_created(&app, runner, &language);
    if let Some(w) = app.get_webview_window(&login_window_label(runner)) {
        let _ = w.close();
    }
    // under gamescope (Steam Deck) nothing else hands focus back
    focus_main(&app);
    match state.stores.get(runner).login(&code).await {
        Ok(_) => after_login(&app, runner).await,
        Err(e) => {
            tracing::error!(?runner, error = %e, "login failed");
            let _ = app.emit(
                STORE_LOGIN_ERROR,
                LoginError {
                    runner,
                    message: e.to_string(),
                },
            );
        }
    }
}

async fn after_login(app: &AppHandle, runner: Runner) {
    let state = app.state::<AppState>();
    if let Err(e) = super::library::refresh_store(app, runner).await {
        tracing::warn!(error = %e, "library refresh after login failed");
    }
    let _ = app.emit(LIBRARY_UPDATED, serde_json::json!({ "runner": runner }));
    let language = state.settings.read().await.language.clone();
    storeview::after_login(app, runner, &language);
}
