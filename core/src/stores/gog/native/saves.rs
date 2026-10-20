//! GOG cloud saves, replacing `gogdl save-sync` (gogdl's `saves.py`). A
//! game's saves live in GOG's cloud storage under the game's own OAuth
//! client (`clientId` / `clientSecret` from its build meta), as gzip files
//! named `<location>/<relative path>`; `X-Object-Meta-LocalLastModified`
//! keeps the file's time. Which side is newer is decided per file against
//! the time of the last sync (`saveTimestamps.json`, kept by the caller):
//! files changed since then on one side only are copied, on both sides
//! it is a conflict and nothing moves (unless forced).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value;

use super::{CLOUD_STORAGE, Client, MAX_JSON, body_limited, md5_hex, net, now_secs, target_build};
use crate::error::{Error, Result};
use crate::types::SaveSyncMode;

/// The largest single save file we take from the cloud (packed and unpacked).
const MAX_SAVE: u64 = 256 << 20;

/// Galaxy's own client string; the storage expects it.
const USER_AGENT: &str = "GOGGalaxyCommunicationService/2.0.13.27 (Windows_32bit) dont_sync_marker/true installation_source/gog";
/// MD5 of an empty gzip: GOG marks deleted files with it.
const DELETED_MARKER: &str = "aadd86936a80ee8a369579c3926f1b3c";

#[derive(Debug, Clone)]
struct SyncFile {
    /// Cloud name without the location prefix, `/` separators.
    relative: String,
    absolute: PathBuf,
    md5: String,
    /// `2024-01-31T20:15:00+00:00`
    update_time: String,
    update_ts: f64,
}

#[derive(Debug, Default)]
pub struct SyncOutcome {
    /// The new sync time to store (the old one when the sync was refused).
    pub timestamp: String,
    /// What happened, for the log and the UI.
    pub lines: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Action {
    Download,
    Upload,
    Conflict,
    None,
}

#[derive(Default)]
struct Classified {
    updated_local: Vec<SyncFile>,
    updated_cloud: Vec<SyncFile>,
    not_local: Vec<SyncFile>,
    not_remote: Vec<SyncFile>,
}

impl Classified {
    fn action(&self) -> Action {
        match (self.updated_local.is_empty(), self.updated_cloud.is_empty()) {
            (true, false) => Action::Download,
            (false, true) => Action::Upload,
            (true, true) => Action::None,
            (false, false) => Action::Conflict,
        }
    }
}

fn classify(local: &[SyncFile], cloud: &[SyncFile], since: f64) -> Classified {
    let mut c = Classified::default();
    for f in local {
        if !cloud.iter().any(|x| x.relative == f.relative) {
            c.not_remote.push(f.clone());
        }
        if f.update_ts > since {
            c.updated_local.push(f.clone());
        }
    }
    for f in cloud {
        if f.md5 == DELETED_MARKER {
            continue;
        }
        if !local.iter().any(|x| x.relative == f.relative) {
            c.not_local.push(f.clone());
        }
        if f.update_ts > since {
            c.updated_cloud.push(f.clone());
        }
    }
    c
}

/// gzip level 6 with a zeroed header time, as gogdl uploads.
fn gzip(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut enc = flate2::GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), flate2::Compression::new(6));
    enc.write_all(data)?;
    enc.finish()
}

/// What a downloaded save holds on disk. The storage answers with the gzip
/// we uploaded and `Content-Encoding: gzip`; gogdl's `requests` decodes
/// that transparently, so do we (by the header only, like `requests`), at
/// most `limit` bytes.
fn decode_body(
    body: &[u8],
    content_encoding: Option<&str>,
    limit: u64,
) -> std::io::Result<Vec<u8>> {
    if !content_encoding.is_some_and(|e| e.trim().eq_ignore_ascii_case("gzip")) {
        return Ok(body.to_vec());
    }
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(body)
        .take(limit.saturating_add(1))
        .read_to_end(&mut out)?;
    if out.len() as u64 > limit {
        return Err(std::io::Error::other(format!(
            "cloud save is larger than {limit} bytes"
        )));
    }
    Ok(out)
}

fn iso_utc(t: SystemTime) -> (String, f64) {
    let dt: DateTime<Utc> = t.into();
    (
        dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string(),
        dt.timestamp() as f64,
    )
}

/// `2024-01-31T20:15:00+00:00`, `2024-01-31T20:15:00.123456` (UTC) → epoch.
fn parse_time(s: &str) -> Option<f64> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp() as f64 + dt.timestamp_subsec_micros() as f64 / 1e6);
    }
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
        .ok()?;
    Some(
        naive.and_utc().timestamp() as f64 + naive.and_utc().timestamp_subsec_micros() as f64 / 1e6,
    )
}

/// Every file under `root`, with gogdl's metadata (MD5 of the gzip, mtime).
fn local_files(root: &Path) -> Result<Vec<SyncFile>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)?.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let data = std::fs::read(&path)?;
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let (update_time, update_ts) = iso_utc(modified);
            out.push(SyncFile {
                relative,
                absolute: path,
                md5: md5_hex(&gzip(&data)?),
                update_time,
                update_ts,
            });
        }
    }
    out.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(out)
}

/// A cloud name must stay inside the save folder.
fn safe_relative(rel: &str) -> bool {
    crate::stores::is_confined_rel_path(rel)
}

fn quote(rel: &str) -> String {
    rel.split('/')
        .map(urlencoding)
        .collect::<Vec<_>>()
        .join("/")
}

/// `urllib.parse.quote` with its default safe set (`/` kept, handled by the caller).
fn urlencoding(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

struct Storage<'a> {
    client: &'a Client,
    token: String,
    user_id: String,
    client_id: String,
    location: String,
    root: PathBuf,
}

impl Storage<'_> {
    fn base(&self) -> String {
        format!("{CLOUD_STORAGE}/v1/{}/{}", self.user_id, self.client_id)
    }

    fn request(&self, method: reqwest::Method, url: String) -> reqwest::RequestBuilder {
        self.client
            .http
            .request(method, url)
            .bearer_auth(&self.token)
            .header("User-Agent", USER_AGENT)
            .header("X-Object-Meta-User-Agent", USER_AGENT)
    }

    fn file_url(&self, rel: &str) -> String {
        format!("{}/{}/{}", self.base(), self.location, quote(rel))
    }

    async fn list(&self) -> Result<Vec<SyncFile>> {
        let resp = self
            .request(reqwest::Method::GET, self.base())
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(net)?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if !resp.status().is_success() {
            return Err(Error::Other(format!(
                "GOG cloud storage: HTTP {}",
                resp.status()
            )));
        }
        let items: Vec<Value> = serde_json::from_slice(&body_limited(resp, MAX_JSON).await?)?;
        let prefix = format!("{}/", self.location);
        let mut out = Vec::new();
        for it in &items {
            let Some(name) = it.get("name").and_then(Value::as_str) else {
                continue;
            };
            if !name.starts_with(&self.location) {
                continue;
            }
            let relative = name.strip_prefix(&prefix).unwrap_or(name).to_string();
            if !safe_relative(&relative) {
                tracing::warn!(name, "cloud save with an unsafe name skipped");
                continue;
            }
            let update_time = it
                .get("last_modified")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let update_ts = parse_time(&update_time).unwrap_or(0.0);
            out.push(SyncFile {
                absolute: self.root.join(&relative),
                relative,
                md5: it
                    .get("hash")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                update_time,
                update_ts,
            });
        }
        Ok(out)
    }

    async fn upload(&self, f: &SyncFile) -> Result<()> {
        let path = f.absolute.clone();
        let packed = tokio::task::spawn_blocking(move || gzip(&std::fs::read(path)?))
            .await
            .map_err(|e| Error::Other(format!("save packing task failed: {e}")))??;
        let etag = md5_hex(&packed);
        let resp = self
            .request(reqwest::Method::PUT, self.file_url(&f.relative))
            .header("X-Object-Meta-LocalLastModified", &f.update_time)
            .header("Etag", etag)
            .header("Content-Encoding", "gzip")
            .body(packed)
            .send()
            .await
            .map_err(net)?;
        if !resp.status().is_success() {
            return Err(Error::Other(format!(
                "upload of {} failed: HTTP {}",
                f.relative,
                resp.status()
            )));
        }
        Ok(())
    }

    async fn download(&self, f: &SyncFile) -> Result<()> {
        let mut last = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            let resp = match self
                .request(reqwest::Method::GET, self.file_url(&f.relative))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    last = net(e).to_string();
                    continue;
                }
            };
            if !resp.status().is_success() {
                last = format!("HTTP {}", resp.status());
                continue;
            }
            let modified = resp
                .headers()
                .get("X-Object-Meta-LocalLastModified")
                .and_then(|v| v.to_str().ok())
                .and_then(parse_time);
            let encoding = resp
                .headers()
                .get(reqwest::header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let bytes = match body_limited(resp, MAX_SAVE).await {
                Ok(b) => b,
                Err(e) => {
                    last = e.to_string();
                    continue;
                }
            };
            let path = f.absolute.clone();
            tokio::task::spawn_blocking(move || -> Result<()> {
                let data = decode_body(&bytes, encoding.as_deref(), MAX_SAVE)?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, &data)?;
                if let Some(ts) = modified.filter(|t| t.is_finite() && *t > 0.0 && *t < 1e11) {
                    let time = UNIX_EPOCH + std::time::Duration::from_secs_f64(ts);
                    if let Ok(file) = std::fs::File::options().write(true).open(&path) {
                        let _ = file.set_modified(time);
                    }
                }
                Ok(())
            })
            .await
            .map_err(|e| Error::Other(format!("save writing task failed: {e}")))??;
            return Ok(());
        }
        Err(Error::Other(format!(
            "download of {} failed: {last}",
            f.relative
        )))
    }

    async fn delete(&self, f: &SyncFile) -> Result<()> {
        let resp = self
            .request(reqwest::Method::DELETE, self.file_url(&f.relative))
            .send()
            .await
            .map_err(net)?;
        if !resp.status().is_success() && resp.status() != reqwest::StatusCode::NOT_FOUND {
            return Err(Error::Other(format!(
                "delete of {} failed: HTTP {}",
                f.relative,
                resp.status()
            )));
        }
        Ok(())
    }
}

/// The game's OAuth client from its build meta (`clientId`, `clientSecret`).
async fn game_client(client: &Client, app_name: &str, platform: &str) -> Result<(String, String)> {
    if let Some(hit) = client.game_clients.lock().await.get(app_name) {
        return Ok(hit.clone());
    }
    let builds = client.builds(app_name, platform).await?;
    let build = target_build(&builds, None, None)
        .ok_or_else(|| Error::Other(format!("GOG has no {platform} build of {app_name}")))?;
    let link = super::meta_url(build)
        .ok_or_else(|| Error::Other("the build has no manifest link".into()))?;
    let meta = client.get_zlib_json(link, false).await?;
    let s = |k: &str| {
        meta.get(k)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let ids = match (s("clientId"), s("clientSecret")) {
        (Some(id), Some(secret)) => (id, secret),
        _ => {
            return Err(Error::Other(format!(
                "{app_name} has no Galaxy client; no cloud saves"
            )));
        }
    };
    client
        .game_clients
        .lock()
        .await
        .insert(app_name.to_string(), ids.clone());
    Ok(ids)
}

/// Sync `dir` (cloud location `location`) of `app_name`; `since` is the
/// time of the last sync ("0": never).
pub async fn sync(
    client: &Client,
    app_name: &str,
    platform: &str,
    dir: &Path,
    location: &str,
    since: &str,
    mode: SaveSyncMode,
) -> Result<SyncOutcome> {
    let since_ts: f64 = since.trim().parse().unwrap_or(0.0);
    let mut out = SyncOutcome {
        timestamp: since.to_string(),
        lines: Vec::new(),
    };
    if !dir.exists() {
        out.lines.push("Save folder missing, creating it".into());
        tokio::fs::create_dir_all(dir).await?;
    }
    // reads and packs every save: off the async threads
    let local = {
        let root = dir.to_path_buf();
        tokio::task::spawn_blocking(move || local_files(&root))
            .await
            .map_err(|e| Error::Other(format!("save scan task failed: {e}")))??
    };
    out.lines.push(format!("Local files: {}", local.len()));

    let (client_id, client_secret) = game_client(client, app_name, platform).await?;
    let creds = client
        .auth
        .credentials_for(&client_id, &client_secret)
        .await?;
    let storage = Storage {
        client,
        token: creds.access_token,
        user_id: creds.user_id,
        client_id,
        location: location.to_string(),
        root: dir.to_path_buf(),
    };
    let cloud = storage.list().await?;
    out.lines.push(format!("Files in cloud: {}", cloud.len()));
    let downloadable: Vec<SyncFile> = cloud
        .iter()
        .filter(|f| f.md5 != DELETED_MARKER)
        .cloned()
        .collect();
    let may_upload = !matches!(mode, SaveSyncMode::Download | SaveSyncMode::ForceDownload);
    let may_download = !matches!(mode, SaveSyncMode::Upload | SaveSyncMode::ForceUpload);

    let finish = |mut out: SyncOutcome, what: &str| {
        out.lines.push(what.to_string());
        out.timestamp = format!("{:.6}", now_secs());
        out
    };

    if !local.is_empty() && cloud.is_empty() {
        if !may_upload {
            out.lines
                .push("No files in cloud; uploading is off for this sync".into());
            return Ok(out);
        }
        out.lines.push("No files in cloud, uploading".into());
        for f in &local {
            storage.upload(f).await?;
        }
        return Ok(finish(out, &format!("Uploaded {} files", local.len())));
    }
    if local.is_empty() && !cloud.is_empty() {
        if !may_download {
            out.lines
                .push("No files locally; downloading is off for this sync".into());
            return Ok(out);
        }
        out.lines.push("No files locally, downloading".into());
        for f in &downloadable {
            storage.download(f).await?;
        }
        return Ok(finish(
            out,
            &format!("Downloaded {} files", downloadable.len()),
        ));
    }
    if local.is_empty() && cloud.is_empty() {
        return Ok(finish(out, "No save files locally or in the cloud"));
    }

    let mut c = classify(&local, &cloud, since_ts);
    let mut action = c.action();
    match mode {
        SaveSyncMode::ForceUpload => {
            out.lines.push("Forcing upload".into());
            c.updated_local = local.clone();
            action = Action::Upload;
        }
        SaveSyncMode::ForceDownload => {
            out.lines.push("Forcing download".into());
            c.updated_cloud = downloadable.clone();
            action = Action::Download;
        }
        SaveSyncMode::Upload if action == Action::Download => {
            out.lines
                .push("Refused to upload: newer files in the cloud".into());
            return Ok(out);
        }
        SaveSyncMode::Download if action == Action::Upload => {
            out.lines
                .push("Refused to download: newer files locally".into());
            return Ok(out);
        }
        _ => {}
    }
    match action {
        Action::Upload => {
            out.lines
                .push(format!("Uploading {} files", c.updated_local.len()));
            for f in &c.updated_local {
                storage.upload(f).await?;
            }
            for f in &c.not_local {
                out.lines.push(format!("Deleting in cloud: {}", f.relative));
                storage.delete(f).await?;
            }
            Ok(finish(out, "Local saves uploaded"))
        }
        Action::Download => {
            out.lines
                .push(format!("Downloading {} files", c.updated_cloud.len()));
            for f in &c.updated_cloud {
                storage.download(f).await?;
            }
            for f in &c.not_remote {
                out.lines.push(format!("Deleting locally: {}", f.relative));
                if let Err(e) = tokio::fs::remove_file(&f.absolute).await
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(e.into());
                }
            }
            Ok(finish(out, "Cloud saves downloaded"))
        }
        Action::Conflict => {
            out.lines.push("Saves changed both locally and in the cloud; nothing synced (force one side to resolve)".into());
            Ok(out)
        }
        Action::None => Ok(finish(out, "Saves are up to date")),
    }
}

/// Cache of game OAuth clients, kept on the [`Client`].
pub(crate) type GameClients = tokio::sync::Mutex<HashMap<String, (String, String)>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn f(rel: &str, ts: f64, md5: &str) -> SyncFile {
        SyncFile {
            relative: rel.into(),
            absolute: PathBuf::from(rel),
            md5: md5.into(),
            update_time: String::new(),
            update_ts: ts,
        }
    }

    #[test]
    fn classifies_like_gogdl() {
        let local = vec![
            f("a", 10.0, "x"),
            f("b", 30.0, "x"),
            f("only-local", 5.0, "x"),
        ];
        let cloud = vec![
            f("a", 10.0, "y"),
            f("b", 10.0, "y"),
            f("only-cloud", 5.0, "y"),
            f("deleted", 50.0, DELETED_MARKER),
        ];
        let c = classify(&local, &cloud, 20.0);
        assert_eq!(
            c.updated_local
                .iter()
                .map(|x| x.relative.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
        assert!(c.updated_cloud.is_empty());
        assert_eq!(
            c.not_remote
                .iter()
                .map(|x| x.relative.as_str())
                .collect::<Vec<_>>(),
            ["only-local"]
        );
        assert_eq!(
            c.not_local
                .iter()
                .map(|x| x.relative.as_str())
                .collect::<Vec<_>>(),
            ["only-cloud"]
        );
        assert_eq!(c.action(), Action::Upload);
        let both = classify(&local, &[f("a", 40.0, "y")], 20.0);
        assert_eq!(both.action(), Action::Conflict);
        assert_eq!(
            classify(&[], &[f("a", 40.0, "y")], 20.0).action(),
            Action::Download
        );
        assert_eq!(
            classify(&[f("a", 1.0, "x")], &[f("a", 1.0, "y")], 20.0).action(),
            Action::None
        );
    }

    #[test]
    fn parses_cloud_times() {
        assert_eq!(parse_time("2024-01-31T20:15:00+00:00"), Some(1706732100.0));
        assert_eq!(parse_time("2024-01-31T20:15:00.500000"), Some(1706732100.5));
        assert_eq!(parse_time("nope"), None);
    }

    #[test]
    fn gzip_round_trips_with_a_zeroed_header_time() {
        let packed = gzip(b"save data").unwrap();
        assert_eq!(
            &packed[..4],
            &[0x1f, 0x8b, 8, 0],
            "gzip magic, deflate, no flags"
        );
        assert_eq!(&packed[4..8], &[0, 0, 0, 0], "mtime 0");
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&packed[..]), &mut out)
            .unwrap();
        assert_eq!(out, b"save data");
    }

    #[test]
    fn downloads_are_gunzipped_by_the_header_only() {
        let raw = b"save slot 1".to_vec();
        let packed = gzip(&raw).unwrap();
        // the upload's Etag is the MD5 of these compressed bytes
        assert_eq!(md5_hex(&packed).len(), 32);
        assert_eq!(decode_body(&packed, Some("gzip"), 1 << 20).unwrap(), raw);
        assert_eq!(decode_body(&packed, Some(" GZIP "), 1 << 20).unwrap(), raw);
        // no header: written as sent, even when it looks like gzip
        assert_eq!(decode_body(&packed, None, 1 << 20).unwrap(), packed);
        assert_eq!(
            decode_body(&packed, Some("identity"), 1 << 20).unwrap(),
            packed
        );
        // a header with a body that is no gzip is an error, not garbage on disk
        assert!(decode_body(b"plain", Some("gzip"), 1 << 20).is_err());
        // a bomb stops at the limit
        let big = gzip(&vec![0u8; 100_000]).unwrap();
        assert!(decode_body(&big, Some("gzip"), 99_999).is_err());
        assert_eq!(
            decode_body(&big, Some("gzip"), 100_000).unwrap().len(),
            100_000
        );
    }

    #[test]
    fn cloud_names_stay_in_the_folder() {
        assert!(safe_relative("slot1/save.dat"));
        assert!(!safe_relative("../x"));
        assert!(!safe_relative("/abs"));
        assert!(!safe_relative(""));
        assert_eq!(quote("a b/ü.sav"), "a%20b/%C3%BC.sav");
    }
}
