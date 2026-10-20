//! Epic's launcher API, the calls legendary's `api/egs.py` makes. The
//! session (`user.json`) is read from the vault for every call and refreshed
//! through Epic's OAuth when its access token is about to expire; tokens
//! never touch the disk unencrypted here.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use super::session::Session;
use crate::error::{Error, Result};
use crate::secrets::SecretFile;

/// legendary's (the Epic Games Launcher's) public OAuth client.
const CLIENT_ID: &str = "34a02cf8f4414e29b15921876da36f9a";
const CLIENT_SECRET: &str = "daafbccc737745039dffe53d94fc76cf";

const OAUTH_HOST: &str = "account-public-service-prod03.ol.epicgames.com";
const LAUNCHER_HOST: &str = "launcher-public-service-prod06.ol.epicgames.com";
const CATALOG_HOST: &str = "catalog-public-service-prod06.ol.epicgames.com";
const ECOMMERCE_HOST: &str = "ecommerceintegration-public-service-ecomprod02.ol.epicgames.com";
const LIBRARY_HOST: &str = "library-service.live.use1a.on.epicgames.com";
const DATASTORAGE_HOST: &str = "datastorage-public-service-liveegs.live.use1a.on.epicgames.com";
/// legendary's own API: the current launcher client id and version.
const LEGENDARY_API: &str = "https://api.legendary.gl/v1/version.json";

/// What the Epic Games Launcher sends; the API answers launcher clients only.
const USER_AGENT: &str =
    "UELauncher/11.0.1-14907503+++Portal+Release-Live Windows/10.0.19041.1.256.64bit";

pub const RATE_LIMIT_MESSAGE: &str =
    "Epic Games is rate-limiting requests (HTTP 429). Please wait a minute and try again.";

/// Metadata locale, as legendary's default; the files on disk stay the same
/// whichever implementation wrote them.
const LOCALE: (&str, &str) = ("en", "US");

pub struct Epic {
    http: reqwest::Client,
    user: Arc<SecretFile>,
    /// One token refresh at a time.
    refresh: Mutex<()>,
    pub(crate) config_dir: PathBuf,
}

/// Without the URL: pre-signed CDN / cloud-save links must not reach logs or toasts.
fn net(e: reqwest::Error) -> Error {
    Error::Other(format!("Epic: {}", e.without_url()))
}

impl Epic {
    pub fn new(user: Arc<SecretFile>, config_dir: PathBuf) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            http,
            user,
            refresh: Mutex::new(()),
            config_dir,
        }
    }

    pub fn logged_in(&self) -> bool {
        self.user.exists()
    }

    /// Epic's OAuth client for launchers: legendary's, unless the launcher
    /// config legendary fetched (`version.json` → `egl_config`) names a newer one.
    fn oauth_client(&self) -> (String, String) {
        let cached = super::read_json(&self.config_dir.join("version.json"));
        let egl = cached.as_ref().and_then(|v| v.pointer("/data/egl_config"));
        let field = |k: &str| {
            egl.and_then(|e| e.get(k))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        match (field("client_id"), field("client_secret")) {
            (Some(id), Some(secret)) => (id, secret),
            _ => (CLIENT_ID.into(), CLIENT_SECRET.into()),
        }
    }

    /// Fetch legendary's `version.json` (launcher client / version) when the
    /// cached copy is older than a day, as `legendary` itself does. Best
    /// effort: without it the built-in client is used.
    pub async fn refresh_launcher_config(&self) {
        let path = self.config_dir.join("version.json");
        let last = super::read_json(&path)
            .and_then(|v| v.get("last_update").and_then(Value::as_f64))
            .unwrap_or(0.0);
        let now = Utc::now().timestamp() as f64;
        if now - last < 86_400.0 {
            return;
        }
        let fetched = async {
            let resp = self
                .http
                .get(LEGENDARY_API)
                .send()
                .await
                .map_err(net)?
                .error_for_status()
                .map_err(net)?;
            resp.json::<Value>().await.map_err(net)
        }
        .await;
        match fetched {
            Ok(data) => {
                let doc = serde_json::json!({ "data": data, "last_update": now });
                if let Err(e) = super::write_json(&path, &doc) {
                    tracing::debug!(error = %e, "could not save version.json");
                }
            }
            Err(e) => tracing::debug!(error = %e, "legendary version info not fetched"),
        }
    }

    /// Authorization code → session (`POST /account/api/oauth/token`), the
    /// JSON legendary stores as `user.json`. Stored in the vault.
    pub async fn login_with_code(&self, code: &str) -> Result<()> {
        let (id, secret) = self.oauth_client();
        let resp = self
            .http
            .post(format!("https://{OAUTH_HOST}/account/api/oauth/token"))
            .basic_auth(id, Some(secret))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("token_type", "eg1"),
            ])
            .send()
            .await
            .map_err(|e| Error::Other(format!("Epic login: {e}")))?;
        let status = resp.status();
        let body = Zeroizing::new(
            resp.bytes()
                .await
                .map_err(|e| Error::Other(format!("Epic login: {e}")))?
                .to_vec(),
        );
        let json: Value = serde_json::from_slice(&body)
            .map_err(|_| Error::Other(format!("Epic login: HTTP {status}")))?;
        if let Some(err) = json.get("errorCode").and_then(Value::as_str) {
            return Err(Error::Helper {
                helper: "Epic".into(),
                message: err.to_string(),
            });
        }
        if !status.is_success()
            || json.get("access_token").is_none()
            || json.get("account_id").is_none()
        {
            return Err(Error::Other(format!("Epic login: HTTP {status}")));
        }
        self.user.replace(&body).await
    }

    fn session(&self) -> Result<Session> {
        let raw = self
            .user
            .read()?
            .ok_or_else(|| Error::NotLoggedIn("Epic Games".into()))?;
        Session::parse(&raw)
    }

    /// Account id and display name of the signed-in user.
    pub fn profile(&self) -> Result<(String, String)> {
        let s = self.session()?;
        Ok((
            s.account_id().unwrap_or_default().to_string(),
            s.display_name().unwrap_or_default().to_string(),
        ))
    }

    /// A usable access token: the stored one while it has 10 minutes left,
    /// else a refreshed session (saved back to the vault). A refresh token
    /// Epic rejects means the login is gone, like legendary's `lock.clear()`.
    /// `rejected`: the access token Epic just answered 401 to. When the
    /// stored one differs, another request refreshed meanwhile (its refresh
    /// ran while this one waited for the lock) and that token is used
    /// instead of refreshing once more.
    async fn token(&self, rejected: Option<&str>) -> Result<Zeroizing<String>> {
        let _one_at_a_time = self.refresh.lock().await;
        // the generation before the read: a logout meanwhile makes the
        // write-back below fail instead of restoring the login
        let generation = self.user.generation();
        let mut s = self.session()?;
        let usable = s.access_token_usable(Utc::now());
        let force = match rejected {
            Some(old) => old == s.access_token.as_str(),
            None => false,
        };
        if usable && !force {
            return Ok(s.access_token.clone());
        }
        let (id, secret) = self.oauth_client();
        let resp = self
            .http
            .post(format!("https://{OAUTH_HOST}/account/api/oauth/token"))
            .basic_auth(id, Some(secret))
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", s.refresh_token.as_str()),
                ("token_type", "eg1"),
            ])
            .send()
            .await
            .map_err(net)?;
        let status = resp.status();
        let body = Zeroizing::new(resp.bytes().await.map_err(net)?.to_vec());
        let json: Value = serde_json::from_slice(&body)
            .map_err(|_| Error::Other(format!("Epic: HTTP {status}")))?;
        if let Some(code) = json.get("errorCode").and_then(Value::as_str) {
            // only a refused refresh token means the login is gone; throttling
            // (429), a WAF block (403) or an outage keep it for the next try
            let login_gone = (status == 400 || status == 401)
                && (code.starts_with("errors.com.epicgames.account.")
                    || code.starts_with("errors.com.epicgames.oauth."));
            if !login_gone {
                return Err(Error::Other(format!("Epic: {code} (HTTP {status})")));
            }
            tracing::warn!(code, "Epic refused the stored login; signed out");
            self.user.revoke().await?;
            return Err(Error::NotLoggedIn("Epic Games".into()));
        }
        let Some(map) = json.as_object().filter(|_| status.is_success()) else {
            return Err(Error::Other(format!("Epic: HTTP {status}")));
        };
        s.merge(map.clone());
        self.user.update(generation, &s.to_bytes()?).await?;
        tracing::debug!("Epic session refreshed");
        Ok(s.access_token.clone())
    }

    /// A signed request, retried once with a fresh token on 401 and with a
    /// pause on 429 (a first login makes many requests at once).
    async fn authed(
        &self,
        make: impl Fn(&reqwest::Client, &str) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        let mut token = self.token(None).await?;
        let mut refreshed = false;
        let mut attempt = 0u32;
        loop {
            let resp = make(&self.http, &token).send().await.map_err(net)?;
            match resp.status().as_u16() {
                401 if !refreshed => {
                    refreshed = true;
                    token = self.token(Some(token.as_str())).await?;
                }
                429 if attempt < 3 => {
                    attempt += 1;
                    let wait = Duration::from_secs(5 * attempt as u64);
                    tracing::warn!(
                        attempt,
                        wait_s = wait.as_secs(),
                        "Epic rate limit (429), retrying"
                    );
                    tokio::time::sleep(wait).await;
                }
                429 => {
                    return Err(Error::Helper {
                        helper: "Epic".into(),
                        message: RATE_LIMIT_MESSAGE.into(),
                    });
                }
                _ => return Ok(resp),
            }
        }
    }

    /// The JSON of a response, Epic's `errorCode` answers turned into errors.
    async fn json(resp: reqwest::Response) -> Result<Value> {
        let status = resp.status();
        let body = resp.bytes().await.map_err(net)?;
        let json: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) if status.is_success() => {
                return Err(Error::Other("Epic: response is not JSON".into()));
            }
            Err(_) => return Err(Error::Other(format!("Epic: HTTP {status}"))),
        };
        if let Some(code) = json.get("errorCode").and_then(Value::as_str) {
            let message = json
                .get("errorMessage")
                .and_then(Value::as_str)
                .unwrap_or(code);
            return Err(Error::Helper {
                helper: "Epic".into(),
                message: message.to_string(),
            });
        }
        if !status.is_success() {
            return Err(Error::Other(format!("Epic: HTTP {status}")));
        }
        Ok(json)
    }

    async fn get_json(&self, url: &str, query: &[(&str, &str)]) -> Result<Value> {
        let resp = self
            .authed(|http, token| http.get(url).query(query).bearer_auth(token))
            .await?;
        Self::json(resp).await
    }

    /// Installable builds the account owns for `platform` (`Windows`, `Mac`).
    pub async fn assets(&self, platform: &str) -> Result<Vec<Value>> {
        let url = format!("https://{LAUNCHER_HOST}/launcher/api/public/assets/{platform}");
        let v = self.get_json(&url, &[("label", "Live")]).await?;
        v.as_array()
            .cloned()
            .ok_or_else(|| Error::Other("Epic: assets is not a list".into()))
    }

    /// Everything in the library, installable or not (third-party titles
    /// have no assets); follows the cursor through all pages.
    pub async fn library_items(&self) -> Result<Vec<Value>> {
        let url = format!("https://{LIBRARY_HOST}/library/api/public/items");
        /// No library has this many pages; a server that keeps answering is stopped here.
        const MAX_PAGES: usize = 1000;
        let mut records = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for _ in 0..MAX_PAGES {
            let mut query = vec![("includeMetadata", "true")];
            if let Some(c) = cursor.as_deref() {
                query.push(("cursor", c));
            }
            let page = self.get_json(&url, &query).await?;
            if let Some(items) = page.get("records").and_then(Value::as_array) {
                records.extend(items.iter().cloned());
            }
            cursor = page
                .pointer("/responseMetadata/nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            match &cursor {
                None => return Ok(records),
                Some(c) if !seen.insert(c.clone()) => {
                    tracing::warn!("Epic library cursor repeats; stopping");
                    return Ok(records);
                }
                Some(_) => {}
            }
        }
        tracing::warn!(
            pages = MAX_PAGES,
            "Epic library has too many pages; stopping"
        );
        Ok(records)
    }

    /// Catalog entry of one item (title, images, custom attributes, DLC list).
    pub async fn game_info(&self, namespace: &str, catalog_item_id: &str) -> Result<Option<Value>> {
        let url =
            format!("https://{CATALOG_HOST}/catalog/api/shared/namespace/{namespace}/bulk/items");
        let v = self
            .get_json(
                &url,
                &[
                    ("id", catalog_item_id),
                    ("includeDLCDetails", "true"),
                    ("includeMainGameDetails", "true"),
                    ("country", LOCALE.1),
                    ("locale", LOCALE.0),
                ],
            )
            .await?;
        Ok(v.get(catalog_item_id).cloned())
    }

    /// The build's manifest record: CDN URLs, hash, sidecar config.
    pub async fn manifest_record(
        &self,
        namespace: &str,
        catalog_item_id: &str,
        app_name: &str,
        platform: &str,
    ) -> Result<Value> {
        let url = format!(
            "https://{LAUNCHER_HOST}/launcher/api/public/assets/v2/platform/{platform}/namespace/{namespace}/catalogItem/{catalog_item_id}/app/{app_name}/label/Live"
        );
        let v = self.get_json(&url, &[]).await?;
        let elements = v
            .get("elements")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match elements.len() {
            1 => Ok(elements.into_iter().next().unwrap_or_default()),
            0 => Err(Error::Other(format!(
                "Epic has no build of {app_name} for {platform}"
            ))),
            n => Err(Error::Other(format!(
                "Epic manifest response has {n} elements"
            ))),
        }
    }

    /// Download the manifest a record points to (first CDN that answers),
    /// checked against the record's SHA-1. Returns the bytes, the base URLs
    /// of the chunks and the record's secrets (keys of encrypted builds).
    pub async fn cdn_manifest(
        &self,
        record: &Value,
    ) -> Result<(Vec<u8>, Vec<String>, HashMap<String, String>)> {
        let secrets: HashMap<String, String> = record
            .get("secrets")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.to_ascii_uppercase(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let mut base_urls: Vec<String> = Vec::new();
        let mut urls: Vec<String> = Vec::new();
        for m in record
            .get("manifests")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(uri) = m.get("uri").and_then(Value::as_str) else {
                continue;
            };
            if let Some((base, _)) = uri.rsplit_once('/')
                && !base_urls.iter().any(|b| b == base)
            {
                base_urls.push(base.to_string());
            }
            let params: Vec<String> = m
                .get("queryParams")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    Some(format!(
                        "{}={}",
                        p.get("name")?.as_str()?,
                        p.get("value")?.as_str()?
                    ))
                })
                .collect();
            urls.push(if params.is_empty() {
                uri.to_string()
            } else {
                format!("{uri}?{}", params.join("&"))
            });
        }
        if urls.is_empty() {
            return Err(Error::Other("Epic returned no manifest URLs".into()));
        }
        let expected = record
            .get("hash")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut last = String::new();
        for url in &urls {
            match self.http.get(url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let bytes = resp.bytes().await.map_err(net)?.to_vec();
                    let got = format!("{:x}", Sha1::digest(&bytes));
                    if !expected.is_empty() && got != expected {
                        return Err(Error::Other("Epic manifest hash mismatch".into()));
                    }
                    return Ok((bytes, base_urls, secrets));
                }
                Ok(resp) => last = format!("HTTP {}", resp.status()),
                Err(e) => last = e.without_url().to_string(),
            }
            tracing::warn!(
                host = url.split('/').nth(2).unwrap_or(""),
                last,
                "manifest CDN failed, trying next"
            );
        }
        Err(Error::Other(format!(
            "could not download the manifest from any CDN ({last})"
        )))
    }

    /// The account's cloud-save files of a game (`manifests_only`: just the
    /// save manifests): path → `{readLink, lastModified, …}`.
    pub async fn cloud_files(
        &self,
        account_id: &str,
        app_name: &str,
        manifests_only: bool,
    ) -> Result<Map<String, Value>> {
        let tail = if manifests_only { "/manifests/" } else { "/" };
        let url = format!(
            "https://{DATASTORAGE_HOST}/api/v1/access/egstore/savesync/{account_id}/{app_name}{tail}"
        );
        let v = self.get_json(&url, &[]).await?;
        Ok(v.get("files")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default())
    }

    /// Ask for upload links for new save files: path → `{writeLink}`.
    pub async fn create_cloud_files(
        &self,
        account_id: &str,
        app_name: &str,
        names: &[String],
    ) -> Result<Map<String, Value>> {
        let url = format!(
            "https://{DATASTORAGE_HOST}/api/v1/access/egstore/savesync/{account_id}/{app_name}/"
        );
        let body = serde_json::json!({ "files": names });
        let resp = self
            .authed(|http, token| http.post(&url).bearer_auth(token).json(&body))
            .await?;
        let v = Self::json(resp).await?;
        Ok(v.get("files")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default())
    }

    /// A plain download (CDN chunks, save files by their read link).
    pub async fn fetch_url(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self.http.get(url).send().await.map_err(net)?;
        if !resp.status().is_success() {
            return Err(Error::Other(format!(
                "download failed: HTTP {}",
                resp.status()
            )));
        }
        Ok(resp.bytes().await.map_err(net)?.to_vec())
    }

    /// A plain upload to a pre-signed link.
    pub async fn put_url(&self, url: &str, body: Vec<u8>) -> Result<()> {
        let resp = self.http.put(url).body(body).send().await.map_err(net)?;
        if !resp.status().is_success() {
            return Err(Error::Other(format!(
                "upload failed: HTTP {}",
                resp.status()
            )));
        }
        Ok(())
    }

    /// The launcher's store GraphQL (achievements), called the way the Epic
    /// Games Launcher does (its user agent, the account's token).
    pub async fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        const STORE_USER_AGENT: &str = "EpicGamesLauncher/14.0.8-22004686+++Portal+Release-Live";
        let body = serde_json::json!({ "query": query, "variables": variables });
        let resp = self
            .authed(|http, token| {
                http.post("https://launcher.store.epicgames.com/graphql")
                    .bearer_auth(token)
                    .header(reqwest::header::USER_AGENT, STORE_USER_AGENT)
                    .json(&body)
            })
            .await?;
        let v = Self::json(resp).await?;
        if let Some(first) = v
            .get("errors")
            .and_then(Value::as_array)
            .and_then(|e| e.first())
        {
            let message = first
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("error");
            return Err(Error::Helper {
                helper: "Epic".into(),
                message: message.to_string(),
            });
        }
        Ok(v)
    }

    /// One-time exchange code the game logs in with (`-AUTH_PASSWORD=`).
    pub async fn exchange_code(&self) -> Result<Zeroizing<String>> {
        let url = format!("https://{OAUTH_HOST}/account/api/oauth/exchange");
        let v = self.get_json(&url, &[]).await?;
        v.get("code")
            .and_then(Value::as_str)
            .map(|c| Zeroizing::new(c.to_owned()))
            .ok_or_else(|| Error::Other("Epic returned no exchange code".into()))
    }

    /// Ownership verification token of a game that requires one (`-epicovt=`).
    pub async fn ownership_token(
        &self,
        account_id: &str,
        namespace: &str,
        catalog_item_id: &str,
    ) -> Result<Vec<u8>> {
        let url = format!(
            "https://{ECOMMERCE_HOST}/ecommerceintegration/api/public/platforms/EPIC/identities/{account_id}/ownershipToken"
        );
        let item = format!("{namespace}:{catalog_item_id}");
        let resp = self
            .authed(|http, token| {
                http.post(&url)
                    .bearer_auth(token)
                    .form(&[("nsCatalogItemId", item.as_str())])
            })
            .await?;
        if !resp.status().is_success() {
            return Err(Error::Other(format!(
                "Epic ownership token: HTTP {}",
                resp.status()
            )));
        }
        Ok(resp.bytes().await.map_err(net)?.to_vec())
    }
}

/// `appName`-style EGS asset → the `app_name`-style record legendary keeps
/// in `assets.json`.
pub(crate) fn asset_record(egs: &Value) -> Map<String, Value> {
    let s = |k: &str| {
        Value::String(
            egs.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        )
    };
    let mut m = Map::new();
    m.insert("app_name".into(), s("appName"));
    m.insert("asset_id".into(), s("assetId"));
    m.insert("build_version".into(), s("buildVersion"));
    m.insert("catalog_item_id".into(), s("catalogItemId"));
    m.insert("label_name".into(), s("labelName"));
    m.insert("namespace".into(), s("namespace"));
    m.insert(
        "metadata".into(),
        egs.get("metadata")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new())),
    );
    m.insert(
        "sidecar_rev".into(),
        egs.get("sidecarRvn").cloned().unwrap_or(Value::from(0)),
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_records_match_legendary() {
        let egs = serde_json::json!({
            "appName": "Catnip", "assetId": "a1", "buildVersion": "1.2", "catalogItemId": "c1",
            "labelName": "Live", "namespace": "ns", "metadata": { "installationPoolId": "x" }, "sidecarRvn": 3
        });
        let r = asset_record(&egs);
        assert_eq!(r["app_name"], "Catnip");
        assert_eq!(r["build_version"], "1.2");
        assert_eq!(r["sidecar_rev"], 3);
        assert_eq!(r["metadata"]["installationPoolId"], "x");
        assert_eq!(asset_record(&serde_json::json!({}))["sidecar_rev"], 0);
    }
}
