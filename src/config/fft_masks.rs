use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

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

/// Header stored at the start of each mask file (16 bytes).
/// Format: [magic(4)] [fft_w(4)] [fft_h(4)] [mask_threshold(4)] [black_threshold(4)]
const MAGIC: &[u8; 4] = b"FFTM";

/// Save the current FFT mask to disk.
pub fn save_mask(
    name: &str,
    resolution: (u32, u32),
    fft_resolution: (u32, u32),
    mask_data: &[u8],
    mask_threshold: f32,
    black_threshold: f32,
) -> Result<(), String> {
    if name.is_empty() {
        return Err("Mask name cannot be empty".to_string());
    }
    // Sanitize name — only allow alphanumeric, underscore, hyphen
    let sanitized: String = name.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();

    let filename = mask_filename(resolution, &sanitized);
    let path = mask_dir().join(&filename);

    let mut file = fs::File::create(&path)
        .map_err(|e| format!("Failed to create mask file: {}", e))?;

    // Write header
    file.write_all(MAGIC).map_err(|e| format!("Write error: {}", e))?;
    file.write_all(&fft_resolution.0.to_le_bytes()).map_err(|e| format!("Write error: {}", e))?;
    file.write_all(&fft_resolution.1.to_le_bytes()).map_err(|e| format!("Write error: {}", e))?;
    file.write_all(&mask_threshold.to_le_bytes()).map_err(|e| format!("Write error: {}", e))?;
    file.write_all(&black_threshold.to_le_bytes()).map_err(|e| format!("Write error: {}", e))?;
    // Write mask data
    file.write_all(mask_data).map_err(|e| format!("Write error: {}", e))?;

    tracing::info!("Saved FFT mask '{}' to {:?}", sanitized, path);
    Ok(())
}

/// Load an FFT mask from disk. Returns (mask_data, fft_resolution, mask_threshold, black_threshold).
pub fn load_mask(
    name: &str,
    resolution: (u32, u32),
) -> Result<(Vec<u8>, (u32, u32), f32, f32), String> {
    let filename = mask_filename(resolution, name);
    let path = mask_dir().join(&filename);

    let mut file = fs::File::open(&path)
        .map_err(|e| format!("Failed to open mask file: {}", e))?;

    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| format!("Failed to read mask file: {}", e))?;

    if buf.len() < 20 {
        return Err("Mask file too small".to_string());
    }

    // Check magic
    if &buf[0..4] != MAGIC {
        return Err("Invalid mask file format".to_string());
    }

    let fft_w = u32::from_le_bytes(buf[4..8].try_into().unwrap());
    let fft_h = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    let mask_threshold = f32::from_le_bytes(buf[12..16].try_into().unwrap());
    let black_threshold = f32::from_le_bytes(buf[16..20].try_into().unwrap());

    let mask_data = buf[20..].to_vec();
    let expected_len = (fft_w * fft_h) as usize;
    if mask_data.len() != expected_len {
        return Err(format!(
            "Mask data size mismatch: expected {} bytes for {}x{}, got {}",
            expected_len, fft_w, fft_h, mask_data.len()
        ));
    }

    tracing::info!("Loaded FFT mask '{}' ({}x{} FFT)", name, fft_w, fft_h);
    Ok((mask_data, (fft_w, fft_h), mask_threshold, black_threshold))
}

/// List all saved mask names for a given resolution.
/// Returns just the name part (without the resolution prefix or .bin extension).
pub fn list_masks_for_resolution(resolution: (u32, u32)) -> Vec<String> {
    let dir = mask_dir();
    let prefix = format!("{}x{}_", resolution.0, resolution.1);

    let mut names = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().to_string();
            if fname.starts_with(&prefix) && fname.ends_with(".bin") {
                // Extract the name part between prefix and .bin
                let name = &fname[prefix.len()..fname.len() - 4];
                if !name.is_empty() {
                    names.push(name.to_string());
                }
            }
        }
    }
    names.sort();
    names
}
