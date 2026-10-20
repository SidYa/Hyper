//! Epic Games Store without `legendary`: the Epic API spoken directly
//! (`api`), legendary's files on disk written the same way (`library`,
//! `installed`), its manifests parsed (`manifest`) and games started with the
//! arguments the Epic Games Launcher passes (`launch`), cloud saves
//! (`saves`) and installs (`download`). A port of legendary 0.21, see
//! `docs/legendary-rust.md`.
//!
//! Everything here reads and writes `legendaryConfig/legendary` exactly like
//! legendary 0.21 does, so a library made by legendary / Heroic keeps working:
//! `assets.json`, `metadata/<app>.json`, `installed.json`,
//! `manifests/<app>_<platform>_<version>.manifest`. The one exception is
//! `user.json`: it lives in the vault and only exists on disk while a
//! legendary call runs.

pub mod achievements;
pub mod api;
pub mod chunk;
pub mod download;
pub mod installed;
pub mod launch;
pub mod library;
pub mod manifest;
pub mod saves;
mod session;

use std::path::Path;

use serde_json::Value;

use crate::error::Result;

pub use api::{Epic, RATE_LIMIT_MESSAGE};

/// Write `value` like Python's `json.dump(indent=2, sort_keys=True)`: keys
/// sorted at every level, two-space indent. legendary reads the same files,
/// and a diff between the two implementations stays readable.
pub(crate) fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&sorted(value.clone()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// JSON files legendary wrote; `None` when missing or unreadable.
pub(crate) fn read_json(path: &Path) -> Option<Value> {
    let raw = std::fs::read(path).ok()?;
    match serde_json::from_slice(&raw) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "unreadable JSON file");
            None
        }
    }
}

/// Objects re-built with their keys in order (serde_json may keep insertion
/// order, depending on features enabled elsewhere in the tree).
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

/// `legendary`'s `clean_filename`: the characters Windows refuses in names.
pub(crate) fn clean_filename(name: &str) -> String {
    name.chars()
        .filter(|c| !"<>:\"/\\|?*".contains(*c))
        .collect()
}

/// `metadata/<app>.json`.
pub(crate) fn metadata_path(config_dir: &Path, app_name: &str) -> std::path::PathBuf {
    config_dir
        .join("metadata")
        .join(format!("{}.json", clean_filename(app_name)))
}

/// The game's manifest as legendary files it, with the platform in the name;
/// older legendary versions left the platform out.
pub(crate) fn manifest_path(
    config_dir: &Path,
    app_name: &str,
    platform: &str,
    version: &str,
) -> std::path::PathBuf {
    config_dir.join("manifests").join(format!(
        "{}.manifest",
        clean_filename(&format!("{app_name}_{platform}_{version}"))
    ))
}

pub(crate) fn manifest_path_legacy(
    config_dir: &Path,
    app_name: &str,
    version: &str,
) -> std::path::PathBuf {
    config_dir.join("manifests").join(format!(
        "{}.manifest",
        clean_filename(&format!("{app_name}_{version}"))
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_written_with_sorted_keys() {
        let dir = std::env::temp_dir().join(format!("hyper-epic-json-{}", std::process::id()));
        let path = dir.join("x.json");
        write_json(
            &path,
            &serde_json::json!({ "b": 1, "a": { "z": [ { "y": 1, "x": 2 } ], "c": true } }),
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "{\n  \"a\": {\n    \"c\": true,\n    \"z\": [\n      {\n        \"x\": 2,\n        \"y\": 1\n      }\n    ]\n  },\n  \"b\": 1\n}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn filenames_follow_legendary() {
        assert_eq!(
            clean_filename("Fortnite_Windows_++Fortnite+Release-1.0"),
            "Fortnite_Windows_++Fortnite+Release-1.0"
        );
        assert_eq!(clean_filename("a:b/c\\d?e*f\"g<h>i|j"), "abcdefghij");
        assert!(
            manifest_path(Path::new("/c"), "App", "Windows", "1.0")
                .ends_with("manifests/App_Windows_1.0.manifest")
        );
    }
}
