//! Resumable download of one big archive, hashed while it streams.
//!
//! The file grows as `<name>.parts` in umu's cache folder; an interrupted
//! download (network, cancel, the app closed) continues from where it
//! stopped on the next launch with a `Range` request, exactly like umu's
//! `write_file_chunks` / `.parts` handling, so a half-fetched runtime left by
//! umu itself is picked up too. The digest is checked against the one
//! published next to the archive before anything is unpacked.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ring::digest::{Context, SHA256, SHA512};
use tokio::io::AsyncWriteExt;

use super::fsutil::hex;
use super::{Cancel, Preparing, Progress};
use crate::error::{Error, Result};
use crate::types::DownloadProgress;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashKind {
    Sha256,
    Sha512,
}

impl HashKind {
    fn context(self) -> Context {
        match self {
            HashKind::Sha256 => Context::new(&SHA256),
            HashKind::Sha512 => Context::new(&SHA512),
        }
    }

    /// Length of the digest in hex characters.
    fn hex_len(self) -> usize {
        match self {
            HashKind::Sha256 => 64,
            HashKind::Sha512 => 128,
        }
    }
}

/// Version numbers and checksum lists are a few KiB; anything bigger is not
/// the file we asked for.
const TEXT_MAX: u64 = 1 << 20;
const TEXT_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Download {
    pub url: String,
    /// Where the partial file lives; the finished file is this path without
    /// its `.parts` suffix.
    pub parts: PathBuf,
    /// Expected digest, lower-case hex.
    pub expected: String,
    pub kind: HashKind,
    /// Shown in the UI ("Downloading UMU-Proton-10.0-4").
    pub label: String,
}

/// Returns `Err(Error::Cancelled)` on cancel (the partial file stays for a
/// resume) and `Error::Other` on a digest mismatch (the partial file is
/// deleted: it was corrupt).
pub async fn download(
    http: &reqwest::Client,
    dl: &Download,
    progress: Progress<'_>,
    cancelled: Cancel<'_>,
) -> Result<PathBuf> {
    if let Some(parent) = dl.parts.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let final_path = finished_path(&dl.parts);
    let mut done = tokio::fs::metadata(&dl.parts)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let mut hasher = if done > 0 {
        tracing::info!(file = %dl.parts.display(), bytes = done, "resuming download");
        progress(Preparing::message(format!(
            "{}: checking the part downloaded earlier",
            dl.label
        )));
        let path = dl.parts.clone();
        let kind = dl.kind;
        tokio::task::spawn_blocking(move || hash_file(&path, kind))
            .await
            .map_err(|e| Error::Other(format!("hashing failed: {e}")))??
    } else {
        dl.kind.context()
    };

    let mut req = http.get(&dl.url);
    if done > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={done}-"));
    }
    let resp = req.send().await.map_err(net)?;
    let status = resp.status();
    let mut file;
    match status.as_u16() {
        206 => {
            file = tokio::fs::OpenOptions::new()
                .append(true)
                .open(&dl.parts)
                .await?;
        }
        200 => {
            // the server ignored the range (or there was none): start over
            if done > 0 {
                tracing::info!(file = %dl.parts.display(), "server does not resume; downloading from the start");
                hasher = dl.kind.context();
                done = 0;
            }
            file = tokio::fs::File::create(&dl.parts).await?;
        }
        416 if done > 0 => {
            // already complete: only the digest check is left
            return verify(dl, hasher, &final_path).await;
        }
        _ => {
            return Err(Error::Other(format!(
                "{}: server answered {status}",
                dl.url
            )));
        }
    }
    let total = resp.content_length().map(|len| len + done);

    let mut resp = resp;
    let started = Instant::now();
    let start_bytes = done;
    let mut last_report = Instant::now() - Duration::from_secs(1);
    loop {
        if cancelled() {
            let _ = file.flush().await;
            return Err(Error::Cancelled);
        }
        let chunk = match resp.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => {
                let _ = file.flush().await;
                return Err(net(e));
            }
        };
        file.write_all(&chunk).await?;
        hasher.update(&chunk);
        done += chunk.len() as u64;
        if last_report.elapsed() >= Duration::from_millis(500) {
            last_report = Instant::now();
            progress(Preparing::download(
                &dl.label,
                done,
                total,
                started,
                start_bytes,
            ));
        }
    }
    file.flush().await?;
    drop(file);
    progress(Preparing::download(
        &dl.label,
        done,
        total.or(Some(done)),
        started,
        start_bytes,
    ));
    verify(dl, hasher, &final_path).await
}

async fn verify(dl: &Download, hasher: Context, final_path: &Path) -> Result<PathBuf> {
    let got = hex(hasher.finish().as_ref());
    if got != dl.expected.to_ascii_lowercase() {
        let _ = tokio::fs::remove_file(&dl.parts).await;
        return Err(Error::Other(format!(
            "{}: checksum mismatch (expected {}, got {got}); the download was removed, try again",
            dl.label, dl.expected
        )));
    }
    tracing::info!(file = %final_path.display(), "download verified");
    tokio::fs::rename(&dl.parts, final_path).await?;
    Ok(final_path.to_path_buf())
}

/// `x.tar.gz.parts` → `x.tar.gz` (a name without the suffix is kept).
pub fn finished_path(parts: &Path) -> PathBuf {
    let name = parts
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    match name.strip_suffix(".parts") {
        Some(stem) => parts.with_file_name(stem),
        None => parts.to_path_buf(),
    }
}

fn hash_file(path: &Path, kind: HashKind) -> Result<Context> {
    use std::io::Read;
    let mut ctx = kind.context();
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(ctx);
        }
        ctx.update(&buf[..n]);
    }
}

fn net(e: reqwest::Error) -> Error {
    Error::Other(format!("network: {e}"))
}

/// Fetch a small text file (a version number, a checksum list): at most
/// [`TEXT_MAX`] bytes, within [`TEXT_TIMEOUT`].
pub async fn text(http: &reqwest::Client, url: &str) -> Result<String> {
    let resp = http
        .get(url)
        .timeout(TEXT_TIMEOUT)
        .send()
        .await
        .map_err(net)?;
    if !resp.status().is_success() {
        return Err(Error::Other(format!(
            "{url}: server answered {}",
            resp.status()
        )));
    }
    let too_big = || Error::Other(format!("{url}: larger than {TEXT_MAX} bytes"));
    if resp.content_length().is_some_and(|len| len > TEXT_MAX) {
        return Err(too_big());
    }
    let mut resp = resp;
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(net)? {
        if body.len() as u64 + chunk.len() as u64 > TEXT_MAX {
            return Err(too_big());
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| Error::Other(format!("{url}: not UTF-8 text")))
}

/// The digest listed for `name` in a `sha256sum` / `sha512sum` style file
/// (`<hex>  <name>` or `<hex> *<name>`); it must have the exact length of
/// a `kind` digest.
pub fn digest_for(sums: &str, name: &str, kind: HashKind) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let digest = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name
            && digest.len() == kind.hex_len()
            && digest.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| digest.to_ascii_lowercase())
    })
}

impl Preparing {
    fn download(
        label: &str,
        done: u64,
        total: Option<u64>,
        started: Instant,
        start_bytes: u64,
    ) -> Self {
        let secs = started.elapsed().as_secs_f64().max(0.001);
        let speed = ((done - start_bytes) as f64 / secs) as u64;
        let total_bytes = total.unwrap_or(0);
        let percent = if total_bytes > 0 {
            (done as f64 / total_bytes as f64 * 100.0) as f32
        } else {
            0.0
        };
        let eta = (speed > 0 && total_bytes > done).then(|| (total_bytes - done) / speed);
        Preparing {
            message: Some(label.to_string()),
            progress: Some(DownloadProgress {
                percent,
                bytes_done: done,
                bytes_total: total_bytes,
                speed,
                disk_speed: speed,
                eta,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_digests_in_sum_files() {
        let sums = "3244fb81cfc17c83f979f0b98b4f020d0897956aac4e9aa2cbc7da9f056f08fd *BUILD_ID.txt\n\
                    c0485966093731ec59d3b53fd1639ec454d94a877531e70750d9aa40d2a2f23b *SteamLinuxRuntime_sniper.tar.xz\n";
        assert_eq!(
            digest_for(sums, "SteamLinuxRuntime_sniper.tar.xz", HashKind::Sha256).as_deref(),
            Some("c0485966093731ec59d3b53fd1639ec454d94a877531e70750d9aa40d2a2f23b")
        );
        let proton = "5e1e7c99e773b4d0757f90075c5ff7f5282bedd13a694fc0414486122e68c9452d3fb971f32ab32704d4463c9622475a44e58fbd014069aa1e84e27b4a4ed7e5  UMU-Proton-10.0-4.tar.gz\n";
        assert!(
            digest_for(proton, "UMU-Proton-10.0-4.tar.gz", HashKind::Sha512)
                .is_some_and(|d| d.len() == 128)
        );
        // the wrong kind of digest for the file is no digest
        assert_eq!(
            digest_for(proton, "UMU-Proton-10.0-4.tar.gz", HashKind::Sha256),
            None
        );
        assert_eq!(
            digest_for(sums, "SteamLinuxRuntime_sniper.tar.xz", HashKind::Sha512),
            None
        );
        assert_eq!(digest_for(sums, "other.tar.xz", HashKind::Sha256), None);
        assert_eq!(digest_for("not a digest  x", "x", HashKind::Sha256), None);
    }

    #[test]
    fn parts_suffix_is_stripped() {
        assert_eq!(
            finished_path(Path::new("/c/UMU-Proton-10.tar.gz.parts")),
            PathBuf::from("/c/UMU-Proton-10.tar.gz")
        );
        assert_eq!(
            finished_path(Path::new("/c/x.tar.xz")),
            PathBuf::from("/c/x.tar.xz")
        );
    }

    #[test]
    fn hashes_match_ring() {
        let base = std::env::temp_dir().join(format!("hyper-fetch-{}", std::process::id()));
        std::fs::create_dir_all(&base).expect("dir");
        let f = base.join("f");
        std::fs::write(&f, b"abc").expect("write");
        let d = hex(hash_file(&f, HashKind::Sha256)
            .expect("hash")
            .finish()
            .as_ref());
        assert_eq!(
            d,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
