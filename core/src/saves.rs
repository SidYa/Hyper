//! Cloud saves: the checks shared by the stores. The sync itself is done by
//! the store clients (`epic::saves`, `gog::native::saves`);
//! auto-sync is the global `autoSyncSaves` setting (see `Stores::launch`).
//! Saves played offline wait in `pending-saves.json` until their upload gets
//! through (`Stores::upload_pending`).
//!
//! Port of the checks around Heroic's `save_sync.ts`.

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::config::AppSettings;
use crate::error::{Error, Result};
use crate::types::Runner;

/// Where Windows keeps a user's folders, as the save-path variables of Epic
/// (`{AppData}`, `{UserDir}`…) and GOG (`<?SAVED_GAMES?>`…) mean them. On
/// Windows these are the user's real folders; for a game under Proton they
/// are inside its prefix (`drive_c/users/steamuser/…`), never the Linux home
/// (Heroic's `getDefaultSavePath` does the same with the prefix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    pub user_profile: PathBuf,
    pub documents: PathBuf,
    pub local_app_data: PathBuf,
    pub local_low: PathBuf,
    pub roaming: PathBuf,
    pub saved_games: PathBuf,
}

impl Roots {
    /// The user's own folders (Windows). `None` only when the home folder is
    /// unknown.
    pub fn host() -> Option<Self> {
        let home = dirs::home_dir()?;
        let local = dirs::data_local_dir()?;
        Some(Self {
            // the shell's Documents, which may be moved (OneDrive)
            documents: dirs::document_dir().unwrap_or_else(|| home.join("Documents")),
            local_low: local.parent()?.join("LocalLow"),
            local_app_data: local,
            roaming: dirs::data_dir()?,
            saved_games: home.join("Saved Games"),
            user_profile: home,
        })
    }

    /// The folders of Proton's `steamuser` inside a Wine prefix.
    pub fn prefix(prefix: &Path) -> Self {
        let user = prefix.join("drive_c").join("users").join("steamuser");
        Self {
            documents: user.join("Documents"),
            local_app_data: user.join("AppData").join("Local"),
            local_low: user.join("AppData").join("LocalLow"),
            roaming: user.join("AppData").join("Roaming"),
            saved_games: user.join("Saved Games"),
            user_profile: user,
        }
    }

    /// The folders themselves (and what holds them: `AppData`, `users`,
    /// `drive_c`, the prefix), the user's media folders and the Windows
    /// system folders next to `users` are never a game's save folder.
    pub fn protected(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .user_profile
            .ancestors()
            .take(4)
            .map(Path::to_path_buf)
            .collect();
        out.extend(
            [
                &self.documents,
                &self.local_app_data,
                &self.local_low,
                &self.roaming,
                &self.saved_games,
            ]
            .into_iter()
            .cloned(),
        );
        out.extend(
            ["Pictures", "Music", "Videos", "Desktop", "Downloads"]
                .into_iter()
                .map(|n| self.user_profile.join(n)),
        );
        if let Some(app_data) = self.local_app_data.parent() {
            out.push(app_data.to_path_buf());
        }
        // `<drive_c>` = users/../..: Windows, Program Files, ProgramData
        if let Some(drive_c) = self.user_profile.parent().and_then(Path::parent) {
            out.extend(
                [
                    "windows",
                    "Program Files",
                    "Program Files (x86)",
                    "ProgramData",
                ]
                .into_iter()
                .map(|n| drive_c.join(n)),
            );
        }
        out
    }
}

/// The roots a game's save paths resolve against: its prefix where games
/// run through Proton, the user's folders where they run natively.
pub fn roots(settings: &AppSettings, runner: Runner, app_name: &str) -> Option<Roots> {
    if crate::platform::current().needs_wine() {
        Some(Roots::prefix(&crate::compat::prefix_dir(
            settings, runner, app_name,
        )))
    } else {
        Roots::host()
    }
}

/// Folders found to exist are marked so the UI can open them.
pub fn describe(locations: Vec<crate::types::SaveLocation>) -> Vec<crate::types::SaveLocationInfo> {
    locations
        .into_iter()
        .map(|l| crate::types::SaveLocationInfo {
            exists: !l.path.is_empty() && Path::new(&l.path).is_dir(),
            name: l.name,
            path: l.path,
        })
        .collect()
}

/// A download replaces the files in the save folder (the sync cleans it
/// first), so a folder picked by mistake must never be a drive root, the
/// user's or the system's folders (`roots`: on this PC or in the game's
/// prefix), the games library (`games_dir`: the `default_install_path`
/// setting), or a folder containing one of those.
pub fn check_save_dir(path: &str, roots: Option<&Roots>, games_dir: &str) -> Result<()> {
    let dir = Path::new(path.trim());
    let refuse = |why: &str| {
        Err(Error::Other(format!(
            "refusing to sync saves with {}: {why}",
            dir.display()
        )))
    };
    if path.trim().is_empty() {
        return Err(Error::Other(
            "the save folder is not known; pick it in the cloud save settings".into(),
        ));
    }
    if !dir.is_absolute() || !dir.components().any(|c| matches!(c, Component::Normal(_))) {
        return refuse("not a full path to a folder");
    }
    if dir.components().any(|c| matches!(c, Component::ParentDir)) {
        return refuse("the path contains \"..\"");
    }
    let mut protected = crate::stores::protected_dirs(games_dir);
    if let Some(home) = dirs::home_dir() {
        protected.extend(
            ["Saved Games", ".steam", ".local", ".config"]
                .into_iter()
                .map(|n| home.join(n)),
        );
    }
    if let Some(appdata) = dirs::data_local_dir().as_deref().and_then(Path::parent) {
        protected.push(appdata.join("LocalLow"));
    }
    protected.extend(
        [
            dirs::data_dir(),
            dirs::picture_dir(),
            dirs::audio_dir(),
            dirs::video_dir(),
        ]
        .into_iter()
        .flatten(),
    );
    if let Some(one_drive) = std::env::var_os("OneDrive").filter(|v| !v.is_empty()) {
        protected.push(PathBuf::from(one_drive));
    }
    protected.extend(
        ["/usr", "/etc", "/var", "/opt"]
            .into_iter()
            .map(PathBuf::from),
    );
    if let Some(roots) = roots {
        protected.extend(roots.protected());
    }
    if crate::stores::is_or_contains(dir, &protected) {
        return refuse("it is a system or user folder, not a game's save folder");
    }
    Ok(())
}

/// Absolute path with `.` / `..` folded away and separators made uniform
/// (`{AppData}/../LocalLow/X` → `…\AppData\LocalLow\X` on Windows, `…/
/// AppData/LocalLow/X` on Linux, where the templates' backslashes are
/// separators too). `None` for a relative path or one that climbs above
/// its root.
pub(crate) fn normalize(path: &str) -> Option<String> {
    #[cfg(unix)]
    let unix = path.replace('\\', "/");
    #[cfg(unix)]
    let path = unix.as_str();
    let mut out = PathBuf::new();
    for c in Path::new(path.trim()).components() {
        match c {
            Component::Prefix(_) | Component::RootDir => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                // `pop` refuses to go above the root ("C:\")
                if !out.pop() {
                    return None;
                }
            }
            Component::Normal(n) => out.push(n),
        }
    }
    #[cfg(windows)]
    let s = out.to_string_lossy().replace('/', "\\");
    #[cfg(unix)]
    let s = out.to_string_lossy().into_owned();
    (out.is_absolute() && out.components().any(|c| matches!(c, Component::Normal(_)))).then_some(s)
}

/// Games whose saves changed while their store was out of reach (or whose
/// upload failed), kept in `pending-saves.json` across restarts until an
/// upload succeeds. `Stores::upload_pending` retries them once online.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingUpload {
    pub runner: Runner,
    pub app_name: String,
}

static PENDING_LOCK: Mutex<()> = Mutex::new(());

fn pending_file() -> PathBuf {
    crate::paths::config_dir().join("pending-saves.json")
}

fn read_pending() -> Vec<PendingUpload> {
    std::fs::read_to_string(pending_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_pending(list: &[PendingUpload]) {
    let file = pending_file();
    let tmp = file.with_extension("json.tmp");
    let written = serde_json::to_string_pretty(list)
        .map_err(std::io::Error::other)
        .and_then(|s| std::fs::write(&tmp, s))
        .and_then(|()| std::fs::rename(&tmp, &file));
    if let Err(e) = written {
        tracing::warn!(error = %e, "could not save the pending save uploads");
    }
}

pub fn pending() -> Vec<PendingUpload> {
    let _g = PENDING_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    read_pending()
}

pub fn is_pending(runner: Runner, app_name: &str) -> bool {
    pending()
        .iter()
        .any(|p| p.runner == runner && p.app_name == app_name)
}

/// Mark (`true`) or clear the game's saves as waiting for an upload.
pub fn set_pending(runner: Runner, app_name: &str, waiting: bool) {
    let _g = PENDING_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = read_pending();
    let had = list
        .iter()
        .any(|p| p.runner == runner && p.app_name == app_name);
    if waiting && !had {
        list.push(PendingUpload {
            runner,
            app_name: app_name.to_string(),
        });
    } else if !waiting && had {
        list.retain(|p| !(p.runner == runner && p.app_name == app_name));
    } else {
        return;
    }
    write_pending(&list);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_broad_folders() {
        assert!(check_save_dir("", None, "").is_err());
        assert!(check_save_dir("C:\\", None, "").is_err());
        assert!(check_save_dir("relative\\saves", None, "").is_err());
        assert!(check_save_dir("D:\\Games\\X\\..\\..", None, "").is_err());
        if let Some(docs) = dirs::document_dir() {
            assert!(
                check_save_dir(&docs.to_string_lossy(), None, "").is_err(),
                "Documents"
            );
            assert!(
                check_save_dir(&docs.join("My Games").join("X").to_string_lossy(), None, "")
                    .is_ok()
            );
        }
        if let Some(local) = dirs::data_local_dir() {
            assert!(
                check_save_dir(&local.to_string_lossy(), None, "").is_err(),
                "LocalAppData"
            );
            assert!(
                check_save_dir(
                    &local.join("Studio").join("Game").to_string_lossy(),
                    None,
                    ""
                )
                .is_ok()
            );
        }
        if let Some(home) = dirs::home_dir() {
            assert!(check_save_dir(&home.join("Saved Games").to_string_lossy(), None, "").is_err());
            assert!(check_save_dir(&home.join(".local").to_string_lossy(), None, "").is_err());
            assert!(check_save_dir(&home.join(".steam").to_string_lossy(), None, "").is_err());
            // Windows drops a trailing dot or space: still Documents
            #[cfg(windows)]
            assert!(
                check_save_dir(
                    &format!("{}.", home.join("Documents").to_string_lossy()),
                    None,
                    ""
                )
                .is_err()
            );
        }
        if let Some(pics) = dirs::picture_dir() {
            assert!(
                check_save_dir(&pics.to_string_lossy(), None, "").is_err(),
                "Pictures"
            );
        }
        // the games library of the settings
        let games = if cfg!(windows) {
            "E:\\Games\\Hyper"
        } else {
            "/mnt/games/Hyper"
        };
        assert!(
            check_save_dir(games, None, games).is_err(),
            "the games library"
        );
        assert!(
            check_save_dir(
                &format!(
                    "{games}{}Game{}Saves",
                    std::path::MAIN_SEPARATOR,
                    std::path::MAIN_SEPARATOR
                ),
                None,
                games
            )
            .is_ok()
        );
        #[cfg(unix)]
        assert!(check_save_dir("/usr/share", None, "").is_err());
    }

    #[test]
    fn prefix_folders_are_protected_too() {
        let base = if cfg!(windows) {
            PathBuf::from("C:\\Prefixes\\gog\\Game")
        } else {
            PathBuf::from("/home/deck/Prefixes/gog/Game")
        };
        let roots = Roots::prefix(&base);
        let s = |p: PathBuf| p.to_string_lossy().into_owned();
        assert!(
            check_save_dir(&s(base.clone()), Some(&roots), "").is_err(),
            "the prefix"
        );
        assert!(check_save_dir(&s(base.join("drive_c")), Some(&roots), "").is_err());
        assert!(
            check_save_dir(&s(base.join("drive_c").join("windows")), Some(&roots), "").is_err(),
            "drive_c/windows"
        );
        assert!(
            check_save_dir(
                &s(base.join("drive_c").join("Program Files (x86)")),
                Some(&roots),
                ""
            )
            .is_err()
        );
        assert!(
            check_save_dir(
                &s(base.join("drive_c").join("ProgramData")),
                Some(&roots),
                ""
            )
            .is_err()
        );
        assert!(
            check_save_dir(&s(roots.user_profile.clone()), Some(&roots), "").is_err(),
            "steamuser"
        );
        assert!(
            check_save_dir(&s(roots.user_profile.join("Pictures")), Some(&roots), "").is_err(),
            "Pictures"
        );
        assert!(
            check_save_dir(
                &s(roots
                    .local_app_data
                    .parent()
                    .expect("AppData")
                    .to_path_buf()),
                Some(&roots),
                ""
            )
            .is_err(),
            "AppData"
        );
        assert!(
            check_save_dir(&s(roots.documents.clone()), Some(&roots), "").is_err(),
            "Documents"
        );
        assert!(check_save_dir(&s(roots.saved_games.clone()), Some(&roots), "").is_err());
        assert!(
            check_save_dir(
                &s(roots.documents.join("My Games").join("X")),
                Some(&roots),
                ""
            )
            .is_ok()
        );
        assert!(
            check_save_dir(
                &s(roots.local_low.join("Studio").join("Game")),
                Some(&roots),
                ""
            )
            .is_ok()
        );
        assert!(
            check_save_dir(
                &s(base
                    .join("drive_c")
                    .join("Program Files (x86)")
                    .join("Studio")
                    .join("Saves")),
                Some(&roots),
                ""
            )
            .is_ok()
        );
        // the same folders are plain game folders without the prefix roots
        assert!(check_save_dir(&s(roots.documents.clone()), None, "").is_ok());
    }

    #[test]
    fn prefix_roots_follow_protons_layout() {
        let r = Roots::prefix(Path::new("/p"));
        assert_eq!(r.user_profile, PathBuf::from("/p/drive_c/users/steamuser"));
        assert_eq!(
            r.local_low,
            PathBuf::from("/p/drive_c/users/steamuser/AppData/LocalLow")
        );
        assert_eq!(
            r.roaming,
            PathBuf::from("/p/drive_c/users/steamuser/AppData/Roaming")
        );
        assert_eq!(
            r.saved_games,
            PathBuf::from("/p/drive_c/users/steamuser/Saved Games")
        );
        assert!(r.protected().contains(&PathBuf::from("/p")));
    }

    #[cfg(windows)]
    #[test]
    fn normalizes_paths() {
        assert_eq!(
            normalize("C:/Users/a/AppData/Local/../LocalLow/X/").as_deref(),
            Some("C:\\Users\\a\\AppData\\LocalLow\\X")
        );
        assert_eq!(
            normalize("E:\\G\\\\.\\saves").as_deref(),
            Some("E:\\G\\saves")
        );
        assert_eq!(normalize("C:\\..\\x"), None);
        assert_eq!(normalize("saves"), None);
        assert_eq!(normalize("C:\\"), None);
    }

    #[cfg(unix)]
    #[test]
    fn normalizes_paths() {
        assert_eq!(
            normalize("/p/drive_c/users/steamuser/AppData/Local/../LocalLow/X/").as_deref(),
            Some("/p/drive_c/users/steamuser/AppData/LocalLow/X")
        );
        // the templates come from Windows: backslashes are separators
        assert_eq!(
            normalize("/games/G\\\\.\\saves").as_deref(),
            Some("/games/G/saves")
        );
        assert_eq!(normalize("/../x"), None);
        assert_eq!(normalize("saves"), None);
        assert_eq!(
            normalize("C:\\Users\\a"),
            None,
            "a drive letter is no Linux path"
        );
        assert_eq!(normalize("/"), None);
    }
}
