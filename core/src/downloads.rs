//! Sequential install/update queue, like Heroic's DownloadManager: one
//! download at a time, the rest wait as `queued`.
//!
//! The queue is mirrored to `downloads-queue.json` in the config folder.
//! When the app is closed while something downloads, the download is
//! cancelled gently ([`DownloadManager::shutdown`]: the engines keep their
//! resume files) and the file is left as it was, so the next start brings
//! every item back as `paused`. Nothing restarts by itself (a Deck on a
//! metered connection must not pull gigabytes unasked): the user presses
//! "Resume" ([`DownloadManager::resume`]) and the engine carries on where it
//! stopped.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{Mutex, Notify};

use crate::error::{Error, Result};
use crate::events::{CoreEvent, Events};
use crate::paths;
use crate::stores::Stores;
use crate::types::{
    DownloadItem, DownloadKind, GameStatus, GameStatusKind, InstallOptions, InstallPlatform, Runner,
};

/// How long [`DownloadManager::shutdown`] waits for the running download to
/// stop before the app exits anyway.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

pub struct DownloadManager {
    stores: Arc<Stores>,
    events: Events,
    queue: Mutex<VecDeque<DownloadItem>>,
    current: Mutex<Option<DownloadItem>>,
    wake: Notify,
    /// Set by [`shutdown`](Self::shutdown): the queue file stays as it is so
    /// the running item comes back paused next time.
    closing: AtomicBool,
    /// `downloads-queue.json`
    file: PathBuf,
}

impl DownloadManager {
    pub fn new(stores: Arc<Stores>, events: Events) -> Arc<Self> {
        Self::with_file(
            stores,
            events,
            paths::config_dir().join("downloads-queue.json"),
        )
    }

    pub fn with_file(stores: Arc<Stores>, events: Events, file: PathBuf) -> Arc<Self> {
        let me = Arc::new(Self {
            stores,
            events,
            queue: Mutex::new(VecDeque::new()),
            current: Mutex::new(None),
            wake: Notify::new(),
            closing: AtomicBool::new(false),
            file,
        });
        me.stores.attach_downloads(Arc::downgrade(&me));
        me
    }

    // Lock order, wherever both are held: `queue` first, then `current`.

    /// Snapshot: the running item first, then the queue (paused ones included).
    pub async fn items(&self) -> Vec<DownloadItem> {
        let q = self.queue.lock().await;
        let mut out = Vec::new();
        if let Some(c) = self.current.lock().await.clone() {
            out.push(c);
        }
        out.extend(q.iter().cloned());
        out
    }

    /// Something is downloading or waits to: closing the app would interrupt it.
    pub async fn is_busy(&self) -> bool {
        let q = self.queue.lock().await;
        self.current.lock().await.is_some() || q.iter().any(|i| !i.paused)
    }

    /// The game is downloading or in the queue (paused or not).
    pub async fn has(&self, runner: Runner, app_name: &str) -> bool {
        let same = |i: &DownloadItem| i.runner == runner && i.app_name == app_name;
        let q = self.queue.lock().await;
        self.current.lock().await.as_ref().is_some_and(same) || q.iter().any(same)
    }

    /// Titles of the active downloads (running first), for the close warning.
    pub async fn busy_titles(&self) -> Vec<String> {
        self.items()
            .await
            .into_iter()
            .filter(|i| !i.paused)
            .map(|i| i.title)
            .collect()
    }

    async fn publish(&self) {
        let items = self.items().await;
        self.persist(&items);
        self.events.emit(CoreEvent::DownloadQueue(items));
    }

    /// Mirror the queue to disk (nothing while closing: see `shutdown`).
    fn persist(&self, items: &[DownloadItem]) {
        if self.closing.load(Ordering::Relaxed) {
            return;
        }
        if items.is_empty() {
            let _ = std::fs::remove_file(&self.file);
            return;
        }
        let write = || -> std::io::Result<()> {
            if let Some(parent) = self.file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let tmp = self.file.with_extension("json.tmp");
            std::fs::write(
                &tmp,
                serde_json::to_vec_pretty(items).map_err(std::io::Error::other)?,
            )?;
            std::fs::rename(&tmp, &self.file)?;
            Ok(())
        };
        if let Err(e) = write() {
            tracing::warn!(error = %e, "could not save the download queue");
        }
    }

    /// Bring back the queue of the previous session, every item paused.
    /// Call once at startup, before [`run`](Self::run).
    pub async fn restore(&self) {
        let Ok(raw) = std::fs::read(&self.file) else {
            return;
        };
        let items: Vec<DownloadItem> = match serde_json::from_slice(&raw) {
            Ok(items) => items,
            Err(e) => {
                tracing::warn!(error = %e, "ignoring an unreadable download queue file");
                let _ = std::fs::remove_file(&self.file);
                return;
            }
        };
        if items.is_empty() {
            return;
        }
        let mut q = self.queue.lock().await;
        for mut item in items {
            if q.iter()
                .any(|i| i.runner == item.runner && i.app_name == item.app_name)
            {
                continue;
            }
            item.running = false;
            item.paused = true;
            tracing::info!(app = %item.app_name, kind = ?item.kind, "download of the last session waits paused");
            self.emit_status(item.runner, &item.app_name, GameStatusKind::Paused);
            q.push_back(item);
        }
        drop(q);
        self.publish().await;
    }

    fn emit_status(&self, runner: Runner, app_name: &str, status: GameStatusKind) {
        self.events.emit(CoreEvent::GameStatus(GameStatus::new(
            runner, app_name, status,
        )));
    }

    pub async fn enqueue(&self, mut item: DownloadItem) -> Result<()> {
        item.running = false;
        item.paused = false;
        let same = |i: &DownloadItem| i.runner == item.runner && i.app_name == item.app_name;
        {
            // both locks: the worker moves an item from the queue to
            // `current` under both too, so it is never missed in between
            let mut q = self.queue.lock().await;
            if self.current.lock().await.as_ref().is_some_and(same) {
                return Err(Error::Busy(item.app_name));
            }
            // a paused leftover of the same game gives way to the new request
            // (same engine, same resume file: it carries on anyway)
            if q.iter().any(|i| same(i) && !i.paused) {
                return Err(Error::Busy(item.app_name));
            }
            q.retain(|i| !same(i));
            self.emit_status(item.runner, &item.app_name, GameStatusKind::Queued);
            q.push_back(item);
        }
        self.publish().await;
        self.wake.notify_one();
        Ok(())
    }

    /// Continue a download left paused from the last session.
    pub async fn resume(&self, runner: Runner, app_name: &str) -> Result<()> {
        {
            let mut q = self.queue.lock().await;
            let Some(item) = q
                .iter_mut()
                .find(|i| i.runner == runner && i.app_name == app_name)
            else {
                return Err(Error::Other(format!(
                    "{app_name} is not in the download queue"
                )));
            };
            item.paused = false;
        }
        self.emit_status(runner, app_name, GameStatusKind::Queued);
        self.publish().await;
        self.wake.notify_one();
        Ok(())
    }

    /// Remove from the queue, or abort if it is the running one.
    pub async fn cancel(&self, runner: Runner, app_name: &str) -> Result<()> {
        let (mut removed, is_current) = {
            let mut q = self.queue.lock().await;
            let before = q.len();
            q.retain(|i| !(i.runner == runner && i.app_name == app_name));
            let is_current = self
                .current
                .lock()
                .await
                .as_ref()
                .is_some_and(|c| c.runner == runner && c.app_name == app_name);
            (q.len() != before, is_current)
        };
        if is_current {
            self.stores.get(runner).cancel(app_name).await?;
            removed = true;
        } else if removed {
            self.emit_status(runner, app_name, GameStatusKind::Done);
        }
        self.publish().await;
        if removed {
            Ok(())
        } else {
            Err(Error::Other(format!("{app_name} is not downloading")))
        }
    }

    /// The app is closing: stop the running download (its resume file stays)
    /// and keep the queue file as it is, so everything comes back paused.
    /// Waits up to [`SHUTDOWN_GRACE`] for the engine to stop.
    pub async fn shutdown(&self) {
        self.closing.store(true, Ordering::Relaxed);
        let running = self.current.lock().await.clone();
        let Some(item) = running else { return };
        tracing::info!(app = %item.app_name, "stopping the download for exit");
        if let Err(e) = self.stores.get(item.runner).cancel(&item.app_name).await {
            tracing::warn!(app = %item.app_name, error = %e, "cancel for exit failed");
            return;
        }
        let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE;
        while self.current.lock().await.is_some() {
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(app = %item.app_name, "download did not stop in time; exiting anyway");
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Worker loop; spawn once at startup.
    pub async fn run(self: Arc<Self>) {
        loop {
            let next = {
                // taken out of the queue and made current under both locks
                // (see `enqueue`)
                let mut q = self.queue.lock().await;
                let next = q
                    .iter()
                    .position(|i| !i.paused)
                    .and_then(|pos| q.remove(pos));
                if let Some(mut item) = next {
                    item.running = true;
                    *self.current.lock().await = Some(item.clone());
                    Some(item)
                } else {
                    None
                }
            };
            let Some(item) = next else {
                self.wake.notified().await;
                continue;
            };
            self.publish().await;

            let store = self.stores.get(item.runner);
            let result = match item.kind {
                DownloadKind::Install => {
                    store
                        .install(
                            &item.app_name,
                            InstallOptions {
                                path: item.path.clone().unwrap_or_default(),
                                platform: item.platform,
                                branch: item.branch.clone(),
                                build_id: item.build_id.clone(),
                            },
                        )
                        .await
                }
                DownloadKind::Update => store.update(&item.app_name).await,
                DownloadKind::Repair => store.repair(&item.app_name).await,
            };
            if let Err(e) = &result {
                tracing::warn!(app = %item.app_name, error = %e, "download finished with error");
            }

            *self.current.lock().await = None;
            self.publish().await;
        }
    }
}

impl DownloadItem {
    pub fn install(
        runner: Runner,
        app_name: impl Into<String>,
        title: impl Into<String>,
        path: impl Into<String>,
        platform: InstallPlatform,
    ) -> Self {
        Self {
            runner,
            app_name: app_name.into(),
            title: title.into(),
            kind: DownloadKind::Install,
            path: Some(path.into()),
            platform,
            running: false,
            paused: false,
            branch: None,
            build_id: None,
        }
    }

    /// An install of a GOG branch / specific build.
    pub fn with_build(mut self, branch: Option<String>, build_id: Option<String>) -> Self {
        self.branch = branch.filter(|b| !b.trim().is_empty());
        self.build_id = build_id.filter(|b| !b.trim().is_empty());
        self
    }

    pub fn update(
        runner: Runner,
        app_name: impl Into<String>,
        title: impl Into<String>,
        platform: InstallPlatform,
    ) -> Self {
        Self {
            runner,
            app_name: app_name.into(),
            title: title.into(),
            kind: DownloadKind::Update,
            path: None,
            platform,
            running: false,
            paused: false,
            branch: None,
            build_id: None,
        }
    }

    pub fn repair(
        runner: Runner,
        app_name: impl Into<String>,
        title: impl Into<String>,
        platform: InstallPlatform,
    ) -> Self {
        Self {
            kind: DownloadKind::Repair,
            ..Self::update(runner, app_name, title, platform)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppSettings;
    use crate::events::NullSink;
    use crate::games::GameRunner;
    use crate::secrets::Vault;
    use crate::stores::StoreContext;
    use tokio::sync::RwLock;

    fn manager(dir: &std::path::Path) -> Arc<DownloadManager> {
        let events: Events = Arc::new(NullSink);
        let stores = Arc::new(Stores::new(StoreContext {
            settings: Arc::new(RwLock::new(AppSettings::default())),
            vault: Arc::new(Vault::new(dir.join("vault"))),
            events: events.clone(),
            games: GameRunner::new(events.clone()),
        }));
        DownloadManager::with_file(stores, events, dir.join("downloads-queue.json"))
    }

    #[tokio::test]
    async fn queue_survives_a_restart_as_paused() {
        let tmp = std::env::temp_dir().join(format!("hyper-downloads-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // session 1: two items queued (no worker runs, so they stay queued)
        let m = manager(&tmp);
        m.enqueue(DownloadItem::install(
            Runner::Gog,
            "a",
            "A",
            "C:/Games",
            InstallPlatform::default(),
        ))
        .await
        .unwrap();
        m.enqueue(DownloadItem::update(
            Runner::Legendary,
            "b",
            "B",
            InstallPlatform::default(),
        ))
        .await
        .unwrap();
        assert!(m.is_busy().await);
        assert_eq!(m.busy_titles().await, ["A", "B"]);
        assert!(tmp.join("downloads-queue.json").exists());
        // the same game again while it waits: refused
        assert!(matches!(
            m.enqueue(DownloadItem::install(
                Runner::Gog,
                "a",
                "A",
                "C:/Games",
                InstallPlatform::default()
            ))
            .await,
            Err(Error::Busy(_))
        ));
        drop(m);

        // session 2: both come back paused, nothing counts as busy
        let m = manager(&tmp);
        m.restore().await;
        let items = m.items().await;
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.paused && !i.running));
        assert!(!m.is_busy().await);
        assert!(m.busy_titles().await.is_empty());

        // resume one: it is queued again (and would start with a worker)
        m.resume(Runner::Legendary, "b").await.unwrap();
        assert!(m.is_busy().await);
        assert_eq!(m.busy_titles().await, ["B"]);
        assert!(m.resume(Runner::Gog, "nope").await.is_err());

        // a new request for the paused game replaces it, unpaused
        m.enqueue(DownloadItem::install(
            Runner::Gog,
            "a",
            "A",
            "D:/Games",
            InstallPlatform::default(),
        ))
        .await
        .unwrap();
        let items = m.items().await;
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| !i.paused));
        assert_eq!(
            items
                .iter()
                .find(|i| i.app_name == "a")
                .unwrap()
                .path
                .as_deref(),
            Some("D:/Games")
        );

        // removing everything removes the file
        m.cancel(Runner::Gog, "a").await.unwrap();
        m.cancel(Runner::Legendary, "b").await.unwrap();
        assert!(!tmp.join("downloads-queue.json").exists());
        assert!(m.cancel(Runner::Legendary, "b").await.is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn unreadable_queue_file_is_dropped() {
        let tmp = std::env::temp_dir().join(format!("hyper-downloads-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("downloads-queue.json"), b"{ not json").unwrap();
        let m = manager(&tmp);
        m.restore().await;
        assert!(m.items().await.is_empty());
        assert!(!tmp.join("downloads-queue.json").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
