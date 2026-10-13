use crate::video::types::{ColorRange, RawFrame};
use anyhow::{Context, Result};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const GAMESCOPE_WIDTH: u32 = 2560;
pub const GAMESCOPE_HEIGHT: u32 = 1440;
pub const GAMESCOPE_FPS: u32 = 60;
pub const GAMESCOPE_FRAME_SIZE: usize = ((GAMESCOPE_WIDTH * GAMESCOPE_HEIGHT * 3) / 2) as usize;

pub fn detect_gamescope_resolution() -> (u32, u32) {
    if let Ok(output) = Command::new("pw-dump").output() {
        if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
            if let Some(nodes) = val.as_array() {
                for node in nodes {
                    if node["info"]["props"]["node.name"].as_str() == Some("gamescope") {
                        if let Some(formats) = node["info"]["params"]["EnumFormat"].as_array() {
                            for fmt in formats {
                                if let (Some(w), Some(h)) = (
                                    fmt["size"]["width"].as_u64(),
                                    fmt["size"]["height"].as_u64(),
                                ) {
                                    if w > 0 && h > 0 {
                                        tracing::info!("Detected Gamescope PipeWire resolution: {w}x{h}");
                                        return (w as u32, h as u32);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    (GAMESCOPE_WIDTH, GAMESCOPE_HEIGHT)
}

pub struct GamescopeVideoHandle {
    pub stop_requested: Arc<AtomicBool>,
    pub width: u32,
    pub height: u32,
    child_process: Arc<Mutex<Option<Child>>>,
    thread_handle: Option<JoinHandle<()>>,
}

impl Drop for GamescopeVideoHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

impl GamescopeVideoHandle {
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
    let (width, height) = detect_gamescope_resolution();
    let frame_size = ((width * height * 3) / 2) as usize;

    let stop_requested = Arc::new(AtomicBool::new(false));
    let child_process = Arc::new(Mutex::new(None));

    let stop_clone = stop_requested.clone();
    let child_process_clone = child_process.clone();

    let thread_handle = thread::Builder::new()
        .name("gamescope-video-pipe".to_string())
        .spawn(move || {
            let mut backoff = Duration::from_millis(200);

            while !stop_clone.load(Ordering::Relaxed) {
                tracing::info!(
                    "Launching optimized GStreamer pipewiresrc for gamescope ({width}x{height} @ {GAMESCOPE_FPS}fps)..."
                );
                let mut cmd = Command::new("gst-launch-1.0");
                cmd.args([
                    "-q",
                    "pipewiresrc",
                    "target-object=gamescope",
                    "keepalive-time=1000",
                    "!",
                    "video/x-raw",
                    "!",
                    "queue",
                    "max-size-buffers=3",
                    "!",
                    "videoconvert",
                    "n-threads=4",
                    "!",
                    &format!(
                        "video/x-raw,format=I420,width={},height={}",
                        width, height
                    ),
                    "!",
                    "fdsink",
                    "fd=1",
                ]);
                cmd.stdout(Stdio::piped());
                cmd.stderr(Stdio::inherit());

                let mut child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("Failed to spawn gst-launch-1.0 for gamescope: {e}");
                        thread::sleep(backoff);
                        backoff = (backoff * 2).min(Duration::from_secs(2));
                        continue;
                    }
                };

                let mut stdout = match child.stdout.take() {
                    Some(s) => s,
                    None => {
                        let _ = child.kill();
                        continue;
                    }
                };

                if let Ok(mut lock) = child_process_clone.lock() {
                    *lock = Some(child);
                }

                backoff = Duration::from_millis(200);
                let mut frame_buffer = vec![0u8; frame_size];

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
                                format: ffmpeg_next::format::Pixel::YUV420P,
                                color_range: ColorRange::from_u8(
                                    color_range.load(Ordering::Relaxed),
                                ),
                            });

                            if frame_sender.send(raw_frame).is_err() {
                                break;
                            }
                            request_repaint();
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
                thread::sleep(Duration::from_millis(500));
            }
        })
        .context("Failed to spawn gamescope video worker thread")?;

    Ok(GamescopeVideoHandle {
        stop_requested,
        width,
        height,
        child_process,
        thread_handle: Some(thread_handle),
    })
}
