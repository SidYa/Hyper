//! Moving an installed game to another folder (Heroic's `moveInstall`,
//! which shells out to robocopy / rsync): the folder is copied with
//! progress, the store records the new place, the old folder goes. A copy
//! instead of a rename, so another drive (an SD card) works the same way.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::error::{Error, Result};
use crate::types::DownloadProgress;

/// Where the game ends up: `<new_base>/<old folder name>`. Refuses a
/// destination that exists already, the folder the game is in now, or
/// anything inside the game folder (also through links and, on Windows,
/// another spelling of the same path).
pub fn destination(old: &Path, new_base: &Path) -> Result<PathBuf> {
    if !new_base.is_absolute() || !new_base.is_dir() {
        return Err(Error::Other(format!(
            "{} is not an existing folder",
            new_base.display()
        )));
    }
    let name = old
        .file_name()
        .ok_or_else(|| Error::Other("the game folder has no name".into()))?;
    let dest = new_base.join(name);
    if dest.exists() {
        return Err(Error::Other(format!("{} exists already", dest.display())));
    }
    let parent = old.parent().unwrap_or(old);
    if overlaps(new_base, old) || same_dir(new_base, parent) {
        return Err(Error::Other("the game is there already".into()));
    }
    Ok(dest)
}

/// Comparable spelling: the resolved path where it exists, lower-cased on
/// Windows (where names are compared without case).
fn comparable(p: &Path) -> PathBuf {
    let real = super::resolve(p).unwrap_or_else(|| p.to_path_buf());
    if cfg!(windows) {
        PathBuf::from(real.to_string_lossy().to_lowercase())
    } else {
        real
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    comparable(a) == comparable(b)
}

/// One of the folders is the other or lies inside it.
fn overlaps(a: &Path, b: &Path) -> bool {
    let (a, b) = (comparable(a), comparable(b));
    a.starts_with(&b) || b.starts_with(&a)
}

/// Refuse the move when the volume of `new_base` has less than `needed`
/// bytes free (plus a little room).
pub fn check_space(new_base: &Path, needed: u64) -> Result<()> {
    const MARGIN: u64 = 64 << 20;
    let Some((free, _)) = crate::browse::free_space(new_base) else {
        return Ok(()); // unknown: let the copy itself fail
    };
    if free < needed.saturating_add(MARGIN) {
        return Err(Error::Other(format!(
            "not enough space on {}: {} needed, {} free",
            new_base.display(),
            super::human_size(needed),
            super::human_size(free)
        )));
    }
    Ok(())
}

/// Bytes of every regular file below `dir` (links are not followed).
pub fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() {
                total += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

/// Copy `from` into `to` (created), reporting bytes done every half second.
/// Blocking.
pub fn copy_dir(
    from: &Path,
    to: &Path,
    total: u64,
    progress: &(dyn Fn(DownloadProgress) + Send + Sync),
) -> std::io::Result<()> {
    let started = Instant::now();
    let mut last = Instant::now();
    let mut report = |bytes: u64, force: bool| {
        if force || last.elapsed().as_millis() >= 500 {
            last = Instant::now();
            let secs = started.elapsed().as_secs_f64().max(0.001);
            let speed = (bytes as f64 / secs) as u64;
            progress(DownloadProgress {
                percent: if total > 0 {
                    (bytes as f64 / total as f64 * 100.0).min(100.0) as f32
                } else {
                    0.0
                },
                bytes_done: bytes,
                bytes_total: total,
                speed: 0,
                disk_speed: speed,
                eta: (speed > 0 && total > bytes).then(|| (total - bytes) / speed),
            });
        }
    };
    let mut done = 0u64;
    copy_tree(from, to, &mut done, &mut report)?;
    report(done, true);
    Ok(())
}

fn copy_tree(
    from: &Path,
    to: &Path,
    done: &mut u64,
    report: &mut dyn FnMut(u64, bool),
) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    let mut buf = vec![0u8; 1 << 20];
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target, done, report)?;
        } else if kind.is_symlink() {
            let link = std::fs::read_link(entry.path())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link, &target)?;
            #[cfg(not(unix))]
            {
                // Windows game folders hold no links worth keeping as links
                let _ = link;
                std::fs::copy(entry.path(), &target)?;
            }
        } else {
            use std::io::{Read, Write};
            let mut src = std::fs::File::open(entry.path())?;
            let mut dst = std::fs::File::create(&target)?;
            loop {
                let n = src.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                dst.write_all(&buf[..n])?;
                *done += n as u64;
                report(*done, false);
            }
            let _ = std::fs::set_permissions(&target, src.metadata()?.permissions());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_a_tree_with_progress_and_refuses_bad_destinations() {
        let base = std::env::temp_dir().join(format!("hyper-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old = base.join("Games").join("Catnip");
        std::fs::create_dir_all(old.join("Bin")).expect("dirs");
        std::fs::write(old.join("Bin").join("game.exe"), vec![7u8; 3000]).expect("file");
        std::fs::write(old.join("readme.txt"), b"hi").expect("file");
        let new_base = base.join("Other");
        std::fs::create_dir_all(&new_base).expect("new base");

        assert!(
            destination(&old, &base.join("Games")).is_err(),
            "same place"
        );
        assert!(destination(&old, &old).is_err(), "into itself");
        assert!(
            destination(&old, &old.join("Bin")).is_err(),
            "inside itself"
        );
        assert!(destination(&old, &base.join("missing")).is_err());
        #[cfg(windows)]
        {
            let upper = PathBuf::from(base.join("GAMES").to_string_lossy().to_uppercase());
            assert!(destination(&old, &upper).is_err(), "same place, other case");
            let upper_inside = PathBuf::from(old.join("BIN").to_string_lossy().to_uppercase());
            assert!(
                destination(&old, &upper_inside).is_err(),
                "inside itself, other case"
            );
        }
        let dest = destination(&old, &new_base).expect("dest");
        assert_eq!(dest, new_base.join("Catnip"));

        assert!(check_space(&new_base, 0).is_ok());
        assert!(
            check_space(&new_base, u64::MAX / 2).is_err(),
            "more than any disk has"
        );

        let seen = std::sync::Mutex::new(Vec::new());
        copy_dir(&old, &dest, 3002, &|p| {
            seen.lock().expect("lock").push(p.bytes_done)
        })
        .expect("copy");
        assert_eq!(
            std::fs::read(dest.join("Bin").join("game.exe"))
                .expect("read")
                .len(),
            3000
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("readme.txt")).expect("read"),
            "hi"
        );
        assert_eq!(seen.lock().expect("lock").last().copied(), Some(3002));
        assert!(destination(&old, &new_base).is_err(), "exists now");
        let _ = std::fs::remove_dir_all(&base);
    }
}
