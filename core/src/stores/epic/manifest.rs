//! Epic's build manifests (`.manifest`): the files of a build, the chunks
//! they are assembled from, and how the game is launched. Binary format as
//! read by legendary's `models/manifest.py` (header magic `0x44BEC00C`, zlib
//! body with meta, chunk list, file list and custom fields) plus the JSON
//! form very old builds still use. Newer builds (version 22+) may keep the
//! launch executable and the file names in an AES-GCM block; the key comes
//! with the manifest record from Epic (`decrypt`). Saved files are either
//! the bytes the CDN served or, once decrypted, our own serialisation,
//! which legendary reads as well.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;

use flate2::read::ZlibDecoder;
use serde_json::Value;
use sha1::{Digest, Sha1};

use crate::error::{Error, Result};
use crate::stores::is_confined_rel_path;

const HEADER_MAGIC: u32 = 0x44BE_C00C;
/// Caps for counts read from the file, so a damaged one cannot ask for
/// gigabytes of allocations.
const MAX_ELEMENTS: usize = 4_000_000;
/// The most a manifest body (or its encrypted block) may inflate to.
const MAX_BODY: usize = 256 << 20;

#[derive(Debug, Clone, Default)]
pub struct Manifest {
    /// Serialisation version of the header (JSON: `ManifestFileVersion`).
    pub version: u32,
    pub meta: Meta,
    pub chunks: Vec<ChunkInfo>,
    pub files: Vec<FileManifest>,
    pub custom_fields: BTreeMap<String, String>,
    /// Names the secret in the record's `secrets` (version 22+); zero once
    /// decrypted or never encrypted.
    pub secret_guid: [u32; 4],
    pub encryption_tag: [u8; 16],
    /// Launch exe and file names are still in `encrypted_data`.
    pub encrypted: bool,
    pub(crate) encrypted_data: Option<EncryptedData>,
}

/// The AES-GCM block of an encrypted manifest (feature level 24+).
#[derive(Debug, Clone, Default)]
pub(crate) struct EncryptedData {
    compressed: bool,
    iv: Vec<u8>,
    ciphertext: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub data_version: u8,
    pub feature_level: u32,
    pub is_file_data: bool,
    pub app_id: u32,
    pub app_name: String,
    pub build_version: String,
    pub launch_exe: String,
    pub launch_command: String,
    pub prereq_ids: Vec<String>,
    pub prereq_name: String,
    pub prereq_path: String,
    pub prereq_args: String,
    /// Stored from data version 1 on; older builds derive it (not needed here).
    pub build_id: String,
    pub uninstall_action_path: String,
    pub uninstall_action_args: String,
}

#[derive(Debug, Clone, Default)]
pub struct ChunkInfo {
    pub guid: [u32; 4],
    pub hash: u64,
    pub sha: [u8; 20],
    pub group_num: u8,
    /// Uncompressed size.
    pub window_size: u32,
    /// Compressed size: what gets downloaded.
    pub file_size: i64,
    /// Version 22+: the key the chunk is sealed with (zero = plain), the
    /// compressed size of its window and its GCM tag (also in the chunk file).
    pub secret_guid: [u32; 4],
    pub window_size_compressed: u32,
    pub encryption_tag: [u8; 16],
}

impl ChunkInfo {
    /// Where the chunk lives below a base URL / inside a cloud save.
    pub fn path(&self, manifest_version: u32) -> String {
        super::chunk::chunk_path(
            self.guid,
            self.hash,
            self.group_num,
            manifest_version,
            self.secret_guid,
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct FileManifest {
    pub filename: String,
    /// Where a symbolic link points (macOS builds); carried through the
    /// writer and otherwise ignored: files are always written as data, no
    /// link is ever created from it.
    pub symlink_target: String,
    pub sha: [u8; 20],
    pub flags: u8,
    pub install_tags: Vec<String>,
    pub chunk_parts: Vec<ChunkPart>,
    pub file_size: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ChunkPart {
    pub guid: [u32; 4],
    pub offset: u32,
    pub size: u32,
    pub file_offset: u64,
}

impl Manifest {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let m = if data.first() == Some(&b'{') {
            Self::parse_json(data)?
        } else {
            Self::parse_binary(data)?
        };
        m.validate()?;
        Ok(m)
    }

    /// Nothing in the manifest may point outside the folder it is applied
    /// to: file names, the launch executable, the prerequisite and
    /// uninstaller paths are checked against [`is_confined_rel_path`]; the
    /// chunk parts must fit their chunk. The names of an encrypted manifest
    /// are checked once `decrypt` filled them in.
    pub fn validate(&self) -> Result<()> {
        let windows: HashMap<[u32; 4], u32> = self
            .chunks
            .iter()
            .map(|c| (c.guid, c.window_size))
            .collect();
        for c in &self.chunks {
            if c.window_size as usize > super::chunk::CHUNK_SIZE {
                return Err(bad(&format!("chunk window of {} bytes", c.window_size)));
            }
        }
        for f in &self.files {
            if !self.encrypted && !is_confined_rel_path(&f.filename) {
                return Err(bad(&format!("unsafe file name {:?}", f.filename)));
            }
            for p in &f.chunk_parts {
                let Some(window) = windows.get(&p.guid) else {
                    return Err(bad(&format!(
                        "{:?} uses a chunk the manifest does not list",
                        f.filename
                    )));
                };
                if p.offset.checked_add(p.size).is_none_or(|end| end > *window) {
                    return Err(bad(&format!(
                        "chunk part of {:?} runs past its chunk",
                        f.filename
                    )));
                }
            }
        }
        if !self.encrypted {
            for (what, path) in [
                ("launch executable", &self.meta.launch_exe),
                ("prerequisite path", &self.meta.prereq_path),
                ("uninstaller path", &self.meta.uninstall_action_path),
            ] {
                if !path.is_empty() && !is_confined_rel_path(path.trim_start_matches(['/', '\\'])) {
                    return Err(bad(&format!("unsafe {what} {path:?}")));
                }
            }
        }
        Ok(())
    }

    /// Bytes to download: the compressed size of every chunk.
    pub fn download_size(&self) -> u64 {
        self.chunks.iter().map(|c| c.file_size.max(0) as u64).sum()
    }

    /// Bytes on disk once installed.
    pub fn disk_size(&self) -> u64 {
        self.files.iter().map(|f| f.file_size).sum()
    }

    /// Open the encrypted part (launch exe, file names) with the key the
    /// manifest record's `secrets` hold for this build. A manifest that is
    /// not encrypted is left alone.
    pub fn decrypt(&mut self, secrets: &HashMap<String, String>) -> Result<()> {
        if !self.encrypted {
            return Ok(());
        }
        let key = secrets
            .get(&super::chunk::secret_hex(self.secret_guid))
            .ok_or(Error::NotImplemented(
                "encrypted Epic manifests without their key",
            ))?;
        let ed = self
            .encrypted_data
            .take()
            .ok_or_else(|| bad("encrypted without an encrypted block"))?;
        let mut plain =
            super::chunk::aes_gcm_open(key, &ed.iv, &ed.ciphertext, &self.encryption_tag)?;
        if ed.compressed {
            plain = inflate(&plain, MAX_BODY)?;
        }
        let mut r = Reader::new(&plain);
        self.meta.launch_exe = r.fstring()?;
        self.meta.launch_command = r.fstring()?;
        let n = r.count()?;
        self.meta.prereq_ids = (0..n).map(|_| r.fstring()).collect::<Result<_>>()?;
        self.meta.prereq_name = r.fstring()?;
        self.meta.prereq_path = r.fstring()?;
        self.meta.prereq_args = r.fstring()?;
        self.meta.uninstall_action_path = r.fstring()?;
        self.meta.uninstall_action_args = r.fstring()?;
        for f in &mut self.files {
            f.filename = r.fstring()?;
            f.symlink_target = r.fstring()?;
        }
        self.encrypted = false;
        self.secret_guid = [0; 4];
        self.encryption_tag = [0; 16];
        self.validate()
    }

    /// Files that belong to an install with `tags` selected: all of them
    /// when no tags were chosen, else the untagged ones plus those tagged.
    pub fn files_for_tags<'a>(
        &'a self,
        tags: &'a [String],
    ) -> impl Iterator<Item = &'a FileManifest> + 'a {
        self.files.iter().filter(move |f| {
            tags.is_empty()
                || f.install_tags.is_empty()
                || f.install_tags.iter().any(|t| tags.contains(t))
        })
    }

    /// Serialise like legendary's `Manifest.write` (compressed, version 18
    /// or the feature level if higher). Used for the save-game manifests we
    /// upload; CDN manifests are kept as downloaded.
    pub fn write(&self) -> Vec<u8> {
        use self::{write_block as block, write_fstring as fstring};
        use std::io::Write;
        let mut version = self.meta.feature_level.max(17);
        if self.meta.data_version >= 2 {
            version = version.max(21);
        } else if self.meta.data_version == 1 {
            version = version.max(18);
        }
        let version = version.min(24);
        let meta = &self.meta;
        let mut body = Vec::new();
        body.extend(block(|b| {
            b.push(meta.data_version);
            b.extend_from_slice(&version.to_le_bytes());
            b.push(meta.is_file_data as u8);
            b.extend_from_slice(&meta.app_id.to_le_bytes());
            for s in [
                &meta.app_name,
                &meta.build_version,
                &meta.launch_exe,
                &meta.launch_command,
            ] {
                fstring(b, s);
            }
            b.extend_from_slice(&(meta.prereq_ids.len() as u32).to_le_bytes());
            for p in &meta.prereq_ids {
                fstring(b, p);
            }
            for s in [&meta.prereq_name, &meta.prereq_path, &meta.prereq_args] {
                fstring(b, s);
            }
            if meta.data_version >= 1 {
                fstring(b, &meta.build_id);
            }
            if meta.data_version >= 2 {
                fstring(b, &meta.uninstall_action_path);
                fstring(b, &meta.uninstall_action_args);
            }
        }));
        body.extend(block(|b| {
            b.push(0);
            b.extend_from_slice(&(self.chunks.len() as u32).to_le_bytes());
            for c in &self.chunks {
                for g in c.guid {
                    b.extend_from_slice(&g.to_le_bytes());
                }
            }
            for c in &self.chunks {
                b.extend_from_slice(&c.hash.to_le_bytes());
            }
            for c in &self.chunks {
                b.extend_from_slice(&c.sha);
            }
            for c in &self.chunks {
                b.push(c.group_num);
            }
            for c in &self.chunks {
                b.extend_from_slice(&c.window_size.to_le_bytes());
            }
            for c in &self.chunks {
                b.extend_from_slice(&c.file_size.to_le_bytes());
            }
            if version >= 22 {
                for c in &self.chunks {
                    for g in c.secret_guid {
                        b.extend_from_slice(&g.to_le_bytes());
                    }
                }
                for c in &self.chunks {
                    b.extend_from_slice(&c.window_size_compressed.to_le_bytes());
                }
                for c in &self.chunks {
                    b.extend_from_slice(&c.encryption_tag);
                }
            }
        }));
        body.extend(block(|b| {
            b.push(0); // no md5 / mime / sha256 kept
            b.extend_from_slice(&(self.files.len() as u32).to_le_bytes());
            for f in &self.files {
                fstring(b, &f.filename);
            }
            for f in &self.files {
                fstring(b, &f.symlink_target);
            }
            for f in &self.files {
                b.extend_from_slice(&f.sha);
            }
            for f in &self.files {
                b.push(f.flags);
            }
            for f in &self.files {
                b.extend_from_slice(&(f.install_tags.len() as u32).to_le_bytes());
                for t in &f.install_tags {
                    fstring(b, t);
                }
            }
            for f in &self.files {
                b.extend_from_slice(&(f.chunk_parts.len() as u32).to_le_bytes());
                for p in &f.chunk_parts {
                    b.extend_from_slice(&28u32.to_le_bytes());
                    for g in p.guid {
                        b.extend_from_slice(&g.to_le_bytes());
                    }
                    b.extend_from_slice(&p.offset.to_le_bytes());
                    b.extend_from_slice(&p.size.to_le_bytes());
                }
            }
        }));
        body.extend(block(|b| {
            b.push(0);
            b.extend_from_slice(&(self.custom_fields.len() as u32).to_le_bytes());
            for k in self.custom_fields.keys() {
                fstring(b, k);
            }
            for v in self.custom_fields.values() {
                fstring(b, v);
            }
        }));
        if version >= 24 {
            // the encrypted block, empty once decrypted (legendary writes it too)
            let ed = self.encrypted_data.clone().unwrap_or_default();
            body.extend(block(|b| {
                b.push(0);
                let mut inner = Vec::new();
                let header_len = (4 + 4 + 1 + 4 + 4 + 4 + ed.iv.len()) as u32;
                inner.extend_from_slice(&header_len.to_le_bytes());
                inner.extend_from_slice(&0u32.to_le_bytes());
                inner.push(ed.compressed as u8);
                inner.extend_from_slice(&0u32.to_le_bytes());
                inner.extend_from_slice(&(ed.ciphertext.len() as u32).to_le_bytes());
                inner.extend_from_slice(&(ed.iv.len() as u32).to_le_bytes());
                inner.extend_from_slice(&ed.iv);
                inner.extend_from_slice(&(ed.ciphertext.len() as u32).to_le_bytes());
                inner.extend_from_slice(&ed.ciphertext);
                b.extend_from_slice(&(inner.len() as u32).to_le_bytes());
                b.extend(inner);
            }));
        }
        let sha: [u8; 20] = Sha1::digest(&body).into();
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = enc.write_all(&body);
        let compressed = enc.finish().unwrap_or_default();
        let header_size: u32 = if version >= 22 { 73 } else { 41 };
        let mut out = Vec::with_capacity(header_size as usize + compressed.len());
        out.extend_from_slice(&HEADER_MAGIC.to_le_bytes());
        out.extend_from_slice(&header_size.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&sha);
        out.push(if self.encrypted { 0x3 } else { 0x1 });
        out.extend_from_slice(&version.to_le_bytes());
        if version >= 22 {
            for g in self.secret_guid {
                out.extend_from_slice(&g.to_le_bytes());
            }
            out.extend_from_slice(&self.encryption_tag);
        }
        out.extend(compressed);
        out
    }

    fn parse_binary(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        if r.u32()? != HEADER_MAGIC {
            return Err(bad("no header magic"));
        }
        let header_size = r.u32()? as usize;
        let size_uncompressed = r.u32()? as usize;
        let _size_compressed = r.u32()?;
        let sha: [u8; 20] = r.bytes(20)?.try_into().map_err(|_| bad("short hash"))?;
        let stored_as = r.u8()?;
        let version = r.u32()?;
        let mut secret_guid = [0u32; 4];
        let mut encryption_tag = [0u8; 16];
        if version >= 22 {
            secret_guid = r.guid()?;
            encryption_tag.copy_from_slice(r.bytes(16)?);
        }
        if r.pos != header_size {
            tracing::warn!(
                read = r.pos,
                header_size,
                version,
                "manifest header size differs"
            );
        }
        let body = data
            .get(header_size..)
            .ok_or_else(|| bad("truncated header"))?;
        let body: Vec<u8> = if stored_as & 0x1 != 0 {
            let out = inflate(body, size_uncompressed.min(MAX_BODY))?;
            if Sha1::digest(&out).as_slice() != sha {
                return Err(bad("body hash mismatch"));
            }
            out
        } else {
            body.to_vec()
        };

        let mut b = Reader::new(&body);
        let meta = Meta::read(&mut b)?;
        let chunks = read_chunk_list(&mut b, meta.feature_level)?;
        let files = read_file_list(&mut b)?;
        let custom_fields = read_custom_fields(&mut b)?;
        let encrypted_data = if meta.feature_level >= 24 {
            read_encrypted_data(&mut b)?
        } else {
            None
        };
        Ok(Self {
            version,
            meta,
            chunks,
            files,
            custom_fields,
            secret_guid,
            encryption_tag,
            encrypted: stored_as & 0x2 != 0,
            encrypted_data,
        })
    }

    fn parse_json(data: &[u8]) -> Result<Self> {
        let v: Value = serde_json::from_slice(data)?;
        let s = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let version = blob_to_num(
            v.get("ManifestFileVersion")
                .and_then(Value::as_str)
                .unwrap_or("013000000000"),
        ) as u32;
        let meta = Meta {
            data_version: 0,
            feature_level: version,
            is_file_data: v
                .get("bIsFileData")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            app_id: blob_to_num(
                v.get("AppID")
                    .and_then(Value::as_str)
                    .unwrap_or("000000000000"),
            ) as u32,
            app_name: s("AppNameString"),
            build_version: s("BuildVersionString"),
            launch_exe: s("LaunchExeString"),
            launch_command: s("LaunchCommand"),
            prereq_ids: v
                .get("PrereqIds")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            prereq_name: s("PrereqName"),
            prereq_path: s("PrereqPath"),
            prereq_args: s("PrereqArgs"),
            ..Meta::default()
        };
        let obj = |k: &str| v.get(k).and_then(Value::as_object);
        let (Some(sizes), Some(hashes), Some(shas), Some(groups)) = (
            obj("ChunkFilesizeList"),
            obj("ChunkHashList"),
            obj("ChunkShaList"),
            obj("DataGroupList"),
        ) else {
            return Err(bad("JSON manifest without chunk lists"));
        };
        let blob = |m: &serde_json::Map<String, Value>, k: &str| {
            blob_to_num(m.get(k).and_then(Value::as_str).unwrap_or(""))
        };
        let mut chunks = Vec::with_capacity(sizes.len());
        for guid in sizes.keys() {
            chunks.push(ChunkInfo {
                guid: guid_from_hex(guid)?,
                hash: blob(hashes, guid),
                sha: hex20(shas.get(guid).and_then(Value::as_str).unwrap_or(""))?,
                group_num: blob(groups, guid) as u8,
                window_size: 1024 * 1024,
                file_size: blob(sizes, guid) as i64,
                ..ChunkInfo::default()
            });
        }
        let mut files = Vec::new();
        for f in v
            .get("FileManifestList")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let fs = |k: &str| f.get(k).and_then(Value::as_str).unwrap_or_default();
            let flag = |k: &str, bit: u8| {
                if f.get(k).and_then(Value::as_bool).unwrap_or(false) {
                    bit
                } else {
                    0
                }
            };
            let mut fm = FileManifest {
                filename: fs("Filename").to_string(),
                sha: blob_bytes20(fs("FileHash")),
                flags: flag("bIsReadOnly", 1)
                    | flag("bIsCompressed", 2)
                    | flag("bIsUnixExecutable", 4),
                install_tags: f
                    .get("InstallTags")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                ..FileManifest::default()
            };
            let mut offset = 0u64;
            for p in f
                .get("FileChunkParts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let ps = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("");
                let size = blob_to_num(ps("Size")) as u32;
                fm.chunk_parts.push(ChunkPart {
                    guid: guid_from_hex(ps("Guid"))?,
                    offset: blob_to_num(ps("Offset")) as u32,
                    size,
                    file_offset: offset,
                });
                offset += size as u64;
            }
            fm.file_size = offset;
            files.push(fm);
        }
        let custom_fields = obj("CustomFields")
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            version,
            meta,
            chunks,
            files,
            custom_fields,
            ..Self::default()
        })
    }
}

fn bad(what: &str) -> Error {
    Error::Other(format!("manifest: {what}"))
}

/// zlib-inflate at most `limit` bytes; more than that is a damaged (or
/// hostile) manifest, not something to allocate for.
fn inflate(compressed: &[u8], limit: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(limit.min(16 << 20));
    ZlibDecoder::new(compressed)
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| bad(&format!("zlib: {e}")))?;
    if out.len() > limit {
        return Err(bad(&format!("inflates past {limit} bytes")));
    }
    Ok(out)
}

/// Unreal `FString` as legendary writes it: NUL-terminated, the length in
/// front; UTF-16 with a negative length when not ASCII.
pub(crate) fn write_fstring(out: &mut Vec<u8>, s: &str) {
    if s.is_empty() {
        out.extend_from_slice(&0i32.to_le_bytes());
    } else if s.is_ascii() {
        out.extend_from_slice(&((s.len() + 1) as i32).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    } else {
        let units: Vec<u16> = s.encode_utf16().collect();
        out.extend_from_slice(&(-((units.len() + 1) as i32)).to_le_bytes());
        for u in units {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
    }
}

/// A block with its own size in front.
pub(crate) fn write_block(body: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut b = vec![0u8; 4];
    body(&mut b);
    let size = (b.len() as u32).to_le_bytes();
    b[..4].copy_from_slice(&size);
    b
}

impl Meta {
    fn read(r: &mut Reader) -> Result<Self> {
        let start = r.pos;
        let meta_size = r.u32()? as usize;
        let mut m = Meta {
            data_version: r.u8()?,
            feature_level: r.u32()?,
            is_file_data: r.u8()? == 1,
            app_id: r.u32()?,
            ..Meta::default()
        };
        m.app_name = r.fstring()?;
        m.build_version = r.fstring()?;
        m.launch_exe = r.fstring()?;
        m.launch_command = r.fstring()?;
        let n = r.count()?;
        for _ in 0..n {
            m.prereq_ids.push(r.fstring()?);
        }
        m.prereq_name = r.fstring()?;
        m.prereq_path = r.fstring()?;
        m.prereq_args = r.fstring()?;
        if m.data_version >= 1 {
            m.build_id = r.fstring()?;
        }
        if m.data_version >= 2 {
            m.uninstall_action_path = r.fstring()?;
            m.uninstall_action_args = r.fstring()?;
        }
        r.seek_block_end(start, meta_size, "meta")?;
        Ok(m)
    }
}

/// The chunk list stores each field for all chunks in turn, not chunk by chunk.
fn read_chunk_list(r: &mut Reader, manifest_version: u32) -> Result<Vec<ChunkInfo>> {
    let start = r.pos;
    let size = r.u32()? as usize;
    let _version = r.u8()?;
    // a chunk takes at least its guid
    let count = r.count_of(16)?;
    let mut chunks: Vec<ChunkInfo> = (0..count).map(|_| ChunkInfo::default()).collect();
    for c in &mut chunks {
        c.guid = r.guid()?;
    }
    for c in &mut chunks {
        c.hash = r.u64()?;
    }
    for c in &mut chunks {
        c.sha = r
            .bytes(20)?
            .try_into()
            .map_err(|_| bad("short chunk hash"))?;
    }
    for c in &mut chunks {
        c.group_num = r.u8()?;
    }
    for c in &mut chunks {
        c.window_size = r.u32()?;
    }
    for c in &mut chunks {
        c.file_size = r.i64()?;
    }
    if manifest_version >= 22 {
        for c in &mut chunks {
            c.secret_guid = r.guid()?;
        }
        for c in &mut chunks {
            c.window_size_compressed = r.u32()?;
        }
        for c in &mut chunks {
            c.encryption_tag.copy_from_slice(r.bytes(16)?);
        }
    }
    r.seek_block_end(start, size, "chunk list")?;
    Ok(chunks)
}

fn read_file_list(r: &mut Reader) -> Result<Vec<FileManifest>> {
    let start = r.pos;
    let size = r.u32()? as usize;
    let version = r.u8()?;
    // a file takes at least two empty strings, its hash, flags and two counts
    let count = r.count_of(25)?;
    let mut files: Vec<FileManifest> = (0..count).map(|_| FileManifest::default()).collect();
    for f in &mut files {
        f.filename = r.fstring()?;
    }
    for f in &mut files {
        f.symlink_target = r.fstring()?;
    }
    for f in &mut files {
        f.sha = r
            .bytes(20)?
            .try_into()
            .map_err(|_| bad("short file hash"))?;
    }
    for f in &mut files {
        f.flags = r.u8()?;
    }
    for f in &mut files {
        let n = r.count()?;
        for _ in 0..n {
            f.install_tags.push(r.fstring()?);
        }
    }
    for f in &mut files {
        let n = r.count()?;
        let mut offset = 0u64;
        for _ in 0..n {
            let part_start = r.pos;
            let part_size = r.u32()? as usize;
            let part = ChunkPart {
                guid: r.guid()?,
                offset: r.u32()?,
                size: r.u32()?,
                file_offset: offset,
            };
            offset += part.size as u64;
            f.chunk_parts.push(part);
            let read = r.pos - part_start;
            if part_size > read {
                r.skip(part_size - read)?;
            }
        }
        f.file_size = offset;
    }
    if version >= 1 {
        for _ in &files {
            if r.u32()? != 0 {
                r.skip(16)?; // md5
            }
        }
        for _ in &files {
            r.fstring()?; // mime type
        }
    }
    if version >= 2 {
        r.skip(files.len() * 32)?; // sha256
    }
    r.seek_block_end(start, size, "file list")?;
    Ok(files)
}

/// The encrypted block (feature level 24+): a header with the IV, then the
/// ciphertext. Missing at the end of a manifest = not there.
fn read_encrypted_data(r: &mut Reader) -> Result<Option<EncryptedData>> {
    if r.data.len() - r.pos < 9 {
        return Ok(None);
    }
    let start = r.pos;
    let size = r.u32()? as usize;
    let _version = r.u8()?;
    let cipher_size = r.u32()? as usize;
    let inner = r.bytes(cipher_size.min(r.data.len() - r.pos))?;
    let mut i = Reader::new(inner);
    let header_start = i.pos;
    let header_size = i.u32()? as usize;
    let _header_version = i.u32()?;
    let stored_as = i.u8()?;
    let _data_uncompressed = i.u32()?;
    let _data_compressed = i.u32()?;
    let iv_len = i.u32()? as usize;
    let iv = i.bytes(iv_len)?.to_vec();
    i.seek_block_end(header_start, header_size, "encrypted header")?;
    let ciphertext_size = i.u32()? as usize;
    let ciphertext = i.bytes(ciphertext_size)?.to_vec();
    r.seek_block_end(start, size, "encrypted data")?;
    Ok(Some(EncryptedData {
        compressed: stored_as & 0x1 != 0,
        iv,
        ciphertext,
    }))
}

fn read_custom_fields(r: &mut Reader) -> Result<BTreeMap<String, String>> {
    let start = r.pos;
    let size = r.u32()? as usize;
    let _version = r.u8()?;
    let count = r.count()?;
    let keys: Vec<String> = (0..count).map(|_| r.fstring()).collect::<Result<_>>()?;
    let values: Vec<String> = (0..count).map(|_| r.fstring()).collect::<Result<_>>()?;
    r.seek_block_end(start, size, "custom fields")?;
    Ok(keys.into_iter().zip(values).collect())
}

/// Little-endian reader over the manifest body.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.data.len())
            .ok_or_else(|| bad("unexpected end"))?;
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.bytes(4)?.try_into().map_err(|_| bad("short u32"))?,
        ))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.bytes(8)?.try_into().map_err(|_| bad("short u64"))?,
        ))
    }

    fn i64(&mut self) -> Result<i64> {
        Ok(self.u64()? as i64)
    }

    fn guid(&mut self) -> Result<[u32; 4]> {
        Ok([self.u32()?, self.u32()?, self.u32()?, self.u32()?])
    }

    /// An element count: bounded, so a damaged file cannot ask for the moon.
    /// Every element is at least a 4-byte string length.
    fn count(&mut self) -> Result<usize> {
        self.count_of(4)
    }

    /// A count of records at least `min_width` bytes each: no more than
    /// the rest of the data can hold.
    fn count_of(&mut self, min_width: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        let remaining = self.data.len() - self.pos;
        if n > MAX_ELEMENTS || n.checked_mul(min_width).is_none_or(|need| need > remaining) {
            return Err(bad("implausible element count"));
        }
        Ok(n)
    }

    /// Unreal `FString`: length-prefixed, NUL-terminated; a negative length
    /// means UTF-16 (the length counts characters).
    fn fstring(&mut self) -> Result<String> {
        let len = self.i32()?;
        if len < 0 {
            let chars = (-(len as i64)) as usize;
            let raw = self.bytes(chars.checked_mul(2).ok_or_else(|| bad("string length"))?)?;
            let units: Vec<u16> = raw[..raw.len() - 2]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            Ok(String::from_utf16_lossy(&units))
        } else if len > 0 {
            let raw = self.bytes(len as usize)?;
            Ok(String::from_utf8_lossy(&raw[..raw.len() - 1]).into_owned())
        } else {
            Ok(String::new())
        }
    }

    /// Every block carries its own size: skip (or warn about) what we did
    /// not understand, as legendary does.
    fn seek_block_end(&mut self, start: usize, size: usize, what: &str) -> Result<()> {
        let end = start.checked_add(size).ok_or_else(|| bad("block size"))?;
        if end > self.data.len() {
            return Err(bad(&format!("{what} runs past the end")));
        }
        if self.pos != end {
            tracing::warn!(
                what,
                read = self.pos - start,
                size,
                "manifest block size differs; skipping the rest"
            );
            self.pos = end;
        }
        Ok(())
    }
}

/// JSON manifests store numbers as three decimal digits per byte, little
/// endian (`"013000000000"` → 13).
pub(crate) fn blob_to_num(s: &str) -> u64 {
    let mut num: u64 = 0;
    for (i, chunk) in s.as_bytes().chunks(3).take(8).enumerate() {
        let b: u64 = std::str::from_utf8(chunk)
            .ok()
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        num |= b << (8 * i);
    }
    num
}

/// The same encoding for a 20-byte hash (60 digits).
fn blob_bytes20(s: &str) -> [u8; 20] {
    let mut out = [0u8; 20];
    for (i, chunk) in s.as_bytes().chunks(3).take(20).enumerate() {
        out[i] = std::str::from_utf8(chunk)
            .ok()
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
    }
    out
}

/// 32 hex digits → four big-endian u32s (the JSON form of a chunk guid).
fn guid_from_hex(s: &str) -> Result<[u32; 4]> {
    if s.len() != 32 {
        return Err(bad("guid length"));
    }
    let mut out = [0u32; 4];
    for (i, part) in out.iter_mut().enumerate() {
        *part = u32::from_str_radix(&s[i * 8..i * 8 + 8], 16).map_err(|_| bad("guid hex"))?;
    }
    Ok(out)
}

fn hex20(s: &str) -> Result<[u8; 20]> {
    if s.len() != 40 {
        return Err(bad("sha length"));
    }
    let mut out = [0u8; 20];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| bad("sha hex"))?;
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// Test-only writer mirroring legendary's `Manifest.write`, so the
    /// parser is checked against the format legendary produces.
    pub(crate) fn build_binary(
        meta: &Meta,
        chunks: &[ChunkInfo],
        files: &[FileManifest],
        custom: &[(&str, &str)],
        version: u32,
    ) -> Vec<u8> {
        use super::{write_block as block, write_fstring as fstring};
        let mut data = Vec::new();
        data.extend(block(|b| {
            b.push(meta.data_version);
            b.extend_from_slice(&meta.feature_level.to_le_bytes());
            b.push(meta.is_file_data as u8);
            b.extend_from_slice(&meta.app_id.to_le_bytes());
            for s in [
                &meta.app_name,
                &meta.build_version,
                &meta.launch_exe,
                &meta.launch_command,
            ] {
                fstring(b, s);
            }
            b.extend_from_slice(&(meta.prereq_ids.len() as u32).to_le_bytes());
            for p in &meta.prereq_ids {
                fstring(b, p);
            }
            for s in [&meta.prereq_name, &meta.prereq_path, &meta.prereq_args] {
                fstring(b, s);
            }
            if meta.data_version >= 1 {
                fstring(b, &meta.build_id);
            }
            if meta.data_version >= 2 {
                fstring(b, &meta.uninstall_action_path);
                fstring(b, &meta.uninstall_action_args);
            }
        }));
        data.extend(block(|b| {
            b.push(0);
            b.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
            for c in chunks {
                for g in c.guid {
                    b.extend_from_slice(&g.to_le_bytes());
                }
            }
            for c in chunks {
                b.extend_from_slice(&c.hash.to_le_bytes());
            }
            for c in chunks {
                b.extend_from_slice(&c.sha);
            }
            for c in chunks {
                b.push(c.group_num);
            }
            for c in chunks {
                b.extend_from_slice(&c.window_size.to_le_bytes());
            }
            for c in chunks {
                b.extend_from_slice(&c.file_size.to_le_bytes());
            }
            if version >= 22 {
                for _ in chunks {
                    b.extend_from_slice(&[0u8; 16]);
                }
                for _ in chunks {
                    b.extend_from_slice(&[0u8; 4]);
                }
                for _ in chunks {
                    b.extend_from_slice(&[0u8; 16]);
                }
            }
        }));
        data.extend(block(|b| {
            b.push(2); // FML version 2: md5 + mime + sha256
            b.extend_from_slice(&(files.len() as u32).to_le_bytes());
            for f in files {
                fstring(b, &f.filename);
            }
            for f in files {
                fstring(b, &f.symlink_target);
            }
            for f in files {
                b.extend_from_slice(&f.sha);
            }
            for f in files {
                b.push(f.flags);
            }
            for f in files {
                b.extend_from_slice(&(f.install_tags.len() as u32).to_le_bytes());
                for t in &f.install_tags {
                    fstring(b, t);
                }
            }
            for f in files {
                b.extend_from_slice(&(f.chunk_parts.len() as u32).to_le_bytes());
                for p in &f.chunk_parts {
                    b.extend_from_slice(&28u32.to_le_bytes());
                    for g in p.guid {
                        b.extend_from_slice(&g.to_le_bytes());
                    }
                    b.extend_from_slice(&p.offset.to_le_bytes());
                    b.extend_from_slice(&p.size.to_le_bytes());
                }
            }
            for _ in files {
                b.extend_from_slice(&1u32.to_le_bytes());
                b.extend_from_slice(&[7u8; 16]);
            }
            for _ in files {
                fstring(b, "application/octet-stream");
            }
            for _ in files {
                b.extend_from_slice(&[9u8; 32]);
            }
        }));
        data.extend(block(|b| {
            b.push(0);
            b.extend_from_slice(&(custom.len() as u32).to_le_bytes());
            for (k, _) in custom {
                fstring(b, k);
            }
            for (_, v) in custom {
                fstring(b, v);
            }
        }));

        let sha: [u8; 20] = Sha1::digest(&data).into();
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&data).unwrap();
        let compressed = enc.finish().unwrap();
        let header_size: u32 = if version >= 22 { 73 } else { 41 };
        let mut out = Vec::new();
        out.extend_from_slice(&HEADER_MAGIC.to_le_bytes());
        out.extend_from_slice(&header_size.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&sha);
        out.push(1); // compressed
        out.extend_from_slice(&version.to_le_bytes());
        if version >= 22 {
            out.extend_from_slice(&[0u8; 32]);
        }
        out.extend(compressed);
        out
    }

    pub(crate) fn sample() -> (Meta, Vec<ChunkInfo>, Vec<FileManifest>) {
        let meta = Meta {
            data_version: 2,
            feature_level: 21,
            app_name: "Catnip".into(),
            build_version: "1.2.3-r42".into(),
            launch_exe: "Bin/Catnip.exe".into(),
            launch_command: "-nolauncher".into(),
            prereq_ids: vec!["vcredist".into()],
            prereq_name: "VC++".into(),
            prereq_path: "redist/vc.exe".into(),
            prereq_args: "/q".into(),
            build_id: "abc".into(),
            uninstall_action_path: "unins.exe".into(),
            ..Meta::default()
        };
        let chunks = vec![
            ChunkInfo {
                guid: [1, 2, 3, 4],
                hash: 99,
                sha: [1; 20],
                group_num: 7,
                window_size: 1 << 20,
                file_size: 700,
                ..ChunkInfo::default()
            },
            ChunkInfo {
                guid: [5, 6, 7, 8],
                hash: 98,
                sha: [2; 20],
                group_num: 8,
                window_size: 1 << 20,
                file_size: 300,
                ..ChunkInfo::default()
            },
        ];
        let files = vec![
            FileManifest {
                filename: "Bin/Catnip.exe".into(),
                sha: [3; 20],
                flags: 4,
                chunk_parts: vec![
                    ChunkPart {
                        guid: [1, 2, 3, 4],
                        offset: 0,
                        size: 1000,
                        file_offset: 0,
                    },
                    ChunkPart {
                        guid: [5, 6, 7, 8],
                        offset: 10,
                        size: 24,
                        file_offset: 1000,
                    },
                ],
                file_size: 1024,
                ..FileManifest::default()
            },
            FileManifest {
                filename: "Content/Ünïcode.pak".into(),
                install_tags: vec!["hd".into()],
                chunk_parts: vec![ChunkPart {
                    guid: [5, 6, 7, 8],
                    offset: 0,
                    size: 10,
                    file_offset: 0,
                }],
                file_size: 10,
                ..FileManifest::default()
            },
        ];
        (meta, chunks, files)
    }

    #[test]
    fn parses_a_binary_manifest() {
        let (meta, chunks, files) = sample();
        for version in [18u32, 21, 22] {
            let bytes = build_binary(&meta, &chunks, &files, &[("key", "value")], version);
            let m = Manifest::parse(&bytes).unwrap_or_else(|e| panic!("v{version}: {e}"));
            assert_eq!(m.version, version);
            assert_eq!(m.meta.app_name, "Catnip");
            assert_eq!(m.meta.launch_exe, "Bin/Catnip.exe");
            assert_eq!(m.meta.prereq_ids, ["vcredist"]);
            assert_eq!(m.meta.uninstall_action_path, "unins.exe");
            assert_eq!(m.chunks.len(), 2);
            assert_eq!(m.chunks[1].guid, [5, 6, 7, 8]);
            assert_eq!(m.download_size(), 1000);
            assert_eq!(m.files.len(), 2);
            assert_eq!(m.files[1].filename, "Content/Ünïcode.pak", "UTF-16 strings");
            assert_eq!(m.files[0].file_size, 1024);
            assert_eq!(m.files[0].chunk_parts[1].file_offset, 1000);
            assert_eq!(m.disk_size(), 1034);
            assert_eq!(
                m.custom_fields.get("key").map(String::as_str),
                Some("value")
            );
            let all: Vec<&str> = m.files_for_tags(&[]).map(|f| f.filename.as_str()).collect();
            assert_eq!(all.len(), 2);
            let untagged = ["".to_string()];
            let base: Vec<&str> = m
                .files_for_tags(&untagged)
                .map(|f| f.filename.as_str())
                .collect();
            assert_eq!(base, ["Bin/Catnip.exe"], "untagged files only");
        }
    }

    #[test]
    fn writer_round_trips_through_the_parser() {
        let (meta, chunks, files) = sample();
        let m = Manifest {
            version: 21,
            meta,
            chunks,
            files,
            custom_fields: [("CloudSaveFolder".to_string(), "{AppData}/x".to_string())].into(),
            ..Manifest::default()
        };
        let bytes = m.write();
        let back = Manifest::parse(&bytes).unwrap();
        assert_eq!(back.version, 21, "data version 2 needs 21");
        assert_eq!(back.meta.app_name, m.meta.app_name);
        assert_eq!(back.meta.uninstall_action_path, "unins.exe");
        assert_eq!(back.chunks.len(), 2);
        assert_eq!(back.files[1].filename, "Content/Ünïcode.pak");
        assert_eq!(back.files[0].chunk_parts.len(), 2);
        assert_eq!(back.disk_size(), m.disk_size());
        assert_eq!(back.custom_fields["CloudSaveFolder"], "{AppData}/x");
    }

    #[test]
    fn rejects_damage() {
        let (meta, chunks, files) = sample();
        let mut bytes = build_binary(&meta, &chunks, &files, &[], 18);
        assert!(Manifest::parse(&bytes[..30]).is_err(), "truncated");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        assert!(Manifest::parse(&bytes).is_err(), "corrupt body");
        let mut encrypted = build_binary(&meta, &chunks, &files, &[], 22);
        encrypted[36] |= 0x2;
        let m = Manifest::parse(&encrypted).unwrap();
        assert!(m.encrypted);
        assert!(
            matches!(
                m.clone().decrypt(&HashMap::new()),
                Err(Error::NotImplemented(_))
            ),
            "no key"
        );
    }

    #[test]
    fn encrypted_manifests_decrypt_with_the_record_secret() {
        use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
        let (meta, chunks, files) = sample();
        // what Epic hides: launch exe + command, prereqs, uninstaller, file names
        let mut plain = Vec::new();
        let fstr = |out: &mut Vec<u8>, s: &str| {
            out.extend_from_slice(&((s.len() + 1) as i32).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
            out.push(0);
        };
        let empty = |out: &mut Vec<u8>| out.extend_from_slice(&0i32.to_le_bytes());
        fstr(&mut plain, &meta.launch_exe);
        fstr(&mut plain, &meta.launch_command);
        plain.extend_from_slice(&1u32.to_le_bytes());
        fstr(&mut plain, "vcredist");
        fstr(&mut plain, &meta.prereq_name);
        fstr(&mut plain, &meta.prereq_path);
        fstr(&mut plain, &meta.prereq_args);
        fstr(&mut plain, &meta.uninstall_action_path);
        empty(&mut plain);
        fstr(&mut plain, "Bin/Catnip.exe");
        empty(&mut plain);
        fstr(&mut plain, "Content/Pak.pak");
        empty(&mut plain);
        let key_bytes = [9u8; 32];
        let iv = [5u8; 12];
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key_bytes).unwrap());
        let mut ciphertext = plain.clone();
        let tag = key
            .seal_in_place_separate_tag(
                Nonce::assume_unique_for_key(iv),
                Aad::empty(),
                &mut ciphertext,
            )
            .unwrap();
        let mut hidden = Manifest {
            version: 24,
            meta: Meta {
                feature_level: 24,
                launch_exe: String::new(),
                launch_command: String::new(),
                prereq_ids: vec![],
                prereq_name: String::new(),
                prereq_path: String::new(),
                prereq_args: String::new(),
                uninstall_action_path: String::new(),
                ..meta.clone()
            },
            chunks: chunks.clone(),
            files: files
                .iter()
                .map(|f| FileManifest {
                    filename: String::new(),
                    ..f.clone()
                })
                .collect(),
            custom_fields: Default::default(),
            secret_guid: [0xA, 0xB, 0xC, 0xD],
            encryption_tag: tag.as_ref().try_into().unwrap(),
            encrypted: true,
            encrypted_data: Some(EncryptedData {
                compressed: false,
                iv: iv.to_vec(),
                ciphertext,
            }),
        };
        hidden.chunks[0].secret_guid = [0xA, 0xB, 0xC, 0xD];
        let bytes = hidden.write();
        let mut m = Manifest::parse(&bytes).unwrap();
        assert!(m.encrypted && m.meta.launch_exe.is_empty());
        assert_eq!(
            m.chunks[0].secret_guid,
            [0xA, 0xB, 0xC, 0xD],
            "chunk keys survive"
        );
        let mut secrets = HashMap::new();
        secrets.insert(
            "0000000A0000000B0000000C0000000D".to_string(),
            key_bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
        );
        m.decrypt(&secrets).unwrap();
        assert!(!m.encrypted);
        assert_eq!(m.meta.launch_exe, "Bin/Catnip.exe");
        assert_eq!(m.meta.prereq_ids, ["vcredist"]);
        assert_eq!(m.files[1].filename, "Content/Pak.pak");
        // saved decrypted, it parses back plain
        let again = Manifest::parse(&m.write()).unwrap();
        assert!(!again.encrypted);
        assert_eq!(again.files[0].filename, "Bin/Catnip.exe");
        assert_eq!(again.chunks[0].path(24), m.chunks[0].path(24));
    }

    #[test]
    fn refuses_paths_that_leave_the_folder() {
        let (meta, chunks, files) = sample();
        for bad_name in [
            "../x.exe",
            "C:/Windows/x",
            "/etc/passwd",
            "Bin/../../x",
            "a\\..\\b",
        ] {
            let mut f = files.clone();
            f[0].filename = bad_name.into();
            let bytes = build_binary(&meta, &chunks, &f, &[], 18);
            assert!(Manifest::parse(&bytes).is_err(), "file name {bad_name}");
        }
        for (field, value) in [
            ("exe", "../../cmd.exe"),
            ("prereq", "..\\evil.exe"),
            ("uninstall", "C:\\x.exe"),
        ] {
            let mut m = meta.clone();
            match field {
                "exe" => m.launch_exe = value.into(),
                "prereq" => m.prereq_path = value.into(),
                _ => m.uninstall_action_path = value.into(),
            }
            let bytes = build_binary(&m, &chunks, &files, &[], 18);
            assert!(Manifest::parse(&bytes).is_err(), "{field} {value}");
        }
        // a leading slash on the launch exe is what some builds ship
        let mut m = meta.clone();
        m.launch_exe = "/Bin/Catnip.exe".into();
        assert!(Manifest::parse(&build_binary(&m, &chunks, &files, &[], 18)).is_ok());
        // empty optional paths stay allowed
        let mut m = meta.clone();
        m.prereq_path = String::new();
        m.uninstall_action_path = String::new();
        assert!(Manifest::parse(&build_binary(&m, &chunks, &files, &[], 18)).is_ok());
        // a chunk part past its window, or of an unknown chunk
        let mut f = files.clone();
        f[0].chunk_parts[0].size = (1 << 20) + 1;
        assert!(
            Manifest::parse(&build_binary(&meta, &chunks, &f, &[], 18)).is_err(),
            "part past the window"
        );
        let mut f = files.clone();
        f[0].chunk_parts[0].offset = u32::MAX;
        assert!(
            Manifest::parse(&build_binary(&meta, &chunks, &f, &[], 18)).is_err(),
            "offset + size overflows"
        );
        let mut f = files.clone();
        f[0].chunk_parts[0].guid = [9, 9, 9, 9];
        assert!(
            Manifest::parse(&build_binary(&meta, &chunks, &f, &[], 18)).is_err(),
            "unknown chunk"
        );
        let mut c = chunks.clone();
        c[0].window_size = (1 << 20) + 1;
        assert!(
            Manifest::parse(&build_binary(&meta, &c, &files, &[], 18)).is_err(),
            "window over 1 MiB"
        );
        // JSON manifests go through the same checks
        let json = r#"{ "ManifestFileVersion": "013000000000", "AppNameString": "X", "LaunchExeString": "../x.exe",
          "FileManifestList": [], "ChunkHashList": {}, "ChunkShaList": {}, "DataGroupList": {}, "ChunkFilesizeList": {} }"#;
        assert!(Manifest::parse(json.as_bytes()).is_err());
    }

    #[test]
    fn bounded_inflate_and_counts() {
        // a body that inflates to more than the header claims is refused
        let (meta, chunks, files) = sample();
        let mut bytes = build_binary(&meta, &chunks, &files, &[], 18);
        bytes[8..12].copy_from_slice(&10u32.to_le_bytes()); // size_uncompressed = 10
        assert!(
            Manifest::parse(&bytes).is_err(),
            "inflates past the declared size"
        );
        assert!(inflate(&[], 10).is_err(), "not zlib");
        // a count the data cannot hold
        let mut r = Reader::new(&[0xff, 0xff, 0, 0, 0, 0, 0, 0]);
        assert!(r.count_of(16).is_err());
        let mut r = Reader::new(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(r.count_of(4).unwrap(), 2);
    }

    #[test]
    fn parses_a_json_manifest() {
        let json = r#"{
          "ManifestFileVersion": "013000000000", "bIsFileData": false, "AppID": "000000000000",
          "AppNameString": "OldGame", "BuildVersionString": "1.0", "LaunchExeString": "Game.exe", "LaunchCommand": "",
          "PrereqIds": [], "PrereqName": "", "PrereqPath": "", "PrereqArgs": "",
          "FileManifestList": [
            { "Filename": "Game.exe",
              "FileHash": "001002003004005006007008009010011012013014015016017018019020",
              "FileChunkParts": [ { "Guid": "0000000A0000000B0000000C0000000D", "Offset": "000000000000", "Size": "016039000000" } ],
              "bIsUnixExecutable": true }
          ],
          "ChunkHashList": { "0000000A0000000B0000000C0000000D": "255255255255255255255255" },
          "ChunkShaList": { "0000000A0000000B0000000C0000000D": "0102030405060708090a0b0c0d0e0f1011121314" },
          "DataGroupList": { "0000000A0000000B0000000C0000000D": "042000000000" },
          "ChunkFilesizeList": { "0000000A0000000B0000000C0000000D": "000001000000" },
          "CustomFields": { "a": "b" }
        }"#;
        let m = Manifest::parse(json.as_bytes()).unwrap();
        assert_eq!(m.version, 13);
        assert_eq!(m.meta.app_name, "OldGame");
        assert_eq!(m.chunks[0].guid, [0xA, 0xB, 0xC, 0xD]);
        assert_eq!(m.chunks[0].hash, u64::MAX);
        assert_eq!(m.chunks[0].group_num, 42);
        assert_eq!(m.chunks[0].file_size, 256, "little-endian digit triplets");
        assert_eq!(m.chunks[0].sha[0..3], [1, 2, 3]);
        assert_eq!(m.files[0].file_size, 16 + 39 * 256);
        assert_eq!(m.files[0].flags, 4);
        assert_eq!(m.files[0].sha[0], 1);
        assert_eq!(m.files[0].sha[19], 20);
        assert_eq!(m.custom_fields["a"], "b");
        assert_eq!(blob_to_num("013000000000"), 13);
    }
}
