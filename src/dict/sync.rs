use super::database::DictDatabase;
use super::models::{DictEvent, DictMetadata};
use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_INDEX_URL: &str = "https://jitendex.org/static/yomitan.json";
pub const DEFAULT_DOWNLOAD_URL: &str =
    "https://github.com/stephenmk/stephenmk.github.io/releases/latest/download/jitendex-yomitan.zip";

/// Checks the remote Jitendex server for the latest dictionary revision.
pub fn check_remote_version(current_revision: Option<&str>) -> Result<(bool, DictMetadata)> {
    let response = ureq::get(DEFAULT_INDEX_URL)
        .timeout(Duration::from_secs(10))
        .call()
        .context("Failed to query Jitendex version index")?;

    let meta: DictMetadata = serde_json::from_reader(response.into_reader())
        .context("Failed to parse Jitendex version metadata")?;

    let has_update = match current_revision {
        Some(current) => current != meta.revision && !meta.revision.is_empty(),
        None => true,
    };

    Ok((has_update, meta))
}

/// Spawns a background thread to check for updates and sends the result over a channel.
pub fn spawn_check_version(
    current_rev: Option<String>,
    is_checking: Arc<AtomicBool>,
    event_tx: Sender<DictEvent>,
) {
    if is_checking.swap(true, Ordering::AcqRel) {
        return; // Already checking
    }

    std::thread::Builder::new()
        .name("jitendex-version-check".into())
        .spawn(move || {
            let res = check_remote_version(current_rev.as_deref());
            is_checking.store(false, Ordering::Release);

            match res {
                Ok((has_update, meta)) => {
                    let _ = event_tx.send(DictEvent::UpdateCheckFinished {
                        update_available: has_update,
                        remote_metadata: Some(meta),
                    });
                }
                Err(e) => {
                    tracing::warn!("Jitendex version check failed: {e}");
                    let _ = event_tx.send(DictEvent::UpdateCheckFinished {
                        update_available: false,
                        remote_metadata: None,
                    });
                }
            }
        })
        .expect("Failed to spawn jitendex version check thread");
}

/// Downloads and indexes Jitendex into SQLite in a background thread.
pub fn spawn_sync(
    custom_download_url: Option<String>,
    dict_dir: PathBuf,
    is_syncing: Arc<AtomicBool>,
    event_tx: Sender<DictEvent>,
) {
    if is_syncing.swap(true, Ordering::AcqRel) {
        return; // Already syncing
    }

    let download_url = custom_download_url.unwrap_or_else(|| DEFAULT_DOWNLOAD_URL.to_string());

    std::thread::Builder::new()
        .name("jitendex-sync-worker".into())
        .spawn(move || {
            let result = run_sync_pipeline(&download_url, &dict_dir, &event_tx);
            is_syncing.store(false, Ordering::Release);
            let _ = event_tx.send(DictEvent::SyncFinished(result));
        })
        .expect("Failed to spawn jitendex sync thread");
}

fn run_sync_pipeline(
    download_url: &str,
    dict_dir: &Path,
    event_tx: &Sender<DictEvent>,
) -> Result<DictMetadata, String> {
    if let Err(e) = std::fs::create_dir_all(dict_dir) {
        return Err(format!("Failed to create dictionary directory: {e}"));
    }

    let temp_zip_path = dict_dir.join("jitendex_temp.zip");
    let target_db_path = dict_dir.join("jitendex.db");

    let _ = event_tx.send(DictEvent::SyncProgress {
        message: "Connecting to Jitendex release...".into(),
        progress: 0.02,
    });

    // 1. Download archive with progress
    let resp = ureq::get(download_url)
        .timeout(Duration::from_secs(300))
        .call()
        .map_err(|e| format!("Failed to download Jitendex archive: {e}"))?;

    let total_bytes = resp
        .header("Content-Length")
        .and_then(|h| h.parse::<u64>().ok())
        .unwrap_or(38 * 1024 * 1024);

    let mut reader = resp.into_reader();
    let mut temp_file = File::create(&temp_zip_path)
        .map_err(|e| format!("Failed to create temp zip file: {e}"))?;

    let mut downloaded: u64 = 0;
    let mut buffer = [0u8; 64 * 1024];
    let mut last_progress_report = 0.0;

    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|e| format!("Network read error: {e}"))?;
        if n == 0 {
            break;
        }
        temp_file
            .write_all(&buffer[..n])
            .map_err(|e| format!("Disk write error: {e}"))?;
        downloaded += n as u64;

        let fraction = downloaded as f32 / total_bytes as f32;
        let progress = 0.02 + 0.38 * fraction.min(1.0); // 2% to 40%
        if progress - last_progress_report >= 0.02 {
            last_progress_report = progress;
            let mb_done = downloaded as f64 / (1024.0 * 1024.0);
            let mb_total = total_bytes as f64 / (1024.0 * 1024.0);
            let _ = event_tx.send(DictEvent::SyncProgress {
                message: format!("Downloading Jitendex: {:.1}/{:.1} MB", mb_done, mb_total),
                progress,
            });
        }
    }
    drop(temp_file);

    let _ = event_tx.send(DictEvent::SyncProgress {
        message: "Download complete. Extracting and indexing term banks...".into(),
        progress: 0.40,
    });

    // 2. Build SQLite database
    let tx_clone = event_tx.clone();
    let meta_result = DictDatabase::build_database_from_zip(
        &temp_zip_path,
        &target_db_path,
        move |msg, fraction| {
            let progress = 0.40 + 0.58 * fraction; // 40% to 98%
            let _ = tx_clone.send(DictEvent::SyncProgress {
                message: msg.to_string(),
                progress,
            });
        },
    );

    // Clean up temporary download file
    let _ = std::fs::remove_file(&temp_zip_path);

    meta_result.map_err(|e| format!("Failed to index dictionary: {e}"))
}
