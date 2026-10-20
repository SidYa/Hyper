//! Play time per game (`playtime.json` in the config dir): total seconds and
//! the last launch. Written by the game runner when a game starts and exits.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::types::PlayStats;

pub struct PlayTime {
    path: PathBuf,
    /// Loaded on first use, so constructing it never touches the disk.
    data: Mutex<Option<HashMap<String, PlayStats>>>,
}

impl PlayTime {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            data: Mutex::new(None),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut HashMap<String, PlayStats>) -> R) -> R {
        let mut guard = self.data.lock().unwrap_or_else(|e| e.into_inner());
        let data = guard.get_or_insert_with(|| {
            std::fs::read_to_string(&self.path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        });
        f(data)
    }

    fn save(&self, data: &HashMap<String, PlayStats>) {
        let write = || -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = self.path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(data)?)?;
            std::fs::rename(&tmp, &self.path)
        };
        if let Err(e) = write() {
            tracing::warn!(path = %self.path.display(), error = %e, "could not save play time");
        }
    }

    pub fn get(&self, key: &str) -> PlayStats {
        self.with(|d| d.get(key).cloned().unwrap_or_default())
    }

    /// The game was just started.
    pub fn started(&self, key: &str) {
        self.with(|d| {
            d.entry(key.to_string()).or_default().last_played =
                Some(chrono::Utc::now().to_rfc3339());
            self.save(d);
        });
    }

    /// A play session of `secs` ended.
    pub fn add(&self, key: &str, secs: u64) {
        self.with(|d| {
            d.entry(key.to_string()).or_default().total_secs += secs;
            self.save(d);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reloads_sessions() {
        let dir = std::env::temp_dir().join(format!("hyper-playtime-{}", std::process::id()));
        let path = dir.join("playtime.json");
        let pt = PlayTime::new(path.clone());
        assert_eq!(pt.get("legendary:Game"), PlayStats::default());
        pt.started("legendary:Game");
        pt.add("legendary:Game", 90);
        pt.add("legendary:Game", 30);

        let again = PlayTime::new(path);
        let s = again.get("legendary:Game");
        assert_eq!(s.total_secs, 120);
        assert!(s.last_played.is_some());
        let _ = std::fs::remove_dir_all(dir);
    }
}
