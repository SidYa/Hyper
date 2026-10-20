//! Folder browsing for the console-mode folder picker (the native dialog
//! cannot be driven with a controller): drives with their free space,
//! folders, a few places to jump to, and creating a folder.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Above this many entries the rest is left out (a picker, not a file manager).
const MAX_FOLDERS: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub name: String,
    pub path: String,
    /// Drives only: bytes free / in total on the volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderListing {
    /// Empty for the list of drives / roots.
    pub path: String,
    /// One level up: `Some("")` for the roots, `None` when already there.
    pub parent: Option<String>,
    pub folders: Vec<FolderEntry>,
    /// Bytes free on the volume of `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free: Option<u64>,
}

/// A place the picker offers a jump to; the UI names it by `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceEntry {
    /// `games`, `home`, `documents`, `desktop`, `downloads`
    pub id: String,
    pub path: String,
}

/// Folders inside `path`; with `None` (or an empty path) the drives. A path
/// that does not exist (yet) opens its nearest existing parent.
pub fn list(path: Option<&str>) -> Result<FolderListing> {
    let Some(start) = path.map(str::trim).filter(|p| !p.is_empty()) else {
        let folders = crate::platform::current()
            .roots()
            .into_iter()
            .map(|r| {
                let space = free_space(&r);
                FolderEntry {
                    name: r.to_string_lossy().into_owned(),
                    path: r.to_string_lossy().into_owned(),
                    free: space.map(|s| s.0),
                    total: space.map(|s| s.1),
                }
            })
            .collect();
        return Ok(FolderListing {
            path: String::new(),
            parent: None,
            folders,
            free: None,
        });
    };
    let mut dir = PathBuf::from(start);
    if !dir.is_absolute() {
        return Err(Error::Other(format!("not a full path: {start}")));
    }
    while !dir.is_dir() {
        if !dir.pop() {
            return list(None);
        }
    }
    let mut folders: Vec<FolderEntry> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (!is_hidden(&e.path(), &name)).then(|| FolderEntry {
                path: e.path().to_string_lossy().into_owned(),
                name,
                free: None,
                total: None,
            })
        })
        .collect();
    // sorted first, so a cut list shows the first names, not a random set
    folders.sort_by_key(|f| natural_key(&f.name));
    folders.truncate(MAX_FOLDERS);
    let parent = Some(
        dir.parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    let free = free_space(&dir).map(|s| s.0);
    Ok(FolderListing {
        path: dir.to_string_lossy().into_owned(),
        parent,
        folders,
        free,
    })
}

/// Places worth a jump: the games library and the user's main folders,
/// those that exist (the library's nearest existing parent otherwise).
pub fn places(games_dir: &str) -> Vec<PlaceEntry> {
    let mut out = Vec::new();
    let games = games_dir.trim();
    if !games.is_empty() {
        let mut p = PathBuf::from(games);
        while !p.is_dir() && p.pop() {}
        if p.is_dir() {
            out.push(PlaceEntry {
                id: "games".into(),
                path: p.to_string_lossy().into_owned(),
            });
        }
    }
    for (id, dir) in [
        ("home", dirs::home_dir()),
        ("documents", dirs::document_dir()),
        ("desktop", dirs::desktop_dir()),
        ("downloads", dirs::download_dir()),
    ] {
        if let Some(d) = dir.filter(|d| d.is_dir()) {
            out.push(PlaceEntry {
                id: id.into(),
                path: d.to_string_lossy().into_owned(),
            });
        }
    }
    out
}

/// Create `name` inside `parent` (fine when it exists already) and return
/// its path. The name must be one plain folder name.
pub fn create(parent: &str, name: &str) -> Result<String> {
    let name = name.trim();
    if !crate::stores::is_plain_folder_name(name) {
        return Err(Error::Other(format!("not a valid folder name: {name:?}")));
    }
    let parent = Path::new(parent.trim());
    if !parent.is_absolute() || !parent.is_dir() {
        return Err(Error::Other(format!(
            "folder not found: {}",
            parent.display()
        )));
    }
    let dir = parent.join(name);
    if dir.exists() && !dir.is_dir() {
        return Err(Error::Other(format!("a file named {name:?} is in the way")));
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir.to_string_lossy().into_owned())
}

/// Bytes free and in total on the volume holding `path`.
pub fn free_space(path: &Path) -> Option<(u64, u64)> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        use windows::core::PCWSTR;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let (mut free, mut total) = (0u64, 0u64);
        // SAFETY: a NUL-terminated path and out-pointers that outlive the call
        unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(wide.as_ptr()),
                Some(&mut free),
                Some(&mut total),
                None,
            )
        }
        .ok()?;
        Some((free, total))
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: a NUL-terminated path and a zeroed struct of the right type
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        Some((
            st.f_bavail as u64 * st.f_frsize as u64,
            st.f_blocks as u64 * st.f_frsize as u64,
        ))
    }
}

/// System and hidden folders (`$Recycle.Bin`, `System Volume Information`,
/// dot folders) are of no use as a games folder.
fn is_hidden(path: &Path, name: &str) -> bool {
    if name.starts_with('$')
        || name.starts_with('.')
        || name.eq_ignore_ascii_case("System Volume Information")
    {
        return true;
    }
    crate::platform::current().is_hidden(path)
}

/// Case-insensitive with numbers compared by value: `Game 2` before `Game 10`.
fn natural_key(name: &str) -> Vec<(u64, String)> {
    let mut key = Vec::new();
    let mut num = String::new();
    let mut text = String::new();
    for c in name.chars() {
        if c.is_ascii_digit() {
            if !text.is_empty() {
                key.push((u64::MAX, std::mem::take(&mut text)));
            }
            num.push(c);
        } else {
            if !num.is_empty() {
                key.push((num.parse().unwrap_or(u64::MAX), String::new()));
                num.clear();
            }
            text.extend(c.to_lowercase());
        }
    }
    if !num.is_empty() {
        key.push((num.parse().unwrap_or(u64::MAX), String::new()));
    }
    if !text.is_empty() {
        key.push((u64::MAX, text));
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_subfolders_and_climbs_to_an_existing_parent() {
        let root = std::env::temp_dir().join(format!("hyper-browse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("b Games")).unwrap();
        std::fs::create_dir_all(root.join("A")).unwrap();
        std::fs::create_dir_all(root.join("Game 10")).unwrap();
        std::fs::create_dir_all(root.join("Game 2")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::write(root.join("file.txt"), "x").unwrap();

        let l = list(Some(&root.join("missing").join("deeper").to_string_lossy())).unwrap();
        assert_eq!(PathBuf::from(&l.path), root);
        let names: Vec<&str> = l.folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["A", "b Games", "Game 2", "Game 10"]);
        assert_eq!(
            l.parent.as_deref().map(PathBuf::from),
            root.parent().map(Path::to_path_buf)
        );
        assert!(l.free.is_some());

        let drives = list(None).unwrap();
        assert!(drives.parent.is_none());
        assert!(!drives.folders.is_empty());
        assert!(drives.folders[0].total.is_some());

        let made = create(&root.to_string_lossy(), "New Games").unwrap();
        assert!(Path::new(&made).is_dir());
        assert!(create(&root.to_string_lossy(), "../x").is_err());
        assert!(create(&root.to_string_lossy(), "").is_err());
        assert!(
            places(&root.join("nope").to_string_lossy())
                .iter()
                .any(|p| p.id == "games" && Path::new(&p.path) == root)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
