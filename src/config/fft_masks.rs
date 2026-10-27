use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub type LoadedMask = (Vec<u8>, (u32, u32), f32, f32);

/// Header stored at the start of each mask file (20 bytes).
/// Format: [magic(4)] [fft_w(4)] [fft_h(4)] [mask_threshold(4)] [black_threshold(4)]
const MAGIC: &[u8; 4] = b"FFTM";
pub const HEADER_SIZE: usize = 20;
pub const MAX_FFT_DIMENSION: u32 = 16384;
pub const MAX_FFT_MASK_BYTES: usize = 64 * 1024 * 1024;

/// Get the directory where FFT masks are stored.
fn mask_dir() -> PathBuf {
    // Derive from confy's config path: ~/.config/michadame/default-config.toml → ~/.config/michadame/fft_masks/
    let dir = confy::get_configuration_file_path("michadame", None)
        .unwrap_or_else(|_| PathBuf::from("michadame.toml"))
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("fft_masks");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// Build the filename for a mask: "{W}x{H}_{name}.bin"
fn mask_filename(resolution: (u32, u32), name: &str) -> String {
    format!("{}x{}_{}.bin", resolution.0, resolution.1, name)
}

/// Sanitize an FFT mask name into its canonical identifier.
/// Keeps alphanumeric characters (including Unicode/Japanese), `_`, and `-`,
/// replacing other characters with `_`. Rejects empty or non-alphanumeric names.
pub fn sanitize_mask_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Mask name cannot be empty".to_string());
    }
    let sanitized: String = trimmed
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches('.').to_string();
    if sanitized.is_empty() || !sanitized.chars().any(|c| c.is_alphanumeric()) {
        return Err("Mask name must contain at least one alphanumeric character".to_string());
    }
    Ok(sanitized)
}

/// Validate FFT mask header parameters and return the expected payload byte length.
fn validate_mask_header(
    resolution: (u32, u32),
    fft_resolution: (u32, u32),
    mask_threshold: f32,
    black_threshold: f32,
) -> Result<usize, String> {
    if resolution.0 == 0 || resolution.1 == 0 {
        return Err("Invalid stream resolution: dimensions must be non-zero".to_string());
    }
    let (fft_w, fft_h) = fft_resolution;
    if fft_w == 0 || fft_h == 0 {
        return Err(format!(
            "Invalid FFT mask dimensions {}x{}: dimensions must be non-zero",
            fft_w, fft_h
        ));
    }
    if fft_w > MAX_FFT_DIMENSION || fft_h > MAX_FFT_DIMENSION {
        return Err(format!(
            "FFT mask dimensions {}x{} exceed maximum supported dimension {}",
            fft_w, fft_h, MAX_FFT_DIMENSION
        ));
    }
    let expected_len = usize::try_from(fft_w)
        .ok()
        .and_then(|w| usize::try_from(fft_h).ok().and_then(|h| w.checked_mul(h)))
        .ok_or_else(|| {
            format!(
                "FFT mask dimensions {}x{} overflow size calculation",
                fft_w, fft_h
            )
        })?;
    if expected_len > MAX_FFT_MASK_BYTES {
        return Err(format!(
            "FFT mask payload size {} bytes for {}x{} exceeds limit of {} bytes",
            expected_len, fft_w, fft_h, MAX_FFT_MASK_BYTES
        ));
    }
    if !mask_threshold.is_finite() {
        return Err("Invalid mask_threshold: must be finite".to_string());
    }
    if !black_threshold.is_finite() {
        return Err("Invalid black_threshold: must be finite".to_string());
    }
    Ok(expected_len)
}

/// Save the current FFT mask to disk in the default mask directory.
/// Returns the canonical sanitized mask name.
pub fn save_mask(
    name: &str,
    resolution: (u32, u32),
    fft_resolution: (u32, u32),
    mask_data: &[u8],
    mask_threshold: f32,
    black_threshold: f32,
) -> Result<String, String> {
    save_mask_in(
        &mask_dir(),
        name,
        resolution,
        fft_resolution,
        mask_data,
        mask_threshold,
        black_threshold,
    )
}

/// Save the current FFT mask to `dir` atomically via a temporary file.
/// Returns the canonical sanitized mask name.
pub fn save_mask_in(
    dir: &Path,
    name: &str,
    resolution: (u32, u32),
    fft_resolution: (u32, u32),
    mask_data: &[u8],
    mask_threshold: f32,
    black_threshold: f32,
) -> Result<String, String> {
    let sanitized = sanitize_mask_name(name)?;
    let expected_len =
        validate_mask_header(resolution, fft_resolution, mask_threshold, black_threshold)?;
    if mask_data.len() != expected_len {
        return Err(format!(
            "Mask data size mismatch: expected {} bytes for {}x{}, got {}",
            expected_len,
            fft_resolution.0,
            fft_resolution.1,
            mask_data.len()
        ));
    }

    fs::create_dir_all(dir).map_err(|e| format!("Failed to create mask directory: {}", e))?;

    let filename = mask_filename(resolution, &sanitized);
    let path = dir.join(&filename);
    let tmp_filename = format!(".{}.tmp-{}", filename, std::process::id());
    let tmp_path = dir.join(&tmp_filename);

    let write_result = (|| -> Result<(), String> {
        let mut file = fs::File::create(&tmp_path)
            .map_err(|e| format!("Failed to create mask file: {}", e))?;

        // Write header
        file.write_all(MAGIC)
            .map_err(|e| format!("Write error: {}", e))?;
        file.write_all(&fft_resolution.0.to_le_bytes())
            .map_err(|e| format!("Write error: {}", e))?;
        file.write_all(&fft_resolution.1.to_le_bytes())
            .map_err(|e| format!("Write error: {}", e))?;
        file.write_all(&mask_threshold.to_le_bytes())
            .map_err(|e| format!("Write error: {}", e))?;
        file.write_all(&black_threshold.to_le_bytes())
            .map_err(|e| format!("Write error: {}", e))?;
        // Write mask data
        file.write_all(mask_data)
            .map_err(|e| format!("Write error: {}", e))?;
        file.sync_all().map_err(|e| format!("Sync error: {}", e))?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    if let Err(e) = fs::rename(&tmp_path, &path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("Failed to finalize mask file: {}", e));
    }

    tracing::info!("Saved FFT mask '{}' to {:?}", sanitized, path);
    Ok(sanitized)
}

/// Load an FFT mask from disk in the default mask directory.
/// Returns (mask_data, fft_resolution, mask_threshold, black_threshold).
pub fn load_mask(name: &str, resolution: (u32, u32)) -> Result<LoadedMask, String> {
    load_mask_in(&mask_dir(), name, resolution)
}

/// Load an FFT mask from `dir`.
/// Returns (mask_data, fft_resolution, mask_threshold, black_threshold).
pub fn load_mask_in(dir: &Path, name: &str, resolution: (u32, u32)) -> Result<LoadedMask, String> {
    let sanitized = sanitize_mask_name(name)?;
    if resolution.0 == 0 || resolution.1 == 0 {
        return Err("Invalid stream resolution: dimensions must be non-zero".to_string());
    }

    let filename = mask_filename(resolution, &sanitized);
    let path = dir.join(&filename);

    let mut file = fs::File::open(&path).map_err(|e| format!("Failed to open mask file: {}", e))?;
    let file_len = file
        .metadata()
        .map_err(|e| format!("Failed to read mask file metadata: {}", e))?
        .len();

    if file_len < HEADER_SIZE as u64 {
        return Err("Mask file too small".to_string());
    }
    if file_len > (HEADER_SIZE + MAX_FFT_MASK_BYTES) as u64 {
        return Err(format!(
            "Mask file too large ({} bytes, max {})",
            file_len,
            HEADER_SIZE + MAX_FFT_MASK_BYTES
        ));
    }

    let mut header = [0u8; HEADER_SIZE];
    file.read_exact(&mut header)
        .map_err(|e| format!("Failed to read mask file header: {}", e))?;

    // Check magic
    if &header[0..4] != MAGIC {
        return Err("Invalid mask file format".to_string());
    }

    let fft_w = u32::from_le_bytes(header[4..8].try_into().unwrap());
    let fft_h = u32::from_le_bytes(header[8..12].try_into().unwrap());
    let mask_threshold = f32::from_le_bytes(header[12..16].try_into().unwrap());
    let black_threshold = f32::from_le_bytes(header[16..20].try_into().unwrap());

    let expected_len =
        validate_mask_header(resolution, (fft_w, fft_h), mask_threshold, black_threshold)?;

    let actual_payload_len = file_len - HEADER_SIZE as u64;
    if actual_payload_len != expected_len as u64 {
        return Err(format!(
            "Mask data size mismatch: expected {} bytes for {}x{}, got {}",
            expected_len, fft_w, fft_h, actual_payload_len
        ));
    }

    let mut mask_data = vec![0u8; expected_len];
    file.read_exact(&mut mask_data)
        .map_err(|e| format!("Failed to read mask file: {}", e))?;

    tracing::info!("Loaded FFT mask '{}' ({}x{} FFT)", sanitized, fft_w, fft_h);
    Ok((mask_data, (fft_w, fft_h), mask_threshold, black_threshold))
}

/// List all saved mask names for a given resolution in the default mask directory.
/// Returns just the name part (without the resolution prefix or .bin extension).
pub fn list_masks_for_resolution(resolution: (u32, u32)) -> Vec<String> {
    list_masks_for_resolution_in(&mask_dir(), resolution)
}

/// List all saved mask names for a given resolution in `dir`.
pub fn list_masks_for_resolution_in(dir: &Path, resolution: (u32, u32)) -> Vec<String> {
    if resolution.0 == 0 || resolution.1 == 0 {
        return Vec::new();
    }
    let prefix = format!("{}x{}_", resolution.0, resolution.1);

    let mut names = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().to_string();
            if fname.starts_with(&prefix) && fname.ends_with(".bin") {
                // Extract the name part between prefix and .bin
                let raw_name = &fname[prefix.len()..fname.len() - 4];
                if let Ok(sanitized) = sanitize_mask_name(raw_name) {
                    if sanitized == raw_name && !names.contains(&sanitized) {
                        names.push(sanitized);
                    }
                }
            }
        }
    }
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_raw_mask_file(
        dir: &Path,
        resolution: (u32, u32),
        name: &str,
        magic: &[u8; 4],
        fft_w: u32,
        fft_h: u32,
        mask_threshold: f32,
        black_threshold: f32,
        payload: &[u8],
    ) {
        let filename = mask_filename(resolution, name);
        let path = dir.join(filename);
        let mut bytes = Vec::with_capacity(HEADER_SIZE + payload.len());
        bytes.extend_from_slice(magic);
        bytes.extend_from_slice(&fft_w.to_le_bytes());
        bytes.extend_from_slice(&fft_h.to_le_bytes());
        bytes.extend_from_slice(&mask_threshold.to_le_bytes());
        bytes.extend_from_slice(&black_threshold.to_le_bytes());
        bytes.extend_from_slice(payload);
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn test_mask_filename() {
        assert_eq!(mask_filename((720, 480), "test"), "720x480_test.bin");
    }

    #[test]
    fn test_save_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let mask_data = vec![255u8; 16];
        let name = "roundtrip";
        let res = (100, 100);
        let fft_res = (4, 4);

        let saved_name = save_mask_in(dir, name, res, fft_res, &mask_data, 0.5, 0.2).unwrap();
        assert_eq!(saved_name, "roundtrip");

        let (loaded_data, loaded_res, mask_thresh, black_thresh) =
            load_mask_in(dir, name, res).unwrap();
        assert_eq!(loaded_data, mask_data);
        assert_eq!(loaded_res, fft_res);
        assert!((mask_thresh - 0.5).abs() < f32::EPSILON);
        assert!((black_thresh - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn test_name_sanitization_logic() {
        assert_eq!(
            sanitize_mask_name("my mask/test!").unwrap(),
            "my_mask_test_"
        );
        assert!(sanitize_mask_name("").is_err());
        assert!(sanitize_mask_name("   ").is_err());
        assert!(sanitize_mask_name("...").is_err());
        assert!(sanitize_mask_name("!@#").is_err());
    }

    #[test]
    fn test_overflow_dimensions_rejected_without_panic_or_allocation() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let res = (640, 480);

        // 65536 * 65536 overflows u32 to 0; must be rejected cleanly without panic
        write_raw_mask_file(dir, res, "overflow", MAGIC, 65536, 65536, 0.5, 0.2, &[]);
        let err = load_mask_in(dir, "overflow", res).unwrap_err();
        assert!(
            err.contains("exceed maximum") || err.contains("overflow"),
            "Unexpected error: {err}"
        );
    }

    #[test]
    fn test_zero_and_oversized_dimensions_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let res = (640, 480);

        // Save validation
        assert!(save_mask_in(dir, "zero_w", res, (0, 100), &[], 0.5, 0.2).is_err());
        assert!(save_mask_in(dir, "zero_h", res, (100, 0), &[], 0.5, 0.2).is_err());
        assert!(save_mask_in(dir, "zero_res", (0, 480), (4, 4), &[0u8; 16], 0.5, 0.2).is_err());
        assert!(save_mask_in(dir, "oversized", res, (20000, 20000), &[], 0.5, 0.2).is_err());

        // Load validation
        write_raw_mask_file(dir, res, "zero_w", MAGIC, 0, 100, 0.5, 0.2, &[]);
        assert!(load_mask_in(dir, "zero_w", res).is_err());

        write_raw_mask_file(dir, res, "zero_h", MAGIC, 100, 0, 0.5, 0.2, &[]);
        assert!(load_mask_in(dir, "zero_h", res).is_err());

        write_raw_mask_file(dir, res, "oversized", MAGIC, 20000, 20000, 0.5, 0.2, &[]);
        assert!(load_mask_in(dir, "oversized", res).is_err());
    }

    #[test]
    fn test_truncated_and_extra_payload_and_invalid_magic_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let res = (640, 480);

        // Truncated payload (expected 16, got 10)
        write_raw_mask_file(dir, res, "truncated", MAGIC, 4, 4, 0.5, 0.2, &[1u8; 10]);
        let err = load_mask_in(dir, "truncated", res).unwrap_err();
        assert!(err.contains("mismatch"), "Unexpected error: {err}");

        // Extra trailing payload (expected 16, got 20)
        write_raw_mask_file(dir, res, "extra", MAGIC, 4, 4, 0.5, 0.2, &[1u8; 20]);
        let err = load_mask_in(dir, "extra", res).unwrap_err();
        assert!(err.contains("mismatch"), "Unexpected error: {err}");

        // Invalid magic
        write_raw_mask_file(dir, res, "badmagic", b"NOPE", 4, 4, 0.5, 0.2, &[1u8; 16]);
        let err = load_mask_in(dir, "badmagic", res).unwrap_err();
        assert!(
            err.contains("Invalid mask file format"),
            "Unexpected error: {err}"
        );

        // Too small (< 20 bytes)
        fs::write(dir.join(mask_filename(res, "tiny")), &[0u8; 12]).unwrap();
        let err = load_mask_in(dir, "tiny", res).unwrap_err();
        assert!(err.contains("too small"), "Unexpected error: {err}");
    }

    #[test]
    fn test_nonfinite_thresholds_rejected_on_save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let res = (640, 480);
        let payload = [128u8; 16];

        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(save_mask_in(dir, "nan_m", res, (4, 4), &payload, bad, 0.2).is_err());
            assert!(save_mask_in(dir, "nan_b", res, (4, 4), &payload, 0.5, bad).is_err());

            write_raw_mask_file(dir, res, "load_bad_m", MAGIC, 4, 4, bad, 0.2, &payload);
            assert!(load_mask_in(dir, "load_bad_m", res).is_err());

            write_raw_mask_file(dir, res, "load_bad_b", MAGIC, 4, 4, 0.5, bad, &payload);
            assert!(load_mask_in(dir, "load_bad_b", res).is_err());
        }
    }

    #[test]
    fn test_canonical_sanitized_names_roundtrip_and_list() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let res = (720, 480);
        let fft_res = (4, 4);
        let data_a = vec![10u8; 16];
        let data_b = vec![20u8; 16];
        let data_c = vec![30u8; 16];

        let name1 = save_mask_in(dir, "my mask", res, fft_res, &data_a, 0.3, 0.1).unwrap();
        assert_eq!(name1, "my_mask");

        let name2 = save_mask_in(dir, "mask!@#", res, fft_res, &data_b, 0.4, 0.2).unwrap();
        assert_eq!(name2, "mask___");

        let name3 = save_mask_in(dir, "マスク１", res, fft_res, &data_c, 0.6, 0.3).unwrap();
        assert_eq!(name3, "マスク１");

        let listed = list_masks_for_resolution_in(dir, res);
        assert_eq!(listed, vec!["mask___", "my_mask", "マスク１"]);

        // Both raw and canonical names load the same saved mask
        assert_eq!(load_mask_in(dir, "my mask", res).unwrap().0, data_a);
        assert_eq!(load_mask_in(dir, "my_mask", res).unwrap().0, data_a);
        assert_eq!(load_mask_in(dir, "mask!@#", res).unwrap().0, data_b);
        assert_eq!(load_mask_in(dir, "mask___", res).unwrap().0, data_b);
        assert_eq!(load_mask_in(dir, "マスク１", res).unwrap().0, data_c);
    }
}
