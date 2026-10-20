//! File-system odds and ends the umu port needs: `flock` guards, the atomic
//! directory swap umu uses for runtime updates, tar extraction, and the few
//! host facts the Steam Runtime environment is built from. Everything here
//! is blocking; callers run it on the blocking pool.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

/// An exclusive `flock(2)` on a lock file, released on drop. umu, Steam and
/// other launchers take the same locks, so two of them never unpack the same
/// runtime or set up the same prefix at once.
pub struct FileLock {
    _file: File,
}

/// How long [`lock`] sleeps between two attempts to take a held lock.
pub const LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(250);

/// Open (create) the lock file without locking it; see [`try_lock`].
pub fn open_lock_file(path: &Path) -> io::Result<File> {
    fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
}

/// One non-blocking attempt (`LOCK_EX | LOCK_NB`) to take the lock on an
/// opened lock file: `Ok(None)` when another process holds it. Never
/// blocks, so callers can poll it next to a cancel flag.
pub fn try_lock(file: File) -> io::Result<std::result::Result<FileLock, File>> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: a valid, open descriptor; flock has no memory arguments
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let err = io::Error::last_os_error();
            return if err.kind() == io::ErrorKind::WouldBlock {
                Ok(Err(file))
            } else {
                Err(err)
            };
        }
    }
    Ok(Ok(FileLock { _file: file }))
}

/// Take the lock, waiting for another holder in [`LOCK_RETRY`] steps while
/// `cancelled` stays false (a blocking `flock` could not be interrupted).
/// `waiting` is called once when the lock is held by someone else, so the
/// caller can say so. Blocking; the async callers use `super::lock`.
#[cfg(test)]
pub fn lock(
    path: &Path,
    cancelled: &dyn Fn() -> bool,
    waiting: &dyn Fn(),
) -> io::Result<Option<FileLock>> {
    let mut file = open_lock_file(path)?;
    let mut reported = false;
    loop {
        file = match try_lock(file)? {
            Ok(lock) => return Ok(Some(lock)),
            Err(file) => file,
        };
        if cancelled() {
            return Ok(None);
        }
        if !reported {
            reported = true;
            waiting();
        }
        std::thread::sleep(LOCK_RETRY);
    }
}

/// Leftovers of an unpack that was interrupted (the app closed mid-way):
/// `<dir>/<prefix>*` folders.
pub fn remove_stale_temp_dirs(dir: &Path, prefix: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(prefix)
            && entry.file_type().is_ok_and(|t| t.is_dir())
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: see `lock`; closing the file would release it anyway
            unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

/// Swap two paths atomically (`renameat2(RENAME_EXCHANGE)`), so a runtime
/// update replaces the folder in one step and the old one can be removed
/// afterwards. File systems without the flag (NFS) get three renames.
pub fn exchange(a: &Path, b: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let ca = CString::new(a.as_os_str().as_bytes())
            .map_err(|_| io::Error::other("path contains NUL"))?;
        let cb = CString::new(b.as_os_str().as_bytes())
            .map_err(|_| io::Error::other("path contains NUL"))?;
        // SAFETY: both strings are valid C strings living through the call
        let ret = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                ca.as_ptr(),
                libc::AT_FDCWD,
                cb.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        if ret == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if !matches!(
            err.raw_os_error(),
            Some(libc::ENOSYS | libc::EINVAL | libc::ENOTSUP)
        ) {
            return Err(err);
        }
    }
    // best-effort fallback: b -> tmp, a -> b, tmp -> a
    let tmp = b.with_file_name(format!(
        "{}.exchange-tmp",
        b.file_name().and_then(|n| n.to_str()).unwrap_or("x")
    ));
    fs::rename(b, &tmp)?;
    if let Err(e) = fs::rename(a, b) {
        let _ = fs::rename(&tmp, b);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, a) {
        let _ = fs::rename(b, a);
        let _ = fs::rename(&tmp, b);
        return Err(e);
    }
    Ok(())
}

pub fn symlink(target: &str, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err(io::Error::other("symlinks are only created on Unix"))
    }
}

pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// A folder that is removed with everything in it when dropped.
pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    /// `<parent>/<prefix><unique>`: next to its final destination, so a
    /// rename into place stays on one file system.
    pub fn new_in(parent: &Path, prefix: &str) -> io::Result<Self> {
        fs::create_dir_all(parent)?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for n in 0..100u128 {
            let path = parent.join(format!(
                "{prefix}{:x}-{:x}",
                std::process::id(),
                nanos.wrapping_add(n)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("could not create a temporary folder"))
    }

    /// Keep the folder (it was renamed away or is wanted).
    #[cfg(test)]
    pub fn keep(mut self) -> PathBuf {
        std::mem::take(&mut self.path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.path.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Unpack a `.tar.gz` / `.tar.xz` into `dest`. The `tar` crate refuses
/// entries that would land outside `dest` (absolute paths, `..`, through a
/// symlink); set-uid bits and ownership are not restored.
pub fn extract_tar(archive: &Path, dest: &Path) -> io::Result<()> {
    let file = io::BufReader::with_capacity(1 << 20, File::open(archive)?);
    let name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let reader: Box<dyn io::Read> = if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        Box::new(lzma_rust2::XzReader::new(file, true))
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Box::new(flate2::bufread::GzDecoder::new(file))
    } else if name.ends_with(".tar") {
        Box::new(file)
    } else {
        return Err(io::Error::other(format!(
            "{name}: not a .tar.gz / .tar.xz archive"
        )));
    };
    fs::create_dir_all(dest)?;
    let mut tar = tar::Archive::new(reader);
    tar.set_preserve_permissions(false);
    tar.set_preserve_ownerships(false);
    tar.set_overwrite(true);
    tar.unpack(dest)
}

/// Login name of the current user (`getpwuid`), the name Wine uses for the
/// `drive_c/users/<name>` folder.
pub fn unix_user_name() -> String {
    #[cfg(unix)]
    {
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut buf = vec![0u8; 16 * 1024];
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: buffer and out-pointers are valid for the call; the result
        // points into `buf`, which outlives the copy below
        let ret = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut pwd,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut result,
            )
        };
        if ret == 0 && !result.is_null() && !pwd.pw_name.is_null() {
            // SAFETY: pw_name is a NUL-terminated string inside `buf`
            let name = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) };
            if let Ok(s) = name.to_str()
                && !s.is_empty()
            {
                return s.to_string();
            }
        }
    }
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "steamuser".into())
}

/// Is `path` a mount point? Like Python's `Path.is_mount`: a different
/// device than its parent, or the same inode (the root of a bind mount).
pub fn is_mount_point(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Some(parent) = path.parent() else {
            return false;
        };
        let (Ok(a), Ok(b)) = (fs::metadata(path), fs::metadata(parent)) else {
            return false;
        };
        a.dev() != b.dev() || a.ino() == b.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// Shared-library folders of the host, from the `ld.so` cache (`ldconfig
/// -p`), resolved; what the Steam Runtime gets as `STEAM_RUNTIME_LIBRARY_PATH`.
pub fn library_paths() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    dirs.push(PathBuf::from("/sbin"));
    dirs.push(PathBuf::from("/usr/sbin"));
    let Some(ldconfig) = dirs
        .iter()
        .map(|d| d.join("ldconfig"))
        .find(|p| p.is_file())
    else {
        tracing::warn!("ldconfig not found; the runtime gets no host library paths");
        return out;
    };
    let mut ldconfig_cmd = std::process::Command::new(ldconfig);
    crate::platform::child_env::apply_std(&mut ldconfig_cmd);
    let output = ldconfig_cmd
        .arg("-p")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(output) = output else { return out };
    for prefix in ldconfig_dirs(&String::from_utf8_lossy(&output.stdout)) {
        let real = fs::canonicalize(&prefix).unwrap_or_else(|_| PathBuf::from(&prefix));
        let real = real.to_string_lossy().into_owned();
        if !real.is_empty() {
            out.insert(real);
        }
    }
    out
}

/// The folders of the libraries `ldconfig -p` lists (`libc.so.6 (libc6,
/// x86-64) => /lib/x86_64-linux-gnu/libc.so.6`), each once, in order.
fn ldconfig_dirs(stdout: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for line in stdout.lines() {
        let Some(last) = line.split_whitespace().last() else {
            continue;
        };
        let Some(idx) = last.rfind('/') else { continue };
        if !last.starts_with('/') {
            continue;
        }
        let prefix = &last[..idx];
        // `/libfoo.so` (a library right under `/`) would give an empty folder
        if prefix.is_empty() || !seen.insert(prefix.to_string()) {
            continue;
        }
        out.push(prefix.to_string());
    }
    out
}

/// Sort key for version-like names: digit runs compare as numbers, the
/// rest case-insensitively (`UMU-Proton-10.0-4` > `UMU-Proton-9.0-3`).
pub fn natural_key(name: &str) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut digits = false;
    let flush = |out: &mut Vec<(u64, String)>, cur: &mut String, digits: bool| {
        if cur.is_empty() {
            return;
        }
        if digits {
            out.push((cur.parse().unwrap_or(u64::MAX), String::new()));
        } else {
            out.push((0, cur.to_ascii_lowercase()));
        }
        cur.clear();
    };
    for c in name.chars() {
        if c.is_ascii_digit() != digits {
            flush(&mut out, &mut cur, digits);
            digits = c.is_ascii_digit();
        }
        cur.push(c);
    }
    flush(&mut out, &mut cur, digits);
    out
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_counts_numbers() {
        let mut names = vec![
            "UMU-Proton-9.0-3",
            "UMU-Proton-10.0-4",
            "UMU-Proton-10.0-1",
            "umu-proton-9.0-3rc1",
        ];
        names.sort_by_key(|n| natural_key(n));
        assert_eq!(
            names,
            [
                "UMU-Proton-9.0-3",
                "umu-proton-9.0-3rc1",
                "UMU-Proton-10.0-1",
                "UMU-Proton-10.0-4"
            ]
        );
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0, 1, 0xab, 0xff]), "0001abff");
    }

    #[test]
    fn ldconfig_folders_skip_the_root_and_repeats() {
        let out = "\t3 libs found in cache `/etc/ld.so.cache'\n\
                   \tlibz.so.1 (libc6,x86-64) => /lib/x86_64-linux-gnu/libz.so.1\n\
                   \tlibc.so.6 (libc6,x86-64) => /lib/x86_64-linux-gnu/libc.so.6\n\
                   \tlibodd.so (libc6,x86-64) => /libodd.so\n\
                   \tlibrel.so (libc6,x86-64) => rel/librel.so\n\
                   \tlibvk.so.1 (libc6,x86-64) => /usr/lib/libvk.so.1\n";
        assert_eq!(ldconfig_dirs(out), ["/lib/x86_64-linux-gnu", "/usr/lib"]);
        assert!(ldconfig_dirs("").is_empty());
    }

    #[test]
    fn stale_temp_dirs_go_other_entries_stay() {
        let base = std::env::temp_dir().join(format!("hyper-stale-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join(".hyper-1-2")).expect("stale");
        fs::create_dir_all(base.join("UMU-Proton-10.0-4")).expect("keep");
        fs::write(base.join(".hyper-note"), "a file").expect("file");
        remove_stale_temp_dirs(&base, ".hyper-");
        assert!(!base.join(".hyper-1-2").exists());
        assert!(base.join("UMU-Proton-10.0-4").is_dir());
        assert!(base.join(".hyper-note").is_file());
        remove_stale_temp_dirs(&base.join("missing"), ".hyper-");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn lock_is_taken_and_gives_up_on_cancel() {
        let base = std::env::temp_dir().join(format!("hyper-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("base");
        let path = base.join("x.lock");
        let held = lock(&path, &|| false, &|| {})
            .expect("lock")
            .expect("taken");
        // flock is per open file description: a second open of the same
        // file sees the lock held, even inside one process (Unix only;
        // elsewhere the lock is a no-op)
        #[cfg(unix)]
        {
            let again = lock(&path, &|| true, &|| {}).expect("no error");
            assert!(again.is_none(), "a held lock is not taken; cancel gives up");
        }
        drop(held);
        let again = lock(&path, &|| false, &|| {}).expect("lock");
        assert!(again.is_some());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn temp_dir_is_removed_on_drop_and_kept_on_keep() {
        let base = std::env::temp_dir().join(format!("hyper-fsutil-{}", std::process::id()));
        let t = TempDir::new_in(&base, ".tmp").expect("temp dir");
        let p = t.path.clone();
        assert!(p.is_dir());
        drop(t);
        assert!(!p.exists());
        let t = TempDir::new_in(&base, ".tmp").expect("temp dir");
        let p = t.keep();
        assert!(p.is_dir());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn exchange_swaps_two_folders() {
        let base = std::env::temp_dir().join(format!("hyper-exchange-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let a = base.join("a");
        let b = base.join("b");
        fs::create_dir_all(&a).expect("a");
        fs::create_dir_all(&b).expect("b");
        fs::write(a.join("x"), "from a").expect("write");
        fs::write(b.join("y"), "from b").expect("write");
        exchange(&a, &b).expect("exchange");
        assert!(a.join("y").is_file() && b.join("x").is_file());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn extracts_a_tar_gz() {
        use std::io::Write;
        let base = std::env::temp_dir().join(format!("hyper-tar-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("base");
        let archive = base.join("t.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(
                File::create(&archive).expect("create"),
                flate2::Compression::fast(),
            );
            let mut tar = tar::Builder::new(gz);
            let data = b"hello";
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, "Top/file.txt", &data[..])
                .expect("append");
            tar.into_inner()
                .expect("tar")
                .finish()
                .expect("gz")
                .flush()
                .expect("flush");
        }
        extract_tar(&archive, &base.join("out")).expect("extract");
        assert_eq!(
            fs::read_to_string(base.join("out/Top/file.txt")).expect("read"),
            "hello"
        );
        assert!(extract_tar(&base.join("nope.zip"), &base).is_err());
        let _ = fs::remove_dir_all(&base);
    }
}
