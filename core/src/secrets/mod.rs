//! Encrypted storage for store credentials.
//!
//! Threat model: an attacker who copies files from the disk (backup theft,
//! another user on the machine, malware running as a different account, a
//! stolen laptop without the Windows password) must not be able to use the
//! Epic / GOG tokens. Blobs are therefore bound to the Windows user account
//! with DPAPI (`CryptProtectData`, user scope + app entropy): they only decrypt
//! for the same user on the same machine.
//!
//! Tokens never touch the disk in plaintext: the store clients read them
//! from the vault and write refreshed ones back (see [`SecretFile`]). Old
//! versions materialised plaintext copies for the helper CLIs; a leftover
//! of those is absorbed into the vault at start.
//!
//! What this does NOT protect against: code running as the same Windows user
//! while that user is logged in — that code could call DPAPI itself. That is
//! the limit of any client-side launcher, including the official ones.
//!
//! Linux has no DPAPI and the Steam Deck's game mode no keyring: blobs are
//! AES-256-GCM sealed with a key file kept outside the config folder (see
//! `key_file`). Vaults written by the old plaintext `owner-file` backend are
//! re-encrypted on first read.

#[cfg(windows)]
mod dpapi;
#[cfg(unix)]
mod key_file;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use zeroize::Zeroizing;

use crate::error::{Error, Result};

const MAGIC: &[u8; 8] = b"HYPVLT01";
/// Early Linux builds stored blobs unencrypted under this backend id.
const LEGACY_PLAIN: &str = "owner-file";
/// In the vault folder once its plaintext entries were encrypted: from then
/// on an `owner-file` blob is refused instead of trusted (it could only have
/// been planted).
const MIGRATED_MARKER: &str = ".encrypted";

/// Decrypted bytes, zeroed when dropped. `stale`: sealed the old way (the
/// app-wide entropy / AAD, not yet bound to the entry name); the vault
/// re-seals it.
pub struct Unprotected {
    pub plain: Zeroizing<Vec<u8>>,
    pub stale: bool,
}

pub trait SecretBackend: Send + Sync {
    fn id(&self) -> &'static str;
    /// Seal `plain` for the entry named `label`; only that entry opens it.
    fn protect(&self, plain: &[u8], label: &str) -> Result<Vec<u8>>;
    fn unprotect(&self, blob: &[u8], label: &str) -> Result<Unprotected>;
}

/// Picks the backend for the current OS.
pub fn default_backend() -> Box<dyn SecretBackend> {
    #[cfg(windows)]
    {
        Box::new(dpapi::Dpapi)
    }
    #[cfg(unix)]
    {
        Box::new(key_file::KeyFile::new(crate::paths::vault_key_file()))
    }
}

/// Write `bytes` to `path` and flush them to disk, readable by the owner
/// only on Unix (where the backend relies on file permissions).
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    #[cfg(unix)]
    {
        // `mode` only applies to new files
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(bytes)?;
    f.sync_all()
}

/// Create `dir` (and its parents) for credentials: owner-only on Unix
/// (0700), also when it exists already. On Windows the `%APPDATA%` ACL is
/// already the user's own.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Unix: make a rename / link into the folder of `path` survive a power
/// loss (the folder entry is flushed too). Nothing to do on Windows.
pub(crate) fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Stores encrypted blobs as `<dir>/<key>.bin`.
pub struct Vault {
    backend: Box<dyn SecretBackend>,
    dir: PathBuf,
}

impl Vault {
    pub fn new(dir: PathBuf) -> Self {
        let v = Self {
            backend: default_backend(),
            dir,
        };
        v.migrate_plaintext();
        v
    }

    /// One-time: encrypt every entry written by the old plaintext backend,
    /// then mark the folder so such entries are refused from now on. The
    /// marker is only written once every entry made it: a failed migration
    /// (key file not writable, disk full) gets another try at the next start
    /// instead of locking the user out of a login that is still on disk.
    fn migrate_plaintext(&self) {
        let marker = self.dir.join(MIGRATED_MARKER);
        if marker.exists() {
            return;
        }
        let mut all_ok = true;
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if let Some(key) = name.strip_suffix(".bin")
                    && let Err(err) = self.load_inner(key, true)
                {
                    tracing::warn!(key, error = %err, "vault entry not migrated");
                    all_ok = false;
                }
            }
        }
        if !all_ok {
            return;
        }
        if create_private_dir(&self.dir)
            .and_then(|_| std::fs::write(&marker, b""))
            .is_err()
        {
            tracing::warn!("could not mark the vault as encrypted");
        }
    }

    #[cfg(test)]
    pub fn with_backend(dir: PathBuf, backend: Box<dyn SecretBackend>) -> Self {
        Self { backend, dir }
    }

    /// The entry name as used on disk and as the seal label (DPAPI entropy /
    /// AES-GCM AAD), so both always agree whatever characters `key` has.
    fn safe_name(key: &str) -> String {
        key.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    fn file(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{}.bin", Self::safe_name(key)))
    }

    pub fn has(&self, key: &str) -> bool {
        self.file(key).is_file()
    }

    pub fn load(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        self.load_inner(key, false)
    }

    fn load_inner(&self, key: &str, migrating: bool) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let path = self.file(key);
        if !path.is_file() {
            return Ok(None);
        }
        let raw = std::fs::read(&path)?;
        if raw.len() < MAGIC.len() + 1 || &raw[..MAGIC.len()] != MAGIC {
            return Err(Error::Secrets(format!(
                "{} is not a vault file",
                path.display()
            )));
        }
        let id_len = raw[MAGIC.len()] as usize;
        let body_start = MAGIC.len() + 1 + id_len;
        if raw.len() < body_start {
            return Err(Error::Secrets(format!("{} is truncated", path.display())));
        }
        let id = String::from_utf8_lossy(&raw[MAGIC.len() + 1..body_start]);
        if id == LEGACY_PLAIN {
            if !migrating && self.dir.join(MIGRATED_MARKER).exists() {
                // planted after the migration: never trusted, and not kept
                // around in plaintext either
                drop(raw);
                if let Err(e) = wipe_file(&path) {
                    tracing::warn!(path = %path.display(), error = %e, "could not remove the refused plaintext entry");
                }
                return Err(Error::Secrets(format!(
                    "{} is unencrypted; refusing it",
                    path.display()
                )));
            }
            let plain = Zeroizing::new(raw[body_start..].to_vec());
            self.store(key, &plain)?;
            tracing::info!(
                key,
                backend = self.backend.id(),
                "plaintext vault entry encrypted"
            );
            return Ok(Some(plain));
        }
        if id != self.backend.id() {
            return Err(Error::Secrets(format!(
                "{} was written by backend `{id}`, current is `{}`",
                path.display(),
                self.backend.id()
            )));
        }
        let u = self
            .backend
            .unprotect(&raw[body_start..], &Self::safe_name(key))?;
        if u.stale {
            self.store(key, &u.plain)?;
            tracing::info!(key, "vault entry re-sealed to its name");
        }
        Ok(Some(u.plain))
    }

    pub fn store(&self, key: &str, plain: &[u8]) -> Result<()> {
        static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
        create_private_dir(&self.dir)?;
        let safe = Self::safe_name(key);
        let blob = self.backend.protect(plain, &safe)?;
        let id = self.backend.id().as_bytes();
        let mut out = Vec::with_capacity(MAGIC.len() + 1 + id.len() + blob.len());
        out.extend_from_slice(MAGIC);
        out.push(id.len() as u8);
        out.extend_from_slice(id);
        out.extend_from_slice(&blob);
        let path = self.file(key);
        // a temp name of this writer alone: two writers of the same entry
        // (a refresh and a leftover absorbed at exit) must not share one
        let tmp = self.dir.join(format!(
            "{safe}.{}-{}.tmp",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        if let Err(e) = write_private(&tmp, &out).and_then(|_| std::fs::rename(&tmp, &path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        sync_dir(&path);
        tracing::debug!(
            key,
            bytes = plain.len(),
            backend = self.backend.id(),
            "secret stored"
        );
        Ok(())
    }

    pub fn delete(&self, key: &str) -> Result<()> {
        let path = self.file(key);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

/// Overwrite a file with zeros and remove it. Best effort: on SSDs and
/// journaling file systems the old blocks may survive, which is exactly why
/// the long-term copy is encrypted.
pub fn wipe_file(path: &Path) -> std::io::Result<()> {
    use std::io::Write;
    // never follow a link: zero-filling whatever it points at is not ours to do
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if !meta.file_type().is_file() {
        return std::fs::remove_file(path);
    }
    // overwrite in place: truncating first would free the blocks unwritten
    let overwritten = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|mut f| {
            let zeros = [0u8; 64 * 1024];
            let mut left = meta.len();
            while left > 0 {
                let n = left.min(zeros.len() as u64) as usize;
                f.write_all(&zeros[..n])?;
                left -= n as u64;
            }
            f.sync_all()
        });
    if let Err(e) = overwritten {
        tracing::warn!(path = %path.display(), error = %e, "could not overwrite before removing");
    }
    std::fs::remove_file(path)
}

/// `path` is a regular file (not a link to one).
fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_file())
        .unwrap_or(false)
}

/// One vault entry. Older versions kept a plaintext copy of it on disk for
/// the helper CLIs (`plain_path`); nothing writes that file any more, and
/// `absorb_leftover` moves what an old version left behind into the vault.
///
/// `generation` counts logins and logouts: a token refresh that started
/// before a logout must not write its result back afterwards (that would
/// log the user in again), so it stores through [`SecretFile::update`] with
/// the generation it read.
pub struct SecretFile {
    vault: Arc<Vault>,
    key: String,
    plain_path: PathBuf,
    generation: AtomicU64,
    write: std::sync::Mutex<()>,
}

impl SecretFile {
    pub fn new(vault: Arc<Vault>, key: impl Into<String>, plain_path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            vault,
            key: key.into(),
            plain_path,
            generation: AtomicU64::new(0),
            write: std::sync::Mutex::new(()),
        })
    }

    /// Is there a secret in the vault for this file?
    pub fn exists(&self) -> bool {
        self.vault.has(&self.key)
    }

    /// Decrypted content.
    pub fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>> {
        self.vault.load(&self.key)
    }

    /// The login generation to pass to [`SecretFile::update`]; take it
    /// before reading the secret the update derives from.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.write.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Store a secret obtained by a new login; starts a new generation, so a
    /// refresh of the previous login still in flight cannot overwrite it.
    pub async fn replace(&self, plain: &[u8]) -> Result<()> {
        let _w = self.lock();
        self.vault.store(&self.key, plain)?;
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// Store a refreshed secret derived from the one read at generation
    /// `seen`; refused when a logout or login happened since.
    pub async fn update(&self, seen: u64, plain: &[u8]) -> Result<()> {
        let _w = self.lock();
        if self.generation.load(Ordering::Acquire) != seen {
            return Err(Error::Secrets(
                "the login changed while a token was being refreshed".into(),
            ));
        }
        self.vault.store(&self.key, plain)
    }

    /// Log out: delete the entry (and any plaintext left by an old version).
    pub async fn revoke(&self) -> Result<()> {
        let _w = self.lock();
        self.generation.fetch_add(1, Ordering::AcqRel);
        let wiped = if is_regular_file(&self.plain_path) {
            wipe_file(&self.plain_path)
        } else {
            Ok(())
        };
        self.vault.delete(&self.key)?;
        wiped?;
        Ok(())
    }

    /// Move any plaintext left on disk (crash, old version) into the vault.
    pub fn absorb_leftover(&self) -> Result<bool> {
        if !is_regular_file(&self.plain_path) {
            return Ok(false);
        }
        let _w = self.lock();
        let plain = Zeroizing::new(std::fs::read(&self.plain_path)?);
        let stored = if plain.is_empty() {
            Ok(())
        } else {
            tracing::warn!(path = %self.plain_path.display(), "plaintext credentials found on disk; moving into the vault");
            self.vault.store(&self.key, &plain)
        };
        // the plaintext goes even when storing failed
        wipe_file(&self.plain_path)?;
        stored?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// XOR "cipher" so the vault logic can be tested on any OS.
    struct Toy;
    impl SecretBackend for Toy {
        fn id(&self) -> &'static str {
            "toy"
        }
        fn protect(&self, plain: &[u8], _: &str) -> Result<Vec<u8>> {
            Ok(plain.iter().map(|b| b ^ 0x5a).collect())
        }
        fn unprotect(&self, blob: &[u8], _: &str) -> Result<Unprotected> {
            Ok(Unprotected {
                plain: Zeroizing::new(blob.iter().map(|b| b ^ 0x5a).collect()),
                stale: false,
            })
        }
    }

    fn vault() -> (Arc<Vault>, tempdir::Dir) {
        let dir = tempdir::Dir::new();
        (
            Arc::new(Vault::with_backend(dir.path().join("vault"), Box::new(Toy))),
            dir,
        )
    }

    mod tempdir {
        use std::path::{Path, PathBuf};
        pub struct Dir(PathBuf);
        impl Dir {
            pub fn new() -> Self {
                let p = std::env::temp_dir().join(format!(
                    "hyper-test-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&p).unwrap();
                Dir(p)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn legacy_plaintext_is_encrypted_on_read() {
        let (v, _d) = vault();
        std::fs::create_dir_all(&v.dir).unwrap();
        let mut raw = MAGIC.to_vec();
        raw.push(LEGACY_PLAIN.len() as u8);
        raw.extend_from_slice(LEGACY_PLAIN.as_bytes());
        raw.extend_from_slice(b"{\"token\":1}");
        std::fs::write(v.file("gog"), raw).unwrap();
        assert_eq!(v.load("gog").unwrap().unwrap().as_slice(), b"{\"token\":1}");
        let raw = std::fs::read(v.file("gog")).unwrap();
        assert!(
            !raw.windows(5).any(|w| w == b"token"),
            "re-stored encrypted"
        );
        assert_eq!(v.load("gog").unwrap().unwrap().as_slice(), b"{\"token\":1}");
    }

    #[test]
    fn vault_round_trip_and_header() {
        let (v, _d) = vault();
        v.store("epic", b"{\"token\":1}").unwrap();
        assert!(v.has("epic"));
        assert_eq!(
            v.load("epic").unwrap().unwrap().as_slice(),
            b"{\"token\":1}"
        );
        let raw = std::fs::read(v.file("epic")).unwrap();
        assert!(raw.starts_with(MAGIC));
        assert!(
            !raw.windows(5).any(|w| w == b"token"),
            "plaintext must not appear on disk"
        );
        v.delete("epic").unwrap();
        assert!(v.load("epic").unwrap().is_none());
    }

    #[tokio::test]
    async fn revoke_deletes_entry_and_leftover_plaintext() {
        let (v, d) = vault();
        let plain = d.path().join("auth.json");
        v.store("gog", b"token").unwrap();
        std::fs::write(&plain, b"old plaintext").unwrap();
        let sf = SecretFile::new(v.clone(), "gog", plain.clone());
        assert!(sf.exists());
        assert_eq!(sf.read().unwrap().unwrap().as_slice(), b"token");
        sf.revoke().await.unwrap();
        assert!(!v.has("gog") && !plain.exists());
        sf.replace(b"again").await.unwrap();
        assert_eq!(v.load("gog").unwrap().unwrap().as_slice(), b"again");
    }

    #[tokio::test]
    async fn refresh_after_logout_does_not_restore_the_login() {
        let (v, d) = vault();
        let sf = SecretFile::new(v.clone(), "epic", d.path().join("user.json"));
        sf.replace(b"session").await.unwrap();
        // a refresh reads the session, then the user logs out before it writes back
        let seen = sf.generation();
        assert_eq!(sf.read().unwrap().unwrap().as_slice(), b"session");
        sf.revoke().await.unwrap();
        assert!(sf.update(seen, b"refreshed").await.is_err());
        assert!(!sf.exists());
        // a refresh of the current login stores fine
        sf.replace(b"again").await.unwrap();
        let seen = sf.generation();
        sf.update(seen, b"again+").await.unwrap();
        assert_eq!(sf.read().unwrap().unwrap().as_slice(), b"again+");
    }

    #[test]
    fn refused_plaintext_is_removed() {
        let (v, _d) = vault();
        std::fs::create_dir_all(&v.dir).unwrap();
        std::fs::write(v.dir.join(MIGRATED_MARKER), b"").unwrap();
        let mut raw = MAGIC.to_vec();
        raw.push(LEGACY_PLAIN.len() as u8);
        raw.extend_from_slice(LEGACY_PLAIN.as_bytes());
        raw.extend_from_slice(b"{\"token\":1}");
        std::fs::write(v.file("epic"), raw).unwrap();
        assert!(v.load("epic").is_err());
        assert!(!v.file("epic").exists(), "the planted plaintext is gone");
    }

    #[test]
    fn planted_plaintext_is_refused_after_migration() {
        let (v, _d) = vault();
        std::fs::create_dir_all(&v.dir).unwrap();
        std::fs::write(v.dir.join(MIGRATED_MARKER), b"").unwrap();
        let mut raw = MAGIC.to_vec();
        raw.push(LEGACY_PLAIN.len() as u8);
        raw.extend_from_slice(LEGACY_PLAIN.as_bytes());
        raw.extend_from_slice(b"{\"token\":1}");
        std::fs::write(v.file("epic"), raw).unwrap();
        assert!(v.load("epic").is_err());
    }

    #[test]
    fn absorb_leftover_moves_plaintext() {
        let (v, d) = vault();
        let plain = d.path().join("user.json");
        std::fs::write(&plain, b"leftover").unwrap();
        let sf = SecretFile::new(v.clone(), "epic", plain.clone());
        assert!(sf.absorb_leftover().unwrap());
        assert!(!plain.exists());
        assert_eq!(v.load("epic").unwrap().unwrap().as_slice(), b"leftover");
    }
}
