//! GOG, natively (port of Heroic's `src/backend/storeManagers/gog/
//! {library,games,user}.ts` and of its `heroic-gogdl` helper, see `native`).
//!
//! - Login: the web flow hands us an authorization code, exchanged here
//!   (`api::exchange_code`); the tokens live in the vault in gogdl's
//!   `auth.json` layout and are refreshed by `native::auth`.
//! - Library: owned products from the Galaxy library API, details from
//!   gamesdb (cached), installed games from our own `installed.json`.
//! - Install / update / repair: `native::download`, with gogdl's manifest
//!   per game under `gogdlConfig/heroic_gogdl/manifests` and its resume file.
//! - Launch: the primary play task of `goggame-<id>.info`, started by the
//!   game runner (play time, kill, cancel).
//! - Cloud saves: `native::saves` per save folder (see `saves`).
//!
//! After an install / update: Galaxy's shared redistributables and the
//! build's setup scripts (`setup`, `native::redist`), like Heroic's `setup`.

pub(crate) mod achievements;
mod api;
mod installed;
pub mod native;
mod saves;
pub(crate) mod setup;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock};

use crate::error::{Error, Result};
use crate::events::CoreEvent;
use crate::paths;
use crate::secrets::SecretFile;
use crate::stores::epic::download::CancelFlag;
use crate::stores::info_cache::InfoCache;
use crate::stores::{GameLog, StoreContext, StoreManager};
use crate::types::DownloadProgress;
use crate::types::{
    GameExtra, GameInfo, GameStatus, GameStatusKind, ImportCandidate, InstallInfo, InstallOptions,
    InstallPlatform, Runner, SaveLocation, SaveSyncMode, StoreStatus,
};
use installed::GogInstalled;
use native::download::{self, Dlcs, Kind, Outcome, Phase, ProgressFn, Request};
use native::info::{Info, Sizes};
use native::redist;

pub const VAULT_KEY: &str = "gog-auth";

/// GOG Galaxy's OAuth client; the same one Heroic and gogdl use.
pub const CLIENT_ID: &str = "46899977096215655";
pub const LOGIN_URL: &str = "https://auth.gog.com/auth?client_id=46899977096215655&redirect_uri=https%3A%2F%2Fembed.gog.com%2Fon_login_success%3Forigin%3Dclient&response_type=code&layout=galaxy";
/// After login GOG redirects here with `?code=…`.
pub const LOGIN_REDIRECT_HOST: &str = "embed.gog.com";
pub const LOGIN_REDIRECT_PATH: &str = "/on_login_success";
pub const STORE_URL: &str = "https://www.gog.com/{lang}/";
/// The only platform we install (Linux runs the Windows build through Proton).
const PLATFORM: &str = "windows";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct GogUser {
    id: String,
    username: String,
}

/// What `native::info` reports for a game, reduced to what we use.
#[derive(Debug, Clone)]
struct BuildInfo {
    folder_name: String,
    version: String,
    language: String,
    dlcs: Vec<String>,
    download_size: u64,
    disk_size: u64,
    branches: Vec<String>,
    builds: Vec<crate::types::BuildEntry>,
}

pub struct Gog {
    ctx: StoreContext,
    auth: Arc<SecretFile>,
    /// The GOG API and CDN client; its `http` serves every GOG request.
    client: Arc<native::Client>,
    /// Owned games without install data (persisted in `library.json`).
    owned: RwLock<Vec<GameInfo>>,
    installed: Mutex<HashMap<String, GogInstalled>>,
    /// Installed games with a newer public build.
    updates: RwLock<HashSet<String>>,
    info_cache: Mutex<HashMap<String, BuildInfo>>,
    /// Install sizes kept across restarts (game page).
    disk_info: InfoCache,
    /// Running downloads, for cancel.
    active: Mutex<HashMap<String, Arc<CancelFlag>>>,
    /// Cloud-save setup of installed games (persisted in `cloudSaves.json`).
    cloud: Mutex<HashMap<String, saves::CloudConfig>>,
}

fn library_file() -> PathBuf {
    paths::gogdl_config_dir().join("library.json")
}

/// GOG language code for the UI language: "uk" → "uk-UA" when the game has
/// it, else English, else whatever the game offers first.
fn pick_language(available: &[String], ui_lang: &str) -> String {
    available
        .iter()
        .find(|l| l.split('-').next() == Some(ui_lang))
        .or_else(|| available.iter().find(|l| l.as_str() == "en-US"))
        .or_else(|| available.first())
        .cloned()
        .unwrap_or_else(|| "en-US".into())
}

/// Sizes for one language (+ the shared depot).
fn size_for(map: &Sizes, lang: &str) -> (u64, u64) {
    let get = |key: &str| map.get(key).copied().unwrap_or((0, 0));
    let (shared, own) = (get("*"), get(lang));
    (shared.0 + own.0, shared.1 + own.1)
}

fn build_info(info: &Info, ui_lang: &str) -> BuildInfo {
    let language = pick_language(&info.languages, ui_lang);
    // Like Heroic: show the size with every owned DLC (we install them all).
    let (mut download_size, mut disk_size) = size_for(&info.size, &language);
    for dlc in &info.dlcs {
        let (d, k) = size_for(&dlc.size, &language);
        download_size += d;
        disk_size += k;
    }
    BuildInfo {
        folder_name: info.folder_name.clone(),
        version: info.version_name.clone(),
        language,
        dlcs: info.dlcs.iter().map(|d| d.id.clone()).collect(),
        download_size,
        disk_size,
        branches: info.branches.clone(),
        builds: info.builds.clone(),
    }
}

/// An app name from the UI as a GOG product id (non-empty ASCII digits):
/// it becomes part of file names (manifest, support folder) and URLs.
fn product_id(app: &str) -> Result<&str> {
    if native::is_product_id(app) {
        Ok(app)
    } else {
        Err(Error::Other(format!("not a GOG product id: {app:?}")))
    }
}

/// The player's password for the game's private branch, when set.
fn branch_password(app_name: &str) -> Option<String> {
    let pw = crate::config::game::GameSettings::load(Runner::Gog, app_name).gog_branch_password;
    Some(pw.trim().to_string()).filter(|p| !p.is_empty())
}

impl Gog {
    pub fn new(ctx: StoreContext) -> Self {
        let auth = SecretFile::new(
            ctx.vault.clone(),
            VAULT_KEY,
            paths::gogdl_config_dir().join("auth.json"),
        );
        if let Err(e) = auth.absorb_leftover() {
            tracing::error!(error = %e, "could not absorb leftover GOG credentials");
        }
        // no whole-request deadline: a 20 MiB v1 piece on a slow link takes as
        // long as it takes; a stalled connection fails by the read timeout
        let http = reqwest::Client::builder()
            .user_agent(format!("Hyper/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(15))
            .read_timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        let owned = std::fs::read_to_string(library_file())
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<GameInfo>>(&s).ok())
            .unwrap_or_default();
        Self {
            ctx,
            client: Arc::new(native::Client::new(auth.clone(), http)),
            auth,
            owned: RwLock::new(owned),
            installed: Mutex::new(installed::load()),
            updates: RwLock::new(HashSet::new()),
            info_cache: Mutex::new(HashMap::new()),
            disk_info: InfoCache::new("gog"),
            active: Mutex::new(HashMap::new()),
            cloud: Mutex::new(saves::load_cache()),
        }
    }

    /// Cloud-save setup of an installed game: cached, else asked from GOG.
    /// `None` when the game has no Galaxy client id.
    async fn cloud_config(
        &self,
        app_name: &str,
        install_path: &str,
    ) -> Result<Option<saves::CloudConfig>> {
        if let Some(hit) = self.cloud.lock().await.get(app_name) {
            return Ok(Some(hit.clone()));
        }
        let Some(client_id) = saves::client_id(app_name, Path::new(install_path)) else {
            tracing::debug!(app = app_name, "no Galaxy client id; no cloud saves");
            return Ok(None);
        };
        let config = saves::fetch(&self.client.http, &client_id).await?;
        let mut all = self.cloud.lock().await;
        all.insert(app_name.to_string(), config.clone());
        saves::save_cache(&all);
        Ok(Some(config))
    }

    fn read_user() -> Option<GogUser> {
        let raw = std::fs::read_to_string(paths::gog_user_file()).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn write_user(user: &GogUser) -> Result<()> {
        std::fs::write(paths::gog_user_file(), serde_json::to_string_pretty(user)?)?;
        Ok(())
    }

    /// See [`crate::stores::Stores::seal_leftovers`].
    pub fn seal_leftover(&self) -> Result<bool> {
        self.auth.absorb_leftover()
    }

    fn require_login(&self) -> Result<()> {
        if self.auth.exists() {
            Ok(())
        } else {
            Err(Error::NotLoggedIn("GOG".into()))
        }
    }

    fn emit_status(&self, app_name: &str, status: GameStatusKind) {
        self.ctx.events.emit(CoreEvent::GameStatus(GameStatus::new(
            Runner::Gog,
            app_name,
            status,
        )));
    }

    /// A failure the UI must show (a queued item would stay "Queued" otherwise).
    fn emit_error(&self, app_name: &str, e: &Error) {
        self.ctx.events.emit(CoreEvent::GameStatus(GameStatus {
            app_name: app_name.to_string(),
            runner: Runner::Gog,
            status: GameStatusKind::Error,
            progress: None,
            message: Some(e.to_string()),
        }));
    }

    /// Current (refreshed when expired) access token.
    async fn credentials(&self) -> Result<api::Credentials> {
        self.require_login()?;
        self.client.auth.credentials().await
    }

    /// `GET https://users.gog.com/users/<id>` → username.
    async fn fetch_username(&self, user_id: &str, access_token: &str) -> Result<String> {
        if !native::is_product_id(user_id) {
            return Err(Error::Other("GOG user info: unusable user id".into()));
        }
        let resp = self
            .client
            .http
            .get(format!("https://users.gog.com/users/{user_id}"))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| Error::Other(format!("GOG user info: {e}")))?;
        if !resp.status().is_success() {
            return Err(Error::Other(format!(
                "GOG user info: HTTP {}",
                resp.status()
            )));
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| Error::Other(e.to_string()))?;
        v.get("username")
            .and_then(|u| u.as_str())
            .map(str::to_owned)
            .ok_or_else(|| Error::Other("GOG user info has no username".into()))
    }

    /// Owned games merged with install state and update flags.
    async fn merged(&self) -> Vec<GameInfo> {
        let installed = self.installed.lock().await.clone();
        let updates = self.updates.read().await.clone();
        let cloud = self.cloud.lock().await.clone();
        self.owned
            .read()
            .await
            .iter()
            .cloned()
            .map(|mut g| {
                if let Some(i) = installed.get(&g.app_name) {
                    g.is_installed = true;
                    g.version = Some(i.version.clone());
                    g.folder_name = Path::new(&i.install_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned());
                    g.install = Some(i.info());
                    g.update_available = updates.contains(&g.app_name);
                    g.cloud_save_enabled = cloud.get(&g.app_name).is_some_and(|c| c.enabled);
                }
                g
            })
            .collect()
    }

    /// Re-read the owned list from GOG and look for updates of installed games.
    async fn refresh(&self) -> Result<()> {
        let creds = self.credentials().await?;
        let entries = api::owned(&self.client.http, &creds).await?;
        tracing::info!(count = entries.len(), "GOG library: owned products");
        let docs = api::gamesdb_all(&self.client.http, entries, &creds.access_token).await;
        let mut games: Vec<GameInfo> = docs.iter().filter_map(api::game_info).collect();
        games.sort_by_key(|g| g.title.to_lowercase());
        games.dedup_by(|a, b| a.app_name == b.app_name);
        tracing::info!(count = games.len(), "GOG library loaded");
        if let Err(e) = std::fs::write(library_file(), serde_json::to_vec(&games)?) {
            tracing::warn!(error = %e, "could not cache the GOG library");
        }
        *self.owned.write().await = games;

        let installed: Vec<GogInstalled> = self.installed.lock().await.values().cloned().collect();
        let mut updates = HashSet::new();
        for g in installed {
            if g.pinned {
                continue;
            }
            let platform = if g.platform.is_empty() {
                PLATFORM
            } else {
                g.platform.as_str()
            };
            let password = branch_password(&g.app_name);
            match self
                .client
                .builds_with_password(&g.app_name, platform, password.as_deref())
                .await
            {
                Ok(builds) => {
                    // the build an update would install; v1 installs record the legacy id
                    let latest = native::target_build(&builds, None, g.branch.as_deref());
                    let same = |key: &str| {
                        latest
                            .and_then(|b| b.get(key))
                            .and_then(native::id_string)
                            .as_deref()
                            == Some(g.build_id.as_str())
                    };
                    if latest.is_some()
                        && !g.build_id.is_empty()
                        && !same("build_id")
                        && !same("legacy_build_id")
                    {
                        updates.insert(g.app_name.clone());
                    }
                }
                Err(e) => tracing::debug!(app = %g.app_name, error = %e, "GOG update check failed"),
            }
            if let Err(e) = self.cloud_config(&g.app_name, &g.install_path).await {
                tracing::debug!(app = %g.app_name, error = %e, "GOG cloud-save check failed");
            }
        }
        *self.updates.write().await = updates;
        Ok(())
    }

    /// The latest build's sizes and languages, cached per game until the
    /// next install / update.
    async fn build_info(&self, app_name: &str) -> Result<BuildInfo> {
        if let Some(hit) = self.info_cache.lock().await.get(app_name) {
            return Ok(hit.clone());
        }
        self.require_login()?;
        let raw = native::info::fetch(
            &self.client,
            app_name,
            PLATFORM,
            None,
            None,
            branch_password(app_name).as_deref(),
        )
        .await?;
        let lang = self.ctx.settings.read().await.language.clone();
        let info = build_info(&raw, &lang);
        self.info_cache
            .lock()
            .await
            .insert(app_name.to_string(), info.clone());
        Ok(info)
    }

    async fn installed_entry(&self, app_name: &str) -> Result<GogInstalled> {
        self.installed
            .lock()
            .await
            .get(app_name)
            .cloned()
            .ok_or_else(|| Error::Other(format!("{app_name} is not installed")))
    }

    async fn save_installed(
        &self,
        entry: Option<GogInstalled>,
        remove: Option<&str>,
    ) -> Result<()> {
        let mut all = self.installed.lock().await;
        if let Some(e) = entry {
            all.insert(e.app_name.clone(), e);
        }
        if let Some(app) = remove {
            all.remove(app);
        }
        installed::save(&all)
    }

    /// Run a download / update / repair and report progress.
    async fn run_download(
        &self,
        app_name: &str,
        status: GameStatusKind,
        req: Request,
    ) -> Result<Outcome> {
        self.require_login()?;
        let cancel = {
            let mut active = self.active.lock().await;
            if active.contains_key(app_name) {
                return Err(Error::Busy(app_name.to_string()));
            }
            let flag = CancelFlag::new();
            active.insert(app_name.to_string(), flag.clone());
            flag
        };
        self.emit_status(app_name, status);
        let (events, app) = (self.ctx.events.clone(), app_name.to_string());
        let on_progress: ProgressFn = Arc::new(move |phase: Phase, progress| {
            // a repair's hashing pass is no download: the UI shows the note, not a speed
            let message = (phase == Phase::Verify).then(|| "Verifying files".to_string());
            events.emit(CoreEvent::GameStatus(GameStatus {
                app_name: app.clone(),
                runner: Runner::Gog,
                status,
                progress: Some(progress),
                message,
            }));
        });
        let log = GameLog::open(Runner::Gog, app_name, "install");
        log.write(&format!(
            "{:?} platform={} language={:?} dlcs={:?}",
            req.kind, req.platform, req.language, req.dlcs
        ));
        let out = download::run(&self.client, req, on_progress, cancel).await;
        self.active.lock().await.remove(app_name);
        match out {
            Ok(o) => {
                log.write(&format!(
                    "finished: build {} ({})",
                    o.build_id, o.version_name
                ));
                Ok(o)
            }
            Err(Error::Cancelled) => {
                log.write("cancelled");
                self.emit_status(app_name, GameStatusKind::Done);
                Err(Error::Cancelled)
            }
            Err(e) => {
                log.write(&e.to_string());
                self.emit_error(app_name, &e);
                Err(e)
            }
        }
    }

    /// After a finished install/update/repair: record what is on disk now.
    async fn record_install(&self, app_name: &str, out: Outcome, pinned: bool) -> Result<()> {
        self.info_cache.lock().await.remove(app_name);
        let install_path = out.game_dir.to_string_lossy().into_owned();
        let previous = self.installed.lock().await.get(app_name).cloned();
        let path = out.game_dir.clone();
        let size_bytes = tokio::task::spawn_blocking(move || installed::dir_size(&path))
            .await
            .unwrap_or(0);
        let pick = |new: String, old: Option<String>| {
            if new.is_empty() {
                old.unwrap_or_default()
            } else {
                new
            }
        };
        let entry = GogInstalled {
            app_name: app_name.to_string(),
            install_path: install_path.clone(),
            platform: PLATFORM.into(),
            version: pick(
                out.version_name,
                previous.as_ref().map(|p| p.version.clone()),
            ),
            build_id: pick(out.build_id, previous.as_ref().map(|p| p.build_id.clone())),
            language: pick(out.language, previous.as_ref().map(|p| p.language.clone())),
            dlcs: out.dlcs,
            size_bytes,
            branch: out.branch,
            pinned,
        };
        self.save_installed(Some(entry), None).await?;
        self.updates.write().await.remove(app_name);
        if let Err(e) = self.cloud_config(app_name, &install_path).await {
            tracing::debug!(app = app_name, error = %e, "GOG cloud-save check failed");
        }
        self.emit_status(app_name, GameStatusKind::Done);
        self.ctx
            .events
            .emit(CoreEvent::LibraryUpdated(Some(Runner::Gog)));
        Ok(())
    }

    /// `update` / `repair` share their request (game folder, language, DLCs).
    async fn maintain(&self, app_name: &str, kind: Kind, status: GameStatusKind) -> Result<()> {
        let g = match self.installed_entry(app_name).await {
            Ok(g) => g,
            Err(e) => {
                self.emit_error(app_name, &e);
                return Err(e);
            }
        };
        let req = Request {
            app_name: app_name.to_string(),
            platform: if g.platform.is_empty() {
                PLATFORM.into()
            } else {
                g.platform.clone()
            },
            kind,
            base_path: None,
            game_dir: Some(PathBuf::from(&g.install_path)),
            support_dir: paths::gog_support_dir().join(app_name),
            language: Some(g.language.clone()).filter(|l| !l.is_empty()),
            dlcs: if g.dlcs.is_empty() {
                Dlcs::None
            } else {
                Dlcs::These(g.dlcs.clone())
            },
            // a pinned build stays on an update too
            build_id: ((kind == Kind::Repair || g.pinned) && !g.build_id.is_empty())
                .then(|| g.build_id.clone()),
            branch: g.branch.clone(),
            password: branch_password(app_name),
            max_workers: self.ctx.settings.read().await.max_workers as usize,
        };
        let out = self.run_download(app_name, status, req).await?;
        if kind == Kind::Update {
            // the build's setup script again (new files, registry keys); the
            // redistributables were installed with the game
            self.finish_setup(app_name, &out, status, false).await;
        }
        self.record_install(app_name, out, g.pinned).await
    }

    /// A status with a note for the UI ("Installing DirectX"); the empty
    /// progress clears the download's bar.
    fn emit_note(&self, app_name: &str, status: GameStatusKind, note: impl Into<String>) {
        self.ctx.events.emit(CoreEvent::GameStatus(GameStatus {
            app_name: app_name.to_string(),
            runner: Runner::Gog,
            status,
            progress: Some(DownloadProgress::default()),
            message: Some(note.into()),
        }));
    }

    /// Galaxy's post-install work (Heroic's `setup`): the shared
    /// redistributables (`install_redist`), then the build's setup script
    /// and, on an install, the installers of the redistributables the game
    /// depends on. Never fails the install: problems are logged, the game
    /// stays installed. Linux: everything runs in the game's Wine prefix,
    /// which is created here when it does not exist yet.
    async fn finish_setup(
        &self,
        app_name: &str,
        out: &Outcome,
        status: GameStatusKind,
        install_redist: bool,
    ) {
        let log = GameLog::open(Runner::Gog, app_name, "setup");
        let redist_dir = paths::gog_redist_dir();
        if install_redist {
            self.emit_note(app_name, status, "Checking Galaxy redistributables");
            let ids = redist::required_ids(&paths::gogdl_manifests_dir());
            log.write(&format!("redistributables wanted: {}", ids.join(", ")));
            let cancel = CancelFlag::new();
            self.active
                .lock()
                .await
                .insert(app_name.to_string(), cancel.clone());
            let (events, app) = (self.ctx.events.clone(), app_name.to_string());
            let on_progress: ProgressFn = Arc::new(move |_phase: Phase, progress| {
                events.emit(CoreEvent::GameStatus(GameStatus {
                    app_name: app.clone(),
                    runner: Runner::Gog,
                    status,
                    progress: Some(progress),
                    message: Some("Downloading Galaxy redistributables".into()),
                }));
            });
            let workers = self.ctx.settings.read().await.max_workers as usize;
            let result = redist::ensure(
                &self.client,
                &ids,
                &redist_dir,
                workers,
                on_progress,
                cancel,
            )
            .await;
            self.active.lock().await.remove(app_name);
            match result {
                Ok(()) => log.write("redistributables up to date"),
                Err(Error::Cancelled) => {
                    log.write("redistributables: cancelled");
                    return;
                }
                Err(e) => {
                    tracing::warn!(app = app_name, error = %e, "GOG redistributables not updated");
                    log.write(&format!("redistributables: {e}"));
                }
            }
        }

        let manifest_file = paths::gogdl_manifests_dir().join(app_name);
        let manifest: serde_json::Value = match std::fs::read_to_string(&manifest_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(m) => m,
            None => {
                tracing::warn!(app = app_name, "no saved manifest; setup scripts skipped");
                log.write("no manifest: setup skipped");
                return;
            }
        };
        let wine = crate::platform::current().needs_wine();
        let support_dir = paths::gog_support_dir().join(app_name);
        let game = setup::Game {
            app_name,
            install_path: &out.game_dir,
            version: &out.version_name,
            build_id: &out.build_id,
            language: &out.language,
            dlcs: &out.dlcs,
        };
        let redist_manifest = redist::installed_manifest(&redist_dir);
        let steps = setup::steps(
            &game,
            &manifest,
            &support_dir,
            &redist_dir,
            redist_manifest.as_ref(),
            install_redist,
            wine,
        );
        log.write(&format!("{} setup step(s)", steps.len()));
        // "Cancel" during the setup stops the running program (or stops
        // waiting for it) and skips the rest; the game stays installed
        let cancel = CancelFlag::new();
        self.active
            .lock()
            .await
            .insert(app_name.to_string(), cancel.clone());
        for step in steps {
            if cancel.is_set() {
                break;
            }
            self.emit_note(app_name, status, step.label.clone());
            log.write(&format!(
                "{}: {} {}",
                step.label,
                step.exe.display(),
                step.args.join(" ")
            ));
            match self.run_setup_step(app_name, status, &step, &cancel).await {
                Ok(code) => log.write(&format!("exit code {code}")),
                Err(_) if cancel.is_set() => {
                    tracing::info!(app = app_name, step = %step.label, "GOG setup cancelled");
                    log.write("cancelled: remaining setup steps skipped");
                    break;
                }
                Err(e) => {
                    tracing::warn!(app = app_name, step = %step.label, error = %e, "GOG setup step failed");
                    log.write(&format!("failed: {e}"));
                    // Windows: a declined UAC prompt ends the setup instead of asking again for every program
                    if !wine {
                        break;
                    }
                }
            }
        }
        self.active.lock().await.remove(app_name);
        log.write("setup finished");
    }

    /// One setup program: elevated on Windows, through Proton in the game's
    /// prefix on Linux (Proton and the prefix are set up first if needed).
    async fn run_setup_step(
        &self,
        app_name: &str,
        status: GameStatusKind,
        step: &setup::Step,
        cancel: &Arc<CancelFlag>,
    ) -> Result<i64> {
        let cancelled = || cancel.is_set();
        if crate::platform::current().needs_wine() {
            let compat = self
                .ctx
                .games
                .compat()
                .ok_or_else(|| Error::Other("Proton is not available".into()))?;
            let on_prepare = |p: crate::compat::Preparing| {
                if let Some(m) = p.message {
                    self.emit_note(app_name, status, m);
                }
            };
            let exit = compat
                .run_in_prefix(
                    Runner::Gog,
                    app_name,
                    &step.exe,
                    &step.args,
                    Some(&step.cwd),
                    &on_prepare,
                    &cancelled,
                )
                .await?;
            return Ok(exit.code().map(i64::from).unwrap_or(-1));
        }
        #[cfg(windows)]
        {
            let (exe, cwd) = (step.exe.clone(), step.cwd.clone());
            let params = setup::windows_command_line(&step.args);
            let cancel = cancel.clone();
            let code = tokio::task::spawn_blocking(move || {
                crate::platform::run_elevated(&exe, &params, &cwd, &|| cancel.is_set())
            })
            .await
            .map_err(|e| Error::Other(format!("setup task failed: {e}")))??;
            Ok(i64::from(code))
        }
        #[cfg(not(windows))]
        Err(Error::Other("Windows programs cannot run here".into()))
    }
}

#[async_trait]
impl StoreManager for Gog {
    fn runner(&self) -> Runner {
        Runner::Gog
    }

    async fn status(&self) -> StoreStatus {
        let logged_in = self.auth.exists();
        let username = if logged_in {
            Self::read_user().map(|u| u.username)
        } else {
            None
        };
        StoreStatus {
            runner: Runner::Gog,
            logged_in,
            username: username.or_else(|| logged_in.then(|| "GOG".to_string())),
            helper_available: true,
        }
    }

    async fn login(&self, code: &str) -> Result<StoreStatus> {
        let code = code.trim();
        if code.is_empty()
            || !code
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Error::Other("invalid authorization code".into()));
        }
        let creds = api::exchange_code(&self.client.http, code).await?;
        let mut entry = creds.clone();
        entry["loginTime"] = native::now_secs().into();
        // gogdl's auth.json layout: one entry per OAuth client (see native::auth)
        let file = serde_json::json!({ CLIENT_ID: entry });
        self.auth
            .replace(&zeroize::Zeroizing::new(serde_json::to_vec(&file)?))
            .await?;

        let user_id = creds
            .get("user_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let token = creds
            .get("access_token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        match self.fetch_username(&user_id, &token).await {
            Ok(username) => {
                if let Err(e) = Self::write_user(&GogUser {
                    id: user_id,
                    username,
                }) {
                    tracing::warn!(error = %e, "could not save GOG profile");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not fetch GOG username"),
        }

        let status = self.status().await;
        tracing::info!(user = ?status.username, "logged in to GOG");
        self.ctx.events.emit(CoreEvent::StoreLogin(status.clone()));
        Ok(status)
    }

    async fn logout(&self) -> Result<()> {
        self.auth.revoke().await?;
        let _ = std::fs::remove_file(paths::gog_user_file());
        let _ = std::fs::remove_file(library_file());
        self.owned.write().await.clear();
        let status = self.status().await;
        self.ctx.events.emit(CoreEvent::StoreLogin(status));
        self.ctx
            .events
            .emit(CoreEvent::LibraryUpdated(Some(Runner::Gog)));
        Ok(())
    }

    async fn library(&self, force_refresh: bool) -> Result<Vec<GameInfo>> {
        if !self.auth.exists() {
            return Ok(Vec::new());
        }
        let empty = self.owned.read().await.is_empty();
        if (force_refresh || empty)
            && let Err(e) = self.refresh().await
        {
            tracing::warn!(error = %e, "GOG library refresh failed; using the cached list");
        }
        Ok(self.merged().await)
    }

    /// The owned DLCs installed with the game (GOG installs them all;
    /// titles from the saved manifest's `HGLdlcs`).
    async fn dlcs(&self, app_name: &str) -> Result<Vec<crate::types::DlcEntry>> {
        product_id(app_name)?;
        let g = self.installed_entry(app_name).await?;
        let manifest: Option<serde_json::Value> =
            std::fs::read_to_string(paths::gogdl_manifests_dir().join(app_name))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok());
        let titles: HashMap<String, String> = manifest
            .as_ref()
            .and_then(|m| m.get("HGLdlcs"))
            .and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|d| {
                        Some((
                            d.get("id")?.as_str()?.to_string(),
                            d.get("title")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(g.dlcs
            .iter()
            .map(|id| crate::types::DlcEntry {
                app_name: id.clone(),
                title: titles.get(id).cloned().unwrap_or_else(|| id.clone()),
                installed: true,
                installable: false,
            })
            .collect())
    }

    /// Heroic's `getAchievements`: the gameplay API for the game's Galaxy
    /// client (its product id when the client id is unknown).
    async fn achievements(&self, app_name: &str, lang: &str) -> Result<crate::types::Achievements> {
        product_id(app_name)?;
        let creds = self.credentials().await?;
        let from_install = self
            .installed
            .lock()
            .await
            .get(app_name)
            .and_then(|g| saves::client_id(app_name, Path::new(&g.install_path)));
        let from_cloud = self
            .cloud
            .lock()
            .await
            .get(app_name)
            .map(|c| c.client_id.clone())
            .filter(|s| !s.is_empty());
        let client_id = from_install
            .or(from_cloud)
            .unwrap_or_else(|| app_name.to_string());
        achievements::fetch(
            &self.client.http,
            &creds.access_token,
            &creds.user_id,
            &client_id,
            &native::languages::code(lang),
        )
        .await
    }

    async fn change_install_path(&self, app_name: &str, new_path: &Path) -> Result<()> {
        product_id(app_name)?;
        let mut g = self.installed_entry(app_name).await?;
        g.install_path = new_path.to_string_lossy().into_owned();
        self.save_installed(Some(g), None).await?;
        self.ctx
            .events
            .emit(CoreEvent::LibraryUpdated(Some(Runner::Gog)));
        Ok(())
    }

    async fn install_info(
        &self,
        app_name: &str,
        _platform: InstallPlatform,
    ) -> Result<InstallInfo> {
        product_id(app_name)?;
        // no cheap way to learn the latest build here, so sizes on disk are
        // only trusted for a few days (installing always asks GOG afresh)
        const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(3 * 86_400);
        if !self.info_cache.lock().await.contains_key(app_name)
            && let Some(hit) = self.disk_info.get(app_name, "", MAX_AGE)
        {
            return Ok(hit);
        }
        let info = self.build_info(app_name).await?;
        let title = self
            .owned
            .read()
            .await
            .iter()
            .find(|g| g.app_name == app_name)
            .map(|g| g.title.clone())
            .unwrap_or_else(|| app_name.to_string());
        let out = InstallInfo {
            app_name: app_name.to_string(),
            title,
            version: info.version,
            download_size: info.download_size,
            disk_size: info.disk_size,
            platform: InstallPlatform::Windows,
            branches: info.branches,
            builds: info.builds,
        };
        if out.download_size > 0 {
            self.disk_info.put(app_name, "", &out);
        }
        Ok(out)
    }

    async fn install(&self, app_name: &str, opts: InstallOptions) -> Result<()> {
        if let Err(e) = product_id(app_name) {
            self.emit_error(app_name, &e);
            return Err(e);
        }
        let info = match self.build_info(app_name).await {
            Ok(i) if i.folder_name.is_empty() => Err(Error::Other(format!(
                "GOG did not report an install folder for {app_name}"
            ))),
            Ok(i) if !super::is_plain_folder_name(&i.folder_name) => Err(Error::Other(format!(
                "GOG reported an unsafe install folder for {app_name}: {:?}",
                i.folder_name
            ))),
            other => other,
        };
        let info = match info {
            Ok(i) => i,
            Err(e) => {
                self.emit_error(app_name, &e);
                return Err(e);
            }
        };
        let req = Request {
            app_name: app_name.to_string(),
            platform: PLATFORM.into(),
            kind: Kind::Install,
            base_path: Some(PathBuf::from(&opts.path)),
            game_dir: None,
            support_dir: paths::gog_support_dir().join(app_name),
            language: Some(info.language.clone()),
            dlcs: if info.dlcs.is_empty() {
                Dlcs::None
            } else {
                Dlcs::All
            },
            build_id: opts.build_id.clone().filter(|b| !b.trim().is_empty()),
            branch: opts.branch.clone().filter(|b| !b.trim().is_empty()),
            password: branch_password(app_name),
            max_workers: self.ctx.settings.read().await.max_workers as usize,
        };
        let pinned = req.build_id.is_some();
        let out = self
            .run_download(app_name, GameStatusKind::Installing, req)
            .await?;
        self.finish_setup(app_name, &out, GameStatusKind::Installing, true)
            .await;
        self.record_install(app_name, out, pinned).await
    }

    async fn update(&self, app_name: &str) -> Result<()> {
        product_id(app_name)?;
        self.maintain(app_name, Kind::Update, GameStatusKind::Updating)
            .await
    }

    async fn repair(&self, app_name: &str) -> Result<()> {
        product_id(app_name)?;
        self.maintain(app_name, Kind::Repair, GameStatusKind::Repairing)
            .await
    }

    async fn cancel(&self, app_name: &str) -> Result<()> {
        product_id(app_name)?;
        match self.active.lock().await.get(app_name) {
            Some(flag) => {
                flag.cancel();
                Ok(())
            }
            None => Err(Error::Other(format!("{app_name} is not downloading"))),
        }
    }

    async fn uninstall(&self, app_name: &str) -> Result<()> {
        product_id(app_name)?;
        let g = self.installed_entry(app_name).await?;
        if self.ctx.games.is_running(Runner::Gog, app_name) {
            return Err(Error::Busy(app_name.to_string()));
        }
        self.emit_status(app_name, GameStatusKind::Uninstalling);
        let dir = PathBuf::from(&g.install_path);
        // never remove a drive root, the games folder or a user/system folder
        let base = self.ctx.settings.read().await.default_install_path.clone();
        if dir.is_dir() && !super::is_safe_game_dir(&dir, &base) {
            self.emit_status(app_name, GameStatusKind::Done);
            return Err(Error::Other(format!(
                "refusing to delete {}: it is not a single game's folder",
                dir.display()
            )));
        }
        if dir.is_dir()
            && let Err(e) = tokio::fs::remove_dir_all(&dir).await
        {
            self.emit_status(app_name, GameStatusKind::Error);
            return Err(e.into());
        }
        let _ = tokio::fs::remove_file(paths::gogdl_manifests_dir().join(app_name)).await;
        let _ = tokio::fs::remove_dir_all(paths::gog_support_dir().join(app_name)).await;
        crate::compat::remove_prefix(&self.ctx.settings, Runner::Gog, app_name).await;
        self.save_installed(None, Some(app_name)).await?;
        self.updates.write().await.remove(app_name);
        {
            let mut cloud = self.cloud.lock().await;
            if cloud.remove(app_name).is_some() {
                saves::save_cache(&cloud);
            }
        }
        saves::forget(app_name);
        self.emit_status(app_name, GameStatusKind::Done);
        self.ctx
            .events
            .emit(CoreEvent::LibraryUpdated(Some(Runner::Gog)));
        Ok(())
    }

    async fn launch(&self, app_name: &str) -> Result<()> {
        product_id(app_name)?;
        let g = self.installed_entry(app_name).await?;
        let ticket = self.ctx.games.prepare(Runner::Gog, app_name)?;
        self.emit_status(app_name, GameStatusKind::Launching);
        let install_dir = PathBuf::from(&g.install_path);
        let spec = match installed::launch_spec(&install_dir, app_name) {
            Ok(s) => s,
            Err(e) => {
                self.emit_status(app_name, GameStatusKind::Done);
                return Err(e);
            }
        };
        let log = GameLog::open(Runner::Gog, app_name, "launch");
        log.write(&format!(
            "{} {}",
            spec.executable,
            crate::games::redact(&spec.args).join(" ")
        ));
        if let Err(e) = self.ctx.games.launch(ticket, spec, Some(install_dir)).await {
            self.emit_status(app_name, GameStatusKind::Done);
            return Err(e);
        }
        Ok(())
    }

    async fn extra_info(&self, app_name: &str, lang: &str) -> Result<GameExtra> {
        product_id(app_name)?;
        Ok(api::gamesdb_cached(app_name)
            .map(|v| api::extra(&v, lang))
            .unwrap_or_default())
    }

    /// Game folders are recognised by their `goggame-<id>.info` (the main
    /// product's id is the one we own), in `dir` itself and its subfolders.
    async fn find_importable(&self, dir: &Path) -> Result<Vec<ImportCandidate>> {
        if !self.auth.exists() {
            return Ok(Vec::new());
        }
        let games = self.library(false).await?;
        let owned: HashMap<&str, &GameInfo> = games
            .iter()
            .filter(|g| !g.is_installed)
            .map(|g| (g.app_name.as_str(), g))
            .collect();
        let mut dirs = vec![dir.to_path_buf()];
        if let Ok(rd) = std::fs::read_dir(dir) {
            dirs.extend(
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.is_dir()),
            );
        }
        let mut out: Vec<ImportCandidate> = Vec::new();
        for d in dirs.into_iter().filter(|d| super::has_game_files(d)) {
            for id in installed::info_ids(&d) {
                if let Some(g) = owned.get(id.as_str())
                    && !out.iter().any(|c| c.app_name == g.app_name)
                {
                    out.push(ImportCandidate {
                        runner: Runner::Gog,
                        app_name: g.app_name.clone(),
                        title: g.title.clone(),
                        path: d.to_string_lossy().into_owned(),
                    });
                }
            }
        }
        out.sort_by(|a, b| a.title.cmp(&b.title));
        tracing::info!(dir = %dir.display(), found = out.len(), "GOG import scan");
        Ok(out)
    }

    /// Build, language and DLCs are read from the folder; the manifest of
    /// that build is fetched so updates and repair work as for a game
    /// installed here.
    async fn import_game(&self, app_name: &str, path: &str) -> Result<()> {
        if !Path::new(path).is_dir() {
            return Err(Error::Other(format!("folder not found: {path}")));
        }
        self.require_login()?;
        let found = native::import::scan(&self.client, Path::new(path), PLATFORM).await?;
        if found.app_name != app_name {
            return Err(Error::Other(format!(
                "{path} holds another game ({})",
                found.app_name
            )));
        }
        if let Err(e) = native::import::restore_manifest(
            &self.client,
            app_name,
            PLATFORM,
            &found.build_id,
            &found.language,
            &found.dlcs,
        )
        .await
        {
            tracing::warn!(app = app_name, error = %e, "no GOG manifest; update / repair need a reinstall");
        }

        let install_path = PathBuf::from(path);
        let size_bytes = tokio::task::spawn_blocking(move || installed::dir_size(&install_path))
            .await
            .unwrap_or(0);
        let entry = GogInstalled {
            app_name: app_name.to_string(),
            install_path: path.to_string(),
            platform: PLATFORM.into(),
            version: found.version_name,
            build_id: found.build_id,
            language: found.language,
            dlcs: found.dlcs,
            size_bytes,
            branch: None,
            pinned: false,
        };
        self.save_installed(Some(entry), None).await?;
        if let Err(e) = self.cloud_config(app_name, path).await {
            tracing::debug!(app = app_name, error = %e, "GOG cloud-save check failed");
        }
        self.ctx
            .events
            .emit(CoreEvent::LibraryUpdated(Some(Runner::Gog)));
        tracing::info!(app = app_name, path, "GOG game imported");
        Ok(())
    }

    async fn save_locations(&self, app_name: &str) -> Result<Vec<SaveLocation>> {
        product_id(app_name)?;
        let g = self.installed_entry(app_name).await?;
        match self.cloud_config(app_name, &g.install_path).await? {
            Some(c) if c.enabled => {
                let roots =
                    crate::saves::roots(&*self.ctx.settings.read().await, Runner::Gog, app_name);
                Ok(roots
                    .map(|r| saves::resolve(&c, &g.install_path, &r))
                    .unwrap_or_default())
            }
            _ => Ok(Vec::new()),
        }
    }

    /// One location after the other, each against its own last-sync time
    /// (`saveTimestamps.json`).
    async fn sync_saves(
        &self,
        app_name: &str,
        locations: &[SaveLocation],
        mode: SaveSyncMode,
    ) -> Result<String> {
        product_id(app_name)?;
        self.require_login()?;
        let log = GameLog::open(Runner::Gog, app_name, "saves");
        let mut summaries = Vec::new();
        for l in locations {
            if matches!(mode, SaveSyncMode::Download | SaveSyncMode::ForceDownload) {
                tokio::fs::create_dir_all(&l.path).await?;
            }
            let ts = saves::timestamp(app_name, &l.name);
            log.write(&format!(
                "sync {} --name {} --ts {ts} {mode:?}",
                l.path, l.name
            ));
            let out = match native::saves::sync(
                &self.client,
                app_name,
                PLATFORM,
                Path::new(&l.path),
                &l.name,
                &ts,
                mode,
            )
            .await
            {
                Ok(o) => o,
                Err(e) => {
                    log.write(&e.to_string());
                    return Err(e);
                }
            };
            for line in &out.lines {
                log.write(line);
            }
            if out.timestamp != ts {
                saves::set_timestamp(app_name, &l.name, &out.timestamp);
            }
            let start = out.lines.len().saturating_sub(2);
            let s = out.lines[start..].join("\n");
            if !s.is_empty() {
                summaries.push(s);
            }
        }
        Ok(summaries.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_ui_language_when_offered() {
        let langs = vec![
            "en-US".to_string(),
            "de-DE".to_string(),
            "uk-UA".to_string(),
        ];
        assert_eq!(pick_language(&langs, "uk"), "uk-UA");
        assert_eq!(pick_language(&langs, "fr"), "en-US");
        assert_eq!(pick_language(&["pl-PL".to_string()], "uk"), "pl-PL");
        assert_eq!(pick_language(&[], "uk"), "en-US");
    }

    #[test]
    fn sums_sizes_for_language_and_dlcs() {
        let info = Info {
            folder_name: "Game".into(),
            version_name: "1.2".into(),
            build_id: "b1".into(),
            languages: vec!["en-US".into()],
            size: [
                ("*".to_string(), (100, 200)),
                ("en-US".to_string(), (10, 20)),
            ]
            .into(),
            dlcs: vec![native::info::DlcInfo {
                id: "42".into(),
                title: "DLC".into(),
                size: [("*".to_string(), (5, 7))].into(),
            }],
            dependencies: vec![],
            depot_version: 2,
            branches: vec!["beta".into()],
            builds: Vec::new(),
        };
        let i = build_info(&info, "uk");
        assert_eq!(i.branches, ["beta"]);
        assert_eq!(i.language, "en-US");
        assert_eq!(i.download_size, 115);
        assert_eq!(i.disk_size, 227);
        assert_eq!(i.dlcs, ["42"]);
        assert_eq!(i.folder_name, "Game");
        assert_eq!(i.version, "1.2");
    }
}
