//! GOG web APIs used next to gogdl (port of the relevant parts of Heroic's
//! `gog/library.ts`):
//!
//! - `galaxy-library.gog.com` — the games the user owns (needs the token),
//! - `gamesdb.gog.com` — title, art, description, genres, screenshots per
//!   game (public, cached on disk: one request per game ever).
//!
//! Builds and update checks go through `native::Client`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::error::{Error, Result};
use crate::paths;
use crate::types::{GameExtra, GameInfo, Runner};

/// The Galaxy client's token and the account id (`native::auth`).
#[derive(Clone)]
pub struct Credentials {
    pub access_token: String,
    pub user_id: String,
}

// never print the token, even in a debug log
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("user_id", &self.user_id)
            .field("access_token", &"<redacted>")
            .finish()
    }
}

/// Galaxy's public OAuth client secret (the one gogdl and Heroic use).
pub(crate) const CLIENT_SECRET: &str =
    "9d85c43b1482497dbbce61f6e4aa173a433796eeae2ca8c5f6129f2dc4de46d9";

/// Authorization code from the login redirect → token response
/// (`access_token`, `refresh_token`, `expires_in`, `user_id`…).
pub async fn exchange_code(http: &reqwest::Client, code: &str) -> Result<Value> {
    let url = reqwest::Url::parse_with_params(
        "https://auth.gog.com/token",
        &[
            ("client_id", super::CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("grant_type", "authorization_code"),
            ("code", code),
            (
                "redirect_uri",
                "https://embed.gog.com/on_login_success?origin=client",
            ),
        ],
    )
    .map_err(|e| Error::Other(e.to_string()))?;
    // the error must not carry the URL: it holds the login code
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Other(format!("GOG login: {}", e.without_url())))?;
    let status = resp.status();
    let json: Value = resp
        .json()
        .await
        .map_err(|_| Error::Other(format!("GOG login: HTTP {status}")))?;
    if let Some(err) = json.get("error") {
        let msg = json
            .get("error_description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Err(Error::Helper {
            helper: "GOG".into(),
            message: format!("{} {msg}", err.as_str().unwrap_or("error"))
                .trim()
                .into(),
        });
    }
    if !status.is_success()
        || json.get("access_token").is_none()
        || json.get("refresh_token").is_none()
    {
        return Err(Error::Other(format!("GOG login: HTTP {status}")));
    }
    Ok(json)
}

/// One owned product from the Galaxy library.
#[derive(Debug, Clone, Deserialize)]
pub struct LibraryEntry {
    pub platform_id: String,
    pub external_id: String,
    #[serde(default)]
    pub certificate: Option<String>,
}

#[derive(Deserialize)]
struct LibraryPage {
    #[serde(default)]
    items: Vec<LibraryEntry>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// All releases the user owns on GOG itself (other platforms linked to the
/// Galaxy account are skipped).
pub async fn owned(http: &reqwest::Client, creds: &Credentials) -> Result<Vec<LibraryEntry>> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut url = reqwest::Url::parse(&format!(
            "https://galaxy-library.gog.com/users/{}/releases",
            creds.user_id
        ))
        .map_err(|e| Error::Other(format!("GOG library: {e}")))?;
        if let Some(t) = &token {
            url.query_pairs_mut().append_pair("page_token", t);
        }
        let resp = http
            .get(url)
            .bearer_auth(&creds.access_token)
            .send()
            .await
            .map_err(|e| Error::Other(format!("GOG library: {}", e.without_url())))?;
        if !resp.status().is_success() {
            return Err(Error::Other(format!("GOG library: HTTP {}", resp.status())));
        }
        let page: LibraryPage = resp
            .json()
            .await
            .map_err(|e| Error::Other(format!("GOG library: {}", e.without_url())))?;
        out.extend(page.items.into_iter().filter(|e| e.platform_id == "gog"));
        match page.next_page_token.filter(|t| !t.is_empty()) {
            Some(t) => token = Some(t),
            None => break,
        }
    }
    Ok(out)
}

const GAMESDB_TTL: Duration = Duration::from_secs(14 * 24 * 3600);

fn gamesdb_cache(id: &str) -> PathBuf {
    let safe: String = id.chars().filter(char::is_ascii_digit).collect();
    paths::cache_dir()
        .join("gog-gamesdb")
        .join(format!("{safe}.json"))
}

/// Cached gamesdb document, whatever its age.
pub fn gamesdb_cached(id: &str) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(gamesdb_cache(id)).ok()?).ok()
}

fn gamesdb_fresh(id: &str) -> Option<Value> {
    let meta = std::fs::metadata(gamesdb_cache(id)).ok()?;
    let age = SystemTime::now()
        .duration_since(meta.modified().ok()?)
        .unwrap_or_default();
    if age > GAMESDB_TTL {
        return None;
    }
    gamesdb_cached(id)
}

/// `gamesdb.gog.com/platforms/gog/external_releases/<id>`, from the disk
/// cache while it is fresh. `Ok(None)` for unknown ids.
pub async fn gamesdb(
    http: &reqwest::Client,
    entry: &LibraryEntry,
    token: &str,
) -> Result<Option<Value>> {
    if let Some(v) = gamesdb_fresh(&entry.external_id) {
        return Ok(Some(v));
    }
    let mut req = http
        .get(format!(
            "https://gamesdb.gog.com/platforms/gog/external_releases/{}",
            entry.external_id
        ))
        .bearer_auth(token);
    if let Some(cert) = &entry.certificate {
        req = req.header("X-GOG-Library-Cert", cert);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| Error::Other(format!("GOG gamesdb: {}", e.without_url())))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        // keep using a stale copy rather than losing the game
        return Ok(gamesdb_cached(&entry.external_id));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| Error::Other(format!("GOG gamesdb: {}", e.without_url())))?;
    let path = gamesdb_cache(&entry.external_id);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, v.to_string()) {
        tracing::debug!(error = %e, "could not cache GOG gamesdb entry");
    }
    Ok(Some(v))
}

/// gamesdb documents for all entries, a few requests at a time.
pub async fn gamesdb_all(
    http: &reqwest::Client,
    entries: Vec<LibraryEntry>,
    token: &str,
) -> Vec<Value> {
    let limit = Arc::new(Semaphore::new(8));
    let mut set = tokio::task::JoinSet::new();
    for (i, entry) in entries.into_iter().enumerate() {
        let (http, token, limit) = (http.clone(), token.to_string(), limit.clone());
        set.spawn(async move {
            let _permit = limit.acquire().await.ok();
            for attempt in 0..3 {
                match gamesdb(&http, &entry, &token).await {
                    Ok(v) => return (i, v),
                    Err(e) if attempt == 2 => {
                        tracing::warn!(id = %entry.external_id, error = %e, "GOG gamesdb failed");
                    }
                    Err(_) => tokio::time::sleep(Duration::from_secs(2)).await,
                }
            }
            (i, gamesdb_cached(&entry.external_id))
        });
    }
    let mut out: Vec<(usize, Value)> = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok((i, Some(v))) = res {
            out.push((i, v));
        }
    }
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, v)| v).collect()
}

/// gamesdb image templates look like `https://…/<hash>{formatter}.{ext}?…`;
/// an empty formatter is the original size.
fn image(v: &Value, pointer: &str, ext: &str) -> Option<String> {
    image_as(v, pointer, "", ext)
}

fn image_as(v: &Value, pointer: &str, formatter: &str, ext: &str) -> Option<String> {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .map(|f| f.replace("{formatter}", formatter).replace("{ext}", ext))
}

/// Pick the best text of a `{"*": …, "en-US": …, …}` map for `lang` ("uk", "en").
fn localized(map: Option<&Value>, lang: &str) -> Option<String> {
    let obj = map?.as_object()?;
    let pick = obj
        .iter()
        .find(|(k, _)| k.split('-').next() == Some(lang))
        .map(|(_, v)| v)
        .or_else(|| obj.get("en-US"))
        .or_else(|| obj.get("*"));
    pick.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn names(v: &Value, pointer: &str) -> Vec<String> {
    v.pointer(pointer)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("name"))
                .filter_map(|n| {
                    n.as_str()
                        .map(str::to_owned)
                        .or_else(|| localized(Some(n), "en"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Library entry for a gamesdb document; `None` for DLC, hidden products and
/// anything that is not a game (like Heroic's `gogToUnifiedInfo`).
pub fn game_info(v: &Value) -> Option<GameInfo> {
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    if !matches!(kind, "game" | "mod") {
        return None;
    }
    if v.pointer("/game/visible_in_library")
        .and_then(Value::as_bool)
        == Some(false)
    {
        return None;
    }
    let app_name = v.get("external_id").and_then(Value::as_str)?.to_string();
    let title = localized(v.get("title"), "en")
        .or_else(|| localized(v.pointer("/game/title"), "en"))
        .unwrap_or_else(|| app_name.clone());
    let background = image(v, "/game/background/url_format", "jpg");
    let logo = image(v, "/game/logo/url_format", "jpg");
    let cover = logo
        .clone()
        .or_else(|| background.clone())
        .unwrap_or_default();
    let developers = names(v, "/game/developers");
    let slug = v.pointer("/game/slug").and_then(Value::as_str);
    Some(GameInfo {
        runner: Some(Runner::Gog),
        app_name,
        title,
        // Galaxy's own tile size (342×482); the original can be tiny
        art_square: image_as(
            v,
            "/game/vertical_cover/url_format",
            "_glx_vertical_cover",
            "jpg",
        )
        .unwrap_or_else(|| cover.clone()),
        art_cover: cover,
        art_logo: None,
        art_background: background,
        art_icon: image(v, "/game/square_icon/url_format", "jpg")
            .or_else(|| image(v, "/game/icon/url_format", "jpg")),
        developer: (!developers.is_empty()).then(|| developers.join(", ")),
        description: localized(v.get("summary"), "en"),
        store_url: slug.map(|s| format!("https://www.gog.com/en/game/{}", s.replace('-', "_"))),
        folder_name: None,
        is_installed: false,
        installable: true,
        is_dlc: false,
        can_run_offline: true,
        cloud_save_enabled: false,
        save_folder: None,
        version: None,
        third_party_managed_app: None,
        update_available: false,
        install: None,
    })
}

/// Store-page details from the cached gamesdb document.
pub fn extra(v: &Value, lang: &str) -> GameExtra {
    let screenshots = v
        .pointer("/game/screenshots")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.get("url_format").and_then(Value::as_str))
                .map(|f| f.replace("{formatter}", "").replace("{ext}", "jpg"))
                .collect()
        })
        .unwrap_or_default();
    let publishers = names(v, "/game/publishers");
    let developers = names(v, "/game/developers");
    GameExtra {
        description: localized(v.get("summary"), lang)
            .or_else(|| localized(v.pointer("/game/summary"), lang)),
        developer: (!developers.is_empty()).then(|| developers.join(", ")),
        publisher: (!publishers.is_empty()).then(|| publishers.join(", ")),
        release_date: v
            .get("first_release_date")
            .or_else(|| v.pointer("/game/first_release_date"))
            .and_then(Value::as_str)
            .map(|d| d.chars().take(10).collect()),
        tags: v
            .pointer("/game/genres")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|g| localized(g.get("name"), lang))
                    .collect()
            })
            .unwrap_or_default(),
        screenshots,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn witcher() -> Value {
        serde_json::json!({
            "type": "game",
            "external_id": "1207658924",
            "title": { "*": "The Witcher: Enhanced Edition", "en-US": "The Witcher: Enhanced Edition" },
            "summary": { "*": "Short", "en-US": "Long English", "uk-UA": "Українською" },
            "first_release_date": "2007-10-26T00:00:00+0000",
            "game": {
                "visible_in_library": true,
                "slug": "the-witcher",
                "logo": { "url_format": "https://images.gog.com/logo{formatter}.{ext}?namespace=gamesdb" },
                "vertical_cover": { "url_format": "https://images.gog.com/vert{formatter}.{ext}?namespace=gamesdb" },
                "background": { "url_format": "https://images.gog.com/bg{formatter}.{ext}?namespace=gamesdb" },
                "developers": [{ "name": "CD Projekt RED" }],
                "publishers": [{ "name": "Atari" }, { "name": "CD Projekt" }],
                "genres": [{ "name": { "*": "Role-playing (RPG)", "en-US": "Role-playing (RPG)" } }],
                "screenshots": [{ "url_format": "https://images.gog.com/s1{formatter}.{ext}?namespace=gamesdb" }]
            }
        })
    }

    #[test]
    fn maps_gamesdb_to_library_entry() {
        let g = game_info(&witcher()).unwrap();
        assert_eq!(g.app_name, "1207658924");
        assert_eq!(g.title, "The Witcher: Enhanced Edition");
        assert_eq!(
            g.art_square,
            "https://images.gog.com/vert_glx_vertical_cover.jpg?namespace=gamesdb"
        );
        assert_eq!(
            g.art_cover,
            "https://images.gog.com/logo.jpg?namespace=gamesdb"
        );
        assert_eq!(g.developer.as_deref(), Some("CD Projekt RED"));
        assert_eq!(g.description.as_deref(), Some("Long English"));
        assert_eq!(
            g.store_url.as_deref(),
            Some("https://www.gog.com/en/game/the_witcher")
        );
    }

    #[test]
    fn skips_dlc_and_hidden_products() {
        let mut dlc = witcher();
        dlc["type"] = "dlc".into();
        assert!(game_info(&dlc).is_none());
        let mut hidden = witcher();
        hidden["game"]["visible_in_library"] = false.into();
        assert!(game_info(&hidden).is_none());
    }

    #[test]
    fn extra_prefers_the_ui_language() {
        let e = extra(&witcher(), "uk");
        assert_eq!(e.description.as_deref(), Some("Українською"));
        assert_eq!(e.publisher.as_deref(), Some("Atari, CD Projekt"));
        assert_eq!(e.release_date.as_deref(), Some("2007-10-26"));
        assert_eq!(e.tags, ["Role-playing (RPG)"]);
        assert_eq!(
            e.screenshots,
            ["https://images.gog.com/s1.jpg?namespace=gamesdb"]
        );
    }
}
