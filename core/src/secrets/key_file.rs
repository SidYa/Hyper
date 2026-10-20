//! Unix backend: AES-256-GCM with a random key kept in its own file
//! (`~/.local/share/hyper/vault.key`, 0600), away from the vault in
//! `~/.config/hyper`. The Steam Deck's game mode runs no Secret Service,
//! so a keyring cannot be relied on there.
//!
//! What this adds over plain 0600 files: the vault folder alone (a backup, a
//! copied or synced config folder) holds no usable tokens. Like DPAPI it does
//! not stop code running as the same user, which can read the key too.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::OnceLock;

use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};

use zeroize::Zeroizing;

use super::{SecretBackend, Unprotected};
use crate::error::{Error, Result};

const KEY_LEN: usize = 32;
const AAD: &[u8] = b"com.mrsidko.hyper.vault.v1";

/// AAD of one entry: the app's, bound to the entry's name, so a blob copied
/// over another entry's file does not open there.
fn aad(label: &str) -> Vec<u8> {
    [AAD, b":", label.as_bytes()].concat()
}

pub struct KeyFile {
    path: PathBuf,
    key: OnceLock<LessSafeKey>,
}

impl KeyFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            key: OnceLock::new(),
        }
    }

    fn key(&self) -> Result<&LessSafeKey> {
        if let Some(k) = self.key.get() {
            return Ok(k);
        }
        let bytes = self.load_or_create()?;
        let key = UnboundKey::new(&AES_256_GCM, &bytes)
            .map_err(|_| Error::Secrets("bad vault key".into()))?;
        Ok(self.key.get_or_init(|| LessSafeKey::new(key)))
    }

    /// Read the key, or create it on first use. An unreadable or damaged
    /// key file is an error, never replaced: that would orphan the vault.
    fn load_or_create(&self) -> Result<Zeroizing<Vec<u8>>> {
        match std::fs::read(&self.path) {
            Ok(bytes) if bytes.len() == KEY_LEN => return Ok(Zeroizing::new(bytes)),
            Ok(_) => {
                return Err(Error::Secrets(format!(
                    "{} is not a vault key",
                    self.path.display()
                )));
            }
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            Err(_) => {}
        }
        let mut bytes = Zeroizing::new(vec![0u8; KEY_LEN]);
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| Error::Secrets("no system randomness".into()))?;
        if let Some(dir) = self.path.parent() {
            super::create_private_dir(dir)?;
        }
        // Written whole to a private temp file, then linked into place:
        // `link` fails when the name exists (two processes starting at once
        // keep one key) and the key never appears half written (a crash
        // between create and write used to leave an empty key for good).
        self.remove_stale_temps();
        let tmp = self
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .and_then(|mut f| f.write_all(&bytes).and_then(|_| f.sync_all()));
        let linked = written.and_then(|_| std::fs::hard_link(&tmp, &self.path));
        let linked = match linked {
            // a file system without hard links: a rename is still atomic,
            // only the "first one wins" guarantee is lost
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported && !self.path.exists() => {
                std::fs::rename(&tmp, &self.path)
            }
            other => other,
        };
        let _ = std::fs::remove_file(&tmp);
        match linked {
            Ok(()) => {
                super::sync_dir(&self.path);
                tracing::info!(path = %self.path.display(), "vault key created");
                Ok(bytes)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => self.load_or_create(),
            Err(e) => Err(e.into()),
        }
    }

    /// `vault.tmp-<pid>` files of earlier runs that crashed between the link
    /// and the unlink: each is a second name of the live key (same inode).
    fn remove_stale_temps(&self) {
        let (Some(dir), Some(stem)) = (self.path.parent(), self.path.file_stem()) else {
            return;
        };
        let prefix = format!("{}.tmp-", stem.to_string_lossy());
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

impl SecretBackend for KeyFile {
    fn id(&self) -> &'static str {
        "aes-gcm-keyfile"
    }

    /// `nonce || ciphertext+tag`
    fn protect(&self, plain: &[u8], label: &str) -> Result<Vec<u8>> {
        let key = self.key()?;
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| Error::Secrets("no system randomness".into()))?;
        let mut data = plain.to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad(label)),
            &mut data,
        )
        .map_err(|_| Error::Secrets("encryption failed".into()))?;
        let mut out = Vec::with_capacity(NONCE_LEN + data.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&data);
        Ok(out)
    }

    /// Entries sealed before the AAD named them open with the plain app AAD
    /// once and are then re-sealed (`stale`).
    fn unprotect(&self, blob: &[u8], label: &str) -> Result<Unprotected> {
        let key = self.key()?;
        if blob.len() < NONCE_LEN {
            return Err(Error::Secrets("vault blob is truncated".into()));
        }
        let (nonce, sealed) = blob.split_at(NONCE_LEN);
        let open = |aad: &[u8]| -> Option<Zeroizing<Vec<u8>>> {
            let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
            let mut data = Zeroizing::new(sealed.to_vec());
            let n = key
                .open_in_place(nonce, Aad::from(aad), &mut data)
                .ok()?
                .len();
            data.truncate(n);
            Some(data)
        };
        if let Some(plain) = open(&aad(label)) {
            return Ok(Unprotected {
                plain,
                stale: false,
            });
        }
        open(AAD)
            .map(|plain| Unprotected { plain, stale: true })
            .ok_or_else(|| {
                Error::Secrets("vault entry does not decrypt (key changed or file damaged)".into())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tamper() {
        let dir = std::env::temp_dir().join(format!("hyper-keyfile-{}", std::process::id()));
        let path = dir.join("vault.key");
        let k = KeyFile::new(path.clone());
        let blob = k.protect(b"{\"token\":1}", "epic").unwrap();
        assert!(!blob.windows(5).any(|w| w == b"token"));
        assert_eq!(
            k.unprotect(&blob, "epic").unwrap().plain.as_slice(),
            b"{\"token\":1}"
        );
        assert!(
            k.unprotect(&blob, "gog").is_err(),
            "bound to the entry name"
        );

        // a second instance reads the same key
        assert_eq!(
            KeyFile::new(path.clone())
                .unprotect(&blob, "epic")
                .unwrap()
                .plain
                .as_slice(),
            b"{\"token\":1}"
        );

        let mut bad = blob.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(k.unprotect(&bad, "epic").is_err());

        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
