//! What an install, update or repair has to do: which files are new or
//! changed (against the installed build's manifest and the resume file of
//! an interrupted run), which chunks to download in which order, what to
//! reuse from the old files, and how much memory and disk that needs. Port
//! of `DLManager.run_analysis` in legendary's `downloader/mp/manager.py`,
//! minus install tags, prefix filters and the reordering optimisation.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use super::super::manifest::{FileManifest, Manifest};

/// One step for the writer, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Task {
    /// Start writing `name` (`.tmp` next to it when parts of the old file
    /// are reused).
    Open {
        name: String,
        tmp: bool,
    },
    /// Append `size` bytes from `offset` of the chunk the download numbered
    /// `dl_index` brings. `cleanup`: the chunk is not needed again afterwards.
    Download {
        guid: [u32; 4],
        offset: u32,
        size: u32,
        dl_index: usize,
        cleanup: bool,
    },
    /// Append `size` bytes read from `old_offset` of the old file `old_name`
    /// (a part of it the new build reuses).
    Reuse {
        size: u32,
        old_name: String,
        old_offset: u64,
    },
    Close {
        name: String,
        tmp: bool,
    },
    /// Replace `name` with its `.tmp`.
    Rename {
        name: String,
    },
    Delete {
        name: String,
    },
    Empty {
        name: String,
    },
    MakeExecutable {
        name: String,
    },
}

#[derive(Debug, Default)]
pub struct Plan {
    pub tasks: Vec<Task>,
    /// Chunks to download, in the order the writer needs them; a chunk that
    /// could not stay cached appears again before its next use.
    pub dl_order: Vec<[u32; 4]>,
    /// Compressed bytes to download.
    pub dl_size: u64,
    pub uncompressed_dl_size: u64,
    /// Size of the whole build on disk.
    pub install_size: u64,
    /// Extra disk space needed while installing.
    pub disk_space_delta: u64,
    pub reuse_size: u64,
    pub num_chunk_tasks: usize,
    pub added: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub removed: usize,
    /// file name → SHA-1 hex, for the resume file.
    pub file_hashes: HashMap<String, String>,
}

pub struct Comparison {
    pub added: BTreeSet<String>,
    pub removed: BTreeSet<String>,
    pub changed: BTreeSet<String>,
    pub unchanged: BTreeSet<String>,
}

/// `ManifestComparison.create`: by name, changed when the hash differs.
pub fn compare(new: &Manifest, old: Option<&Manifest>) -> Comparison {
    let mut c = Comparison {
        added: BTreeSet::new(),
        removed: BTreeSet::new(),
        changed: BTreeSet::new(),
        unchanged: BTreeSet::new(),
    };
    let Some(old) = old else {
        c.added = new.files.iter().map(|f| f.filename.clone()).collect();
        return c;
    };
    let mut old_files: HashMap<&str, &[u8; 20]> = old
        .files
        .iter()
        .map(|f| (f.filename.as_str(), &f.sha))
        .collect();
    for f in &new.files {
        match old_files.remove(f.filename.as_str()) {
            Some(old_sha) if *old_sha == f.sha => c.unchanged.insert(f.filename.clone()),
            Some(_) => c.changed.insert(f.filename.clone()),
            None => c.added.insert(f.filename.clone()),
        };
    }
    c.removed = old_files.into_keys().map(str::to_owned).collect();
    c
}

fn hex(sha: &[u8; 20]) -> String {
    sha.iter().map(|b| format!("{b:02x}")).collect()
}

/// Work out the plan. `resume_file`: legendary's `<app>.resume` /
/// `<app>.repair` (`sha1:filename` per finished / verified file).
/// `cache_cap`: how many downloaded bytes may wait in memory for a later
/// use; beyond that a chunk is downloaded again when needed.
pub fn analyse(
    manifest: &Manifest,
    old: Option<&Manifest>,
    install_dir: &Path,
    resume_file: Option<&Path>,
    cache_cap: u64,
) -> Plan {
    let mut plan = Plan {
        install_size: manifest.disk_size(),
        ..Plan::default()
    };
    let biggest_chunk = manifest
        .chunks
        .iter()
        .map(|c| c.window_size as u64)
        .max()
        .unwrap_or(1024 * 1024);
    let mut mc = compare(manifest, old);
    let by_name: HashMap<&str, &FileManifest> = manifest
        .files
        .iter()
        .map(|f| (f.filename.as_str(), f))
        .collect();

    // an interrupted run: files it finished (and that still match) are done
    let mut resumed = false;
    if let Some(text) = resume_file.and_then(|p| std::fs::read_to_string(p).ok()) {
        resumed = true;
        let (mut missing, mut mismatch) = (0, 0);
        let mut completed: Vec<String> = Vec::new();
        for line in text.lines() {
            let Some((hash, name)) = line.trim().split_once(':') else {
                continue;
            };
            // the resume file is a plain text file: a name must stay inside the folder
            if !crate::stores::is_confined_rel_path(name) || !by_name.contains_key(name) {
                mismatch += 1;
            } else if !install_dir.join(name).exists() {
                missing += 1;
            } else if by_name.get(name).map(|f| hex(&f.sha)) != Some(hash.to_string()) {
                mismatch += 1;
            } else {
                completed.push(name.to_string());
            }
        }
        if missing > 0 || mismatch > 0 {
            tracing::warn!(
                missing,
                mismatch,
                "previously finished files gone or changed; downloading them again"
            );
        }
        tracing::info!(
            count = completed.len(),
            "files skipped based on resume data"
        );
        for name in completed {
            mc.added.remove(&name);
            mc.changed.remove(&name);
            mc.unchanged.insert(name);
        }
    }
    if !resumed {
        // files missing on disk are installed whatever the old manifest says
        let missing: Vec<String> = manifest
            .files
            .iter()
            .filter(|f| !mc.added.contains(&f.filename) && !install_dir.join(&f.filename).exists())
            .map(|f| f.filename.clone())
            .collect();
        if !missing.is_empty() {
            tracing::info!(count = missing.len(), "files missing on disk");
        }
        for name in missing {
            mc.changed.remove(&name);
            mc.unchanged.remove(&name);
            mc.added.insert(name);
        }
    }
    plan.added = mc.added.len();
    plan.changed = mc.changed.len();
    plan.unchanged = mc.unchanged.len();
    plan.removed = mc.removed.len();

    // how often each chunk is needed, and the disk space the run takes
    let mut files: Vec<&FileManifest> = manifest.files.iter().collect();
    files.sort_by_key(|f| f.filename.to_lowercase());
    let mut references: HashMap<[u32; 4], i64> = HashMap::new();
    let old_sizes: HashMap<&str, u64> = old
        .map(|o| {
            o.files
                .iter()
                .map(|f| (f.filename.as_str(), f.file_size))
                .collect()
        })
        .unwrap_or_default();
    let mut tmp_size: i64 = 0;
    for f in &files {
        plan.file_hashes.insert(f.filename.clone(), hex(&f.sha));
        if mc.unchanged.contains(&f.filename) {
            continue;
        }
        for p in &f.chunk_parts {
            *references.entry(p.guid).or_default() += 1;
        }
        tmp_size += f.file_size as i64;
        plan.disk_space_delta = plan.disk_space_delta.max(tmp_size.max(0) as u64);
        if mc.changed.contains(&f.filename) {
            // the old file goes once the new one is written
            tmp_size -= old_sizes.get(f.filename.as_str()).copied().unwrap_or(0) as i64;
        }
    }

    // parts of changed files that the old file already holds:
    // file → (guid, chunk offset, size) → offset in the old file
    type Reusable<'a> = HashMap<&'a str, HashMap<([u32; 4], u32, u32), u64>>;
    let mut reusable: Reusable = HashMap::new();
    if let Some(old) = old {
        let old_by_name: HashMap<&str, &FileManifest> =
            old.files.iter().map(|f| (f.filename.as_str(), f)).collect();
        for name in &mc.changed {
            let (Some(old_file), Some(new_file)) =
                (old_by_name.get(name.as_str()), by_name.get(name.as_str()))
            else {
                continue;
            };
            // spans in u64: the parser checks `offset + size` fits the chunk, but
            // the sums here must not overflow for any input
            let mut existing: HashMap<[u32; 4], Vec<(u64, u64, u64)>> = HashMap::new();
            let mut off = 0u64;
            for p in &old_file.chunk_parts {
                existing.entry(p.guid).or_default().push((
                    off,
                    p.offset as u64,
                    p.offset as u64 + p.size as u64,
                ));
                off += p.size as u64;
            }
            for p in &new_file.chunk_parts {
                let (start, end) = (p.offset as u64, p.offset as u64 + p.size as u64);
                if let Some(spans) = existing.get(&p.guid)
                    && let Some((file_off, cp_off, _)) =
                        spans.iter().find(|(_, s, e)| *s <= start && end <= *e)
                {
                    *references.entry(p.guid).or_default() -= 1;
                    reusable
                        .entry(name.as_str())
                        .or_default()
                        .insert((p.guid, p.offset, p.size), file_off + (start - cp_off));
                    plan.reuse_size += p.size as u64;
                }
            }
        }
    }

    // the tasks, simulating the chunk cache on the way
    let chunk_sizes: HashMap<[u32; 4], (u64, u64)> = manifest
        .chunks
        .iter()
        .map(|c| (c.guid, (c.file_size.max(0) as u64, c.window_size as u64)))
        .collect();
    let mut cached: HashMap<[u32; 4], usize> = HashMap::new(); // guid → dl_index holding it
    let mut cache_bytes: u64 = 0;
    for f in &files {
        if mc.unchanged.contains(&f.filename) {
            continue;
        }
        if f.chunk_parts.is_empty() {
            plan.tasks.push(Task::Empty {
                name: f.filename.clone(),
            });
            continue;
        }
        let reuse = reusable.get(f.filename.as_str());
        let mut chunk_tasks = Vec::with_capacity(f.chunk_parts.len());
        let mut reused = 0;
        for p in &f.chunk_parts {
            if let Some(file_off) = reuse.and_then(|r| r.get(&(p.guid, p.offset, p.size))) {
                reused += 1;
                chunk_tasks.push(Task::Reuse {
                    size: p.size,
                    old_name: f.filename.clone(),
                    old_offset: *file_off,
                });
                continue;
            }
            let refs = references.entry(p.guid).or_default();
            *refs -= 1;
            let (dl_index, was_cached) = match cached.get(&p.guid) {
                Some(i) => (*i, true),
                None => {
                    plan.dl_order.push(p.guid);
                    (plan.dl_order.len() - 1, false)
                }
            };
            let size = chunk_sizes
                .get(&p.guid)
                .map(|s| s.1)
                .unwrap_or(biggest_chunk);
            let cleanup = if *refs <= 0 {
                if was_cached {
                    cached.remove(&p.guid);
                    cache_bytes -= size;
                }
                true
            } else if was_cached {
                false
            } else if cache_bytes + size <= cache_cap {
                cached.insert(p.guid, dl_index);
                cache_bytes += size;
                false
            } else {
                // no room to keep it: fetched again when next needed
                true
            };
            chunk_tasks.push(Task::Download {
                guid: p.guid,
                offset: p.offset,
                size: p.size,
                dl_index,
                cleanup,
            });
        }
        plan.num_chunk_tasks += chunk_tasks.len();
        let tmp = reused > 0;
        plan.tasks.push(Task::Open {
            name: f.filename.clone(),
            tmp,
        });
        plan.tasks.extend(chunk_tasks);
        plan.tasks.push(Task::Close {
            name: f.filename.clone(),
            tmp,
        });
        if tmp {
            plan.tasks.push(Task::Rename {
                name: f.filename.clone(),
            });
        }
        if f.flags & 0x4 != 0 {
            plan.tasks.push(Task::MakeExecutable {
                name: f.filename.clone(),
            });
        }
    }
    for name in &mc.removed {
        plan.tasks.push(Task::Delete { name: name.clone() });
    }
    for guid in &plan.dl_order {
        if let Some((compressed, window)) = chunk_sizes.get(guid) {
            plan.dl_size += compressed;
            plan.uncompressed_dl_size += window;
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::super::super::manifest::{ChunkInfo, ChunkPart, Meta};
    use super::*;

    fn chunk(n: u32) -> ChunkInfo {
        ChunkInfo {
            guid: [n, 0, 0, 0],
            hash: n as u64,
            sha: [n as u8; 20],
            group_num: 1,
            window_size: 1 << 20,
            file_size: 1000,
            ..ChunkInfo::default()
        }
    }
    fn file(name: &str, sha: u8, parts: &[(u32, u32, u32)]) -> FileManifest {
        let mut off = 0;
        let chunk_parts = parts
            .iter()
            .map(|(g, o, s)| {
                let p = ChunkPart {
                    guid: [*g, 0, 0, 0],
                    offset: *o,
                    size: *s,
                    file_offset: off,
                };
                off += *s as u64;
                p
            })
            .collect();
        FileManifest {
            filename: name.into(),
            sha: [sha; 20],
            chunk_parts,
            file_size: off,
            ..FileManifest::default()
        }
    }
    fn manifest(files: Vec<FileManifest>, chunks: Vec<ChunkInfo>) -> Manifest {
        Manifest {
            version: 18,
            meta: Meta::default(),
            chunks,
            files,
            ..Manifest::default()
        }
    }

    #[test]
    fn fresh_install_downloads_every_chunk_once_and_caches_shared_ones() {
        let m = manifest(
            vec![
                file("a.bin", 1, &[(1, 0, 100), (2, 0, 100)]),
                file("b.bin", 2, &[(2, 100, 50), (3, 0, 10)]),
                file("empty", 3, &[]),
            ],
            vec![chunk(1), chunk(2), chunk(3)],
        );
        let dir = std::env::temp_dir().join("hyper-plan-none");
        let p = analyse(&m, None, &dir, None, 1 << 30);
        assert_eq!(
            p.dl_order,
            [[1, 0, 0, 0], [2, 0, 0, 0], [3, 0, 0, 0]],
            "chunk 2 downloaded once, kept for b.bin"
        );
        assert_eq!(p.dl_size, 3000);
        assert_eq!(p.install_size, 260);
        assert_eq!(p.added, 3);
        let cleanups: Vec<bool> = p
            .tasks
            .iter()
            .filter_map(|t| {
                if let Task::Download { cleanup, .. } = t {
                    Some(*cleanup)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            cleanups,
            [true, false, true, true],
            "chunk 2 stays after a.bin, goes after b.bin"
        );
        assert!(p.tasks.contains(&Task::Empty {
            name: "empty".into()
        }));
        assert_eq!(p.num_chunk_tasks, 4);
    }

    #[test]
    fn no_cache_room_means_downloading_again() {
        let m = manifest(
            vec![
                file("a.bin", 1, &[(1, 0, 100)]),
                file("b.bin", 2, &[(1, 100, 100)]),
            ],
            vec![chunk(1)],
        );
        let dir = std::env::temp_dir().join("hyper-plan-nocache");
        let p = analyse(&m, None, &dir, None, 0);
        assert_eq!(p.dl_order.len(), 2, "fetched for each use");
        assert_eq!(p.dl_size, 2000);
    }

    #[test]
    fn update_reuses_old_file_parts_and_removes_old_files() {
        let old = manifest(
            vec![
                file("a.bin", 1, &[(1, 0, 100), (2, 0, 100)]),
                file("gone.bin", 9, &[(9, 0, 5)]),
            ],
            vec![chunk(1), chunk(2), chunk(9)],
        );
        let new = manifest(
            vec![
                file("a.bin", 5, &[(1, 0, 100), (4, 0, 100)]),
                file("same.bin", 7, &[(7, 0, 1)]),
            ],
            vec![chunk(1), chunk(4), chunk(7)],
        );
        let dir = std::env::temp_dir().join(format!("hyper-plan-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.bin"), vec![0; 200]).unwrap();
        std::fs::write(dir.join("same.bin"), vec![0; 1]).unwrap();
        let mut old_same = old;
        old_same.files.push(file("same.bin", 7, &[(7, 0, 1)]));
        let p = analyse(&new, Some(&old_same), &dir, None, 1 << 30);
        assert_eq!(p.changed, 1);
        assert_eq!(p.unchanged, 1);
        assert_eq!(p.removed, 1);
        assert_eq!(
            p.dl_order,
            [[4, 0, 0, 0]],
            "only the new chunk; chunk 1 comes from the old file"
        );
        assert_eq!(p.reuse_size, 100);
        assert!(p.tasks.contains(&Task::Open {
            name: "a.bin".into(),
            tmp: true
        }));
        assert!(p.tasks.contains(&Task::Rename {
            name: "a.bin".into()
        }));
        assert!(p.tasks.contains(&Task::Delete {
            name: "gone.bin".into()
        }));
        assert!(
            matches!(p.tasks.iter().find(|t| matches!(t, Task::Reuse { .. })), Some(Task::Reuse { old_name, old_offset: 0, size: 100 }) if old_name == "a.bin")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn resume_lines_that_leave_the_folder_are_ignored() {
        let m = manifest(vec![file("a.bin", 1, &[(1, 0, 100)])], vec![chunk(1)]);
        let dir =
            std::env::temp_dir().join(format!("hyper-plan-resume-escape-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let resume = dir.join("x.resume");
        std::fs::write(
            &resume,
            format!(
                "{}:../a.bin\n{}:C:/a.bin\n",
                "01".repeat(20),
                "01".repeat(20)
            ),
        )
        .unwrap();
        let p = analyse(&m, None, &dir, Some(&resume), 1 << 30);
        assert_eq!(p.dl_order, [[1, 0, 0, 0]], "nothing counts as done");
        assert_eq!(p.unchanged, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn resume_file_skips_finished_files() {
        let m = manifest(
            vec![
                file("a.bin", 1, &[(1, 0, 100)]),
                file("b.bin", 2, &[(2, 0, 100)]),
            ],
            vec![chunk(1), chunk(2)],
        );
        let dir = std::env::temp_dir().join(format!("hyper-plan-resume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.bin"), vec![0; 100]).unwrap();
        let resume = dir.join("x.resume");
        std::fs::write(
            &resume,
            format!("{}:a.bin\n{}:b.bin\n", "01".repeat(20), "02".repeat(20)),
        )
        .unwrap();
        let p = analyse(&m, None, &dir, Some(&resume), 1 << 30);
        assert_eq!(
            p.dl_order,
            [[2, 0, 0, 0]],
            "a.bin done and present; b.bin listed but missing"
        );
        assert_eq!(p.unchanged, 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
