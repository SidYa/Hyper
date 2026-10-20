//! Events the core pushes to its host. The Tauri app forwards them to the
//! webview; tests can collect them.

use std::sync::Arc;

use crate::types::{DownloadItem, GameStatus, Runner, StoreStatus};

#[derive(Debug, Clone)]
pub enum CoreEvent {
    /// A game's status or download progress changed.
    GameStatus(GameStatus),
    /// The library of one (or every) store changed; the UI should re-fetch.
    LibraryUpdated(Option<Runner>),
    /// Login state of a store changed.
    StoreLogin(StoreStatus),
    /// The install/update queue changed.
    DownloadQueue(Vec<DownloadItem>),
    /// A game's saves wait for an upload (`pending`: played offline, or the
    /// upload failed) or the waiting upload went through.
    SavesPending {
        runner: Runner,
        app_name: String,
        pending: bool,
    },
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: CoreEvent);
}

/// Sink that drops everything; used in tests and before the window exists.
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: CoreEvent) {}
}

pub type Events = Arc<dyn EventSink>;
