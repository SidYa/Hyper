//! From a manifest diff to the writer's task list and the download order
//! (gogdl's `ExecutingManager.setup`). Shared chunks are fetched once and
//! kept in memory until their last use while that fits the cache cap;
//! generation-1 files are fetched as byte ranges of `main.bin` in pieces of
//! `biggest_chunk`. The resume file (`.gogdl-resume` in the game folder,
//! `hash:support:path` per finished file) lets an interrupted run skip what
//! it already wrote.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::super::manifest::{Changed, Diff, GameFile};
use crate::error::{Error, Result};
use crate::stores::is_confined_rel_path;

/// v1 files are fetched in pieces of the build's biggest chunk, within
/// these bounds (each piece is held in memory whole).
const MIN_PIECE: u64 = 10 << 20;
const MAX_PIECE: u64 = 64 << 20;
/// No single download may take more than half the memory cap: the
/// dispatcher could never fit it and would wait forever.
const MAX_DOWNLOAD: u64 = super::MEMORY_CAP / 2;

/// One fetch from the CDN.
#[derive(Debug, Clone)]
pub struct Download {
    /// Product id, or `redist` for the shared redistributables store.
    pub product: String,
    /// v2: the chunk's `compressedMd5`; v1: `<file hash>_<piece>`.
    pub key: String,
    /// v2: MD5 of the decompressed chunk.
    pub md5: Option<String>,
    /// Bytes after decompression.
    pub size: u64,
    pub compressed_size: u64,
    /// v1: (offset, size) in the depot's `main.bin`.
    pub v1_range: Option<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Task {
    /// Start writing `path` (`.tmp` next to it when parts are copied from
    /// the old file of the same name).
    Open {
        path: String,
        support: bool,
        tmp: bool,
    },
    /// Append download `dl_index`; `cleanup`: not needed again afterwards.
    Chunk {
        dl_index: usize,
        cleanup: bool,
    },
    /// Append `size` bytes at `offset` of an existing file.
    Copy {
        from: String,
        from_support: bool,
        offset: u64,
        size: u64,
    },
    Close {
        path: String,
        support: bool,
        tmp: bool,
    },
    /// Replace `path` with its `.tmp`.
    Rename {
        path: String,
        support: bool,
    },
    Delete {
        path: String,
        support: bool,
    },
    Empty {
        path: String,
        support: bool,
    },
    /// A whole file identical to one written earlier (v1).
    CopyFile {
        path: String,
        support: bool,
        from: String,
        from_support: bool,
    },
    MakeExecutable {
        path: String,
        support: bool,
    },
    Symlink {
        path: String,
        target: String,
    },
}

#[derive(Debug, Default)]
pub struct Plan {
    pub tasks: Vec<Task>,
    /// In the order the writer needs them.
    pub downloads: Vec<Download>,
    /// Compressed bytes to fetch.
    pub download_size: u64,
    /// Bytes to write.
    pub disk_size: u64,
    /// Extra disk space the run needs at its peak.
    pub disk_delta: u64,
    /// lower-cased path → (checksum, support), for the resume file.
    pub hashes: HashMap<String, (String, bool)>,
    pub resumed: usize,
}

/// `.gogdl-resume` lines; paths are compared lower-cased with `/`. A line
/// whose path would leave the folder is ignored (the file is in the game
/// folder, so anything could have written it).
fn read_resume(
    resume_file: &Path,
    game_dir: &Path,
    support_dir: &Path,
    hashes: &HashMap<String, (String, bool)>,
) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
    let (mut completed, mut missing, mut mismatched) =
        (HashSet::new(), HashSet::new(), HashSet::new());
    let Ok(text) = std::fs::read_to_string(resume_file) else {
        return (completed, missing, mismatched);
    };
    for line in text.lines() {
        let mut parts = line.trim().splitn(3, ':');
        let (Some(hash), Some(support), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if !is_confined_rel_path(path) {
            tracing::warn!(line, "resume file line ignored");
            continue;
        }
        let key = path.replace('\\', "/").to_lowercase();
        let root = if support == "support" {
            support_dir
        } else {
            game_dir
        };
        let exists = |rel: &str| super::target(root, rel, false).is_ok_and(|p| p.exists());
        if !exists(&key) && !exists(path) {
            missing.insert(key);
        } else if hashes.get(&key).map(|h| h.0.as_str()) != Some(hash) {
            mismatched.insert(key);
        } else {
            completed.insert(key);
        }
    }
    if !missing.is_empty() || !mismatched.is_empty() {
        tracing::warn!(
            missing = missing.len(),
            mismatched = mismatched.len(),
            "previously finished files gone or changed; downloading them again"
        );
    }
    (completed, missing, mismatched)
}

struct Builder<'a> {
    plan: Plan,
    cache_cap: u64,
    biggest_chunk: u64,
    /// how often each v2 chunk is still needed
    refs: HashMap<&'a str, i64>,
    /// chunk → download index holding it in memory
    cached: HashMap<&'a str, usize>,
    cache_bytes: u64,
    /// v1 hash → (path, support) of a file written this run
    v1_written: HashMap<String, (String, bool)>,
    running: i64,
}

impl<'a> Builder<'a> {
    fn bump(&mut self, bytes: i64) {
        self.running = self.running.saturating_add(bytes);
        self.plan.disk_delta = self.plan.disk_delta.max(self.running.max(0) as u64);
    }

    /// A v2 chunk: from memory when cached, else a new download.
    fn chunk_task(&mut self, product: &str, chunk: &'a super::super::manifest::ChunkRef) -> Task {
        let refs = self.refs.entry(chunk.compressed_md5.as_str()).or_insert(1);
        *refs -= 1;
        let refs = *refs;
        let (dl_index, was_cached) = match self.cached.get(chunk.compressed_md5.as_str()) {
            Some(i) => (*i, true),
            None => {
                self.plan.downloads.push(Download {
                    product: product.to_string(),
                    key: chunk.compressed_md5.clone(),
                    md5: Some(chunk.md5.clone()),
                    size: chunk.size,
                    compressed_size: chunk.compressed_size,
                    v1_range: None,
                });
                self.plan.download_size = self
                    .plan
                    .download_size
                    .saturating_add(chunk.compressed_size);
                (self.plan.downloads.len() - 1, false)
            }
        };
        let cleanup = if refs <= 0 {
            if was_cached {
                self.cached.remove(chunk.compressed_md5.as_str());
                self.cache_bytes = self.cache_bytes.saturating_sub(chunk.size);
            }
            true
        } else if was_cached {
            false
        } else if self.cache_bytes.saturating_add(chunk.size) <= self.cache_cap {
            self.cached.insert(chunk.compressed_md5.as_str(), dl_index);
            self.cache_bytes = self.cache_bytes.saturating_add(chunk.size);
            false
        } else {
            // no room to keep it: fetched again when next needed
            true
        };
        Task::Chunk { dl_index, cleanup }
    }

    fn v2_file(&mut self, f: &'a GameFile, reuse: Option<(&[Option<u64>], bool)>) {
        let mut tasks = Vec::with_capacity(f.chunks.len() + 3);
        let mut reused = 0;
        for (i, chunk) in f.chunks.iter().enumerate() {
            if let Some((offsets, old_support)) = reuse
                && let Some(Some(off)) = offsets.get(i)
            {
                reused += 1;
                tasks.push(Task::Copy {
                    from: f.path.clone(),
                    from_support: old_support,
                    offset: *off,
                    size: chunk.size,
                });
                continue;
            }
            tasks.push(self.chunk_task(&f.product, chunk));
        }
        let tmp = reused > 0;
        self.plan.tasks.push(Task::Open {
            path: f.path.clone(),
            support: f.support,
            tmp,
        });
        self.plan.tasks.extend(tasks);
        self.plan.tasks.push(Task::Close {
            path: f.path.clone(),
            support: f.support,
            tmp,
        });
        if tmp {
            self.plan.tasks.push(Task::Rename {
                path: f.path.clone(),
                support: f.support,
            });
        }
        if f.executable {
            self.plan.tasks.push(Task::MakeExecutable {
                path: f.path.clone(),
                support: f.support,
            });
        }
        self.plan.disk_size = self.plan.disk_size.saturating_add(f.size);
        self.bump(i64::try_from(f.size).unwrap_or(i64::MAX));
    }

    fn v1_file(&mut self, f: &GameFile, offset: u64) {
        let hash = f.md5.clone().unwrap_or_default();
        if let Some((from, from_support)) = self.v1_written.get(&hash).cloned() {
            self.plan.tasks.push(Task::CopyFile {
                path: f.path.clone(),
                support: f.support,
                from,
                from_support,
            });
            if f.executable {
                self.plan.tasks.push(Task::MakeExecutable {
                    path: f.path.clone(),
                    support: f.support,
                });
            }
            self.bump(f.size as i64);
            return;
        }
        self.plan.tasks.push(Task::Open {
            path: f.path.clone(),
            support: f.support,
            tmp: false,
        });
        let mut left = f.size;
        let mut piece_offset = 0u64;
        let mut i = 0;
        while left > 0 {
            let size = left.min(self.biggest_chunk);
            self.plan.downloads.push(Download {
                product: f.product.clone(),
                key: format!("{hash}_{i}"),
                md5: None,
                size,
                compressed_size: size,
                v1_range: Some((offset + piece_offset, size)),
            });
            self.plan.tasks.push(Task::Chunk {
                dl_index: self.plan.downloads.len() - 1,
                cleanup: true,
            });
            self.plan.download_size = self.plan.download_size.saturating_add(size);
            piece_offset += size;
            left -= size;
            i += 1;
        }
        self.plan.tasks.push(Task::Close {
            path: f.path.clone(),
            support: f.support,
            tmp: false,
        });
        if f.executable {
            self.plan.tasks.push(Task::MakeExecutable {
                path: f.path.clone(),
                support: f.support,
            });
        }
        self.plan.disk_size = self.plan.disk_size.saturating_add(f.size);
        self.bump(i64::try_from(f.size).unwrap_or(i64::MAX));
        self.v1_written.insert(hash, (f.path.clone(), f.support));
    }
}

/// Every path of the diff must stay inside its folder, every chunk id must
/// mean one size (the cache accounts by id) and fit in memory.
fn check(diff: &Diff) -> Result<()> {
    let changed_files = diff.changed.iter().map(|c| &c.file);
    let files = diff
        .new
        .iter()
        .chain(changed_files)
        .chain(diff.redist.iter())
        .chain(diff.deleted.iter())
        .chain(diff.removed_redist.iter());
    let mut sizes: HashMap<&str, u64> = HashMap::new();
    for f in files {
        if !is_confined_rel_path(&f.path) {
            return Err(Error::Other(format!(
                "GOG manifest path is not confined to the game folder: {:?}",
                f.path
            )));
        }
        for c in &f.chunks {
            if c.size > MAX_DOWNLOAD {
                return Err(Error::Other(format!(
                    "GOG chunk of {:?} is {} bytes; too large to download",
                    f.path, c.size
                )));
            }
            if sizes
                .insert(c.compressed_md5.as_str(), c.size)
                .is_some_and(|seen| seen != c.size)
            {
                return Err(Error::Other(format!(
                    "GOG chunk {} is listed with two sizes",
                    c.compressed_md5
                )));
            }
        }
    }
    for l in &diff.links {
        if !is_confined_rel_path(&l.path) || !is_confined_rel_path(&l.target) {
            return Err(Error::Other(format!(
                "GOG depot link is not confined to the game folder: {:?} -> {:?}",
                l.path, l.target
            )));
        }
    }
    Ok(())
}

pub fn analyse(
    diff: &Diff,
    game_dir: &Path,
    support_dir: &Path,
    resume_file: &Path,
    cache_cap: u64,
) -> Result<Plan> {
    check(diff)?;
    let mut plan = Plan::default();
    let changed_files: Vec<&GameFile> = diff.changed.iter().map(|c| &c.file).collect();
    let to_write = diff
        .new
        .iter()
        .chain(changed_files.iter().copied())
        .chain(diff.redist.iter());
    let mut biggest_chunk = 0u64;
    for f in to_write.clone() {
        if let Some(sum) = f.checksum() {
            plan.hashes.insert(f.path.to_lowercase(), (sum, f.support));
        }
        biggest_chunk = biggest_chunk.max(f.chunks.iter().map(|c| c.size).max().unwrap_or(0));
    }
    let biggest_chunk = if biggest_chunk == 0 {
        20 << 20
    } else {
        biggest_chunk.clamp(MIN_PIECE, MAX_PIECE)
    };
    let (completed, missing, mismatched) =
        read_resume(resume_file, game_dir, support_dir, &plan.hashes);
    plan.resumed = completed.len();

    let mut b = Builder {
        plan,
        cache_cap,
        biggest_chunk,
        refs: HashMap::new(),
        cached: HashMap::new(),
        cache_bytes: 0,
        v1_written: HashMap::new(),
        running: 0,
    };
    for f in diff.deleted.iter().chain(diff.removed_redist.iter()) {
        b.plan.tasks.push(Task::Delete {
            path: f.path.clone(),
            support: f.support,
        });
        b.bump(-i64::try_from(f.size).unwrap_or(i64::MAX));
    }

    // how often each chunk is needed, counting only what will be fetched
    let is_done = |f: &GameFile| completed.contains(&f.path.to_lowercase());
    let dirty = |f: &GameFile| {
        let key = f.path.to_lowercase();
        missing.contains(&key) || mismatched.contains(&key)
    };
    for f in diff.new.iter().chain(diff.redist.iter()) {
        if is_done(f) {
            continue;
        }
        for c in &f.chunks {
            *b.refs.entry(c.compressed_md5.as_str()).or_default() += 1;
        }
    }
    for c in &diff.changed {
        if is_done(&c.file) {
            continue;
        }
        let reuse_ok = !dirty(&c.file);
        for (i, chunk) in c.file.chunks.iter().enumerate() {
            if reuse_ok && c.reuse.get(i).is_some_and(Option::is_some) {
                continue;
            }
            *b.refs.entry(chunk.compressed_md5.as_str()).or_default() += 1;
        }
    }

    // the tasks, in gogdl's order: new, changed, redistributables
    for f in diff
        .new
        .iter()
        .chain(changed_files.iter().copied())
        .chain(diff.redist.iter())
    {
        let change: Option<&Changed> = diff.changed.iter().find(|c| std::ptr::eq(&c.file, f));
        if let Some(offset) = f.v1_offset {
            if f.size == 0 {
                b.plan.tasks.push(Task::Empty {
                    path: f.path.clone(),
                    support: f.support,
                });
                continue;
            }
            if is_done(f) {
                b.v1_written.insert(
                    f.md5.clone().unwrap_or_default(),
                    (f.path.clone(), f.support),
                );
                continue;
            }
            b.v1_file(f, offset);
            continue;
        }
        if f.chunks.is_empty() {
            b.plan.tasks.push(Task::Empty {
                path: f.path.clone(),
                support: f.support,
            });
            continue;
        }
        if is_done(f) {
            continue;
        }
        let reuse = change
            .filter(|_| !dirty(f))
            .map(|c| (c.reuse.as_slice(), c.old_support));
        b.v2_file(f, reuse);
    }
    for l in &diff.links {
        b.plan.tasks.push(Task::Symlink {
            path: l.path.clone(),
            target: l.target.clone(),
        });
    }
    Ok(b.plan)
}

#[cfg(test)]
mod tests {
    use super::super::super::manifest::tests::file;
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("hyper-gog-plan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn shared_chunks_are_fetched_once_and_kept_until_their_last_use() {
        let diff = Diff {
            new: vec![
                file("a.bin", &[("x", 100), ("y", 100)]),
                file("b.bin", &[("y", 100), ("z", 10)]),
                file("empty", &[]),
            ],
            ..Diff::default()
        };
        let d = dir("shared");
        let p = analyse(
            &diff,
            &d,
            &d.join("support"),
            &d.join(".gogdl-resume"),
            1 << 30,
        )
        .unwrap();
        assert_eq!(
            p.downloads
                .iter()
                .map(|x| x.key.as_str())
                .collect::<Vec<_>>(),
            ["cx", "cy", "cz"]
        );
        assert_eq!(p.download_size, 50 + 50 + 5);
        assert_eq!(p.disk_size, 310);
        let cleanups: Vec<bool> = p
            .tasks
            .iter()
            .filter_map(|t| {
                if let Task::Chunk { cleanup, .. } = t {
                    Some(*cleanup)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(cleanups, [true, false, true, true]);
        assert!(p.tasks.contains(&Task::Empty {
            path: "empty".into(),
            support: false
        }));
        assert_eq!(p.hashes["a.bin"].0, "x");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn changed_files_copy_reusable_chunks_into_a_tmp_file() {
        let changed = Changed {
            file: file("a.bin", &[("x", 10), ("n", 10), ("y", 10)]),
            old_support: false,
            reuse: vec![Some(0), None, Some(10)],
        };
        let diff = Diff {
            changed: vec![changed],
            ..Diff::default()
        };
        let d = dir("changed");
        let p = analyse(
            &diff,
            &d,
            &d.join("support"),
            &d.join(".gogdl-resume"),
            1 << 30,
        )
        .unwrap();
        assert_eq!(p.downloads.len(), 1);
        assert_eq!(p.downloads[0].key, "cn");
        assert_eq!(
            p.tasks[0],
            Task::Open {
                path: "a.bin".into(),
                support: false,
                tmp: true
            }
        );
        assert!(matches!(
            p.tasks[1],
            Task::Copy {
                offset: 0,
                size: 10,
                ..
            }
        ));
        assert!(matches!(
            p.tasks[2],
            Task::Chunk {
                dl_index: 0,
                cleanup: true
            }
        ));
        assert!(matches!(
            p.tasks[3],
            Task::Copy {
                offset: 10,
                size: 10,
                ..
            }
        ));
        assert_eq!(
            p.tasks[5],
            Task::Rename {
                path: "a.bin".into(),
                support: false
            }
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn resume_file_skips_finished_files() {
        let d = dir("resume");
        std::fs::write(d.join("a.bin"), b"done").unwrap();
        std::fs::write(
            d.join(".gogdl-resume"),
            "x::a.bin\ny::gone.bin\nwrong::b.bin\n",
        )
        .unwrap();
        std::fs::write(d.join("b.bin"), b"changed").unwrap();
        let diff = Diff {
            new: vec![
                file("a.bin", &[("x", 1)]),
                file("b.bin", &[("b", 1)]),
                file("gone.bin", &[("y", 1)]),
            ],
            ..Diff::default()
        };
        let p = analyse(
            &diff,
            &d,
            &d.join("support"),
            &d.join(".gogdl-resume"),
            1 << 30,
        )
        .unwrap();
        assert_eq!(p.resumed, 1);
        assert_eq!(
            p.downloads
                .iter()
                .map(|x| x.key.as_str())
                .collect::<Vec<_>>(),
            ["cb", "cy"]
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn resume_lines_outside_the_folder_are_ignored() {
        let d = dir("resume-escape");
        std::fs::write(d.join("a.bin"), b"done").unwrap();
        std::fs::write(
            d.join(".gogdl-resume"),
            "x::../../a.bin\nx::C:\\a.bin\nx::a.bin\n",
        )
        .unwrap();
        let diff = Diff {
            new: vec![file("a.bin", &[("x", 1)])],
            ..Diff::default()
        };
        let p = analyse(
            &diff,
            &d,
            &d.join("support"),
            &d.join(".gogdl-resume"),
            1 << 30,
        )
        .unwrap();
        assert_eq!(p.resumed, 1, "only the confined line counts");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn unsafe_or_oversized_diffs_are_refused() {
        let d = dir("refused");
        let plan = |diff: &Diff| {
            analyse(
                diff,
                &d,
                &d.join("support"),
                &d.join(".gogdl-resume"),
                1 << 30,
            )
        };
        assert!(
            plan(&Diff {
                new: vec![file("../evil.bin", &[("x", 1)])],
                ..Diff::default()
            })
            .is_err()
        );
        assert!(
            plan(&Diff {
                deleted: vec![file("..\\..\\evil.bin", &[("x", 1)])],
                ..Diff::default()
            })
            .is_err()
        );
        assert!(
            plan(&Diff {
                links: vec![super::super::super::manifest::Link {
                    path: "l".into(),
                    target: "/etc/passwd".into()
                }],
                ..Diff::default()
            })
            .is_err()
        );
        // one chunk id, two sizes across files
        assert!(
            plan(&Diff {
                new: vec![file("a", &[("x", 1)]), file("b", &[("x", 2)])],
                ..Diff::default()
            })
            .is_err()
        );
        // a chunk the dispatcher could never fit in memory
        assert!(
            plan(&Diff {
                new: vec![file("huge", &[("h", MAX_DOWNLOAD + 1)])],
                ..Diff::default()
            })
            .is_err()
        );
        assert!(
            plan(&Diff {
                new: vec![file("ok", &[("h", MAX_DOWNLOAD)])],
                ..Diff::default()
            })
            .is_ok()
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn v1_pieces_are_capped() {
        let big = GameFile {
            product: "100".into(),
            path: "data.bin".into(),
            size: 100 << 20,
            md5: Some("h1".into()),
            v1_offset: Some(0),
            ..GameFile::default()
        };
        // a v2 chunk of 200 MiB in the same build would make the pieces 200 MiB; they stay at 64
        let v2 = file("x.bin", &[("x", 200 << 20)]);
        let diff = Diff {
            new: vec![big, v2],
            ..Diff::default()
        };
        let d = dir("v1-cap");
        let p = analyse(
            &diff,
            &d,
            &d.join("support"),
            &d.join(".gogdl-resume"),
            1 << 30,
        )
        .unwrap();
        let pieces: Vec<u64> = p
            .downloads
            .iter()
            .filter_map(|x| x.v1_range.map(|r| r.1))
            .collect();
        assert_eq!(pieces, [64 << 20, 36 << 20]);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn v1_files_are_split_into_pieces_and_duplicates_copied() {
        let big = GameFile {
            product: "100".into(),
            path: "data.bin".into(),
            size: 25 << 20,
            md5: Some("h1".into()),
            v1_offset: Some(1000),
            ..GameFile::default()
        };
        let twin = GameFile {
            product: "100".into(),
            path: "copy.bin".into(),
            size: 25 << 20,
            md5: Some("h1".into()),
            v1_offset: Some(5000),
            executable: true,
            ..GameFile::default()
        };
        let diff = Diff {
            new: vec![big, twin],
            ..Diff::default()
        };
        let d = dir("v1");
        let p = analyse(
            &diff,
            &d,
            &d.join("support"),
            &d.join(".gogdl-resume"),
            1 << 30,
        )
        .unwrap();
        assert_eq!(p.downloads.len(), 2, "20 MiB pieces; the twin is copied");
        assert_eq!(p.downloads[0].v1_range, Some((1000, 20 << 20)));
        assert_eq!(p.downloads[1].v1_range, Some((1000 + (20 << 20), 5 << 20)));
        assert!(p.tasks.iter().any(|t| matches!(t, Task::CopyFile { path, from, .. } if path == "copy.bin" && from == "data.bin")));
        assert!(
            p.tasks
                .iter()
                .any(|t| matches!(t, Task::MakeExecutable { path, .. } if path == "copy.bin"))
        );
        assert_eq!(p.download_size, 25 << 20);
        let _ = std::fs::remove_dir_all(d);
    }
}
