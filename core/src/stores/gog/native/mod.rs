//! GOG without `gogdl`: port of Heroic's `heroic-gogdl` 1.3 (Python, GPL-3)
//! to Rust, in one process. The on-disk layout stays gogdl's (`auth.json`
//! entries in the vault, `manifests/<id>` with the `HGL*` keys, `.gogdl-resume`
//! in the game folder, `saveTimestamps.json`), so installs made with gogdl
//! keep working and the owner can switch back at any point.
//!
//! - `auth`: token refresh from the vault (no plaintext file any more).
//! - `languages`, `manifest`: builds, v1 / v2 metas, depot file lists, diff.
//! - `info`: what `gogdl info` reported (sizes, languages, DLCs, folder).
//! - `import`: what `gogdl import` read from a game folder.
//! - `saves`: `gogdl save-sync` against GOG's cloud storage.
//! - `download`: `gogdl download | update | repair`.

pub mod auth;
pub mod download;
pub mod import;
pub mod info;
pub mod languages;
pub mod manifest;
pub mod redist;
pub mod saves;

use std::collections::HashSet;
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::secrets::SecretFile;

pub const GOG_CDN: &str = "https://gog-cdn-fastly.gog.com";
pub const CONTENT_SYSTEM: &str = "https://content-system.gog.com";
pub const GOG_EMBED: &str = "https://embed.gog.com";
pub const CLOUD_STORAGE: &str = "https://cloudstorage.gog.com";

/// How long the owned-products list answers `owns` without asking again.
const OWNED_TTL: Duration = Duration::from_secs(5 * 60);
const ZLIB_RETRIES: u32 = 5;
/// The largest JSON document (build meta, depot manifest, listing) we
/// accept from GOG; real ones are a few MiB at most.
pub(crate) const MAX_JSON: u64 = 64 << 20;
/// Idle limit for one read of a response body.
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(60);

/// A reqwest error without its URL: secure links and token requests carry
/// secrets in their query string.
pub(crate) fn net(e: reqwest::Error) -> Error {
    Error::Other(format!("GOG: {}", e.without_url()))
}

/// Lower-case hex MD5 (chunk ids, file checks, cloud-save Etags).
pub(crate) fn md5_hex(data: &[u8]) -> String {
    hex(Md5::digest(data))
}

/// Lower-case hex of a digest.
pub(crate) fn hex(digest: impl AsRef<[u8]>) -> String {
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Seconds since the epoch, as gogdl writes `loginTime` and the save
/// timestamps.
pub(crate) fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// A product id as GOG hands them out (and as the UI sends them back):
/// one run of ASCII digits, so it can be part of a path or a URL.
pub(crate) fn is_product_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_digit())
}

/// The response body, read piece by piece with an idle timeout per read and
/// refused once it is longer than `limit` bytes.
pub(crate) async fn body_limited(mut resp: reqwest::Response, limit: u64) -> Result<Vec<u8>> {
    if resp.content_length().is_some_and(|n| n > limit) {
        return Err(Error::Other(format!(
            "GOG: response of {} bytes exceeds the {limit} byte limit",
            resp.content_length().unwrap_or(0)
        )));
    }
    let mut out = Vec::with_capacity(resp.content_length().unwrap_or(0).min(limit) as usize);
    loop {
        let piece = tokio::time::timeout(BODY_READ_TIMEOUT, resp.chunk())
            .await
            .map_err(|_| Error::Other("GOG: the response stalled".into()))?
            .map_err(net)?;
        let Some(piece) = piece else { break };
        if out.len() as u64 + piece.len() as u64 > limit {
            return Err(Error::Other(format!(
                "GOG: response exceeds the {limit} byte limit"
            )));
        }
        out.extend_from_slice(&piece);
    }
    Ok(out)
}

/// The GOG web API with the account's tokens.
pub struct Client {
    pub http: reqwest::Client,
    pub auth: auth::Auth,
    owned: Mutex<Option<(Instant, HashSet<String>)>>,
    /// Games' own OAuth clients (cloud saves), by product id.
    pub(crate) game_clients: saves::GameClients,
}

impl Client {
    pub fn new(user: Arc<SecretFile>, http: reqwest::Client) -> Self {
        Self {
            auth: auth::Auth::new(user, http.clone()),
            http,
            owned: Mutex::new(None),
            game_clients: saves::GameClients::default(),
        }
    }

    /// `GET url` with the account token when `authed`; JSON body.
    pub async fn get_json(&self, url: &str, authed: bool) -> Result<Value> {
        let mut req = self.http.get(url).header("Accept", "application/json");
        if authed {
            req = req.bearer_auth(self.auth.credentials().await?.access_token);
        }
        let started = Instant::now();
        let resp = req.send().await.map_err(net)?;
        let status = resp.status();
        tracing::debug!(url = redact_url(url), %status, ms = started.elapsed().as_millis() as u64, "GOG GET");
        if !status.is_success() {
            return Err(Error::Other(format!(
                "GOG: HTTP {status} for {}",
                redact_url(url)
            )));
        }
        Ok(serde_json::from_slice(
            &body_limited(resp, MAX_JSON).await?,
        )?)
    }

    /// A zlib-compressed JSON document (build metas, depot manifests) or a
    /// plain one; GOG serves both from the same URLs. Retried a few times
    /// like gogdl's `get_zlib_encoded`.
    pub async fn get_zlib_json(&self, url: &str, authed: bool) -> Result<Value> {
        let mut last = String::new();
        for attempt in 0..ZLIB_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            let mut req = self.http.get(url);
            if authed {
                req = req.bearer_auth(self.auth.credentials().await?.access_token);
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    last = net(e).to_string();
                    continue;
                }
            };
            let status = resp.status();
            tracing::debug!(url = redact_url(url), %status, attempt, "GOG GET (zlib)");
            if !status.is_success() {
                return Err(Error::Other(format!(
                    "GOG: HTTP {status} for {}",
                    redact_url(url)
                )));
            }
            let bytes = match body_limited(resp, MAX_JSON).await {
                Ok(b) => b,
                Err(e) => {
                    last = e.to_string();
                    continue;
                }
            };
            let value = match inflate(&bytes, MAX_JSON) {
                Ok(raw) => serde_json::from_slice(&raw)?,
                Err(_) => serde_json::from_slice(&bytes)?,
            };
            return Ok(value);
        }
        Err(Error::Other(format!("GOG: {last}")))
    }

    /// Builds of `product` on `platform`, newest first.
    pub async fn builds(&self, product: &str, platform: &str) -> Result<Vec<Value>> {
        self.builds_with_password(product, platform, None).await
    }

    /// `builds` including a private branch's (gogdl's `--password`). The
    /// password goes into the query URL-encoded; errors never carry the URL.
    pub async fn builds_with_password(
        &self,
        product: &str,
        platform: &str,
        password: Option<&str>,
    ) -> Result<Vec<Value>> {
        if !is_product_id(product) {
            return Err(Error::Other(format!("not a GOG product id: {product:?}")));
        }
        let mut url = reqwest::Url::parse(&format!(
            "{CONTENT_SYSTEM}/products/{product}/os/{platform}/builds?generation=2&_version=2"
        ))
        .map_err(|e| Error::Other(format!("GOG builds: {e}")))?;
        if let Some(pw) = password.map(str::trim).filter(|p| !p.is_empty()) {
            url.query_pairs_mut().append_pair("password", pw);
        }
        let v = self
            .get_json(url.as_str(), true)
            .await
            .map_err(|e| Error::Other(format!("GOG builds: {e}")))?;
        Ok(v.get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Does the account own `product` (a DLC, mostly)?
    pub async fn owns(&self, product: &str) -> Result<bool> {
        // the cache is not held across the request: parallel callers would queue behind it
        if let Some(ids) = self
            .owned
            .lock()
            .await
            .as_ref()
            .filter(|(at, _)| at.elapsed() < OWNED_TTL)
            .map(|(_, ids)| ids.contains(product))
        {
            return Ok(ids);
        }
        let v = self
            .get_json(&format!("{GOG_EMBED}/user/data/games"), true)
            .await?;
        let ids: HashSet<String> = v
            .get("owned")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(id_string).collect())
            .unwrap_or_default();
        let owns = ids.contains(product);
        *self.owned.lock().await = Some((Instant::now(), ids));
        Ok(owns)
    }

    /// CDN endpoints for a product's depot (generation 2: chunks under
    /// `path`; generation 1: `main.bin` of `path`).
    pub async fn secure_link(
        &self,
        product: &str,
        path: &str,
        generation: u8,
    ) -> Result<Vec<Value>> {
        if !is_product_id(product) {
            return Err(Error::Other(format!("not a GOG product id: {product:?}")));
        }
        let url = match generation {
            1 => format!(
                "{CONTENT_SYSTEM}/products/{product}/secure_link?_version=2&type=depot&path={path}"
            ),
            _ => format!(
                "{CONTENT_SYSTEM}/products/{product}/secure_link?_version=2&generation=2&path={path}"
            ),
        };
        let mut last = String::new();
        for attempt in 0..ZLIB_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            match self.get_json(&url, true).await {
                Ok(v) => {
                    return v
                        .get("urls")
                        .and_then(Value::as_array)
                        .cloned()
                        .filter(|u| !u.is_empty())
                        .ok_or_else(|| Error::Other("GOG secure link has no urls".into()));
                }
                Err(e) => last = e.to_string(),
            }
        }
        Err(Error::Other(format!("GOG secure link: {last}")))
    }

    /// CDN endpoints of the shared redistributables store.
    pub async fn dependency_link(&self) -> Result<Vec<Value>> {
        let v = self
            .get_json(
                &format!(
                    "{CONTENT_SYSTEM}/open_link?generation=2&_version=2&path=/dependencies/store/"
                ),
                true,
            )
            .await?;
        v.get("urls")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| Error::Other("GOG dependency link has no urls".into()))
    }

    /// The redistributables repository: `depots[]` with `dependencyId`,
    /// `executable.path`, `manifest`, sizes; plus `build_id`.
    pub async fn dependencies_repo(&self) -> Result<Value> {
        let build = self
            .get_json(
                &format!("{CONTENT_SYSTEM}/dependencies/repository?generation=2"),
                true,
            )
            .await?;
        let url = build
            .get("repository_manifest")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Other("GOG dependencies repository has no manifest".into()))?;
        // the manifest URL is a public CDN link: no token goes there
        let mut repo = self.get_zlib_json(url, false).await?;
        repo["build_id"] = build.get("build_id").cloned().unwrap_or(Value::Null);
        Ok(repo)
    }
}

/// zlib → bytes, at most `limit` of them (a bomb is refused, not inflated).
pub(crate) fn inflate(data: &[u8], limit: u64) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .take(limit.saturating_add(1))
        .read_to_end(&mut out)?;
    if out.len() as u64 > limit {
        return Err(std::io::Error::other(format!(
            "inflated data exceeds {limit} bytes"
        )));
    }
    Ok(out)
}

/// A URL without its query (secure links carry signatures there).
fn redact_url(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// Product ids come as numbers or strings depending on the endpoint.
pub(crate) fn id_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `abcdef…` → `ab/cd/abcdef…`: where the CDN keeps a chunk / manifest.
/// Only a hex digest is a valid name (it ends up in a URL path).
pub fn galaxy_path(hash: &str) -> Result<String> {
    if hash.len() < 4 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Other(format!(
            "GOG: not a chunk / manifest id: {hash:?}"
        )));
    }
    Ok(format!("{}/{}/{hash}", &hash[0..2], &hash[2..4]))
}

/// The build to install: the one asked for by id, else the newest of
/// `branch` (`None`: the public, branch-less builds), else the newest
/// public one, else the newest.
pub fn target_build<'a>(
    items: &'a [Value],
    build_id: Option<&str>,
    branch: Option<&str>,
) -> Option<&'a Value> {
    let is_public = |b: &&Value| b.get("branch").is_none_or(Value::is_null);
    if let Some(id) = build_id.filter(|s| !s.is_empty())
        && let Some(b) = items.iter().find(|b| {
            b.get("build_id").and_then(Value::as_str) == Some(id)
                || b.get("legacy_build_id").and_then(id_string).as_deref() == Some(id)
        })
    {
        return Some(b);
    }
    if let Some(name) = branch.map(str::trim).filter(|s| !s.is_empty())
        && let Some(b) = items
            .iter()
            .find(|b| b.get("branch").and_then(Value::as_str) == Some(name))
    {
        return Some(b);
    }
    items.iter().find(is_public).or_else(|| items.first())
}

/// Names of the branches in a `builds` listing (the public one is unnamed).
pub fn branch_names(items: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in items
        .iter()
        .filter_map(|b| b.get("branch").and_then(Value::as_str))
    {
        if !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// The listing as the install dialog shows it, newest first.
pub fn build_entries(items: &[Value]) -> Vec<crate::types::BuildEntry> {
    items
        .iter()
        .filter_map(|b| {
            let build_id = b
                .get("build_id")
                .and_then(id_string)
                .or_else(|| b.get("legacy_build_id").and_then(id_string))?;
            Some(crate::types::BuildEntry {
                build_id,
                version: b
                    .get("version_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                date: b
                    .get("date_published")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                branch: b.get("branch").and_then(Value::as_str).map(str::to_owned),
            })
        })
        .collect()
}

/// Where a build's meta is: `link` (plain builds listing) or the first of
/// `urls` (`_version=2`, several CDNs).
pub fn meta_url(build: &Value) -> Option<&str> {
    build
        .get("link")
        .and_then(Value::as_str)
        .or_else(|| build.pointer("/urls/0/url").and_then(Value::as_str))
}

/// `{url_format}` with its `{parameters}` filled in (gogdl's
/// `merge_url_with_params`), `path` extended by `suffix`.
pub fn endpoint_url(endpoint: &Value, suffix: &str) -> Option<String> {
    if let Some(url) = endpoint.get("url").and_then(Value::as_str)
        && endpoint.get("url_format").is_none()
    {
        return Some(format!("{url}{suffix}"));
    }
    let mut url = endpoint.get("url_format")?.as_str()?.to_string();
    let params = endpoint.get("parameters")?.as_object()?;
    for (key, value) in params {
        let mut text = match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if key == "path" {
            text.push_str(suffix);
        }
        url = url.replace(&format!("{{{key}}}"), &text);
    }
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn galaxy_paths_split_the_hash() {
        assert_eq!(galaxy_path("abcdef0123").unwrap(), "ab/cd/abcdef0123");
        for bad in ["ab/cd/x", "abc", "", "../../etc", "ab?x=1", "abcd\u{e9}"] {
            assert!(galaxy_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn inflate_refuses_more_than_the_limit() {
        use std::io::Write;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&[0u8; 10_000]).unwrap();
        let packed = enc.finish().unwrap();
        assert_eq!(inflate(&packed, 10_000).unwrap().len(), 10_000);
        assert!(inflate(&packed, 9_999).is_err());
    }

    #[test]
    fn product_ids_are_digits() {
        assert!(is_product_id("1207658924"));
        for bad in [
            "",
            "12a",
            "../1",
            "1 ",
            "-1",
            "12345678901234567890123456789012345",
        ] {
            assert!(!is_product_id(bad), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn body_limited_stops_at_the_limit() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let body = vec![b'x'; 5000];
                let mut r = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                r.extend_from_slice(&body);
                let _ = sock.write_all(&r).await;
                let _ = sock.shutdown().await;
            }
        });
        let http = reqwest::Client::new();
        let ok = body_limited(http.get(&base).send().await.unwrap(), 5000)
            .await
            .unwrap();
        assert_eq!(ok.len(), 5000);
        assert!(
            body_limited(http.get(&base).send().await.unwrap(), 4999)
                .await
                .is_err()
        );
    }

    #[test]
    fn picks_the_public_build() {
        let items = vec![
            serde_json::json!({ "build_id": "beta", "branch": "beta" }),
            serde_json::json!({ "build_id": "pub", "branch": null }),
        ];
        assert_eq!(target_build(&items, None, None).unwrap()["build_id"], "pub");
        assert_eq!(
            target_build(&items, Some("beta"), None).unwrap()["build_id"],
            "beta"
        );
        assert_eq!(
            target_build(&items, Some("gone"), None).unwrap()["build_id"],
            "pub"
        );
        assert_eq!(
            target_build(&items, None, Some("beta")).unwrap()["build_id"],
            "beta"
        );
        assert_eq!(
            target_build(&items, None, Some("nope")).unwrap()["build_id"],
            "pub"
        );
        assert_eq!(branch_names(&items), ["beta"]);
        assert_eq!(build_entries(&items).len(), 2);
        assert_eq!(build_entries(&items)[0].branch.as_deref(), Some("beta"));
    }

    #[test]
    fn meta_url_accepts_both_listing_formats() {
        assert_eq!(
            meta_url(&serde_json::json!({ "link": "https://cdn/meta/a" })),
            Some("https://cdn/meta/a")
        );
        assert_eq!(
            meta_url(
                &serde_json::json!({ "urls": [{ "url": "https://cdn/meta/b", "url_format": "x" }] })
            ),
            Some("https://cdn/meta/b")
        );
        assert_eq!(meta_url(&serde_json::json!({})), None);
    }

    #[test]
    fn fills_in_secure_link_parameters() {
        let ep = serde_json::json!({
            "url_format": "https://cdn.gog.com/{path}?token={token}&expires={expires_at}",
            "parameters": { "path": "/content-system/v2/store/123", "token": "abc", "expires_at": 1700000000 }
        });
        assert_eq!(
            endpoint_url(&ep, "/ab/cd/abcd").unwrap(),
            "https://cdn.gog.com//content-system/v2/store/123/ab/cd/abcd?token=abc&expires=1700000000"
        );
        let redist = serde_json::json!({ "url": "https://cdn.gog.com/dependencies/store" });
        assert_eq!(
            endpoint_url(&redist, "/ab/cd/abcd").unwrap(),
            "https://cdn.gog.com/dependencies/store/ab/cd/abcd"
        );
    }
}
