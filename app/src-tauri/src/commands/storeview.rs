//! The Store tab: the real Epic / GOG web stores embedded as child webviews
//! of the main window, positioned over the area the Svelte UI reserves for
//! them. Each store webview shares its browser profile (`data_directory`)
//! with that store's login window, so logging in from the sidebar also logs
//! the embedded store page in.

use hyper_core::error::{Error, Result};
use hyper_core::stores::{gog, legendary};
use hyper_core::types::Runner;
use hyper_core::{paths, platform};
use serde::{Deserialize, Serialize};
use tauri::webview::{
    DownloadEvent, NewWindowFeatures, NewWindowResponse, PageLoadEvent, Webview, WebviewBuilder,
};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, State, Url, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_opener::OpenerExt;

use crate::MAIN_WINDOW;
use crate::state::AppState;

/// Stores whose page should open on their sign-in page next time it is
/// created (a launcher login happened before the Store tab was opened).
static PENDING_SIGNIN: std::sync::Mutex<Vec<Runner>> = std::sync::Mutex::new(Vec::new());

/// Same user agent Heroic uses for store pages (and the GOG login window).
pub(crate) const STORE_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/200.0";

/// Injected into every store page: mouse back/forward buttons, plus a
/// gamepad-driven virtual cursor (port of the original project's
/// webviewPreload). The host drives it through `store_webview_command`.
const INIT_SCRIPT: &str = r#"
(() => {
  document.addEventListener('mouseup', (ev) => {
    if (ev.button === 3) { history.back(); ev.preventDefault(); }
    else if (ev.button === 4) { history.forward(); ev.preventDefault(); }
  });

  const ATTR = 'data-hyper-cursor';
  const SEL = 'a[href], button:not([disabled]), input:not([type=\"hidden\"]):not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex=\"-1\"]), [role=\"button\"], [role=\"link\"]';
  let cursor = null;
  const style = () => {
    if (document.querySelector('[data-hyper-style]')) return;
    const st = document.createElement('style');
    st.setAttribute('data-hyper-style', '');
    st.textContent = '[' + ATTR + '] { outline: 3px solid #ffae00 !important; outline-offset: 2px !important; }';
    document.documentElement.appendChild(st);
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', style, { once: true }); else style();

  const visible = (el) => {
    if (!el.offsetParent && el.tagName !== 'BODY' && getComputedStyle(el).position !== 'fixed') return false;
    const r = el.getBoundingClientRect();
    if (!r.width || !r.height) return false;
    return r.bottom > -200 && r.top < innerHeight + 200 && r.right > -200 && r.left < innerWidth + 200;
  };
  const focusables = () => Array.from(document.querySelectorAll(SEL)).filter(visible);
  const current = () => (cursor && cursor.isConnected && visible(cursor)) ? cursor : null;
  const setCursor = (el) => {
    if (cursor && cursor !== el) cursor.removeAttribute(ATTR);
    cursor = el;
    if (el) { el.setAttribute(ATTR, ''); el.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'smooth' }); }
  };
  const initial = () => {
    const list = focusables();
    list.sort((a, b) => { const ra = a.getBoundingClientRect(), rb = b.getBoundingClientRect(); return ra.top - rb.top || ra.left - rb.left; });
    return list.find((el) => el.getBoundingClientRect().top >= 0) || list[0] || null;
  };
  const navigate = (dir) => {
    const cur = current() || initial();
    if (!cur) return false;
    if (cur !== cursor) { setCursor(cur); return true; }
    const cr = cur.getBoundingClientRect(); const cx = cr.left + cr.width / 2, cy = cr.top + cr.height / 2;
    let best = null, bestScore = Infinity;
    for (const cand of focusables()) {
      if (cand === cur) continue;
      const r = cand.getBoundingClientRect(); const dx = r.left + r.width / 2 - cx, dy = r.top + r.height / 2 - cy;
      let ok = false, perp = 0;
      if (dir === 'right') { ok = dx > 8 && Math.abs(dy) <= Math.abs(dx) * 1.5; perp = Math.abs(dy); }
      else if (dir === 'left') { ok = dx < -8 && Math.abs(dy) <= Math.abs(dx) * 1.5; perp = Math.abs(dy); }
      else if (dir === 'down') { ok = dy > 8 && Math.abs(dx) <= Math.abs(dy) * 1.5; perp = Math.abs(dx); }
      else { ok = dy < -8 && Math.abs(dx) <= Math.abs(dy) * 1.5; perp = Math.abs(dx); }
      if (!ok) continue;
      const score = Math.hypot(dx, dy) + perp * 2;
      if (score < bestScore) { bestScore = score; best = cand; }
    }
    if (best) { setCursor(best); return true; }
    if (dir === 'down' || dir === 'up') { scrollBy({ top: dir === 'down' ? 400 : -400, behavior: 'smooth' }); return true; }
    return false;
  };
  const activate = () => {
    const el = current(); if (!el) return;
    const tag = el.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || el.isContentEditable) el.focus(); else if (typeof el.click === 'function') el.click();
  };
  const focusSearch = () => {
    for (const el of document.querySelectorAll('input[type=\"search\"], input[type=\"text\"], input[role=\"searchbox\"], [role=\"search\"] input')) {
      if (visible(el)) { setCursor(el); el.focus(); return true; }
    }
    return false;
  };
  window.__hyper = (cmd) => {
    if (!cmd || typeof cmd !== 'object') return;
    switch (cmd.type) {
      case 'enter': setCursor(current() || initial()); break;
      case 'exit': if (cursor) cursor.removeAttribute(ATTR); cursor = null; break;
      case 'click': activate(); break;
      case 'goBack': if (history.length > 1) history.back(); break;
      case 'goForward': history.forward(); break;
      case 'focusSearch': focusSearch(); break;
      case 'navigate': navigate(cmd.direction); break;
      case 'scroll': scrollBy({ top: cmd.direction === 'down' ? 60 : -60 }); break;
    }
  };

  // The app's scrollbar (app.css), in the colours of the current theme.
  // Chromium ignores ::-webkit-scrollbar where a site set scrollbar-color /
  // scrollbar-width, so those are reset on the page itself only; inner
  // scrollers a site hides on purpose (carousels) stay hidden.
  // A constructed sheet: a <style> element could be refused by the site's CSP.
  window.__hyperScrollbar = (thumb, hover) => {
    const sheet = window.__hyperSheet || (window.__hyperSheet = new CSSStyleSheet());
    sheet.replaceSync(
      'html, body { scrollbar-width: auto !important; scrollbar-color: auto !important; }' +
      '::-webkit-scrollbar { width: 6px !important; height: 6px !important; }' +
      '::-webkit-scrollbar-thumb { background: ' + thumb + ' !important; border-radius: 3px !important; }' +
      '::-webkit-scrollbar-thumb:hover { background: ' + hover + ' !important; }' +
      '::-webkit-scrollbar-track, ::-webkit-scrollbar-corner { background: transparent !important; }' +
      '::-webkit-scrollbar-button { display: none !important; }'
    );
    if (!document.adoptedStyleSheets.includes(sheet)) {
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
    }
  };
})();
"#;

/// Scrollbar colours (thumb, thumb on hover) of the app's current theme,
/// applied to every store page; `None` until the UI sent them.
static SCROLLBAR: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);

/// Script applying the stored scrollbar colours (empty when none yet).
fn scrollbar_script() -> String {
    let colors = SCROLLBAR.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match colors {
        Some((thumb, hover)) => format!(
            "window.__hyperScrollbar && window.__hyperScrollbar({}, {});",
            serde_json::Value::from(thumb),
            serde_json::Value::from(hover)
        ),
        None => String::new(),
    }
}

/// A CSS colour as the theme produces it (`#rrggbb`, `rgb(…)`); anything
/// else is refused so nothing but a colour reaches the page's stylesheet.
fn is_css_color(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "#(),.% ".contains(c))
}

/// The app's theme changed (or the Store tab opened): use its scrollbar
/// colours on the store pages, the open ones included.
#[tauri::command]
pub async fn store_webview_scrollbar(app: AppHandle, thumb: String, hover: String) -> Result<()> {
    let (thumb, hover) = (thumb.trim().to_string(), hover.trim().to_string());
    if !is_css_color(&thumb) || !is_css_color(&hover) {
        return Err(Error::Other("invalid scrollbar colour".into()));
    }
    *SCROLLBAR.lock().unwrap_or_else(|e| e.into_inner()) = Some((thumb, hover));
    let script = scrollbar_script();
    for runner in Runner::stores() {
        if let Some(w) = existing(&app, runner) {
            let _ = w.eval(&script);
        }
    }
    Ok(())
}

/// Parts of query parameter names whose values never go to the log: secrets
/// (`exchange_code`, `authCode`, `session_id`, `client_secret`…), nested
/// URLs (`redirect_uri`, `returnUrl`, `continue`, `state`…, which carry the
/// outer flow's parameters) and account data (`email`, `login_hint`…).
/// Matched as substrings, so a store renaming its parameter does not leak
/// it into the log.
const SECRET_PARAMS: [&str; 26] = [
    "code", "token", "secret", "session", "sid", "ticket", "password", "otp", "auth", "redirect",
    "return", "url", "uri", "state", "continue", "next", "key", "sig", "hash", "nonce", "jwt",
    "pwd", "email", "login", "user", "phone",
];

/// Steps of a sign-in flow (login / SSO pages, success redirects), worth
/// logging to see how a store hands its session around.
fn is_auth_step(url: &Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    let path = url.path().to_ascii_lowercase();
    host.starts_with("login.")
        || host.starts_with("auth.")
        || path.contains("login")
        || path.contains("/sso")
        || path.starts_with("/id/")
        || path.starts_with("/auth/")
}

/// Longest path segment of a sign-in step that is still a route name and
/// not a token / id embedded in the path.
const MAX_PATH_SEGMENT: usize = 24;

/// A sign-in step for the log: scheme, host and path only (no query, which
/// carries codes and nested URLs); a path segment longer than
/// [`MAX_PATH_SEGMENT`] is a token or an id and becomes `***`.
fn auth_step_summary(url: &Url) -> String {
    let path: Vec<&str> = url
        .path()
        .split('/')
        .map(|seg| {
            if seg.chars().count() > MAX_PATH_SEGMENT {
                "***"
            } else {
                seg
            }
        })
        .collect();
    format!(
        "{}://{}{}",
        url.scheme(),
        url.host_str().unwrap_or_default(),
        path.join("/")
    )
}

/// The URL with secret parameter values replaced by `***`.
pub(crate) fn redact(url: &Url) -> String {
    let mut u = url.clone();
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| {
            let lower = k.to_ascii_lowercase();
            let secret = SECRET_PARAMS.iter().any(|s| lower.contains(s));
            (
                k.into_owned(),
                if secret {
                    "***".to_string()
                } else {
                    v.into_owned()
                },
            )
        })
        .collect();
    if pairs.is_empty() {
        u.set_query(None);
    } else {
        u.query_pairs_mut().clear().extend_pairs(pairs);
    }
    u.set_fragment(None);
    u.to_string()
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub fn label(runner: Runner) -> String {
    format!("store-{}", runner.id())
}

fn store_lang(language: &str, runner: Runner) -> &'static str {
    match (runner, language) {
        (Runner::Legendary, "uk") => "uk",
        (Runner::Legendary, _) => "en-US",
        // GOG has no Ukrainian storefront yet
        (Runner::Gog | Runner::Sideload, _) => "en",
    }
}

/// The store's sign-in page, where the web session can pick up a launcher
/// login. Only Epic has one we can send the user to.
fn signin_url(runner: Runner, language: &str) -> Option<String> {
    match runner {
        Runner::Legendary => {
            Some(legendary::STORE_SIGNIN_URL.replace("{lang}", store_lang(language, runner)))
        }
        Runner::Gog | Runner::Sideload => None,
    }
}

pub fn home_url(runner: Runner, language: &str) -> Result<String> {
    let lang = store_lang(language, runner);
    match runner {
        Runner::Legendary => Ok(legendary::STORE_URL.replace("{lang}", lang)),
        Runner::Gog => Ok(gog::STORE_URL.replace("{lang}", lang)),
        Runner::Sideload => Err(Error::Other("sideloaded games have no store page".into())),
    }
}

pub(crate) enum Page {
    Store,
    /// a web page, but not the stores or the sign-in / payment pages they use
    Elsewhere,
    /// not a web page (`file:`, `javascript:`, protocol handlers)
    Refused,
}

/// Sites a store page may show in place: the stores themselves and what
/// their sign-in and checkout go through (the sign-in hosts only, not the
/// whole of Google / Microsoft / Apple). A host matches itself and its
/// subdomains. Anything else on https opens in the user's browser.
const STORE_SITES: [&str; 21] = [
    "epicgames.com",
    "epicgames.dev",
    "unrealengine.com",
    "fab.com",
    "gog.com",
    "gogalaxy.com",
    "gog-statics.com",
    "accounts.google.com",
    "www.facebook.com",
    "appleid.apple.com",
    "login.live.com",
    "login.microsoftonline.com",
    "login.xbox.com",
    "playstation.com",
    "sonyentertainmentnetwork.com",
    "steampowered.com",
    "nintendo.com",
    "paypal.com",
    "paysafecard.com",
    "hcaptcha.com",
    "arkoselabs.com",
];

/// Like [`STORE_SITES`], limited to one path prefix of the host.
const STORE_PATHS: [(&str, &str); 1] = [("steamcommunity.com", "/openid")];

/// Whether `url` is one of the stores' own pages (see [`STORE_SITES`]):
/// https on a listed host. Blank / inline pages count as the store's own.
pub(crate) fn site_policy(url: &Url) -> Page {
    match url.scheme() {
        "https" | "http" => {}
        // blank pages, inline and script-made frames
        "about" | "data" | "blob" => return Page::Store,
        _ => return Page::Refused,
    }
    if url.scheme() != "https" {
        return Page::Elsewhere;
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let suffix = |d: &str| host == d || host.ends_with(&format!(".{d}"));
    if STORE_SITES.iter().any(|d| suffix(d))
        || STORE_PATHS
            .iter()
            .any(|(d, p)| suffix(d) && url.path().starts_with(p))
    {
        Page::Store
    } else {
        Page::Elsewhere
    }
}

/// The navigation policy. WebView2 asks only about the page itself;
/// WebKitGTK also about every frame inside it (captchas, trailers, payment
/// forms), which must load in place. There only the scheme is checked here
/// and the page itself once it is committed (`leave_off_store` from
/// `on_page_load`).
pub(crate) fn page_policy(url: &Url) -> Page {
    if !cfg!(windows) {
        return match site_policy(url) {
            Page::Refused => Page::Refused,
            Page::Store | Page::Elsewhere => Page::Store,
        };
    }
    site_policy(url)
}

/// Window label prefix of store popups (`store-popup-<store>-<n>`); their
/// size and place are not remembered (`lib.rs`, window-state filter).
pub const POPUP_PREFIX: &str = "store-popup-";

/// Popups opened so far (each gets its own window label).
static POPUPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// A popup asked by a store page (or by one of its popups): only for the
/// stores' own sites (`STORE_SITES`), and in a window of ours rather than
/// WebView2's default one, so the same rules keep applying after it opened:
/// [`page_policy`] for every navigation (off-store links go to the
/// browser), popups of its own only through here, no downloads, no IPC (the
/// capabilities name the `main` webview only). It shares the opener's
/// browser environment (`window_features`: the same WebView2 environment /
/// related WebKitGTK view), so what it signs in sticks to the store page.
fn popup(
    app: &AppHandle,
    runner: Runner,
    url: &Url,
    features: NewWindowFeatures,
) -> NewWindowResponse<tauri::Wry> {
    if !matches!(site_policy(url), Page::Store) {
        tracing::warn!(?runner, url = %redact(url), "store popup refused");
        return NewWindowResponse::Deny;
    }
    tracing::info!(?runner, url = %redact(url), "store popup");
    let Ok(blank) = Url::parse("about:blank") else {
        return NewWindowResponse::Deny;
    };
    let n = POPUPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (nav_app, popup_app) = (app.clone(), app.clone());
    let mut builder = WebviewWindowBuilder::new(
        app,
        format!("{POPUP_PREFIX}{}-{n}", runner.id()),
        // WebView2 navigates the new webview to `url` itself once it is
        // handed over; it must not have loaded anything else before
        WebviewUrl::External(blank),
    )
    .title(url.host_str().unwrap_or("Store"))
    .inner_size(600.0, 720.0)
    .focused(true)
    .devtools(crate::devtools_enabled())
    .user_agent(STORE_USER_AGENT)
    .on_document_title_changed(|w, title| {
        let _ = w.set_title(&title);
    })
    .on_page_load(move |w, payload| {
        if !cfg!(windows) && matches!(payload.event(), PageLoadEvent::Started) {
            leave_off_store(w.as_ref(), runner, payload.url());
        }
    })
    .on_navigation(move |url| match page_policy(url) {
        Page::Store => true,
        Page::Elsewhere => {
            tracing::info!(?runner, url = %redact(url), "opening off-store link from a popup in the browser");
            open_in_browser(&nav_app, url);
            false
        }
        Page::Refused => {
            tracing::warn!(?runner, scheme = url.scheme(), "store popup navigation refused");
            false
        }
    })
    .on_new_window(move |url, features| popup(&popup_app, runner, &url, features))
    .on_download(|_, event| refuse_download(event));
    if features.position().is_none() {
        builder = builder.center();
    }
    match builder.window_features(features).build() {
        Ok(window) => NewWindowResponse::Create { window },
        Err(e) => {
            tracing::warn!(?runner, error = %e, "could not open the store popup");
            NewWindowResponse::Deny
        }
    }
}

/// WebKitGTK: the committed document is checked where `on_navigation`
/// could not tell it from a frame. An off-store page goes back (or blank)
/// and opens in the browser; plain http and other schemes just go back.
fn leave_off_store(webview: &Webview, runner: Runner, url: &Url) {
    const BACK: &str =
        "if (history.length > 1) history.back(); else location.replace('about:blank');";
    match site_policy(url) {
        Page::Store => {}
        Page::Elsewhere if url.scheme() == "https" => {
            tracing::info!(?runner, url = %redact(url), "off-store page left; opening it in the browser");
            let _ = webview.eval(BACK);
            open_in_browser(webview.app_handle(), url);
        }
        Page::Elsewhere | Page::Refused => {
            tracing::warn!(?runner, scheme = url.scheme(), "off-store page left");
            let _ = webview.eval(BACK);
        }
    }
}

/// Open `url` in the user's browser, off the webview's callback thread
/// (the opener blocks on the shell).
pub(crate) fn open_in_browser(app: &AppHandle, url: &Url) {
    let app = app.clone();
    let url = url.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = app.opener().open_url(&url, None::<&str>) {
            tracing::warn!(error = %e, "could not open the browser");
        }
    });
}

/// A file download asked by a store page: refused, the launcher is no
/// browser (installers and the like belong in the user's own browser).
pub(crate) fn refuse_download(event: DownloadEvent<'_>) -> bool {
    if let DownloadEvent::Requested { url, .. } = event {
        tracing::warn!(url = %redact(&url), "store download refused");
    }
    false
}

fn apply_bounds(webview: &Webview, b: Bounds) -> Result<()> {
    // wry ignores the bounds of a GTK child webview; placed by hand there
    #[cfg(target_os = "linux")]
    crate::gtk_layout::place(webview.app_handle(), webview.label().to_string(), b);
    webview
        .set_position(LogicalPosition::new(b.x, b.y))
        .and_then(|_| webview.set_size(LogicalSize::new(b.width.max(1.0), b.height.max(1.0))))
        .map_err(|e| Error::Other(e.to_string()))
}

fn existing(app: &AppHandle, runner: Runner) -> Option<Webview> {
    app.get_webview(&label(runner))
}

/// Create (first time) or re-show the store webview and move it to `bounds`.
/// Returns the current page URL.
#[tauri::command]
pub async fn store_webview_show(
    app: AppHandle,
    state: State<'_, AppState>,
    runner: Runner,
    bounds: Bounds,
) -> Result<String> {
    tracing::debug!(?runner, ?bounds, "store webview show");
    for other in Runner::stores() {
        if other != runner
            && let Some(w) = existing(&app, other)
        {
            let _ = w.hide();
        }
    }
    let webview = match existing(&app, runner) {
        Some(w) => {
            apply_bounds(&w, bounds)?;
            w.show().map_err(|e| Error::Other(e.to_string()))?;
            w
        }
        None => {
            let language = state.settings.read().await.language.clone();
            create(&app, runner, &language, bounds)?
        }
    };
    // A newly shown WebView2 grabs keyboard focus. Chromium only delivers
    // Gamepad API input to the focused document, so hand focus back to the
    // app UI; the controller then drives the store through
    // `store_webview_command`.
    refocus_ui(&app);
    Ok(webview.url().map(|u| u.to_string()).unwrap_or_default())
}

/// Build the store webview (on its sign-in page when a launcher login asked
/// for it, else on the store's home page) at `bounds`.
fn create(app: &AppHandle, runner: Runner, language: &str, bounds: Bounds) -> Result<Webview> {
    let pending = take_pending_signin(runner);
    let start = match pending.then(|| signin_url(runner, language)).flatten() {
        Some(u) => u,
        None => home_url(runner, language)?,
    };
    let url = Url::parse(&start).map_err(|e| Error::Other(e.to_string()))?;
    let window = app
        .get_window(MAIN_WINDOW)
        .ok_or_else(|| Error::Other("main window missing".into()))?;
    let nav_app = app.clone();
    let popup_app = app.clone();
    let builder = WebviewBuilder::new(label(runner), WebviewUrl::External(url))
        .devtools(crate::devtools_enabled())
        .user_agent(STORE_USER_AGENT)
        .data_directory(paths::webview_data_dir(runner.id()))
        // the colours known now apply from the first paint of every page;
        // later theme changes arrive through `on_page_load`
        .initialization_script(format!("{INIT_SCRIPT}\n{}", scrollbar_script()))
        .on_page_load(move |w, payload| match payload.event() {
            PageLoadEvent::Started => {
                if !cfg!(windows) {
                    leave_off_store(&w, runner, payload.url());
                }
            }
            PageLoadEvent::Finished => {
                let _ = w.eval(scrollbar_script());
            }
        })
        .on_navigation(move |url| {
            if is_auth_step(url) {
                tracing::info!(?runner, step = %auth_step_summary(url), "store sign-in step");
            }
            match page_policy(url) {
                Page::Store => true,
                Page::Elsewhere => {
                    // a link off the store: the user's browser, not a page
                    // dressed up as part of the launcher
                    tracing::info!(?runner, url = %redact(url), "opening off-store link in the browser");
                    open_in_browser(&nav_app, url);
                    false
                }
                Page::Refused => {
                    tracing::warn!(?runner, scheme = url.scheme(), "store page navigation refused");
                    false
                }
            }
        })
        // Sign-in and checkout open popups (social logins, verification,
        // PayPal…). Without a handler wry silently blocks window.open; see
        // `popup` for the window they get.
        .on_new_window(move |url, features| popup(&popup_app, runner, &url, features))
        .on_download(|_, event| refuse_download(event));
    let webview = window
        .add_child(
            builder,
            LogicalPosition::new(bounds.x, bounds.y),
            LogicalSize::new(bounds.width.max(1.0), bounds.height.max(1.0)),
        )
        .map_err(|e| Error::Other(format!("could not open store view: {e}")))?;
    #[cfg(target_os = "linux")]
    crate::gtk_layout::adopt(&webview, bounds);
    Ok(webview)
}

/// Make sure the store's webview exists (created hidden, off-screen, when
/// the Store tab was not opened yet). Called before a finished login window
/// closes: both share one browser profile and thus one WebView2 browser
/// process, and with the login window gone first that process shuts down
/// just as the Store tab opens on it, leaving the tab blank until a restart.
/// Not needed with WebKitGTK, where a hidden child webview is not even
/// hidden (it showed over half of the console UI).
pub fn ensure_created(app: &AppHandle, runner: Runner, language: &str) {
    if !platform::current().webviews_share_browser_process() || existing(app, runner).is_some() {
        return;
    }
    let offscreen = Bounds {
        x: -10.0,
        y: -10.0,
        width: 1.0,
        height: 1.0,
    };
    match create(app, runner, language, offscreen) {
        Ok(w) => {
            let _ = w.hide();
        }
        Err(e) => tracing::warn!(?runner, error = %e, "could not pre-create the store view"),
    }
}

/// Give keyboard focus back to the Svelte UI webview.
pub fn refocus_ui(app: &AppHandle) {
    if let Some(ui) = app.get_webview(MAIN_WINDOW) {
        let _ = ui.set_focus();
    }
}

/// Called by the UI when a controller press arrives while the store page
/// holds focus (e.g. after a mouse click into it).
#[tauri::command]
pub async fn store_webview_release_focus(app: AppHandle) -> Result<()> {
    refocus_ui(&app);
    Ok(())
}

#[tauri::command]
pub async fn store_webview_bounds(app: AppHandle, runner: Runner, bounds: Bounds) -> Result<()> {
    tracing::debug!(?runner, ?bounds, "store webview bounds");
    if let Some(w) = existing(&app, runner) {
        apply_bounds(&w, bounds)?;
    }
    Ok(())
}

/// Hide one store webview, or every one when `runner` is `None` (the UI
/// left the Store tab).
#[tauri::command]
pub async fn store_webview_hide(app: AppHandle, runner: Option<Runner>) -> Result<()> {
    tracing::debug!(?runner, "store webview hide");
    let targets: Vec<Runner> = match runner {
        Some(r) => vec![r],
        None => Runner::stores().to_vec(),
    };
    for r in targets {
        if let Some(w) = existing(&app, r) {
            let _ = w.hide();
        }
    }
    Ok(())
}

/// A gamepad command for the page's cursor script (`window.__hyper` in
/// `INIT_SCRIPT`). Mirrors `StoreWebviewCommand` in `app/src/lib/api/commands.ts`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StoreWebviewCommand {
    Enter,
    Exit,
    Click,
    GoBack,
    GoForward,
    FocusSearch,
    Navigate { direction: Direction },
    Scroll { direction: ScrollDirection },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScrollDirection {
    Up,
    Down,
}

/// Forward a gamepad command to the page's injected cursor script. The
/// command is typed, so only its known shape reaches the page's script.
#[tauri::command]
pub async fn store_webview_command(
    app: AppHandle,
    runner: Runner,
    command: StoreWebviewCommand,
) -> Result<()> {
    if let Some(w) = existing(&app, runner) {
        let json = serde_json::to_string(&command).map_err(|e| Error::Other(e.to_string()))?;
        w.eval(format!("window.__hyper && window.__hyper({json})"))
            .map_err(|e| Error::Other(e.to_string()))?;
    }
    Ok(())
}

fn take_pending_signin(runner: Runner) -> bool {
    let mut list = PENDING_SIGNIN.lock().unwrap_or_else(|e| e.into_inner());
    let had = list.contains(&runner);
    list.retain(|r| *r != runner);
    had
}

/// After a launcher login: bring the embedded store along. Where the store
/// has a sign-in page (Epic) it is opened, now or when the Store tab is
/// first shown, so the web session only needs the account confirmed; other
/// stores just reload to pick up new cookies.
pub fn after_login(app: &AppHandle, runner: Runner, language: &str) {
    let Some(url) = signin_url(runner, language).and_then(|u| tauri::Url::parse(&u).ok()) else {
        if let Some(w) = existing(app, runner) {
            let _ = w.reload();
        }
        return;
    };
    match existing(app, runner) {
        Some(w) => {
            let _ = w.navigate(url);
        }
        None => {
            let mut list = PENDING_SIGNIN.lock().unwrap_or_else(|e| e.into_inner());
            if !list.contains(&runner) {
                list.push(runner);
            }
        }
    }
}

/// Before wiping a browser profile: the webview must not be using it.
pub fn close(app: &AppHandle, runner: Runner) {
    if let Some(w) = existing(app, runner) {
        #[cfg(target_os = "linux")]
        crate::gtk_layout::forget(app, label(runner));
        let _ = w.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_codes_and_keeps_the_rest() {
        let url = Url::parse("https://www.gog.com/on_login_success?origin=client&code=SECRET1&Access_Token=SECRET2#frag")
            .unwrap();
        let out = redact(&url);
        assert!(!out.contains("SECRET"), "{out}");
        assert!(out.contains("origin=client"), "{out}");
        assert!(is_auth_step(&url));
        assert!(!is_auth_step(
            &Url::parse("https://store.epicgames.com/en-US/p/hades").unwrap()
        ));
    }

    #[test]
    fn redacts_nested_urls_and_account_data() {
        let url = Url::parse(
            "https://accounts.google.com/o/oauth2/auth?client_id=app&redirect_uri=https%3A%2F%2Fx%2F%3Fcode%3DSECRET1&state=SECRET2&login_hint=SECRET3%40example.com&continue=SECRET4&scope=openid",
        )
        .unwrap();
        let out = redact(&url);
        assert!(!out.contains("SECRET"), "{out}");
        assert!(out.contains("client_id=app"), "{out}");
        assert!(out.contains("scope=openid"), "{out}");
    }

    #[test]
    fn auth_steps_are_logged_without_query_or_tokens() {
        let url = Url::parse(
            "https://www.epicgames.com/id/login/verify/0123456789abcdef0123456789abcdef?redirectUrl=https%3A%2F%2Fx&email=a%40b.c",
        )
        .unwrap();
        let out = auth_step_summary(&url);
        assert_eq!(out, "https://www.epicgames.com/id/login/verify/***");
        assert_eq!(
            auth_step_summary(&Url::parse("https://login.live.com/").unwrap()),
            "https://login.live.com/"
        );
    }

    #[test]
    fn only_the_sign_in_hosts_count_as_store_sites() {
        let store = |s: &str| matches!(site_policy(&Url::parse(s).unwrap()), Page::Store);
        assert!(store("https://store.epicgames.com/en-US/"));
        assert!(store("https://accounts.google.com/o/oauth2/auth"));
        assert!(store("https://login.microsoftonline.com/common/oauth2"));
        assert!(store("https://steamcommunity.com/openid/login"));
        assert!(store("about:blank"));
        assert!(!store("https://docs.google.com/"));
        assert!(!store("https://www.apple.com/"));
        assert!(!store("https://steamcommunity.com/id/someone"));
        assert!(!store("http://store.epicgames.com/"));
        assert!(matches!(
            site_policy(&Url::parse("file:///C:/x").unwrap()),
            Page::Refused
        ));
    }

    #[test]
    fn webview_commands_serialise_like_the_ui_sends_them() {
        let cmd: StoreWebviewCommand =
            serde_json::from_str(r#"{"type":"navigate","direction":"left"}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&cmd).unwrap(),
            r#"{"type":"navigate","direction":"left"}"#
        );
        let cmd: StoreWebviewCommand = serde_json::from_str(r#"{"type":"goBack"}"#).unwrap();
        assert_eq!(serde_json::to_string(&cmd).unwrap(), r#"{"type":"goBack"}"#);
        assert!(
            serde_json::from_str::<StoreWebviewCommand>(r#"{"type":"eval","code":"1"}"#).is_err()
        );
    }
}
