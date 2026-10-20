//! Epic cloud saves without `legendary sync-saves`. A save in the cloud is
//! a manifest named after its time (`2024.01.31-20.15.00.manifest`) plus
//! the chunks it lists, all under the account's `savesync` storage. Port of
//! `check_savegame_state`, `download_saves` and `upload_save` in legendary's
//! `core.py` and of `utils/savegame_helper.py`.
//!
//! Which way to sync is decided like legendary's `sync-saves`: the newer
//! side wins, within a minute both count as the same, and the mode only
//! says what is allowed (download / upload / force one of them).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};

use super::api::Epic;
use super::chunk::{CHUNK_SIZE, Chunk};
use super::manifest::{ChunkInfo, ChunkPart, FileManifest, Manifest, Meta};
use super::{metadata_path, read_json};
use crate::error::{Error, Result};
use crate::types::SaveSyncMode;

const TIME_FORMAT: &str = "%Y.%m.%d-%H.%M.%S";
/// Saves this close in time are the same save (clock drift, file system
/// time stamp granularity).
const SAME_AGE: Duration = Duration::seconds(60);
/// Save manifests are written for this manifest version (legendary's default).
const SAVE_MANIFEST_VERSION: u32 = 18;

struct RemoteSave {
    /// The path in the storage listing, e.g. `…/<app>/manifests/<name>`.
    path: String,
    /// `<time>.manifest`
    manifest_name: String,
    time: DateTime<Utc>,
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    NoSave,
    LocalNewer,
    RemoteNewer,
    SameAge,
}

/// Sync the save folder `save_dir` of `app_name` with the cloud. Returns a
/// short summary of what happened (shown by the UI).
pub async fn sync(
    epic: &Epic,
    app_name: &str,
    save_dir: &Path,
    mode: SaveSyncMode,
) -> Result<String> {
    let game = read_json(&metadata_path(&epic.config_dir, app_name))
        .ok_or_else(|| Error::GameNotFound(app_name.to_string()))?;
    let attr = |k: &str| {
        game.pointer(&format!("/metadata/customAttributes/{k}/value"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let title = game
        .get("app_title")
        .and_then(Value::as_str)
        .unwrap_or(app_name)
        .to_string();
    let cloud_folder = attr("CloudSaveFolder");
    let cloud_folder_mac = attr("CloudSaveFolder_MAC");
    if cloud_folder.is_none() && cloud_folder_mac.is_none() {
        return Err(Error::Other(format!(
            "{title} does not support cloud saves"
        )));
    }
    let filters = Filters {
        include: attr("CloudIncludeList").map(|s| s.split(',').map(str::to_owned).collect()),
        exclude: attr("CloudExcludeList").map(|s| s.split(',').map(str::to_owned).collect()),
    };
    let (account_id, _) = epic.profile()?;

    let manifests = epic.cloud_files(&account_id, app_name, true).await?;
    let remote = latest_remote(&manifests);
    let local = {
        let dir = save_dir.to_path_buf();
        tokio::task::spawn_blocking(move || newest_local(&dir))
            .await
            .map_err(|e| Error::Other(format!("save scan task failed: {e}")))?
    };
    let state = match (&local, &remote) {
        (None, None) => State::NoSave,
        (Some(_), None) => State::LocalNewer,
        (None, Some(_)) => State::RemoteNewer,
        (Some(l), Some(r)) => {
            if (*l - r.time).abs() < SAME_AGE {
                State::SameAge
            } else if *l > r.time {
                State::LocalNewer
            } else {
                State::RemoteNewer
            }
        }
    };
    let fmt = |t: &DateTime<Utc>| t.format("%Y-%m-%d %H:%M:%S").to_string();
    tracing::info!(app = app_name, ?state, local = ?local.as_ref().map(fmt), remote = ?remote.as_ref().map(|r| fmt(&r.time)), "cloud save state");

    let force_download = mode == SaveSyncMode::ForceDownload;
    let force_upload = mode == SaveSyncMode::ForceUpload;
    match state {
        State::NoSave => return Ok("No cloud or local save game found.".into()),
        State::SameAge if !force_download && !force_upload => {
            return Ok(format!(
                "Save game for \"{title}\" is up to date, skipping..."
            ));
        }
        _ => {}
    }
    if (state == State::RemoteNewer && !force_upload) || force_download {
        let Some(remote) = remote else {
            return Err(Error::Other("there is no cloud save to download".into()));
        };
        if mode == SaveSyncMode::Upload {
            return Ok(format!(
                "Cloud save for \"{title}\" is newer ({}); downloading is off for this sync.",
                fmt(&remote.time)
            ));
        }
        download(epic, &account_id, app_name, &remote, save_dir, &filters).await?;
        Ok(format!(
            "Cloud save for \"{title}\" is newer ({}).\nDownloaded the cloud save.",
            fmt(&remote.time)
        ))
    } else {
        if mode == SaveSyncMode::Download {
            return Ok(format!(
                "Local save for \"{title}\" is newer; uploading is off for this sync."
            ));
        }
        let when = local.unwrap_or_else(Utc::now);
        let uploaded = upload(
            epic,
            &account_id,
            app_name,
            save_dir,
            cloud_folder.as_deref(),
            cloud_folder_mac.as_deref(),
            &filters,
            when,
        )
        .await?;
        Ok(match uploaded {
            0 => "No save files to upload.".to_string(),
            n => format!(
                "Local save for \"{title}\" is newer ({}).\nUploaded the local save game ({n} files).",
                fmt(&when)
            ),
        })
    }
}

/// The newest save manifest in the listing.
fn latest_remote(files: &Map<String, Value>) -> Option<RemoteSave> {
    files
        .iter()
        .filter(|(name, _)| name.contains(".manifest"))
        .filter_map(|(name, info)| {
            let manifest_name = name.split('/').nth(4)?.to_string();
            let time = manifest_time(&manifest_name)?;
            // legendary orders by the storage's lastModified; the name is
            // the save's own time and the two agree for saves legendary wrote
            let modified = info
                .get("lastModified")
                .and_then(Value::as_str)
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or(time);
            Some((
                modified,
                RemoteSave {
                    path: name.clone(),
                    manifest_name,
                    time,
                },
            ))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, save)| save)
}

/// `2024.01.31-20.15.00.manifest` → its UTC time.
fn manifest_time(manifest_name: &str) -> Option<DateTime<Utc>> {
    let stem = manifest_name.strip_suffix(".manifest")?;
    NaiveDateTime::parse_from_str(stem, TIME_FORMAT)
        .ok()
        .map(|t| t.and_utc())
}

/// Newest modification time of any file below `dir`.
fn newest_local(dir: &Path) -> Option<DateTime<Utc>> {
    let mut newest: Option<DateTime<Utc>> = None;
    for path in walk(dir) {
        if let Some(t) = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .map(DateTime::<Utc>::from)
        {
            newest = Some(newest.map_or(t, |n| n.max(t)));
        }
    }
    newest
}

/// Every file below `dir`, depth first.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // a download interrupted mid-swap: not part of the save
                if !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".hyper-incoming-")
                {
                    stack.push(path);
                }
            } else if path.is_file() {
                out.push(path);
            }
        }
    }
    out
}

/// `CloudIncludeList` / `CloudExcludeList`: patterns relative to the save folder.
#[derive(Clone)]
struct Filters {
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
}

impl Filters {
    fn keeps(&self, rel: &str) -> bool {
        if let Some(inc) = &self.include
            && !matches_any(rel, inc)
        {
            return false;
        }
        if let Some(exc) = &self.exclude
            && matches_any(rel, exc)
        {
            return false;
        }
        true
    }
}

/// legendary's `_filename_matches`: a folder prefix (`Saves/`), a suffix, or
/// a wildcard pattern.
fn matches_any(name: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| {
        let p = p.trim();
        !p.is_empty()
            && ((p.ends_with('/') && name.starts_with(p))
                || name.ends_with(p)
                || glob_match(p, name))
    })
}

/// `fnmatch` with `*` and `?`.
fn glob_match(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Relative path of a save file, with `/` separators as in the manifests.
fn relative(dir: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(dir)
        .ok()
        .map(|r| r.to_string_lossy().replace('\\', "/"))
}

/// Fetch the save's manifest and chunks and write its files over the save
/// folder (files matching the filters are removed first, like legendary's
/// `clean_dir`).
async fn download(
    epic: &Epic,
    account_id: &str,
    app_name: &str,
    remote: &RemoteSave,
    save_dir: &Path,
    filters: &Filters,
) -> Result<()> {
    let files = epic.cloud_files(account_id, app_name, false).await?;
    let link = |path: &str| -> Result<String> {
        files
            .get(path)
            .and_then(|f| f.get("readLink"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::Other(format!("cloud save file missing: {path}")))
    };
    let manifest_bytes = epic.fetch_url(&link(&remote.path)?).await?;
    if manifest_bytes.is_empty() {
        return Err(Error::Other("the cloud save manifest is empty".into()));
    }
    let manifest = Manifest::parse(&manifest_bytes)?;
    // chunk paths are relative to the save's folder in the storage
    let prefix: Vec<&str> = remote.path.splitn(4, '/').take(3).collect();
    let prefix = prefix.join("/");

    // every chunk first: a save with a piece missing is not applied at all
    let mut chunks: HashMap<[u32; 4], Vec<u8>> = HashMap::new();
    for ci in &manifest.chunks {
        let path = format!("{prefix}/{}", ci.path(manifest.version));
        let url = link(&path).map_err(|_| {
            Error::Other("a chunk of the cloud save is missing; nothing was changed".into())
        })?;
        let chunk = Chunk::parse(&epic.fetch_url(&url).await?, &HashMap::new())?;
        chunk.check(ci)?;
        chunks.insert(chunk.guid, chunk.data);
    }

    let mtime: std::time::SystemTime = remote.time.into();
    let (dir, filters) = (save_dir.to_path_buf(), filters.clone());
    tokio::task::spawn_blocking(move || apply_save(&dir, &manifest, &chunks, &filters, mtime))
        .await
        .map_err(|e| Error::Other(format!("save download task failed: {e}")))??;
    tracing::info!(
        app = app_name,
        save = remote.manifest_name,
        "cloud save downloaded"
    );
    Ok(())
}

/// Folder the downloaded save is assembled in before it replaces the local one.
fn incoming_dir(save_dir: &Path) -> PathBuf {
    save_dir.join(format!(".hyper-incoming-{}", std::process::id()))
}

/// Write the save's files next to the local save first (every part present,
/// every file its listed size), and only then clear the local files the
/// filters cover and move the new ones into place. A failure before the
/// swap leaves the local save as it was.
fn apply_save(
    save_dir: &Path,
    manifest: &Manifest,
    chunks: &HashMap<[u32; 4], Vec<u8>>,
    filters: &Filters,
    mtime: std::time::SystemTime,
) -> Result<()> {
    std::fs::create_dir_all(save_dir)?;
    let incoming = incoming_dir(save_dir);
    let _ = std::fs::remove_dir_all(&incoming);
    let staged = stage_save(&incoming, manifest, chunks, mtime);
    if let Err(e) = staged {
        let _ = std::fs::remove_dir_all(&incoming);
        return Err(e);
    }
    let doomed: Vec<String> = walk(save_dir)
        .iter()
        .filter(|p| !p.starts_with(&incoming))
        .filter_map(|p| relative(save_dir, p))
        .filter(|r| filters.keeps(r))
        .collect();
    super::installed::delete_filelist(save_dir, &doomed, false, true);
    let mut result = Ok(());
    for fm in &manifest.files {
        let (from, to) = (incoming.join(&fm.filename), save_dir.join(&fm.filename));
        let moved = to
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::rename(&from, &to));
        if let Err(e) = moved {
            tracing::warn!(file = fm.filename, error = %e, "save file not moved into place");
            result = Err(Error::Io(e));
        }
    }
    let _ = std::fs::remove_dir_all(&incoming);
    result
}

/// The cloud save's files under `incoming`.
fn stage_save(
    incoming: &Path,
    manifest: &Manifest,
    chunks: &HashMap<[u32; 4], Vec<u8>>,
    mtime: std::time::SystemTime,
) -> Result<()> {
    /// Preallocation cap: a size from the manifest is a claim, not a promise.
    const PREALLOC: u64 = 64 << 20;
    for fm in &manifest.files {
        if !crate::stores::is_confined_rel_path(&fm.filename) {
            return Err(Error::Other(format!(
                "cloud save file name leaves the folder: {:?}",
                fm.filename
            )));
        }
        let mut data = Vec::with_capacity(fm.file_size.min(PREALLOC) as usize);
        for part in &fm.chunk_parts {
            let chunk = chunks.get(&part.guid).ok_or_else(|| {
                Error::Other(format!(
                    "{}: a chunk of the cloud save is missing",
                    fm.filename
                ))
            })?;
            let out_of_range = || Error::Other("chunk part out of range".into());
            let end = part
                .offset
                .checked_add(part.size)
                .ok_or_else(out_of_range)? as usize;
            data.extend_from_slice(
                chunk
                    .get(part.offset as usize..end)
                    .ok_or_else(out_of_range)?,
            );
        }
        if data.len() as u64 != fm.file_size {
            return Err(Error::Other(format!(
                "{}: {} bytes, the save lists {}",
                fm.filename,
                data.len(),
                fm.file_size
            )));
        }
        let path = incoming.join(&fm.filename);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &data)?;
        if let Ok(f) = std::fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(mtime);
        }
    }
    Ok(())
}

/// Pack the save folder into chunks and a manifest named after `when`, and
/// upload them. Returns how many files went up.
#[allow(clippy::too_many_arguments)]
async fn upload(
    epic: &Epic,
    account_id: &str,
    app_name: &str,
    save_dir: &Path,
    cloud_folder: Option<&str>,
    cloud_folder_mac: Option<&str>,
    filters: &Filters,
    when: DateTime<Utc>,
) -> Result<usize> {
    // walking and reading the save folder is blocking I/O
    let packed = {
        let (dir, filters, app, account) = (
            save_dir.to_path_buf(),
            filters.clone(),
            app_name.to_string(),
            account_id.to_string(),
        );
        let (cf, cfm) = (
            cloud_folder.map(str::to_owned),
            cloud_folder_mac.map(str::to_owned),
        );
        tokio::task::spawn_blocking(move || {
            pack(
                &dir,
                &filters,
                &app,
                &account,
                cf.as_deref(),
                cfm.as_deref(),
                when,
            )
        })
        .await
        .map_err(|e| Error::Other(format!("save packing task failed: {e}")))??
    };
    let Some((file_count, chunk_count, uploads)) = packed else {
        return Ok(0);
    };

    let names: Vec<String> = uploads.iter().map(|(n, _)| n.clone()).collect();
    let links = epic
        .create_cloud_files(account_id, app_name, &names)
        .await?;
    for (name, bytes) in uploads {
        let link = links
            .get(&name)
            .and_then(|f| f.get("writeLink"))
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Other(format!("Epic gave no upload link for {name}")))?;
        epic.put_url(link, bytes).await?;
    }
    tracing::info!(
        app = app_name,
        files = file_count,
        chunks = chunk_count,
        "cloud save uploaded"
    );
    Ok(file_count)
}

/// The save folder as chunks plus its manifest, ready to upload: `(files,
/// chunks, [(storage path, bytes)])`; `None` when there is nothing to upload.
type Packed = (usize, usize, Vec<(String, Vec<u8>)>);

#[allow(clippy::too_many_arguments)]
fn pack(
    save_dir: &Path,
    filters: &Filters,
    app_name: &str,
    account_id: &str,
    cloud_folder: Option<&str>,
    cloud_folder_mac: Option<&str>,
    when: DateTime<Utc>,
) -> Result<Option<Packed>> {
    let mut paths: Vec<(String, PathBuf)> = walk(save_dir)
        .into_iter()
        .filter_map(|p| relative(save_dir, &p).map(|r| (r, p)))
        .filter(|(r, _)| filters.keeps(r))
        .collect();
    if paths.is_empty() {
        return Ok(None);
    }
    paths.sort_by_key(|(r, _)| r.to_lowercase());

    let mut manifest = Manifest {
        version: SAVE_MANIFEST_VERSION,
        meta: Meta {
            feature_level: SAVE_MANIFEST_VERSION,
            app_name: format!("{app_name}{account_id}"),
            build_version: when.format(TIME_FORMAT).to_string(),
            ..Meta::default()
        },
        ..Manifest::default()
    };
    if let Some(f) = cloud_folder {
        manifest
            .custom_fields
            .insert("CloudSaveFolder".into(), f.to_string());
    }
    if let Some(f) = cloud_folder_mac {
        manifest
            .custom_fields
            .insert("CloudSaveFolder_MAC".into(), f.to_string());
    }

    // files are laid out back to back in 1 MiB chunks
    let mut uploads: Vec<(String, Vec<u8>)> = Vec::new();
    let mut buffer: Vec<u8> = Vec::with_capacity(CHUNK_SIZE);
    let mut pending_parts: Vec<(usize, usize)> = Vec::new(); // (file index, part index) waiting for the chunk's guid
    let finish_chunk = |buffer: &mut Vec<u8>,
                        pending: &mut Vec<(usize, usize)>,
                        manifest: &mut Manifest,
                        uploads: &mut Vec<(String, Vec<u8>)>|
     -> Result<()> {
        let chunk = Chunk::new(std::mem::take(buffer))?;
        for (fi, pi) in pending.drain(..) {
            manifest.files[fi].chunk_parts[pi].guid = chunk.guid;
        }
        let bytes = chunk.write();
        let ci = ChunkInfo {
            guid: chunk.guid,
            hash: chunk.hash,
            sha: chunk.sha,
            group_num: Chunk::group_num(chunk.guid),
            window_size: CHUNK_SIZE as u32,
            file_size: bytes.len() as i64,
            ..ChunkInfo::default()
        };
        uploads.push((ci.path(SAVE_MANIFEST_VERSION), bytes));
        manifest.chunks.push(ci);
        Ok(())
    };
    for (rel, path) in &paths {
        let data = std::fs::read(path)?;
        let mut fm = FileManifest {
            filename: rel.clone(),
            sha: Sha1::digest(&data).into(),
            file_size: data.len() as u64,
            ..FileManifest::default()
        };
        let fi = manifest.files.len();
        let mut file_offset = 0usize;
        while file_offset < data.len() {
            let take = (data.len() - file_offset).min(CHUNK_SIZE - buffer.len());
            fm.chunk_parts.push(ChunkPart {
                guid: [0; 4],
                offset: buffer.len() as u32,
                size: take as u32,
                file_offset: file_offset as u64,
            });
            pending_parts.push((fi, fm.chunk_parts.len() - 1));
            buffer.extend_from_slice(&data[file_offset..file_offset + take]);
            file_offset += take;
            if buffer.len() >= CHUNK_SIZE {
                // the file's entry must exist before the chunk closes over it
                manifest.files.push(std::mem::take(&mut fm));
                finish_chunk(&mut buffer, &mut pending_parts, &mut manifest, &mut uploads)?;
                fm = manifest.files.pop().unwrap_or_default();
            }
        }
        manifest.files.push(fm);
    }
    if !buffer.is_empty() {
        finish_chunk(&mut buffer, &mut pending_parts, &mut manifest, &mut uploads)?;
    }
    let manifest_name = format!("manifests/{}.manifest", manifest.meta.build_version);
    uploads.push((manifest_name, manifest.write()));
    Ok(Some((paths.len(), manifest.chunks.len(), uploads)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_names_carry_the_time() {
        let t = manifest_time("2024.01.31-20.15.00.manifest").unwrap();
        assert_eq!(t.format(TIME_FORMAT).to_string(), "2024.01.31-20.15.00");
        assert!(manifest_time("nope").is_none());
        let files: Map<String, Value> = serde_json::from_str(r#"{
            "a/b/App/manifests/2024.01.31-20.15.00.manifest": { "lastModified": "2024-01-31T20:15:01.000Z" },
            "a/b/App/manifests/2024.02.01-09.00.00.manifest": { "lastModified": "2024-02-01T09:00:02.000Z" },
            "a/b/App/ChunksV4/01/x.chunk": {}
        }"#).unwrap();
        let latest = latest_remote(&files).unwrap();
        assert_eq!(latest.manifest_name, "2024.02.01-09.00.00.manifest");
        assert_eq!(
            latest.path,
            "a/b/App/manifests/2024.02.01-09.00.00.manifest"
        );
    }

    #[test]
    fn filters_like_legendary() {
        assert!(glob_match("*.sav", "slot1.sav"));
        assert!(glob_match("Save?/*.dat", "Save1/a.dat"));
        assert!(!glob_match("*.sav", "slot1.bak"));
        let f = Filters {
            include: Some(vec!["Saves/".into(), "*.sav".into()]),
            exclude: Some(vec!["backup".into()]),
        };
        assert!(f.keeps("Saves/x.bin"));
        assert!(f.keeps("slot.sav"));
        assert!(!f.keeps("other/x.bin"));
        assert!(!f.keeps("Saves/x.backup"), "excluded by suffix");
        assert!(matches_any("a/b.txt", &["b.txt".to_string()]));
    }

    #[test]
    fn packs_files_into_chunks_that_restore() {
        let dir = std::env::temp_dir().join(format!("hyper-epic-saves-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Slots")).unwrap();
        std::fs::write(dir.join("Slots").join("b.sav"), vec![7u8; 300]).unwrap();
        std::fs::write(dir.join("a.sav"), vec![1u8; CHUNK_SIZE + 10]).unwrap();
        // package without the network: the same loop as `upload`, checked by assembling again
        let mut paths: Vec<(String, PathBuf)> = walk(&dir)
            .into_iter()
            .filter_map(|p| relative(&dir, &p).map(|r| (r, p)))
            .collect();
        paths.sort_by_key(|(r, _)| r.to_lowercase());
        assert_eq!(
            paths.iter().map(|(r, _)| r.as_str()).collect::<Vec<_>>(),
            ["a.sav", "Slots/b.sav"]
        );
        assert!(newest_local(&dir).is_some());

        // pack for real, then restore into another folder from the uploads
        let all = Filters {
            include: None,
            exclude: None,
        };
        let (files, chunk_count, uploads) = pack(
            &dir,
            &all,
            "App",
            "acc",
            Some("{AppData}/x"),
            None,
            Utc::now(),
        )
        .unwrap()
        .unwrap();
        assert_eq!((files, chunk_count), (2, 2));
        let (_, manifest_bytes) = uploads
            .iter()
            .find(|(n, _)| n.starts_with("manifests/"))
            .unwrap();
        let manifest = Manifest::parse(manifest_bytes).unwrap();
        let mut chunks = HashMap::new();
        for (name, bytes) in uploads.iter().filter(|(n, _)| n.ends_with(".chunk")) {
            let c = Chunk::parse(bytes, &HashMap::new()).unwrap();
            let ci = manifest
                .chunks
                .iter()
                .find(|ci| ci.path(manifest.version) == *name)
                .unwrap();
            c.check(ci).unwrap();
            chunks.insert(c.guid, c.data);
        }
        let target = dir.with_extension("restored");
        let _ = std::fs::remove_dir_all(&target);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("stale.sav"), b"old").unwrap();
        let mtime = std::time::SystemTime::now();

        // a chunk missing: nothing changes locally
        let mut partial = chunks.clone();
        partial.remove(&manifest.chunks[1].guid);
        assert!(apply_save(&target, &manifest, &partial, &all, mtime).is_err());
        assert_eq!(
            std::fs::read(target.join("stale.sav")).unwrap(),
            b"old",
            "local save untouched"
        );
        assert!(!incoming_dir(&target).exists(), "staging folder cleaned up");

        apply_save(&target, &manifest, &chunks, &all, mtime).unwrap();
        assert_eq!(
            std::fs::read(target.join("a.sav")).unwrap(),
            vec![1u8; CHUNK_SIZE + 10]
        );
        assert_eq!(
            std::fs::read(target.join("Slots").join("b.sav")).unwrap(),
            vec![7u8; 300]
        );
        assert!(
            !target.join("stale.sav").exists(),
            "replaced by the cloud save"
        );
        assert!(!incoming_dir(&target).exists());

        // a save whose file names leave the folder is refused before anything is touched
        let mut evil = manifest.clone();
        evil.files[0].filename = "../escape.sav".into();
        assert!(apply_save(&target, &evil, &chunks, &all, mtime).is_err());
        assert!(!dir.with_extension("escape.sav").exists() && target.join("a.sav").exists());
        let _ = std::fs::remove_dir_all(target);
        let _ = std::fs::remove_dir_all(dir);
    }
}
