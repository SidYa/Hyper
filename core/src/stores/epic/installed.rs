//! `installed.json` the way legendary keeps it, and the three operations on
//! it we do ourselves: sizes of a build (`legendary info`), importing a
//! folder (`legendary import`) and uninstalling (`legendary uninstall`).
//! Ports of the matching parts of legendary's `core.py` and `cli.py`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value, json};

use super::api::Epic;
use super::library::{catalog_ids, dlcs_of};
use super::manifest::Manifest;
use super::{manifest_path, manifest_path_legacy, metadata_path, read_json, write_json};
use crate::error::{Error, Result};
use crate::stores::legendary::library::read_latest_versions;

pub const NO_FILES_MESSAGE: &str =
    "The game files are not in this folder (it may be left over after uninstalling).";

/// Download and disk size of the latest build, plus its version.
pub struct BuildInfo {
    pub version: String,
    pub download_size: u64,
    pub disk_size: u64,
}

pub async fn build_info(epic: &Epic, app_name: &str, platform: &str) -> Result<BuildInfo> {
    let dir = &epic.config_dir;
    let game = read_json(&metadata_path(dir, app_name))
        .ok_or_else(|| Error::GameNotFound(app_name.to_string()))?;
    if game.pointer(&format!("/asset_infos/{platform}")).is_none() {
        // titles another launcher installs, or builds for other platforms only
        return Err(Error::Helper {
            helper: "Epic".into(),
            message: format!("{app_name} has no {platform} build to install"),
        });
    }
    let (namespace, catalog_id) = catalog_ids(&game);
    let record = epic
        .manifest_record(&namespace, &catalog_id, app_name, platform)
        .await?;
    let (bytes, _, _) = epic.cdn_manifest(&record).await?;
    // sizes need no names: an encrypted manifest is fine as it is
    let manifest = Manifest::parse(&bytes)?;
    let version = read_latest_versions(dir)
        .remove(&(platform.to_string(), app_name.to_string()))
        .unwrap_or_else(|| manifest.meta.build_version.clone());
    Ok(BuildInfo {
        version,
        download_size: manifest.download_size(),
        disk_size: manifest.disk_size(),
    })
}

fn installed_file(dir: &Path) -> PathBuf {
    dir.join("installed.json")
}

pub(crate) fn read_installed_raw(dir: &Path) -> Map<String, Value> {
    read_json(&installed_file(dir))
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

pub(crate) fn write_installed_raw(dir: &Path, all: Map<String, Value>) -> Result<()> {
    write_json(&installed_file(dir), &Value::Object(all))
}

/// Register the game at `app_path` as installed. The manifest comes from an
/// Epic Games Launcher install in the folder (`.egstore`) when there is one,
/// else from the CDN; it is saved for later updates / repairs, which need
/// verification first unless EGL finished the install. `with_dlcs`: the
/// game's installed DLCs are imported from the same folder afterwards.
pub async fn import(
    epic: &Arc<Epic>,
    app_name: &str,
    app_path: &Path,
    platform: &str,
    with_dlcs: bool,
) -> Result<()> {
    let dir = epic.config_dir.clone();
    let game = read_json(&metadata_path(&dir, app_name))
        .ok_or_else(|| Error::GameNotFound(app_name.to_string()))?;
    if read_installed_raw(&dir).contains_key(app_name) {
        return Err(Error::Other(format!("{app_name} is already installed")));
    }
    let is_dlc = game.pointer("/metadata/mainGameItem").is_some();
    if is_dlc {
        let base = game
            .pointer("/metadata/mainGameItem/releaseInfo/0/appId")
            .and_then(Value::as_str);
        match base {
            Some(base) if read_installed_raw(&dir).contains_key(base) => {}
            Some(_) => return Err(Error::Other("the DLC's base game is not installed".into())),
            None => return Err(Error::Other("the DLC has no base game information".into())),
        }
    }
    let (namespace, catalog_id) = catalog_ids(&game);

    let mut needs_verification = true;
    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut secrets = std::collections::HashMap::new();
    let mut base_urls: Vec<String> = game
        .get("base_urls")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let egstore = app_path.join(".egstore");
    if !is_dlc
        && egstore.is_dir()
        && let Some(manifest_file) = egl_manifest(&egstore, app_name)
    {
        match std::fs::read(&manifest_file) {
            Ok(bytes) => {
                tracing::info!(app = app_name, "Epic Games Launcher install found");
                manifest_bytes = Some(bytes);
                // an install EGL finished needs no verification
                let pending = egstore.join("Pending");
                let pending_files = std::fs::read_dir(&pending)
                    .map(|rd| rd.count() > 0)
                    .unwrap_or(false);
                needs_verification = egstore.join("bps").exists() || pending_files;
            }
            Err(e) => {
                tracing::warn!(error = %e, "EGL manifest unreadable; importing as a plain folder")
            }
        }
    }
    let bytes = match manifest_bytes {
        Some(b) => b,
        None => {
            let record = epic
                .manifest_record(&namespace, &catalog_id, app_name, platform)
                .await?;
            let (bytes, urls, record_secrets) = epic.cdn_manifest(&record).await?;
            secrets = record_secrets;
            if base_urls.is_empty() {
                base_urls = urls;
                let mut updated = game.clone();
                updated["base_urls"] = json!(base_urls);
                write_json(&metadata_path(&dir, app_name), &updated)?;
            }
            bytes
        }
    };
    let mut manifest = Manifest::parse(&bytes)?;
    let was_encrypted = manifest.encrypted;
    manifest.decrypt(&secrets)?;
    let saved = manifest_path(&dir, app_name, platform, &manifest.meta.build_version);
    if let Some(parent) = saved.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // kept decrypted, so uninstall and repair work without asking Epic for the key
    std::fs::write(
        &saved,
        if was_encrypted {
            manifest.write()
        } else {
            bytes
        },
    )?;

    // the folder must hold this game: most of its files, and its executable
    let case_insensitive = !platform.starts_with("Win") || cfg!(not(windows));
    let total = manifest.files.len().max(1);
    let manifest = Arc::new(manifest);
    let found = {
        let (m, root) = (manifest.clone(), app_path.to_path_buf());
        tokio::task::spawn_blocking(move || {
            m.files
                .iter()
                .filter(|f| {
                    locate_checked(&root, &f.filename, case_insensitive).is_ok_and(|p| p.is_file())
                })
                .count()
        })
        .await
        .map_err(|e| Error::Other(format!("import task failed: {e}")))?
    };
    if found == 0 {
        return Err(Error::Other(NO_FILES_MESSAGE.into()));
    }
    if !is_dlc {
        let exe = locate_checked(
            app_path,
            manifest.meta.launch_exe.trim_start_matches(['/', '\\']),
            case_insensitive,
        )?;
        if !exe.is_file() {
            return Err(Error::Other(format!(
                "the game's executable was not found at {}",
                exe.display()
            )));
        }
    }
    if (found as f64) / (total as f64) < 0.95 {
        tracing::warn!(
            app = app_name,
            found,
            total,
            "some files are missing; the install may be incomplete"
        );
    }

    let attr = |k: &str, default: &str| {
        game.pointer(&format!("/metadata/customAttributes/{k}/value"))
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_ascii_lowercase()
    };
    let prereq = (!manifest.meta.prereq_ids.is_empty()).then(|| {
        json!({
            "ids": manifest.meta.prereq_ids,
            "name": manifest.meta.prereq_name,
            "path": manifest.meta.prereq_path,
            "args": manifest.meta.prereq_args,
        })
    });
    let entry = json!({
        "app_name": app_name,
        "install_path": app_path.to_string_lossy(),
        "title": game.get("app_title").cloned().unwrap_or(Value::from(app_name)),
        "version": manifest.meta.build_version,
        "base_urls": base_urls,
        "can_run_offline": attr("CanRunOffline", "true") == "true",
        "egl_guid": "",
        "executable": manifest.meta.launch_exe,
        "install_size": manifest.disk_size(),
        "install_tags": [],
        "is_dlc": is_dlc,
        "launch_parameters": manifest.meta.launch_command,
        "manifest_path": "",
        "needs_verification": needs_verification,
        "platform": platform,
        "prereq_info": prereq.unwrap_or(Value::Null),
        "uninstaller": Value::Null,
        "requires_ot": attr("OwnershipToken", "false") == "true",
        "save_path": Value::Null,
        "is_preloaded": false,
    });
    let mut installed = read_installed_raw(&dir);
    installed.insert(app_name.to_string(), entry);
    write_json(&installed_file(&dir), &Value::Object(installed))?;
    tracing::info!(app = app_name, path = %app_path.display(), needs_verification, "Epic game imported");

    if with_dlcs && !is_dlc {
        for dlc in dlcs_of(&dir, &catalog_id, platform) {
            if read_installed_raw(&dir).contains_key(&dlc) {
                continue;
            }
            if let Err(e) = Box::pin(import(epic, &dlc, app_path, platform, false)).await {
                tracing::info!(app = app_name, dlc, error = %e, "DLC not imported");
            }
        }
    }
    Ok(())
}

/// The manifest EGL left for this game: `.egstore/<guid>.mancpn` names the
/// app, `<guid>.manifest` is the file.
fn egl_manifest(egstore: &Path, app_name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(egstore).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("mancpn") {
            continue;
        }
        let Some(mancpn) = read_json(&path) else {
            continue;
        };
        if mancpn.get("AppName").and_then(Value::as_str) == Some(app_name) {
            let manifest = path.with_extension("manifest");
            return manifest.is_file().then_some(manifest);
        }
    }
    None
}

/// Record a game's folder after a move: its entry and the DLCs that were
/// installed into the same folder.
pub fn change_install_path(dir: &Path, app_name: &str, new_path: &Path) -> Result<()> {
    let mut all = read_installed_raw(dir);
    let old = all
        .get(app_name)
        .and_then(|e| e.get("install_path"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::Other(format!("{app_name} is not installed")))?;
    let new = new_path.to_string_lossy().into_owned();
    for (name, entry) in all.iter_mut() {
        let same_folder = entry.get("install_path").and_then(Value::as_str) == Some(old.as_str());
        if name == app_name || same_folder {
            entry["install_path"] = Value::String(new.clone());
        }
    }
    write_installed_raw(dir, all)
}

/// Remove the game (its DLCs first) from `installed.json` and delete the
/// files its manifest lists; folders left empty go too. The caller removes
/// what the game itself wrote (`remove_game_dir`). A DLC on its own loses
/// only the files no other title in the same folder lists, and never the
/// folder.
pub async fn uninstall(dir: PathBuf, app_name: String) -> Result<()> {
    tokio::task::spawn_blocking(move || uninstall_blocking(&dir, &app_name))
        .await
        .map_err(|e| Error::Other(format!("uninstall task failed: {e}")))?
}

fn uninstall_blocking(dir: &Path, app_name: &str) -> Result<()> {
    let mut installed = read_installed_raw(dir);
    let entry = installed
        .get(app_name)
        .cloned()
        .ok_or_else(|| Error::Other(format!("{app_name} is not installed")))?;
    let is_dlc = entry
        .get("is_dlc")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut keep: BTreeSet<String> = BTreeSet::new();
    if is_dlc {
        // what the base game and the other DLCs in the folder still need
        let folder = entry
            .get("install_path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        for (other, other_entry) in installed.iter().filter(|(n, _)| n.as_str() != app_name) {
            if other_entry.get("install_path").and_then(Value::as_str) != Some(folder) {
                continue;
            }
            match installed_manifest(dir, other, other_entry) {
                Some(m) => keep.extend(m.files.iter().map(|f| f.filename.clone())),
                None => tracing::warn!(
                    app = app_name,
                    other,
                    "no manifest for a title sharing the folder; its files may go with the DLC"
                ),
            }
        }
    } else {
        let platform = entry
            .get("platform")
            .and_then(Value::as_str)
            .unwrap_or("Windows");
        if let Some(game) = read_json(&metadata_path(dir, app_name)) {
            let (_, catalog_id) = catalog_ids(&game);
            for dlc in dlcs_of(dir, &catalog_id, platform) {
                if let Some(dlc_entry) = installed.get(&dlc).cloned() {
                    tracing::info!(app = app_name, dlc, "uninstalling DLC");
                    remove_files(dir, &dlc, &dlc_entry, false, &BTreeSet::new());
                    installed.remove(&dlc);
                }
            }
        }
    }
    remove_files(dir, app_name, &entry, !is_dlc, &keep);
    installed.remove(app_name);
    write_json(&installed_file(dir), &Value::Object(installed))?;
    tracing::info!(app = app_name, "Epic game uninstalled");
    Ok(())
}

/// The manifest of the build an `installed.json` entry records.
fn installed_manifest(dir: &Path, app_name: &str, entry: &Value) -> Option<Manifest> {
    let s = |k: &str| entry.get(k).and_then(Value::as_str).unwrap_or_default();
    let (version, platform) = (s("version"), s("platform"));
    let bytes = std::fs::read(manifest_path(dir, app_name, platform, version))
        .or_else(|_| std::fs::read(manifest_path_legacy(dir, app_name, version)));
    match bytes.map_err(Error::from).and_then(|b| Manifest::parse(&b)) {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::warn!(app = app_name, error = %e, "no manifest for the installed build");
            None
        }
    }
}

/// Delete the files the installed build's manifest lists (for the install
/// tags chosen), except those in `keep`. Without the manifest only the entry
/// goes; the folder is then left to `remove_game_dir`.
fn remove_files(
    dir: &Path,
    app_name: &str,
    entry: &Value,
    delete_root: bool,
    keep: &BTreeSet<String>,
) {
    let s = |k: &str| entry.get(k).and_then(Value::as_str).unwrap_or_default();
    let (install_path, platform) = (s("install_path"), s("platform"));
    let tags: Vec<String> = entry
        .get("install_tags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let Some(manifest) = installed_manifest(dir, app_name, entry) else {
        tracing::warn!(app = app_name, "files left to the folder cleanup");
        return;
    };
    if install_path.is_empty() {
        return;
    }
    let files: Vec<String> = manifest
        .files_for_tags(&tags)
        .map(|f| f.filename.clone())
        .filter(|n| !keep.contains(n))
        .collect();
    let root = Path::new(install_path);
    if !delete_filelist(root, &files, delete_root, platform.starts_with("Win")) {
        tracing::warn!(
            app = app_name,
            path = install_path,
            "some game files could not be deleted"
        );
    }
}

/// legendary's `delete_filelist`: the files, then every folder they were in
/// (deepest first, only if empty), then the root when asked. `root` must be
/// absolute and every name confined to it (`installed.json` and the
/// manifests are plain files anyone can edit); a name that is not is
/// skipped and reported.
pub(crate) fn delete_filelist(
    root: &Path,
    files: &[String],
    delete_root: bool,
    case_insensitive: bool,
) -> bool {
    if !root.is_absolute() {
        tracing::warn!(root = %root.display(), "refusing to delete below a relative folder");
        return false;
    }
    let mut ok = true;
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for name in files {
        let full = match locate_checked(root, name, case_insensitive) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "not deleted");
                ok = false;
                continue;
            }
        };
        let rel = Path::new(name);
        let mut parent = rel.parent();
        while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
            dirs.insert(p.to_path_buf());
            parent = p.parent();
        }
        if let Err(e) = std::fs::remove_file(&full)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(file = %full.display(), error = %e, "could not delete");
            ok = false;
        }
    }
    let mut dirs: Vec<PathBuf> = dirs.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.as_os_str().len()));
    for d in dirs {
        // only empty folders go: anything the game wrote stays for the caller
        if let Ok(full) = locate_checked(root, &d.to_string_lossy(), case_insensitive) {
            let _ = std::fs::remove_dir(full);
        }
    }
    if delete_root {
        let _ = std::fs::remove_dir(root);
    }
    ok
}

/// [`locate`] for names that come from files on disk or from a server
/// (manifests, `installed.json`, resume files, cloud-save listings): refused
/// unless `rel` stays inside `root` (`is_confined_rel_path`). Every writer,
/// deleter and launcher goes through this one.
pub(crate) fn locate_checked(root: &Path, rel: &str, case_insensitive: bool) -> Result<PathBuf> {
    if !crate::stores::is_confined_rel_path(rel) {
        return Err(Error::Other(format!(
            "refusing a path that leaves the game folder: {rel:?}"
        )));
    }
    Ok(locate(root, rel, case_insensitive))
}

/// `root/rel`, found case-insensitively when asked (Windows games on Linux
/// may have been written with other casing). Returns the path as given
/// when nothing matches. No containment check: see [`locate_checked`].
pub(crate) fn locate(root: &Path, rel: &str, case_insensitive: bool) -> PathBuf {
    let direct = root.join(rel);
    if !case_insensitive || direct.exists() {
        return direct;
    }
    let mut cur = root.to_path_buf();
    for part in Path::new(rel).components() {
        let want = part.as_os_str();
        let exact = cur.join(want);
        if exact.exists() {
            cur = exact;
            continue;
        }
        let found = std::fs::read_dir(&cur).ok().and_then(|rd| {
            rd.flatten().map(|e| e.file_name()).find(|n| {
                n.to_string_lossy()
                    .eq_ignore_ascii_case(&want.to_string_lossy())
            })
        });
        match found {
            Some(name) => cur = cur.join(name),
            None => return direct,
        }
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::super::manifest::tests::{build_binary, sample};
    use super::super::manifest::{FileManifest, Meta};
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hyper-epic-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn uninstall_deletes_manifest_files_and_empty_folders() {
        let root = temp("uninstall");
        let config = root.join("config");
        let game_dir = root.join("Games").join("Catnip");
        let (meta, chunks, files) = sample();
        let bytes = build_binary(&meta, &chunks, &files, &[], 18);
        let mpath = manifest_path(&config, "Catnip", "Windows", &meta.build_version);
        std::fs::create_dir_all(mpath.parent().unwrap()).unwrap();
        std::fs::write(&mpath, &bytes).unwrap();
        for f in &files {
            let p = game_dir.join(&f.filename);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        // something the game wrote itself
        std::fs::write(game_dir.join("Content").join("settings.ini"), b"keep").unwrap();
        let installed = json!({
            "Catnip": { "app_name": "Catnip", "install_path": game_dir.to_string_lossy(), "version": meta.build_version,
                        "platform": "Windows", "install_tags": [], "is_dlc": false, "title": "Catnip" },
            "Other": { "app_name": "Other", "install_path": "/elsewhere", "version": "1", "platform": "Windows" }
        });
        write_json(&installed_file(&config), &installed).unwrap();

        uninstall_blocking(&config, "Catnip").unwrap();

        assert!(
            !game_dir.join("Bin").exists(),
            "file and its emptied folder gone"
        );
        assert!(!game_dir.join("Content/Ünïcode.pak").exists());
        assert!(
            game_dir.join("Content/settings.ini").exists(),
            "the game's own file stays"
        );
        assert!(game_dir.exists(), "root not empty, left to remove_game_dir");
        let left = read_installed_raw(&config);
        assert!(!left.contains_key("Catnip") && left.contains_key("Other"));
        assert!(
            uninstall_blocking(&config, "Catnip").is_err(),
            "not installed any more"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_dlc_keeps_what_the_base_game_lists() {
        let root = temp("dlc-uninstall");
        let config = root.join("config");
        let game_dir = root.join("Games").join("Catnip");
        let (meta, chunks, files) = sample();
        // the base game's build lists both files; the DLC's lists the shared one and its own
        let base_bytes = build_binary(&meta, &chunks, &files, &[], 18);
        let mpath = manifest_path(&config, "Catnip", "Windows", &meta.build_version);
        std::fs::create_dir_all(mpath.parent().unwrap()).unwrap();
        std::fs::write(&mpath, &base_bytes).unwrap();
        let dlc_files = vec![
            files[1].clone(),
            FileManifest {
                filename: "Dlc/extra.pak".into(),
                ..files[1].clone()
            },
        ];
        let dlc_meta = Meta {
            app_name: "CatnipDlc".into(),
            ..meta.clone()
        };
        std::fs::write(
            manifest_path(&config, "CatnipDlc", "Windows", &meta.build_version),
            build_binary(&dlc_meta, &chunks, &dlc_files, &[], 18),
        )
        .unwrap();
        for f in files.iter().chain(&dlc_files) {
            let p = game_dir.join(&f.filename);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let installed = json!({
            "Catnip": { "app_name": "Catnip", "install_path": game_dir.to_string_lossy(), "version": meta.build_version,
                        "platform": "Windows", "install_tags": [], "is_dlc": false, "title": "Catnip" },
            "CatnipDlc": { "app_name": "CatnipDlc", "install_path": game_dir.to_string_lossy(), "version": meta.build_version,
                        "platform": "Windows", "install_tags": [], "is_dlc": true, "title": "DLC" }
        });
        write_json(&installed_file(&config), &installed).unwrap();

        uninstall_blocking(&config, "CatnipDlc").unwrap();

        assert!(
            !game_dir.join("Dlc").exists(),
            "the DLC's own file and folder are gone"
        );
        assert!(
            game_dir.join("Content/Ünïcode.pak").exists(),
            "a file the base game lists stays"
        );
        assert!(game_dir.join("Bin/Catnip.exe").exists());
        assert!(game_dir.exists(), "the folder is the base game's");
        let left = read_installed_raw(&config);
        assert!(!left.contains_key("CatnipDlc") && left.contains_key("Catnip"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn deletion_stays_inside_the_root() {
        let root = temp("confined");
        let outside = root.join("outside.txt");
        std::fs::write(&outside, b"keep").unwrap();
        let game = root.join("Game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("ok.txt"), b"x").unwrap();
        assert!(locate_checked(&game, "../outside.txt", false).is_err());
        assert!(locate_checked(&game, "C:\\outside.txt", false).is_err());
        assert!(locate_checked(&game, "sub/ok.txt", true).is_ok());
        let ok = delete_filelist(
            &game,
            &["../outside.txt".to_string(), "ok.txt".to_string()],
            false,
            false,
        );
        assert!(!ok, "the escaping name is reported");
        assert!(outside.exists(), "and left alone");
        assert!(!game.join("ok.txt").exists(), "the confined one went");
        assert!(
            !delete_filelist(Path::new("relative/dir"), &["x".to_string()], false, false),
            "relative roots are refused"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn locate_is_case_insensitive_when_asked() {
        let root = temp("locate");
        std::fs::create_dir_all(root.join("Bin")).unwrap();
        std::fs::write(root.join("Bin").join("Game.exe"), b"x").unwrap();
        assert!(locate(&root, "bin/GAME.EXE", true).is_file());
        // Windows finds it as given; a case-sensitive file system needs the search
        #[cfg(unix)]
        assert_eq!(
            locate(&root, "bin/GAME.EXE", true),
            root.join("Bin").join("Game.exe")
        );
        assert_eq!(locate(&root, "nope/x", true), root.join("nope/x"));
        let _ = std::fs::remove_dir_all(root);
    }
}
