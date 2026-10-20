//! `gogdl repair`'s check: which installed files differ from the manifest.
//! Generation-2 files are compared chunk by chunk (MD5 of each `size`
//! bytes), generation-1 files by their whole MD5; a missing file counts as
//! broken. Only the broken ones are downloaded again.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use md5::{Digest, Md5};

use super::super::hex;
use super::super::manifest::GameFile;
use super::target;
use crate::error::{Error, Result};

/// Progress: bytes hashed, bytes total.
pub type VerifyProgress = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// Is the file on disk the one the manifest describes? `buf` is the one
/// read buffer: a chunk is hashed in pieces of it, whatever its size.
fn intact(path: &Path, f: &GameFile, buf: &mut [u8], hashed: &mut u64) -> std::io::Result<bool> {
    let mut file = std::fs::File::open(path)?;
    if f.v1_offset.is_some() || f.chunks.is_empty() {
        let Some(want) = f.md5.as_deref() else {
            return Ok(true);
        };
        let mut hasher = Md5::new();
        loop {
            let n = file.read(buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            *hashed += n as u64;
        }
        return Ok(hex(hasher.finalize()).eq_ignore_ascii_case(want));
    }
    for chunk in &f.chunks {
        let mut hasher = Md5::new();
        let mut left = chunk.size;
        while left > 0 {
            let n = left.min(buf.len() as u64) as usize;
            if file.read_exact(&mut buf[..n]).is_err() {
                return Ok(false);
            }
            hasher.update(&buf[..n]);
            *hashed += n as u64;
            left -= n as u64;
        }
        if !hex(hasher.finalize()).eq_ignore_ascii_case(&chunk.md5) {
            return Ok(false);
        }
    }
    // a longer file than the manifest says is wrong too
    Ok(file.read(&mut [0u8; 1])? == 0)
}

/// Indexes into `files` of the ones to download again.
pub fn broken(
    files: &[GameFile],
    game_dir: &Path,
    support_dir: &Path,
    on_progress: VerifyProgress,
    cancel: &AtomicBool,
) -> Result<Vec<usize>> {
    let total: u64 = files.iter().map(|f| f.size).sum();
    let mut hashed = 0u64;
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 << 20];
    for (i, f) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        if f.size == 0 {
            continue;
        }
        let root = if f.support { support_dir } else { game_dir };
        let path = target(root, &f.path, false)?;
        let ok = match intact(&path, f, &mut buf, &mut hashed) {
            Ok(ok) => ok,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };
        if !ok {
            tracing::warn!(file = f.path, "file missing or does not match its hash");
            out.push(i);
        }
        on_progress(hashed.min(total), total);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::super::manifest::ChunkRef;
    use super::*;

    #[test]
    fn finds_missing_and_damaged_files() {
        let dir = std::env::temp_dir().join(format!("hyper-gog-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ok.bin"), b"abcdef").unwrap();
        std::fs::write(dir.join("bad.bin"), b"abcxyz").unwrap();
        std::fs::write(dir.join("long.bin"), b"abcdefg").unwrap();
        std::fs::write(dir.join("v1.bin"), b"hello").unwrap();
        let chunk = |data: &[u8]| ChunkRef {
            compressed_md5: String::new(),
            md5: hex(Md5::digest(data)),
            size: data.len() as u64,
            compressed_size: 0,
        };
        let v2 = |name: &str| GameFile {
            path: name.into(),
            size: 6,
            chunks: vec![chunk(b"abc"), chunk(b"def")],
            ..GameFile::default()
        };
        let files = vec![
            v2("ok.bin"),
            v2("bad.bin"),
            v2("long.bin"),
            v2("missing.bin"),
            GameFile {
                path: "v1.bin".into(),
                size: 5,
                md5: Some(hex(Md5::digest(b"hello"))),
                v1_offset: Some(0),
                ..GameFile::default()
            },
            GameFile {
                path: "empty".into(),
                ..GameFile::default()
            },
        ];
        let r = broken(
            &files,
            &dir,
            &dir,
            Arc::new(|_, _| {}),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(r, [1, 2, 3]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn chunks_larger_than_the_buffer_are_hashed_in_pieces() {
        let dir = std::env::temp_dir().join(format!("hyper-gog-verify-big-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.join("big.bin"), &data).unwrap();
        let chunk = ChunkRef {
            compressed_md5: String::new(),
            md5: hex(Md5::digest(&data)),
            size: data.len() as u64,
            compressed_size: 0,
        };
        let f = GameFile {
            path: "big.bin".into(),
            size: data.len() as u64,
            chunks: vec![chunk],
            ..GameFile::default()
        };
        let mut buf = vec![0u8; 1 << 20];
        let mut hashed = 0;
        assert!(intact(&dir.join("big.bin"), &f, &mut buf, &mut hashed).unwrap());
        assert_eq!(hashed, data.len() as u64);
        assert_eq!(
            buf.len(),
            1 << 20,
            "the buffer never grows to the chunk's size"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
