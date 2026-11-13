use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const BLAZEFACE_ONNX_FILENAME: &str = "blaze.onnx";
pub const BLAZEFACE_ONNX_URL: &str =
    "https://huggingface.co/garavv/blazeface-onnx/resolve/main/blaze.onnx";
pub const BLAZEFACE_ONNX_SHA256: &str =
    "564740c5146673c840257402cee8309161848e48e64d277a862ab4d501adf8a5";
pub const BLAZEFACE_EXPECTED_BYTES: u64 = 536_330;

pub const FACE_LANDMARKS_ONNX_FILENAME: &str = "face_landmarks.onnx";
pub const FACE_LANDMARKS_ONNX_URL: &str =
    "https://huggingface.co/naklitechie/face-landmarks-onnx/resolve/main/face_landmarks.onnx";
pub const FACE_LANDMARKS_ONNX_SHA256: &str =
    "f38c3321ceffbc9e95103480ad38cc3f52e7e1bde2bcee7cd9355d0b9138ac0c";
pub const FACE_LANDMARKS_EXPECTED_BYTES: u64 = 4_912_314;

pub const VTUBER_MODEL_BUNDLE_VERSION: &str = "mediapipe-v2-onnx-1";

/// Fast-path readiness flag once the ONNX models have been verified or loaded in this process.
pub static VTUBER_MODELS_READY_PINNED: AtomicBool = AtomicBool::new(false);

static STAGING_SEQ: AtomicU64 = AtomicU64::new(1);
const INSTALL_CACHE_TTL: Duration = Duration::from_secs(2);
static INSTALL_CACHE: OnceLock<Mutex<Option<(PathBuf, Instant, bool)>>> = OnceLock::new();

fn install_cache() -> &'static Mutex<Option<(PathBuf, Instant, bool)>> {
    INSTALL_CACHE.get_or_init(|| Mutex::new(None))
}

pub fn invalidate_vtuber_models_cache() {
    if let Ok(mut guard) = install_cache().lock() {
        *guard = None;
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VtuberTrackerManifest {
    pub version: String,
    pub detector_sha256: String,
    pub landmarks_sha256: String,
}

#[derive(Debug, Clone)]
pub enum VtuberModelSyncEvent {
    Progress { message: String, progress: f32 },
    Finished(Result<VtuberTrackerManifest, String>),
}

/// Resolves `~/.config/michadame/vtuber_tracker` using the same config root as `screen_ai`.
pub fn resolve_vtuber_tracker_directory() -> PathBuf {
    if let Ok(config_path) = confy::get_configuration_file_path("michadame", None) {
        if let Some(parent) = config_path.parent() {
            return parent.join("vtuber_tracker");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".config")
            .join("michadame")
            .join("vtuber_tracker")
    } else {
        PathBuf::from("./vtuber_tracker")
    }
}

pub fn detector_model_path(dir: &Path) -> PathBuf {
    dir.join(BLAZEFACE_ONNX_FILENAME)
}

pub fn landmarks_model_path(dir: &Path) -> PathBuf {
    dir.join(FACE_LANDMARKS_ONNX_FILENAME)
}

pub fn are_vtuber_models_installed(dir: &Path) -> bool {
    if VTUBER_MODELS_READY_PINNED.load(Ordering::Relaxed)
        && dir == resolve_vtuber_tracker_directory()
    {
        return true;
    }

    if let Ok(guard) = install_cache().lock() {
        if let Some((cached_dir, checked_at, installed)) = &*guard {
            if cached_dir == dir && checked_at.elapsed() < INSTALL_CACHE_TTL {
                return *installed;
            }
        }
    }

    let det = detector_model_path(dir);
    let lm = landmarks_model_path(dir);
    let manifest_path = dir.join("manifest.json");

    let installed = det.is_file()
        && lm.is_file()
        && std::fs::metadata(&det).is_ok_and(|m| m.len() > 100_000)
        && std::fs::metadata(&lm).is_ok_and(|m| m.len() > 1_000_000)
        && std::fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|s| serde_json::from_str::<VtuberTrackerManifest>(&s).ok())
            .is_some_and(|m| {
                m.version == VTUBER_MODEL_BUNDLE_VERSION
                    && m.detector_sha256 == BLAZEFACE_ONNX_SHA256
                    && m.landmarks_sha256 == FACE_LANDMARKS_ONNX_SHA256
            });

    if let Ok(mut guard) = install_cache().lock() {
        *guard = Some((dir.to_path_buf(), Instant::now(), installed));
    }
    installed
}

/// Pure-Rust FIPS 180-4 SHA-256 implementation for verifying downloaded ONNX model integrity.
pub fn sha256_hex(bytes: &[u8]) -> String {
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

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((bytes.len() + 72) & !63);
    padded.extend_from_slice(bytes);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 64];
    for chunk in padded.chunks_exact(64) {
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

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = String::with_capacity(64);
    for word in h {
        use std::fmt::Write as _;
        let _ = write!(out, "{word:08x}");
    }
    out
}

fn download_verified_file<F>(
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
        .map_err(|e| format!("Failed to create VTuber tracker directory: {e}"))?;

    progress_cb(format!("Downloading {label}..."), progress_base);

    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("Failed to download {label}: {e}"))?;

    let total_bytes = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(fallback_size);

    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = parent.join(format!(
        ".vtuber_dl_{}_{}_{}.tmp",
        label.replace(' ', "_"),
        std::process::id(),
        seq
    ));

    let mut body_bytes = Vec::with_capacity(total_bytes.min(16 * 1024 * 1024) as usize);
    let read_res = (|| -> Result<(), String> {
        let mut reader = resp.into_reader();
        let mut buf = [0u8; 32768];
        let mut last_report: u64 = 0;

        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("Stream read error while downloading {label}: {e}"))?;
            if n == 0 {
                break;
            }
            body_bytes.extend_from_slice(&buf[..n]);
            let downloaded = body_bytes.len() as u64;
            if downloaded.saturating_sub(last_report) >= 128 * 1024 || downloaded >= total_bytes {
                last_report = downloaded;
                let frac = (downloaded as f32 / total_bytes.max(1) as f32).clamp(0.0, 1.0);
                progress_cb(
                    format!(
                        "Downloading {label}... {:.2} / {:.2} MB",
                        downloaded as f32 / (1024.0 * 1024.0),
                        total_bytes as f32 / (1024.0 * 1024.0)
                    ),
                    progress_base + progress_span * frac,
                );
            }
        }
        Ok(())
    })();

    if let Err(e) = read_res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    let actual_sha = sha256_hex(&body_bytes);
    if actual_sha != expected_sha256 {
        return Err(format!(
            "SHA-256 mismatch for {label}: expected {expected_sha256}, got {actual_sha}"
        ));
    }

    let write_res = (|| -> Result<(), String> {
        let mut file = std::fs::File::create(&tmp_path)
            .map_err(|e| format!("Failed to create temporary file for {label}: {e}"))?;
        file.write_all(&body_bytes)
            .map_err(|e| format!("Failed to write {label} to disk: {e}"))?;
        file.flush()
            .map_err(|e| format!("Failed to flush {label} to disk: {e}"))?;
        std::fs::rename(&tmp_path, dest_path)
            .map_err(|e| format!("Failed to finalize {label} on disk: {e}"))?;
        Ok(())
    })();

    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    Ok(())
}

/// Downloads and atomically stages both BlazeFace and MediaPipe FaceMesh V2 ONNX models.
pub fn download_and_install_vtuber_models<F>(
    target_dir: &Path,
    mut progress_cb: F,
) -> Result<VtuberTrackerManifest, String>
where
    F: FnMut(String, f32),
{
    std::fs::create_dir_all(target_dir)
        .map_err(|e| format!("Failed to create directory {}: {e}", target_dir.display()))?;

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        .build();

    let det_path = detector_model_path(target_dir);
    download_verified_file(
        &agent,
        BLAZEFACE_ONNX_URL,
        BLAZEFACE_ONNX_SHA256,
        BLAZEFACE_EXPECTED_BYTES,
        &det_path,
        "BlazeFace Detector",
        0.02,
        0.18,
        &mut progress_cb,
    )?;

    let lm_path = landmarks_model_path(target_dir);
    download_verified_file(
        &agent,
        FACE_LANDMARKS_ONNX_URL,
        FACE_LANDMARKS_ONNX_SHA256,
        FACE_LANDMARKS_EXPECTED_BYTES,
        &lm_path,
        "MediaPipe Face Landmarker",
        0.20,
        0.75,
        &mut progress_cb,
    )?;

    progress_cb("Writing VTuber tracker manifest...".to_string(), 0.97);
    let manifest = VtuberTrackerManifest {
        version: VTUBER_MODEL_BUNDLE_VERSION.to_string(),
        detector_sha256: BLAZEFACE_ONNX_SHA256.to_string(),
        landmarks_sha256: FACE_LANDMARKS_ONNX_SHA256.to_string(),
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("Failed to serialize tracker manifest: {e}"))?;
    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_manifest = target_dir.join(format!(
        ".vtuber_manifest_{}_{}.json.tmp",
        std::process::id(),
        seq
    ));
    std::fs::write(&tmp_manifest, manifest_json.as_bytes())
        .map_err(|e| format!("Failed to write tracker manifest: {e}"))?;
    std::fs::rename(&tmp_manifest, target_dir.join("manifest.json"))
        .map_err(|e| format!("Failed to finalize tracker manifest: {e}"))?;

    invalidate_vtuber_models_cache();
    if target_dir == resolve_vtuber_tracker_directory() {
        VTUBER_MODELS_READY_PINNED.store(true, Ordering::Relaxed);
    }
    progress_cb("VTuber face tracker ready.".to_string(), 1.0);

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_nist_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn test_are_vtuber_models_installed_checks_manifest_and_sizes() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!are_vtuber_models_installed(dir.path()));

        std::fs::write(detector_model_path(dir.path()), vec![0u8; 150_000]).unwrap();
        std::fs::write(landmarks_model_path(dir.path()), vec![0u8; 1_500_000]).unwrap();
        let manifest = VtuberTrackerManifest {
            version: VTUBER_MODEL_BUNDLE_VERSION.to_string(),
            detector_sha256: BLAZEFACE_ONNX_SHA256.to_string(),
            landmarks_sha256: FACE_LANDMARKS_ONNX_SHA256.to_string(),
        };
        std::fs::write(
            dir.path().join("manifest.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
        invalidate_vtuber_models_cache();

        assert!(are_vtuber_models_installed(dir.path()));
    }
}
