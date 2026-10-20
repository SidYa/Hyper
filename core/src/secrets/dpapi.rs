//! Windows Data Protection API backend. Blobs are encrypted with a key
//! derived from the current user's logon credentials (user scope), plus an
//! application entropy so other DPAPI users on the account cannot decrypt
//! them by accident. The entropy also names the entry (`…:epic`), so a blob
//! copied over another entry's file does not decrypt there.

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::core::PCWSTR;
use zeroize::{Zeroize, Zeroizing};

use super::{SecretBackend, Unprotected};
use crate::error::{Error, Result};

const ENTROPY: &[u8] = b"com.mrsidko.hyper.vault.v1";

pub struct Dpapi;

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

/// Entropy of one entry: the app's, bound to the entry's name.
fn entropy(label: &str) -> Vec<u8> {
    [ENTROPY, b":", label.as_bytes()].concat()
}

/// Copy the output blob, zero DPAPI's buffer and free it.
unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Zeroizing<Vec<u8>> {
    // SAFETY: on success DPAPI returns a LocalAlloc'd buffer of cbData bytes
    let buf = unsafe { std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize) };
    let v = Zeroizing::new(buf.to_vec());
    buf.zeroize();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
    }
    v
}

fn unprotect_with(blob_in: &[u8], entropy: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let input = blob(blob_in);
    let entropy = blob(entropy);
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    }
    .map_err(|e| Error::Secrets(format!("CryptUnprotectData: {e}")))?;
    Ok(unsafe { take(out) })
}

impl SecretBackend for Dpapi {
    fn id(&self) -> &'static str {
        "dpapi-user"
    }

    fn protect(&self, plain: &[u8], label: &str) -> Result<Vec<u8>> {
        let input = blob(plain);
        let entropy = entropy(label);
        let entropy = blob(&entropy);
        let desc: Vec<u16> = format!("Hyper:{label}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut out = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptProtectData(
                &input,
                PCWSTR(desc.as_ptr()),
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        }
        .map_err(|e| Error::Secrets(format!("CryptProtectData: {e}")))?;
        // the blob is ciphertext: no need to keep it zeroizing
        Ok(unsafe { take(out) }.to_vec())
    }

    /// Entries written before the entropy named them decrypt with the plain
    /// app entropy once and are then re-sealed (`stale`).
    fn unprotect(&self, blob_in: &[u8], label: &str) -> Result<Unprotected> {
        match unprotect_with(blob_in, &entropy(label)) {
            Ok(plain) => Ok(Unprotected {
                plain,
                stale: false,
            }),
            Err(bound) => match unprotect_with(blob_in, ENTROPY) {
                Ok(plain) => Ok(Unprotected { plain, stale: true }),
                Err(_) => Err(bound),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let d = Dpapi;
        let blob = d.protect(b"hello", "test").unwrap();
        assert_ne!(blob, b"hello");
        let u = d.unprotect(&blob, "test").unwrap();
        assert_eq!(u.plain.as_slice(), b"hello");
        assert!(!u.stale);
    }

    #[test]
    fn bound_to_the_entry_name() {
        let d = Dpapi;
        let blob = d.protect(b"hello", "epic").unwrap();
        assert!(
            d.unprotect(&blob, "gog").is_err(),
            "an Epic blob must not open as GOG's"
        );
    }

    #[test]
    fn old_blobs_open_once_and_are_marked_stale() {
        let input = blob(b"hello");
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        }
        .unwrap();
        let old = unsafe { take(out) }.to_vec();
        let u = Dpapi.unprotect(&old, "epic").unwrap();
        assert_eq!(u.plain.as_slice(), b"hello");
        assert!(u.stale);
    }

    #[test]
    fn tampered_blob_fails() {
        let d = Dpapi;
        let mut blob = d.protect(b"hello", "test").unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0xff;
        assert!(d.unprotect(&blob, "test").is_err());
    }
}
