//! GOG build metas and depot manifests (gogdl's `dl/objects/{v1,v2}.py`).
//!
//! A build's meta lists its depots per product (game, DLCs) and language;
//! each depot's manifest lists the files. Generation 2 (Galaxy 2.0) files
//! are made of zlib-compressed chunks with MD5s; generation 1 files are
//! byte ranges of one big `main.bin` per depot, hashed whole. The meta we
//! keep in `manifests/<id>` after a download carries gogdl's extra keys
//! (`HGLPlatform`, `HGLInstallLanguage`, `HGLdlcs`), so an install made by
//! either tool is readable by the other.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::{Client, GOG_CDN, galaxy_path, id_string, is_product_id, languages};
use crate::error::{Error, Result};
use crate::stores::is_confined_rel_path;

/// The product a depot's files belong to: a product id, or the shared
/// redistributables store (`redist_files`).
pub const REDIST_PRODUCT: &str = "redist";

/// One compressed piece of a generation-2 file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRef {
    pub compressed_md5: String,
    pub md5: String,
    pub size: u64,
    pub compressed_size: u64,
}

/// A file of the build, from either generation.
#[derive(Debug, Clone, Default)]
pub struct GameFile {
    /// Product id the file belongs to (`redist` for shared redistributables).
    pub product: String,
    /// Relative path with `/` separators; support files live under the
    /// product id in the support folder.
    pub path: String,
    pub support: bool,
    pub executable: bool,
    /// Size on disk.
    pub size: u64,
    /// Whole-file hashes when the manifest has them (v1 always, as `hash`).
    pub md5: Option<String>,
    pub sha256: Option<String>,
    /// Generation 2: the chunks; empty for an empty file.
    pub chunks: Vec<ChunkRef>,
    /// Generation 1: offset in the depot's `main.bin`.
    pub v1_offset: Option<u64>,
}

impl GameFile {
    /// What the resume file records for a finished file.
    pub fn checksum(&self) -> Option<String> {
        self.md5
            .clone()
            .or_else(|| self.sha256.clone())
            .or_else(|| self.chunks.first().map(|c| c.md5.clone()))
    }
}

#[derive(Debug, Clone)]
pub struct Link {
    pub path: String,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct Depot {
    /// Products the depot serves (v2: one; v1: `gameIDs`).
    pub products: Vec<String>,
    pub languages: Vec<String>,
    pub manifest: String,
    pub size: u64,
    pub compressed_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dlc {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    /// 1 or 2
    pub version: u8,
    /// The meta as GOG sent it, plus the `HGL*` keys.
    pub data: Value,
    pub platform: String,
    pub product_id: String,
    /// Install language code.
    pub language: String,
    pub dlcs: Vec<Dlc>,
    /// Depots of the game and the chosen DLCs, every language.
    pub all_depots: Vec<Depot>,
    /// Those of them in the install language.
    pub depots: Vec<Depot>,
    /// Redistributable ids the build needs.
    pub dependencies: Vec<String>,
    pub install_directory: String,
    pub build_id: String,
    /// v1: the build's timestamp, part of every CDN path.
    pub timestamp: String,
    pub files: Vec<GameFile>,
    pub dirs: Vec<String>,
    pub links: Vec<Link>,
}

/// `\` → `/`, no leading separator.
pub fn clean_path(p: &str) -> String {
    p.replace('\\', "/").trim_start_matches('/').to_string()
}

/// A manifest path cleaned and checked to stay inside the folder it is
/// joined to; GOG never sends anything else, so a bad one is an error.
fn confined(p: &str) -> Result<String> {
    let path = clean_path(p);
    if is_confined_rel_path(&path) {
        Ok(path)
    } else {
        Err(Error::Other(format!(
            "GOG manifest path is not confined to the game folder: {p:?}"
        )))
    }
}

/// The product a file is recorded under: a product id, or `redist`.
fn check_product(product: &str) -> Result<()> {
    if product == REDIST_PRODUCT || is_product_id(product) {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "GOG manifest has an unusable product id: {product:?}"
        )))
    }
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn u64_of(v: Option<&Value>) -> u64 {
    v.and_then(|x| {
        x.as_u64()
            .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
    })
    .unwrap_or(0)
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(id_string).collect())
        .unwrap_or_default()
}

impl Manifest {
    /// A build's meta for an install of `language` with `dlcs`.
    pub fn new(mut meta: Value, platform: &str, language: &str, dlcs: Vec<Dlc>) -> Result<Self> {
        let version = match meta.get("version").and_then(Value::as_u64) {
            Some(1) => 1,
            Some(2) | None => 2,
            Some(other) => {
                return Err(Error::Other(format!(
                    "unsupported GOG depot generation {other}"
                )));
            }
        };
        let language = languages::code(language);
        meta["HGLPlatform"] = platform.into();
        meta["HGLInstallLanguage"] = language.clone().into();
        meta["HGLdlcs"] = dlcs
            .iter()
            .map(|d| json!({ "id": d.id, "title": d.title }))
            .collect::<Vec<_>>()
            .into();
        let mut m = Self {
            version,
            data: meta,
            platform: platform.to_string(),
            product_id: String::new(),
            language,
            dlcs,
            all_depots: Vec::new(),
            depots: Vec::new(),
            dependencies: Vec::new(),
            install_directory: String::new(),
            build_id: String::new(),
            timestamp: String::new(),
            files: Vec::new(),
            dirs: Vec::new(),
            links: Vec::new(),
        };
        if version == 1 {
            m.parse_v1()?
        } else {
            m.parse_v2()?
        }
        Ok(m)
    }

    /// The meta saved after a download (with the `HGL*` keys).
    pub fn from_saved(meta: Value) -> Result<Self> {
        let platform = meta
            .get("HGLPlatform")
            .and_then(Value::as_str)
            .unwrap_or("windows")
            .to_string();
        let language = meta
            .get("HGLInstallLanguage")
            .and_then(Value::as_str)
            .unwrap_or("en-US")
            .to_string();
        let dlcs = meta
            .get("HGLdlcs")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|d| {
                        Some(Dlc {
                            id: d.get("id").and_then(id_string)?,
                            title: d
                                .get("title")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self::new(meta, &platform, &language, dlcs)
    }

    pub fn to_json(&self) -> &Value {
        &self.data
    }

    fn wanted(&self, product: &str, dlc_ids: &HashSet<&str>) -> bool {
        dlc_ids.contains(product) || self.product_id == product
    }

    fn parse_v2(&mut self) -> Result<()> {
        let d = &self.data;
        self.product_id = d
            .get("baseProductId")
            .and_then(id_string)
            .ok_or_else(|| Error::Other("GOG meta has no baseProductId".into()))?;
        self.install_directory = d
            .get("installDirectory")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.build_id = d.get("buildId").and_then(id_string).unwrap_or_default();
        let dlc_ids: HashSet<&str> = self.dlcs.iter().map(|x| x.id.as_str()).collect();
        let mut all = Vec::new();
        let mut deps = Vec::new();
        for depot in d
            .get("depots")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(redist) = depot.get("redist").and_then(Value::as_str) {
                deps.push(redist.to_string());
                continue;
            }
            let Some(product) = depot.get("productId").and_then(id_string) else {
                continue;
            };
            if !self.wanted(&product, &dlc_ids) {
                continue;
            }
            all.push(Depot {
                products: vec![product],
                languages: strings(depot.get("languages")),
                manifest: depot
                    .get("manifest")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                size: u64_of(depot.get("size")),
                compressed_size: u64_of(depot.get("compressedSize")),
            });
        }
        if let Some(list) = d.get("dependencies").and_then(Value::as_array) {
            for dep in list.iter().filter_map(Value::as_str) {
                if !deps.iter().any(|x| x == dep) {
                    deps.push(dep.to_string());
                }
            }
        }
        self.dependencies = deps;
        self.finish_depots(all);
        Ok(())
    }

    fn parse_v1(&mut self) -> Result<()> {
        let product = self
            .data
            .get("product")
            .cloned()
            .ok_or_else(|| Error::Other("GOG v1 meta has no product".into()))?;
        self.product_id = product
            .get("rootGameID")
            .and_then(id_string)
            .ok_or_else(|| Error::Other("GOG meta has no rootGameID".into()))?;
        self.install_directory = product
            .get("installDirectory")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.timestamp = product
            .get("timestamp")
            .and_then(id_string)
            .unwrap_or_default();
        if !self.timestamp.bytes().all(|b| b.is_ascii_digit()) {
            // part of every CDN path of the build
            return Err(Error::Other(format!(
                "GOG v1 meta has an unusable timestamp: {:?}",
                self.timestamp
            )));
        }
        let dlc_ids: HashSet<&str> = self.dlcs.iter().map(|x| x.id.as_str()).collect();
        let mut all = Vec::new();
        let mut deps = Vec::new();
        for depot in product
            .get("depots")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(redist) = depot.get("redist").and_then(Value::as_str) {
                deps.push(redist.to_string());
                continue;
            }
            let products = strings(depot.get("gameIDs"));
            if !products.iter().any(|p| self.wanted(p, &dlc_ids)) {
                continue;
            }
            let size = u64_of(depot.get("size"));
            all.push(Depot {
                products,
                languages: strings(depot.get("languages")),
                manifest: depot
                    .get("manifest")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                size,
                compressed_size: size,
            });
        }
        self.dependencies = deps;
        self.finish_depots(all);
        Ok(())
    }

    fn finish_depots(&mut self, all: Vec<Depot>) {
        self.depots = all
            .iter()
            .filter(|d| {
                d.languages
                    .iter()
                    .any(|l| languages::depot_matches(l, &self.language))
            })
            .cloned()
            .collect();
        self.all_depots = all;
    }

    /// Languages the build (game + chosen DLCs) comes in, as codes.
    pub fn languages(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for l in self.all_depots.iter().flat_map(|d| d.languages.iter()) {
            if l == "*" || l == "Neutral" {
                continue;
            }
            let code = languages::code(l);
            if seen.insert(code.clone()) {
                out.push(code);
            }
        }
        out
    }

    /// product → language code (`*` shared) → (download, disk) bytes, as
    /// `gogdl info` reports them.
    pub fn sizes(&self) -> HashMap<String, HashMap<String, (u64, u64)>> {
        let mut out: HashMap<String, HashMap<String, (u64, u64)>> = HashMap::new();
        for depot in &self.all_depots {
            for product in &depot.products {
                let per_lang = out.entry(product.clone()).or_default();
                per_lang.entry("*".into()).or_default();
                for lang in &depot.languages {
                    let key = if lang == "*" || lang == "Neutral" {
                        "*".to_string()
                    } else {
                        languages::code(lang)
                    };
                    let e = per_lang.entry(key).or_default();
                    e.0 += depot.compressed_size;
                    e.1 += depot.size;
                }
            }
        }
        out
    }

    /// The game's Galaxy client (v2 metas), for cloud saves.
    pub fn client(&self) -> Option<(String, String)> {
        let s = |k: &str| {
            self.data
                .get(k)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        Some((s("clientId")?, s("clientSecret")?))
    }

    /// Fetch the depot manifests of the install language and fill `files`,
    /// `dirs` and `links`.
    pub async fn fetch_files(&mut self, client: &Client) -> Result<()> {
        self.files.clear();
        self.dirs.clear();
        self.links.clear();
        let depots = self.depots.clone();
        for depot in depots {
            let product = depot.products.first().cloned().unwrap_or_default();
            check_product(&product)?;
            if self.version == 2 {
                let url = format!(
                    "{GOG_CDN}/content-system/v2/meta/{}",
                    galaxy_path(&depot.manifest)?
                );
                let doc = client.get_zlib_json(&url, false).await?;
                self.add_v2_items(&doc, &product)?;
            } else {
                if !is_confined_rel_path(&depot.manifest) || depot.manifest.contains(['/', '\\']) {
                    return Err(Error::Other(format!(
                        "GOG v1 depot has an unusable manifest name: {:?}",
                        depot.manifest
                    )));
                }
                let url = format!(
                    "{GOG_CDN}/content-system/v1/manifests/{product}/{}/{}/{}",
                    self.platform, self.timestamp, depot.manifest
                );
                let doc = client.get_json(&url, false).await?;
                self.add_v1_records(&doc, &product)?;
            }
        }
        Ok(())
    }

    /// Files, folders and links of a v2 depot manifest. A path that would
    /// leave the game folder fails the whole build: nothing is written.
    pub(crate) fn add_v2_items(&mut self, doc: &Value, product: &str) -> Result<()> {
        check_product(product)?;
        for item in doc
            .pointer("/depot/items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match item.get("type").and_then(Value::as_str) {
                Some("DepotFile") => self.files.push(depot_file(item, product)?),
                Some("DepotLink") => {
                    if let (Some(path), Some(target)) = (
                        item.get("path").and_then(Value::as_str),
                        item.get("target").and_then(Value::as_str),
                    ) {
                        // the link's target is joined to the game folder as well; an
                        // absolute target is refused, not made relative
                        if !is_confined_rel_path(target) {
                            return Err(Error::Other(format!(
                                "GOG depot link target is not confined to the game folder: {target:?}"
                            )));
                        }
                        self.links.push(Link {
                            path: confined(path)?,
                            target: target.replace('\\', "/"),
                        });
                    }
                }
                _ => {
                    if let Some(path) = item.get("path").and_then(Value::as_str) {
                        self.dirs
                            .push(confined(path.trim_end_matches(['/', '\\']))?);
                    }
                }
            }
        }
        Ok(())
    }

    fn add_v1_records(&mut self, doc: &Value, product: &str) -> Result<()> {
        check_product(product)?;
        for rec in doc
            .pointer("/depot/files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(path) = rec.get("path").and_then(Value::as_str) else {
                continue;
            };
            if rec.get("directory").is_some() {
                self.dirs
                    .push(confined(path.trim_end_matches(['/', '\\']))?);
                continue;
            }
            let support = rec.get("support").and_then(Value::as_bool).unwrap_or(false);
            let md5 = rec.get("hash").and_then(Value::as_str).map(str::to_owned);
            if md5.as_deref().is_some_and(|h| !is_hex(h)) {
                return Err(Error::Other(format!(
                    "GOG v1 record of {path:?} has an unusable hash"
                )));
            }
            self.files.push(GameFile {
                product: product.to_string(),
                path: confined(path)?,
                support,
                executable: rec
                    .get("executable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                size: u64_of(rec.get("size")),
                md5,
                sha256: None,
                chunks: Vec::new(),
                v1_offset: Some(u64_of(rec.get("offset"))),
            });
        }
        Ok(())
    }
}

/// A generation-2 `DepotFile` item (game depots and redistributables).
/// Support files go under the product id in the support folder.
pub fn depot_file(item: &Value, product: &str) -> Result<GameFile> {
    check_product(product)?;
    let flags: Vec<&str> = item
        .get("flags")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let support = flags.contains(&"support");
    let mut path = confined(item.get("path").and_then(Value::as_str).unwrap_or_default())?;
    if support {
        path = format!("{product}/{path}");
    }
    let mut chunks: Vec<ChunkRef> = Vec::new();
    let mut sizes: HashMap<String, u64> = HashMap::new();
    for c in item
        .get("chunks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let chunk = ChunkRef {
            compressed_md5: c
                .get("compressedMd5")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            md5: c
                .get("md5")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            size: u64_of(c.get("size")),
            compressed_size: u64_of(c.get("compressedSize")),
        };
        // the ids are CDN paths and cache keys: hex only, one size per id
        if !is_hex(&chunk.compressed_md5) || !is_hex(&chunk.md5) {
            return Err(Error::Other(format!(
                "GOG manifest chunk of {path:?} has an unusable id"
            )));
        }
        if sizes
            .insert(chunk.compressed_md5.clone(), chunk.size)
            .is_some_and(|seen| seen != chunk.size)
        {
            return Err(Error::Other(format!(
                "GOG manifest chunk {} of {path:?} is listed with two sizes",
                chunk.compressed_md5
            )));
        }
        chunks.push(chunk);
    }
    let size = chunks
        .iter()
        .try_fold(0u64, |acc, c| acc.checked_add(c.size))
        .ok_or_else(|| Error::Other(format!("GOG manifest file {path:?} is impossibly large")))?;
    Ok(GameFile {
        product: product.to_string(),
        path,
        support,
        executable: flags.contains(&"executable"),
        size,
        md5: item.get("md5").and_then(Value::as_str).map(str::to_owned),
        sha256: item
            .get("sha256")
            .and_then(Value::as_str)
            .map(str::to_owned),
        chunks,
        v1_offset: None,
    })
}

/// A changed file: written anew, or assembled partly from the old file.
#[derive(Debug, Clone)]
pub struct Changed {
    pub file: GameFile,
    pub old_support: bool,
    /// Per chunk of `file`: its offset in the old file when the old file
    /// already holds it (same MD5).
    pub reuse: Vec<Option<u64>>,
}

#[derive(Debug, Default)]
pub struct Diff {
    pub new: Vec<GameFile>,
    pub changed: Vec<Changed>,
    pub deleted: Vec<GameFile>,
    pub redist: Vec<GameFile>,
    pub removed_redist: Vec<GameFile>,
    pub links: Vec<Link>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.new.is_empty()
            && self.changed.is_empty()
            && self.deleted.is_empty()
            && self.redist.is_empty()
            && self.removed_redist.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "new {} changed {} deleted {} redist {} removed redist {}",
            self.new.len(),
            self.changed.len(),
            self.deleted.len(),
            self.redist.len(),
            self.removed_redist.len()
        )
    }
}

/// Which chunks of `new` the old file already has (gogdl's `FileDiff`).
fn partial(new: &GameFile, old: &GameFile) -> Changed {
    let mut old_offsets: HashMap<&str, u64> = HashMap::new();
    let mut off = 0u64;
    for c in &old.chunks {
        old_offsets.entry(c.md5.as_str()).or_insert(off);
        off = off.saturating_add(c.size);
    }
    let reuse = new
        .chunks
        .iter()
        .map(|c| old_offsets.get(c.md5.as_str()).copied())
        .collect();
    Changed {
        file: new.clone(),
        old_support: old.support,
        reuse,
    }
}

/// `ManifestDiff.compare` of both generations: by lower-cased path.
pub fn compare(new: &Manifest, old: Option<&Manifest>) -> Diff {
    let mut diff = Diff {
        links: new.links.clone(),
        ..Diff::default()
    };
    let Some(old) = old else {
        diff.new = new.files.clone();
        return diff;
    };
    let new_files: HashMap<String, &GameFile> = new
        .files
        .iter()
        .map(|f| (f.path.to_lowercase(), f))
        .collect();
    let old_files: HashMap<String, &GameFile> = old
        .files
        .iter()
        .map(|f| (f.path.to_lowercase(), f))
        .collect();
    for (key, f) in &old_files {
        if !new_files.contains_key(key) {
            diff.deleted.push((*f).clone());
        }
    }
    if new.version == 1 && old.version == 2 {
        // a downgrade of the depot format: everything is written anew
        diff.new = new.files.clone();
        return diff;
    }
    let mut ordered: Vec<&GameFile> = new.files.iter().collect();
    ordered.sort_by_key(|f| f.path.to_lowercase());
    for file in ordered {
        let Some(old_file) = old_files.get(&file.path.to_lowercase()) else {
            diff.new.push(file.clone());
            continue;
        };
        if new.version == 1 {
            if file.md5 != old_file.md5 {
                diff.changed.push(Changed {
                    file: file.clone(),
                    old_support: old_file.support,
                    reuse: Vec::new(),
                });
            }
            continue;
        }
        if old.version == 1 {
            // v1 → v2 upgrade: compare the whole-file hash
            if file.chunks.is_empty() {
                continue;
            }
            let new_sum = file
                .md5
                .clone()
                .or_else(|| file.chunks.first().map(|c| c.md5.clone()));
            if new_sum.is_some() && old_file.md5 != new_sum {
                diff.changed.push(Changed {
                    file: file.clone(),
                    old_support: old_file.support,
                    reuse: Vec::new(),
                });
            }
            continue;
        }
        if file.chunks.len() == 1 && old_file.chunks.len() == 1 {
            if file.chunks[0].md5 != old_file.chunks[0].md5 {
                diff.changed.push(Changed {
                    file: file.clone(),
                    old_support: old_file.support,
                    reuse: Vec::new(),
                });
            }
        } else if (file.md5.is_some() && old_file.md5.is_some() && file.md5 != old_file.md5)
            || (file.sha256.is_some()
                && old_file.sha256.is_some()
                && file.sha256 != old_file.sha256)
            || file.chunks.len() != old_file.chunks.len()
        {
            diff.changed.push(partial(file, old_file));
        }
    }
    diff
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn v2_meta() -> Value {
        json!({
            "version": 2,
            "baseProductId": "100",
            "buildId": "b100",
            "clientId": "cid",
            "clientSecret": "csec",
            "installDirectory": "Game",
            "dependencies": ["DirectX"],
            "products": [
                { "productId": "100", "name": "Game" },
                { "productId": "200", "name": "DLC" }
            ],
            "depots": [
                { "productId": "100", "languages": ["*"], "manifest": "aa11", "size": 1000, "compressedSize": 600 },
                { "productId": "100", "languages": ["en-US"], "manifest": "bb22", "size": 100, "compressedSize": 50 },
                { "productId": "100", "languages": ["de-DE"], "manifest": "cc33", "size": 120, "compressedSize": 70 },
                { "productId": "200", "languages": ["*"], "manifest": "dd44", "size": 300, "compressedSize": 200 },
                { "redist": "DirectX", "size": 5 }
            ]
        })
    }

    pub(crate) fn file(path: &str, chunks: &[(&str, u64)]) -> GameFile {
        let chunks: Vec<ChunkRef> = chunks
            .iter()
            .map(|(md5, size)| ChunkRef {
                compressed_md5: format!("c{md5}"),
                md5: md5.to_string(),
                size: *size,
                compressed_size: *size / 2,
            })
            .collect();
        GameFile {
            product: "100".into(),
            path: path.into(),
            size: chunks.iter().map(|c| c.size).sum(),
            chunks,
            ..GameFile::default()
        }
    }

    fn manifest(version: u8, files: Vec<GameFile>) -> Manifest {
        let meta = if version == 2 {
            v2_meta()
        } else {
            json!({ "version": 1, "product": { "rootGameID": "100", "installDirectory": "Game", "timestamp": "1", "depots": [] } })
        };
        let mut m = Manifest::new(meta, "windows", "en-US", vec![]).unwrap();
        m.files = files;
        m
    }

    #[test]
    fn v2_meta_picks_depots_by_language_and_dlc() {
        let m = Manifest::new(
            v2_meta(),
            "windows",
            "English",
            vec![Dlc {
                id: "200".into(),
                title: "DLC".into(),
            }],
        )
        .unwrap();
        assert_eq!(m.language, "en-US");
        assert_eq!(m.all_depots.len(), 4);
        assert_eq!(
            m.depots
                .iter()
                .map(|d| d.manifest.as_str())
                .collect::<Vec<_>>(),
            ["aa11", "bb22", "dd44"]
        );
        assert_eq!(m.dependencies, ["DirectX"]);
        assert_eq!(m.languages(), ["en-US", "de-DE"]);
        let sizes = m.sizes();
        assert_eq!(sizes["100"]["*"], (600, 1000));
        assert_eq!(sizes["100"]["en-US"], (50, 100));
        assert_eq!(sizes["200"]["*"], (200, 300));
        assert_eq!(m.client(), Some(("cid".into(), "csec".into())));
        assert_eq!(m.data["HGLdlcs"][0]["id"], "200");
        // without the DLC its depot is left out
        let base = Manifest::new(v2_meta(), "windows", "en-US", vec![]).unwrap();
        assert_eq!(base.all_depots.len(), 3);
        let again = Manifest::from_saved(m.data.clone()).unwrap();
        assert_eq!(again.dlcs, m.dlcs);
        assert_eq!(again.language, "en-US");
    }

    #[test]
    fn v1_meta_uses_language_names() {
        let meta = json!({
            "version": 1,
            "product": {
                "rootGameID": "100", "installDirectory": "Old Game", "timestamp": "123",
                "gameIDs": [{ "gameID": "100", "name": { "en": "Old Game" } }],
                "depots": [
                    { "gameIDs": ["100"], "languages": ["Neutral"], "manifest": "m1.json", "size": "500" },
                    { "gameIDs": ["100"], "languages": ["English"], "manifest": "m2.json", "size": "50" },
                    { "gameIDs": ["100"], "languages": ["German"], "manifest": "m3.json", "size": "60" },
                    { "redist": "MSVC2010", "targetDir": "", "size": 1 }
                ]
            }
        });
        let m = Manifest::new(meta, "windows", "en-US", vec![]).unwrap();
        assert_eq!(m.version, 1);
        assert_eq!(m.timestamp, "123");
        assert_eq!(
            m.depots
                .iter()
                .map(|d| d.manifest.as_str())
                .collect::<Vec<_>>(),
            ["m1.json", "m2.json"]
        );
        assert_eq!(m.languages(), ["en-US", "de-DE"]);
        assert_eq!(m.sizes()["100"]["en-US"], (50, 50));
        assert_eq!(m.dependencies, ["MSVC2010"]);
    }

    #[test]
    fn depot_items_become_files_dirs_and_links() {
        let doc = json!({ "depot": { "items": [
            { "type": "DepotFile", "path": "bin\\game.exe", "flags": ["executable"], "md5": "ff",
              "chunks": [{ "compressedMd5": "c1", "md5": "a1", "size": 10, "compressedSize": 5 }] },
            { "type": "DepotFile", "path": "/setup.txt", "flags": ["support"], "chunks": [] },
            { "type": "DepotDirectory", "path": "saves/" },
            { "type": "DepotLink", "path": "link", "target": "bin/game.exe" }
        ] } });
        let mut m = manifest(2, vec![]);
        m.add_v2_items(&doc, "100").unwrap();
        assert_eq!(m.files.len(), 2);
        assert_eq!(m.files[0].path, "bin/game.exe");
        assert!(m.files[0].executable && !m.files[0].support);
        assert_eq!(m.files[0].size, 10);
        assert_eq!(m.files[0].checksum().as_deref(), Some("ff"));
        assert_eq!(m.files[1].path, "100/setup.txt");
        assert!(m.files[1].support);
        assert_eq!(m.dirs, ["saves"]);
        assert_eq!(m.links[0].target, "bin/game.exe");
    }

    #[test]
    fn depot_items_outside_the_game_folder_are_refused() {
        let items = |item: Value| json!({ "depot": { "items": [item] } });
        let chunk = json!([{ "compressedMd5": "c1", "md5": "a1", "size": 1, "compressedSize": 1 }]);
        let bad = [
            json!({ "type": "DepotFile", "path": "..\\evil.exe", "chunks": chunk }),
            json!({ "type": "DepotFile", "path": "C:\\Windows\\evil.exe", "chunks": chunk }),
            json!({ "type": "DepotFile", "path": "bin/../../x", "chunks": chunk }),
            json!({ "type": "DepotFile", "path": "nul", "chunks": chunk }),
            json!({ "type": "DepotDirectory", "path": "../up" }),
            json!({ "type": "DepotLink", "path": "ok", "target": "/etc/passwd" }),
            json!({ "type": "DepotLink", "path": "../link", "target": "ok" }),
            // chunk ids are CDN paths
            json!({ "type": "DepotFile", "path": "ok.bin", "chunks": [{ "compressedMd5": "../x", "md5": "a1", "size": 1, "compressedSize": 1 }] }),
            // one id, two sizes
            json!({ "type": "DepotFile", "path": "ok.bin", "chunks": [
                { "compressedMd5": "c1", "md5": "a1", "size": 1, "compressedSize": 1 },
                { "compressedMd5": "c1", "md5": "a1", "size": 2, "compressedSize": 1 }
            ] }),
        ];
        for item in bad {
            let mut m = manifest(2, vec![]);
            assert!(
                m.add_v2_items(&items(item.clone()), "100").is_err(),
                "{item}"
            );
        }
        // the product id becomes a folder name for support files
        let mut m = manifest(2, vec![]);
        assert!(m.add_v2_items(&items(json!({ "type": "DepotFile", "path": "s.txt", "flags": ["support"], "chunks": [] })), "../100").is_err());
        assert!(
            depot_file(
                &json!({ "type": "DepotFile", "path": "s.txt", "chunks": [] }),
                REDIST_PRODUCT
            )
            .is_ok()
        );
        // v1 records are checked the same way
        let mut v1 = manifest(1, vec![]);
        assert!(v1.add_v1_records(&json!({ "depot": { "files": [{ "path": "../x", "size": 1, "offset": 0, "hash": "ab" }] } }), "100").is_err());
        assert!(v1.add_v1_records(&json!({ "depot": { "files": [{ "path": "x", "size": 1, "offset": 0, "hash": "zz" }] } }), "100").is_err());
        assert!(v1.add_v1_records(&json!({ "depot": { "files": [{ "path": "x", "size": 1, "offset": 0, "hash": "ab" }] } }), "x100").is_err());
        v1.add_v1_records(&json!({ "depot": { "files": [{ "path": "dir\\x", "size": 1, "offset": 0, "hash": "ab" }, { "path": "d/", "directory": true }] } }), "100").unwrap();
        assert_eq!(v1.files[0].path, "dir/x");
        assert_eq!(v1.dirs, ["d"]);
    }

    #[test]
    fn compare_finds_new_changed_deleted_and_reusable_chunks() {
        let old = manifest(
            2,
            vec![
                file("a.bin", &[("x", 10), ("y", 10)]),
                file("gone.bin", &[("g", 1)]),
                file("same.bin", &[("s", 1)]),
            ],
        );
        let new = manifest(
            2,
            vec![
                file("A.BIN", &[("x", 10), ("z", 10), ("y", 10)]),
                file("new.bin", &[("n", 1)]),
                file("same.bin", &[("s", 1)]),
            ],
        );
        let d = compare(&new, Some(&old));
        assert_eq!(
            d.new.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["new.bin"]
        );
        assert_eq!(
            d.deleted
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["gone.bin"]
        );
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].reuse, [Some(0), None, Some(10)]);
        // no old manifest: everything is new
        assert_eq!(compare(&new, None).new.len(), 3);
        // single-chunk files compare by chunk hash
        let old1 = manifest(2, vec![file("a", &[("x", 1)])]);
        let new1 = manifest(2, vec![file("a", &[("y", 1)])]);
        let d1 = compare(&new1, Some(&old1));
        assert_eq!(d1.changed.len(), 1);
        assert!(d1.changed[0].reuse.is_empty());
    }
}
