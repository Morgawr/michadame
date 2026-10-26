use super::lens::{build_line_text_and_spans, estimate_line_char_em, is_dot_leader_char};
use super::models::{CenterRotatedBox, Geometry, ParsedLine, TextLayoutWord, WritingDirection};
use crossbeam_channel::Sender;
use prost::Message;
use serde::{Deserialize, Serialize};
use std::ffi::{c_char, c_void, CStr};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

pub const CIPD_PACKAGE: &str = "chromium/third_party/screen-ai/linux";
pub const CIPD_RESOLVE_URL: &str =
    "https://chrome-infra-packages.appspot.com/prpc/cipd.Repository/ResolveVersion";
pub const CIPD_DESCRIBE_URL: &str =
    "https://chrome-infra-packages.appspot.com/prpc/cipd.Repository/DescribeInstance";
pub const CIPD_DOWNLOAD_URL: &str =
    "https://chrome-infra-packages.appspot.com/dl/chromium/third_party/screen-ai/linux/+/latest";

pub const DEFAULT_MAX_DIMENSION: u32 = 2048;

/// Metadata describing an installed or remote Chrome Screen AI OCR package.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScreenAiMetadata {
    pub version: String,
    pub instance_digest: String,
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

/// Resolves a relative model file path inside `screen_ai_dir`, checking both the root
/// directory and a `resources/` subdirectory.
pub fn resolve_model_file_path(dir: &Path, rel_path: &str) -> Option<PathBuf> {
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

/// Returns true if the Chrome Screen AI shared library and model manifest are present on disk.
pub fn is_screen_ai_installed(dir: &Path) -> bool {
    resolve_model_file_path(dir, "libchromescreenai.so").is_some()
        && resolve_model_file_path(dir, "files_list_ocr.txt").is_some()
}

/// Loads installed `ScreenAiMetadata` from `dir/metadata.json` if the model files exist.
pub fn load_installed_metadata(dir: &Path) -> Option<ScreenAiMetadata> {
    if !is_screen_ai_installed(dir) {
        return None;
    }
    let meta_path = dir.join("metadata.json");
    if let Ok(content) = std::fs::read_to_string(&meta_path) {
        if let Ok(meta) = serde_json::from_str::<ScreenAiMetadata>(&content) {
            return Some(meta);
        }
    }
    Some(ScreenAiMetadata {
        version: "installed".to_string(),
        instance_digest: String::new(),
    })
}

/// Saves `ScreenAiMetadata` to `dir/metadata.json`.
pub fn save_installed_metadata(dir: &Path, meta: &ScreenAiMetadata) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create Screen AI directory: {e}"))?;
    let meta_path = dir.join("metadata.json");
    let json = serde_json::to_string_pretty(meta)
        .map_err(|e| format!("Failed to serialize Screen AI metadata: {e}"))?;
    std::fs::write(&meta_path, json).map_err(|e| format!("Failed to write Screen AI metadata: {e}"))
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

/// Downloads and extracts the latest Chrome Screen AI Linux package from CIPD into `target_dir`.
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
    let meta = check_remote_screen_ai_version().unwrap_or_else(|_| ScreenAiMetadata {
        version: "latest".to_string(),
        instance_digest: String::new(),
    });

    progress_cb(
        format!("Downloading Chrome Screen AI ({})...", meta.version),
        0.05,
    );

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        .build();

    let resp = agent
        .get(CIPD_DOWNLOAD_URL)
        .call()
        .map_err(|e| format!("Failed to download Chrome Screen AI package: {e}"))?;

    let total_bytes = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(78_000_000);

    let tmp_zip_path = target_dir.join("screen_ai_download.zip.tmp");
    {
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
    }

    progress_cb(
        "Extracting Chrome Screen AI model files...".to_string(),
        0.76,
    );

    let extract_result = extract_screen_ai_archive(&tmp_zip_path, target_dir, &progress_cb);
    let _ = std::fs::remove_file(&tmp_zip_path);
    extract_result?;

    save_installed_metadata(target_dir, &meta)?;
    update_active_model_dir(target_dir);

    progress_cb("Chrome Screen AI local model ready.".to_string(), 1.0);
    Ok(meta)
}

fn extract_screen_ai_archive<F>(
    zip_path: &Path,
    target_dir: &Path,
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

        // Strip leading "resources/" prefix so model files sit directly in target_dir
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
            continue;
        }

        let out_path = target_dir.join(rel_path);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("Failed to create directory {}: {e}", out_path.display()))?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent dir {}: {e}", parent.display()))?;
        }

        let tmp_out = out_path.with_extension("extract_tmp");
        {
            let mut out_file = std::fs::File::create(&tmp_out)
                .map_err(|e| format!("Failed to create file {}: {e}", tmp_out.display()))?;
            std::io::copy(&mut entry, &mut out_file)
                .map_err(|e| format!("Failed to extract {}: {e}", out_path.display()))?;
            out_file
                .flush()
                .map_err(|e| format!("Failed to flush {}: {e}", out_path.display()))?;
        }

        #[cfg(unix)]
        if out_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".so"))
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp_out, std::fs::Permissions::from_mode(0o755));
        }

        std::fs::rename(&tmp_out, &out_path)
            .map_err(|e| format!("Failed to finalize {}: {e}", out_path.display()))?;

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

    if !is_screen_ai_installed(target_dir) {
        return Err(
            "Extracted archive did not contain libchromescreenai.so and files_list_ocr.txt"
                .to_string(),
        );
    }

    Ok(())
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

fn active_model_dir_lock() -> &'static RwLock<PathBuf> {
    ACTIVE_MODEL_DIR.get_or_init(|| RwLock::new(resolve_screen_ai_directory()))
}

fn update_active_model_dir(dir: &Path) {
    if let Ok(mut guard) = active_model_dir_lock().write() {
        *guard = dir.to_path_buf();
    }
}

extern "C" fn get_file_content_size_cb(relative_file_path: *const c_char) -> u32 {
    if relative_file_path.is_null() {
        return 0;
    }
    let Ok(rel_str) = unsafe { CStr::from_ptr(relative_file_path) }.to_str() else {
        return 0;
    };
    let dir = match active_model_dir_lock().read() {
        Ok(g) => g.clone(),
        Err(_) => return 0,
    };
    let Some(full_path) = resolve_model_file_path(&dir, rel_str) else {
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
    let dir = match active_model_dir_lock().read() {
        Ok(g) => g.clone(),
        Err(_) => return,
    };
    let Some(full_path) = resolve_model_file_path(&dir, rel_str) else {
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
    update_active_model_dir(model_dir);

    if guard.is_none() {
        let so_path =
            resolve_model_file_path(model_dir, "libchromescreenai.so").ok_or_else(|| {
                format!(
                    "Chrome Screen AI library not found in {}",
                    model_dir.display()
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

    #[test]
    #[ignore]
    fn test_live_screen_ai_ocr_execution() {
        let model_dir = resolve_screen_ai_directory();
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
