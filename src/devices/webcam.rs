use crate::video::decoder::monotonic_nanos;
use crate::video::types::{checkout_frame_buffer, recycle_frame_buffer, VideoFormat};
use anyhow::{anyhow, Context, Result};
use std::ffi::{c_void, CString};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

const WEBCAM_OPEN_ATTEMPTS: u8 = 10;
const WEBCAM_OPEN_RETRY_DELAY: Duration = Duration::from_millis(150);
const WEBCAM_STALL_TIMEOUT: Duration = Duration::from_secs(2);
pub const DEFAULT_WEBCAM_MAX_WIDTH: u32 = 960;
pub const DEFAULT_WEBCAM_MAX_HEIGHT: u32 = 540;
const MIN_WEBCAM_SCALE_DIM: u32 = 64;

static WEBCAM_TARGET_WIDTH: AtomicU32 = AtomicU32::new(DEFAULT_WEBCAM_MAX_WIDTH);
static WEBCAM_TARGET_HEIGHT: AtomicU32 = AtomicU32::new(DEFAULT_WEBCAM_MAX_HEIGHT);

pub fn set_webcam_target_dimensions(width: u32, height: u32) {
    let qw = width
        .div_ceil(32)
        .saturating_mul(32)
        .clamp(MIN_WEBCAM_SCALE_DIM, DEFAULT_WEBCAM_MAX_WIDTH);
    let qh = height
        .div_ceil(32)
        .saturating_mul(32)
        .clamp(MIN_WEBCAM_SCALE_DIM, DEFAULT_WEBCAM_MAX_HEIGHT);
    WEBCAM_TARGET_WIDTH.store(qw, Ordering::Relaxed);
    WEBCAM_TARGET_HEIGHT.store(qh, Ordering::Relaxed);
}

pub(crate) fn compute_bounded_pip_dimensions(
    src_w: u32,
    src_h: u32,
    max_w: u32,
    max_h: u32,
) -> (u32, u32) {
    if src_w == 0 || src_h == 0 || max_w == 0 || max_h == 0 {
        return (src_w, src_h);
    }
    if src_w <= max_w && src_h <= max_h {
        return (src_w, src_h);
    }
    let scale = (max_w as f64 / src_w as f64).min(max_h as f64 / src_h as f64);
    let out_w = (((src_w as f64 * scale).round() as u32) & !1)
        .max(2)
        .min(src_w);
    let out_h = (((src_h as f64 * scale).round() as u32) & !1)
        .max(2)
        .min(src_h);
    (out_w, out_h)
}

#[derive(Clone, Debug)]
pub struct WebcamFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Drop for WebcamFrame {
    fn drop(&mut self) {
        recycle_frame_buffer(std::mem::take(&mut self.rgba));
    }
}

pub struct WebcamStreamHandle {
    pub device: String,
    pub format_fourcc: String,
    pub resolution: (u32, u32),
    pub framerate: u32,
    pub stop_requested: Arc<AtomicBool>,
    pub thread_handle: Option<thread::JoinHandle<()>>,
}

impl WebcamStreamHandle {
    pub fn take_stop_handle(&mut self) -> Option<thread::JoinHandle<()>> {
        self.stop_requested.store(true, Ordering::Relaxed);
        self.thread_handle.take()
    }
}

impl Drop for WebcamStreamHandle {
    fn drop(&mut self) {
        self.stop_requested.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread_handle.take() {
            reap_webcam_thread(handle);
        }
    }
}

fn reap_webcam_thread(handle: thread::JoinHandle<()>) {
    if handle.is_finished() {
        let _ = handle.join();
        return;
    }
    let _ = thread::Builder::new()
        .name("webcam-reaper".to_string())
        .spawn(move || {
            let _ = handle.join();
        });
}

struct WebcamInterruptState {
    stop_requested: Arc<AtomicBool>,
    last_progress_ns: AtomicU64,
}

impl WebcamInterruptState {
    fn new(stop_requested: Arc<AtomicBool>) -> Self {
        Self {
            stop_requested,
            last_progress_ns: AtomicU64::new(monotonic_nanos()),
        }
    }

    fn mark_progress(&self) {
        self.last_progress_ns
            .store(monotonic_nanos(), Ordering::Relaxed);
    }

    fn timed_out(&self) -> bool {
        let last_ns = self.last_progress_ns.load(Ordering::Relaxed);
        monotonic_nanos().saturating_sub(last_ns) >= WEBCAM_STALL_TIMEOUT.as_nanos() as u64
    }
}

unsafe extern "C" fn webcam_interrupt_callback(opaque: *mut c_void) -> libc::c_int {
    let state = unsafe { &*(opaque as *const WebcamInterruptState) };
    (state.stop_requested.load(Ordering::Relaxed) || state.timed_out()) as libc::c_int
}

fn setup_webcam_ffmpeg_options(
    format: &VideoFormat,
    resolution: (u32, u32),
    framerate: u32,
) -> ffmpeg_next::Dictionary<'static> {
    crate::video::decoder::setup_v4l2_ffmpeg_options(format, resolution, framerate, false).1
}

fn open_webcam_input(
    device: &str,
    options: ffmpeg_next::Dictionary<'_>,
    interrupt_state: &mut WebcamInterruptState,
) -> Result<ffmpeg_next::format::context::Input> {
    use ffmpeg_next::sys::{
        av_find_input_format, avformat_alloc_context, avformat_close_input, avformat_open_input,
        AVIOInterruptCB,
    };

    let device_c = CString::new(device).context("Webcam device path contains a NUL byte")?;

    unsafe {
        let mut context = avformat_alloc_context();
        if context.is_null() {
            return Err(anyhow!(
                "FFmpeg could not allocate input context for webcam"
            ));
        }

        (*context).interrupt_callback = AVIOInterruptCB {
            callback: Some(webcam_interrupt_callback),
            opaque: interrupt_state as *mut WebcamInterruptState as *mut c_void,
        };

        let input_fmt = av_find_input_format(c"v4l2".as_ptr());
        let mut options_ptr = options.disown();
        let open_result =
            avformat_open_input(&mut context, device_c.as_ptr(), input_fmt, &mut options_ptr);
        drop(ffmpeg_next::Dictionary::own(options_ptr));

        if open_result < 0 {
            avformat_close_input(&mut context);
            return Err(ffmpeg_next::Error::from(open_result))
                .context("FFmpeg could not open webcam input");
        }

        // `v4l2_read_header` already populates `codec_type`, `codec_id`, `width`, and `height`
        // during `avformat_open_input`. Skipping `avformat_find_stream_info` when `codec_id`
        // and dimensions are known avoids redundant probing decode passes and prevents consuming
        // or delaying the initial H.264 IDR keyframe.
        if let Err(err) = crate::video::decoder::probe_v4l2_stream_info_if_needed(context) {
            avformat_close_input(&mut context);
            return Err(err).context("FFmpeg could not read webcam stream info");
        }

        Ok(ffmpeg_next::format::context::Input::wrap(context))
    }
}

pub(crate) fn send_latest_webcam_frame(
    frame_sender: &crossbeam_channel::Sender<Arc<WebcamFrame>>,
    frame_drop_rx: &crossbeam_channel::Receiver<Arc<WebcamFrame>>,
    mut frame: Arc<WebcamFrame>,
) -> bool {
    loop {
        match frame_sender.try_send(frame) {
            Ok(()) => return true,
            Err(crossbeam_channel::TrySendError::Full(returned)) => {
                frame = returned;
                let _ = frame_drop_rx.try_recv();
            }
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => return false,
        }
    }
}

pub fn start_webcam_stream(
    device: &str,
    format: VideoFormat,
    resolution: (u32, u32),
    framerate: u32,
    frame_sender: crossbeam_channel::Sender<Arc<WebcamFrame>>,
    frame_drop_rx: crossbeam_channel::Receiver<Arc<WebcamFrame>>,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    prev_thread: Option<thread::JoinHandle<()>>,
) -> Result<WebcamStreamHandle> {
    let stop_requested = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop_requested);
    let device_str = device.to_string();
    let device_clone = device_str.clone();

    let format_fourcc = format.fourcc.clone();
    let thread_handle = thread::Builder::new()
        .name("webcam-capture".to_string())
        .spawn(move || {
            if let Some(prev) = prev_thread {
                let _ = prev.join();
            }
            if stop_clone.load(Ordering::Relaxed) {
                return;
            }
            if let Err(e) = run_webcam_worker(
                &device_clone,
                &format,
                resolution,
                framerate,
                &frame_sender,
                &frame_drop_rx,
                &request_repaint,
                &stop_clone,
            ) {
                tracing::error!(
                    "Webcam capture worker for {} exited with error: {:#}",
                    device_clone,
                    e
                );
            }
        })
        .context("Failed to spawn webcam capture thread")?;

    Ok(WebcamStreamHandle {
        device: device_str,
        format_fourcc,
        resolution,
        framerate,
        stop_requested,
        thread_handle: Some(thread_handle),
    })
}

#[allow(clippy::too_many_arguments)]
fn run_webcam_worker(
    device: &str,
    format: &VideoFormat,
    resolution: (u32, u32),
    framerate: u32,
    frame_sender: &crossbeam_channel::Sender<Arc<WebcamFrame>>,
    frame_drop_rx: &crossbeam_channel::Receiver<Arc<WebcamFrame>>,
    request_repaint: &Arc<dyn Fn() + Send + Sync>,
    stop_requested: &Arc<AtomicBool>,
) -> Result<()> {
    ffmpeg_next::init().context("Failed to initialize FFmpeg")?;

    let mut interrupt_state = Box::new(WebcamInterruptState::new(Arc::clone(stop_requested)));
    let mut open_attempt = 1;

    let mut ictx = loop {
        if stop_requested.load(Ordering::Relaxed) {
            return Ok(());
        }
        interrupt_state.mark_progress();
        let options = setup_webcam_ffmpeg_options(format, resolution, framerate);

        match open_webcam_input(device, options, &mut interrupt_state) {
            Ok(input) => break input,
            Err(_) if stop_requested.load(Ordering::Relaxed) => return Ok(()),
            Err(e) if open_attempt < WEBCAM_OPEN_ATTEMPTS => {
                tracing::warn!(
                    "Webcam open attempt {} failed: {}; retrying...",
                    open_attempt,
                    e
                );
                open_attempt += 1;
                thread::sleep(WEBCAM_OPEN_RETRY_DELAY);
            }
            Err(e) => return Err(e).context("Failed to open webcam after retries"),
        }
    };

    let input_stream = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .context("Could not find video stream in webcam")?;
    let video_stream_index = input_stream.index();

    let mut codec_ctx =
        ffmpeg_next::codec::context::Context::from_parameters(input_stream.parameters())
            .context("Failed to create webcam codec context")?;
    codec_ctx.set_threading(ffmpeg_next::codec::threading::Config {
        kind: ffmpeg_next::codec::threading::Type::Slice,
        count: 1,
    });
    unsafe {
        (*codec_ctx.as_mut_ptr()).flags |= ffmpeg_next::sys::AV_CODEC_FLAG_LOW_DELAY as libc::c_int;
    }
    let mut decoder = codec_ctx
        .decoder()
        .video()
        .context("Failed to create webcam video decoder")?;

    let mut scaler: Option<ffmpeg_next::software::scaling::context::Context> = None;
    let mut decoded_frame = ffmpeg_next::frame::Video::empty();
    let mut rgba_frame = ffmpeg_next::frame::Video::empty();

    while !stop_requested.load(Ordering::Relaxed) {
        interrupt_state.mark_progress();
        let mut packet = ffmpeg_next::Packet::empty();
        match packet.read(&mut ictx) {
            Ok(()) => {}
            Err(ffmpeg_next::Error::Eof) => break,
            Err(_) if stop_requested.load(Ordering::Relaxed) => break,
            Err(ffmpeg_next::Error::Other { errno }) if errno == libc::EAGAIN => {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(e) => {
                tracing::warn!("Webcam packet read error: {}", e);
                break;
            }
        }

        if crate::video::decoder::should_skip_v4l2_packet(&packet, video_stream_index) {
            continue;
        }

        if decoder.send_packet(&packet).is_ok() {
            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                if stop_requested.load(Ordering::Relaxed) {
                    return Ok(());
                }

                if decoded_frame.width() == 0
                    || decoded_frame.height() == 0
                    || decoded_frame.format() == ffmpeg_next::format::Pixel::None
                {
                    continue;
                }

                let (dst_w, dst_h) = compute_bounded_pip_dimensions(
                    decoded_frame.width(),
                    decoded_frame.height(),
                    WEBCAM_TARGET_WIDTH.load(Ordering::Relaxed),
                    WEBCAM_TARGET_HEIGHT.load(Ordering::Relaxed),
                );
                let needs_reinit = scaler.as_ref().is_none_or(|s| {
                    decoded_frame.width() != s.input().width
                        || decoded_frame.height() != s.input().height
                        || decoded_frame.format() != s.input().format
                        || dst_w != s.output().width
                        || dst_h != s.output().height
                });
                if needs_reinit {
                    match ffmpeg_next::software::scaling::context::Context::get(
                        decoded_frame.format(),
                        decoded_frame.width(),
                        decoded_frame.height(),
                        ffmpeg_next::format::Pixel::RGBA,
                        dst_w,
                        dst_h,
                        ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR,
                    ) {
                        Ok(new_scaler) => scaler = Some(new_scaler),
                        Err(e) => {
                            tracing::warn!(
                                "Failed to create scaler for webcam RGBA conversion: {}",
                                e
                            );
                            continue;
                        }
                    }
                }

                if let Some(ref mut active_scaler) = scaler {
                    if active_scaler.run(&decoded_frame, &mut rgba_frame).is_ok() {
                        let w = rgba_frame.width();
                        let h = rgba_frame.height();
                        let stride = rgba_frame.stride(0);
                        let row_bytes = (w * 4) as usize;
                        let total_bytes = row_bytes * h as usize;
                        let plane_data = rgba_frame.data(0);

                        let mut tight_rgba = checkout_frame_buffer(total_bytes);
                        if stride == row_bytes {
                            let Some(src) = plane_data.get(..total_bytes) else {
                                continue;
                            };
                            tight_rgba.copy_from_slice(src);
                        } else {
                            let mut copy_ok = true;
                            for y in 0..h as usize {
                                let start = y * stride;
                                let end = start + row_bytes;
                                if let Some(row) = plane_data.get(start..end) {
                                    let dst_start = y * row_bytes;
                                    tight_rgba[dst_start..dst_start + row_bytes]
                                        .copy_from_slice(row);
                                } else {
                                    copy_ok = false;
                                    break;
                                }
                            }
                            if !copy_ok {
                                continue;
                            }
                        }

                        let frame = Arc::new(WebcamFrame {
                            width: w,
                            height: h,
                            rgba: tight_rgba,
                        });

                        // Drain stale queued frame if full so UI always displays the newest frame
                        if !send_latest_webcam_frame(frame_sender, frame_drop_rx, frame) {
                            return Ok(());
                        }
                        request_repaint();
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_setup_webcam_ffmpeg_options_preserves_initial_keyframe_buffer() {
        let fmt = VideoFormat {
            fourcc: "H264".to_string(),
            description: "H.264".to_string(),
            resolutions: Vec::new(),
        };
        let opts = setup_webcam_ffmpeg_options(&fmt, (1920, 1080), 30);
        assert_eq!(opts.get("input_format"), Some("h264"));
        assert_eq!(opts.get("video_size"), Some("1920x1080"));
        assert_eq!(opts.get("framerate"), Some("30"));
        // Must NOT contain `nobuffer` so `avformat_find_stream_info` (if invoked)
        // retains the initial IDR keyframe in `packet_buffer`.
        let fflags = opts.get("fflags").unwrap_or("");
        assert!(!fflags.contains("nobuffer"));
    }

    #[test]
    fn test_setup_webcam_ffmpeg_options_maps_mjpg_and_yuyv() {
        let mjpg = VideoFormat {
            fourcc: "MJPG".to_string(),
            description: "Motion-JPEG".to_string(),
            resolutions: Vec::new(),
        };
        let opts = setup_webcam_ffmpeg_options(&mjpg, (1280, 720), 60);
        assert_eq!(opts.get("input_format"), Some("mjpeg"));

        let yuyv = VideoFormat {
            fourcc: "YUYV".to_string(),
            description: "YUYV 4:2:2".to_string(),
            resolutions: Vec::new(),
        };
        let opts_yuyv = setup_webcam_ffmpeg_options(&yuyv, (640, 480), 30);
        assert_eq!(opts_yuyv.get("input_format"), Some("yuyv422"));
    }

    #[test]
    fn test_take_stop_handle_signals_stop_and_extracts_join_handle() {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let join = thread::spawn(move || {
            while !stop_clone.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(5));
            }
        });
        let mut handle = WebcamStreamHandle {
            device: "/dev/video2".to_string(),
            format_fourcc: "H264".to_string(),
            resolution: (1920, 1080),
            framerate: 30,
            stop_requested: Arc::clone(&stop),
            thread_handle: Some(join),
        };

        let extracted = handle.take_stop_handle();
        assert!(stop.load(Ordering::Relaxed));
        assert!(extracted.is_some());
        assert!(handle.thread_handle.is_none());
        extracted.unwrap().join().unwrap();
    }

    #[test]
    fn test_webcam_interrupt_honors_stop_and_timeout() {
        let stop = Arc::new(AtomicBool::new(false));
        let state = WebcamInterruptState::new(Arc::clone(&stop));
        assert_eq!(
            unsafe {
                webcam_interrupt_callback(&state as *const WebcamInterruptState as *mut c_void)
            },
            0
        );

        state.last_progress_ns.store(
            monotonic_nanos() - WEBCAM_STALL_TIMEOUT.as_nanos() as u64,
            Ordering::Relaxed,
        );
        assert!(state.timed_out());

        state.mark_progress();
        assert!(!state.timed_out());
        stop.store(true, Ordering::Relaxed);
        assert_eq!(
            unsafe {
                webcam_interrupt_callback(&state as *const WebcamInterruptState as *mut c_void)
            },
            1
        );
    }

    #[test]
    fn test_compute_bounded_pip_dimensions_caps_4k_without_upscaling_small_inputs() {
        assert_eq!(
            compute_bounded_pip_dimensions(640, 480, 960, 540),
            (640, 480)
        );
        assert_eq!(
            compute_bounded_pip_dimensions(3840, 2160, 960, 540),
            (960, 540)
        );
        let (w, h) = compute_bounded_pip_dimensions(1920, 1080, 320, 180);
        assert_eq!((w, h), (320, 180));
        assert_eq!(w % 2, 0);
        assert_eq!(h % 2, 0);
    }

    #[test]
    fn test_send_latest_webcam_frame_evicts_stale_frame_and_keeps_newest() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        for seq in 1..=3u8 {
            let mut buf = checkout_frame_buffer(96 * 1024);
            buf[0] = seq;
            let frame = Arc::new(WebcamFrame {
                width: 160,
                height: 150,
                rgba: buf,
            });
            assert!(send_latest_webcam_frame(&tx, &rx, frame));
        }

        let latest = rx.try_recv().expect("latest frame must be present");
        assert_eq!(latest.rgba[0], 3, "channel must retain the newest frame");
        assert!(rx.try_recv().is_err());
    }
}
