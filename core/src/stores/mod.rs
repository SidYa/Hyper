//! Store managers. Each store implements [`StoreManager`]; [`Stores`] owns
//! one instance per [`Runner`] and is shared through the app state.

pub mod epic;
pub mod gog;
mod info_cache;
pub mod legendary;
pub mod moving;
pub mod sideload;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::config::AppSettings;
use crate::error::{Error, Result};
use crate::events::{CoreEvent, Events};
use crate::games::GameRunner;
use crate::net;
use crate::saves;
use crate::secrets::Vault;
use crate::types::{
    CloudSaves, GameInfo, GameStatus, GameStatusKind, InstallInfo, InstallOptions, Runner,
    SaveLocation, SaveSyncMode, StoreStatus,
};

/// Everything a store needs from its host.
#[derive(Clone)]
pub struct StoreContext {
    pub settings: Arc<RwLock<AppSettings>>,
    pub vault: Arc<Vault>,
    pub events: Events,
    pub games: Arc<GameRunner>,
}

#[async_trait]
pub trait StoreManager: Send + Sync {
    fn runner(&self) -> Runner;

    /// Login state + helper availability.
    async fn status(&self) -> StoreStatus;

    /// Finish a login with the code the store's web flow handed us.
    async fn login(&self, code: &str) -> Result<StoreStatus>;

    async fn logout(&self) -> Result<()>;

    /// Returns the (cached) library. `force_refresh` re-queries the store.
    async fn library(&self, force_refresh: bool) -> Result<Vec<GameInfo>>;

    /// Finds a single game by app name.
    async fn game_info(&self, app_name: &str) -> Result<Option<GameInfo>> {
        Ok(self
            .library(false)
            .await?
            .into_iter()
            .find(|g| g.app_name == app_name))
    }

    /// Sizes needed before installing.
    async fn install_info(
        &self,
        app_name: &str,
        platform: crate::types::InstallPlatform,
    ) -> Result<InstallInfo>;

    /// The add-ons of a game and whether they are installed.
    async fn dlcs(&self, _app_name: &str) -> Result<Vec<crate::types::DlcEntry>> {
        Ok(Vec::new())
    }

    /// The game's achievements with the account's progress (`lang`: the UI
    /// language for the texts). Empty when the store has none.
    async fn achievements(
        &self,
        _app_name: &str,
        _lang: &str,
    ) -> Result<crate::types::Achievements> {
        Ok(crate::types::Achievements::default())
    }

    /// Record a game's new folder after its files were moved there.
    async fn change_install_path(
        &self,
        _app_name: &str,
        _new_path: &std::path::Path,
    ) -> Result<()> {
        Err(Error::NotImplemented("moving this game"))
    }

    /// Blocking install; progress is reported through the event sink.
    async fn install(&self, app_name: &str, opts: InstallOptions) -> Result<()>;

    /// Blocking update; progress is reported through the event sink.
    async fn update(&self, app_name: &str) -> Result<()>;

    /// Verify the installed files and re-download broken ones. Blocking;
    /// progress is reported through the event sink.
    async fn repair(&self, _app_name: &str) -> Result<()> {
        Err(Error::NotImplemented("repairing games for this store"))
    }

    /// Store-page details for the game page (description, screenshots…).
    /// `lang` is the UI language code.
    async fn extra_info(&self, _app_name: &str, _lang: &str) -> Result<crate::types::GameExtra> {
        Ok(crate::types::GameExtra::default())
    }

    /// Abort the running install/update of `app_name`.
    async fn cancel(&self, app_name: &str) -> Result<()>;

    async fn uninstall(&self, app_name: &str) -> Result<()>;

    /// Start the game; returns once the process is running.
    async fn launch(&self, app_name: &str) -> Result<()>;

    /// Owned, not-installed games whose folders exist under `dir` (or that
    /// `dir` itself is). Used to re-attach games after reinstalling Windows.
    async fn find_importable(
        &self,
        _dir: &std::path::Path,
    ) -> Result<Vec<crate::types::ImportCandidate>> {
        Ok(Vec::new())
    }

    /// Register an existing installation at `path` as installed.
    async fn import_game(&self, _app_name: &str, _path: &str) -> Result<()> {
        Err(Error::NotImplemented("importing games for this store"))
    }

    /// Cloud-save folders of an installed game as the store defines them,
    /// resolved for this PC (an empty path when that failed). No entries:
    /// the game has no cloud saves.
    async fn save_locations(&self, _app_name: &str) -> Result<Vec<SaveLocation>> {
        Ok(Vec::new())
    }

    /// Sync `locations` (already checked by [`saves::check_save_dir`]) with
    /// the cloud. Returns a short summary of what the helper did.
    async fn sync_saves(
        &self,
        _app_name: &str,
        _locations: &[SaveLocation],
        _mode: SaveSyncMode,
    ) -> Result<String> {
        Err(Error::NotImplemented("cloud saves for this store"))
    }
}

pub struct Stores {
    pub legendary: Arc<legendary::Legendary>,
    pub gog: Arc<gog::Gog>,
    pub sideload: Arc<sideload::Sideload>,
    ctx: StoreContext,
    syncing: SyncSet,
    /// Pending uploads that failed online: not retried before this time.
    retry_at: std::sync::Mutex<HashMap<String, Instant>>,
    /// The download queue, once it exists (it owns us, hence weak): a game
    /// being downloaded is not moved.
    downloads: OnceLock<Weak<crate::downloads::DownloadManager>>,
}

impl Stores {
    pub fn new(ctx: StoreContext) -> Self {
        Self {
            legendary: Arc::new(legendary::Legendary::new(ctx.clone())),
            gog: Arc::new(gog::Gog::new(ctx.clone())),
            sideload: Arc::new(sideload::Sideload::new(ctx.clone())),
            ctx,
            syncing: SyncSet::default(),
            retry_at: std::sync::Mutex::default(),
            downloads: OnceLock::new(),
        }
    }

    /// Called by the download manager when it is created.
    pub(crate) fn attach_downloads(&self, downloads: Weak<crate::downloads::DownloadManager>) {
        let _ = self.downloads.set(downloads);
    }

    /// Whether an install / update / repair of the game is running or waits
    /// in the queue (paused ones included: they hold a resume file).
    async fn is_downloading(&self, runner: Runner, app_name: &str) -> bool {
        match self.downloads.get().and_then(Weak::upgrade) {
            Some(d) => d.has(runner, app_name).await,
            None => false,
        }
    }

    /// Games whose cloud saves are being synced right now (`runner:app`
    /// keys): closing the app would leave the sync half done.
    pub fn syncing_games(&self) -> Vec<(Runner, String)> {
        let set = self.syncing.lock().unwrap_or_else(|e| e.into_inner());
        set.iter()
            .filter_map(|key| {
                let (runner, app) = key.split_once(':')?;
                Some((
                    Runner::all().into_iter().find(|r| r.id() == runner)?,
                    app.to_string(),
                ))
            })
            .collect()
    }

    /// Cloud-save support and save folders of a game.
    pub async fn cloud_saves(&self, runner: Runner, app_name: &str) -> Result<CloudSaves> {
        // The save folders are Windows locations (AppData, Documents): on
        // Linux they resolve inside the game's Wine prefix (`saves::roots`).
        let locations = self.get(runner).save_locations(app_name).await?;
        Ok(CloudSaves {
            supported: !locations.is_empty(),
            locations: saves::describe(locations),
            pending_upload: saves::is_pending(runner, app_name),
        })
    }

    /// The folders to sync, every one of them checked.
    async fn sync_locations(&self, runner: Runner, app_name: &str) -> Result<Vec<SaveLocation>> {
        let state = self.cloud_saves(runner, app_name).await?;
        if !state.supported {
            return Err(Error::Other("this game has no cloud saves".into()));
        }
        let (roots, games_dir) = {
            let settings = self.ctx.settings.read().await;
            (
                saves::roots(&settings, runner, app_name),
                settings.default_install_path.clone(),
            )
        };
        let mut out = Vec::new();
        for l in state.locations {
            saves::check_save_dir(&l.path, roots.as_ref(), &games_dir)?;
            out.push(SaveLocation {
                name: l.name,
                path: l.path,
            });
        }
        Ok(out)
    }

    fn emit_status(
        &self,
        runner: Runner,
        app_name: &str,
        status: GameStatusKind,
        message: Option<String>,
    ) {
        let mut s = GameStatus::new(runner, app_name, status);
        s.message = message;
        self.ctx.events.emit(CoreEvent::GameStatus(s));
    }

    /// Sync a game's saves by hand. Refused while the game runs: its save
    /// files are in use and would change under the sync.
    pub async fn sync_saves(
        &self,
        runner: Runner,
        app_name: &str,
        mode: SaveSyncMode,
    ) -> Result<String> {
        if self.ctx.games.is_active(runner, app_name) {
            return Err(Error::Busy(app_name.to_string()));
        }
        let _claim = SyncClaim::take(&self.syncing, runner, app_name)
            .ok_or_else(|| Error::Busy(app_name.to_string()))?;
        let locations = self.sync_locations(runner, app_name).await?;
        self.emit_status(runner, app_name, GameStatusKind::Syncing, None);
        let out = self
            .get(runner)
            .sync_saves(app_name, &locations, mode)
            .await;
        self.emit_status(runner, app_name, GameStatusKind::Done, None);
        match &out {
            Ok(summary) => {
                tracing::info!(app = app_name, ?mode, summary, "saves synced");
                // the cloud now has these saves, or the user chose the cloud ones
                if mode != SaveSyncMode::Download && saves::is_pending(runner, app_name) {
                    saves::set_pending(runner, app_name, false);
                    self.ctx.events.emit(CoreEvent::SavesPending {
                        runner,
                        app_name: app_name.into(),
                        pending: false,
                    });
                }
            }
            Err(e) => tracing::warn!(app = app_name, ?mode, error = %e, "save sync failed"),
        }
        out
    }

    /// Upload the saves left from offline play (see [`saves::pending`]) for
    /// every store reachable now. A game running or syncing is skipped (its
    /// own exit hook uploads); after a failed upload the game waits
    /// [`PENDING_RETRY`] before the next try.
    pub async fn upload_pending(&self) {
        let list = saves::pending();
        if list.is_empty() {
            return;
        }
        let mut reachable: HashMap<Runner, bool> = HashMap::new();
        for p in list {
            let (runner, app) = (p.runner, p.app_name.as_str());
            let key = format!("{}:{app}", runner.id());
            let waiting = self
                .retry_at
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key)
                .is_some_and(|t| Instant::now() < *t);
            if waiting || self.ctx.games.is_active(runner, app) {
                continue;
            }
            let online = match reachable.get(&runner) {
                Some(r) => *r,
                None => *reachable
                    .entry(runner)
                    .or_insert(net::reachable(runner).await),
            };
            if !online {
                continue;
            }
            let Some(_claim) = SyncClaim::take(&self.syncing, runner, app) else {
                continue;
            };
            // the reachability check took a while: the game may have been
            // started meanwhile (its own exit hook uploads then)
            if self.ctx.games.is_active(runner, app) {
                continue;
            }
            let locations = match self.sync_locations(runner, app).await {
                Ok(l) => l,
                Err(e) => {
                    // uninstalled since, or no save folder any more: nothing to upload
                    tracing::warn!(app, error = %e, "dropping the pending save upload");
                    saves::set_pending(runner, app, false);
                    continue;
                }
            };
            self.emit_status(runner, app, GameStatusKind::Syncing, None);
            let out = self
                .get(runner)
                .sync_saves(app, &locations, SaveSyncMode::Upload)
                .await;
            self.emit_status(runner, app, GameStatusKind::Done, None);
            match out {
                Ok(summary) => {
                    tracing::info!(app, summary, "pending saves uploaded");
                    saves::set_pending(runner, app, false);
                    self.retry_at
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&key);
                    self.ctx.events.emit(CoreEvent::SavesPending {
                        runner,
                        app_name: app.into(),
                        pending: false,
                    });
                }
                Err(e) => {
                    tracing::warn!(app, error = %e, "uploading pending saves failed; retrying later");
                    self.retry_at
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(key, Instant::now() + PENDING_RETRY);
                }
            }
        }
    }

    /// [`Stores::upload_pending`] at start and then every minute, so saves
    /// played offline reach the cloud soon after the connection is back.
    pub async fn watch_pending(self: Arc<Self>) {
        loop {
            self.upload_pending().await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    }

    /// Launch a game. With auto-sync on, newer cloud saves are downloaded
    /// first and the local ones uploaded after the game exits. A failed sync
    /// is reported but never stops the game from starting (like Heroic).
    ///
    /// Offline (the store does not answer) the game starts without the
    /// download and the upload waits in [`saves::pending`] until the store is
    /// back, like Steam. Saves still waiting from an offline session are the
    /// newest, so the next online launch uploads them instead of downloading.
    pub async fn launch(&self, runner: Runner, app_name: &str) -> Result<()> {
        let store = self.get(runner);
        let claim = SyncClaim::take(&self.syncing, runner, app_name)
            .ok_or_else(|| Error::Busy(app_name.to_string()))?;
        if !self.ctx.settings.read().await.auto_sync_saves
            || self.ctx.games.is_active(runner, app_name)
        {
            drop(claim);
            return store.launch(app_name).await;
        }
        // auto-sync is global: games without cloud saves just start
        let supported = match self.cloud_saves(runner, app_name).await {
            Ok(s) => s.supported,
            Err(e) => {
                tracing::warn!(app = app_name, error = %e, "could not check cloud saves");
                true // let `sync_locations` report it
            }
        };
        if !supported {
            drop(claim);
            return store.launch(app_name).await;
        }
        let locations = match self.sync_locations(runner, app_name).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(app = app_name, error = %e, "auto-sync skipped");
                self.emit_status(
                    runner,
                    app_name,
                    GameStatusKind::Error,
                    Some(sync_failed(&e)),
                );
                return store.launch(app_name).await;
            }
        };

        if !net::reachable(runner).await {
            tracing::info!(
                app = app_name,
                "offline: saves are uploaded once the store is reachable"
            );
        } else if saves::is_pending(runner, app_name) {
            self.emit_status(runner, app_name, GameStatusKind::Syncing, None);
            match store
                .sync_saves(app_name, &locations, SaveSyncMode::Upload)
                .await
            {
                Ok(summary) => {
                    tracing::info!(
                        app = app_name,
                        summary,
                        "pending saves uploaded before launch"
                    );
                    saves::set_pending(runner, app_name, false);
                    self.ctx.events.emit(CoreEvent::SavesPending {
                        runner,
                        app_name: app_name.into(),
                        pending: false,
                    });
                }
                Err(e) => {
                    // still pending; never download over saves the cloud does not have
                    tracing::warn!(app = app_name, error = %e, "uploading pending saves failed");
                    self.emit_status(
                        runner,
                        app_name,
                        GameStatusKind::Error,
                        Some(sync_failed(&e)),
                    );
                }
            }
        } else {
            self.emit_status(runner, app_name, GameStatusKind::Syncing, None);
            match store
                .sync_saves(app_name, &locations, SaveSyncMode::Download)
                .await
            {
                Ok(summary) => {
                    tracing::info!(app = app_name, summary, "saves downloaded before launch")
                }
                Err(e) => {
                    tracing::warn!(app = app_name, error = %e, "downloading saves failed");
                    self.emit_status(
                        runner,
                        app_name,
                        GameStatusKind::Error,
                        Some(sync_failed(&e)),
                    );
                }
            }
        }

        let (hook_store, events, app) =
            (store.clone(), self.ctx.events.clone(), app_name.to_string());
        let syncing = self.syncing.clone();
        self.ctx.games.on_exit(
            runner,
            app_name,
            Box::new(move || {
                Box::pin(async move {
                    // the game was running until now, so only a manual sync
                    // that slipped in between can hold it: that one uploads
                    let Some(_claim) = SyncClaim::take(&syncing, runner, &app) else {
                        tracing::info!(app, "saves are being synced already; no upload after exit");
                        return;
                    };
                    upload_after_exit(&*hook_store, &events, runner, &app, &locations).await;
                })
            }),
        );
        let launched = store.launch(app_name).await;
        drop(claim);
        if launched.is_err() {
            self.ctx.games.clear_exit_hook(runner, app_name);
            self.emit_status(runner, app_name, GameStatusKind::Done, None);
        }
        launched
    }

    /// On exit: move any plaintext credentials still on disk into the vault.
    pub fn seal_leftovers(&self) {
        for (store, sealed) in [
            ("Epic", self.legendary.seal_leftover()),
            ("GOG", self.gog.seal_leftover()),
        ] {
            if let Err(e) = sealed {
                tracing::warn!(store, error = %e, "could not seal credentials on exit");
            }
        }
    }

    pub fn get(&self, runner: Runner) -> Arc<dyn StoreManager> {
        match runner {
            Runner::Legendary => self.legendary.clone(),
            Runner::Gog => self.gog.clone(),
            Runner::Sideload => self.sideload.clone(),
        }
    }

    pub fn all(&self) -> Vec<Arc<dyn StoreManager>> {
        Runner::all().into_iter().map(|r| self.get(r)).collect()
    }

    /// Achievements of a game; the last answer is kept on disk and shown
    /// when the store cannot be reached.
    pub async fn achievements(
        &self,
        runner: Runner,
        app_name: &str,
    ) -> Result<crate::types::Achievements> {
        let lang = self.ctx.settings.read().await.language.clone();
        let file = crate::paths::cache_dir().join("achievements").join(format!(
            "{}-{}.json",
            runner.id(),
            crate::compat::safe_name(app_name)
        ));
        match self.get(runner).achievements(app_name, &lang).await {
            Ok(a) => {
                if let Some(parent) = file.parent()
                    && let Ok(bytes) = serde_json::to_vec(&a)
                {
                    let _ = std::fs::create_dir_all(parent);
                    let _ = std::fs::write(&file, bytes);
                }
                Ok(a)
            }
            Err(e) => match std::fs::read(&file)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
            {
                Some(cached) => {
                    tracing::debug!(app = app_name, error = %e, "achievements from the cache");
                    Ok(cached)
                }
                None => Err(e),
            },
        }
    }

    /// Move an installed game to `<new_base>/<its folder name>` (Heroic's
    /// `moveInstall`): copy with progress under the `moving` status, record
    /// the new place, delete the old folder. The prefix stays where it is.
    /// The game is reserved for the whole move (no launch, no second move),
    /// and a game being downloaded is not moved. A sideloaded game is never
    /// moved: its "folder" is wherever its executable lies.
    pub async fn move_game(&self, runner: Runner, app_name: &str, new_base: &str) -> Result<()> {
        if runner == Runner::Sideload {
            return Err(Error::NotImplemented(
                "moving a sideloaded game: move its files and edit the entry",
            ));
        }
        let game = self.find_game(runner, app_name).await?;
        let old = game
            .install
            .as_ref()
            .map(|i| std::path::PathBuf::from(&i.install_path))
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| Error::Other(format!("{} is not installed here", game.title)))?;
        if self.is_downloading(runner, app_name).await {
            return Err(Error::Busy(app_name.to_string()));
        }
        // the launch ticket: fails while the game runs or starts, and keeps
        // it from starting until the move is through
        let ticket = self.ctx.games.prepare(runner, app_name)?;
        let base = self.ctx.settings.read().await.default_install_path.clone();
        if !is_safe_game_dir(&old, &base) {
            return Err(Error::Other(format!(
                "refusing to move {}: it is not a single game's folder",
                old.display()
            )));
        }
        let dest = moving::destination(&old, std::path::Path::new(new_base.trim()))?;
        tracing::info!(app = app_name, from = %old.display(), to = %dest.display(), "moving game");
        self.emit_status(runner, app_name, GameStatusKind::Moving, None);
        let (events, app) = (self.ctx.events.clone(), app_name.to_string());
        let (o, d) = (old.clone(), dest.clone());
        let copied = tokio::task::spawn_blocking(move || {
            let total = moving::dir_size(&o);
            moving::check_space(d.parent().unwrap_or(&d), total).map_err(std::io::Error::other)?;
            moving::copy_dir(&o, &d, total, &move |progress| {
                let mut s = GameStatus::new(runner, &app, GameStatusKind::Moving);
                s.progress = Some(progress);
                events.emit(CoreEvent::GameStatus(s));
            })
        })
        .await
        .map_err(|e| Error::Other(format!("move task failed: {e}")));
        if let Err(e) = copied.and_then(|r| r.map_err(Error::Io)) {
            let _ = tokio::fs::remove_dir_all(&dest).await;
            self.emit_status(runner, app_name, GameStatusKind::Done, None);
            return Err(e);
        }
        if let Err(e) = self.get(runner).change_install_path(app_name, &dest).await {
            // both copies exist now; the record still points at the old one
            let _ = tokio::fs::remove_dir_all(&dest).await;
            self.emit_status(runner, app_name, GameStatusKind::Done, None);
            return Err(e);
        }
        if let Err(e) = tokio::fs::remove_dir_all(&old).await {
            tracing::warn!(dir = %old.display(), error = %e, "old game folder not removed");
        }
        drop(ticket);
        self.emit_status(runner, app_name, GameStatusKind::Done, None);
        self.ctx
            .events
            .emit(CoreEvent::LibraryUpdated(Some(runner)));
        Ok(())
    }

    /// Look a game up across every store.
    pub async fn find_game(&self, runner: Runner, app_name: &str) -> Result<GameInfo> {
        self.get(runner)
            .game_info(app_name)
            .await?
            .ok_or_else(|| Error::GameNotFound(app_name.to_string()))
    }
}

/// Games whose saves are being synced, keyed `runner:app`. A sync and a
/// launch of the same game never overlap: the pre-launch download runs
/// before the game is reserved by the game runner, so it needs its own lock.
type SyncSet = Arc<std::sync::Mutex<HashSet<String>>>;

/// Holds a game in the [`SyncSet`] until dropped.
struct SyncClaim {
    set: SyncSet,
    key: String,
}

impl SyncClaim {
    fn take(set: &SyncSet, runner: Runner, app_name: &str) -> Option<Self> {
        let key = format!("{}:{app_name}", runner.id());
        let mut s = set.lock().unwrap_or_else(|e| e.into_inner());
        s.insert(key.clone()).then(|| Self {
            set: set.clone(),
            key,
        })
    }
}

impl Drop for SyncClaim {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

fn sync_failed(e: &Error) -> String {
    format!("Cloud saves: {e}")
}

/// How long a pending upload that failed while online waits before the next try.
const PENDING_RETRY: Duration = Duration::from_secs(15 * 60);

/// Upload a game's saves after it exited. Offline, or when the upload fails,
/// they wait in [`saves::pending`] for [`Stores::upload_pending`].
async fn upload_after_exit(
    store: &dyn StoreManager,
    events: &Events,
    runner: Runner,
    app: &str,
    locations: &[SaveLocation],
) {
    if !net::reachable(runner).await {
        saves::set_pending(runner, app, true);
        events.emit(CoreEvent::SavesPending {
            runner,
            app_name: app.into(),
            pending: true,
        });
        return;
    }
    events.emit(CoreEvent::GameStatus(GameStatus::new(
        runner,
        app,
        GameStatusKind::Syncing,
    )));
    match store.sync_saves(app, locations, SaveSyncMode::Upload).await {
        Ok(summary) => {
            tracing::info!(app, summary, "saves uploaded after exit");
            if saves::is_pending(runner, app) {
                saves::set_pending(runner, app, false);
                events.emit(CoreEvent::SavesPending {
                    runner,
                    app_name: app.into(),
                    pending: false,
                });
            }
        }
        Err(e) => {
            tracing::warn!(app, error = %e, "uploading saves failed; retrying later");
            saves::set_pending(runner, app, true);
            let mut s = GameStatus::new(runner, app, GameStatusKind::Error);
            s.message = Some(sync_failed(&e));
            events.emit(CoreEvent::GameStatus(s));
        }
    }
}

/// Folder-name key: lower-case letters and digits only, so "Hollow Knight",
/// "HollowKnight" and "hollow_knight" all match.
pub(crate) fn folder_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether `dir` holds an actual game: at least one executable within a few
/// levels. Folders left behind by an uninstall (saves, configs, logs) keep
/// the game's name but have none, and must not be offered for import.
pub(crate) fn has_game_files(dir: &std::path::Path) -> bool {
    const MAX_DEPTH: usize = 4;
    const MAX_ENTRIES: usize = 20_000;
    // the stores install Windows builds everywhere (Proton on Linux), so a
    // game is recognised by its .exe on every host
    let suffix = ".exe";
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(|e| e.ok()) {
            seen += 1;
            if seen > MAX_ENTRIES {
                // a huge tree: certainly not just leftovers
                return true;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_file() {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                if name.ends_with(suffix) {
                    return true;
                }
            } else if ft.is_dir() && depth < MAX_DEPTH {
                stack.push((e.path(), depth + 1));
            }
        }
    }
    false
}

/// Whether a game's install folder may be deleted as a whole on uninstall.
/// Refuses anything that is not clearly one game's own folder: a drive root,
/// the games base folder, the user's or the system's folders, our config, or
/// any folder that contains one of those.
///
/// The path comes from store metadata (GOG's folder name, legendary's
/// install path) or hand-edited files, so it is checked the way the OS
/// resolves it: `.`/`..` are refused outright, and both sides are also
/// compared after resolving links, 8.3 names and `\\?\` prefixes.
/// `D:\Games\..` must never pass for a game folder and take the drive along.
pub(crate) fn is_safe_game_dir(dir: &std::path::Path, default_install_path: &str) -> bool {
    use std::path::Component;
    if !dir.is_absolute() || !dir.is_dir() {
        return false;
    }
    if dir
        .components()
        .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return false;
    }
    let Some(real) = resolve(dir) else {
        return false;
    };
    if !real.components().any(|c| matches!(c, Component::Normal(_))) {
        return false;
    }
    let protected: Vec<std::path::PathBuf> = protected_dirs(default_install_path)
        .into_iter()
        .flat_map(|p| {
            let r = resolve(&p);
            std::iter::once(p).chain(r)
        })
        .collect();
    !is_or_contains(&real, &protected) && !is_or_contains(dir, &protected)
}

/// The path the OS really means, without Windows' `\\?\` prefix so it
/// compares with plain paths. `None` when it does not exist.
pub(crate) fn resolve(p: &std::path::Path) -> Option<std::path::PathBuf> {
    let real = std::fs::canonicalize(p).ok()?;
    let s = real.to_string_lossy().into_owned();
    Some(
        match (s.strip_prefix(r"\\?\UNC\"), s.strip_prefix(r"\\?\")) {
            (Some(rest), _) => std::path::PathBuf::from(format!(r"\\{rest}")),
            (None, Some(rest)) => std::path::PathBuf::from(rest),
            _ => real,
        },
    )
}

/// Windows device names: a file of that name is the device, on any path.
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A folder name from store metadata that is one plain name: no separators,
/// no `.`/`..`, no drive, nothing Windows refuses or silently rewrites (a
/// trailing dot or space, a device name, `<>:"|?*`, control characters).
/// Anything else could put an install (and later its uninstall) outside the
/// chosen games folder, or record a folder other than the one created.
pub(crate) fn is_plain_folder_name(name: &str) -> bool {
    use std::path::Component;
    let mut parts = std::path::Path::new(name).components();
    matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    ) && !name.is_empty()
        && name.len() <= 255
        && !name.ends_with(['.', ' '])
        && !name.starts_with(' ')
        && !name
            .chars()
            .any(|c| c.is_control() || "<>:\"|?*/\\".contains(c))
        && !is_reserved_name(name)
}

fn is_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("");
    RESERVED_NAMES.iter().any(|r| r.eq_ignore_ascii_case(stem))
}

/// A path from a manifest (a store's file list, a cloud-save listing, a
/// resume file) that stays inside the folder it is joined to: relative, no
/// `..`, no drive or root, no NUL, every component a plain name. `\` counts
/// as a separator on every platform, as the stores write Windows paths.
pub(crate) fn is_confined_rel_path(rel: &str) -> bool {
    use std::path::Component;
    if rel.is_empty() || rel.contains('\0') {
        return false;
    }
    let unix = rel.replace('\\', "/");
    let mut any = false;
    for c in std::path::Path::new(&unix).components() {
        match c {
            Component::Normal(part) => {
                any = true;
                let part = part.to_string_lossy();
                // `C:` as a component on Linux (where it is no prefix), `..`
                // hidden behind a trailing dot, device names
                if part.contains(':')
                    || part.trim_end_matches(['.', ' ']).is_empty()
                    || is_reserved_name(&part)
                {
                    return false;
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    any
}

/// Whether `dir` is one of `protected`, or a folder containing one of them.
/// Compared as written and, when it exists, as the OS resolves it (links,
/// 8.3 names); a trailing `.` or space of a name is dropped first, as
/// Windows does (`Documents.` is `Documents`).
pub(crate) fn is_or_contains(dir: &std::path::Path, protected: &[std::path::PathBuf]) -> bool {
    let norm = |p: &std::path::Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .split('\\')
            .map(|c| c.trim_end_matches(['.', ' ']).to_lowercase())
            .collect::<Vec<_>>()
            .join("\\")
            .trim_end_matches('\\')
            .to_string()
    };
    let protected: Vec<String> = protected
        .iter()
        .map(|p| norm(p))
        .filter(|p| !p.is_empty())
        .collect();
    let hit = |target: String| {
        protected.iter().any(|p| {
            // the folder is a protected one, or contains one
            *p == target || p.starts_with(&format!("{target}\\"))
        })
    };
    hit(norm(dir)) || resolve(dir).is_some_and(|real| hit(norm(&real)))
}

/// Folders that must never be deleted or overwritten as a whole: the user's
/// and the system's folders, our config, the games base folder(s).
pub(crate) fn protected_dirs(default_install_path: &str) -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut protected: Vec<PathBuf> = [
        dirs::home_dir(),
        dirs::document_dir(),
        dirs::desktop_dir(),
        dirs::download_dir(),
        dirs::config_dir(),
        dirs::data_local_dir(),
        Some(crate::paths::config_dir()),
        Some(crate::platform::current().default_install_path()),
    ]
    .into_iter()
    .flatten()
    .collect();
    for var in [
        "WINDIR",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
        "PUBLIC",
    ] {
        if let Some(v) = std::env::var_os(var) {
            protected.push(PathBuf::from(v));
        }
    }
    if !default_install_path.trim().is_empty() {
        protected.push(PathBuf::from(default_install_path.trim()));
    }
    protected
}

/// Delete a game's install folder after an uninstall (leftover saves,
/// configs, logs…), when [`is_safe_game_dir`] allows it. Never fails the
/// uninstall: a locked file just leaves the folder behind.
pub(crate) async fn remove_game_dir(dir: &std::path::Path, settings: &RwLock<AppSettings>) {
    let base = settings.read().await.default_install_path.clone();
    if !dir.exists() {
        return;
    }
    if !is_safe_game_dir(dir, &base) {
        tracing::warn!(dir = %dir.display(), "not removing install folder: not clearly a single game's folder");
        return;
    }
    match tokio::fs::remove_dir_all(dir).await {
        Ok(()) => tracing::info!(dir = %dir.display(), "install folder removed"),
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "could not remove the install folder")
        }
    }
}

/// "12.3 GB" style formatting used for install sizes.
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Per-game log file (`logs/games/<runner>-<app>-<kind>.log`), truncated on
/// open. Failures to create it are ignored: logging must never break an
/// install or a launch.
pub(crate) struct GameLog {
    file: Option<std::fs::File>,
}

impl GameLog {
    pub(crate) fn open(runner: Runner, app_name: &str, kind: &str) -> Self {
        // app names come from store metadata: never a path
        let name = crate::compat::safe_name(app_name);
        let path = crate::paths::game_logs_dir().join(format!("{}-{name}-{kind}.log", runner.id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .ok();
        Self { file }
    }

    /// The same file, kept: for a later line (the command as finally run).
    pub(crate) fn append(runner: Runner, app_name: &str, kind: &str) -> Self {
        let name = crate::compat::safe_name(app_name);
        let path = crate::paths::game_logs_dir().join(format!("{}-{name}-{kind}.log", runner.id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        Self { file }
    }

    pub(crate) fn write(&self, line: &str) {
        use std::io::Write;
        if let Some(mut f) = self.file.as_ref() {
            let _ = writeln!(f, "{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_folder_names() {
        for ok in ["Limbo", "Baldur's Gate 3", "Game.v2", "A B"] {
            assert!(is_plain_folder_name(ok), "{ok}");
        }
        for bad in [
            "", ".", "..", "a/b", "a\\b", "C:", "Game.", "Game ", " Game", "CON", "nul.txt", "a<b",
            "a\tb", "a?b",
        ] {
            assert!(!is_plain_folder_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn confined_relative_paths() {
        for ok in [
            "Game.exe",
            "Bin/Win64/Game.exe",
            "Bin\\Win64\\Game.exe",
            "./Engine/x",
            "a.b/c.d",
            "Saves/slot.",
        ] {
            assert!(is_confined_rel_path(ok), "{ok}");
        }
        for bad in [
            "",
            "..",
            "../x",
            "a/../../x",
            "a\\..\\x",
            "/etc/passwd",
            "\\\\server\\share\\x",
            "C:/Windows/x",
            "C:\\x",
            "a/C:/x",
            "a/.../x",
            "a\0b",
            "a/CON/b",
            "a/nul.txt",
        ] {
            assert!(!is_confined_rel_path(bad), "{bad:?}");
        }
    }

    #[test]
    fn leftover_folders_are_not_games() {
        let root = std::env::temp_dir().join(format!("hyper-gamefiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let leftover = root.join("Limbo");
        std::fs::create_dir_all(leftover.join("Saved").join("Config")).unwrap();
        std::fs::write(
            leftover.join("Saved").join("Config").join("settings.ini"),
            "x",
        )
        .unwrap();
        assert!(!has_game_files(&leftover));

        let game = root.join("Game");
        std::fs::create_dir_all(game.join("Binaries").join("Win64")).unwrap();
        std::fs::write(game.join("Binaries").join("Win64").join("Game.EXE"), "x").unwrap();
        assert!(has_game_files(&game));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_single_game_folders_are_deletable() {
        let root = std::env::temp_dir().join(format!("hyper-safedir-{}", std::process::id()));
        let game = root.join("Games").join("Limbo");
        std::fs::create_dir_all(&game).unwrap();
        let base = root.join("Games");
        let base_s = base.to_string_lossy().into_owned();

        assert!(
            is_safe_game_dir(&game, &base_s),
            "a game inside the base folder"
        );
        assert!(!is_safe_game_dir(&base, &base_s), "the base folder itself");
        assert!(
            !is_safe_game_dir(&root, &base_s),
            "a folder containing the base folder"
        );
        assert!(
            !is_safe_game_dir(std::path::Path::new(r"C:\"), ""),
            "a drive root"
        );
        if let Some(home) = dirs::home_dir() {
            assert!(!is_safe_game_dir(&home, ""), "the home folder");
        }
        if let Some(docs) = dirs::document_dir() {
            assert!(!is_safe_game_dir(&docs, ""), "Documents");
        }
        assert!(
            !is_safe_game_dir(&root.join("missing"), ""),
            "a folder that does not exist"
        );
        // store metadata pointing above the game folder
        assert!(
            !is_safe_game_dir(&game.join(".."), &base_s),
            "the base folder through .."
        );
        assert!(
            !is_safe_game_dir(&base.join(".."), &base_s),
            "the base's parent through .."
        );
        assert!(
            !is_safe_game_dir(&base.join("."), &base_s),
            "the base folder through ."
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_names_from_stores_are_single_names() {
        assert!(is_plain_folder_name("Hollow Knight"));
        assert!(is_plain_folder_name("Game v1.2"));
        for bad in [
            "",
            ".",
            "..",
            "...",
            "a/b",
            r"a\b",
            r"C:\Games",
            "C:",
            "/etc",
            r"..\x",
        ] {
            assert!(!is_plain_folder_name(bad), "{bad:?}");
        }
    }
}
