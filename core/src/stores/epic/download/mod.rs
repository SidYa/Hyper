//! Install, update and repair without `legendary`: the build's manifest
//! from the CDN, a plan of what to fetch and write (`plan`), then parallel
//! chunk downloads feeding one writer that assembles the files in place,
//! with a resume file so an interrupted run picks up where it stopped.
//! Repairs hash the installed files first (`verify`) and fetch only what
//! differs. Port of legendary's `prepare_download`, `install_game` and the
//! `DLManager` / worker processes, in one process on tokio.
//!
//! Files and names are legendary's: manifests in `manifests/`, resume files
//! in `tmp/<app>.resume` / `tmp/<app>.repair`, the `installed.json` entry.

pub mod plan;
pub mod verify;

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinSet;

use super::api::Epic;
use super::chunk::{CHUNK_SIZE, Chunk};
use super::installed::{locate_checked, read_installed_raw, write_installed_raw};
use super::library::{catalog_ids, is_third_party};
use super::manifest::{ChunkInfo, Manifest};
use super::{
    clean_filename, manifest_path, manifest_path_legacy, metadata_path, read_json, write_json,
};
use crate::error::{Error, Result};
use crate::types::DownloadProgress;
use plan::{Plan, Task};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Install,
    Update,
    Repair,
}

pub struct Request {
    pub app_name: String,
    pub platform: String,
    /// Install: the folder the game's folder is created in.
    pub base_path: Option<PathBuf>,
    pub kind: Kind,
    /// Parallel chunk downloads; 0 = automatic.
    pub max_workers: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Hashing the installed files before a repair.
    Verify,
    Download,
}

pub type ProgressFn = Arc<dyn Fn(Phase, DownloadProgress) + Send + Sync>;

/// Set to stop a run; it ends with [`Error::Cancelled`] and resumes next time.
#[derive(Default)]
pub struct CancelFlag {
    pub(crate) flag: AtomicBool,
    pub(crate) notify: Notify,
}

impl CancelFlag {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// Downloaded chunks waiting in memory (cached for reuse plus in flight).
const MEMORY_CAP: u64 = 1024 * 1024 * 1024;
const MAX_RETRIES: u32 = 7;
const CHUNK_TIMEOUT: Duration = Duration::from_secs(30);
/// Backoff unit between retries of a chunk (1, 2, 4 … units).
#[cfg(not(test))]
const RETRY_UNIT: Duration = Duration::from_secs(1);
#[cfg(test)]
const RETRY_UNIT: Duration = Duration::from_millis(5);

fn blocking_failed(what: &str) -> impl FnOnce(tokio::task::JoinError) -> Error + '_ {
    move |e| Error::Other(format!("{what} task failed: {e}"))
}

pub async fn run(
    epic: &Arc<Epic>,
    req: Request,
    on_progress: ProgressFn,
    cancel: Arc<CancelFlag>,
) -> Result<()> {
    let dir = epic.config_dir.clone();
    let app = req.app_name.as_str();
    let game =
        read_json(&metadata_path(&dir, app)).ok_or_else(|| Error::GameNotFound(app.to_string()))?;
    if is_third_party(&game) {
        return Err(Error::Other(
            "this title is installed through another launcher".into(),
        ));
    }
    let platform = if game
        .pointer(&format!("/asset_infos/{}", req.platform))
        .is_some()
    {
        req.platform.clone()
    } else if game.pointer("/asset_infos/Windows").is_some() {
        tracing::warn!(
            app,
            platform = req.platform,
            "no build for the platform; using Windows"
        );
        "Windows".to_string()
    } else {
        return Err(Error::Helper {
            helper: "Epic".into(),
            message: format!("{app} has no build to install"),
        });
    };
    let is_dlc = game.pointer("/metadata/mainGameItem").is_some();
    let installed_all = read_installed_raw(&dir);
    let existing = installed_all.get(app).cloned();
    let needs_verification = existing
        .as_ref()
        .and_then(|e| e.get("needs_verification"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let repair = req.kind == Kind::Repair || (req.kind == Kind::Update && needs_verification);
    if repair && existing.is_none() {
        return Err(Error::Other(format!("{app} is not installed")));
    }
    let base_app = game
        .pointer("/metadata/mainGameItem/releaseInfo/0/appId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if is_dlc
        && existing.is_none()
        && !base_app
            .as_deref()
            .is_some_and(|b| installed_all.contains_key(b))
    {
        return Err(Error::Other("the DLC's base game is not installed".into()));
    }

    // the installed build's manifest, for patching (and for a repair); big
    // manifests take a while to read and parse, so off the runtime
    let mut old: Option<Manifest> = match existing.as_ref() {
        Some(e) => {
            let s = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or_default();
            let (primary, legacy) = (
                manifest_path(&dir, app, s("platform"), s("version")),
                manifest_path_legacy(&dir, app, s("version")),
            );
            let app = app.to_string();
            tokio::task::spawn_blocking(move || {
                let bytes = std::fs::read(primary).or_else(|_| std::fs::read(legacy)).ok()?;
                match Manifest::parse(&bytes) {
                    Ok(m) => Some(m),
                    Err(e) => {
                        tracing::warn!(app, error = %e, "installed build's manifest unreadable; no patching");
                        None
                    }
                }
            })
            .await
            .map_err(blocking_failed("manifest"))?
        }
        None => None,
    };

    let (namespace, catalog_id) = catalog_ids(&game);
    let record = epic
        .manifest_record(&namespace, &catalog_id, app, &platform)
        .await?;
    let (new_bytes, base_urls, secrets) = epic.cdn_manifest(&record).await?;
    if !base_urls.is_empty() {
        let mut updated = game.clone();
        updated["base_urls"] = json!(base_urls);
        write_json(&metadata_path(&dir, app), &updated)?;
    }
    let new = {
        let (dir, app, platform, secrets) = (
            dir.clone(),
            app.to_string(),
            platform.clone(),
            secrets.clone(),
        );
        tokio::task::spawn_blocking(move || -> Result<Manifest> {
            let mut new = Manifest::parse(&new_bytes)?;
            let was_encrypted = new.encrypted;
            new.decrypt(&secrets)?;
            let saved = manifest_path(&dir, &app, &platform, &new.meta.build_version);
            if let Some(parent) = saved.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // kept decrypted, so uninstall and repair work without asking Epic for the key
            std::fs::write(
                &saved,
                if was_encrypted {
                    new.write()
                } else {
                    new_bytes
                },
            )?;
            Ok(new)
        })
        .await
        .map_err(blocking_failed("manifest"))??
    };

    let install_dir: PathBuf = match (&existing, is_dlc) {
        (Some(e), _) => PathBuf::from(
            e.get("install_path")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        (None, true) => PathBuf::from(
            base_app
                .as_deref()
                .and_then(|b| installed_all.get(b))
                .and_then(|e| e.get("install_path"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        (None, false) => {
            let base = req
                .base_path
                .clone()
                .ok_or_else(|| Error::Other("no install folder chosen".into()))?;
            let folder = game
                .pointer("/metadata/customAttributes/FolderName/value")
                .and_then(Value::as_str)
                .unwrap_or(app);
            if !super::super::is_plain_folder_name(folder) {
                return Err(Error::Other(format!(
                    "Epic reported an unsafe install folder: {folder:?}"
                )));
            }
            std::fs::create_dir_all(&base)?;
            base.join(folder.trim())
        }
    };
    if install_dir.as_os_str().is_empty() {
        return Err(Error::Other("install folder unknown".into()));
    }
    tracing::info!(app, path = %install_dir.display(), ?req.kind, repair, "preparing Epic download");

    let tmp = dir.join("tmp");
    std::fs::create_dir_all(&tmp)?;
    let (manifest, old, resume_file) = if repair {
        // against the installed build when its manifest is known, else the latest one
        let target = match old.take() {
            Some(o) => o,
            None => {
                tracing::warn!(
                    app,
                    "no manifest for the installed build; repairing against the latest build"
                );
                new
            }
        };
        let repair_file = tmp.join(clean_filename(&format!("{app}.repair")));
        if !repair_file.exists() {
            let (m, d, rf, p, c) = (
                target.clone(),
                install_dir.clone(),
                repair_file.clone(),
                on_progress.clone(),
                cancel.clone(),
            );
            tokio::task::spawn_blocking(move || {
                let started = Instant::now();
                let progress: verify::VerifyProgress = Arc::new(move |done, total| {
                    let secs = started.elapsed().as_secs_f64().max(0.001);
                    p(
                        Phase::Verify,
                        DownloadProgress {
                            percent: if total > 0 {
                                (done as f64 / total as f64 * 100.0) as f32
                            } else {
                                0.0
                            },
                            bytes_done: done,
                            bytes_total: total,
                            speed: 0,
                            disk_speed: (done as f64 / secs) as u64,
                            eta: None,
                        },
                    )
                });
                verify::verify(&d, &m, &rf, progress, &c.flag)
            })
            .await
            .map_err(blocking_failed("verification"))??;
        }
        (target, None, repair_file)
    } else {
        (new, old, tmp.join(clean_filename(&format!("{app}.resume"))))
    };

    // the plan stats every file of the build: off the runtime as well
    let manifest = Arc::new(manifest);
    let old = old.map(Arc::new);
    let plan = {
        let (m, o, d, r) = (
            manifest.clone(),
            old.clone(),
            install_dir.clone(),
            resume_file.clone(),
        );
        tokio::task::spawn_blocking(move || {
            plan::analyse(&m, o.as_deref(), &d, Some(&r), MEMORY_CAP / 2)
        })
        .await
        .map_err(blocking_failed("planning"))?
    };
    tracing::info!(
        app,
        dl_mib = plan.dl_size / (1 << 20),
        install_mib = plan.install_size / (1 << 20),
        reuse_mib = plan.reuse_size / (1 << 20),
        added = plan.added,
        changed = plan.changed,
        unchanged = plan.unchanged,
        removed = plan.removed,
        "download planned"
    );
    if plan.dl_size > 0 || !plan.tasks.is_empty() {
        let check_dir = if install_dir.exists() {
            install_dir.clone()
        } else {
            install_dir
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default()
        };
        if let Some((free, _)) = crate::browse::free_space(&check_dir)
            && free < plan.disk_space_delta
        {
            return Err(Error::Other(format!(
                "not enough disk space: {:.2} GiB free, {:.2} GiB needed",
                free as f64 / (1u64 << 30) as f64,
                plan.disk_space_delta as f64 / (1u64 << 30) as f64
            )));
        }
        let workers = if req.max_workers > 0 {
            req.max_workers.min(32)
        } else {
            (std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                * 2)
            .min(16)
        };
        execute(
            epic,
            &manifest,
            plan,
            &install_dir,
            &base_urls,
            Arc::new(secrets),
            &resume_file,
            workers,
            on_progress,
            cancel,
        )
        .await?;
    } else {
        tracing::info!(app, "nothing to download: the install is up to date");
    }

    // the install is complete: record it, forget the resume data
    let attr = |k: &str, default: &str| {
        game.pointer(&format!("/metadata/customAttributes/{k}/value"))
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_ascii_lowercase()
    };
    let keep = |k: &str, default: Value| {
        existing
            .as_ref()
            .and_then(|e| e.get(k))
            .cloned()
            .unwrap_or(default)
    };
    let prereq = (!manifest.meta.prereq_ids.is_empty()).then(|| {
        json!({ "ids": manifest.meta.prereq_ids, "name": manifest.meta.prereq_name, "path": manifest.meta.prereq_path, "args": manifest.meta.prereq_args })
    });
    let uninstaller = (!manifest.meta.uninstall_action_path.is_empty())
        .then(|| json!({ "path": manifest.meta.uninstall_action_path, "args": manifest.meta.uninstall_action_args }));
    let entry = json!({
        "app_name": app,
        "install_path": install_dir.to_string_lossy(),
        "title": game.get("app_title").cloned().unwrap_or(Value::from(app)),
        "version": manifest.meta.build_version,
        "base_urls": base_urls,
        "can_run_offline": attr("CanRunOffline", "true") == "true",
        "egl_guid": keep("egl_guid", Value::from("")),
        "executable": manifest.meta.launch_exe,
        "install_size": manifest.disk_size(),
        "install_tags": keep("install_tags", json!([])),
        "is_dlc": is_dlc,
        "launch_parameters": manifest.meta.launch_command,
        "manifest_path": "",
        "needs_verification": false,
        "platform": platform,
        "prereq_info": prereq.unwrap_or(Value::Null),
        "uninstaller": uninstaller.unwrap_or(Value::Null),
        "requires_ot": attr("OwnershipToken", "false") == "true",
        "save_path": keep("save_path", Value::Null),
        "is_preloaded": false,
    });
    let mut all = read_installed_raw(&dir);
    all.insert(app.to_string(), entry);
    write_installed_raw(&dir, all)?;
    let _ = std::fs::remove_file(&resume_file);
    tracing::info!(
        app,
        version = manifest.meta.build_version,
        "Epic download finished"
    );
    Ok(())
}

/// Counters shared between the downloads, the writer and the reporter.
struct Shared {
    /// download index → chunk data, until its last use
    buffer: Mutex<HashMap<usize, Arc<Vec<u8>>>>,
    arrived: Condvar,
    buffered_bytes: AtomicU64,
    inflight_bytes: AtomicU64,
    memory_freed: Notify,
    bytes_downloaded: AtomicU64,
    bytes_written: AtomicU64,
    chunks_done: AtomicUsize,
    failed: Mutex<Option<String>>,
    cancel: Arc<CancelFlag>,
}

impl Shared {
    fn fail(&self, what: String) {
        if let Ok(mut f) = self.failed.lock()
            && f.is_none()
        {
            *f = Some(what);
        }
        self.cancel.cancel();
        self.arrived.notify_all();
    }

    fn failure(&self) -> Option<String> {
        self.failed.lock().ok().and_then(|f| f.clone())
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute(
    epic: &Arc<Epic>,
    manifest: &Manifest,
    plan: Plan,
    install_dir: &Path,
    base_urls: &[String],
    secrets: Arc<HashMap<String, String>>,
    resume_file: &Path,
    workers: usize,
    on_progress: ProgressFn,
    cancel: Arc<CancelFlag>,
) -> Result<()> {
    if base_urls.is_empty() {
        return Err(Error::Other(
            "Epic gave no download servers for this build".into(),
        ));
    }
    // where each chunk lives, and what it must turn out to be
    let chunk_paths: HashMap<[u32; 4], (String, ChunkInfo)> = manifest
        .chunks
        .iter()
        .map(|c| (c.guid, (c.path(manifest.version), c.clone())))
        .collect();
    let shared = Arc::new(Shared {
        buffer: Mutex::new(HashMap::new()),
        arrived: Condvar::new(),
        buffered_bytes: AtomicU64::new(0),
        inflight_bytes: AtomicU64::new(0),
        memory_freed: Notify::new(),
        bytes_downloaded: AtomicU64::new(0),
        bytes_written: AtomicU64::new(0),
        chunks_done: AtomicUsize::new(0),
        failed: Mutex::new(None),
        cancel: cancel.clone(),
    });
    let total_chunk_tasks = plan.num_chunk_tasks.max(1);
    let dl_total = plan.dl_size;
    let dl_order = plan.dl_order.clone();

    // the writer: files in order, waiting for each chunk to arrive
    let writer = {
        let (shared, dir, resume, hashes, tasks) = (
            shared.clone(),
            install_dir.to_path_buf(),
            resume_file.to_path_buf(),
            plan.file_hashes,
            plan.tasks,
        );
        tokio::task::spawn_blocking(move || write_all(&shared, &dir, &resume, &hashes, tasks))
    };

    // the downloads: in the writer's order, as many in flight as the memory cap allows
    let dispatcher = {
        let (shared, epic, base_urls) = (shared.clone(), epic.clone(), base_urls.to_vec());
        tokio::spawn(async move {
            let sem = Arc::new(Semaphore::new(workers * 2));
            let mut set: JoinSet<()> = JoinSet::new();
            for (index, guid) in dl_order.into_iter().enumerate() {
                let Some((path, info)) = chunk_paths.get(&guid).cloned() else {
                    shared.fail(format!("chunk {guid:08x?} not in the manifest"));
                    break;
                };
                let window = info.window_size as u64;
                loop {
                    if shared.cancel.is_set() {
                        break;
                    }
                    let used = shared.buffered_bytes.load(Ordering::Relaxed)
                        + shared.inflight_bytes.load(Ordering::Relaxed);
                    if used + window <= MEMORY_CAP {
                        break;
                    }
                    tokio::select! {
                        _ = shared.memory_freed.notified() => {}
                        _ = shared.cancel.notify.notified() => {}
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                    }
                }
                if shared.cancel.is_set() {
                    break;
                }
                let Ok(permit) = sem.clone().acquire_owned().await else {
                    break;
                };
                shared.inflight_bytes.fetch_add(window, Ordering::Relaxed);
                let (shared, epic, base_urls, secrets) = (
                    shared.clone(),
                    epic.clone(),
                    base_urls.clone(),
                    secrets.clone(),
                );
                set.spawn(async move {
                    let _permit = permit;
                    match fetch_chunk(&epic, &base_urls, &path, &info, &secrets, &shared).await {
                        Ok((chunk, compressed)) => {
                            let len = chunk.data.len() as u64;
                            if let Ok(mut buf) = shared.buffer.lock() {
                                buf.insert(index, Arc::new(chunk.data));
                            }
                            shared.buffered_bytes.fetch_add(len, Ordering::Relaxed);
                            shared
                                .bytes_downloaded
                                .fetch_add(compressed, Ordering::Relaxed);
                            shared.arrived.notify_all();
                        }
                        Err(e) => shared.fail(format!("chunk download failed: {e}")),
                    }
                    shared.inflight_bytes.fetch_sub(window, Ordering::Relaxed);
                });
                while let Some(_done) = set.try_join_next() {}
            }
            while set.join_next().await.is_some() {}
        })
    };

    // progress once a second until the writer is done
    let started = Instant::now();
    let mut last = (Instant::now(), 0u64, 0u64);
    let mut writer = writer;
    let outcome = loop {
        tokio::select! {
            res = &mut writer => break res,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
        let now = Instant::now();
        let (dl, wr) = (
            shared.bytes_downloaded.load(Ordering::Relaxed),
            shared.bytes_written.load(Ordering::Relaxed),
        );
        let dt = now.duration_since(last.0).as_secs_f64().max(0.001);
        let speed = ((dl.saturating_sub(last.1)) as f64 / dt) as u64;
        let disk_speed = ((wr.saturating_sub(last.2)) as f64 / dt) as u64;
        last = (now, dl, wr);
        let done = shared.chunks_done.load(Ordering::Relaxed);
        let elapsed = started.elapsed().as_secs_f64();
        let average = if elapsed > 0.0 {
            dl as f64 / elapsed
        } else {
            0.0
        };
        let eta =
            (average > 0.0 && dl < dl_total).then(|| ((dl_total - dl) as f64 / average) as u64);
        on_progress(
            Phase::Download,
            DownloadProgress {
                percent: (done as f64 / total_chunk_tasks as f64 * 100.0).min(100.0) as f32,
                bytes_done: dl.min(dl_total),
                bytes_total: dl_total,
                speed,
                disk_speed,
                eta,
            },
        );
    };
    dispatcher.abort();
    let _ = dispatcher.await;
    let written = outcome.map_err(|e| Error::Other(format!("writer task failed: {e}")))?;
    if let Some(what) = shared.failure() {
        return Err(Error::Other(what));
    }
    if cancel.is_set() {
        return Err(Error::Cancelled);
    }
    written
}

/// One chunk from the CDN, retried with backoff and across the CDNs. Only a
/// chunk that is what the manifest lists (`expected`: guid, size, SHA-1)
/// counts; anything else is a failed attempt.
async fn fetch_chunk(
    epic: &Epic,
    base_urls: &[String],
    path: &str,
    expected: &ChunkInfo,
    secrets: &HashMap<String, String>,
    shared: &Shared,
) -> Result<(Chunk, u64)> {
    let mut last = String::new();
    for attempt in 0..MAX_RETRIES {
        if shared.cancel.is_set() {
            return Err(Error::Cancelled);
        }
        if attempt > 1 {
            tokio::time::sleep(RETRY_UNIT * (1 << (attempt - 1).min(5))).await;
        }
        let base = &base_urls[(attempt as usize / 2) % base_urls.len()];
        let url = format!("{base}/{path}");
        let fetched = tokio::time::timeout(CHUNK_TIMEOUT, epic.fetch_url(&url)).await;
        match fetched {
            Ok(Ok(bytes)) => {
                match Chunk::parse(&bytes, secrets).and_then(|c| c.check(expected).map(|()| c)) {
                    Ok(chunk) => return Ok((chunk, bytes.len() as u64)),
                    Err(e) => last = format!("{e}"),
                }
            }
            Ok(Err(e)) => last = e.to_string(),
            Err(_) => last = "timed out".into(),
        }
        tracing::warn!(path, attempt, last, "chunk download failed; retrying");
    }
    Err(Error::Other(format!("{path}: {last}")))
}

/// Where a file of the build is written (case-insensitively on Linux).
/// Refuses a name that leaves the folder, wherever it came from.
fn target_path(install_dir: &Path, name: &str, tmp: bool) -> Result<PathBuf> {
    let path = locate_checked(install_dir, name, cfg!(not(windows)))?;
    Ok(if tmp {
        PathBuf::from(format!("{}.tmp", path.display()))
    } else {
        path
    })
}

/// A file being assembled, hashed as it is written so the result is
/// checked against the manifest before it counts as done.
struct OpenFile {
    file: File,
    hasher: Sha1,
}

impl OpenFile {
    fn append(&mut self, piece: &[u8]) -> std::io::Result<()> {
        self.file.write_all(piece)?;
        self.hasher.update(piece);
        Ok(())
    }
}

/// `sha1:name` into the resume file: the file is complete and verified.
fn record_done(resume_file: &Path, hash: &str, name: &str) {
    if let Ok(mut rf) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(resume_file)
    {
        let _ = writeln!(rf, "{hash}:{name}");
    }
}

fn write_all(
    shared: &Shared,
    install_dir: &Path,
    resume_file: &Path,
    hashes: &HashMap<String, String>,
    tasks: Vec<Task>,
) -> Result<()> {
    let mut current: Option<OpenFile> = None;
    let no_file = || Error::Other("chunk without an open file".into());
    for task in tasks {
        if shared.cancel.is_set() {
            return Err(Error::Cancelled);
        }
        if let Some(what) = shared.failure() {
            return Err(Error::Other(what));
        }
        match task {
            Task::Open { name, tmp } => {
                let path = target_path(install_dir, &name, tmp)?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                current = Some(OpenFile {
                    file: File::create(&path)?,
                    hasher: Sha1::new(),
                });
            }
            Task::Reuse {
                size,
                old_name,
                old_offset,
            } => {
                if size as usize > CHUNK_SIZE {
                    return Err(Error::Other(format!(
                        "{old_name}: a chunk part of {size} bytes"
                    )));
                }
                let mut src =
                    File::open(locate_checked(install_dir, &old_name, cfg!(not(windows)))?)?;
                src.seek(SeekFrom::Start(old_offset))?;
                let mut piece = vec![0u8; size as usize];
                src.read_exact(&mut piece)?;
                current.as_mut().ok_or_else(no_file)?.append(&piece)?;
                shared
                    .bytes_written
                    .fetch_add(size as u64, Ordering::Relaxed);
                shared.chunks_done.fetch_add(1, Ordering::Relaxed);
            }
            Task::Download {
                offset,
                size,
                dl_index,
                cleanup,
                ..
            } => {
                let data = wait_for_chunk(shared, dl_index, cleanup)?;
                let out_of_range = || Error::Other("chunk part out of range".into());
                let end = offset.checked_add(size).ok_or_else(out_of_range)? as usize;
                let piece = data.get(offset as usize..end).ok_or_else(out_of_range)?;
                current.as_mut().ok_or_else(no_file)?.append(piece)?;
                shared
                    .bytes_written
                    .fetch_add(size as u64, Ordering::Relaxed);
                shared.chunks_done.fetch_add(1, Ordering::Relaxed);
            }
            Task::Close { name, tmp } => {
                let open = current
                    .take()
                    .ok_or_else(|| Error::Other(format!("{name}: closed without being open")))?;
                let _ = open.file.sync_data();
                drop(open.file);
                let got = format!("{:x}", open.hasher.finalize());
                let expected = hashes
                    .get(&name)
                    .ok_or_else(|| Error::Other(format!("{name}: no hash in the plan")))?;
                if got != *expected {
                    // legendary's behaviour: the run fails, a repair fetches the file again
                    let _ = std::fs::remove_file(target_path(install_dir, &name, tmp)?);
                    return Err(Error::Other(format!("file hash mismatch: {name}")));
                }
                // a file assembled as `.tmp` is done once it replaced the old one
                if !tmp {
                    record_done(resume_file, expected, &name);
                }
            }
            Task::Rename { name } => {
                let path = target_path(install_dir, &name, false)?;
                let tmp = target_path(install_dir, &name, true)?;
                if let Err(e) = std::fs::remove_file(&path)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(e.into());
                }
                std::fs::rename(&tmp, &path)?;
                if let Some(hash) = hashes.get(&name) {
                    record_done(resume_file, hash, &name);
                }
            }
            Task::Delete { name } => {
                let path = target_path(install_dir, &name, false)?;
                if let Err(e) = std::fs::remove_file(&path)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!(file = %path.display(), error = %e, "could not delete");
                }
            }
            Task::Empty { name } => {
                let path = target_path(install_dir, &name, false)?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                File::create(&path)?;
            }
            Task::MakeExecutable { name } => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let path = target_path(install_dir, &name, false)?;
                    if let Ok(meta) = std::fs::metadata(&path) {
                        let mut perm = meta.permissions();
                        perm.set_mode(perm.mode() | 0o111);
                        let _ = std::fs::set_permissions(&path, perm);
                    }
                }
                #[cfg(not(unix))]
                let _ = name;
            }
        }
    }
    Ok(())
}

/// Block until download `index` has arrived; `cleanup` drops it afterwards
/// and lets the downloads use the memory again.
fn wait_for_chunk(shared: &Shared, index: usize, cleanup: bool) -> Result<Arc<Vec<u8>>> {
    let mut buffer = shared
        .buffer
        .lock()
        .map_err(|_| Error::Other("download buffer poisoned".into()))?;
    loop {
        if let Some(data) = buffer.get(&index) {
            let data = data.clone();
            if cleanup {
                buffer.remove(&index);
                shared
                    .buffered_bytes
                    .fetch_sub(data.len() as u64, Ordering::Relaxed);
                shared.memory_freed.notify_one();
            }
            return Ok(data);
        }
        if shared.cancel.is_set() {
            return Err(shared
                .failure()
                .map(Error::Other)
                .unwrap_or(Error::Cancelled));
        }
        let (guard, _) = shared
            .arrived
            .wait_timeout(buffer, Duration::from_millis(500))
            .map_err(|_| Error::Other("download buffer poisoned".into()))?;
        buffer = guard;
    }
}

#[cfg(test)]
mod tests {
    use super::super::manifest::{ChunkInfo, ChunkPart, FileManifest, Meta};
    use super::*;
    use sha1::Digest;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A one-file-per-request HTTP server for the chunks, on a free port.
    async fn serve(files: HashMap<String, Vec<u8>>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let files = Arc::new(files);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let files = files.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .trim_start_matches('/')
                        .to_string();
                    let resp = match files.get(&path) {
                        Some(body) => {
                            let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                            r.extend_from_slice(body);
                            r
                        }
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = sock.write_all(&resp).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        base
    }

    fn epic(dir: &Path) -> Arc<Epic> {
        let vault = Arc::new(crate::secrets::Vault::new(dir.join("vault")));
        Arc::new(Epic::new(
            crate::secrets::SecretFile::new(vault, "test", dir.join("user.json")),
            dir.to_path_buf(),
        ))
    }

    #[tokio::test]
    async fn downloads_a_build_and_resumes() {
        let root = std::env::temp_dir().join(format!("hyper-epic-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // two chunks, three files: one spans both chunks, one shares a chunk, one empty
        let data_a: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let data_b: Vec<u8> = (0..1000u32).map(|i| (i % 7) as u8).collect();
        let chunk1 = Chunk::new(data_a[..200_000].to_vec()).unwrap();
        let mut second = data_a[200_000..].to_vec();
        second.extend_from_slice(&data_b);
        let chunk2 = Chunk::new(second).unwrap();
        let ci = |c: &Chunk, bytes: &[u8]| ChunkInfo {
            guid: c.guid,
            hash: c.hash,
            sha: c.sha,
            group_num: Chunk::group_num(c.guid),
            window_size: c.data.len() as u32,
            file_size: bytes.len() as i64,
            ..ChunkInfo::default()
        };
        let (bytes1, bytes2) = (chunk1.write(), chunk2.write());
        let sha = |d: &[u8]| -> [u8; 20] { sha1::Sha1::digest(d).into() };
        let manifest = Manifest {
            version: 18,
            meta: Meta {
                build_version: "1.0".into(),
                ..Meta::default()
            },
            chunks: vec![ci(&chunk1, &bytes1), ci(&chunk2, &bytes2)],
            files: vec![
                FileManifest {
                    filename: "Bin/a.bin".into(),
                    sha: sha(&data_a),
                    chunk_parts: vec![
                        ChunkPart {
                            guid: chunk1.guid,
                            offset: 0,
                            size: 200_000,
                            file_offset: 0,
                        },
                        ChunkPart {
                            guid: chunk2.guid,
                            offset: 0,
                            size: 100_000,
                            file_offset: 200_000,
                        },
                    ],
                    file_size: 300_000,
                    ..FileManifest::default()
                },
                FileManifest {
                    filename: "b.bin".into(),
                    sha: sha(&data_b),
                    chunk_parts: vec![ChunkPart {
                        guid: chunk2.guid,
                        offset: 100_000,
                        size: 1000,
                        file_offset: 0,
                    }],
                    file_size: 1000,
                    ..FileManifest::default()
                },
                FileManifest {
                    filename: "empty.txt".into(),
                    ..FileManifest::default()
                },
            ],
            ..Manifest::default()
        };
        let mut served = HashMap::new();
        served.insert(manifest.chunks[0].path(18), bytes1);
        served.insert(manifest.chunks[1].path(18), bytes2);
        let base = serve(served).await;

        let install = root.join("Game");
        let resume = root.join("x.resume");
        let plan = plan::analyse(&manifest, None, &install, Some(&resume), MEMORY_CAP);
        assert_eq!(plan.dl_order.len(), 2);
        let epic = epic(&root);
        let reports = Arc::new(Mutex::new(Vec::new()));
        let r2 = reports.clone();
        let on_progress: ProgressFn = Arc::new(move |_, p| r2.lock().unwrap().push(p));
        let no_secrets = Arc::new(HashMap::new());
        execute(
            &epic,
            &manifest,
            plan,
            &install,
            std::slice::from_ref(&base),
            no_secrets.clone(),
            &resume,
            2,
            on_progress,
            CancelFlag::new(),
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(install.join("Bin/a.bin")).unwrap(), data_a);
        assert_eq!(std::fs::read(install.join("b.bin")).unwrap(), data_b);
        assert_eq!(
            std::fs::metadata(install.join("empty.txt")).unwrap().len(),
            0
        );
        let resume_text = std::fs::read_to_string(&resume).unwrap();
        // empty files are not recorded (legendary neither); they are just recreated
        assert!(
            resume_text.contains(":Bin/a.bin")
                && resume_text.contains(":b.bin")
                && !resume_text.contains("empty")
        );

        // a second run finds everything done
        let again = plan::analyse(&manifest, None, &install, Some(&resume), MEMORY_CAP);
        assert_eq!(again.dl_size, 0);
        assert!(
            again.tasks.iter().all(|t| matches!(t, Task::Empty { .. })),
            "{:?}",
            again.tasks
        );

        // a damaged file is fetched again after verification
        std::fs::write(install.join("b.bin"), b"broken").unwrap();
        let repair = root.join("x.repair");
        verify::verify(
            &install,
            &manifest,
            &repair,
            Arc::new(|_, _| {}),
            &AtomicBool::new(false),
        )
        .unwrap();
        let fix = plan::analyse(&manifest, None, &install, Some(&repair), MEMORY_CAP);
        assert_eq!(fix.dl_order, [chunk2.guid]);
        execute(
            &epic,
            &manifest,
            fix,
            &install,
            &[base],
            no_secrets,
            &repair,
            2,
            Arc::new(|_, _| {}),
            CancelFlag::new(),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(install.join("b.bin")).unwrap(), data_b);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_dead_cdn_fails_the_run_and_a_cancel_stops_it() {
        let root = std::env::temp_dir().join(format!("hyper-epic-dl-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let chunk = Chunk::new(vec![1u8; 10]).unwrap();
        let bytes = chunk.write();
        let manifest = Manifest {
            version: 18,
            meta: Meta::default(),
            chunks: vec![ChunkInfo {
                guid: chunk.guid,
                hash: chunk.hash,
                sha: chunk.sha,
                group_num: 0,
                window_size: 1 << 20,
                file_size: bytes.len() as i64,
                ..ChunkInfo::default()
            }],
            files: vec![FileManifest {
                filename: "a".into(),
                chunk_parts: vec![ChunkPart {
                    guid: chunk.guid,
                    offset: 0,
                    size: 10,
                    file_offset: 0,
                }],
                file_size: 10,
                ..FileManifest::default()
            }],
            ..Manifest::default()
        };
        let base = serve(HashMap::new()).await; // answers 404 to everything
        let install = root.join("Game");
        let epic = epic(&root);
        let cancel = CancelFlag::new();
        cancel.cancel();
        let plan = plan::analyse(&manifest, None, &install, None, MEMORY_CAP);
        let out = execute(
            &epic,
            &manifest,
            plan,
            &install,
            &[base],
            Arc::new(HashMap::new()),
            &root.join("r"),
            1,
            Arc::new(|_, _| {}),
            cancel,
        )
        .await;
        assert!(matches!(out, Err(Error::Cancelled)), "{out:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    fn one_file_manifest(chunk: &Chunk, bytes: &[u8], file_sha: [u8; 20]) -> Manifest {
        Manifest {
            version: 18,
            chunks: vec![ChunkInfo {
                guid: chunk.guid,
                hash: chunk.hash,
                sha: chunk.sha,
                group_num: Chunk::group_num(chunk.guid),
                window_size: chunk.data.len() as u32,
                file_size: bytes.len() as i64,
                ..ChunkInfo::default()
            }],
            files: vec![FileManifest {
                filename: "a.bin".into(),
                sha: file_sha,
                chunk_parts: vec![ChunkPart {
                    guid: chunk.guid,
                    offset: 0,
                    size: 10,
                    file_offset: 0,
                }],
                file_size: 10,
                ..FileManifest::default()
            }],
            ..Manifest::default()
        }
    }

    #[tokio::test]
    async fn a_chunk_other_than_the_manifest_lists_fails_the_run() {
        let root = std::env::temp_dir().join(format!("hyper-epic-dl-wrong-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let wanted = Chunk::new(vec![1u8; 10]).unwrap();
        let other = Chunk::new(vec![2u8; 10]).unwrap();
        let wanted_bytes = wanted.write();
        let sha: [u8; 20] = sha1::Sha1::digest([1u8; 10]).into();
        let manifest = one_file_manifest(&wanted, &wanted_bytes, sha);
        // a valid chunk file, but not the one the manifest lists, under its path
        let mut served = HashMap::new();
        served.insert(manifest.chunks[0].path(18), other.write());
        let base = serve(served).await;
        let install = root.join("Game");
        let resume = root.join("r.resume");
        let plan = plan::analyse(&manifest, None, &install, None, MEMORY_CAP);
        let out = execute(
            &epic(&root),
            &manifest,
            plan,
            &install,
            &[base],
            Arc::new(HashMap::new()),
            &resume,
            1,
            Arc::new(|_, _| {}),
            CancelFlag::new(),
        )
        .await;
        let msg = format!("{out:?}");
        assert!(out.is_err() && msg.contains("guid differs"), "{msg}");
        assert!(!resume.exists(), "nothing recorded as done");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_file_that_does_not_hash_right_is_removed_and_fails_the_run() {
        let root = std::env::temp_dir().join(format!("hyper-epic-dl-hash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let chunk = Chunk::new(vec![1u8; 10]).unwrap();
        let bytes = chunk.write();
        let manifest = one_file_manifest(&chunk, &bytes, [0xAB; 20]); // not the data's hash
        let mut served = HashMap::new();
        served.insert(manifest.chunks[0].path(18), bytes);
        let base = serve(served).await;
        let install = root.join("Game");
        let resume = root.join("r.resume");
        let plan = plan::analyse(&manifest, None, &install, None, MEMORY_CAP);
        let out = execute(
            &epic(&root),
            &manifest,
            plan,
            &install,
            &[base],
            Arc::new(HashMap::new()),
            &resume,
            1,
            Arc::new(|_, _| {}),
            CancelFlag::new(),
        )
        .await;
        assert!(
            matches!(&out, Err(Error::Other(m)) if m == "file hash mismatch: a.bin"),
            "{out:?}"
        );
        assert!(!install.join("a.bin").exists(), "the bad file is deleted");
        assert!(!resume.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    fn shared_with(chunks: Vec<Vec<u8>>) -> Shared {
        Shared {
            buffer: Mutex::new(
                chunks
                    .into_iter()
                    .enumerate()
                    .map(|(i, c)| (i, Arc::new(c)))
                    .collect(),
            ),
            arrived: Condvar::new(),
            buffered_bytes: AtomicU64::new(0),
            inflight_bytes: AtomicU64::new(0),
            memory_freed: Notify::new(),
            bytes_downloaded: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            chunks_done: AtomicUsize::new(0),
            failed: Mutex::new(None),
            cancel: CancelFlag::new(),
        }
    }

    #[test]
    fn a_patched_file_is_recorded_only_once_it_replaced_the_old_one() {
        let root = std::env::temp_dir().join(format!("hyper-epic-dl-tmp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.bin"), b"OLD--rest").unwrap();
        let resume = root.join("r.resume");
        // 5 bytes reused from the old file, 3 downloaded
        let mut want = b"OLD--".to_vec();
        want.extend_from_slice(b"new");
        let hash = format!("{:x}", sha1::Sha1::digest(&want));
        let hashes: HashMap<String, String> = [("a.bin".to_string(), hash.clone())].into();
        let shared = shared_with(vec![b"xxnewxx".to_vec()]);
        let assemble = vec![
            Task::Open {
                name: "a.bin".into(),
                tmp: true,
            },
            Task::Reuse {
                size: 5,
                old_name: "a.bin".into(),
                old_offset: 0,
            },
            Task::Download {
                guid: [0; 4],
                offset: 2,
                size: 3,
                dl_index: 0,
                cleanup: false,
            },
            Task::Close {
                name: "a.bin".into(),
                tmp: true,
            },
        ];
        write_all(&shared, &root, &resume, &hashes, assemble).unwrap();
        assert!(!resume.exists(), "not done while it is still the .tmp");
        assert_eq!(std::fs::read(root.join("a.bin.tmp")).unwrap(), want);
        write_all(
            &shared,
            &root,
            &resume,
            &hashes,
            vec![Task::Rename {
                name: "a.bin".into(),
            }],
        )
        .unwrap();
        assert_eq!(std::fs::read(root.join("a.bin")).unwrap(), want);
        assert_eq!(
            std::fs::read_to_string(&resume).unwrap(),
            format!("{hash}:a.bin\n")
        );
        // names that leave the folder are refused by every task
        let out = write_all(
            &shared,
            &root,
            &resume,
            &hashes,
            vec![Task::Empty {
                name: "../escape".into(),
            }],
        );
        assert!(out.is_err() && !root.parent().unwrap().join("escape").exists());
        let out = write_all(
            &shared,
            &root,
            &resume,
            &hashes,
            vec![Task::Download {
                guid: [0; 4],
                offset: u32::MAX,
                size: 2,
                dl_index: 0,
                cleanup: false,
            }],
        );
        assert!(
            out.is_err(),
            "offset + size overflow is an error, not a panic"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
