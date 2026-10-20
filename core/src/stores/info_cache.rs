//! Install sizes on disk (`<cache>/install-info-<store>.json`). Getting them
//! takes the helper several seconds (login check + the game's manifest from
//! the CDN), so the game page shows the last known value at once. An entry
//! is used while the store's build `version` is unchanged (when the store
//! tells it without a request) and not older than its maximum age.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::types::InstallInfo;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    /// Build the sizes belong to; empty when unknown.
    version: String,
    /// Unix seconds.
    saved_at: u64,
    info: InstallInfo,
}

pub(crate) struct InfoCache {
    path: PathBuf,
    entries: Mutex<HashMap<String, Entry>>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl InfoCache {
    pub(crate) fn new(store: &str) -> Self {
        let path = paths::cache_dir().join(format!("install-info-{store}.json"));
        Self::at(path)
    }

    fn at(path: PathBuf) -> Self {
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            entries: Mutex::new(entries),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The cached sizes, if they are for `version` and younger than `max_age`.
    pub(crate) fn get(&self, key: &str, version: &str, max_age: Duration) -> Option<InstallInfo> {
        let entries = self.lock();
        let e = entries.get(key)?;
        let fresh = now().saturating_sub(e.saved_at) <= max_age.as_secs();
        (fresh && e.version == version).then(|| e.info.clone())
    }

    pub(crate) fn put(&self, key: &str, version: &str, info: &InstallInfo) {
        let mut entries = self.lock();
        entries.insert(
            key.to_string(),
            Entry {
                version: version.to_string(),
                saved_at: now(),
                info: info.clone(),
            },
        );
        let written = serde_json::to_vec(&*entries)
            .map_err(std::io::Error::other)
            .and_then(|raw| {
                if let Some(dir) = self.path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&self.path, raw)
            });
        if let Err(e) = written {
            tracing::debug!(error = %e, "could not save the install size cache");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_sizes_per_build_and_age() {
        let path =
            std::env::temp_dir().join(format!("hyper-info-cache-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let info = InstallInfo {
            app_name: "Game".into(),
            download_size: 42,
            ..Default::default()
        };
        let cache = InfoCache::at(path.clone());
        cache.put("Game:Windows", "1.0", &info);
        let day = Duration::from_secs(86_400);
        assert_eq!(
            cache
                .get("Game:Windows", "1.0", day)
                .map(|i| i.download_size),
            Some(42)
        );
        assert!(
            cache.get("Game:Windows", "1.1", day).is_none(),
            "a new build"
        );
        // survives a restart
        let again = InfoCache::at(path.clone());
        assert!(again.get("Game:Windows", "1.0", day).is_some());
        let _ = std::fs::remove_file(&path);
    }
}
