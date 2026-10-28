use super::lens::{build_line_text_and_spans, estimate_line_char_em, is_dot_leader_char};
use super::models::{CenterRotatedBox, Geometry, ParsedLine, TextLayoutWord, WritingDirection};
use crossbeam_channel::Sender;
use prost::Message;
use serde::{Deserialize, Serialize};
use std::ffi::{c_char, c_void, CStr};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

pub const CIPD_PACKAGE: &str = "chromium/third_party/screen-ai/linux";
pub const CIPD_RESOLVE_URL: &str =
    "https://chrome-infra-packages.appspot.com/prpc/cipd.Repository/ResolveVersion";
pub const CIPD_DESCRIBE_URL: &str =
    "https://chrome-infra-packages.appspot.com/prpc/cipd.Repository/DescribeInstance";
#[allow(dead_code)]
pub const CIPD_DOWNLOAD_URL: &str =
    "https://chrome-infra-packages.appspot.com/dl/chromium/third_party/screen-ai/linux/+/latest";

pub const DEFAULT_MAX_DIMENSION: u32 = 2048;

static STAGING_SEQ: AtomicU64 = AtomicU64::new(1);

/// Metadata describing an installed or remote Chrome Screen AI OCR package.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScreenAiMetadata {
    pub version: String,
    pub instance_digest: String,
}

/// Pinned state of the currently loaded native Screen AI runtime in this process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveRuntimePin {
    pub base_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub metadata: ScreenAiMetadata,
}

/// Background events sent between Screen AI sync/update worker threads and the UI.
#[derive(Debug)]
pub enum ScreenAiEvent {
    UpdateCheckFinished {
        update_available: bool,
        remote_metadata: Option<ScreenAiMetadata>,
    },
    SyncProgress {
        message: String,
        progress: f32,
    },
    SyncFinished(Result<ScreenAiMetadata, String>),
}

/// Resolves the standard directory path for storing Chrome Screen AI model files
/// (`~/.config/michadame/screen_ai`).
pub fn resolve_screen_ai_directory() -> PathBuf {
    if let Ok(config_path) = confy::get_configuration_file_path("michadame", None) {
        if let Some(parent) = config_path.parent() {
            return parent.join("screen_ai");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".config")
            .join("michadame")
            .join("screen_ai")
    } else {
        PathBuf::from("./screen_ai")
    }
}

/// Resolves the active versioned directory inside `dir` (via `active_version` -> `versions/<id>`),
/// falling back to `dir` itself for legacy flat layouts or when `dir` is already a version directory.
pub fn resolve_active_version_dir(dir: &Path) -> PathBuf {
    let active_marker = dir.join("active_version");
    if let Ok(raw) = std::fs::read_to_string(&active_marker) {
        let ver_id = raw.trim();
        if !ver_id.is_empty() {
            let rel = Path::new(ver_id);
            if rel.components().count() == 1
                && rel.components().all(|c| matches!(c, Component::Normal(_)))
            {
                let version_dir = dir.join("versions").join(rel);
                if version_dir.is_dir() {
                    return version_dir;
                }
            }
        }
    }
    dir.to_path_buf()
}

fn resolve_model_file_path_in_concrete_dir(dir: &Path, rel_path: &str) -> Option<PathBuf> {
    let trimmed = rel_path.trim_start_matches("./").trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let rel = Path::new(trimmed);
    for comp in rel.components() {
        if !matches!(comp, Component::Normal(_)) {
            return None;
        }
    }
    let direct = dir.join(rel);
    if direct.is_file() {
        return Some(direct);
    }
    let nested = dir.join("resources").join(rel);
    if nested.is_file() {
        return Some(nested);
    }
    None
}

/// Resolves a relative model file path inside `screen_ai_dir`, checking the active version
/// subdirectory first, then the root directory and `resources/` subdirectory.
pub fn resolve_model_file_path(dir: &Path, rel_path: &str) -> Option<PathBuf> {
    let active_dir = resolve_active_version_dir(dir);
    if let Some(found) = resolve_model_file_path_in_concrete_dir(&active_dir, rel_path) {
        return Some(found);
    }
    if active_dir != dir {
        return resolve_model_file_path_in_concrete_dir(dir, rel_path);
    }
    None
}

/// Validates that a concrete Screen AI directory contains a valid ELF `libchromescreenai.so`
/// and a non-empty `files_list_ocr.txt` (plus optional `manifest.json`) whose listed files
/// all exist on disk and are non-empty.
pub fn validate_staged_screen_ai_dir(dir: &Path) -> Result<(), String> {
    let so_path =
        resolve_model_file_path_in_concrete_dir(dir, "libchromescreenai.so").ok_or_else(|| {
            format!(
                "Missing libchromescreenai.so in extracted Screen AI directory {}",
                dir.display()
            )
        })?;
    let mut so_file = std::fs::File::open(&so_path)
        .map_err(|e| format!("Failed to open {}: {e}", so_path.display()))?;
    let so_len = so_file
        .metadata()
        .map_err(|e| format!("Failed to stat {}: {e}", so_path.display()))?
        .len();
    if so_len < 4 {
        return Err(format!(
            "Invalid libchromescreenai.so in {}: file is too small ({so_len} bytes)",
            dir.display()
        ));
    }
    let mut magic = [0u8; 4];
    so_file
        .read_exact(&mut magic)
        .map_err(|e| format!("Failed to read ELF header from {}: {e}", so_path.display()))?;
    if &magic != b"\x7fELF" {
        return Err(format!(
            "Invalid libchromescreenai.so in {}: missing ELF magic header",
            dir.display()
        ));
    }

    let list_path =
        resolve_model_file_path_in_concrete_dir(dir, "files_list_ocr.txt").ok_or_else(|| {
            format!(
                "Missing files_list_ocr.txt in extracted Screen AI directory {}",
                dir.display()
            )
        })?;
    let list_content = std::fs::read_to_string(&list_path)
        .map_err(|e| format!("Failed to read {}: {e}", list_path.display()))?;
    let mut listed_count = 0usize;
    for raw_line in list_content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        listed_count += 1;
        let resolved = resolve_model_file_path_in_concrete_dir(dir, line).ok_or_else(|| {
            format!(
                "Model file '{}' listed in files_list_ocr.txt is missing in {}",
                line,
                dir.display()
            )
        })?;
        let file_len = std::fs::metadata(&resolved)
            .map_err(|e| format!("Failed to stat model file {}: {e}", resolved.display()))?
            .len();
        if file_len == 0 {
            return Err(format!(
                "Model file '{}' listed in files_list_ocr.txt is empty (0 bytes)",
                line
            ));
        }
    }
    if listed_count == 0 {
        return Err(format!(
            "Invalid files_list_ocr.txt in {}: no model files listed",
            dir.display()
        ));
    }

    // If a manifest.json exists in the extracted directory, validate its listed files as well
    if let Some(manifest_path) = resolve_model_file_path_in_concrete_dir(dir, "manifest.json") {
        let manifest_raw = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("Failed to read {}: {e}", manifest_path.display()))?;
        let manifest_val: serde_json::Value = serde_json::from_str(&manifest_raw)
            .map_err(|e| format!("Invalid JSON in {}: {e}", manifest_path.display()))?;
        if let Some(files_arr) = manifest_val.get("files").and_then(|v| v.as_array()) {
            for item in files_arr {
                let rel_opt = item.as_str().or_else(|| {
                    item.as_object()
                        .and_then(|o| o.get("name").or_else(|| o.get("path")))
                        .and_then(|v| v.as_str())
                });
                if let Some(rel_name) = rel_opt {
                    let clean = rel_name
                        .trim_start_matches("./")
                        .trim_start_matches('/')
                        .strip_prefix("resources/")
                        .unwrap_or(rel_name);
                    if clean.is_empty() || clean.starts_with(".cipdpkg") {
                        continue;
                    }
                    let resolved =
                        resolve_model_file_path_in_concrete_dir(dir, clean).ok_or_else(|| {
                            format!(
                                "File '{}' listed in manifest.json is missing in {}",
                                clean,
                                dir.display()
                            )
                        })?;
                    let len = std::fs::metadata(&resolved)
                        .map_err(|e| format!("Failed to stat {}: {e}", resolved.display()))?
                        .len();
                    if len == 0 {
                        return Err(format!(
                            "File '{}' listed in manifest.json is empty (0 bytes)",
                            clean
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}

/// Returns true if the Chrome Screen AI shared library and model manifest are present and valid on disk.
pub fn is_screen_ai_installed(dir: &Path) -> bool {
    let active_dir = resolve_active_version_dir(dir);
    if validate_staged_screen_ai_dir(&active_dir).is_ok() {
        return true;
    }
    if active_dir != dir {
        return validate_staged_screen_ai_dir(dir).is_ok();
    }
    false
}

/// Loads installed `ScreenAiMetadata` from `dir/metadata.json` (or the active version directory)
/// if the model files exist and are valid.
pub fn load_installed_metadata(dir: &Path) -> Option<ScreenAiMetadata> {
    if !is_screen_ai_installed(dir) {
        return None;
    }
    let active_dir = resolve_active_version_dir(dir);
    for candidate_dir in [dir, active_dir.as_path()] {
        let meta_path = candidate_dir.join("metadata.json");
        if let Ok(content) = std::fs::read_to_string(&meta_path) {
            if let Ok(meta) = serde_json::from_str::<ScreenAiMetadata>(&content) {
                return Some(meta);
            }
        }
    }
    Some(ScreenAiMetadata {
        version: "installed".to_string(),
        instance_digest: String::new(),
    })
}

fn write_file_atomically(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory {}: {e}", parent.display()))?;
    }
    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = path.with_extension(format!("tmp.{}.{}", std::process::id(), seq));
    if let Err(e) = std::fs::write(&tmp_path, content) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!(
            "Failed to write temporary file {}: {e}",
            tmp_path.display()
        ));
    }
    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!(
            "Failed to atomically replace {}: {e}",
            path.display()
        ));
    }
    Ok(())
}

/// Saves `ScreenAiMetadata` to `dir/metadata.json` (and the active version directory if distinct).
#[allow(dead_code)]
pub fn save_installed_metadata(dir: &Path, meta: &ScreenAiMetadata) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create Screen AI directory: {e}"))?;
    let json = serde_json::to_string_pretty(meta)
        .map_err(|e| format!("Failed to serialize Screen AI metadata: {e}"))?;
    write_file_atomically(&dir.join("metadata.json"), json.as_bytes())?;

    let active_dir = resolve_active_version_dir(dir);
    if active_dir != dir && active_dir.is_dir() {
        let _ = write_file_atomically(&active_dir.join("metadata.json"), json.as_bytes());
        let cipd_version_json = serde_json::json!({
            "package_name": CIPD_PACKAGE,
            "instance_id": meta.instance_digest,
            "version": meta.version,
        });
        if let Ok(cipd_bytes) = serde_json::to_vec_pretty(&cipd_version_json) {
            let _ = write_file_atomically(&active_dir.join(".cipd_version"), &cipd_bytes);
        }
    }
    Ok(())
}

/// Strips the pRPC XSSI protection prefix `)]}'` from a CIPD JSON response body.
pub fn strip_prpc_xssi_prefix(raw: &str) -> &str {
    let trimmed = raw.trim_start();
    if let Some(rest) = trimmed.strip_prefix(")]}'") {
        rest.trim_start()
    } else {
        trimmed
    }
}

/// Parses the JSON bodies from `ResolveVersion` and `DescribeInstance` into `ScreenAiMetadata`.
pub fn parse_cipd_version_responses(
    resolve_body: &str,
    describe_body: Option<&str>,
) -> Result<ScreenAiMetadata, String> {
    let resolve_json: serde_json::Value =
        serde_json::from_str(strip_prpc_xssi_prefix(resolve_body))
            .map_err(|e| format!("Failed to parse CIPD ResolveVersion JSON: {e}"))?;

    let hex_digest = resolve_json
        .get("instance")
        .and_then(|i| i.get("hexDigest"))
        .and_then(|d| d.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Missing instance.hexDigest in CIPD ResolveVersion response".to_string())?
        .to_string();

    let mut version = None;
    if let Some(desc_raw) = describe_body {
        if let Ok(desc_json) =
            serde_json::from_str::<serde_json::Value>(strip_prpc_xssi_prefix(desc_raw))
        {
            if let Some(tags) = desc_json.get("tags").and_then(|t| t.as_array()) {
                for tag_obj in tags {
                    if tag_obj.get("key").and_then(|k| k.as_str()) == Some("version") {
                        if let Some(val) = tag_obj.get("value").and_then(|v| v.as_str()) {
                            if !val.is_empty() {
                                version = Some(val.to_string());
                                break;
                            }
                        }
                    }
                    if let Some(tag_str) = tag_obj.get("tag").and_then(|t| t.as_str()) {
                        if let Some(val) = tag_str.strip_prefix("version:") {
                            if !val.is_empty() {
                                version = Some(val.to_string());
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    let resolved_version = version.unwrap_or_else(|| {
        let short_len = hex_digest.len().min(12);
        hex_digest[..short_len].to_string()
    });

    Ok(ScreenAiMetadata {
        version: resolved_version,
        instance_digest: hex_digest,
    })
}

fn decode_hex_bytes(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    for chunk in bytes.chunks_exact(2) {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

fn base64url_encode_no_pad(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3F) as usize] as char);
        }
    }
    out
}

/// Converts a resolved `ScreenAiMetadata` into an immutable CIPD download URL pinned to the
/// exact resolved instance digest (never falling back to mutable `latest`).
pub fn build_cipd_instance_download_url(meta: &ScreenAiMetadata) -> Result<String, String> {
    let digest = meta.instance_digest.trim();
    if digest.is_empty() || digest.eq_ignore_ascii_case("latest") {
        return Err(
            "Cannot build immutable CIPD download URL: missing resolved instance_digest"
                .to_string(),
        );
    }

    // CIPD ObjectRefToInstanceID encodes SHA256 as base64url(digest || hash_algo).
    // The algorithm byte is a suffix, not a prefix.
    let instance_ref = if digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit()) {
        let raw_hash = decode_hex_bytes(digest)
            .ok_or_else(|| "Invalid SHA256 hex digest in ScreenAiMetadata".to_string())?;
        let mut payload = Vec::with_capacity(1 + raw_hash.len());
        payload.extend_from_slice(&raw_hash);
        payload.push(0x02);
        base64url_encode_no_pad(&payload)
    } else if digest
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        digest.to_string()
    } else {
        return Err(format!(
            "Invalid characters in Screen AI instance_digest: '{digest}'"
        ));
    };

    Ok(format!(
        "https://chrome-infra-packages.appspot.com/dl/{CIPD_PACKAGE}/+/{instance_ref}"
    ))
}

/// Queries CIPD pRPC endpoints to resolve the latest `chromium/third_party/screen-ai/linux` version.
pub fn check_remote_screen_ai_version() -> Result<ScreenAiMetadata, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(15))
        .timeout_write(Duration::from_secs(10))
        .build();

    let resolve_req = serde_json::json!({
        "package": CIPD_PACKAGE,
        "version": "latest"
    });

    let resolve_resp = agent
        .post(CIPD_RESOLVE_URL)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .send_string(&resolve_req.to_string())
        .map_err(|e| format!("CIPD ResolveVersion request failed: {e}"))?;

    let resolve_body = resolve_resp
        .into_string()
        .map_err(|e| format!("Failed to read CIPD ResolveVersion response: {e}"))?;

    let partial = parse_cipd_version_responses(&resolve_body, None)?;

    let describe_req = serde_json::json!({
        "package": CIPD_PACKAGE,
        "instance": {
            "hashAlgo": "SHA256",
            "hexDigest": partial.instance_digest
        },
        "describeTags": true
    });

    let describe_body = agent
        .post(CIPD_DESCRIBE_URL)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .send_string(&describe_req.to_string())
        .ok()
        .and_then(|r| r.into_string().ok());

    parse_cipd_version_responses(&resolve_body, describe_body.as_deref())
}

fn sanitize_version_dir_name(meta: &ScreenAiMetadata) -> String {
    let sanitize = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
            .trim_matches('.')
            .to_string()
    };

    let ver = sanitize(meta.version.trim());
    let dig = sanitize(meta.instance_digest.trim());
    let short_dig = &dig[..dig.len().min(12)];

    match (ver.is_empty(), short_dig.is_empty()) {
        (false, false) if ver != short_dig => format!("{ver}-{short_dig}"),
        (false, _) => ver,
        (true, false) => short_dig.to_string(),
        (true, true) => "installed".to_string(),
    }
}

struct StagingCleanupGuard {
    staging_dir: PathBuf,
    committed: bool,
}

impl StagingCleanupGuard {
    fn new(staging_dir: PathBuf) -> Self {
        Self {
            staging_dir,
            committed: false,
        }
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for StagingCleanupGuard {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_dir_all(&self.staging_dir);
            if let Some(versions_dir) = self.staging_dir.parent() {
                if versions_dir
                    .read_dir()
                    .ok()
                    .is_some_and(|mut it| it.next().is_none())
                {
                    let _ = std::fs::remove_dir(versions_dir);
                }
            }
        }
    }
}

/// Downloads and installs the resolved Chrome Screen AI Linux package from CIPD into `target_dir`
/// using a staged versioned directory and atomic activation.
pub fn download_and_install_screen_ai<F>(
    target_dir: &Path,
    progress_cb: F,
) -> Result<ScreenAiMetadata, String>
where
    F: Fn(String, f32),
{
    std::fs::create_dir_all(target_dir)
        .map_err(|e| format!("Failed to create directory {}: {e}", target_dir.display()))?;

    progress_cb(
        "Checking latest Chrome Screen AI version...".to_string(),
        0.02,
    );
    let meta = check_remote_screen_ai_version()?;
    let download_url = build_cipd_instance_download_url(&meta)?;

    progress_cb(
        format!("Downloading Chrome Screen AI ({})...", meta.version),
        0.05,
    );

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        .build();

    let resp = agent
        .get(&download_url)
        .call()
        .map_err(|e| format!("Failed to download Chrome Screen AI package: {e}"))?;

    let total_bytes = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(78_000_000);

    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_zip_path = target_dir.join(format!(
        ".screen_ai_download_{}_{}.zip.tmp",
        std::process::id(),
        seq
    ));
    let download_res = (|| -> Result<(), String> {
        let mut out_file = std::fs::File::create(&tmp_zip_path)
            .map_err(|e| format!("Failed to create temporary zip file: {e}"))?;
        let mut reader = resp.into_reader();
        let mut buf = [0u8; 65536];
        let mut downloaded: u64 = 0;
        let mut last_report: u64 = 0;

        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("Error reading Chrome Screen AI download stream: {e}"))?;
            if n == 0 {
                break;
            }
            out_file
                .write_all(&buf[..n])
                .map_err(|e| format!("Error writing temporary zip file: {e}"))?;
            downloaded += n as u64;

            if downloaded - last_report >= 512 * 1024 || downloaded >= total_bytes {
                last_report = downloaded;
                let frac = (downloaded as f32 / total_bytes.max(1) as f32).clamp(0.0, 1.0);
                let mb_done = downloaded as f32 / (1024.0 * 1024.0);
                let mb_total = total_bytes as f32 / (1024.0 * 1024.0);
                progress_cb(
                    format!(
                        "Downloading Chrome Screen AI ({})... {:.1} / {:.1} MB",
                        meta.version, mb_done, mb_total
                    ),
                    0.05 + 0.70 * frac,
                );
            }
        }
        out_file
            .flush()
            .map_err(|e| format!("Failed to flush temporary zip file: {e}"))?;
        Ok(())
    })();

    if let Err(e) = download_res {
        let _ = std::fs::remove_file(&tmp_zip_path);
        return Err(e);
    }

    progress_cb(
        "Extracting Chrome Screen AI model files...".to_string(),
        0.76,
    );

    let install_result = install_screen_ai_archive(&tmp_zip_path, target_dir, &meta, &progress_cb);
    let _ = std::fs::remove_file(&tmp_zip_path);
    install_result?;

    progress_cb("Chrome Screen AI local model ready.".to_string(), 1.0);
    Ok(meta)
}

fn extract_zip_into_staging_dir<F>(
    zip_path: &Path,
    staging_dir: &Path,
    progress_cb: &F,
) -> Result<(), String>
where
    F: Fn(String, f32),
{
    let file = std::fs::File::open(zip_path)
        .map_err(|e| format!("Failed to open downloaded zip archive: {e}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("Failed to read Chrome Screen AI zip archive: {e}"))?;

    let total_entries = archive.len().max(1);
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("Failed to read zip entry #{i}: {e}"))?;
        let raw_name = entry.name().to_string();

        // Skip CIPD internal metadata files under .cipdpkg/
        if raw_name.starts_with(".cipdpkg/") || raw_name == ".cipdpkg" {
            continue;
        }

        // Strip leading "resources/" prefix so model files sit directly in staging_dir
        let rel_str = raw_name
            .strip_prefix("resources/")
            .unwrap_or(raw_name.as_str());
        if rel_str.is_empty() {
            continue;
        }

        let rel_path = Path::new(rel_str);
        if !rel_path
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(format!(
                "Refusing to extract unsafe archive entry path: '{raw_name}'"
            ));
        }

        let out_path = staging_dir.join(rel_path);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("Failed to create directory {}: {e}", out_path.display()))?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent dir {}: {e}", parent.display()))?;
        }

        let mut out_file = std::fs::File::create(&out_path)
            .map_err(|e| format!("Failed to create file {}: {e}", out_path.display()))?;
        std::io::copy(&mut entry, &mut out_file)
            .map_err(|e| format!("Failed to extract {}: {e}", out_path.display()))?;
        out_file
            .flush()
            .map_err(|e| format!("Failed to flush {}: {e}", out_path.display()))?;

        #[cfg(unix)]
        if out_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".so"))
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755));
        }

        let frac = (i + 1) as f32 / total_entries as f32;
        progress_cb(
            format!(
                "Extracting Chrome Screen AI model files ({}/{})...",
                i + 1,
                total_entries
            ),
            0.76 + 0.22 * frac,
        );
    }

    validate_staged_screen_ai_dir(staging_dir)
}

/// Extracts a Chrome Screen AI ZIP archive into a staged versioned subdirectory under `target_dir`,
/// validates `libchromescreenai.so` (ELF magic) and all files listed in `files_list_ocr.txt` /
/// `manifest.json`, writes `.cipd_version` and `metadata.json`, and atomically activates the new
/// version without mutating any currently pinned runtime directory.
pub fn install_screen_ai_archive<F>(
    zip_path: &Path,
    target_dir: &Path,
    meta: &ScreenAiMetadata,
    progress_cb: &F,
) -> Result<PathBuf, String>
where
    F: Fn(String, f32),
{
    let versions_dir = target_dir.join("versions");
    std::fs::create_dir_all(&versions_dir)
        .map_err(|e| format!("Failed to create {}: {e}", versions_dir.display()))?;

    let version_id = sanitize_version_dir_name(meta);
    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let staging_dir = versions_dir.join(format!(
        ".staging-{}-{}-{}",
        version_id,
        std::process::id(),
        seq
    ));
    if staging_dir.exists() {
        let _ = std::fs::remove_dir_all(&staging_dir);
    }
    std::fs::create_dir_all(&staging_dir).map_err(|e| {
        format!(
            "Failed to create staging dir {}: {e}",
            staging_dir.display()
        )
    })?;

    let mut guard = StagingCleanupGuard::new(staging_dir.clone());

    extract_zip_into_staging_dir(zip_path, &staging_dir, progress_cb)?;

    let meta_json = serde_json::to_string_pretty(meta)
        .map_err(|e| format!("Failed to serialize Screen AI metadata: {e}"))?;
    std::fs::write(staging_dir.join("metadata.json"), &meta_json)
        .map_err(|e| format!("Failed to write staged metadata.json: {e}"))?;

    let cipd_version_json = serde_json::json!({
        "package_name": CIPD_PACKAGE,
        "instance_id": meta.instance_digest,
        "version": meta.version,
    });
    let cipd_version_str = serde_json::to_string_pretty(&cipd_version_json)
        .map_err(|e| format!("Failed to serialize .cipd_version: {e}"))?;
    std::fs::write(staging_dir.join(".cipd_version"), cipd_version_str)
        .map_err(|e| format!("Failed to write staged .cipd_version: {e}"))?;

    let final_version_dir = versions_dir.join(&version_id);

    // If the exact same version_id directory is currently the pinned runtime directory,
    // keep that directory intact and use a distinct suffixed version directory.
    let final_version_dir = if active_pinned_runtime_dir()
        .as_deref()
        .is_some_and(|pinned| pinned == final_version_dir.as_path())
    {
        versions_dir.join(format!("{version_id}-r{seq}"))
    } else {
        final_version_dir
    };
    let final_version_name = final_version_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&version_id)
        .to_string();

    let backup_dir = if final_version_dir.exists() {
        let bak = versions_dir.join(format!(
            ".backup-{}-{}-{}",
            final_version_name,
            std::process::id(),
            seq
        ));
        let _ = std::fs::remove_dir_all(&bak);
        std::fs::rename(&final_version_dir, &bak).map_err(|e| {
            format!(
                "Failed to move existing version dir {} aside: {e}",
                final_version_dir.display()
            )
        })?;
        Some(bak)
    } else {
        None
    };

    if let Err(e) = std::fs::rename(&staging_dir, &final_version_dir) {
        if let Some(ref bak) = backup_dir {
            let _ = std::fs::rename(bak, &final_version_dir);
        }
        return Err(format!(
            "Failed to atomically activate staged Screen AI directory {}: {e}",
            final_version_dir.display()
        ));
    }

    guard.commit();
    if let Some(bak) = backup_dir {
        let _ = std::fs::remove_dir_all(bak);
    }

    write_file_atomically(
        &target_dir.join("active_version"),
        final_version_name.as_bytes(),
    )?;
    write_file_atomically(&target_dir.join("metadata.json"), meta_json.as_bytes())?;

    // Only update ACTIVE_MODEL_DIR if no native runtime is currently pinned in this process.
    update_active_model_dir_if_unpinned(&final_version_dir);

    Ok(final_version_dir)
}

#[cfg(test)]
fn extract_screen_ai_archive<F>(
    zip_path: &Path,
    target_dir: &Path,
    progress_cb: &F,
) -> Result<(), String>
where
    F: Fn(String, f32),
{
    let existing_meta = load_installed_metadata(target_dir).unwrap_or_else(|| ScreenAiMetadata {
        version: "installed".to_string(),
        instance_digest: String::new(),
    });
    install_screen_ai_archive(zip_path, target_dir, &existing_meta, progress_cb).map(|_| ())
}

/// Spawns a background thread to check if a newer Chrome Screen AI package is available on CIPD.
pub fn spawn_check_screen_ai_version(
    current_digest: Option<String>,
    is_checking: Arc<AtomicBool>,
    event_tx: Sender<ScreenAiEvent>,
) {
    if is_checking.swap(true, Ordering::AcqRel) {
        return;
    }

    let _ = std::thread::Builder::new()
        .name("screen-ai-version-check".into())
        .spawn(move || {
            let res = check_remote_screen_ai_version();
            is_checking.store(false, Ordering::Release);
            match res {
                Ok(remote) => {
                    let update_available = match current_digest {
                        Some(ref cur) if !cur.is_empty() => cur != &remote.instance_digest,
                        Some(_) => false,
                        None => true,
                    };
                    let _ = event_tx.send(ScreenAiEvent::UpdateCheckFinished {
                        update_available,
                        remote_metadata: Some(remote),
                    });
                }
                Err(e) => {
                    tracing::debug!("Screen AI version check failed: {}", e);
                    let _ = event_tx.send(ScreenAiEvent::UpdateCheckFinished {
                        update_available: current_digest.is_none(),
                        remote_metadata: None,
                    });
                }
            }
        });
}

/// Spawns a background thread to download and install the Chrome Screen AI model.
pub fn spawn_sync_screen_ai(
    target_dir: PathBuf,
    is_syncing: Arc<AtomicBool>,
    event_tx: Sender<ScreenAiEvent>,
) {
    if is_syncing.swap(true, Ordering::AcqRel) {
        return;
    }

    let _ = std::thread::Builder::new()
        .name("screen-ai-sync".into())
        .spawn(move || {
            let tx_prog = event_tx.clone();
            let result = download_and_install_screen_ai(&target_dir, move |message, progress| {
                let _ = tx_prog.send(ScreenAiEvent::SyncProgress { message, progress });
            });
            is_syncing.store(false, Ordering::Release);
            let _ = event_tx.send(ScreenAiEvent::SyncFinished(result));
        });
}

// ============================================================================
// SkBitmap C++ ABI & libchromescreenai.so FFI Bindings
// ============================================================================

/// Skia `SkColorType::kBGRA_8888_SkColorType`
pub const SK_COLOR_TYPE_BGRA_8888: i32 = 6;
/// Skia `SkAlphaType::kPremul_SkAlphaType`
pub const SK_ALPHA_TYPE_PREMUL: i32 = 2;

#[repr(C)]
pub struct SkImageInfo {
    pub f_color_space: *const c_void,
    pub f_color_type: i32,
    pub f_alpha_type: i32,
    pub f_width: i32,
    pub f_height: i32,
}

#[repr(C)]
pub struct SkPixmap {
    pub f_pixels: *const c_void,
    pub f_row_bytes: usize,
    pub f_info: SkImageInfo,
}

#[repr(C)]
pub struct FakeSkPixelRef {
    pub vtable_ptr: *const *const c_void,
    pub refcount: i32,
    pub _pad: i32,
    pub f_width: i32,
    pub f_height: i32,
    pub f_pixels: *const c_void,
    pub f_row_bytes: usize,
    pub _extra: [u8; 64],
}

#[repr(C)]
pub struct SkBitmap {
    pub f_pixel_ref: *const FakeSkPixelRef,
    pub f_pixmap: SkPixmap,
    pub f_flags: u8,
}

static DUMMY_VTABLE: [usize; 16] = [0; 16];

static ACTIVE_MODEL_DIR: OnceLock<RwLock<PathBuf>> = OnceLock::new();
static PINNED_RUNTIME_STATE: OnceLock<RwLock<Option<ActiveRuntimePin>>> = OnceLock::new();

fn active_model_dir_lock() -> &'static RwLock<PathBuf> {
    ACTIVE_MODEL_DIR.get_or_init(|| {
        let base = resolve_screen_ai_directory();
        RwLock::new(resolve_active_version_dir(&base))
    })
}

fn pinned_runtime_state_lock() -> &'static RwLock<Option<ActiveRuntimePin>> {
    PINNED_RUNTIME_STATE.get_or_init(|| RwLock::new(None))
}

fn active_pinned_runtime_dir() -> Option<PathBuf> {
    pinned_runtime_state_lock()
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|p| p.runtime_dir.clone()))
}

fn update_active_model_dir_if_unpinned(dir: &Path) {
    if active_pinned_runtime_dir().is_some() {
        return;
    }
    let concrete = resolve_active_version_dir(dir);
    if let Ok(mut guard) = active_model_dir_lock().write() {
        *guard = concrete;
    }
}

/// Returns the immutable model directory currently serving the active Screen AI callbacks.
pub fn active_runtime_model_dir() -> PathBuf {
    if let Some(pinned_dir) = active_pinned_runtime_dir() {
        return pinned_dir;
    }
    active_model_dir_lock()
        .read()
        .map(|g| resolve_active_version_dir(&g))
        .unwrap_or_else(|_| resolve_active_version_dir(&resolve_screen_ai_directory()))
}

/// Returns the metadata of the Screen AI version currently loaded/pinned in this process, if any.
#[allow(dead_code)]
pub fn active_loaded_screen_ai_metadata() -> Option<ScreenAiMetadata> {
    pinned_runtime_state_lock()
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|p| p.metadata.clone()))
}

/// Returns true if a newer Screen AI version has been installed into `dir` on disk than the
/// version currently pinned by the loaded runtime in this process (meaning a restart is needed
/// to load the new `libchromescreenai.so`).
pub fn is_screen_ai_restart_required(dir: &Path) -> bool {
    let Some(pin) = pinned_runtime_state_lock()
        .read()
        .ok()
        .and_then(|g| g.clone())
    else {
        return false;
    };
    let disk_version_dir = resolve_active_version_dir(dir);
    if disk_version_dir != pin.runtime_dir {
        return true;
    }
    if let Some(disk_meta) = load_installed_metadata(dir) {
        if !disk_meta.instance_digest.is_empty() && !pin.metadata.instance_digest.is_empty() {
            return disk_meta.instance_digest != pin.metadata.instance_digest;
        }
        return disk_meta.version != pin.metadata.version;
    }
    false
}

/// Pins the process-wide Screen AI file callbacks to the concrete active version directory inside
/// `model_dir` if not already pinned. Once pinned, subsequent installs on disk will not mutate or
/// redirect the directory read by the running native library until `reset_pinned_runtime_state`
/// (or process restart).
pub fn pin_runtime_to_installed_version(model_dir: &Path) -> Result<ActiveRuntimePin, String> {
    if let Some(existing) = pinned_runtime_state_lock()
        .read()
        .ok()
        .and_then(|g| g.clone())
    {
        return Ok(existing);
    }

    let concrete_dir = resolve_active_version_dir(model_dir);
    validate_staged_screen_ai_dir(&concrete_dir)?;

    let metadata = load_installed_metadata(&concrete_dir)
        .or_else(|| load_installed_metadata(model_dir))
        .unwrap_or_else(|| ScreenAiMetadata {
            version: "installed".to_string(),
            instance_digest: String::new(),
        });

    let pin = ActiveRuntimePin {
        base_dir: model_dir.to_path_buf(),
        runtime_dir: concrete_dir.clone(),
        metadata,
    };

    if let Ok(mut dir_guard) = active_model_dir_lock().write() {
        *dir_guard = concrete_dir;
    }
    if let Ok(mut pin_guard) = pinned_runtime_state_lock().write() {
        if pin_guard.is_none() {
            *pin_guard = Some(pin.clone());
        } else if let Some(ref existing) = *pin_guard {
            return Ok(existing.clone());
        }
    }

    Ok(pin)
}

/// Clears the pinned runtime state (used to simulate process restart in tests).
#[allow(dead_code)]
pub fn reset_pinned_runtime_state() {
    if let Ok(mut pin_guard) = pinned_runtime_state_lock().write() {
        *pin_guard = None;
    }
}

extern "C" fn get_file_content_size_cb(relative_file_path: *const c_char) -> u32 {
    if relative_file_path.is_null() {
        return 0;
    }
    let Ok(rel_str) = unsafe { CStr::from_ptr(relative_file_path) }.to_str() else {
        return 0;
    };
    let dir = active_runtime_model_dir();
    let Some(full_path) = resolve_model_file_path_in_concrete_dir(&dir, rel_str)
        .or_else(|| resolve_model_file_path(&dir, rel_str))
    else {
        tracing::warn!("Screen AI requested missing model file size: {}", rel_str);
        return 0;
    };
    std::fs::metadata(full_path)
        .map(|m| m.len() as u32)
        .unwrap_or(0)
}

extern "C" fn get_file_content_cb(
    relative_file_path: *const c_char,
    buffer_size: u32,
    content_buffer: *mut u8,
) {
    if relative_file_path.is_null() || content_buffer.is_null() || buffer_size == 0 {
        return;
    }
    let Ok(rel_str) = unsafe { CStr::from_ptr(relative_file_path) }.to_str() else {
        return;
    };
    let dir = active_runtime_model_dir();
    let Some(full_path) = resolve_model_file_path_in_concrete_dir(&dir, rel_str)
        .or_else(|| resolve_model_file_path(&dir, rel_str))
    else {
        tracing::warn!(
            "Screen AI requested missing model file content: {}",
            rel_str
        );
        return;
    };
    if let Ok(bytes) = std::fs::read(full_path) {
        let to_copy = bytes.len().min(buffer_size as usize);
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), content_buffer, to_copy);
        }
    }
}

type SetFileContentFunctionsFn = unsafe extern "C" fn(
    extern "C" fn(*const c_char) -> u32,
    extern "C" fn(*const c_char, u32, *mut u8),
);
type InitOCRUsingCallbackFn = unsafe extern "C" fn() -> bool;
type SetOCRLightModeFn = unsafe extern "C" fn(bool);
type GetMaxImageDimensionFn = unsafe extern "C" fn() -> u32;
type PerformOCRFn = unsafe extern "C" fn(*const SkBitmap, &mut u32) -> *mut c_char;
type FreeLibraryAllocatedCharArrayFn = unsafe extern "C" fn(*mut c_char);

struct ScreenAiLibrary {
    _lib: libloading::Library,
    set_file_content_functions: SetFileContentFunctionsFn,
    init_ocr_using_callback: InitOCRUsingCallbackFn,
    set_ocr_light_mode: Option<SetOCRLightModeFn>,
    get_max_image_dimension: Option<GetMaxImageDimensionFn>,
    perform_ocr: PerformOCRFn,
    free_char_array: FreeLibraryAllocatedCharArrayFn,
    initialized: bool,
    max_dimension: u32,
}

static SCREEN_AI_RUNTIME: OnceLock<Mutex<Option<ScreenAiLibrary>>> = OnceLock::new();

fn runtime_mutex() -> &'static Mutex<Option<ScreenAiLibrary>> {
    SCREEN_AI_RUNTIME.get_or_init(|| Mutex::new(None))
}

fn ensure_runtime_loaded_and_initialized(
    guard: &mut Option<ScreenAiLibrary>,
    model_dir: &Path,
) -> Result<u32, String> {
    let pin = pin_runtime_to_installed_version(model_dir)?;
    let pinned_dir = &pin.runtime_dir;

    if guard.is_none() {
        let so_path = resolve_model_file_path_in_concrete_dir(pinned_dir, "libchromescreenai.so")
            .or_else(|| resolve_model_file_path(pinned_dir, "libchromescreenai.so"))
            .ok_or_else(|| {
                format!(
                    "Chrome Screen AI library not found in {}",
                    pinned_dir.display()
                )
            })?;

        unsafe {
            #[cfg(unix)]
            let lib: libloading::Library = libloading::os::unix::Library::open(
                Some(&so_path),
                libc::RTLD_LAZY | libc::RTLD_LOCAL,
            )
            .map_err(|e| format!("Failed to load {}: {e}", so_path.display()))?
            .into();

            #[cfg(not(unix))]
            let lib = libloading::Library::new(&so_path)
                .map_err(|e| format!("Failed to load {}: {e}", so_path.display()))?;

            let set_file_content_functions: SetFileContentFunctionsFn = *lib
                .get(b"SetFileContentFunctions\0")
                .map_err(|e| format!("Missing SetFileContentFunctions symbol: {e}"))?;
            let init_ocr_using_callback: InitOCRUsingCallbackFn = *lib
                .get(b"InitOCRUsingCallback\0")
                .map_err(|e| format!("Missing InitOCRUsingCallback symbol: {e}"))?;
            let set_ocr_light_mode: Option<SetOCRLightModeFn> =
                lib.get(b"SetOCRLightMode\0").ok().map(|s| *s);
            let get_max_image_dimension: Option<GetMaxImageDimensionFn> =
                lib.get(b"GetMaxImageDimension\0").ok().map(|s| *s);
            let perform_ocr: PerformOCRFn = *lib
                .get(b"PerformOCR\0")
                .map_err(|e| format!("Missing PerformOCR symbol: {e}"))?;
            let free_char_array: FreeLibraryAllocatedCharArrayFn = *lib
                .get(b"FreeLibraryAllocatedCharArray\0")
                .map_err(|e| format!("Missing FreeLibraryAllocatedCharArray symbol: {e}"))?;

            *guard = Some(ScreenAiLibrary {
                _lib: lib,
                set_file_content_functions,
                init_ocr_using_callback,
                set_ocr_light_mode,
                get_max_image_dimension,
                perform_ocr,
                free_char_array,
                initialized: false,
                max_dimension: DEFAULT_MAX_DIMENSION,
            });
        }
    }

    let runtime = guard.as_mut().unwrap();
    if !runtime.initialized {
        unsafe {
            (runtime.set_file_content_functions)(get_file_content_size_cb, get_file_content_cb);
            if let Some(set_light) = runtime.set_ocr_light_mode {
                set_light(false);
            }
            if !(runtime.init_ocr_using_callback)() {
                return Err("Chrome Screen AI InitOCRUsingCallback() returned false".to_string());
            }
            if let Some(get_max) = runtime.get_max_image_dimension {
                let d = get_max();
                if d >= 256 {
                    runtime.max_dimension = d;
                }
            }
            runtime.initialized = true;
        }
    }

    Ok(runtime.max_dimension)
}

/// Warms up / initializes the local Chrome Screen AI library if installed in `model_dir`.
pub fn warmup_screen_ai(model_dir: &Path) -> Result<u32, String> {
    if !is_screen_ai_installed(model_dir) {
        return Err("Chrome Screen AI model is not installed yet".to_string());
    }
    let mut guard = runtime_mutex()
        .lock()
        .map_err(|_| "Screen AI mutex poisoned".to_string())?;
    ensure_runtime_loaded_and_initialized(&mut guard, model_dir)
}

/// Executes local OCR on a top-down `BGRA_8888` image buffer using `libchromescreenai.so`.
pub fn execute_screen_ai_ocr_bgra(
    model_dir: &Path,
    bgra: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<ParsedLine>, String> {
    if width == 0 || height == 0 {
        return Err("Invalid image dimensions for Screen AI OCR".to_string());
    }
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "Image dimensions overflow".to_string())?;
    if bgra.len() < expected_len {
        return Err(format!(
            "BGRA buffer too small: got {} bytes, expected {}",
            bgra.len(),
            expected_len
        ));
    }

    let proto_bytes = {
        let mut guard = runtime_mutex()
            .lock()
            .map_err(|_| "Screen AI mutex poisoned".to_string())?;
        ensure_runtime_loaded_and_initialized(&mut guard, model_dir)?;
        let runtime = guard.as_ref().unwrap();

        let row_bytes = (width as usize) * 4;
        let fake_pixel_ref = FakeSkPixelRef {
            vtable_ptr: DUMMY_VTABLE.as_ptr() as *const *const c_void,
            refcount: 1,
            _pad: 0,
            f_width: width as i32,
            f_height: height as i32,
            f_pixels: bgra.as_ptr() as *const c_void,
            f_row_bytes: row_bytes,
            _extra: [0u8; 64],
        };

        let bitmap = SkBitmap {
            f_pixel_ref: &fake_pixel_ref as *const FakeSkPixelRef,
            f_pixmap: SkPixmap {
                f_pixels: bgra.as_ptr() as *const c_void,
                f_row_bytes: row_bytes,
                f_info: SkImageInfo {
                    f_color_space: std::ptr::null(),
                    f_color_type: SK_COLOR_TYPE_BGRA_8888,
                    f_alpha_type: SK_ALPHA_TYPE_PREMUL,
                    f_width: width as i32,
                    f_height: height as i32,
                },
            },
            f_flags: 0,
        };

        let mut output_len: u32 = 0;
        unsafe {
            let out_ptr = (runtime.perform_ocr)(&bitmap as *const SkBitmap, &mut output_len);
            if out_ptr.is_null() {
                Vec::new()
            } else {
                let bytes =
                    std::slice::from_raw_parts(out_ptr as *const u8, output_len as usize).to_vec();
                (runtime.free_char_array)(out_ptr);
                bytes
            }
        }
    };

    if proto_bytes.is_empty() {
        return Ok(Vec::new());
    }

    parse_visual_annotation_to_lines(&proto_bytes, width, height)
}

// ============================================================================
// Image Conversion & Scaling Helpers
// ============================================================================

/// Computes target dimensions for Chrome Screen AI OCR, preserving aspect ratio and
/// clamping the maximum dimension to `max_dim` (typically `2048`).
pub fn calculate_screen_ai_target_dimensions(width: u32, height: u32, max_dim: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (0, 0);
    }
    let limit = max_dim.max(1);
    if width <= limit && height <= limit {
        return (width, height);
    }
    let scale = (limit as f32 / width as f32).min(limit as f32 / height as f32);
    let target_w = ((width as f32 * scale).round() as u32).max(1);
    let target_h = ((height as f32 * scale).round() as u32).max(1);
    (target_w, target_h)
}

/// Single-pass vertical flip (from OpenGL bottom-left origin to top-left origin),
/// downscale, and `RGBA_8888` -> `BGRA_8888` conversion.
pub fn convert_and_scale_gl_rgba_to_bgra(
    raw_rgba: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    out_bgra: &mut Vec<u8>,
) {
    let expected_src = (src_w as usize) * (src_h as usize) * 4;
    let total_dst = (dst_w as usize) * (dst_h as usize) * 4;
    out_bgra.clear();
    if raw_rgba.len() < expected_src || src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return;
    }
    out_bgra.resize(total_dst, 0);

    let src_w_usize = src_w as usize;
    let src_h_usize = src_h as usize;
    let dst_w_usize = dst_w as usize;
    let dst_h_usize = dst_h as usize;

    if src_w_usize == dst_w_usize && src_h_usize == dst_h_usize {
        let row_bytes = src_w_usize * 4;
        for dst_y in 0..dst_h_usize {
            let src_y = src_h_usize - 1 - dst_y;
            let src_row = &raw_rgba[src_y * row_bytes..(src_y + 1) * row_bytes];
            let dst_row = &mut out_bgra[dst_y * row_bytes..(dst_y + 1) * row_bytes];
            for (src_px, dst_px) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(4)) {
                dst_px[0] = src_px[2]; // B
                dst_px[1] = src_px[1]; // G
                dst_px[2] = src_px[0]; // R
                dst_px[3] = 255; // A (opaque)
            }
        }
        return;
    }

    for dst_y in 0..dst_h_usize {
        let flipped_y = (dst_y * src_h_usize) / dst_h_usize;
        let src_y = src_h_usize - 1 - flipped_y;
        let src_row_offset = src_y * src_w_usize * 4;
        let dst_row_offset = dst_y * dst_w_usize * 4;

        for dst_x in 0..dst_w_usize {
            let src_x = (dst_x * src_w_usize) / dst_w_usize;
            let src_idx = src_row_offset + src_x * 4;
            let dst_idx = dst_row_offset + dst_x * 4;

            out_bgra[dst_idx] = raw_rgba[src_idx + 2]; // B
            out_bgra[dst_idx + 1] = raw_rgba[src_idx + 1]; // G
            out_bgra[dst_idx + 2] = raw_rgba[src_idx]; // R
            out_bgra[dst_idx + 3] = 255; // A
        }
    }
}

// ============================================================================
// Chrome Screen AI VisualAnnotation Protobuf Schema & Conversion
// ============================================================================

#[derive(Clone, PartialEq, Message)]
pub struct ScreenAiRect {
    #[prost(int32, tag = "1")]
    pub x: i32,
    #[prost(int32, tag = "2")]
    pub y: i32,
    #[prost(int32, tag = "3")]
    pub width: i32,
    #[prost(int32, tag = "4")]
    pub height: i32,
    #[prost(float, tag = "5")]
    pub angle: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct ScreenAiSymbolBox {
    #[prost(message, optional, tag = "1")]
    pub bounding_box: Option<ScreenAiRect>,
    #[prost(string, tag = "2")]
    pub utf8_string: String,
    #[prost(float, tag = "3")]
    pub confidence: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct ScreenAiWordBox {
    #[prost(message, repeated, tag = "1")]
    pub symbols: Vec<ScreenAiSymbolBox>,
    #[prost(message, optional, tag = "2")]
    pub bounding_box: Option<ScreenAiRect>,
    #[prost(string, tag = "3")]
    pub utf8_string: String,
    #[prost(string, tag = "5")]
    pub language: String,
    #[prost(bool, tag = "6")]
    pub has_space_after: bool,
    #[prost(int32, tag = "12")]
    pub direction: i32,
    #[prost(int32, tag = "13")]
    pub content_type: i32,
    #[prost(float, tag = "15")]
    pub confidence: f32,
    #[prost(message, optional, tag = "17")]
    pub whitespace_bounding_box: Option<ScreenAiRect>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ScreenAiLineBox {
    #[prost(message, repeated, tag = "1")]
    pub words: Vec<ScreenAiWordBox>,
    #[prost(message, optional, tag = "2")]
    pub bounding_box: Option<ScreenAiRect>,
    #[prost(string, tag = "3")]
    pub utf8_string: String,
    #[prost(string, tag = "4")]
    pub language: String,
    #[prost(int32, tag = "5")]
    pub block_id: i32,
    #[prost(int32, tag = "7")]
    pub direction: i32,
    #[prost(int32, tag = "8")]
    pub content_type: i32,
    #[prost(float, tag = "10")]
    pub confidence: f32,
    #[prost(int32, tag = "11")]
    pub paragraph_id: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct VisualAnnotation {
    #[prost(message, repeated, tag = "2")]
    pub lines: Vec<ScreenAiLineBox>,
}

/// Maps Chrome Screen AI's `Direction` enum to [`WritingDirection`].
///
/// In `chrome_screen_ai.proto`:
/// - `0 = DIRECTION_UNSPECIFIED`
/// - `1 = DIRECTION_LEFT_TO_RIGHT`
/// - `2 = DIRECTION_RIGHT_TO_LEFT`
/// - `3 = DIRECTION_TOP_TO_BOTTOM`
pub fn screen_ai_direction_to_writing_direction(dir: i32) -> WritingDirection {
    match dir {
        2 => WritingDirection::RightToLeft,
        3 => WritingDirection::TopToBottom,
        _ => WritingDirection::LeftToRight,
    }
}

/// Converts a `ScreenAiRect` (top-left `(x, y)` before clockwise rotation by `angle` degrees)
/// into normalized `(center_x, center_y, screen_width, screen_height, angle_deg)` in `[0.0, 1.0]`.
pub fn rect_to_normalized_box(
    rect: &ScreenAiRect,
    img_w: u32,
    img_h: u32,
) -> Option<(f32, f32, f32, f32, f32)> {
    if rect.width <= 0 || rect.height <= 0 || img_w == 0 || img_h == 0 {
        return None;
    }

    let w_px = rect.width as f32;
    let h_px = rect.height as f32;
    let theta = rect.angle.to_radians();
    let cos_t = theta.cos();
    let sin_t = theta.sin();

    let cx_px = rect.x as f32 + 0.5 * w_px * cos_t - 0.5 * h_px * sin_t;
    let cy_px = rect.y as f32 + 0.5 * w_px * sin_t + 0.5 * h_px * cos_t;

    let screen_w_px = (w_px * cos_t.abs() + h_px * sin_t.abs()).max(1.0);
    let screen_h_px = (w_px * sin_t.abs() + h_px * cos_t.abs()).max(1.0);

    let cx = cx_px / img_w as f32;
    let cy = cy_px / img_h as f32;
    let w = screen_w_px / img_w as f32;
    let h = screen_h_px / img_h as f32;

    Some((cx, cy, w, h, rect.angle))
}

fn normalized_box_to_geometry(cx: f32, cy: f32, w: f32, h: f32) -> Geometry {
    Geometry {
        bounding_box: Some(CenterRotatedBox {
            center_x: cx,
            center_y: cy,
            width: w,
            height: h,
            rotation_z: 0.0,
            coordinate_type: 0,
        }),
    }
}

fn union_normalized_boxes(boxes: &[(f32, f32, f32, f32)]) -> Option<(f32, f32, f32, f32)> {
    if boxes.is_empty() {
        return None;
    }
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for &(cx, cy, w, h) in boxes {
        min_x = min_x.min(cx - 0.5 * w);
        max_x = max_x.max(cx + 0.5 * w);
        min_y = min_y.min(cy - 0.5 * h);
        max_y = max_y.max(cy + 0.5 * h);
    }
    if max_x <= min_x || max_y <= min_y {
        return None;
    }
    let uw = max_x - min_x;
    let uh = max_y - min_y;
    Some((min_x + 0.5 * uw, min_y + 0.5 * uh, uw, uh))
}

/// Converts a `ScreenAiLineBox`'s words and symbols into `Vec<TextLayoutWord>` with screen-space
/// normalized bounding boxes so [`estimate_line_char_em`] and [`build_line_text_and_spans`] produce
/// exact per-character spans and dot-leader normalizations.
fn build_layout_words_for_line(
    line: &ScreenAiLineBox,
    img_w: u32,
    img_h: u32,
) -> Vec<TextLayoutWord> {
    let mut out = Vec::new();

    if line.words.is_empty() {
        if !line.utf8_string.is_empty() {
            let geom = line
                .bounding_box
                .as_ref()
                .and_then(|r| rect_to_normalized_box(r, img_w, img_h))
                .map(|(cx, cy, w, h, _)| normalized_box_to_geometry(cx, cy, w, h));
            out.push(TextLayoutWord {
                id: None,
                plain_text: line.utf8_string.clone(),
                text_separator: None,
                geometry: geom,
            });
        }
        return out;
    }

    for (w_idx, word) in line.words.iter().enumerate() {
        let is_last_word = w_idx + 1 == line.words.len();
        let has_ws_box = word
            .whitespace_bounding_box
            .as_ref()
            .is_some_and(|r| r.width > 0 && r.height > 0);
        let sep_after = if !is_last_word && (word.has_space_after || has_ws_box) {
            Some(" ".to_string())
        } else {
            None
        };

        let non_empty_symbols: Vec<&ScreenAiSymbolBox> = word
            .symbols
            .iter()
            .filter(|s| !s.utf8_string.is_empty())
            .collect();

        if non_empty_symbols.is_empty() {
            if word.utf8_string.is_empty() {
                continue;
            }
            let geom = word
                .bounding_box
                .as_ref()
                .and_then(|r| rect_to_normalized_box(r, img_w, img_h))
                .map(|(cx, cy, w, h, _)| normalized_box_to_geometry(cx, cy, w, h));
            out.push(TextLayoutWord {
                id: None,
                plain_text: word.utf8_string.clone(),
                text_separator: sep_after,
                geometry: geom,
            });
            continue;
        }

        // Emit per-symbol TextLayoutWords, grouping consecutive dot-leader symbols inside the word
        // so `normalize_dot_leader_word` can collapse multi-cell ellipsis runs accurately.
        let start_out_len = out.len();
        let mut s_idx = 0;
        while s_idx < non_empty_symbols.len() {
            let sym = non_empty_symbols[s_idx];
            let is_dot = sym.utf8_string.chars().all(is_dot_leader_char);

            if is_dot {
                let mut dot_text = String::new();
                let mut dot_boxes = Vec::new();
                while s_idx < non_empty_symbols.len()
                    && non_empty_symbols[s_idx]
                        .utf8_string
                        .chars()
                        .all(is_dot_leader_char)
                {
                    let cur_sym = non_empty_symbols[s_idx];
                    dot_text.push_str(&cur_sym.utf8_string);
                    if let Some((cx, cy, w, h, _)) = cur_sym
                        .bounding_box
                        .as_ref()
                        .and_then(|r| rect_to_normalized_box(r, img_w, img_h))
                    {
                        dot_boxes.push((cx, cy, w, h));
                    }
                    s_idx += 1;
                }
                let geom = union_normalized_boxes(&dot_boxes)
                    .or_else(|| {
                        word.bounding_box
                            .as_ref()
                            .and_then(|r| rect_to_normalized_box(r, img_w, img_h))
                            .map(|(cx, cy, w, h, _)| (cx, cy, w, h))
                    })
                    .map(|(cx, cy, w, h)| normalized_box_to_geometry(cx, cy, w, h));
                out.push(TextLayoutWord {
                    id: None,
                    plain_text: dot_text,
                    text_separator: None,
                    geometry: geom,
                });
            } else {
                let geom = sym
                    .bounding_box
                    .as_ref()
                    .and_then(|r| rect_to_normalized_box(r, img_w, img_h))
                    .map(|(cx, cy, w, h, _)| normalized_box_to_geometry(cx, cy, w, h));
                out.push(TextLayoutWord {
                    id: None,
                    plain_text: sym.utf8_string.clone(),
                    text_separator: None,
                    geometry: geom,
                });
                s_idx += 1;
            }
        }

        if out.len() > start_out_len {
            if let Some(last_emitted) = out.last_mut() {
                last_emitted.text_separator = sep_after;
            }
        }
    }

    out
}

/// Decodes a serialized `chrome_screen_ai.VisualAnnotation` protobuf into `Vec<ParsedLine>`.
pub fn parse_visual_annotation_to_lines(
    proto_bytes: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<ParsedLine>, String> {
    if proto_bytes.is_empty() || width == 0 || height == 0 {
        return Ok(Vec::new());
    }

    let annotation = VisualAnnotation::decode(proto_bytes)
        .map_err(|e| format!("Failed to decode Chrome Screen AI VisualAnnotation protobuf: {e}"))?;

    let mut parsed_lines = Vec::new();
    let mut paragraph_keys: Vec<(i32, i32)> = Vec::new();

    for line in &annotation.lines {
        let p_key = (line.block_id, line.paragraph_id);
        let p_idx = match paragraph_keys.iter().position(|&k| k == p_key) {
            Some(idx) => idx,
            None => {
                let idx = paragraph_keys.len();
                paragraph_keys.push(p_key);
                idx
            }
        };

        let layout_words = build_layout_words_for_line(line, width, height);
        if layout_words.is_empty() {
            continue;
        }

        // Resolve line bounding box from line.bounding_box or union of word boxes
        let mut resolved_box = line
            .bounding_box
            .as_ref()
            .and_then(|r| rect_to_normalized_box(r, width, height));

        if resolved_box.is_none() {
            let mut word_boxes = Vec::new();
            let mut angle_sum = 0.0f32;
            let mut angle_cnt = 0usize;
            for w in &line.words {
                if let Some((cx, cy, bw, bh, ang)) = w
                    .bounding_box
                    .as_ref()
                    .and_then(|r| rect_to_normalized_box(r, width, height))
                {
                    word_boxes.push((cx, cy, bw, bh));
                    angle_sum += ang;
                    angle_cnt += 1;
                }
            }
            if let Some((cx, cy, uw, uh)) = union_normalized_boxes(&word_boxes) {
                let avg_ang = if angle_cnt > 0 {
                    angle_sum / angle_cnt as f32
                } else {
                    0.0
                };
                resolved_box = Some((cx, cy, uw, uh, avg_ang));
            }
        }

        let Some((cx, cy, w, h, angle_deg)) = resolved_box else {
            continue;
        };

        let line_dir = screen_ai_direction_to_writing_direction(line.direction);
        let words_vertical = !line.words.is_empty()
            && line
                .words
                .iter()
                .filter(|w| screen_ai_direction_to_writing_direction(w.direction).is_vertical())
                .count()
                * 2
                >= line.words.len();
        let abs_angle = angle_deg.abs();
        let is_rotated_vert = abs_angle > 45.0 && abs_angle < 135.0;
        let raw_char_count: usize = layout_words
            .iter()
            .map(|w| w.plain_text.chars().count())
            .sum();

        let writing_direction = if line_dir.is_vertical()
            || words_vertical
            || is_rotated_vert
            || (h > 1.8 * w && raw_char_count >= 2)
        {
            WritingDirection::TopToBottom
        } else {
            line_dir
        };

        let is_vertical = writing_direction.is_vertical();
        let cx_clamped = cx.clamp(0.0, 1.0);
        let cy_clamped = cy.clamp(0.0, 1.0);
        let w_clamped = w.clamp(0.001, 1.0);
        let h_clamped = h.clamp(0.001, 1.0);

        // `rect_to_normalized_box` already projects rotated boxes onto screen axes (w = horizontal,
        // h = vertical), so pass `is_rotated_vert = false` to avoid double-swapping dimensions.
        let char_em = estimate_line_char_em(
            &layout_words,
            w_clamped,
            h_clamped,
            width,
            height,
            is_vertical,
            false,
        );

        let (line_min, line_max) = if is_vertical {
            (
                (cy_clamped - h_clamped * 0.5).clamp(0.0, 1.0),
                (cy_clamped + h_clamped * 0.5).clamp(0.0, 1.0),
            )
        } else {
            (
                (cx_clamped - w_clamped * 0.5).clamp(0.0, 1.0),
                (cx_clamped + w_clamped * 0.5).clamp(0.0, 1.0),
            )
        };

        let (line_text, char_spans) = build_line_text_and_spans(
            &layout_words,
            line_min,
            line_max,
            char_em,
            is_vertical,
            false,
        );

        if line_text.is_empty() {
            continue;
        }

        parsed_lines.push(ParsedLine {
            text: line_text,
            center_x: cx_clamped,
            center_y: cy_clamped,
            width: w_clamped,
            height: h_clamped,
            paragraph_idx: p_idx,
            writing_direction,
            char_spans,
        });
    }

    Ok(parsed_lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::lens::group_lines_into_blocks;

    #[test]
    fn test_skbitmap_abi_layout() {
        assert_eq!(std::mem::size_of::<SkImageInfo>(), 24);
        assert_eq!(std::mem::size_of::<SkPixmap>(), 40);
        assert_eq!(std::mem::size_of::<SkBitmap>(), 56);
    }

    #[test]
    fn test_strip_prpc_xssi_prefix_and_parse_cipd() {
        let resolve_resp = ")]}'\n{\"instance\": {\"hashAlgo\": \"SHA256\", \"hexDigest\": \"cd837ed33b7098f1c65f22c4618c79a98dc86e50b58fd7f2d3cae94cf9ce36ac\"}}";
        let describe_resp = ")]}'\n{\"tags\": [{\"key\": \"version\", \"value\": \"153.05\"}]}";

        let meta = parse_cipd_version_responses(resolve_resp, Some(describe_resp)).unwrap();
        assert_eq!(meta.version, "153.05");
        assert_eq!(
            meta.instance_digest,
            "cd837ed33b7098f1c65f22c4618c79a98dc86e50b58fd7f2d3cae94cf9ce36ac"
        );

        // Fallback to short digest when describe body is unavailable
        let fallback = parse_cipd_version_responses(resolve_resp, None).unwrap();
        assert_eq!(fallback.version, "cd837ed33b70");
    }

    #[test]
    fn test_calculate_screen_ai_target_dimensions() {
        assert_eq!(calculate_screen_ai_target_dimensions(0, 1080, 2048), (0, 0));
        assert_eq!(
            calculate_screen_ai_target_dimensions(1920, 1080, 2048),
            (1920, 1080)
        );
        assert_eq!(
            calculate_screen_ai_target_dimensions(3840, 2160, 2048),
            (2048, 1152)
        );
        assert_eq!(
            calculate_screen_ai_target_dimensions(1200, 2400, 2048),
            (1024, 2048)
        );
    }

    #[test]
    fn test_convert_and_scale_gl_rgba_to_bgra_flip_and_swizzle() {
        // 2x2 OpenGL RGBA image:
        // Row 0 (bottom in GL): red (255,0,0,255), green (0,255,0,255)
        // Row 1 (top in GL):    blue (0,0,255,255), yellow (255,255,0,255)
        let raw_rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 255, // bottom row
            0, 0, 255, 255, 255, 255, 0, 255, // top row
        ];
        let mut out_bgra = Vec::new();
        convert_and_scale_gl_rgba_to_bgra(&raw_rgba, 2, 2, 2, 2, &mut out_bgra);

        // Top row in output should be GL row 1 in BGRA order:
        // blue -> [255, 0, 0, 255], yellow -> [0, 255, 255, 255]
        // Bottom row in output should be GL row 0 in BGRA order:
        // red -> [0, 0, 255, 255], green -> [0, 255, 0, 255]
        assert_eq!(
            out_bgra,
            vec![
                255, 0, 0, 255, 0, 255, 255, 255, // top row
                0, 0, 255, 255, 0, 255, 0, 255, // bottom row
            ]
        );
    }

    #[test]
    fn test_rect_to_normalized_box_horizontal_and_rotated_vertical() {
        // Horizontal rect at (100, 50) with w=200, h=40 on 1000x500 image
        let h_rect = ScreenAiRect {
            x: 100,
            y: 50,
            width: 200,
            height: 40,
            angle: 0.0,
        };
        let (cx, cy, w, h, _) = rect_to_normalized_box(&h_rect, 1000, 500).unwrap();
        assert!((cx - 0.20).abs() < 1e-5);
        assert!((cy - 0.14).abs() < 1e-5);
        assert!((w - 0.20).abs() < 1e-5);
        assert!((h - 0.08).abs() < 1e-5);

        // Vertical rect rotated 90 degrees clockwise at (298, 55) with w=325 (vertical length),
        // h=47 (horizontal column thickness) on 400x600 image
        let v_rect = ScreenAiRect {
            x: 298,
            y: 55,
            width: 325,
            height: 47,
            angle: 90.0,
        };
        let (vcx, vcy, vw, vh, _) = rect_to_normalized_box(&v_rect, 400, 600).unwrap();
        assert!((vcx - 274.5 / 400.0).abs() < 1e-4);
        assert!((vcy - 217.5 / 600.0).abs() < 1e-4);
        assert!((vw - 47.0 / 400.0).abs() < 1e-4);
        assert!((vh - 325.0 / 600.0).abs() < 1e-4);
    }

    #[test]
    fn test_parse_visual_annotation_horizontal_and_dot_leader() {
        // Construct a line: "……テスト" where "……" is transcribed as 6 '…' symbols over 2 character cells (80px)
        // and "テ", "ス", "ト" are 40px wide each on a 1000x500 screen.
        let mut symbols = Vec::new();
        for i in 0..6 {
            symbols.push(ScreenAiSymbolBox {
                bounding_box: Some(ScreenAiRect {
                    x: 100 + i * 13,
                    y: 100,
                    width: 13,
                    height: 40,
                    angle: 0.0,
                }),
                utf8_string: "…".to_string(),
                confidence: 0.95,
            });
        }
        for (idx, ch) in ["テ", "ス", "ト"].iter().enumerate() {
            symbols.push(ScreenAiSymbolBox {
                bounding_box: Some(ScreenAiRect {
                    x: 180 + (idx as i32) * 40,
                    y: 100,
                    width: 40,
                    height: 40,
                    angle: 0.0,
                }),
                utf8_string: (*ch).to_string(),
                confidence: 0.99,
            });
        }

        let annotation = VisualAnnotation {
            lines: vec![ScreenAiLineBox {
                words: vec![ScreenAiWordBox {
                    symbols,
                    bounding_box: Some(ScreenAiRect {
                        x: 100,
                        y: 100,
                        width: 200,
                        height: 40,
                        angle: 0.0,
                    }),
                    utf8_string: "………………テスト".to_string(),
                    language: "ja".to_string(),
                    has_space_after: false,
                    direction: 1,
                    content_type: 0,
                    confidence: 0.98,
                    whitespace_bounding_box: None,
                }],
                bounding_box: Some(ScreenAiRect {
                    x: 100,
                    y: 100,
                    width: 200,
                    height: 40,
                    angle: 0.0,
                }),
                utf8_string: "………………テスト".to_string(),
                language: "ja".to_string(),
                block_id: 0,
                direction: 1,
                content_type: 0,
                confidence: 0.98,
                paragraph_id: 0,
            }],
        };

        let mut buf = Vec::new();
        annotation.encode(&mut buf).unwrap();

        let lines = parse_visual_annotation_to_lines(&buf, 1000, 500).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].writing_direction, WritingDirection::LeftToRight);
        assert_eq!(lines[0].text, "……テスト");
        assert_eq!(lines[0].char_spans.len(), 5);
    }

    #[test]
    fn test_parse_visual_annotation_vertical_columns_and_grouping() {
        // Two vertical Japanese columns on a 400x600 screen:
        // Right column (x=298): "縦書き"
        // Left column  (x=245): "日本語"
        let make_vert_col = |x: i32, chars: &[&str], block_id: i32| -> ScreenAiLineBox {
            let symbols: Vec<ScreenAiSymbolBox> = chars
                .iter()
                .enumerate()
                .map(|(i, c)| ScreenAiSymbolBox {
                    bounding_box: Some(ScreenAiRect {
                        x,
                        y: 60 + (i as i32) * 50,
                        width: 48,
                        height: 46,
                        angle: 90.0,
                    }),
                    utf8_string: (*c).to_string(),
                    confidence: 0.99,
                })
                .collect();
            let text: String = chars.iter().copied().collect();
            let total_len = (chars.len() as i32) * 50;
            ScreenAiLineBox {
                words: vec![ScreenAiWordBox {
                    symbols,
                    bounding_box: Some(ScreenAiRect {
                        x,
                        y: 60,
                        width: total_len,
                        height: 46,
                        angle: 90.0,
                    }),
                    utf8_string: text.clone(),
                    language: "ja".to_string(),
                    has_space_after: false,
                    direction: 3, // DIRECTION_TOP_TO_BOTTOM
                    content_type: 0,
                    confidence: 0.99,
                    whitespace_bounding_box: None,
                }],
                bounding_box: Some(ScreenAiRect {
                    x,
                    y: 60,
                    width: total_len,
                    height: 46,
                    angle: 90.0,
                }),
                utf8_string: text,
                language: "ja".to_string(),
                block_id,
                direction: 3, // DIRECTION_TOP_TO_BOTTOM
                content_type: 0,
                confidence: 0.99,
                paragraph_id: 0,
            }
        };

        let annotation = VisualAnnotation {
            lines: vec![
                make_vert_col(298, &["縦", "書", "き"], 0),
                make_vert_col(245, &["日", "本", "語"], 0),
            ],
        };

        let mut buf = Vec::new();
        annotation.encode(&mut buf).unwrap();

        let lines = parse_visual_annotation_to_lines(&buf, 400, 600).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].writing_direction, WritingDirection::TopToBottom);
        assert_eq!(lines[1].writing_direction, WritingDirection::TopToBottom);
        assert_eq!(lines[0].char_spans.len(), 3);
        assert_eq!(lines[1].char_spans.len(), 3);

        // Verify vertical block grouping merges right-to-left columns into "縦書き日本語"
        let boxes = group_lines_into_blocks(&lines, 0.6);
        assert_eq!(boxes[0].writing_direction, WritingDirection::TopToBottom);
        assert_eq!(boxes[0].text, "縦書き日本語");
    }

    #[test]
    fn test_extract_screen_ai_archive_and_metadata_roundtrip() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "michadame-screen-ai-extract-test-{}-{}",
            std::process::id(),
            crate::replay::now_us()
        ));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let zip_path = tmp_dir.join("pkg.zip");
        let target_dir = tmp_dir.join("installed");

        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();

            zip.start_file(".cipdpkg/manifest.json", options).unwrap();
            zip.write_all(b"{}").unwrap();

            zip.start_file("resources/libchromescreenai.so", options)
                .unwrap();
            zip.write_all(b"\x7fELF-dummy-so").unwrap();

            zip.start_file("resources/files_list_ocr.txt", options)
                .unwrap();
            zip.write_all(b"gocr/model.tflite\n").unwrap();

            zip.start_file("resources/gocr/model.tflite", options)
                .unwrap();
            zip.write_all(b"dummy-tflite").unwrap();

            zip.finish().unwrap();
        }

        extract_screen_ai_archive(&zip_path, &target_dir, &|_, _| {}).unwrap();
        assert!(is_screen_ai_installed(&target_dir));
        assert!(!target_dir.join(".cipdpkg").exists());
        assert!(resolve_model_file_path(&target_dir, "gocr/model.tflite").is_some());
        assert!(resolve_model_file_path(&target_dir, "../outside").is_none());

        let meta = ScreenAiMetadata {
            version: "153.05".to_string(),
            instance_digest: "cd837ed33b70".to_string(),
        };
        save_installed_metadata(&target_dir, &meta).unwrap();
        let loaded = load_installed_metadata(&target_dir).unwrap();
        assert_eq!(loaded, meta);

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    fn write_screen_ai_test_zip(zip_path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn test_staged_screen_ai_extraction_failures_preserve_existing_installation() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let target_dir = tmp_dir.path().join("screen_ai");

        // 1. Seed a valid v1 installation
        let v1_zip = tmp_dir.path().join("v1.zip");
        write_screen_ai_test_zip(
            &v1_zip,
            &[
                ("resources/libchromescreenai.so", b"\x7fELF-valid-v1-so"),
                ("resources/files_list_ocr.txt", b"gocr/model.tflite\n"),
                ("resources/gocr/model.tflite", b"v1-model-weights"),
            ],
        );
        let v1_meta = ScreenAiMetadata {
            version: "153.01".to_string(),
            instance_digest: "111111111111".to_string(),
        };
        install_screen_ai_archive(&v1_zip, &target_dir, &v1_meta, &|_, _| {}).unwrap();
        assert!(is_screen_ai_installed(&target_dir));
        assert_eq!(load_installed_metadata(&target_dir), Some(v1_meta.clone()));

        let assert_v1_intact = |target_dir: &Path| {
            assert!(is_screen_ai_installed(target_dir));
            assert_eq!(load_installed_metadata(target_dir), Some(v1_meta.clone()));
            let model_path = resolve_model_file_path(target_dir, "gocr/model.tflite").unwrap();
            assert_eq!(std::fs::read(model_path).unwrap(), b"v1-model-weights");
            let so_path = resolve_model_file_path(target_dir, "libchromescreenai.so").unwrap();
            assert_eq!(std::fs::read(so_path).unwrap(), b"\x7fELF-valid-v1-so");
            // Ensure no leftover .staging-* directories remain
            if let Ok(entries) = std::fs::read_dir(target_dir.join("versions")) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    assert!(
                        !name_str.starts_with(".staging-"),
                        "Leftover staging dir found: {name_str}"
                    );
                }
            }
        };

        let bad_meta = ScreenAiMetadata {
            version: "153.99".to_string(),
            instance_digest: "999999999999".to_string(),
        };

        // Case (a): Corrupt non-ZIP archive
        let corrupt_zip = tmp_dir.path().join("corrupt.zip");
        std::fs::write(&corrupt_zip, b"not a zip archive at all").unwrap();
        assert!(
            install_screen_ai_archive(&corrupt_zip, &target_dir, &bad_meta, &|_, _| {}).is_err()
        );
        assert_v1_intact(&target_dir);

        // Case (b): libchromescreenai.so without ELF magic header
        let non_elf_zip = tmp_dir.path().join("non_elf.zip");
        write_screen_ai_test_zip(
            &non_elf_zip,
            &[
                (
                    "resources/libchromescreenai.so",
                    b"NOT_AN_ELF_SHARED_OBJECT",
                ),
                ("resources/files_list_ocr.txt", b"gocr/model.tflite\n"),
                ("resources/gocr/model.tflite", b"v2-model-weights"),
            ],
        );
        assert!(
            install_screen_ai_archive(&non_elf_zip, &target_dir, &bad_meta, &|_, _| {}).is_err()
        );
        assert_v1_intact(&target_dir);

        // Case (c): files_list_ocr.txt references a missing model file
        let missing_model_zip = tmp_dir.path().join("missing_model.zip");
        write_screen_ai_test_zip(
            &missing_model_zip,
            &[
                ("resources/libchromescreenai.so", b"\x7fELF-valid-v2-so"),
                (
                    "resources/files_list_ocr.txt",
                    b"gocr/model.tflite\ngocr/missing_detector.tflite\n",
                ),
                ("resources/gocr/model.tflite", b"v2-model-weights"),
            ],
        );
        assert!(
            install_screen_ai_archive(&missing_model_zip, &target_dir, &bad_meta, &|_, _| {})
                .is_err()
        );
        assert_v1_intact(&target_dir);

        // Case (d): manifest.json references a missing file
        let bad_manifest_zip = tmp_dir.path().join("bad_manifest.zip");
        write_screen_ai_test_zip(
            &bad_manifest_zip,
            &[
                ("resources/libchromescreenai.so", b"\x7fELF-valid-v2-so"),
                ("resources/files_list_ocr.txt", b"gocr/model.tflite\n"),
                ("resources/gocr/model.tflite", b"v2-model-weights"),
                (
                    "resources/manifest.json",
                    br#"{"files":["gocr/model.tflite","gocr/missing_manifest_file.tflite"]}"#,
                ),
            ],
        );
        assert!(
            install_screen_ai_archive(&bad_manifest_zip, &target_dir, &bad_meta, &|_, _| {})
                .is_err()
        );
        assert_v1_intact(&target_dir);
    }

    #[test]
    fn test_pinned_runtime_isolation_across_versioned_update() {
        use std::ffi::CString;

        let tmp_dir = tempfile::tempdir().unwrap();
        let target_dir = tmp_dir.path().join("screen_ai");

        reset_pinned_runtime_state();

        // 1. Install v1 and pin runtime to v1
        let v1_zip = tmp_dir.path().join("v1.zip");
        write_screen_ai_test_zip(
            &v1_zip,
            &[
                ("resources/libchromescreenai.so", b"\x7fELF-v1-so"),
                ("resources/files_list_ocr.txt", b"gocr/model.tflite\n"),
                ("resources/gocr/model.tflite", b"model-v1-weights"),
            ],
        );
        let v1_meta = ScreenAiMetadata {
            version: "153.01".to_string(),
            instance_digest: "aaaa11112222".to_string(),
        };
        let v1_dir = install_screen_ai_archive(&v1_zip, &target_dir, &v1_meta, &|_, _| {}).unwrap();
        let pin = pin_runtime_to_installed_version(&target_dir).unwrap();
        assert_eq!(pin.runtime_dir, v1_dir);
        assert_eq!(active_loaded_screen_ai_metadata(), Some(v1_meta.clone()));
        assert!(!is_screen_ai_restart_required(&target_dir));

        let c_rel = CString::new("gocr/model.tflite").unwrap();
        let read_via_cb = || -> Vec<u8> {
            let sz = get_file_content_size_cb(c_rel.as_ptr());
            let mut buf = vec![0u8; sz as usize];
            get_file_content_cb(c_rel.as_ptr(), sz, buf.as_mut_ptr());
            buf
        };
        assert_eq!(read_via_cb(), b"model-v1-weights");

        // 2. Install v2 while v1 runtime is still pinned
        let v2_zip = tmp_dir.path().join("v2.zip");
        write_screen_ai_test_zip(
            &v2_zip,
            &[
                ("resources/libchromescreenai.so", b"\x7fELF-v2-so"),
                ("resources/files_list_ocr.txt", b"gocr/model.tflite\n"),
                ("resources/gocr/model.tflite", b"model-v2-updated-weights"),
            ],
        );
        let v2_meta = ScreenAiMetadata {
            version: "153.05".to_string(),
            instance_digest: "bbbb33334444".to_string(),
        };
        let v2_dir = install_screen_ai_archive(&v2_zip, &target_dir, &v2_meta, &|_, _| {}).unwrap();
        assert_ne!(v1_dir, v2_dir);

        // Disk metadata reflects v2, while active loaded runtime remains pinned to v1
        assert_eq!(load_installed_metadata(&target_dir), Some(v2_meta.clone()));
        assert_eq!(active_loaded_screen_ai_metadata(), Some(v1_meta.clone()));
        assert!(is_screen_ai_restart_required(&target_dir));
        assert_eq!(active_runtime_model_dir(), v1_dir);
        assert_eq!(
            read_via_cb(),
            b"model-v1-weights",
            "Pinned runtime callbacks must continue reading v1 model files until restart"
        );

        // 3. Simulate restart -> runtime pins to v2 and reads v2 model files
        reset_pinned_runtime_state();
        let pin2 = pin_runtime_to_installed_version(&target_dir).unwrap();
        assert_eq!(pin2.runtime_dir, v2_dir);
        assert_eq!(active_loaded_screen_ai_metadata(), Some(v2_meta));
        assert!(!is_screen_ai_restart_required(&target_dir));
        assert_eq!(read_via_cb(), b"model-v2-updated-weights");

        reset_pinned_runtime_state();
    }

    #[test]
    fn test_build_cipd_instance_download_url_rejects_mutable_and_encodes_sha256() {
        let empty_meta = ScreenAiMetadata {
            version: "latest".to_string(),
            instance_digest: String::new(),
        };
        assert!(build_cipd_instance_download_url(&empty_meta).is_err());

        let latest_meta = ScreenAiMetadata {
            version: "latest".to_string(),
            instance_digest: "latest".to_string(),
        };
        assert!(build_cipd_instance_download_url(&latest_meta).is_err());

        let valid_sha256 = ScreenAiMetadata {
            version: "153.05".to_string(),
            instance_digest: "cd837ed33b7098f1c65f22c4618c79a98dc86e50b58fd7f2d3cae94cf9ce36ac"
                .to_string(),
        };
        let url = build_cipd_instance_download_url(&valid_sha256).unwrap();
        // Known CIPD instance ID resolved from the upstream registry. Checking
        // only the URL prefix previously missed an invalid hash encoding.
        assert_eq!(url,
            "https://chrome-infra-packages.appspot.com/dl/chromium/third_party/screen-ai/linux/+/zYN-0ztwmPHGXyLEYYx5qY3IblC1j9fy08rpTPnONqwC");
    }

    #[test]
    #[ignore = "downloads the live Screen AI package; requires MICHADAME_TEST_SCREEN_AI_DIR"]
    fn download_and_initialize_screen_ai() {
        let target = std::env::var_os("MICHADAME_TEST_SCREEN_AI_DIR")
            .expect("Set MICHADAME_TEST_SCREEN_AI_DIR to a disposable installation directory");
        let target = Path::new(&target);
        let metadata = download_and_install_screen_ai(target, |message, _| {
            eprintln!("{message}");
        })
        .expect("Screen AI download and installation failed");
        assert!(is_screen_ai_installed(target));
        assert_eq!(load_installed_metadata(target), Some(metadata));
        let max_dimension =
            warmup_screen_ai(target).expect("Downloaded Screen AI failed to initialize");
        assert!(max_dimension >= 1024);
        eprintln!("Downloaded Screen AI initialized; maximum dimension: {max_dimension}");
    }

    #[test]
    #[ignore]
    fn test_live_screen_ai_ocr_execution() {
        let model_dir = std::env::var_os("MICHADAME_TEST_SCREEN_AI_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(resolve_screen_ai_directory);
        if !is_screen_ai_installed(&model_dir) {
            return;
        }
        let max_dim = warmup_screen_ai(&model_dir).expect("Failed to warmup Screen AI");
        assert!(max_dim >= 1024);

        let img = image::open("examples/retro.png")
            .or_else(|_| image::open("assets/logo.png"))
            .expect("Failed to open test image");
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        let mut bgra = Vec::with_capacity((w * h * 4) as usize);
        for px in rgba.pixels() {
            bgra.push(px[2]);
            bgra.push(px[1]);
            bgra.push(px[0]);
            bgra.push(255);
        }

        let lines = execute_screen_ai_ocr_bgra(&model_dir, &bgra, w, h)
            .expect("Failed to execute Screen AI OCR");
        assert!(
            !lines.is_empty(),
            "Expected local Screen AI OCR to detect text"
        );
        let boxes = group_lines_into_blocks(&lines, 0.6);
        assert!(!boxes.is_empty());
    }
}
