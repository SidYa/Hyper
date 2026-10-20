//! Epic's chunk files: 1 MiB blocks (zlib-compressed on the CDN and in
//! cloud saves) that game files and save games are assembled from. Format
//! from legendary's `models/chunk.py` (magic `0xB1FE3AA2`, header versions
//! 2–4; version 4 chunks may be AES-GCM encrypted with a key from the
//! build's manifest record) and its `rolling_hash.py` (Epic's CRC-64
//! variant over the data).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::OnceLock;

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use sha1::{Digest, Sha1};

use crate::error::{Error, Result};

const HEADER_MAGIC: u32 = 0xB1FE_3AA2;
/// Every chunk holds this much uncompressed (padded with zeros).
pub const CHUNK_SIZE: usize = 1024 * 1024;
const HASH_POLY: u64 = 0xC96C_5795_D787_0F42;

#[derive(Debug, Clone)]
pub struct Chunk {
    pub guid: [u32; 4],
    /// Epic's rolling hash of the uncompressed data.
    pub hash: u64,
    pub sha: [u8; 20],
    /// Uncompressed, `CHUNK_SIZE` bytes for a chunk we made.
    pub data: Vec<u8>,
}

fn bad(what: &str) -> Error {
    Error::Other(format!("chunk: {what}"))
}

impl Chunk {
    /// A new chunk from (up to 1 MiB of) data, padded like legendary does.
    pub fn new(mut data: Vec<u8>) -> Result<Self> {
        if data.len() > CHUNK_SIZE {
            return Err(bad("more than 1 MiB of data"));
        }
        data.resize(CHUNK_SIZE, 0);
        let mut guid = [0u32; 4];
        for g in &mut guid {
            *g = rand::random();
        }
        Ok(Self {
            guid,
            hash: rolling_hash(&data),
            sha: Sha1::digest(&data).into(),
            data,
        })
    }

    /// Parse a downloaded chunk; the data is checked against the hashes the
    /// header carries. An encrypted chunk (header version 4) is opened with
    /// the key `secrets` holds for its secret guid.
    pub fn parse(raw: &[u8], secrets: &HashMap<String, String>) -> Result<Self> {
        let rd =
            |pos: usize, n: usize| raw.get(pos..pos + n).ok_or_else(|| bad("truncated header"));
        let u32_at = |pos: usize| -> Result<u32> {
            Ok(u32::from_le_bytes(
                rd(pos, 4)?.try_into().map_err(|_| bad("u32"))?,
            ))
        };
        if u32_at(0)? != HEADER_MAGIC {
            return Err(bad("no header magic"));
        }
        let header_version = u32_at(4)?;
        let header_size = u32_at(8)? as usize;
        let _compressed_size = u32_at(12)?;
        let guid = [u32_at(16)?, u32_at(20)?, u32_at(24)?, u32_at(28)?];
        let hash = u64::from_le_bytes(rd(32, 8)?.try_into().map_err(|_| bad("u64"))?);
        let stored_as = rd(40, 1)?[0];
        let mut pos = 41;
        let mut sha = [0u8; 20];
        let mut hash_type = 0u8;
        if header_version >= 2 {
            sha.copy_from_slice(rd(pos, 20)?);
            hash_type = rd(pos + 20, 1)?[0];
            pos += 21;
        }
        let mut uncompressed_size = CHUNK_SIZE;
        if header_version >= 3 {
            uncompressed_size = u32_at(pos)? as usize;
            pos += 4;
        }
        let mut secret_guid = [0u32; 4];
        let mut tag = [0u8; 16];
        if header_version >= 4 {
            secret_guid = [
                u32_at(pos)?,
                u32_at(pos + 4)?,
                u32_at(pos + 8)?,
                u32_at(pos + 12)?,
            ];
            tag.copy_from_slice(rd(pos + 16, 16)?);
            pos += 32;
        }
        if pos != header_size {
            return Err(bad("header size differs"));
        }
        let body = raw.get(header_size..).ok_or_else(|| bad("truncated"))?;
        let decrypted;
        let body: &[u8] = if stored_as & 0x2 != 0 {
            let key = secrets
                .get(&secret_hex(secret_guid))
                .ok_or(Error::NotImplemented(
                    "encrypted Epic chunks without their key",
                ))?;
            // the nonce is the start of the chunk's own SHA-1
            decrypted = aes_gcm_open(key, &sha[..12], body, &tag)?;
            &decrypted
        } else {
            body
        };
        // never more than a chunk holds, whatever the header or the zlib stream claim
        let limit = uncompressed_size.min(CHUNK_SIZE);
        let data = if stored_as & 0x1 != 0 {
            let mut out = Vec::with_capacity(limit);
            ZlibDecoder::new(body)
                .take(limit as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|e| bad(&format!("zlib: {e}")))?;
            if out.len() > limit {
                return Err(bad(&format!("inflates past {limit} bytes")));
            }
            out
        } else {
            if body.len() > limit {
                return Err(bad(&format!("{} bytes in a chunk of {limit}", body.len())));
            }
            body.to_vec()
        };
        if hash_type & 0x2 != 0 && Sha1::digest(&data).as_slice() != sha {
            return Err(bad("SHA-1 mismatch"));
        }
        if hash_type & 0x1 != 0 && rolling_hash(&data) != hash {
            return Err(bad("rolling hash mismatch"));
        }
        Ok(Self {
            guid,
            hash,
            sha,
            data,
        })
    }

    /// Is this the chunk the manifest asked for? The header's own hashes
    /// only prove the file is intact; the guid, the SHA-1 and the window
    /// size must match what the manifest lists or a swapped / truncated
    /// chunk would end up in the game files.
    pub fn check(&self, expected: &super::manifest::ChunkInfo) -> Result<()> {
        if self.guid != expected.guid {
            return Err(bad("guid differs from the manifest"));
        }
        if self.data.len() != expected.window_size as usize {
            return Err(bad(&format!(
                "{} bytes, the manifest lists {}",
                self.data.len(),
                expected.window_size
            )));
        }
        if Sha1::digest(&self.data).as_slice() != expected.sha {
            return Err(bad("SHA-1 differs from the manifest"));
        }
        Ok(())
    }

    /// The chunk as a file: header version 3, zlib body (legendary's `write`).
    pub fn write(&self) -> Vec<u8> {
        let mut enc = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = enc.write_all(&self.data);
        let compressed = enc.finish().unwrap_or_default();
        let mut out = Vec::with_capacity(66 + compressed.len());
        out.extend_from_slice(&HEADER_MAGIC.to_le_bytes());
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&66u32.to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        for g in self.guid {
            out.extend_from_slice(&g.to_le_bytes());
        }
        out.extend_from_slice(&self.hash.to_le_bytes());
        out.push(0x1); // compressed
        out.extend_from_slice(&self.sha);
        out.push(0x3); // rolling hash + sha
        out.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        out.extend(compressed);
        out
    }

    /// Where a chunk lives below the base URL / in a save, for `manifest_version`.
    pub fn path(guid: [u32; 4], hash: u64, group_num: u8, manifest_version: u32) -> String {
        chunk_path(guid, hash, group_num, manifest_version, [0; 4])
    }

    /// legendary's `group_num` for a chunk it made: CRC-32 of the guid, mod 100.
    pub fn group_num(guid: [u32; 4]) -> u8 {
        let mut bytes = Vec::with_capacity(16);
        for g in guid {
            bytes.extend_from_slice(&g.to_le_bytes());
        }
        (crc32fast::hash(&bytes) % 100) as u8
    }
}

fn chunk_dir(version: u32) -> &'static str {
    match version {
        22.. => "ChunksV5",
        15..=21 => "ChunksV4",
        6..=14 => "ChunksV3",
        3..=5 => "ChunksV2",
        _ => "Chunks",
    }
}

/// `ChunkInfo.path` in legendary: the old hex form, and from manifest
/// version 22 the base64 one whose secret part is `plain` or the chunk's
/// secret guid.
pub fn chunk_path(
    guid: [u32; 4],
    hash: u64,
    group_num: u8,
    manifest_version: u32,
    secret_guid: [u32; 4],
) -> String {
    let dir = chunk_dir(manifest_version);
    if manifest_version >= 22 {
        let le = |g: [u32; 4]| {
            let mut bytes = Vec::with_capacity(16);
            for v in g {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            bytes
        };
        let secret = if secret_guid == [0; 4] {
            "plain".to_string()
        } else {
            b64url(&le(secret_guid))
        };
        format!(
            "{dir}/{secret}/{group_num:02}/{}_{}.chunk",
            b64url(&hash.to_le_bytes()),
            b64url(&le(guid))
        )
    } else {
        let guid_hex: String = guid.iter().map(|g| format!("{g:08X}")).collect();
        format!("{dir}/{group_num:02}/{hash:016X}_{guid_hex}.chunk")
    }
}

/// A secret guid as the manifest record's `secrets` keys spell it.
pub(crate) fn secret_hex(guid: [u32; 4]) -> String {
    guid.iter().map(|g| format!("{g:08X}")).collect()
}

/// AES-256-GCM with the tag kept apart (Python's `decrypt_and_verify`).
pub(crate) fn aes_gcm_open(
    key_hex: &str,
    nonce: &[u8],
    ciphertext: &[u8],
    tag: &[u8],
) -> Result<Vec<u8>> {
    use ring::aead::{AES_128_GCM, AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
    let key = hex_bytes(key_hex).ok_or_else(|| Error::Other("Epic secret is not hex".into()))?;
    let algorithm = match key.len() {
        32 => &AES_256_GCM,
        16 => &AES_128_GCM,
        n => return Err(Error::Other(format!("Epic secret has {n} bytes"))),
    };
    let key = LessSafeKey::new(
        UnboundKey::new(algorithm, &key).map_err(|_| Error::Other("bad Epic secret".into()))?,
    );
    let nonce = Nonce::try_assume_unique_for_key(nonce)
        .map_err(|_| Error::Other("Epic nonce is not 96 bits".into()))?;
    let mut buf = Vec::with_capacity(ciphertext.len() + tag.len());
    buf.extend_from_slice(ciphertext);
    buf.extend_from_slice(tag);
    let n = key
        .open_in_place(nonce, Aad::empty(), &mut buf)
        .map_err(|_| Error::Other("Epic data does not decrypt (wrong key or damaged)".into()))?
        .len();
    buf.truncate(n);
    Ok(buf)
}

pub(crate) fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// URL-safe base64 without padding (what Epic uses in chunk names).
fn b64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let n = group
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        let chars = group.len() + 1;
        for i in 0..chars {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// Epic's rolling hash, a CRC-64-ECMA variation (`rolling_hash.py`).
pub fn rolling_hash(data: &[u8]) -> u64 {
    static TABLE: OnceLock<[u64; 256]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u64; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut v = i as u64;
            for _ in 0..8 {
                v = if v & 1 != 0 {
                    (v >> 1) ^ HASH_POLY
                } else {
                    v >> 1
                };
            }
            *slot = v;
        }
        t
    });
    data.iter()
        .fold(0u64, |h, b| h.rotate_left(1) ^ table[*b as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_round_trip() {
        let c = Chunk::new(b"hello save".to_vec()).unwrap();
        assert_eq!(c.data.len(), CHUNK_SIZE, "padded");
        let bytes = c.write();
        let none = HashMap::new();
        let back = Chunk::parse(&bytes, &none).unwrap();
        assert_eq!(back.guid, c.guid);
        assert_eq!(back.hash, c.hash);
        assert_eq!(back.sha, c.sha);
        assert_eq!(&back.data[..10], b"hello save");
        let mut bad = bytes.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0x55;
        assert!(Chunk::parse(&bad, &none).is_err());
    }

    #[test]
    fn chunks_are_checked_against_the_manifest() {
        use super::super::manifest::ChunkInfo;
        let c = Chunk::new(b"payload".to_vec()).unwrap();
        let info = ChunkInfo {
            guid: c.guid,
            hash: c.hash,
            sha: c.sha,
            window_size: CHUNK_SIZE as u32,
            ..ChunkInfo::default()
        };
        c.check(&info).unwrap();
        let other = Chunk::new(b"other".to_vec()).unwrap();
        assert!(
            other.check(&info).is_err(),
            "another chunk under the expected guid"
        );
        assert!(
            Chunk {
                guid: c.guid,
                ..other.clone()
            }
            .check(&info)
            .is_err(),
            "right guid, wrong data"
        );
        assert!(
            c.check(&ChunkInfo {
                window_size: 10,
                ..info.clone()
            })
            .is_err(),
            "size differs"
        );
        // a header claiming more than a chunk holds, and a body bigger than that
        let mut raw = c.write();
        raw[66 - 4..66].copy_from_slice(&(CHUNK_SIZE as u32 * 2).to_le_bytes());
        let back = Chunk::parse(&raw, &HashMap::new()).unwrap();
        assert_eq!(back.data.len(), CHUNK_SIZE, "capped at the chunk size");
        let big = Chunk {
            data: vec![0u8; CHUNK_SIZE + 1],
            ..c.clone()
        };
        let raw = big.write();
        assert!(
            Chunk::parse(&raw, &HashMap::new()).is_err(),
            "inflates past a chunk"
        );
    }

    #[test]
    fn encrypted_chunks_open_with_their_key() {
        use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
        let c = Chunk::new(b"secret save".to_vec()).unwrap();
        let key_bytes = [7u8; 32];
        let key_hex: String = key_bytes.iter().map(|b| format!("{b:02x}")).collect();
        // the compressed body, sealed with the chunk's sha as nonce
        let mut enc = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&c.data).unwrap();
        let mut body = enc.finish().unwrap();
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key_bytes).unwrap());
        let tag = key
            .seal_in_place_separate_tag(
                Nonce::try_assume_unique_for_key(&c.sha[..12]).unwrap(),
                Aad::empty(),
                &mut body,
            )
            .unwrap();
        let secret_guid = [1u32, 2, 3, 4];
        let mut raw = Vec::new();
        raw.extend_from_slice(&HEADER_MAGIC.to_le_bytes());
        raw.extend_from_slice(&4u32.to_le_bytes());
        raw.extend_from_slice(&98u32.to_le_bytes());
        raw.extend_from_slice(&(body.len() as u32).to_le_bytes());
        for g in c.guid {
            raw.extend_from_slice(&g.to_le_bytes());
        }
        raw.extend_from_slice(&c.hash.to_le_bytes());
        raw.push(0x3); // compressed + encrypted
        raw.extend_from_slice(&c.sha);
        raw.push(0x3);
        raw.extend_from_slice(&(CHUNK_SIZE as u32).to_le_bytes());
        for g in secret_guid {
            raw.extend_from_slice(&g.to_le_bytes());
        }
        raw.extend_from_slice(tag.as_ref());
        raw.extend(body);
        let mut secrets = HashMap::new();
        assert!(
            matches!(Chunk::parse(&raw, &secrets), Err(Error::NotImplemented(_))),
            "no key"
        );
        secrets.insert(secret_hex(secret_guid), key_hex);
        let back = Chunk::parse(&raw, &secrets).unwrap();
        assert_eq!(&back.data[..11], b"secret save");
        assert_eq!(
            secret_hex([0xDEADBEEF, 1, 2, 3]),
            "DEADBEEF000000010000000200000003"
        );
        assert!(
            chunk_path(c.guid, c.hash, 1, 22, secret_guid).contains("/AQAAAAIAAAADAAAABAAAAA/")
        );
    }

    #[test]
    fn rolling_hash_is_stable_and_bytewise() {
        assert_eq!(rolling_hash(&[0]), 0);
        assert_ne!(rolling_hash(&[1]), 0);
        assert_ne!(rolling_hash(b"ab"), rolling_hash(b"ba"));
        assert_eq!(rolling_hash(b"abc"), rolling_hash(b"abc"));
    }

    #[test]
    fn chunk_paths_like_legendary() {
        let guid = [0xDEADBEEF, 1, 2, 3];
        assert_eq!(
            chunk_path(guid, 0x1234, 7, 18, [0; 4]),
            "ChunksV4/07/0000000000001234_DEADBEEF000000010000000200000003.chunk"
        );
        assert!(chunk_path(guid, 0x1234, 7, 22, [0; 4]).starts_with("ChunksV5/plain/07/"));
        assert!(chunk_path(guid, 0x1234, 7, 22, [0; 4]).ends_with(".chunk"));
        assert_eq!(chunk_dir(13), "ChunksV3");
        assert_eq!(b64url(b"Man"), "TWFu");
        assert_eq!(b64url(b"Ma"), "TWE");
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
        assert!(Chunk::group_num(guid) < 100);
    }
}
