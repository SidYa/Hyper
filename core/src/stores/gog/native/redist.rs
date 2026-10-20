//! Galaxy's shared redistributables (`__redist`: DirectX, Visual C++
//! runtimes, the setup-script interpreter ISI…): installed once, into one
//! folder, for every game that needs them. Port of gogdl's `redist` command
//! (`dl/managers/dependencies.py` for the `__redist` depots) and Heroic's
//! `redist.ts`. Redistributables that live in the game folder are part of
//! the game's own download (`download::run`).
//!
//! Files on disk are gogdl's: `.gogdl-redist-manifest` in the folder is the
//! repository as GOG sent it plus `HGLInstalled`, the ids installed here.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use serde_json::Value;

use super::Client;
use super::download::{self, Links, ProgressFn, RESUME_FILE};
use super::manifest::{Diff, GameFile};
use crate::error::{Error, Result};
use crate::stores::epic::download::CancelFlag;

pub const MANIFEST_FILE: &str = ".gogdl-redist-manifest";
/// GOG's setup-script interpreter: always installed (Heroic does the same),
/// the v2 setup scripts need it.
pub const ISI: &str = "ISI";

/// One redistributable of the repository (`depots[]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub readable_name: String,
    /// Installer path below the redist folder (`__redist/DirectX/DXSETUP.exe`);
    /// empty when nothing is to be run.
    pub exe_path: String,
    pub exe_args: String,
    /// Installed system-wide (`__redist/…`), not into the game folder.
    pub system: bool,
}

pub fn entries(repo: &Value) -> Vec<Entry> {
    repo.get("depots")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|d| {
                    let path = d
                        .pointer("/executable/path")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Some(Entry {
                        id: d.get("dependencyId")?.as_str()?.to_string(),
                        readable_name: d
                            .get("readableName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        exe_path: path.to_string(),
                        exe_args: d
                            .pointer("/executable/arguments")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        system: path.starts_with("__redist"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn entry(repo: &Value, id: &str) -> Option<Entry> {
    entries(repo).into_iter().find(|e| e.id == id)
}

/// Dependency ids of every game installed here (their saved manifests),
/// plus ISI (Heroic's `getRequiredRedistList`).
pub fn required_ids(manifests_dir: &Path) -> Vec<String> {
    let mut out = vec![ISI.to_string()];
    let Ok(entries) = std::fs::read_dir(manifests_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        for id in manifest_dependencies(&v) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// The redistributables a saved game manifest asks for (v1: `redist`
/// depots; v2: `dependencies`).
pub fn manifest_dependencies(manifest: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if manifest.get("version").and_then(Value::as_u64) == Some(1) {
        for depot in manifest
            .pointer("/product/depots")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = depot.get("redist").and_then(Value::as_str)
                && !out.iter().any(|x| x == id)
            {
                out.push(id.to_string());
            }
        }
    } else {
        for id in manifest
            .get("dependencies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !out.iter().any(|x| x == id) {
                out.push(id.to_string());
            }
        }
    }
    out
}

pub fn installed_manifest(dir: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(MANIFEST_FILE)).ok()?).ok()
}

/// Download or update the system-wide redistributables `ids` into `dir`
/// (`DependenciesManager.get`). Nothing to do is cheap: the file lists are
/// compared, only what changed is fetched.
pub async fn ensure(
    client: &Arc<Client>,
    ids: &[String],
    dir: &Path,
    max_workers: usize,
    on_progress: ProgressFn,
    cancel: Arc<CancelFlag>,
) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let repo = client.dependencies_repo().await?;
    let wanted: Vec<String> = entries(&repo)
        .into_iter()
        .filter(|e| e.system && ids.contains(&e.id))
        .map(|e| e.id)
        .collect();

    // what an earlier run installed (its own repository copy lists the files)
    let existing = installed_manifest(dir);
    let old_ids: Vec<String> = existing
        .as_ref()
        .and_then(|m| m.get("HGLInstalled"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let new_files = download::redist_files(client, &repo, &wanted, false).await?;
    let old_files = match &existing {
        Some(m) if !old_ids.is_empty() => download::redist_files(client, m, &old_ids, false)
            .await
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let diff = compare(new_files, &old_files);
    tracing::info!(wanted = ?wanted, diff = diff.summary(), "GOG redistributables");

    let write_manifest = || -> Result<()> {
        let mut m = repo.clone();
        m["HGLInstalled"] = wanted
            .iter()
            .map(|s| Value::String(s.clone()))
            .collect::<Vec<_>>()
            .into();
        std::fs::write(dir.join(MANIFEST_FILE), m.to_string())?;
        Ok(())
    };
    if diff.is_empty() {
        return write_manifest();
    }

    let support_dir = dir.join("gog-support");
    let resume_file = dir.join(RESUME_FILE);
    let plan = download::plan::analyse(
        &diff,
        dir,
        &support_dir,
        &resume_file,
        download::MEMORY_CAP / 2,
    )?;
    if let Some((free, _)) = crate::browse::free_space(dir)
        && free < plan.disk_delta
    {
        return Err(Error::Other(format!(
            "not enough disk space for the redistributables: {:.2} GiB free, {:.2} GiB needed",
            free as f64 / (1u64 << 30) as f64,
            plan.disk_delta as f64 / (1u64 << 30) as f64
        )));
    }
    let links = Arc::new(Links::new(client.clone(), 2, String::new()));
    links.refresh("redist").await?;
    let workers = if max_workers > 0 {
        max_workers.min(32)
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(16)
    };
    download::execute(
        links,
        plan,
        dir,
        &support_dir,
        &resume_file,
        workers,
        on_progress,
        cancel,
    )
    .await?;
    let _ = std::fs::remove_file(&resume_file);
    let _ = std::fs::remove_dir_all(dir.join(download::LEFTOVER_CACHE_DIR));
    write_manifest()?;
    tracing::info!(dir = %dir.display(), "GOG redistributables up to date");
    Ok(())
}

/// gogdl's `DependenciesDiff.compare`: by path (case-insensitive); a
/// changed file is fetched whole again.
pub fn compare(new_files: Vec<GameFile>, old_files: &[GameFile]) -> Diff {
    let mut diff = Diff::default();
    if old_files.is_empty() {
        diff.new = new_files;
        return diff;
    }
    let old: HashMap<String, &GameFile> = old_files
        .iter()
        .map(|f| (f.path.to_lowercase(), f))
        .collect();
    let new_paths: HashSet<String> = new_files.iter().map(|f| f.path.to_lowercase()).collect();
    for (path, f) in &old {
        if !new_paths.contains(path) {
            diff.deleted.push((*f).clone());
        }
    }
    for f in new_files {
        match old.get(&f.path.to_lowercase()) {
            None => diff.new.push(f),
            Some(o) => {
                let changed = if f.chunks.len() == 1 && o.chunks.len() == 1 {
                    f.chunks[0].md5 != o.chunks[0].md5
                } else {
                    (f.md5.is_some() && o.md5.is_some() && f.md5 != o.md5)
                        || (f.sha256.is_some() && f.sha256 != o.sha256)
                        || f.chunks.len() != o.chunks.len()
                };
                if changed {
                    diff.new.push(f);
                }
            }
        }
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stores::gog::native::manifest::ChunkRef;

    fn repo() -> Value {
        serde_json::json!({
            "build_id": "b1",
            "depots": [
                { "dependencyId": "ISI", "readableName": "Script interpreter", "executable": { "path": "__redist/ISI/scriptinterpreter.exe", "arguments": "" }, "manifest": "m1" },
                { "dependencyId": "DirectX", "readableName": "DirectX", "executable": { "path": "__redist/DirectX/DXSETUP.exe", "arguments": "/silent" }, "manifest": "m2" },
                { "dependencyId": "scummvm", "readableName": "ScummVM", "executable": { "path": "scummvm.exe", "arguments": "" }, "manifest": "m3" },
                { "dependencyId": "PHYSXLEGACY", "readableName": "PhysX", "executable": { "path": "__redist/PhysX/PhysX-9.13.msi", "arguments": "" }, "manifest": "m4" }
            ]
        })
    }

    #[test]
    fn reads_the_repository() {
        let e = entries(&repo());
        assert_eq!(e.len(), 4);
        assert!(e[0].system && e[1].system && !e[2].system);
        assert_eq!(
            entry(&repo(), "DirectX").map(|e| e.exe_args),
            Some("/silent".to_string())
        );
        assert_eq!(entry(&repo(), "nope"), None);
    }

    #[test]
    fn collects_required_ids_from_saved_manifests() {
        let dir = std::env::temp_dir().join(format!("hyper-redist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("1"),
            r#"{ "version": 2, "dependencies": ["DirectX", "MSVC2019"] }"#,
        )
        .expect("v2");
        std::fs::write(dir.join("2"), r#"{ "version": 1, "product": { "depots": [ { "redist": "DirectX" }, { "languages": ["English"], "manifest": "x" }, { "redist": "scummvm" } ] } }"#).expect("v1");
        std::fs::write(dir.join("3"), "not json").expect("junk");
        assert_eq!(
            required_ids(&dir),
            ["ISI", "DirectX", "MSVC2019", "scummvm"]
        );
        assert_eq!(required_ids(&dir.join("missing")), ["ISI"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn file(path: &str, md5: &str) -> GameFile {
        GameFile {
            product: "redist".into(),
            path: path.into(),
            size: 1,
            md5: Some(md5.into()),
            chunks: vec![ChunkRef {
                compressed_md5: format!("c{md5}"),
                md5: md5.into(),
                size: 1,
                compressed_size: 1,
            }],
            ..GameFile::default()
        }
    }

    #[test]
    fn compares_like_gogdl() {
        let old = vec![
            file("__redist/DirectX/a.dll", "1"),
            file("__redist/DirectX/gone.dll", "2"),
            file("__redist/ISI/isi.exe", "3"),
        ];
        let new = vec![
            file("__redist/directx/A.dll", "1"),
            file("__redist/DirectX/new.dll", "4"),
            file("__redist/ISI/isi.exe", "9"),
        ];
        let d = compare(new, &old);
        let names = |v: &[GameFile]| v.iter().map(|f| f.path.clone()).collect::<Vec<_>>();
        assert_eq!(
            names(&d.new),
            ["__redist/DirectX/new.dll", "__redist/ISI/isi.exe"]
        );
        assert_eq!(names(&d.deleted), ["__redist/DirectX/gone.dll"]);
        let fresh = compare(vec![file("x", "1")], &[]);
        assert_eq!(fresh.new.len(), 1);
        assert!(fresh.deleted.is_empty());
    }
}
