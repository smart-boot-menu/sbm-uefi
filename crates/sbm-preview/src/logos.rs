//! Hardware logo resolver and per-machine QOI cache manager.
//!
//! Implements the host-side stage of Smart Boot Menu's two-stage logo pipeline:
//!
//! 1. **Catalog (`smart-boot-menu/hardware-logos`):** Maximally compressed
//!    128 px AVIF 4:4:4 badges indexed by `index.json` specificity rules.
//! 2. **Machine-local ESP cache (`\EFI\SBM\cache\logos\`, simulated under
//!    `build/sim/logo-cache/`):** Resolves ONLY the detected CPU and primary GPU
//!    for this machine, transcodes each matched `.avif` once to compact `.qoi`
//!    (`rapid-qoi`), and records the mapping in `manifest.json`.
//! 3. **UEFI runtime (`sbm-gfx`):** Decodes cached `.qoi` in `#![no_std]` via
//!    [`LogoImage::from_qoi`] in microseconds, falling back cleanly to built-in
//!    vector icons whenever a badge is uncached, unmatched, or still probing.

use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use rapid_qoi::{Colors, Qoi};
use sbm_gfx::LogoImage;
use serde::{Deserialize, Serialize};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Environment override for the `hardware-logos` repository root.
pub const ENV_LOGOS_DIR: &str = "SBM_HARDWARE_LOGOS_DIR";
/// Environment override for the per-machine QOI cache directory.
pub const ENV_CACHE_DIR: &str = "SBM_LOGO_CACHE_DIR";

#[derive(Clone, Debug, Deserialize)]
pub struct LogoCatalog {
    pub schema_version: String,
    pub logos: Vec<LogoEntry>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LogoEntry {
    pub id: String,
    pub category: String,
    pub vendor: String,
    pub label: String,
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub alpha: bool,
    pub priority: i32,
    #[serde(rename = "match")]
    pub rule: MatchRule,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct MatchRule {
    #[serde(default)]
    pub all_of: Vec<String>,
    #[serde(default)]
    pub any_of: Vec<String>,
    #[serde(default)]
    pub not_any: Vec<String>,
}

/// On-disk cache manifest mirroring `\EFI\SBM\cache\logos\manifest.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CacheManifest {
    pub schema_version: String,
    pub entries: BTreeMap<String, CachedBadgeEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedBadgeEntry {
    pub id: String,
    pub sha256: String,
    pub qoi_file: String,
    pub width: u32,
    pub height: u32,
    pub qoi_bytes: usize,
}

/// Result of resolving a CPU or GPU model string against the catalog & cache.
#[derive(Clone, Debug)]
pub struct ResolvedLogo {
    pub id: String,
    pub label: String,
    pub qoi_path: PathBuf,
    pub from_cache: bool,
    pub image: LogoImage,
}

/// Memoized CPU + primary GPU badges for one simulated machine so 60 fps
/// rendering performs zero filesystem or subprocess work.
#[derive(Clone, Debug, Default)]
pub struct ResolvedHardwareLogos {
    pub cpu_id: Option<String>,
    pub cpu_logo: Option<LogoImage>,
    pub gpu_id: Option<String>,
    pub gpu_logo: Option<LogoImage>,
}

impl ResolvedHardwareLogos {
    /// Resolve CPU and primary GPU badges using the default catalog & cache.
    pub fn resolve_default(cpu_model: &str, primary_gpu: Option<&str>) -> Self {
        let Some(cache) = LogoCache::discover_default() else {
            return Self::default();
        };
        Self::resolve_with(&cache, cpu_model, primary_gpu)
    }

    /// Resolve CPU and primary GPU badges against a specific [`LogoCache`].
    pub fn resolve_with(cache: &LogoCache, cpu_model: &str, primary_gpu: Option<&str>) -> Self {
        let cpu = (!cpu_model.trim().is_empty())
            .then(|| cache.resolve("cpu", cpu_model))
            .flatten();
        let gpu = primary_gpu
            .filter(|g| !g.trim().is_empty())
            .and_then(|g| cache.resolve("gpu", g));
        Self {
            cpu_id: cpu.as_ref().map(|r| r.id.clone()),
            cpu_logo: cpu.map(|r| r.image),
            gpu_id: gpu.as_ref().map(|r| r.id.clone()),
            gpu_logo: gpu.map(|r| r.image),
        }
    }
}

/// Normalize a hardware model string for deterministic substring matching:
/// ASCII lowercase with collapsed single-space separation.
pub fn normalize_model(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_space = true;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(ch.to_ascii_lowercase());
            in_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

impl MatchRule {
    pub fn matches(&self, normalized: &str) -> bool {
        if normalized.is_empty() {
            return false;
        }
        if !self
            .all_of
            .iter()
            .all(|tok| normalized.contains(&tok.to_ascii_lowercase()))
        {
            return false;
        }
        if !self.any_of.is_empty()
            && !self
                .any_of
                .iter()
                .any(|tok| normalized.contains(&tok.to_ascii_lowercase()))
        {
            return false;
        }
        if self
            .not_any
            .iter()
            .any(|tok| normalized.contains(&tok.to_ascii_lowercase()))
        {
            return false;
        }
        true
    }
}

impl LogoCatalog {
    pub fn load(catalog_dir: &Path) -> Result<Self, String> {
        let index_path = catalog_dir.join("index.json");
        let text = fs::read_to_string(&index_path)
            .map_err(|err| format!("read {}: {err}", index_path.display()))?;
        serde_json::from_str(&text).map_err(|err| format!("parse {}: {err}", index_path.display()))
    }

    /// Return the highest-priority matching badge for `category` (`"cpu"` or `"gpu"`).
    /// Ties break deterministically by `id` ascending.
    pub fn match_hardware(&self, category: &str, model: &str) -> Option<&LogoEntry> {
        let normalized = normalize_model(model);
        if normalized.is_empty() {
            return None;
        }
        let mut best: Option<&LogoEntry> = None;
        for entry in &self.logos {
            if !entry.category.eq_ignore_ascii_case(category) {
                continue;
            }
            if !entry.rule.matches(&normalized) {
                continue;
            }
            let replace = match best {
                None => true,
                Some(prev) => {
                    entry.priority > prev.priority
                        || (entry.priority == prev.priority && entry.id < prev.id)
                }
            };
            if replace {
                best = Some(entry);
            }
        }
        best
    }
}

/// Host-side AVIF-to-QOI hardware logo cache manager.
#[derive(Clone, Debug)]
pub struct LogoCache {
    catalog_dir: PathBuf,
    cache_dir: PathBuf,
    catalog: LogoCatalog,
}

impl LogoCache {
    /// Locate the sibling `../hardware-logos` repo (or `SBM_HARDWARE_LOGOS_DIR`)
    /// and default simulator cache directory (`build/sim/logo-cache/`).
    pub fn discover_default() -> Option<Self> {
        let catalog_dir = default_catalog_dir()?;
        let cache_dir = default_cache_dir();
        Self::open(catalog_dir, cache_dir).ok()
    }

    pub fn open(catalog_dir: PathBuf, cache_dir: PathBuf) -> Result<Self, String> {
        let catalog = LogoCatalog::load(&catalog_dir)?;
        Ok(Self {
            catalog_dir,
            cache_dir,
            catalog,
        })
    }

    pub fn catalog(&self) -> &LogoCatalog {
        &self.catalog
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Resolve a hardware model string to a decoded [`LogoImage`], reading from
    /// the `.qoi` cache on hit or transcoding the canonical `.avif` once on miss.
    pub fn resolve(&self, category: &str, model: &str) -> Option<ResolvedLogo> {
        let entry = self.catalog.match_hardware(category, model)?;
        let qoi_filename = cached_qoi_filename(entry);
        let qoi_path = self.cache_dir.join(&qoi_filename);

        // Fast path: cached QOI already exists and decodes cleanly via no_std rapid-qoi.
        if let Ok(qoi_bytes) = fs::read(&qoi_path)
            && let Some(image) = LogoImage::from_qoi(&qoi_bytes)
        {
            self.record_manifest_entry(
                category,
                model,
                entry,
                &qoi_filename,
                &image,
                qoi_bytes.len(),
            );
            return Some(ResolvedLogo {
                id: entry.id.clone(),
                label: entry.label.clone(),
                qoi_path,
                from_cache: true,
                image,
            });
        }

        // Cache miss: decode canonical AVIF from catalog and encode compact QOI once.
        let avif_path = self.catalog_dir.join(&entry.path);
        if !avif_path.is_file() {
            return None;
        }
        let (width, height, rgba, has_alpha) = decode_avif_to_rgba(&avif_path)?;
        let qoi_bytes = encode_qoi(width, height, &rgba, has_alpha)?;
        let image = LogoImage::from_qoi(&qoi_bytes)?;

        if fs::create_dir_all(&self.cache_dir).is_ok() {
            let tmp_path = self.cache_dir.join(unique_tmp_name("qoi"));
            if fs::write(&tmp_path, &qoi_bytes).is_ok() {
                let _ = fs::rename(&tmp_path, &qoi_path);
            }
            self.record_manifest_entry(
                category,
                model,
                entry,
                &qoi_filename,
                &image,
                qoi_bytes.len(),
            );
        }

        Some(ResolvedLogo {
            id: entry.id.clone(),
            label: entry.label.clone(),
            qoi_path,
            from_cache: false,
            image,
        })
    }

    fn record_manifest_entry(
        &self,
        category: &str,
        model: &str,
        entry: &LogoEntry,
        qoi_file: &str,
        image: &LogoImage,
        qoi_bytes: usize,
    ) {
        let manifest_path = self.cache_dir.join("manifest.json");
        let mut manifest = fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|s| serde_json::from_str::<CacheManifest>(&s).ok())
            .unwrap_or_else(|| CacheManifest {
                schema_version: "1.0".into(),
                entries: BTreeMap::new(),
            });
        if manifest.schema_version.is_empty() {
            manifest.schema_version = "1.0".into();
        }
        let key = format!(
            "{}:{}",
            category.to_ascii_lowercase(),
            normalize_model(model)
        );
        let unchanged = manifest.entries.get(&key).is_some_and(|prev| {
            prev.id == entry.id
                && prev.sha256 == entry.sha256
                && prev.qoi_file == qoi_file
                && prev.qoi_bytes == qoi_bytes
        });
        if unchanged && manifest_path.is_file() {
            return;
        }
        manifest.entries.insert(
            key,
            CachedBadgeEntry {
                id: entry.id.clone(),
                sha256: entry.sha256.clone(),
                qoi_file: qoi_file.to_string(),
                width: image.width,
                height: image.height,
                qoi_bytes,
            },
        );
        if let Ok(json) = serde_json::to_string_pretty(&manifest) {
            let tmp = self.cache_dir.join(unique_tmp_name("manifest.json"));
            if fs::write(&tmp, json).is_ok() {
                let _ = fs::rename(&tmp, &manifest_path);
            }
        }
    }
}

pub fn default_catalog_dir() -> Option<PathBuf> {
    if let Ok(env_dir) = std::env::var(ENV_LOGOS_DIR) {
        let p = PathBuf::from(env_dir.trim());
        if p.join("index.json").is_file() {
            return Some(p);
        }
        return None;
    }
    let sibling = crate::workspace_root().join("../hardware-logos");
    if sibling.join("index.json").is_file() {
        return Some(sibling);
    }
    None
}

pub fn default_cache_dir() -> PathBuf {
    if let Ok(env_dir) = std::env::var(ENV_CACHE_DIR)
        && !env_dir.trim().is_empty()
    {
        return PathBuf::from(env_dir.trim());
    }
    crate::workspace_root().join("build/sim/logo-cache")
}

fn cached_qoi_filename(entry: &LogoEntry) -> String {
    let safe_id: String = entry
        .id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let sha_prefix: String = entry
        .sha256
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(12)
        .collect();
    if sha_prefix.is_empty() {
        format!("{safe_id}.qoi")
    } else {
        format!("{safe_id}-{sha_prefix}.qoi")
    }
}

fn unique_tmp_name(ext: &str) -> String {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    format!(".tmp-{}-{seq}.{ext}", std::process::id())
}

fn encode_qoi(width: u32, height: u32, rgba: &[u8], has_alpha: bool) -> Option<Vec<u8>> {
    if has_alpha {
        Qoi {
            width,
            height,
            colors: Colors::SrgbLinA,
        }
        .encode_alloc(rgba)
        .ok()
    } else {
        let mut rgb = Vec::with_capacity(
            (width as usize)
                .checked_mul(height as usize)?
                .checked_mul(3)?,
        );
        for px in rgba.as_chunks::<4>().0 {
            rgb.extend_from_slice(&px[..3]);
        }
        Qoi {
            width,
            height,
            colors: Colors::Srgb,
        }
        .encode_alloc(&rgb)
        .ok()
    }
}

/// Decode a canonical `.avif` badge to straight-alpha RGBA8888 using `avifdec`
/// (from `libavif`) into an ephemeral PNG, then read via the `png` crate.
fn decode_avif_to_rgba(avif_path: &Path) -> Option<(u32, u32, Vec<u8>, bool)> {
    let tmp_png = std::env::temp_dir().join(unique_tmp_name("png"));
    let status = Command::new("avifdec")
        .arg("--png-compress")
        .arg("0")
        .arg(avif_path)
        .arg(&tmp_png)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        let _ = fs::remove_file(&tmp_png);
        return None;
    }
    let png_bytes = fs::read(&tmp_png).ok();
    let _ = fs::remove_file(&tmp_png);
    let png_bytes = png_bytes?;
    decode_png_to_rgba(&png_bytes)
}

fn decode_png_to_rgba(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>, bool)> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let raw = &buf[..info.buffer_size()];
    let (w, h) = (info.width, info.height);
    let count = (w as usize).checked_mul(h as usize)?;
    let mut rgba = Vec::with_capacity(count.checked_mul(4)?);
    match info.color_type {
        png::ColorType::Rgba => {
            rgba.extend_from_slice(raw);
        }
        png::ColorType::Rgb => {
            for px in raw.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for px in raw.as_chunks::<2>().0 {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in raw {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
        }
        png::ColorType::Indexed => return None,
    }
    let has_alpha = rgba.as_chunks::<4>().0.iter().any(|px| px[3] < 250);
    Some((w, h, rgba, has_alpha))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_catalog() -> LogoCatalog {
        serde_json::from_str(
            r#"{
              "schema_version": "1.0",
              "logos": [
                {
                  "id": "amd-ryzen-9-7000",
                  "category": "cpu",
                  "vendor": "amd",
                  "label": "AMD Ryzen 9 7000 Series",
                  "path": "cpu/amd-ryzen-9-7000.avif",
                  "width": 128,
                  "height": 113,
                  "bytes": 1626,
                  "sha256": "a644072f4498758988938af18f1db21000dc0376b7947664304460aa26ac7036",
                  "alpha": false,
                  "priority": 300,
                  "match": { "all_of": ["ryzen 9"], "any_of": [" 7", "7000"], "not_any": [] }
                },
                {
                  "id": "amd-ryzen-9",
                  "category": "cpu",
                  "vendor": "amd",
                  "label": "AMD Ryzen 9",
                  "path": "cpu/amd-ryzen-9.avif",
                  "width": 128,
                  "height": 113,
                  "bytes": 1912,
                  "sha256": "ff07c2e98e938176d2043a4813cf5f2a7c683951457e59656ef1588be3426427",
                  "alpha": false,
                  "priority": 200,
                  "match": { "all_of": ["ryzen 9"], "any_of": [], "not_any": [] }
                },
                {
                  "id": "amd-radeon-graphics",
                  "category": "gpu",
                  "vendor": "amd",
                  "label": "AMD Radeon Graphics",
                  "path": "gpu/amd-radeon-graphics.avif",
                  "width": 128,
                  "height": 108,
                  "bytes": 2994,
                  "sha256": "5dd0d052fc06d90a3ef3c13cbee85a8ec7e50172f69be4aac7057ca7c833f584",
                  "alpha": true,
                  "priority": 150,
                  "match": { "all_of": ["radeon"], "any_of": [], "not_any": [] }
                }
              ]
            }"#,
        )
        .expect("parse sample catalog")
    }

    #[test]
    fn specificity_matching_selects_series_before_family() {
        let cat = sample_catalog();
        assert_eq!(
            cat.match_hardware("cpu", "AMD Ryzen 9 7940HS")
                .map(|e| e.id.as_str()),
            Some("amd-ryzen-9-7000")
        );
        assert_eq!(
            cat.match_hardware("cpu", "AMD   RYZEN 9   9950X")
                .map(|e| e.id.as_str()),
            Some("amd-ryzen-9")
        );
        assert_eq!(
            cat.match_hardware("gpu", "AMD Radeon RX 7700S")
                .map(|e| e.id.as_str()),
            Some("amd-radeon-graphics")
        );
        assert!(
            cat.match_hardware("cpu", "Intel Core Ultra 7 258V")
                .is_none()
        );
        assert!(cat.match_hardware("gpu", "AMD Ryzen 9 7940HS").is_none());
    }
}
