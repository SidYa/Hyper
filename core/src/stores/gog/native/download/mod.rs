//! `gogdl download | update | repair` in one process: the build's meta and
//! depot manifests, a diff against the installed build's manifest, the
//! plan (`plan`), then parallel chunk downloads feeding one writer, with
//! gogdl's resume file so an interrupted run picks up where it stopped. A
//! repair hashes the installed files first (`verify`) and fetches only what
//! differs. Port of gogdl's `dl/managers/{v1,v2,dependencies}.py` and
//! `task_executor.py`.
//!
//! Not ported: xdelta patches (gogdl falls back to chunk updates without
//! them, so do we). Redistributables that live in the game folder are part
//! of the download; the system-wide ones (`__redist`) are `super::redist`.

pub mod plan;
pub mod verify;

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::{Notify, RwLock, Semaphore};
use tokio::task::JoinSet;

use super::manifest::{self, Diff, GameFile, Manifest, REDIST_PRODUCT};
use super::{Client, GOG_CDN, endpoint_url, galaxy_path, id_string, md5_hex, target_build};
use crate::error::{Error, Result};
use crate::stores::epic::download::CancelFlag;
use crate::types::DownloadProgress;
use plan::{Download, Plan, Task};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Install,
    Update,
    Repair,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dlcs {
    /// Every owned DLC.
    All,
    None,
    /// These, when owned.
    These(Vec<String>),
}

pub struct Request {
    pub app_name: String,
    pub platform: String,
    pub kind: Kind,
    /// Install: the folder the game's folder is created in.
    pub base_path: Option<PathBuf>,
    /// Update / repair: the game folder.
    pub game_dir: Option<PathBuf>,
    /// Where support files (setup scripts) go.
    pub support_dir: PathBuf,
    /// Install language (code or name); the build's default otherwise.
    pub language: Option<String>,
    pub dlcs: Dlcs,
    /// Repair: the installed build; install / update: a chosen (pinned) build.
    pub build_id: Option<String>,
    /// GOG branch (`None`: public builds).
    pub branch: Option<String>,
    /// Password of a private branch.
    pub password: Option<String>,
    /// Parallel chunk downloads; 0 = automatic.
    pub max_workers: usize,
}

/// What ends up installed.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub game_dir: PathBuf,
    pub build_id: String,
    pub version_name: String,
    pub language: String,
    pub dlcs: Vec<String>,
    pub folder_name: String,
    /// The build's branch (`None`: public).
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Verify,
    Download,
}

pub type ProgressFn = Arc<dyn Fn(Phase, DownloadProgress) + Send + Sync>;

/// Downloaded chunks waiting in memory (cached for reuse plus in flight).
pub(super) const MEMORY_CAP: u64 = 1024 * 1024 * 1024;
const MAX_RETRIES: u32 = 7;
/// Idle limit per read of a chunk body (the client has no total timeout: a
/// 20 MiB v1 piece on a slow link takes as long as it takes).
const CHUNK_READ_TIMEOUT: Duration = Duration::from_secs(60);
pub const RESUME_FILE: &str = ".gogdl-resume";
/// gogdl's chunk cache folder: never created here (chunks stay in memory),
/// only removed when gogdl left one behind.
pub(super) const LEFTOVER_CACHE_DIR: &str = ".gogdl-download-cache";

/// Where a file of the build is written (case-insensitively on Linux). The
/// path must be confined to `root`: the manifests are checked when they
/// are read, this is the last line.
pub(super) fn target(root: &Path, rel: &str, tmp: bool) -> Result<PathBuf> {
    let path = crate::stores::epic::installed::locate_checked(root, rel, cfg!(not(windows)))?;
    if !path.starts_with(root) {
        return Err(Error::Other(format!(
            "GOG file path resolved outside the game folder: {rel:?}"
        )));
    }
    Ok(if tmp {
        PathBuf::from(format!("{}.tmp", path.display()))
    } else {
        path
    })
}

/// Files of the game-folder redistributables a build needs (gogdl's
/// `DependenciesManager.get(return_files=True)`): the `__redist` ones are
/// installed system-wide by Galaxy and skipped.
pub(super) async fn redist_files(
    client: &Client,
    repo: &Value,
    ids: &[String],
    game_dir_only: bool,
) -> Result<Vec<GameFile>> {
    let mut out = Vec::new();
    for depot in repo
        .get("depots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = depot
            .get("dependencyId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !ids.iter().any(|x| x == id) {
            continue;
        }
        let system = depot
            .pointer("/executable/path")
            .and_then(Value::as_str)
            .is_some_and(|p| p.starts_with("__redist"));
        if system == game_dir_only {
            continue;
        }
        let Some(manifest) = depot.get("manifest").and_then(Value::as_str) else {
            continue;
        };
        let url = format!(
            "{GOG_CDN}/content-system/v2/dependencies/meta/{}",
            galaxy_path(manifest)?
        );
        let doc = client.get_zlib_json(&url, false).await?;
        for item in doc
            .pointer("/depot/items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if item.get("type").and_then(Value::as_str) == Some("DepotFile") {
                out.push(manifest::depot_file(item, REDIST_PRODUCT)?);
            }
        }
    }
    Ok(out)
}

/// The saved manifest of the installed build, when there is a readable one.
async fn saved_manifest(manifest_file: &Path, app: &str) -> Option<Manifest> {
    let text = tokio::fs::read_to_string(manifest_file).await.ok()?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    match Manifest::from_saved(value) {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::warn!(app, error = %e, "installed build's manifest unreadable");
            None
        }
    }
}

pub async fn run(
    client: &Arc<Client>,
    req: Request,
    on_progress: ProgressFn,
    cancel: Arc<CancelFlag>,
) -> Result<Outcome> {
    let app = req.app_name.as_str();
    if !super::is_product_id(app) {
        return Err(Error::Other(format!("not a GOG product id: {app:?}")));
    }
    let manifest_file = crate::paths::gogdl_manifests_dir().join(app);
    let old = saved_manifest(&manifest_file, app).await;

    let builds = client
        .builds_with_password(app, &req.platform, req.password.as_deref())
        .await?;
    let build = target_build(&builds, req.build_id.as_deref(), req.branch.as_deref())
        .cloned()
        .ok_or_else(|| Error::Other(format!("GOG has no {} build of {app}", req.platform)))?;
    let version_name = build
        .get("version_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let repair = req.kind == Kind::Repair;
    let (mut manifest, mut old) = if repair {
        let Some(saved) = old else {
            return Err(Error::Other(format!(
                "no manifest of the installed build of {app}; reinstall the game"
            )));
        };
        (saved, None)
    } else {
        let link = super::meta_url(&build)
            .ok_or_else(|| Error::Other("the build has no manifest link".into()))?;
        let meta = client.get_zlib_json(link, false).await?;
        let dlcs = match &req.dlcs {
            Dlcs::None => Vec::new(),
            Dlcs::All => super::info::owned_dlcs(client, &meta, app, None).await?,
            Dlcs::These(ids) => super::info::owned_dlcs(client, &meta, app, Some(ids)).await?,
        };
        let default_lang = if meta.get("version").and_then(Value::as_u64) == Some(1) {
            "English"
        } else {
            "en-US"
        };
        let language = req
            .language
            .clone()
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| default_lang.to_string());
        (Manifest::new(meta, &req.platform, &language, dlcs)?, old)
    };

    let game_dir: PathBuf = match (&req.game_dir, &req.base_path) {
        (Some(dir), _) => dir.clone(),
        (None, Some(base)) => {
            let folder = manifest.install_directory.trim();
            if !crate::stores::is_plain_folder_name(folder) {
                return Err(Error::Other(format!(
                    "GOG reported an unsafe install folder: {folder:?}"
                )));
            }
            base.join(folder)
        }
        (None, None) => return Err(Error::Other("no install folder chosen".into())),
    };
    let support_dir = req.support_dir.clone();
    tracing::info!(app, path = %game_dir.display(), ?req.kind, generation = manifest.version, "preparing GOG download");

    manifest.fetch_files(client).await?;
    if let Some(o) = old.as_mut() {
        o.fetch_files(client).await?;
    }
    let mut diff = manifest::compare(&manifest, old.as_ref());

    // redistributables that live in the game folder
    let repo = if manifest.dependencies.is_empty()
        && old.as_ref().is_none_or(|o| o.dependencies.is_empty())
    {
        None
    } else {
        match client.dependencies_repo().await {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(app, error = %e, "GOG dependencies repository unavailable; redistributables skipped");
                None
            }
        }
    };
    if let Some(repo) = &repo {
        if let Some(o) = &old {
            let removed: Vec<String> = o
                .dependencies
                .iter()
                .filter(|d| !manifest.dependencies.contains(d))
                .cloned()
                .collect();
            if !removed.is_empty() {
                diff.removed_redist = redist_files(client, repo, &removed, true).await?;
            }
        }
        diff.redist = redist_files(client, repo, &manifest.dependencies, true).await?;
    }
    tracing::info!(app, diff = diff.summary(), "GOG manifest diff");

    let outcome = Outcome {
        game_dir: game_dir.clone(),
        build_id: if manifest.version == 1 {
            build
                .get("legacy_build_id")
                .and_then(id_string)
                .unwrap_or_default()
        } else {
            manifest.build_id.clone()
        },
        version_name,
        language: manifest.language.clone(),
        dlcs: manifest.dlcs.iter().map(|d| d.id.clone()).collect(),
        folder_name: manifest.install_directory.clone(),
        branch: build
            .get("branch")
            .and_then(Value::as_str)
            .map(str::to_owned),
    };
    let save_manifest = async {
        if let Some(parent) = manifest_file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&manifest_file, manifest.to_json().to_string()).await?;
        Ok::<(), Error>(())
    };
    if diff.is_empty() {
        tracing::info!(app, "nothing to do: the install is up to date");
        save_manifest.await?;
        return Ok(outcome);
    }

    let resume_file = game_dir.join(RESUME_FILE);
    if repair {
        // the verify pass decides what is good; an old resume file must not
        let _ = tokio::fs::remove_file(&resume_file).await;
        // only what is missing or damaged is fetched again
        let (files, links, gd, sd, p, c) = (
            diff.new.clone(),
            diff.links.clone(),
            game_dir.clone(),
            support_dir.clone(),
            on_progress.clone(),
            cancel.clone(),
        );
        let redist = diff.redist.clone();
        let (bad_files, bad_redist) = tokio::task::spawn_blocking(move || {
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
            let bad_files = verify::broken(&files, &gd, &sd, progress.clone(), &c.flag)?;
            let bad_redist = verify::broken(&redist, &gd, &sd, progress, &c.flag)?;
            Ok::<_, Error>((bad_files, bad_redist))
        })
        .await
        .map_err(|e| Error::Other(format!("verification task failed: {e}")))??;
        let repaired = Diff {
            new: bad_files.iter().map(|i| diff.new[*i].clone()).collect(),
            redist: bad_redist.iter().map(|i| diff.redist[*i].clone()).collect(),
            links: links
                .into_iter()
                .filter(|l| !target(&game_dir, &l.path, false).is_ok_and(|p| p.exists()))
                .collect(),
            ..Diff::default()
        };
        if repaired.new.is_empty() && repaired.redist.is_empty() {
            tracing::info!(app, "all files look good");
            return Ok(outcome);
        }
        tracing::info!(
            app,
            broken = repaired.new.len() + repaired.redist.len(),
            "broken files found; repairing"
        );
        diff = repaired;
    }

    let plan = {
        let (diff, gd, sd, rf) = (
            std::mem::take(&mut diff),
            game_dir.clone(),
            support_dir.clone(),
            resume_file.clone(),
        );
        tokio::task::spawn_blocking(move || plan::analyse(&diff, &gd, &sd, &rf, MEMORY_CAP / 2))
            .await
            .map_err(|e| Error::Other(format!("planning task failed: {e}")))??
    };
    tracing::info!(
        app,
        dl_mib = plan.download_size >> 20,
        write_mib = plan.disk_size >> 20,
        resumed = plan.resumed,
        tasks = plan.tasks.len(),
        "download planned"
    );
    let check_dir = if game_dir.exists() {
        game_dir.clone()
    } else {
        game_dir.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    if let Some((free, _)) = crate::browse::free_space(&check_dir)
        && free < plan.disk_delta
    {
        return Err(Error::Other(format!(
            "not enough disk space: {:.2} GiB free, {:.2} GiB needed",
            free as f64 / (1u64 << 30) as f64,
            plan.disk_delta as f64 / (1u64 << 30) as f64
        )));
    }
    {
        let (gd, dirs) = (game_dir.clone(), manifest.dirs.clone());
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&gd)?;
            for d in &dirs {
                let _ = std::fs::create_dir_all(target(&gd, d, false)?);
            }
            Ok::<(), Error>(())
        })
        .await
        .map_err(|e| Error::Other(format!("folder task failed: {e}")))??;
    }

    // CDN endpoints per product; refreshed when they expire mid-download
    let links = Arc::new(Links::new(
        client.clone(),
        manifest.version,
        format!("/{}/{}/", manifest.platform, manifest.timestamp),
    ));
    let mut products: Vec<String> = plan.downloads.iter().map(|d| d.product.clone()).collect();
    products.sort();
    products.dedup();
    for p in &products {
        links.refresh(p).await?;
    }

    let workers = if req.max_workers > 0 {
        req.max_workers.min(32)
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(16)
    };
    execute(
        links,
        plan,
        &game_dir,
        &support_dir,
        &resume_file,
        workers,
        on_progress,
        cancel,
    )
    .await?;

    save_manifest.await?;
    let _ = tokio::fs::remove_file(&resume_file).await;
    let _ = tokio::fs::remove_dir_all(game_dir.join(LEFTOVER_CACHE_DIR)).await;
    tracing::info!(app, build = outcome.build_id, "GOG download finished");
    Ok(outcome)
}

/// Secure links per product, with a one-at-a-time refresh.
pub(super) struct Links {
    client: Arc<Client>,
    generation: u8,
    /// v1: `/<platform>/<timestamp>/`
    v1_path: String,
    map: RwLock<HashMap<String, (Instant, Vec<Value>)>>,
    refreshing: tokio::sync::Mutex<()>,
}

impl Links {
    pub(super) fn new(client: Arc<Client>, generation: u8, v1_path: String) -> Self {
        Self {
            client,
            generation,
            v1_path,
            map: RwLock::new(HashMap::new()),
            refreshing: tokio::sync::Mutex::new(()),
        }
    }

    async fn get(&self, product: &str) -> Vec<Value> {
        self.map
            .read()
            .await
            .get(product)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    /// New endpoints for `product`; a refresh done moments ago by another
    /// worker is reused.
    pub(super) async fn refresh(&self, product: &str) -> Result<()> {
        let _one = self.refreshing.lock().await;
        if self
            .map
            .read()
            .await
            .get(product)
            .is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(10))
        {
            return Ok(());
        }
        let urls = if product == REDIST_PRODUCT {
            self.client.dependency_link().await?
        } else if self.generation == 1 {
            self.client.secure_link(product, &self.v1_path, 1).await?
        } else {
            self.client.secure_link(product, "/", 2).await?
        };
        self.map
            .write()
            .await
            .insert(product.to_string(), (Instant::now(), urls));
        Ok(())
    }
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
pub(super) async fn execute(
    links: Arc<Links>,
    plan: Plan,
    game_dir: &Path,
    support_dir: &Path,
    resume_file: &Path,
    workers: usize,
    on_progress: ProgressFn,
    cancel: Arc<CancelFlag>,
) -> Result<()> {
    let shared = Arc::new(Shared {
        buffer: Mutex::new(HashMap::new()),
        arrived: Condvar::new(),
        buffered_bytes: AtomicU64::new(0),
        inflight_bytes: AtomicU64::new(0),
        memory_freed: Notify::new(),
        bytes_downloaded: AtomicU64::new(0),
        bytes_written: AtomicU64::new(0),
        failed: Mutex::new(None),
        cancel: cancel.clone(),
    });
    let (dl_total, disk_total) = (plan.download_size, plan.disk_size.max(1));
    let downloads = plan.downloads;

    // the writer: files in order, waiting for each chunk to arrive
    let writer = {
        let (shared, gd, sd, resume, hashes, tasks) = (
            shared.clone(),
            game_dir.to_path_buf(),
            support_dir.to_path_buf(),
            resume_file.to_path_buf(),
            plan.hashes,
            plan.tasks,
        );
        tokio::task::spawn_blocking(move || write_all(&shared, &gd, &sd, &resume, &hashes, tasks))
    };

    // the downloads: in the writer's order, as many in flight as the memory cap allows
    let dispatcher = {
        let (shared, links) = (shared.clone(), links.clone());
        tokio::spawn(async move {
            let sem = Arc::new(Semaphore::new(workers * 2));
            let mut set: JoinSet<()> = JoinSet::new();
            for (index, dl) in downloads.into_iter().enumerate() {
                let window = dl.size;
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
                let (shared, links) = (shared.clone(), links.clone());
                set.spawn(async move {
                    let _permit = permit;
                    match fetch(&links, &dl, &shared).await {
                        Ok((data, compressed)) => {
                            let len = data.len() as u64;
                            if let Ok(mut buf) = shared.buffer.lock() {
                                buf.insert(index, Arc::new(data));
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
        let elapsed = started.elapsed().as_secs_f64();
        let average = if elapsed > 0.0 {
            wr as f64 / elapsed
        } else {
            0.0
        };
        let eta =
            (average > 0.0 && wr < disk_total).then(|| ((disk_total - wr) as f64 / average) as u64);
        on_progress(
            Phase::Download,
            DownloadProgress {
                percent: (wr as f64 / disk_total as f64 * 100.0).min(100.0) as f32,
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

/// The `Range` header of a v1 piece; an overflowing range is a bad manifest.
fn range_header(offset: u64, size: u64) -> Result<String> {
    let end = offset
        .checked_add(size)
        .and_then(|e| e.checked_sub(1))
        .ok_or_else(|| Error::Other("v1 piece range overflows".into()))?;
    Ok(format!("bytes={offset}-{end}"))
}

/// The chunk's body, read piece by piece with an idle timeout per read, at
/// most `dl.compressed_size` bytes (one more and it is refused).
async fn read_body(mut resp: reqwest::Response, expected: u64) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expected.min(MEMORY_CAP) as usize);
    loop {
        let piece = tokio::time::timeout(CHUNK_READ_TIMEOUT, resp.chunk())
            .await
            .map_err(|_| Error::Other("timed out".into()))?
            .map_err(super::net)?;
        let Some(piece) = piece else { break };
        if out.len() as u64 + piece.len() as u64 > expected {
            return Err(Error::Other(format!(
                "chunk is longer than its {expected} bytes"
            )));
        }
        out.extend_from_slice(&piece);
    }
    Ok(out)
}

/// One download from the CDN: (bytes to write, bytes transferred). Retried
/// with backoff across the endpoints; an expired link is renewed.
async fn fetch(links: &Links, dl: &Download, shared: &Shared) -> Result<(Vec<u8>, u64)> {
    let mut last = String::new();
    for attempt in 0..MAX_RETRIES {
        if shared.cancel.is_set() {
            return Err(Error::Cancelled);
        }
        if attempt > 1 {
            tokio::time::sleep(Duration::from_secs(1 << (attempt - 1).min(5))).await;
        }
        let endpoints = links.get(&dl.product).await;
        if endpoints.is_empty() {
            links.refresh(&dl.product).await?;
            continue;
        }
        let endpoint = &endpoints[attempt as usize % endpoints.len()];
        let suffix = if dl.v1_range.is_some() {
            "/main.bin".to_string()
        } else {
            format!("/{}", galaxy_path(&dl.key)?)
        };
        let Some(url) = endpoint_url(endpoint, &suffix) else {
            last = "secure link endpoint unusable".into();
            links.refresh(&dl.product).await?;
            continue;
        };
        let mut req = links.client.http.get(&url);
        if let Some((offset, size)) = dl.v1_range {
            req = req.header("Range", range_header(offset, size)?);
        }
        let outcome = async {
            let resp = req.send().await.map_err(super::net)?;
            let status = resp.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(Error::Other(format!("HTTP {status}: link expired")));
            }
            if !status.is_success() {
                return Err(Error::Other(format!("HTTP {status}")));
            }
            read_body(resp, dl.compressed_size).await
        }
        .await;
        match outcome {
            Ok(bytes) => match finish(dl, &bytes) {
                Ok(data) => return Ok((data, bytes.len() as u64)),
                Err(e) => last = e.to_string(),
            },
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => {
                last = e.to_string();
                if last.contains("link expired") {
                    links.refresh(&dl.product).await?;
                }
            }
        }
        tracing::warn!(
            key = dl.key,
            attempt,
            last,
            "GOG chunk download failed; retrying"
        );
    }
    Err(Error::Other(format!("{}: {last}", dl.key)))
}

/// Check and unpack a downloaded chunk.
fn finish(dl: &Download, bytes: &[u8]) -> Result<Vec<u8>> {
    if let Some((_, size)) = dl.v1_range {
        if bytes.len() as u64 != size {
            return Err(Error::Other(format!(
                "got {} bytes, expected {size}",
                bytes.len()
            )));
        }
        return Ok(bytes.to_vec());
    }
    if !md5_hex(bytes).eq_ignore_ascii_case(&dl.key) {
        return Err(Error::Other(
            "compressed chunk does not match its MD5".into(),
        ));
    }
    let data =
        super::inflate(bytes, dl.size).map_err(|e| Error::Other(format!("chunk inflate: {e}")))?;
    if data.len() as u64 != dl.size {
        return Err(Error::Other(format!(
            "chunk is {} bytes, expected {}",
            data.len(),
            dl.size
        )));
    }
    if let Some(md5) = &dl.md5
        && !md5_hex(&data).eq_ignore_ascii_case(md5)
    {
        return Err(Error::Other("chunk does not match its MD5".into()));
    }
    Ok(data)
}

/// gogdl's resume line for a finished file: `hash:support:path`.
fn record_done(
    resume_file: &Path,
    hashes: &HashMap<String, (String, bool)>,
    path: &str,
    support: bool,
) {
    if let Some((hash, _)) = hashes.get(&path.to_lowercase())
        && let Ok(mut rf) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(resume_file)
    {
        let _ = writeln!(rf, "{hash}:{}:{path}", if support { "support" } else { "" });
    }
}

fn write_all(
    shared: &Shared,
    game_dir: &Path,
    support_dir: &Path,
    resume_file: &Path,
    hashes: &HashMap<String, (String, bool)>,
    tasks: Vec<Task>,
) -> Result<()> {
    let root = |support: bool| if support { support_dir } else { game_dir };
    let mut current: Option<File> = None;
    let mut buf = vec![0u8; 1 << 20];
    for task in tasks {
        if shared.cancel.is_set() {
            return Err(Error::Cancelled);
        }
        if let Some(what) = shared.failure() {
            return Err(Error::Other(what));
        }
        match task {
            Task::Open { path, support, tmp } => {
                let p = target(root(support), &path, tmp)?;
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                current = Some(File::create(&p)?);
            }
            Task::Chunk { dl_index, cleanup } => {
                let data = wait_for_chunk(shared, dl_index, cleanup)?;
                current
                    .as_mut()
                    .ok_or_else(|| Error::Other("chunk without an open file".into()))?
                    .write_all(&data)?;
                shared
                    .bytes_written
                    .fetch_add(data.len() as u64, Ordering::Relaxed);
            }
            Task::Copy {
                from,
                from_support,
                offset,
                size,
            } => {
                let mut src = File::open(target(root(from_support), &from, false)?)?;
                src.seek(SeekFrom::Start(offset))?;
                let out = current
                    .as_mut()
                    .ok_or_else(|| Error::Other("copy without an open file".into()))?;
                let mut left = size;
                while left > 0 {
                    let n = left.min(buf.len() as u64) as usize;
                    src.read_exact(&mut buf[..n])?;
                    out.write_all(&buf[..n])?;
                    left -= n as u64;
                }
                shared.bytes_written.fetch_add(size, Ordering::Relaxed);
            }
            Task::Close { path, support, tmp } => {
                if let Some(f) = current.take() {
                    f.sync_data().ok();
                }
                // a `.tmp` file is finished only once it has replaced the old one
                if !tmp {
                    record_done(resume_file, hashes, &path, support);
                }
            }
            Task::Rename { path, support } => {
                let p = target(root(support), &path, false)?;
                let tmp = target(root(support), &path, true)?;
                if let Err(e) = std::fs::remove_file(&p)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(e.into());
                }
                std::fs::rename(&tmp, &p)?;
                record_done(resume_file, hashes, &path, support);
            }
            Task::Delete { path, support } => {
                let p = target(root(support), &path, false)?;
                if let Err(e) = std::fs::remove_file(&p)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!(file = %p.display(), error = %e, "could not delete");
                }
            }
            Task::Empty { path, support } => {
                let p = target(root(support), &path, false)?;
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                File::create(&p)?;
            }
            Task::CopyFile {
                path,
                support,
                from,
                from_support,
            } => {
                let p = target(root(support), &path, false)?;
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let n = std::fs::copy(target(root(from_support), &from, false)?, &p)?;
                shared.bytes_written.fetch_add(n, Ordering::Relaxed);
            }
            Task::MakeExecutable { path, support } => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let p = target(root(support), &path, false)?;
                    if let Ok(meta) = std::fs::metadata(&p) {
                        let mut perm = meta.permissions();
                        perm.set_mode(perm.mode() | 0o111);
                        let _ = std::fs::set_permissions(&p, perm);
                    }
                }
                #[cfg(not(unix))]
                let _ = (path, support);
            }
            Task::Symlink {
                path,
                target: link_target,
            } => {
                #[cfg(unix)]
                {
                    // both ends stay in the game folder
                    let p = target(game_dir, &path, false)?;
                    let to = target(game_dir, &link_target, false)?;
                    let _ = std::fs::remove_file(&p);
                    if let Err(e) = std::os::unix::fs::symlink(to, &p) {
                        tracing::warn!(file = %p.display(), error = %e, "could not create the link");
                    }
                }
                #[cfg(not(unix))]
                tracing::warn!(path, target = link_target, "depot link skipped on Windows");
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
    use super::super::manifest::{ChunkRef, Diff};
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A tiny HTTP server for chunks (with `Range` for `main.bin`).
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
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .split('?')
                        .next()
                        .unwrap_or("/")
                        .trim_start_matches('/')
                        .to_string();
                    // hyper sends header names in lower case
                    let range = req
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("range: bytes=")
                                .map(str::to_owned)
                        })
                        .and_then(|r| {
                            let (a, b) = r.trim().split_once('-')?;
                            Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
                        });
                    let resp = match files.get(&path) {
                        Some(body) => {
                            let (code, slice) = match range {
                                Some((a, b)) => ("206 Partial Content", &body[a..=b.min(body.len() - 1)]),
                                None => ("200 OK", &body[..]),
                            };
                            let mut r = format!("HTTP/1.1 {code}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", slice.len()).into_bytes();
                            r.extend_from_slice(slice);
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

    fn deflate(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    fn client(dir: &Path) -> Arc<Client> {
        let vault = Arc::new(crate::secrets::Vault::new(dir.join("vault")));
        let user = crate::secrets::SecretFile::new(vault, "test", dir.join("auth.json"));
        Arc::new(Client::new(user, reqwest::Client::new()))
    }

    async fn links_for(client: Arc<Client>, base: &str, generation: u8) -> Arc<Links> {
        let links = Links::new(client, generation, "/windows/1/".into());
        let ep = serde_json::json!({ "url_format": format!("{base}/store{{path}}"), "parameters": { "path": "" } });
        links
            .map
            .write()
            .await
            .insert("100".into(), (Instant::now(), vec![ep.clone()]));
        links.map.write().await.insert(
            "redist".into(),
            (
                Instant::now(),
                vec![serde_json::json!({ "url": format!("{base}/redist") })],
            ),
        );
        Arc::new(links)
    }

    #[tokio::test]
    async fn downloads_v2_chunks_with_reuse_and_resumes() {
        let root = std::env::temp_dir().join(format!("hyper-gog-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let data_x: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let data_y: Vec<u8> = (0..1000u32).map(|i| (i % 7) as u8).collect();
        let (cx, cy) = (deflate(&data_x), deflate(&data_y));
        let chunk = |raw: &[u8], packed: &[u8]| ChunkRef {
            compressed_md5: md5_hex(packed),
            md5: md5_hex(raw),
            size: raw.len() as u64,
            compressed_size: packed.len() as u64,
        };
        let a = GameFile {
            product: "100".into(),
            path: "Bin/a.bin".into(),
            size: 301_000,
            chunks: vec![chunk(&data_x, &cx), chunk(&data_y, &cy)],
            md5: Some("a".into()),
            ..GameFile::default()
        };
        let b = GameFile {
            product: "100".into(),
            path: "b.bin".into(),
            size: 1000,
            chunks: vec![chunk(&data_y, &cy)],
            ..GameFile::default()
        };
        let redist = GameFile {
            product: "redist".into(),
            path: "__support/x.dll".into(),
            size: 1000,
            chunks: vec![chunk(&data_y, &cy)],
            ..GameFile::default()
        };
        let mut served = HashMap::new();
        served.insert(
            format!("store/{}", galaxy_path(&md5_hex(&cx)).unwrap()),
            cx.clone(),
        );
        served.insert(
            format!("store/{}", galaxy_path(&md5_hex(&cy)).unwrap()),
            cy.clone(),
        );
        served.insert(
            format!("redist/{}", galaxy_path(&md5_hex(&cy)).unwrap()),
            cy.clone(),
        );
        let base = serve(served).await;
        let client = client(&root);
        let links = links_for(client.clone(), &base, 2).await;

        let game = root.join("Game");
        let support = root.join("support");
        let resume = game.join(RESUME_FILE);
        std::fs::create_dir_all(&game).unwrap();
        let diff = Diff {
            new: vec![a.clone(), b.clone()],
            redist: vec![redist.clone()],
            ..Diff::default()
        };
        let plan = plan::analyse(&diff, &game, &support, &resume, MEMORY_CAP).unwrap();
        assert_eq!(plan.downloads.len(), 2, "the shared chunk is fetched once");
        let progress: ProgressFn = Arc::new(|_, _| {});
        execute(
            links.clone(),
            plan,
            &game,
            &support,
            &resume,
            2,
            progress.clone(),
            CancelFlag::new(),
        )
        .await
        .unwrap();
        let mut want_a = data_x.clone();
        want_a.extend_from_slice(&data_y);
        assert_eq!(std::fs::read(game.join("Bin/a.bin")).unwrap(), want_a);
        assert_eq!(std::fs::read(game.join("b.bin")).unwrap(), data_y);
        assert_eq!(std::fs::read(game.join("__support/x.dll")).unwrap(), data_y);
        let text = std::fs::read_to_string(&resume).unwrap();
        assert!(text.contains("a::Bin/a.bin"), "{text}");

        // a second run finds everything done
        let again = plan::analyse(&diff, &game, &support, &resume, MEMORY_CAP).unwrap();
        assert_eq!(again.downloads.len(), 0);
        assert_eq!(again.resumed, 3);

        // an update that keeps chunk y of a.bin copies it from the old file
        let a2 = GameFile {
            chunks: vec![chunk(&data_y, &cy), chunk(&data_x, &cx)],
            ..a.clone()
        };
        let changed = manifest::Changed {
            file: a2,
            old_support: false,
            reuse: vec![Some(300_000), None],
        };
        let _ = std::fs::remove_file(&resume);
        let diff2 = Diff {
            changed: vec![changed],
            ..Diff::default()
        };
        let plan2 = plan::analyse(&diff2, &game, &support, &resume, MEMORY_CAP).unwrap();
        assert_eq!(plan2.downloads.len(), 1);
        execute(
            links,
            plan2,
            &game,
            &support,
            &resume,
            2,
            progress,
            CancelFlag::new(),
        )
        .await
        .unwrap();
        let mut want = data_y.clone();
        want.extend_from_slice(&data_x);
        assert_eq!(std::fs::read(game.join("Bin/a.bin")).unwrap(), want);
        assert!(!game.join("Bin/a.bin.tmp").exists());
        // the `.tmp` file counts as done only after it replaced the old one
        let text = std::fs::read_to_string(&resume).unwrap();
        assert_eq!(
            text.lines().filter(|l| l.ends_with("::Bin/a.bin")).count(),
            1,
            "{text}"
        );
        assert!(target(&game, "../x", false).is_err());
        assert!(target(&game, "C:\\x", false).is_err());
        assert_eq!(
            target(&game, "b.bin", true).unwrap(),
            PathBuf::from(format!("{}.tmp", game.join("b.bin").display()))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn downloads_v1_ranges_of_main_bin() {
        let root = std::env::temp_dir().join(format!("hyper-gog-dl1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let main_bin: Vec<u8> = (0..50_000u32).map(|i| (i % 253) as u8).collect();
        let mut served = HashMap::new();
        served.insert("store/main.bin".to_string(), main_bin.clone());
        let base = serve(served).await;
        let client = client(&root);
        let links = links_for(client, &base, 1).await;
        let f = GameFile {
            product: "100".into(),
            path: "data/game.dat".into(),
            size: 20_000,
            md5: Some(md5_hex(&main_bin[10_000..30_000])),
            v1_offset: Some(10_000),
            ..GameFile::default()
        };
        let game = root.join("Game");
        std::fs::create_dir_all(&game).unwrap();
        let diff = Diff {
            new: vec![f],
            ..Diff::default()
        };
        let plan = plan::analyse(
            &diff,
            &game,
            &root.join("support"),
            &game.join(RESUME_FILE),
            MEMORY_CAP,
        )
        .unwrap();
        assert_eq!(plan.downloads.len(), 1);
        execute(
            links,
            plan,
            &game,
            &root.join("support"),
            &game.join(RESUME_FILE),
            2,
            Arc::new(|_, _| {}),
            CancelFlag::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(game.join("data/game.dat")).unwrap(),
            &main_bin[10_000..30_000]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn finish_checks_hashes() {
        let raw = b"hello world".to_vec();
        let packed = deflate(&raw);
        let dl = Download {
            product: "1".into(),
            key: md5_hex(&packed),
            md5: Some(md5_hex(&raw)),
            size: raw.len() as u64,
            compressed_size: packed.len() as u64,
            v1_range: None,
        };
        assert_eq!(finish(&dl, &packed).unwrap(), raw);
        let wrong = Download {
            key: "00".into(),
            ..dl.clone()
        };
        assert!(finish(&wrong, &packed).is_err());
        let short = Download {
            v1_range: Some((0, 5)),
            ..dl
        };
        assert!(finish(&short, b"1234").is_err());
        assert_eq!(finish(&short, b"12345").unwrap(), b"12345");
    }
}
