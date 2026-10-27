use crate::video::types::{ColorRange, RawFrame};
use anyhow::{Context, Result};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const GAMESCOPE_WIDTH: u32 = 3840;
pub const GAMESCOPE_HEIGHT: u32 = 2160;
pub const GAMESCOPE_FPS: u32 = 60;
pub const GAMESCOPE_FRAME_SIZE: usize =
    (GAMESCOPE_WIDTH as usize) * (GAMESCOPE_HEIGHT as usize) * 4;
pub const PW_DUMP_TIMEOUT: Duration = Duration::from_millis(800);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GamescopeNodeInfo {
    pub node_id: u32,
    pub width: u32,
    pub height: u32,
}

fn extract_format_dimensions(fmt: &serde_json::Value) -> Option<(u32, u32)> {
    let (w, h) = fmt["size"]["width"]
        .as_u64()
        .zip(fmt["size"]["height"].as_u64())
        .or_else(|| {
            fmt["size"]["default"]["width"]
                .as_u64()
                .zip(fmt["size"]["default"]["height"].as_u64())
        })?;
    if w > 0 && h > 0 && w <= 16384 && h <= 16384 {
        Some((w as u32, h as u32))
    } else {
        None
    }
}

/// Parses `pw-dump` JSON output to locate the active Gamescope video node and its negotiated dimensions.
/// Returns `None` when the Gamescope node or its format is not yet present.
pub fn parse_gamescope_node_info(json_bytes: &[u8]) -> Option<GamescopeNodeInfo> {
    let val = serde_json::from_slice::<serde_json::Value>(json_bytes).ok()?;
    let nodes = val.as_array()?;

    let mut fallback_enum_match: Option<GamescopeNodeInfo> = None;
    let mut active_format_match: Option<GamescopeNodeInfo> = None;

    for node in nodes {
        let props = &node["info"]["props"];
        let is_gamescope = props["node.name"].as_str() == Some("gamescope")
            || (props["application.name"].as_str() == Some("gamescope")
                && props["media.class"]
                    .as_str()
                    .map_or(true, |c| c.contains("Video")));
        if !is_gamescope {
            continue;
        }

        let node_id = node["id"].as_u64().unwrap_or(0) as u32;
        let params = &node["info"]["params"];

        // Prefer actively negotiated `Format` over `EnumFormat`
        if let Some(formats) = params["Format"].as_array() {
            for fmt in formats {
                if let Some((width, height)) = extract_format_dimensions(fmt) {
                    active_format_match = Some(GamescopeNodeInfo {
                        node_id,
                        width,
                        height,
                    });
                }
            }
        }

        if let Some(formats) = params["EnumFormat"].as_array() {
            for fmt in formats {
                if let Some((width, height)) = extract_format_dimensions(fmt) {
                    fallback_enum_match = Some(GamescopeNodeInfo {
                        node_id,
                        width,
                        height,
                    });
                    break;
                }
            }
        }
    }

    active_format_match.or(fallback_enum_match)
}

/// Runs `pw-dump` with a bounded timeout and cancellation check so a hung PipeWire daemon
/// cannot stall the worker thread or block shutdown.
pub fn run_pw_dump_with_timeout(
    stop_signal: Option<&AtomicBool>,
    timeout: Duration,
) -> Option<Vec<u8>> {
    if stop_signal.map_or(false, |s| s.load(Ordering::Relaxed)) {
        return None;
    }

    let mut child = Command::new("pw-dump")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut stdout = child.stdout.take()?;
    let (tx, rx) = crossbeam_channel::bounded(1);
    thread::spawn(move || {
        let mut buf = Vec::new();
        let res = stdout.read_to_end(&mut buf).map(|_| buf);
        let _ = tx.send(res);
    });

    let deadline = Instant::now() + timeout;
    loop {
        if stop_signal.map_or(false, |s| s.load(Ordering::Relaxed)) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }

        let now = Instant::now();
        if now >= deadline {
            tracing::debug!("pw-dump timed out after {:?}", timeout);
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }

        let wait_slice = (deadline - now).min(Duration::from_millis(20));
        match rx.recv_timeout(wait_slice) {
            Ok(Ok(bytes)) => {
                let _ = child.wait();
                return Some(bytes);
            }
            Ok(Err(_)) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                let _ = child.wait();
                return None;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }
    }
}

pub fn get_gamescope_node_info_with_timeout(
    stop_signal: Option<&AtomicBool>,
    timeout: Duration,
) -> Option<GamescopeNodeInfo> {
    let stdout = run_pw_dump_with_timeout(stop_signal, timeout)?;
    let info = parse_gamescope_node_info(&stdout)?;
    tracing::info!(
        "Discovered Gamescope PipeWire node {} ({}x{})",
        info.node_id,
        info.width,
        info.height
    );
    Some(info)
}

pub fn get_gamescope_node_info() -> Option<GamescopeNodeInfo> {
    get_gamescope_node_info_with_timeout(None, PW_DUMP_TIMEOUT)
}

pub fn detect_gamescope_resolution() -> (u32, u32) {
    get_gamescope_node_info_with_timeout(None, Duration::from_millis(250))
        .map(|info| (info.width, info.height))
        .unwrap_or((GAMESCOPE_WIDTH, GAMESCOPE_HEIGHT))
}

pub fn rgba_frame_byte_size(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)
        .filter(|&sz| sz > 0)
}

pub fn build_gst_pipeline_args(info: &GamescopeNodeInfo) -> Vec<String> {
    vec![
        "-q".to_string(),
        "pipewiresrc".to_string(),
        "target-object=gamescope".to_string(),
        "keepalive-time=1000".to_string(),
        "!".to_string(),
        "video/x-raw".to_string(),
        "!".to_string(),
        "queue".to_string(),
        "max-size-buffers=3".to_string(),
        "!".to_string(),
        "videoconvert".to_string(),
        "n-threads=4".to_string(),
        "!".to_string(),
        "videoscale".to_string(),
        "!".to_string(),
        format!(
            "video/x-raw,format=RGBA,width={},height={}",
            info.width, info.height
        ),
        "!".to_string(),
        "fdsink".to_string(),
        "fd=1".to_string(),
    ]
}

fn sleep_cancelable(stop_signal: &AtomicBool, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if stop_signal.load(Ordering::Relaxed) {
            return false;
        }
        let rem = deadline.saturating_duration_since(Instant::now());
        thread::sleep(rem.min(Duration::from_millis(25)));
    }
    !stop_signal.load(Ordering::Relaxed)
}

pub struct GamescopeVideoHandle {
    pub stop_requested: Arc<AtomicBool>,
    pub width: u32,
    pub height: u32,
    current_width: Arc<AtomicU32>,
    current_height: Arc<AtomicU32>,
    child_process: Arc<Mutex<Option<Child>>>,
    thread_handle: Option<JoinHandle<()>>,
}

impl Drop for GamescopeVideoHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

impl GamescopeVideoHandle {
    pub fn current_resolution(&self) -> (u32, u32) {
        (
            self.current_width.load(Ordering::Acquire),
            self.current_height.load(Ordering::Acquire),
        )
    }

    pub fn stop(&mut self) {
        self.stop_requested.store(true, Ordering::Relaxed);
        if let Ok(mut lock) = self.child_process.lock() {
            if let Some(mut child) = lock.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

pub fn start_gamescope_video(
    frame_sender: crossbeam_channel::Sender<Arc<RawFrame>>,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    color_range: Arc<std::sync::atomic::AtomicU8>,
) -> Result<GamescopeVideoHandle> {
    let stop_requested = Arc::new(AtomicBool::new(false));
    let child_process = Arc::new(Mutex::new(None));
    let current_width = Arc::new(AtomicU32::new(GAMESCOPE_WIDTH));
    let current_height = Arc::new(AtomicU32::new(GAMESCOPE_HEIGHT));

    let stop_clone = stop_requested.clone();
    let child_process_clone = child_process.clone();
    let width_clone = current_width.clone();
    let height_clone = current_height.clone();

    let thread_handle = thread::Builder::new()
        .name("gamescope-video-pipe".to_string())
        .spawn(move || {
            let mut backoff = Duration::from_millis(200);

            while !stop_clone.load(Ordering::Relaxed) {
                let Some(node_info) =
                    get_gamescope_node_info_with_timeout(Some(&stop_clone), PW_DUMP_TIMEOUT)
                else {
                    if !sleep_cancelable(&stop_clone, backoff) {
                        break;
                    }
                    backoff = (backoff * 2).min(Duration::from_secs(1));
                    continue;
                };

                let width = node_info.width;
                let height = node_info.height;
                let Some(frame_size) = rgba_frame_byte_size(width, height) else {
                    if !sleep_cancelable(&stop_clone, backoff) {
                        break;
                    }
                    continue;
                };

                width_clone.store(width, Ordering::Release);
                height_clone.store(height, Ordering::Release);

                tracing::info!(
                    "Launching GStreamer pipewiresrc for gamescope node {} ({width}x{height} RGBA @ {GAMESCOPE_FPS}fps)...",
                    node_info.node_id
                );

                let args = build_gst_pipeline_args(&node_info);
                let mut cmd = Command::new("gst-launch-1.0");
                cmd.args(&args);
                cmd.stdout(Stdio::piped());
                cmd.stderr(Stdio::inherit());

                let mut child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("Failed to spawn gst-launch-1.0 for gamescope: {e}");
                        if !sleep_cancelable(&stop_clone, backoff) {
                            break;
                        }
                        backoff = (backoff * 2).min(Duration::from_secs(2));
                        continue;
                    }
                };

                let mut stdout = match child.stdout.take() {
                    Some(s) => s,
                    None => {
                        let _ = child.kill();
                        let _ = child.wait();
                        continue;
                    }
                };

                if let Ok(mut lock) = child_process_clone.lock() {
                    *lock = Some(child);
                }

                backoff = Duration::from_millis(200);
                let mut frame_buffer = vec![0u8; frame_size];
                let mut last_res_check = Instant::now();
                let mut resolution_changed = false;

                while !stop_clone.load(Ordering::Relaxed) {
                    match stdout.read_exact(&mut frame_buffer) {
                        Ok(()) => {
                            let raw_frame = Arc::new(RawFrame {
                                captured_at: crate::replay::now_us(),
                                rate: crate::replay::config::Rate {
                                    num: GAMESCOPE_FPS,
                                    den: 1,
                                },
                                width,
                                height,
                                data: frame_buffer.clone(),
                                format: ffmpeg_next::format::Pixel::RGBA,
                                color_range: ColorRange::from_u8(
                                    color_range.load(Ordering::Relaxed),
                                ),
                            });

                            if frame_sender.send(raw_frame).is_err() {
                                break;
                            }
                            request_repaint();

                            if last_res_check.elapsed() >= Duration::from_secs(2) {
                                last_res_check = Instant::now();
                                if let Some(new_info) = get_gamescope_node_info_with_timeout(
                                    Some(&stop_clone),
                                    Duration::from_millis(250),
                                ) {
                                    if (new_info.width, new_info.height) != (width, height) {
                                        tracing::info!(
                                            "Gamescope resolution changed from {}x{} to {}x{}; reconnecting pipeline",
                                            width,
                                            height,
                                            new_info.width,
                                            new_info.height
                                        );
                                        resolution_changed = true;
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::debug!("Gamescope video stream ended or read failed: {e}");
                            break;
                        }
                    }
                }

                if let Ok(mut lock) = child_process_clone.lock() {
                    if let Some(mut c) = lock.take() {
                        let _ = c.kill();
                        let _ = c.wait();
                    }
                }

                if stop_clone.load(Ordering::Relaxed) {
                    break;
                }
                if !resolution_changed && !sleep_cancelable(&stop_clone, Duration::from_millis(300))
                {
                    break;
                }
            }
        })
        .context("Failed to spawn gamescope video worker thread")?;

    Ok(GamescopeVideoHandle {
        stop_requested,
        width: GAMESCOPE_WIDTH,
        height: GAMESCOPE_HEIGHT,
        current_width,
        current_height,
        child_process,
        thread_handle: Some(thread_handle),
    })
}

pub fn start_video_capture(
    frame_sender: crossbeam_channel::Sender<Arc<RawFrame>>,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    color_range: Arc<std::sync::atomic::AtomicU8>,
) -> Result<GamescopeVideoHandle> {
    start_gamescope_video(frame_sender, request_repaint, color_range)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gamescope_node_info_missing_node_returns_none() {
        // Empty array or non-gamescope nodes must return None instead of a fake 3840x2160 guess
        assert_eq!(parse_gamescope_node_info(b"[]"), None);

        let unrelated_json = br#"[
            {
                "id": 42,
                "info": {
                    "props": {
                        "node.name": "alsa_output.pci-0000_00_1f.3"
                    },
                    "params": {
                        "EnumFormat": [
                            { "size": { "width": 1920, "height": 1080 } }
                        ]
                    }
                }
            }
        ]"#;
        assert_eq!(parse_gamescope_node_info(unrelated_json), None);

        // Gamescope node present before format negotiation completes (empty EnumFormat/Format)
        let unnegotiated_gamescope = br#"[
            {
                "id": 77,
                "info": {
                    "props": {
                        "node.name": "gamescope"
                    },
                    "params": {
                        "EnumFormat": []
                    }
                }
            }
        ]"#;
        assert_eq!(parse_gamescope_node_info(unnegotiated_gamescope), None);
    }

    #[test]
    fn test_parse_gamescope_node_info_late_720p_and_resolution_change() {
        // 1. Initially node is absent -> None
        let snapshot_0 = b"[]";
        assert_eq!(parse_gamescope_node_info(snapshot_0), None);

        // 2. Game launches at 1280x720
        let snapshot_720p = br#"[
            {
                "id": 88,
                "info": {
                    "props": {
                        "node.name": "gamescope"
                    },
                    "params": {
                        "EnumFormat": [
                            {
                                "size": { "width": 1280, "height": 720 }
                            }
                        ]
                    }
                }
            }
        ]"#;
        let info_720p = parse_gamescope_node_info(snapshot_720p).expect("expected 720p node");
        assert_eq!(
            info_720p,
            GamescopeNodeInfo {
                node_id: 88,
                width: 1280,
                height: 720
            }
        );
        assert_eq!(
            rgba_frame_byte_size(info_720p.width, info_720p.height),
            Some(1280 * 720 * 4)
        );

        // 3. Game changes resolution to 1920x1080 on reconnect (with active Format overriding default EnumFormat)
        let snapshot_1080p = br#"[
            {
                "id": 91,
                "info": {
                    "props": {
                        "node.name": "gamescope"
                    },
                    "params": {
                        "EnumFormat": [
                            {
                                "size": {
                                    "default": { "width": 1280, "height": 720 }
                                }
                            }
                        ],
                        "Format": [
                            {
                                "size": { "width": 1920, "height": 1080 }
                            }
                        ]
                    }
                }
            }
        ]"#;
        let info_1080p = parse_gamescope_node_info(snapshot_1080p).expect("expected 1080p node");
        assert_eq!(
            info_1080p,
            GamescopeNodeInfo {
                node_id: 91,
                width: 1920,
                height: 1080
            }
        );
        assert_eq!(
            rgba_frame_byte_size(info_1080p.width, info_1080p.height),
            Some(1920 * 1080 * 4)
        );
    }

    #[test]
    fn test_build_gst_pipeline_includes_videoscale_and_rgba_caps() {
        let info = GamescopeNodeInfo {
            node_id: 88,
            width: 1280,
            height: 720,
        };
        let args = build_gst_pipeline_args(&info);
        assert!(args.iter().any(|a| a == "videoscale"));
        assert!(args
            .iter()
            .any(|a| a == "video/x-raw,format=RGBA,width=1280,height=720"));
    }

    #[test]
    fn test_sleep_cancelable_stops_immediately_on_signal() {
        let stop = AtomicBool::new(true);
        let start = Instant::now();
        assert!(!sleep_cancelable(&stop, Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_millis(50));
    }
}
