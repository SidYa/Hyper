//! Installed GOG games (`installed.json` next to the gogdl config) and the
//! bits read from a game folder: `goggame-<id>.info` play tasks → how to
//! start the game, the same way `gogdl launch` does on Windows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::paths;
use crate::stores::human_size;
use crate::types::{InstallPlatform, InstalledInfo, LaunchSpec};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GogInstalled {
    pub app_name: String,
    pub install_path: String,
    /// gogdl platform: "windows" (Linux / macOS builds come with phase 6)
    pub platform: String,
    pub version: String,
    pub build_id: String,
    pub language: String,
    /// Owned DLCs installed with the game.
    pub dlcs: Vec<String>,
    pub size_bytes: u64,
    /// Branch the build came from (`None`: public).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The build was chosen by hand: no update checks, updates keep it.
    #[serde(default)]
    pub pinned: bool,
}

impl GogInstalled {
    pub fn info(&self) -> InstalledInfo {
        InstalledInfo {
            executable: String::new(),
            install_path: self.install_path.clone(),
            install_size: human_size(self.size_bytes),
            install_size_bytes: self.size_bytes,
            is_dlc: false,
            version: self.version.clone(),
            platform: InstallPlatform::Windows,
            language: Some(self.language.clone()).filter(|l| !l.is_empty()),
            build_id: Some(self.build_id.clone()).filter(|b| !b.is_empty()),
            branch: self.branch.clone(),
            installed_dlcs: Some(self.dlcs.clone()).filter(|d| !d.is_empty()),
            pinned_version: self.pinned,
        }
    }
}

fn store_file() -> PathBuf {
    paths::gogdl_config_dir().join("installed.json")
}

pub fn load() -> HashMap<String, GogInstalled> {
    std::fs::read_to_string(store_file())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<GogInstalled>>(&s).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|g| (g.app_name.clone(), g))
        .collect()
}

pub fn save(all: &HashMap<String, GogInstalled>) -> Result<()> {
    let mut list: Vec<&GogInstalled> = all.values().collect();
    list.sort_by(|a, b| a.app_name.cmp(&b.app_name));
    let path = store_file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&list)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Total size of the files under `dir`.
pub fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in rd.flatten() {
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(entry.path()),
                Ok(t) if t.is_file() => total += entry.metadata().map(|m| m.len()).unwrap_or(0),
                _ => {}
            }
        }
    }
    total
}

/// Product ids of the `goggame-<id>.info` files directly in `dir` (the game
/// and its installed DLCs).
pub fn info_ids(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.strip_prefix("goggame-")?.strip_suffix(".info")?;
            (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit())).then(|| id.to_string())
        })
        .collect()
}

/// POSIX-like word splitting (quotes, backslash escapes), as gogdl's
/// `shlex.split` does for string task arguments.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    in_word = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    out
}

/// Launch command of a Windows GOG game from `goggame-<id>.info`: the
/// primary play task's executable, working directory and arguments.
pub fn launch_spec(install_path: &Path, app_name: &str) -> Result<LaunchSpec> {
    let info_path = install_path.join(format!("goggame-{app_name}.info"));
    let raw = std::fs::read_to_string(&info_path).map_err(|_| {
        Error::Other(format!(
            "{} is missing; verify the game files",
            info_path.display()
        ))
    })?;
    let v: Value = serde_json::from_str(&raw)?;
    spec_from_info(&v, install_path)
}

pub fn spec_from_info(v: &Value, install_path: &Path) -> Result<LaunchSpec> {
    let tasks = v
        .get("playTasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let is_file = |t: &&Value| {
        t.get("type")
            .and_then(Value::as_str)
            .is_none_or(|k| k == "FileTask")
    };
    let task = tasks
        .iter()
        .filter(is_file)
        .find(|t| t.get("isPrimary").and_then(Value::as_bool) == Some(true))
        .or_else(|| tasks.iter().find(is_file))
        .ok_or_else(|| Error::Other("the game has no play task to start".into()))?;
    let exe = task
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Other("the play task has no executable".into()))?;
    let working = task.get("workingDir").and_then(Value::as_str).unwrap_or("");
    // the info file is read from the game folder: what it starts must be in there
    for p in [exe, working] {
        if !stays_inside(&native_path(p)) {
            return Err(Error::Other(format!(
                "the play task points outside the game folder: {p}"
            )));
        }
    }
    let args = match task.get("arguments") {
        Some(Value::String(s)) => split_args(&s.replace('\\', "/")),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    Ok(LaunchSpec {
        executable: install_path
            .join(native_path(exe))
            .to_string_lossy()
            .into_owned(),
        args,
        working_dir: install_path
            .join(native_path(working))
            .to_string_lossy()
            .into_owned(),
        env: Vec::new(),
    })
}

/// A relative path that cannot leave the folder it is joined to: no root,
/// drive or `..`.
pub(crate) fn stays_inside(p: &str) -> bool {
    use std::path::Component;
    Path::new(p)
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// The info file writes paths Windows-style (`DOSBOX\DOSBox.exe`); on Linux
/// `\` is no separator.
fn native_path(p: &str) -> String {
    if std::path::MAIN_SEPARATOR == '\\' {
        p.to_string()
    } else {
        p.replace('\\', "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_tasks_stay_in_the_game_folder() {
        let info = |path: &str, dir: &str| serde_json::json!({ "playTasks": [{ "isPrimary": true, "path": path, "workingDir": dir }] });
        let root = Path::new("/games/x");
        assert!(spec_from_info(&info("bin/Game.exe", ""), root).is_ok());
        assert!(spec_from_info(&info("Game.exe", "bin"), root).is_ok());
        for (path, dir) in [
            ("../evil.exe", ""),
            ("Game.exe", ".."),
            (r"..\..\evil.exe", ""),
            ("/usr/bin/evil", ""),
        ] {
            assert!(
                spec_from_info(&info(path, dir), root).is_err(),
                "{path} {dir}"
            );
        }
        #[cfg(windows)]
        assert!(spec_from_info(&info(r"C:\Windows\evil.exe", ""), root).is_err());
    }

    #[test]
    fn splits_like_shlex() {
        assert_eq!(
            split_args(r#"-conf "a b.conf" -x 'c d' plain"#),
            ["-conf", "a b.conf", "-x", "c d", "plain"]
        );
        assert_eq!(split_args("  "), Vec::<String>::new());
        assert_eq!(split_args(r#""""#), [""]);
    }

    #[test]
    fn picks_the_primary_file_task() {
        let info = serde_json::json!({
            "playTasks": [
                { "category": "document", "type": "FileTask", "path": "manual.pdf" },
                { "category": "game", "isPrimary": true, "type": "FileTask",
                  "path": "DOSBOX\\DOSBox.exe", "workingDir": "DOSBOX",
                  "arguments": "-conf \"..\\dosbox_game.conf\" -noconsole" },
                { "type": "URLTask", "link": "https://gog.com" }
            ]
        });
        let s = spec_from_info(&info, Path::new("C:\\Games\\Doom")).unwrap();
        // the info file's backslashes become the host's separator
        let sep = std::path::MAIN_SEPARATOR;
        assert!(
            s.executable.ends_with(&format!("DOSBOX{sep}DOSBox.exe")),
            "{}",
            s.executable
        );
        assert!(s.working_dir.ends_with("DOSBOX"));
        assert_eq!(s.args, ["-conf", "../dosbox_game.conf", "-noconsole"]);
    }

    #[test]
    fn falls_back_to_the_first_file_task() {
        let info =
            serde_json::json!({ "playTasks": [ { "type": "FileTask", "path": "game.exe" } ] });
        let s = spec_from_info(&info, Path::new("C:\\G")).unwrap();
        assert!(s.executable.ends_with("game.exe"));
        assert!(s.args.is_empty());
    }
}
