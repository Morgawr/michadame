use crate::config::VoiceModelId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const VC_MANIFEST_VERSION: &str = "rvc-v2-onnx-1";

pub const ORT_WEBGPU_WHEEL_URL: &str =
    "https://files.pythonhosted.org/packages/24/11/6cd5f840be97819cbd172bacc1868726ab5fa54ceaaa79508c5a64e61233/onnxruntime_webgpu-1.27.0-cp312-cp312-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl";
pub const ORT_WEBGPU_WHEEL_SHA256: &str =
    "fe5e49e00ad71bc667316d57670bfbf525438783d4a3468e0d337314ec985cbe";
pub const ORT_WEBGPU_WHEEL_BYTES: u64 = 25_163_729;
pub const ORT_LIB_SONAME: &str = "libonnxruntime.so.1.27.0";
pub const ORT_LIB_LINKNAME: &str = "libonnxruntime.so";
pub const ORT_SHARED_PROVIDER_NAME: &str = "libonnxruntime_providers_shared.so";

pub const CONTENTVEC_FILENAME: &str = "contentvec_768l12.onnx";
pub const CONTENTVEC_URL: &str =
    "https://huggingface.co/TigreGotico/voiceclonnx-rvc/resolve/main/contentvec_768l12.onnx";
pub const CONTENTVEC_SHA256: &str =
    "cbddd8fa9352b3128df6359e2e3da6be0f9072e768e51e3efc3004e5030ea97f";
pub const CONTENTVEC_EXPECTED_BYTES: u64 = 377_755_911;

pub const RMVPE_FILENAME: &str = "rmvpe.onnx";
pub const RMVPE_URL: &str =
    "https://huggingface.co/TigreGotico/voiceclonnx-rvc/resolve/main/rmvpe.onnx";
pub const RMVPE_SHA256: &str = "5370e71ac80af8b4b7c793d27efd51fd8bf962de3a7ede0766dac0befa3660fd";
pub const RMVPE_EXPECTED_BYTES: u64 = 361_688_443;

pub struct BuiltinVoiceAssetSpec {
    pub url: &'static str,
    pub sha256: &'static str,
    pub expected_bytes: u64,
}

pub fn builtin_voice_spec(id: VoiceModelId) -> BuiltinVoiceAssetSpec {
    match id {
        VoiceModelId::TsukuyomiChan => BuiltinVoiceAssetSpec {
            url: "https://huggingface.co/wok000/vcclient_model/resolve/main/rvc_v2_alpha/tsukuyomi-chan/tsukuyomi_v2_40k_e100_float.onnx",
            sha256: "aa8be3d080508b4766152ddf5946692618e6f521da0935d0c85c9a49f0ccb4e9",
            expected_bytes: 110_411_680,
        },
        VoiceModelId::Amitaro => BuiltinVoiceAssetSpec {
            url: "https://huggingface.co/wok000/vcclient_model/resolve/main/rvc_v2_alpha/amitaro/amitaro_v2_40k_e100_float.onnx",
            sha256: "473ac222cbdcd2258edc4fb4ab8663398dd398912f9e01ab970b992c17937cb4",
            expected_bytes: 110_411_680,
        },
        VoiceModelId::KikotoKurage => BuiltinVoiceAssetSpec {
            url: "https://huggingface.co/wok000/vcclient_model/resolve/main/rvc_v2_alpha/kikoto_kurage/kikoto_kurage_v2_40k_e100_float.onnx",
            sha256: "91b8541b79e7f15e483eb8447247843ddec013bc711fe13bb0a1e11d924ba465",
            expected_bytes: 110_411_680,
        },
        VoiceModelId::KikotoMahiro => BuiltinVoiceAssetSpec {
            url: "https://huggingface.co/wok000/vcclient_model/resolve/main/rvc_v2_alpha/kikoto_mahiro/kikoto_mahiro_v2_40k_float.onnx",
            sha256: "29d7b1a16be196cb3dca2b27267c488134638d6c6651ea3ea9d7d1cc80ac0d5c",
            expected_bytes: 110_411_680,
        },
        VoiceModelId::TokinaShigure => BuiltinVoiceAssetSpec {
            url: "https://huggingface.co/wok000/vcclient_model/resolve/main/rvc_v2_alpha/tokina_shigure/tokina_shigure_v2_40k_e100_float.onnx",
            sha256: "7d0e915f623b1ec32da110a870c2f7f3bdb70f62ad3b6b91489a94421d21fb58",
            expected_bytes: 110_411_680,
        },
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct VcManifest {
    pub version: String,
    pub runtime_wheel_sha256: String,
    pub contentvec_sha256: String,
    pub rmvpe_sha256: String,
    pub voices: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VcSyncTarget {
    BaseEngine,
    VoiceModel(VoiceModelId),
}

#[derive(Clone, Debug)]
pub enum VcSyncEvent {
    Progress {
        message: String,
        progress: f32,
    },
    Finished {
        target: VcSyncTarget,
        result: Result<(), String>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VcInstallStatus {
    pub runtime_ready: bool,
    pub base_models_ready: bool,
    pub voices_ready: [bool; 5],
}

impl VcInstallStatus {
    pub fn is_engine_ready(&self) -> bool {
        (self.runtime_ready || has_system_ort_dylib()) && self.base_models_ready
    }

    pub fn is_voice_installed(&self, id: VoiceModelId) -> bool {
        let idx = VoiceModelId::ALL.iter().position(|&x| x == id).unwrap_or(0);
        self.voices_ready[idx]
    }
}

static STAGING_SEQ: AtomicU64 = AtomicU64::new(1);
const INSTALL_CACHE_TTL: Duration = Duration::from_secs(2);
static INSTALL_CACHE: OnceLock<Mutex<Option<(PathBuf, Instant, VcInstallStatus)>>> =
    OnceLock::new();

fn install_cache() -> &'static Mutex<Option<(PathBuf, Instant, VcInstallStatus)>> {
    INSTALL_CACHE.get_or_init(|| Mutex::new(None))
}

pub fn invalidate_vc_install_cache() {
    if let Ok(mut guard) = install_cache().lock() {
        *guard = None;
    }
}

/// Resolves `~/.config/michadame/voice_changer` alongside the app config root.
pub fn resolve_voice_changer_directory() -> PathBuf {
    if let Ok(config_path) = confy::get_configuration_file_path("michadame", None) {
        if let Some(parent) = config_path.parent() {
            return parent.join("voice_changer");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".config")
            .join("michadame")
            .join("voice_changer")
    } else {
        PathBuf::from("./voice_changer")
    }
}

pub fn runtime_dir(base_dir: &Path) -> PathBuf {
    base_dir.join("runtime")
}

pub fn runtime_dylib_path(base_dir: &Path) -> PathBuf {
    runtime_dir(base_dir).join(ORT_LIB_SONAME)
}

pub fn contentvec_model_path(base_dir: &Path) -> PathBuf {
    base_dir.join("base").join(CONTENTVEC_FILENAME)
}

pub fn rmvpe_model_path(base_dir: &Path) -> PathBuf {
    base_dir.join("base").join(RMVPE_FILENAME)
}

pub fn builtin_voice_model_path(base_dir: &Path, id: VoiceModelId) -> PathBuf {
    base_dir.join("models").join(id.filename())
}

pub fn manifest_path(base_dir: &Path) -> PathBuf {
    base_dir.join("manifest.json")
}

pub fn has_system_ort_dylib() -> bool {
    if let Ok(p) = std::env::var("ORT_DYLIB_PATH") {
        if !p.is_empty() && Path::new(&p).is_file() {
            return true;
        }
    }
    [
        "/opt/rocm/lib/libonnxruntime.so",
        "/usr/lib64/libonnxruntime.so",
        "/usr/lib/x86_64-linux-gnu/libonnxruntime.so",
        "/usr/lib/libonnxruntime.so",
    ]
    .iter()
    .any(|p| Path::new(p).is_file())
}

fn load_manifest(base_dir: &Path) -> VcManifest {
    std::fs::read_to_string(manifest_path(base_dir))
        .ok()
        .and_then(|s| serde_json::from_str::<VcManifest>(&s).ok())
        .unwrap_or_default()
}

fn save_manifest_atomic(base_dir: &Path, manifest: &VcManifest) -> Result<(), String> {
    std::fs::create_dir_all(base_dir)
        .map_err(|e| format!("Failed to create voice changer directory: {e}"))?;
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|e| format!("Failed to serialize voice changer manifest: {e}"))?;
    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = base_dir.join(format!(".vc_manifest_{}_{}.tmp", std::process::id(), seq));
    std::fs::write(&tmp, json.as_bytes())
        .map_err(|e| format!("Failed to write temporary manifest: {e}"))?;
    std::fs::rename(&tmp, manifest_path(base_dir)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Failed to finalize voice changer manifest: {e}")
    })
}

pub fn query_vc_install_status(base_dir: &Path) -> VcInstallStatus {
    if let Ok(guard) = install_cache().lock() {
        if let Some((cached_dir, checked_at, status)) = &*guard {
            if cached_dir == base_dir && checked_at.elapsed() < INSTALL_CACHE_TTL {
                return *status;
            }
        }
    }

    let manifest = load_manifest(base_dir);
    let rt_file = runtime_dylib_path(base_dir);
    let runtime_ready = rt_file.is_file()
        && std::fs::metadata(&rt_file).is_ok_and(|m| m.len() > 5_000_000)
        && (manifest.runtime_wheel_sha256.is_empty()
            || manifest.runtime_wheel_sha256 == ORT_WEBGPU_WHEEL_SHA256);

    let cv_file = contentvec_model_path(base_dir);
    let rm_file = rmvpe_model_path(base_dir);
    let base_models_ready = cv_file.is_file()
        && rm_file.is_file()
        && std::fs::metadata(&cv_file).is_ok_and(|m| m.len() > 100_000_000)
        && std::fs::metadata(&rm_file).is_ok_and(|m| m.len() > 100_000_000)
        && (manifest.contentvec_sha256.is_empty()
            || manifest.contentvec_sha256 == CONTENTVEC_SHA256)
        && (manifest.rmvpe_sha256.is_empty() || manifest.rmvpe_sha256 == RMVPE_SHA256);

    let mut voices_ready = [false; 5];
    for (idx, &id) in VoiceModelId::ALL.iter().enumerate() {
        let spec = builtin_voice_spec(id);
        let p = builtin_voice_model_path(base_dir, id);
        let sha_ok = manifest
            .voices
            .get(id.filename())
            .is_none_or(|recorded| recorded == spec.sha256);
        voices_ready[idx] =
            p.is_file() && std::fs::metadata(&p).is_ok_and(|m| m.len() > 10_000_000) && sha_ok;
    }

    let status = VcInstallStatus {
        runtime_ready,
        base_models_ready,
        voices_ready,
    };

    if let Ok(mut guard) = install_cache().lock() {
        *guard = Some((base_dir.to_path_buf(), Instant::now(), status));
    }
    status
}

pub fn delete_builtin_voice_model(base_dir: &Path, id: VoiceModelId) -> Result<(), String> {
    let path = builtin_voice_model_path(base_dir, id);
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("Failed to delete {}: {e}", id.short_label()))?;
    }
    let mut manifest = load_manifest(base_dir);
    if manifest.voices.remove(id.filename()).is_some() {
        let _ = save_manifest_atomic(base_dir, &manifest);
    }
    invalidate_vc_install_cache();
    Ok(())
}

/// Streaming FIPS 180-4 SHA-256 state so 370+ MB model downloads stream directly
/// to disk in 64 KB chunks without buffering hundreds of megabytes in memory.
pub struct Sha256Hasher {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_bytes: u64,
}

impl Default for Sha256Hasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256Hasher {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    pub fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: [0u8; 64],
            buf_len: 0,
            total_bytes: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total_bytes = self.total_bytes.wrapping_add(data.len() as u64);

        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress_block(&block);
                self.buf_len = 0;
            }
        }

        let mut chunks = data.chunks_exact(64);
        for block in &mut chunks {
            self.compress_block(block);
        }
        let rem = chunks.remainder();
        if !rem.is_empty() {
            self.buf[..rem.len()].copy_from_slice(rem);
            self.buf_len = rem.len();
        }
    }

    pub fn finalize_hex(mut self) -> String {
        let bit_len = self.total_bytes.wrapping_mul(8);
        self.buf[self.buf_len] = 0x80;
        self.buf_len += 1;

        if self.buf_len > 56 {
            self.buf[self.buf_len..64].fill(0);
            let block = self.buf;
            self.compress_block(&block);
            self.buf_len = 0;
        }
        self.buf[self.buf_len..56].fill(0);
        self.buf[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let final_block = self.buf;
        self.compress_block(&final_block);

        let mut out = String::with_capacity(64);
        for word in self.state {
            use std::fmt::Write as _;
            let _ = write!(out, "{word:08x}");
        }
        out
    }

    fn compress_block(&mut self, chunk: &[u8]) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(Self::K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

#[cfg(test)]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256Hasher::new();
    h.update(bytes);
    h.finalize_hex()
}

#[allow(clippy::too_many_arguments)]
fn stream_verified_file<F>(
    agent: &ureq::Agent,
    url: &str,
    expected_sha256: &str,
    fallback_size: u64,
    dest_path: &Path,
    label: &str,
    progress_base: f32,
    progress_span: f32,
    progress_cb: &mut F,
) -> Result<(), String>
where
    F: FnMut(String, f32),
{
    let parent = dest_path
        .parent()
        .ok_or_else(|| "Invalid destination path".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create directory {}: {e}", parent.display()))?;

    progress_cb(format!("Connecting for {label}..."), progress_base);

    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("Failed to download {label}: {e}"))?;

    let total_bytes = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(fallback_size);

    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let safe_tag: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let tmp_path = parent.join(format!(
        ".vc_dl_{}_{}_{}.tmp",
        safe_tag,
        std::process::id(),
        seq
    ));

    let dl_res = (|| -> Result<String, String> {
        let mut file = std::fs::File::create(&tmp_path)
            .map_err(|e| format!("Failed to create temp file for {label}: {e}"))?;
        let mut reader = resp.into_reader();
        let mut hasher = Sha256Hasher::new();
        let mut buf = [0u8; 65_536];
        let mut downloaded: u64 = 0;
        let mut last_report: u64 = 0;

        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("Network read error while downloading {label}: {e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| format!("Disk write error for {label}: {e}"))?;
            hasher.update(&buf[..n]);
            downloaded += n as u64;

            if downloaded.saturating_sub(last_report) >= 256 * 1024 || downloaded >= total_bytes {
                last_report = downloaded;
                let frac = (downloaded as f32 / total_bytes.max(1) as f32).clamp(0.0, 1.0);
                progress_cb(
                    format!(
                        "Downloading {label}... {:.1} / {:.1} MB",
                        downloaded as f32 / (1024.0 * 1024.0),
                        total_bytes as f32 / (1024.0 * 1024.0)
                    ),
                    progress_base + progress_span * frac,
                );
            }
        }
        file.flush()
            .map_err(|e| format!("Failed to flush {label} to disk: {e}"))?;
        Ok(hasher.finalize_hex())
    })();

    let actual_sha = match dl_res {
        Ok(sha) => sha,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }
    };

    if actual_sha != expected_sha256 {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!(
            "SHA-256 verification failed for {label}: expected {expected_sha256}, got {actual_sha}"
        ));
    }

    std::fs::rename(&tmp_path, dest_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to finalize {label} on disk: {e}")
    })
}

/// Extracts `libonnxruntime.so.1.27.0` and `libonnxruntime_providers_shared.so` from a verified
/// ONNX Runtime wheel `.whl` (ZIP) into `runtime/` and creates `libonnxruntime.so`.
pub fn extract_ort_runtime_wheel(wheel_path: &Path, rt_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(rt_dir)
        .map_err(|e| format!("Failed to create runtime dir {}: {e}", rt_dir.display()))?;

    let wheel_file = std::fs::File::open(wheel_path)
        .map_err(|e| format!("Failed to open downloaded runtime bundle: {e}"))?;
    let mut archive = zip::ZipArchive::new(wheel_file)
        .map_err(|e| format!("Corrupt runtime ZIP archive: {e}"))?;

    let mut found_main_so = false;
    for idx in 0..archive.len() {
        let mut entry = archive
            .by_index(idx)
            .map_err(|e| format!("ZIP entry error: {e}"))?;
        let name = entry.name().to_string();
        let target_filename = if name.ends_with(ORT_LIB_SONAME) {
            found_main_so = true;
            Some(ORT_LIB_SONAME)
        } else if name.ends_with(ORT_SHARED_PROVIDER_NAME) {
            Some(ORT_SHARED_PROVIDER_NAME)
        } else {
            None
        };

        if let Some(fname) = target_filename {
            let dest = rt_dir.join(fname);
            let tmp = rt_dir.join(format!(".{fname}.{}.tmp", std::process::id()));
            let mut out =
                std::fs::File::create(&tmp).map_err(|e| format!("Failed to stage {fname}: {e}"))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| format!("Failed to extract {fname}: {e}"))?;
            out.flush()
                .map_err(|e| format!("Failed to flush {fname}: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
            }
            std::fs::rename(&tmp, &dest).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("Failed to install {fname}: {e}")
            })?;
        }
    }

    if !found_main_so {
        return Err(format!(
            "Runtime archive did not contain expected {ORT_LIB_SONAME}"
        ));
    }

    // Ensure `libonnxruntime.so` symlink (or copy fallback) points to `libonnxruntime.so.1.27.0`
    let link_path = rt_dir.join(ORT_LIB_LINKNAME);
    let _ = std::fs::remove_file(&link_path);
    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(ORT_LIB_SONAME, &link_path).is_err() {
            let _ = std::fs::copy(rt_dir.join(ORT_LIB_SONAME), &link_path);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::copy(rt_dir.join(ORT_LIB_SONAME), &link_path);
    }

    Ok(())
}

fn build_http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(300))
        .build()
}

/// Ensures the ONNX Runtime GPU library + ContentVec + RMVPE base models are downloaded and verified.
pub fn download_and_install_base_engine<F>(
    base_dir: &Path,
    mut progress_cb: F,
) -> Result<(), String>
where
    F: FnMut(String, f32),
{
    download_base_engine_inner(base_dir, 0.0, 1.0, &mut progress_cb)?;
    invalidate_vc_install_cache();
    Ok(())
}

fn download_base_engine_inner<F>(
    base_dir: &Path,
    base_prog: f32,
    span_prog: f32,
    progress_cb: &mut F,
) -> Result<(), String>
where
    F: FnMut(String, f32),
{
    std::fs::create_dir_all(base_dir)
        .map_err(|e| format!("Failed to create {}: {e}", base_dir.display()))?;
    let agent = build_http_agent();
    let status = query_vc_install_status(base_dir);
    let mut manifest = load_manifest(base_dir);
    manifest.version = VC_MANIFEST_VERSION.to_string();

    if !status.runtime_ready {
        let wheel_tmp =
            runtime_dir(base_dir).join(format!(".ort_wheel_{}.whl", std::process::id()));
        stream_verified_file(
            &agent,
            ORT_WEBGPU_WHEEL_URL,
            ORT_WEBGPU_WHEEL_SHA256,
            ORT_WEBGPU_WHEEL_BYTES,
            &wheel_tmp,
            "ONNX Runtime (Vulkan GPU + CPU)",
            base_prog,
            span_prog * 0.06,
            progress_cb,
        )?;
        progress_cb(
            "Extracting ONNX Runtime GPU libraries...".to_string(),
            base_prog + span_prog * 0.07,
        );
        let extract_res = extract_ort_runtime_wheel(&wheel_tmp, &runtime_dir(base_dir));
        let _ = std::fs::remove_file(&wheel_tmp);
        extract_res?;
        manifest.runtime_wheel_sha256 = ORT_WEBGPU_WHEEL_SHA256.to_string();
        save_manifest_atomic(base_dir, &manifest)?;
    } else if manifest.runtime_wheel_sha256.is_empty() {
        manifest.runtime_wheel_sha256 = ORT_WEBGPU_WHEEL_SHA256.to_string();
    }

    let cv_path = contentvec_model_path(base_dir);
    let cv_installed = cv_path.is_file()
        && std::fs::metadata(&cv_path).is_ok_and(|m| m.len() == CONTENTVEC_EXPECTED_BYTES);
    if !cv_installed {
        stream_verified_file(
            &agent,
            CONTENTVEC_URL,
            CONTENTVEC_SHA256,
            CONTENTVEC_EXPECTED_BYTES,
            &cv_path,
            "ContentVec 768d Encoder",
            base_prog + span_prog * 0.08,
            span_prog * 0.46,
            progress_cb,
        )?;
    }
    manifest.contentvec_sha256 = CONTENTVEC_SHA256.to_string();
    save_manifest_atomic(base_dir, &manifest)?;

    let rm_path = rmvpe_model_path(base_dir);
    let rm_installed = rm_path.is_file()
        && std::fs::metadata(&rm_path).is_ok_and(|m| m.len() == RMVPE_EXPECTED_BYTES);
    if !rm_installed {
        stream_verified_file(
            &agent,
            RMVPE_URL,
            RMVPE_SHA256,
            RMVPE_EXPECTED_BYTES,
            &rm_path,
            "RMVPE Neural Pitch Tracker",
            base_prog + span_prog * 0.54,
            span_prog * 0.45,
            progress_cb,
        )?;
    }
    manifest.rmvpe_sha256 = RMVPE_SHA256.to_string();
    save_manifest_atomic(base_dir, &manifest)?;

    Ok(())
}

/// Downloads a built-in voice model (and automatically installs the base engine first if missing).
pub fn download_and_install_voice_model<F>(
    base_dir: &Path,
    model_id: VoiceModelId,
    mut progress_cb: F,
) -> Result<(), String>
where
    F: FnMut(String, f32),
{
    let status = query_vc_install_status(base_dir);
    let (voice_base, voice_span) = if !status.is_engine_ready() {
        download_base_engine_inner(base_dir, 0.0, 0.84, &mut progress_cb)?;
        (0.84, 0.15)
    } else {
        (0.02, 0.96)
    };

    let agent = build_http_agent();
    let spec = builtin_voice_spec(model_id);
    let dest = builtin_voice_model_path(base_dir, model_id);

    stream_verified_file(
        &agent,
        spec.url,
        spec.sha256,
        spec.expected_bytes,
        &dest,
        model_id.short_label(),
        voice_base,
        voice_span,
        &mut progress_cb,
    )?;

    let mut manifest = load_manifest(base_dir);
    if manifest.version.is_empty() {
        manifest.version = VC_MANIFEST_VERSION.to_string();
    }
    manifest
        .voices
        .insert(model_id.filename().to_string(), spec.sha256.to_string());
    save_manifest_atomic(base_dir, &manifest)?;
    invalidate_vc_install_cache();
    progress_cb(format!("{} ready!", model_id.short_label()), 1.0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streaming_sha256_matches_nist_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        // Multi-chunk streaming across 64-byte block boundaries
        let mut data = Vec::with_capacity(10_000);
        for i in 0..10_000usize {
            data.push((i.wrapping_mul(31) & 0xff) as u8);
        }
        let one_shot = sha256_hex(&data);
        let mut chunked = Sha256Hasher::new();
        for part in data.chunks(37) {
            chunked.update(part);
        }
        assert_eq!(one_shot, chunked.finalize_hex());
    }

    #[test]
    fn test_extract_ort_runtime_wheel_and_status() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wheel_path = dir.path().join("fake_ort.whl");
        {
            let f = std::fs::File::create(&wheel_path).unwrap();
            let mut zip = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file(format!("onnxruntime/capi/{ORT_LIB_SONAME}"), opts)
                .unwrap();
            zip.write_all(&vec![0x7fu8; 6_000_000]).unwrap();
            zip.start_file(format!("onnxruntime/capi/{ORT_SHARED_PROVIDER_NAME}"), opts)
                .unwrap();
            zip.write_all(&[1, 2, 3, 4]).unwrap();
            zip.finish().unwrap();
        }

        let rt = runtime_dir(dir.path());
        extract_ort_runtime_wheel(&wheel_path, &rt).expect("extract wheel");
        assert!(rt.join(ORT_LIB_SONAME).is_file());
        assert!(rt.join(ORT_LIB_LINKNAME).exists());
        assert!(rt.join(ORT_SHARED_PROVIDER_NAME).is_file());
    }
}
