//! Epic Games Store, through our own Epic client (`stores::epic`): login,
//! library, sizes, launch, import, uninstall, cloud saves, install / update /
//! repair. The module keeps its `legendary` name (it is the `Runner` id
//! stored in configs) and legendary's file layout in `legendaryConfig/`.
//!
//! Port of `src/backend/storeManagers/legendary/{library,games,user}.ts`.

mod extra;
pub(crate) mod library;
mod saves;
mod third_party;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock};

use crate::error::{Error, Result};
use crate::events::CoreEvent;
use crate::paths;
use crate::secrets::SecretFile;
use crate::stores::epic;
use crate::stores::info_cache::InfoCache;
use crate::stores::{GameLog, StoreContext, StoreManager, folder_key};
use crate::types::{
    GameInfo, GameStatus, GameStatusKind, ImportCandidate, InstallInfo, InstallOptions,
    InstallPlatform, LaunchSpec, Runner, SaveLocation, SaveSyncMode, StoreStatus,
};

pub const VAULT_KEY: &str = "epic-user";

/// Epic's web login; after success it redirects to `localhost/...?code=…`.
pub const LOGIN_URL: &str = "https://www.epicgames.com/id/login?responseType=code";
pub const LOGIN_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) EpicGamesLauncher";

/// Store front shown in the Store tab; `{lang}` is replaced (en-US, uk, …).
pub const STORE_URL: &str = "https://store.epicgames.com/{lang}/";
/// The store's own "Sign in" page. Right after a launcher login Epic still
/// wants the account confirmed once for the web (its silent `prompt=none`
/// check fails until then); this page shows that one-click choice.
pub const STORE_SIGNIN_URL: &str =
    "https://store.epicgames.com/login?state=%2F{lang}%2F&lang={lang}";

pub struct Legendary {
    ctx: StoreContext,
    /// The Epic session, in the vault.
    user: Arc<SecretFile>,
    /// Epic's API, spoken directly.
    epic: Arc<epic::Epic>,
    library: RwLock<Vec<GameInfo>>,
    /// Set once the metadata was synced in this session.
    refreshed: Mutex<bool>,
    /// When the last sync ran; also serialises concurrent refreshes.
    last_list: Mutex<Option<Instant>>,
    /// Build sizes per "app:platform" (like Heroic's installStore).
    info_cache: Mutex<HashMap<String, InstallInfo>>,
    /// The same, kept across restarts.
    disk_info: InfoCache,
    /// Downloads in progress (`epic::download`), by app name.
    active: Mutex<HashMap<String, Arc<epic::download::CancelFlag>>>,
    /// For Epic store pages (description, screenshots).
    http: reqwest::Client,
}

impl Legendary {
    pub fn new(ctx: StoreContext) -> Self {
        let user = SecretFile::new(
            ctx.vault.clone(),
            VAULT_KEY,
            paths::legendary_config_dir().join("user.json"),
        );
        // a user.json left by an older version (or legendary itself)
        if let Err(e) = user.absorb_leftover() {
            tracing::error!(error = %e, "could not absorb leftover Epic credentials");
        }
        let epic = Arc::new(epic::Epic::new(user.clone(), paths::legendary_config_dir()));
        // ownership tokens of launches the app did not survive
        epic::launch::remove_ownership_tokens(&paths::legendary_config_dir().join("tmp"));
        Self {
            ctx,
            epic,
            user,
            library: RwLock::new(Vec::new()),
            refreshed: Mutex::new(false),
            last_list: Mutex::new(None),
            info_cache: Mutex::new(HashMap::new()),
            disk_info: InfoCache::new("legendary"),
            active: Mutex::new(HashMap::new()),
            // Epic's GraphQL answers launcher clients only
            http: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) EpicGamesLauncher")
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
        }
    }

    /// The session's display name (through `Session`, which moves the tokens
    /// straight into zeroed strings).
    fn username(&self) -> Option<String> {
        self.epic
            .profile()
            .ok()
            .map(|(_, name)| name)
            .filter(|n| !n.is_empty())
    }

    fn account_id(&self) -> Option<String> {
        self.epic
            .profile()
            .ok()
            .map(|(id, _)| id)
            .filter(|id| !id.is_empty())
    }

    /// See [`crate::stores::Stores::seal_leftovers`].
    pub fn seal_leftover(&self) -> Result<bool> {
        // games read their ownership token only at start
        epic::launch::remove_ownership_tokens(&paths::legendary_config_dir().join("tmp"));
        self.user.absorb_leftover()
    }

    fn require_login(&self) -> Result<()> {
        if self.user.exists() {
            Ok(())
        } else {
            Err(Error::NotLoggedIn("Epic Games".into()))
        }
    }

    /// The launcher of an EA / Ubisoft title, when the game has one we support.
    fn third_party_of(game: &GameInfo) -> Option<third_party::Launcher> {
        game.third_party_managed_app
            .as_deref()
            .and_then(third_party::Launcher::from_app)
    }

    /// A status with a note for the UI ("Installing the EA app").
    fn emit_note(&self, app_name: &str, status: GameStatusKind, note: impl Into<String>) {
        self.ctx.events.emit(CoreEvent::GameStatus(GameStatus {
            app_name: app_name.to_string(),
            runner: Runner::Legendary,
            status,
            progress: Some(crate::types::DownloadProgress::default()),
            message: Some(note.into()),
        }));
    }

    /// "Install" an EA / Ubisoft title: get that launcher onto the PC (its
    /// installer, run once; the game itself is downloaded by the launcher
    /// at the first start) and remember the game as installed. Heroic's
    /// `installEA` / `installUbisoft`. Linux: the installer runs in the
    /// game's Wine prefix through Proton.
    async fn install_third_party(
        &self,
        app_name: &str,
        launcher: third_party::Launcher,
        platform: InstallPlatform,
    ) -> Result<()> {
        self.emit_note(
            app_name,
            GameStatusKind::Installing,
            format!("Installing {}", launcher.display_name()),
        );
        let log = GameLog::open(Runner::Legendary, app_name, "install");
        log.write(&format!(
            "third-party install through {}",
            launcher.display_name()
        ));
        let result = async {
            let installer =
                third_party::fetch_installer(&self.http, launcher, &paths::epic_redist_dir())
                    .await?;
            log.write(&format!("running {}", installer.display()));
            let args: Vec<String> = launcher
                .installer_args()
                .iter()
                .map(|s| s.to_string())
                .collect();
            let code = if crate::platform::current().needs_wine() {
                let compat = self
                    .ctx
                    .games
                    .compat()
                    .ok_or_else(|| Error::Other("Proton is not available".into()))?;
                let on_prepare = |p: crate::compat::Preparing| {
                    if let Some(m) = p.message {
                        self.emit_note(app_name, GameStatusKind::Installing, m);
                    }
                };
                compat
                    .run_in_prefix(
                        Runner::Legendary,
                        app_name,
                        &installer,
                        &args,
                        None,
                        &on_prepare,
                        &|| false,
                    )
                    .await?
                    .code()
            } else {
                let mut installer_cmd = tokio::process::Command::new(&installer);
                crate::platform::child_env::apply(&mut installer_cmd);
                installer_cmd
                    .args(&args)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await
                    .map_err(|e| {
                        Error::Other(format!(
                            "could not start the {} installer: {e}",
                            launcher.display_name()
                        ))
                    })?
                    .code()
            };
            log.write(&format!("installer exit code {code:?}"));
            // Heroic: 3 = the user closed the installer
            if code == Some(3) {
                return Err(Error::Cancelled);
            }
            third_party::add_installed(
                &paths::legendary_config_dir(),
                app_name,
                platform.legendary_name(),
            )
        }
        .await;
        self.reload_library_from_disk().await;
        match &result {
            Ok(()) => {
                log.write("finished");
                self.emit_status(app_name, GameStatusKind::Done);
            }
            Err(Error::Cancelled) => {
                log.write("cancelled");
                self.emit_status(app_name, GameStatusKind::Done);
            }
            Err(e) => {
                log.write(&e.to_string());
                self.ctx.events.emit(CoreEvent::GameStatus(GameStatus {
                    app_name: app_name.to_string(),
                    runner: Runner::Legendary,
                    status: GameStatusKind::Error,
                    progress: None,
                    message: Some(e.to_string()),
                }));
            }
        }
        result
    }

    /// Start an EA / Ubisoft title: open the launcher's URI with Epic's
    /// exchange code (legendary's `launch --origin` / `--ubisoft`). The
    /// launcher takes it from there; we cannot follow the game's process.
    async fn launch_third_party(
        &self,
        app_name: &str,
        launcher: third_party::Launcher,
    ) -> Result<()> {
        self.emit_status(app_name, GameStatusKind::Launching);
        let result = async {
            let offline = !crate::net::reachable(Runner::Legendary).await;
            let token = if offline {
                "0".to_string()
            } else {
                match self.epic.exchange_code().await {
                    Ok(code) => code.to_string(),
                    Err(e) => {
                        tracing::warn!(app = app_name, error = %e, "no Epic exchange code; the launcher asks for the login itself");
                        "0".to_string()
                    }
                }
            };
            let (account_id, display_name) = self.epic.profile()?;
            let language = self.ctx.settings.read().await.language.clone();
            let metadata = epic::read_json(&epic::metadata_path(&paths::legendary_config_dir(), app_name)).unwrap_or_default();
            let uri = third_party::launch_uri(launcher, app_name, &metadata, &token, &account_id, &display_name, &language)?;
            let log = GameLog::open(Runner::Legendary, app_name, "launch");
            log.write(&format!("{}: {}", launcher.display_name(), crate::games::redact_text(&uri)));
            if crate::platform::current().needs_wine() {
                // Wine's `start` opens the URI with the handler the launcher registered in the prefix
                let compat = self.ctx.games.compat().ok_or_else(|| Error::Other("Proton is not available".into()))?;
                let on_prepare = |p: crate::compat::Preparing| {
                    if let Some(m) = p.message {
                        self.emit_note(app_name, GameStatusKind::Launching, m);
                    }
                };
                compat.run_in_prefix(Runner::Legendary, app_name, std::path::Path::new("start"), &[uri], None, &on_prepare, &|| false).await?;
                return Ok(());
            }
            #[cfg(windows)]
            {
                crate::platform::open_uri(&uri).map_err(|e| Error::Other(format!("could not open {}: {e}", launcher.display_name())))
            }
            #[cfg(not(windows))]
            Err(Error::Other("no way to open the launcher here".into()))
        }
        .await;
        self.emit_status(app_name, GameStatusKind::Done);
        result
    }

    /// The build's prerequisite installer (Visual C++, DirectX…) once after
    /// the install, as legendary offers on Windows and Heroic does: elevated
    /// on Windows, through Proton in the prefix on Linux, where Heroic also
    /// registers the `com.epicgames.launcher` URL class some games check.
    /// Marked done either way (`prereq_info.installed`, legendary's flag),
    /// so a failing installer is not retried at every update.
    async fn run_prereqs(&self, app_name: &str, log: &GameLog) {
        let dir = paths::legendary_config_dir();
        let mut all = epic::installed::read_installed_raw(&dir);
        let Some(entry) = all.get_mut(app_name) else {
            return;
        };
        let Some(info) = entry.get("prereq_info").filter(|v| v.is_object()).cloned() else {
            return;
        };
        if info.get("installed").and_then(serde_json::Value::as_bool) == Some(true) {
            return;
        }
        let path = info
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .replace('\\', "/");
        let args = info
            .get("args")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let name = info
            .get("name")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("prerequisites")
            .to_string();
        let install_path = entry
            .get("install_path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let wine = crate::platform::current().needs_wine();
        let on_prepare = |p: crate::compat::Preparing| {
            if let Some(m) = p.message {
                self.emit_note(app_name, GameStatusKind::Installing, m);
            }
        };
        if wine && let Some(compat) = self.ctx.games.compat() {
            let args = [
                "add".to_string(),
                "HKEY_CLASSES_ROOT\\com.epicgames.launcher".to_string(),
                "/f".to_string(),
            ];
            if let Err(e) = compat
                .run_in_prefix(
                    Runner::Legendary,
                    app_name,
                    std::path::Path::new("reg"),
                    &args,
                    None,
                    &on_prepare,
                    &|| false,
                )
                .await
            {
                log.write(&format!("registry key: {e}"));
            }
        }
        // run elevated: only a program inside the game folder (installed.json is a plain file)
        let exe = if path.is_empty() || install_path.is_empty() {
            None
        } else {
            match epic::installed::locate_checked(
                std::path::Path::new(&install_path),
                path.trim_start_matches('/'),
                false,
            ) {
                Ok(exe) => Some(exe),
                Err(e) => {
                    tracing::warn!(app = app_name, error = %e, "Epic prerequisites not run");
                    log.write(&format!("prerequisites refused: {e}"));
                    None
                }
            }
        };
        if let Some(exe) = exe {
            self.emit_note(
                app_name,
                GameStatusKind::Installing,
                format!("Installing {name}"),
            );
            let cwd = exe
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(&install_path));
            let argv = epic::launch::split_args_keep_quotes(&args);
            log.write(&format!("prerequisites: {} {}", exe.display(), args));
            // "Cancel" stops the installer (or stops waiting for it)
            let cancel = epic::download::CancelFlag::new();
            self.active
                .lock()
                .await
                .insert(app_name.to_string(), cancel.clone());
            let cancelled = || cancel.is_set();
            let result: Result<String> = if wine {
                match self.ctx.games.compat() {
                    Some(compat) => compat
                        .run_in_prefix(
                            Runner::Legendary,
                            app_name,
                            &exe,
                            &argv,
                            Some(&cwd),
                            &on_prepare,
                            &cancelled,
                        )
                        .await
                        .map(|s| format!("{s}")),
                    None => Err(Error::Other("Proton is not available".into())),
                }
            } else {
                #[cfg(windows)]
                {
                    let params = crate::stores::gog::setup::windows_command_line(&argv);
                    let (exe, cwd) = (exe.clone(), cwd.clone());
                    let cancel = cancel.clone();
                    tokio::task::spawn_blocking(move || {
                        crate::platform::run_elevated(&exe, &params, &cwd, &|| cancel.is_set())
                    })
                    .await
                    .map_err(|e| Error::Other(format!("prerequisites task failed: {e}")))
                    .and_then(|r| r.map(|code| format!("exit code {code}")).map_err(Error::Io))
                }
                #[cfg(not(windows))]
                Err(Error::Other("no way to run Windows programs here".into()))
            };
            self.active.lock().await.remove(app_name);
            match result {
                Ok(s) => log.write(&format!("prerequisites: {s}")),
                Err(e) => {
                    tracing::warn!(app = app_name, error = %e, "Epic prerequisites not installed");
                    log.write(&format!("prerequisites failed: {e}"));
                }
            }
        }
        if let Some(obj) = entry
            .get_mut("prereq_info")
            .and_then(serde_json::Value::as_object_mut)
        {
            obj.insert("installed".into(), serde_json::Value::Bool(true));
        }
        if let Err(e) = epic::installed::write_installed_raw(&dir, all) {
            tracing::warn!(app = app_name, error = %e, "could not mark the prerequisites as installed");
        }
    }

    fn emit_status(&self, app_name: &str, status: GameStatusKind) {
        self.ctx.events.emit(CoreEvent::GameStatus(GameStatus::new(
            Runner::Legendary,
            app_name,
            status,
        )));
    }

    /// Re-read installed.json / metadata into the cache and tell the UI.
    async fn reload_library_from_disk(&self) {
        let games = library::load_library(&paths::legendary_config_dir());
        *self.library.write().await = games;
        self.ctx
            .events
            .emit(CoreEvent::LibraryUpdated(Some(Runner::Legendary)));
    }

    /// Bring `assets.json` and the metadata up to date (`epic::library`).
    /// Failures only log: the library on disk is still shown.
    async fn refresh_metadata(&self) {
        match epic::library::sync(self.epic.clone()).await {
            Ok(_) => *self.refreshed.lock().await = true,
            Err(e) => {
                tracing::warn!(error = %e, "Epic library sync failed; using the metadata on disk")
            }
        }
    }

    /// Install / update / repair by `epic::download`: the status with
    /// progress while it runs, `Done` or `Error` after. A cancel ends it
    /// with `Cancelled`; the resume file keeps what is done.
    async fn download(
        &self,
        app_name: &str,
        kind: epic::download::Kind,
        status: GameStatusKind,
        base_path: Option<PathBuf>,
        platform: InstallPlatform,
    ) -> Result<()> {
        self.require_login()?;
        let cancel = {
            let mut active = self.active.lock().await;
            if active.contains_key(app_name) {
                return Err(Error::Busy(app_name.to_string()));
            }
            let flag = epic::download::CancelFlag::new();
            active.insert(app_name.to_string(), flag.clone());
            flag
        };
        self.emit_status(app_name, status);
        let (events, app) = (self.ctx.events.clone(), app_name.to_string());
        let on_progress: epic::download::ProgressFn = Arc::new(move |_phase, progress| {
            events.emit(CoreEvent::GameStatus(GameStatus {
                app_name: app.clone(),
                runner: Runner::Legendary,
                status,
                progress: Some(progress),
                message: None,
            }));
        });
        let max_workers = self.ctx.settings.read().await.max_workers as usize;
        let request = epic::download::Request {
            app_name: app_name.to_string(),
            platform: platform.legendary_name().to_string(),
            base_path,
            kind,
            max_workers,
        };
        let log = GameLog::open(Runner::Legendary, app_name, "install");
        log.write(&format!("{kind:?} platform={}", platform.legendary_name()));
        let out = epic::download::run(&self.epic, request, on_progress, cancel).await;
        self.active.lock().await.remove(app_name);
        self.reload_library_from_disk().await;
        match out {
            Ok(()) => {
                log.write("finished");
                if kind == epic::download::Kind::Install && platform == InstallPlatform::Windows {
                    self.run_prereqs(app_name, &log).await;
                }
                self.emit_status(app_name, GameStatusKind::Done);
                Ok(())
            }
            Err(Error::Cancelled) => {
                log.write("cancelled");
                self.emit_status(app_name, GameStatusKind::Done);
                Err(Error::Cancelled)
            }
            Err(e) => {
                log.write(&e.to_string());
                self.ctx.events.emit(CoreEvent::GameStatus(GameStatus {
                    app_name: app_name.to_string(),
                    runner: Runner::Legendary,
                    status: GameStatusKind::Error,
                    progress: None,
                    message: Some(e.to_string()),
                }));
                Err(e)
            }
        }
    }
}

#[async_trait]
impl StoreManager for Legendary {
    fn runner(&self) -> Runner {
        Runner::Legendary
    }

    async fn status(&self) -> StoreStatus {
        let username = self.username();
        StoreStatus {
            runner: Runner::Legendary,
            logged_in: username.is_some(),
            username,
            helper_available: true,
        }
    }

    async fn login(&self, code: &str) -> Result<StoreStatus> {
        let code = code.trim();
        if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(Error::Other("invalid authorization code".into()));
        }
        self.epic.login_with_code(code).await?;
        *self.refreshed.lock().await = false;
        let status = self.status().await;
        if !status.logged_in {
            return Err(Error::Other(
                "login finished but no user info was stored".into(),
            ));
        }
        tracing::info!(user = ?status.username, "logged in to Epic");
        self.ctx.events.emit(CoreEvent::StoreLogin(status.clone()));
        Ok(status)
    }

    async fn logout(&self) -> Result<()> {
        // also stops a running download from storing its token again
        self.user.revoke().await?;
        self.library.write().await.clear();
        *self.refreshed.lock().await = false;
        let status = self.status().await;
        self.ctx.events.emit(CoreEvent::StoreLogin(status));
        self.ctx
            .events
            .emit(CoreEvent::LibraryUpdated(Some(Runner::Legendary)));
        Ok(())
    }

    async fn library(&self, force_refresh: bool) -> Result<Vec<GameInfo>> {
        if !self.user.exists() {
            return Ok(Vec::new());
        }
        // Only an explicit refresh hits the network; the startup refresh runs in
        // the background (see app setup), so the UI shows the cached library at once.
        if force_refresh {
            // One refresh at a time, and not more than every 30 s. The lock is
            // taken only here so plain reads never wait for the network.
            let mut last = self.last_list.lock().await;
            let recent = last.is_some_and(|t| t.elapsed() < Duration::from_secs(30));
            if !(recent && *self.refreshed.lock().await) {
                self.refresh_metadata().await;
                *last = Some(Instant::now());
                let games = library::load_library(&paths::legendary_config_dir());
                tracing::info!(count = games.len(), "Epic library loaded");
                *self.library.write().await = games;
            }
        } else if self.library.read().await.is_empty() {
            let started = Instant::now();
            let games = library::load_library(&paths::legendary_config_dir());
            tracing::info!(
                count = games.len(),
                ms = started.elapsed().as_millis() as u64,
                "Epic library read from disk"
            );
            *self.library.write().await = games;
        }
        Ok(self.library.read().await.clone())
    }

    /// legendary's `get_dlc_for_game`: the DLCs with a build for the game's
    /// platform, installed or not.
    async fn dlcs(&self, app_name: &str) -> Result<Vec<crate::types::DlcEntry>> {
        let dir = paths::legendary_config_dir();
        let game = epic::read_json(&epic::metadata_path(&dir, app_name))
            .ok_or_else(|| Error::GameNotFound(app_name.into()))?;
        let (_, catalog_id) = epic::library::catalog_ids(&game);
        let installed = library::read_installed(&dir);
        let platform = installed
            .get(app_name)
            .map(|i| i.platform.clone())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "Windows".into());
        let mut out = Vec::new();
        for dlc in epic::library::dlcs_of(&dir, &catalog_id, &platform) {
            let title = epic::read_json(&epic::metadata_path(&dir, &dlc))
                .and_then(|v| {
                    v.pointer("/metadata/title")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| dlc.clone());
            out.push(crate::types::DlcEntry {
                installed: installed.contains_key(&dlc),
                installable: true,
                app_name: dlc,
                title,
            });
        }
        out.sort_by_key(|d| d.title.to_lowercase());
        Ok(out)
    }

    async fn achievements(&self, app_name: &str, lang: &str) -> Result<crate::types::Achievements> {
        self.require_login()?;
        let game = epic::read_json(&epic::metadata_path(
            &paths::legendary_config_dir(),
            app_name,
        ))
        .ok_or_else(|| Error::GameNotFound(app_name.into()))?;
        let (namespace, _) = epic::library::catalog_ids(&game);
        if namespace.is_empty() {
            return Ok(crate::types::Achievements::default());
        }
        let (account_id, _) = self.epic.profile()?;
        epic::achievements::fetch(&self.epic, &namespace, &account_id, lang).await
    }

    async fn change_install_path(&self, app_name: &str, new_path: &std::path::Path) -> Result<()> {
        epic::installed::change_install_path(&paths::legendary_config_dir(), app_name, new_path)?;
        self.info_cache.lock().await.clear();
        self.reload_library_from_disk().await;
        Ok(())
    }

    async fn install_info(&self, app_name: &str, platform: InstallPlatform) -> Result<InstallInfo> {
        self.require_login()?;
        if let Some(game) = self.game_info(app_name).await?
            && Self::third_party_of(&game).is_some()
        {
            // nothing to download from Epic: the other launcher does that
            return Ok(InstallInfo {
                app_name: app_name.to_string(),
                title: game.title,
                version: String::new(),
                download_size: 0,
                disk_size: 0,
                platform,
                ..Default::default()
            });
        }
        let key = format!("{app_name}:{}", platform.legendary_name());
        if let Some(hit) = self.info_cache.lock().await.get(&key) {
            return Ok(hit.clone());
        }
        // the latest build, as of the last sync: sizes on disk are reused
        // until it changes
        let build = library::read_latest_versions(&paths::legendary_config_dir())
            .remove(&(platform.legendary_name().to_string(), app_name.to_string()))
            .unwrap_or_default();
        let max_age = Duration::from_secs(if build.is_empty() {
            86_400
        } else {
            30 * 86_400
        });
        if let Some(hit) = self.disk_info.get(&key, &build, max_age) {
            self.info_cache.lock().await.insert(key, hit.clone());
            return Ok(hit);
        }
        let b =
            epic::installed::build_info(&self.epic, app_name, platform.legendary_name()).await?;
        let title = self
            .library
            .read()
            .await
            .iter()
            .find(|g| g.app_name == app_name)
            .map(|g| g.title.clone())
            .unwrap_or_else(|| app_name.to_string());
        let info = InstallInfo {
            app_name: app_name.to_string(),
            title,
            version: b.version,
            download_size: b.download_size,
            disk_size: b.disk_size,
            platform,
            branches: Vec::new(),
            builds: Vec::new(),
        };
        if info.download_size > 0 {
            self.disk_info.put(&key, &build, &info);
            self.info_cache.lock().await.insert(key, info.clone());
        }
        Ok(info)
    }

    async fn install(&self, app_name: &str, opts: InstallOptions) -> Result<()> {
        if let Some(game) = self.game_info(app_name).await?
            && let Some(launcher) = Self::third_party_of(&game)
        {
            return self
                .install_third_party(app_name, launcher, opts.platform)
                .await;
        }
        if opts.path.trim().is_empty() {
            return Err(Error::Other("install path is empty".into()));
        }
        std::fs::create_dir_all(&opts.path)?;
        self.download(
            app_name,
            epic::download::Kind::Install,
            GameStatusKind::Installing,
            Some(PathBuf::from(&opts.path)),
            opts.platform,
        )
        .await
    }

    async fn update(&self, app_name: &str) -> Result<()> {
        let game = self
            .game_info(app_name)
            .await?
            .ok_or_else(|| Error::GameNotFound(app_name.into()))?;
        let platform = game
            .install
            .as_ref()
            .map(|i| i.platform)
            .unwrap_or_default();
        self.download(
            app_name,
            epic::download::Kind::Update,
            GameStatusKind::Updating,
            None,
            platform,
        )
        .await
    }

    async fn repair(&self, app_name: &str) -> Result<()> {
        let game = self
            .game_info(app_name)
            .await?
            .ok_or_else(|| Error::GameNotFound(app_name.into()))?;
        if !game.is_installed {
            return Err(Error::Other(format!("{} is not installed", game.title)));
        }
        let platform = game
            .install
            .as_ref()
            .map(|i| i.platform)
            .unwrap_or_default();
        self.download(
            app_name,
            epic::download::Kind::Repair,
            GameStatusKind::Repairing,
            None,
            platform,
        )
        .await
    }

    async fn extra_info(&self, app_name: &str, lang: &str) -> Result<crate::types::GameExtra> {
        let path = epic::metadata_path(&paths::legendary_config_dir(), app_name);
        let raw = tokio::fs::read_to_string(&path)
            .await
            .map_err(|_| Error::GameNotFound(app_name.into()))?;
        let v: serde_json::Value = serde_json::from_str(&raw)?;
        let md = v
            .get("metadata")
            .ok_or_else(|| Error::GameNotFound(app_name.into()))?;
        let namespace = md
            .get("namespace")
            .and_then(|s| s.as_str())
            .unwrap_or_default();
        let title = md.get("title").and_then(|s| s.as_str()).unwrap_or(app_name);
        if namespace.is_empty() {
            return Ok(crate::types::GameExtra::default());
        }
        extra::fetch(&self.http, namespace, title, lang).await
    }

    async fn cancel(&self, app_name: &str) -> Result<()> {
        match self.active.lock().await.get(app_name) {
            Some(flag) => {
                flag.cancel();
                Ok(())
            }
            None => Err(Error::Other(format!("{app_name} is not downloading"))),
        }
    }

    async fn find_importable(&self, dir: &std::path::Path) -> Result<Vec<ImportCandidate>> {
        if !self.user.exists() {
            return Ok(Vec::new());
        }
        let games = self.library(false).await?;
        // Heroic and legendary install into <base>/<FolderName>; fall back to
        // the app name and the title for folders copied by hand.
        let mut by_key: HashMap<String, &GameInfo> = HashMap::new();
        for g in games
            .iter()
            .filter(|g| !g.is_installed && !g.is_dlc && g.installable)
        {
            for name in [
                g.folder_name.as_deref(),
                Some(g.app_name.as_str()),
                Some(g.title.as_str()),
            ]
            .into_iter()
            .flatten()
            {
                by_key.entry(folder_key(name)).or_insert(g);
            }
        }
        let mut dirs = vec![dir.to_path_buf()];
        if let Ok(rd) = std::fs::read_dir(dir) {
            dirs.extend(
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.is_dir()),
            );
        }
        let mut out: Vec<ImportCandidate> = Vec::new();
        for d in dirs {
            let Some(name) = d.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if let Some(g) = by_key.get(&folder_key(name))
                && !out.iter().any(|c| c.app_name == g.app_name)
                && super::has_game_files(&d)
            {
                out.push(ImportCandidate {
                    runner: Runner::Legendary,
                    app_name: g.app_name.clone(),
                    title: g.title.clone(),
                    path: d.to_string_lossy().into_owned(),
                });
            }
        }
        out.sort_by(|a, b| a.title.cmp(&b.title));
        tracing::info!(dir = %dir.display(), found = out.len(), "import scan");
        Ok(out)
    }

    async fn import_game(&self, app_name: &str, path: &str) -> Result<()> {
        self.require_login()?;
        if !std::path::Path::new(path).is_dir() {
            return Err(Error::Other(format!("folder not found: {path}")));
        }
        // the library must know the game (right after a login it may not yet)
        if !epic::metadata_path(&paths::legendary_config_dir(), app_name).is_file() {
            self.refresh_metadata().await;
        }
        epic::installed::import(
            &self.epic,
            app_name,
            std::path::Path::new(path),
            "Windows",
            true,
        )
        .await?;
        self.reload_library_from_disk().await;
        Ok(())
    }

    async fn uninstall(&self, app_name: &str) -> Result<()> {
        let game = self.game_info(app_name).await?;
        // a DLC shares the game's folder: only its own files go (`epic::installed::uninstall`)
        let is_dlc = game.as_ref().is_some_and(|g| g.is_dlc);
        if game
            .as_ref()
            .is_some_and(|g| Self::third_party_of(g).is_some())
        {
            // only our record: the launcher owns the files (Heroic does the same)
            third_party::remove_installed(&paths::legendary_config_dir(), app_name)?;
            self.reload_library_from_disk().await;
            self.emit_status(app_name, GameStatusKind::Done);
            return Ok(());
        }
        // the files the build listed go first; whatever the game wrote next
        // to them (saves, configs, logs) is cleaned up afterwards
        let install_dir = game
            .and_then(|g| g.install)
            .map(|i| std::path::PathBuf::from(i.install_path))
            .filter(|p| !p.as_os_str().is_empty());
        self.emit_status(app_name, GameStatusKind::Uninstalling);
        let removed =
            epic::installed::uninstall(paths::legendary_config_dir(), app_name.to_string()).await;
        if removed.is_ok() && !is_dlc {
            if let Some(dir) = &install_dir {
                super::remove_game_dir(dir, &self.ctx.settings).await;
            }
            crate::compat::remove_prefix(&self.ctx.settings, Runner::Legendary, app_name).await;
        }
        self.reload_library_from_disk().await;
        self.emit_status(app_name, GameStatusKind::Done);
        removed
    }

    async fn launch(&self, app_name: &str) -> Result<()> {
        let game = self
            .game_info(app_name)
            .await?
            .ok_or_else(|| Error::GameNotFound(app_name.into()))?;
        if !game.is_installed {
            return Err(Error::Other(format!("{} is not installed", game.title)));
        }
        if let Some(launcher) = Self::third_party_of(&game) {
            return self.launch_third_party(app_name, launcher).await;
        }
        // Reserve the game first: a cancel while the launch is prepared
        // marks the ticket and the game is never spawned.
        let ticket = self.ctx.games.prepare(Runner::Legendary, app_name)?;
        self.emit_status(app_name, GameStatusKind::Launching);
        let language = self.ctx.settings.read().await.language.clone();
        // Without a network the game starts without Epic's login (games
        // that need Epic online then complain themselves). Offline is also
        // the fallback when the online attempt fails, e.g. Epic down or the
        // connection dropped meanwhile.
        let offline = !crate::net::reachable(Runner::Legendary).await;
        let mut spec = epic::launch::spec(&self.epic, app_name, offline, &language).await;
        if let Err(e) = &spec
            && !offline
            && !ticket.is_cancelled()
            && !matches!(e, Error::NotLoggedIn(_))
        {
            tracing::warn!(app = app_name, error = %e, "online launch failed; trying offline");
            if let Ok(s) = epic::launch::spec(&self.epic, app_name, true, &language).await {
                spec = Ok(s);
            }
        } else if offline {
            tracing::info!(
                app = app_name,
                can_run_offline = game.can_run_offline,
                "Epic not reachable; launching offline"
            );
        }
        let spec: LaunchSpec = match spec {
            Ok(s) => s,
            Err(e) => {
                self.emit_status(app_name, GameStatusKind::Done);
                return Err(if ticket.is_cancelled() {
                    Error::Cancelled
                } else {
                    e
                });
            }
        };
        let log = GameLog::open(Runner::Legendary, app_name, "launch");
        log.write(&format!(
            "{} {}",
            spec.executable,
            crate::games::redact(&spec.args).join(" ")
        ));
        let install_dir = game
            .install
            .as_ref()
            .map(|i| std::path::PathBuf::from(&i.install_path));
        if let Err(e) = self.ctx.games.launch(ticket, spec, install_dir).await {
            self.emit_status(app_name, GameStatusKind::Done);
            return Err(e);
        }
        Ok(())
    }

    async fn save_locations(&self, app_name: &str) -> Result<Vec<SaveLocation>> {
        let game = self
            .game_info(app_name)
            .await?
            .ok_or_else(|| Error::GameNotFound(app_name.into()))?;
        let (Some(folder), Some(install)) = (game.save_folder.as_deref(), game.install.as_ref())
        else {
            return Ok(Vec::new());
        };
        let epic_id = self.account_id();
        let roots = crate::saves::roots(
            &*self.ctx.settings.read().await,
            Runner::Legendary,
            app_name,
        );
        let path = roots
            .and_then(|r| saves::resolve(folder, &install.install_path, epic_id.as_deref(), &r))
            .unwrap_or_else(|| {
                tracing::warn!(
                    app = app_name,
                    folder,
                    "could not resolve the Epic save folder"
                );
                String::new()
            });
        Ok(vec![SaveLocation {
            name: SaveLocation::EPIC.into(),
            path,
        }])
    }

    /// `epic::saves`: the newer side wins; `mode` says what is allowed.
    async fn sync_saves(
        &self,
        app_name: &str,
        locations: &[SaveLocation],
        mode: SaveSyncMode,
    ) -> Result<String> {
        self.require_login()?;
        let dir = locations
            .first()
            .map(|l| l.path.clone())
            .filter(|p| !p.is_empty())
            .ok_or_else(|| Error::Other("the save folder is not known".into()))?;
        if matches!(mode, SaveSyncMode::Download | SaveSyncMode::ForceDownload) {
            std::fs::create_dir_all(&dir)?;
        }
        let log = GameLog::open(Runner::Legendary, app_name, "saves");
        log.write(&format!("sync {mode:?} {dir}"));
        let out = epic::saves::sync(&self.epic, app_name, std::path::Path::new(&dir), mode).await;
        match &out {
            Ok(summary) => log.write(summary),
            Err(e) => log.write(&e.to_string()),
        }
        out
    }
}
