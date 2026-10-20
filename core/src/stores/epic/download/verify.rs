//! `legendary verify`: hash every installed file against the manifest and
//! write the repair file (`tmp/<app>.repair`, `sha1:filename` for every
//! file that exists, with the hash it really has) that a repair resumes
//! from: matching files are skipped, the rest downloaded again.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sha1::{Digest, Sha1};

use super::super::manifest::Manifest;
use crate::error::{Error, Result};

#[derive(Debug, Default)]
pub struct VerifyReport {
    pub checked: usize,
    pub mismatched: usize,
    pub missing: usize,
}

/// Progress: bytes hashed, bytes total.
pub type VerifyProgress = Arc<dyn Fn(u64, u64) + Send + Sync>;

pub fn verify(
    install_dir: &Path,
    manifest: &Manifest,
    repair_file: &Path,
    on_progress: VerifyProgress,
    cancel: &AtomicBool,
) -> Result<VerifyReport> {
    if !install_dir.is_dir() {
        return Err(Error::Other(format!(
            "install folder missing: {}",
            install_dir.display()
        )));
    }
    let mut files: Vec<_> = manifest.files.iter().collect();
    files.sort_by_key(|f| f.filename.to_lowercase());
    let total: u64 = files.iter().map(|f| f.file_size).sum();
    let mut done = 0u64;
    let mut report = VerifyReport::default();
    let mut lines = String::new();
    let mut buf = vec![0u8; 1 << 20];
    for f in files {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        // a name leaving the folder counts as missing (and is never written either)
        let path =
            super::super::installed::locate_checked(install_dir, &f.filename, cfg!(not(windows)));
        let Some(mut file) = path.ok().and_then(|p| std::fs::File::open(p).ok()) else {
            report.missing += 1;
            report.checked += 1;
            continue;
        };
        let mut hasher = Sha1::new();
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            done += n as u64;
        }
        on_progress(done.min(total), total);
        let got: [u8; 20] = hasher.finalize().into();
        if got != f.sha {
            tracing::warn!(file = f.filename, "file does not match its hash");
            report.mismatched += 1;
        }
        let hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
        lines.push_str(&format!("{hex}:{}\n", f.filename));
        report.checked += 1;
    }
    if let Some(parent) = repair_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(repair_file, lines)?;
    tracing::info!(?report, "verification done");
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::super::super::manifest::{ChunkPart, FileManifest, Meta};
    use super::*;

    #[test]
    fn writes_real_hashes_for_existing_files() {
        let dir = std::env::temp_dir().join(format!("hyper-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ok.bin"), b"abc").unwrap();
        std::fs::write(dir.join("bad.bin"), b"xyz").unwrap();
        let sha_abc: [u8; 20] = Sha1::digest(b"abc").into();
        let mk = |name: &str, sha: [u8; 20]| FileManifest {
            filename: name.into(),
            sha,
            chunk_parts: vec![ChunkPart {
                guid: [1; 4],
                offset: 0,
                size: 3,
                file_offset: 0,
            }],
            file_size: 3,
            ..FileManifest::default()
        };
        let m = Manifest {
            version: 18,
            meta: Meta::default(),
            chunks: vec![],
            files: vec![
                mk("ok.bin", sha_abc),
                mk("bad.bin", sha_abc),
                mk("gone.bin", sha_abc),
            ],
            ..Manifest::default()
        };
        let repair = dir.join("x.repair");
        let r = verify(
            &dir,
            &m,
            &repair,
            Arc::new(|_, _| {}),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!((r.checked, r.mismatched, r.missing), (3, 1, 1));
        let text = std::fs::read_to_string(&repair).unwrap();
        let hex_abc: String = sha_abc.iter().map(|b| format!("{b:02x}")).collect();
        assert!(text.contains(&format!("{hex_abc}:ok.bin")));
        assert!(text.contains(":bad.bin") && !text.contains(&format!("{hex_abc}:bad.bin")));
        assert!(!text.contains("gone.bin"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
