use crate::video::types::{RawFrame, VideoFormat};
use anyhow::{anyhow, Context, Result};
use std::ffi::{c_void, CString};
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const VIDEO_OPEN_ATTEMPTS: u8 = 3;
const VIDEO_OPEN_RETRY_DELAY: Duration = Duration::from_millis(250);
const VIDEO_RECONNECT_DELAY: Duration = Duration::from_millis(100);
const VIDEO_READ_RETRY_DELAY: Duration = Duration::from_millis(1);
const VIDEO_STALL_TIMEOUT: Duration = Duration::from_secs(2);
const VIDEO_RECOVERY_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug)]
pub enum VideoThreadEvent {
    Started,
    Failed(String),
    Stopped,
}

pub struct VideoThreadConfig {
    pub device: String,
    pub format: VideoFormat,
    pub resolution: (u32, u32),
    pub framerate: u32,
    pub scaler_filter: Arc<AtomicU8>,
    pub color_range: Arc<AtomicU8>,
    pub stop_requested: Arc<AtomicBool>,
    pub request_repaint: Arc<dyn Fn() + Send + Sync>,
}

struct VideoInterruptState {
    stop_requested: Arc<AtomicBool>,
    last_progress: Mutex<Instant>,
}

struct VideoFrameProgress {
    last_frame_at: Instant,
}

impl VideoFrameProgress {
    fn new() -> Self {
        Self {
            last_frame_at: Instant::now(),
        }
    }

    fn mark_frame(&mut self) {
        self.last_frame_at = Instant::now();
    }

    fn stalled_for(&self, timeout: Duration) -> bool {
        self.last_frame_at.elapsed() >= timeout
    }
}

impl VideoInterruptState {
    fn new(stop_requested: Arc<AtomicBool>) -> Self {
        Self {
            stop_requested,
            last_progress: Mutex::new(Instant::now()),
        }
    }

    fn mark_progress(&self) {
        if let Ok(mut last_progress) = self.last_progress.lock() {
            *last_progress = Instant::now();
        }
    }

    fn timed_out(&self) -> bool {
        self.last_progress
            .lock()
            .map(|last_progress| last_progress.elapsed() >= VIDEO_STALL_TIMEOUT)
            .unwrap_or(true)
    }
}

unsafe extern "C" fn video_interrupt_callback(opaque: *mut c_void) -> libc::c_int {
    let state = unsafe { &*(opaque as *const VideoInterruptState) };
    (state.stop_requested.load(Ordering::Relaxed) || state.timed_out()) as libc::c_int
}

fn renderable_pixel_format(format: ffmpeg_next::format::Pixel) -> ffmpeg_next::format::Pixel {
    use ffmpeg_next::format::Pixel;

    match format {
        Pixel::YUV422P | Pixel::YUV420P | Pixel::YUVJ422P | Pixel::YUVJ420P | Pixel::YUYV422 => {
            format
        }
        _ => Pixel::YUV420P,
    }
}

fn plane_tight_row_bytes(frame: &ffmpeg_next::frame::Video, plane: usize) -> Result<usize> {
    use ffmpeg_next::format::Pixel;

    match frame.format() {
        Pixel::YUYV422 if plane == 0 => Ok(frame.width() as usize * 2),
        Pixel::YUV422P | Pixel::YUV420P | Pixel::YUVJ422P | Pixel::YUVJ420P => {
            Ok(frame.plane_width(plane) as usize)
        }
        other => Err(anyhow!("Unsupported normalized pixel format: {:?}", other)),
    }
}

fn copy_frame_tightly(frame: &ffmpeg_next::frame::Video) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    for plane in 0..frame.planes() {
        let row_bytes = plane_tight_row_bytes(frame, plane)?;
        let row_count = frame.plane_height(plane) as usize;
        let stride = frame.stride(plane);
        let plane_data = frame.data(plane);

        if row_bytes > stride {
            return Err(anyhow!(
                "Plane {} row width {} exceeds stride {}",
                plane,
                row_bytes,
                stride
            ));
        }

        data.reserve(row_bytes * row_count);
        for row in 0..row_count {
            let start = row * stride;
            let end = start + row_bytes;
            let row_data = plane_data.get(start..end).ok_or_else(|| {
                anyhow!(
                    "Plane {} row {} is out of bounds: {}..{} of {}",
                    plane,
                    row,
                    start,
                    end,
                    plane_data.len()
                )
            })?;
            data.extend_from_slice(row_data);
        }
    }
    Ok(data)
}

fn setup_ffmpeg_options(
    format: &VideoFormat,
    resolution: (u32, u32),
    framerate: u32,
) -> (String, ffmpeg_next::Dictionary<'_>) {
    let mut pixel_format_str = format.fourcc.trim_end_matches('\0').to_lowercase();
    if pixel_format_str == "yuyv" {
        pixel_format_str = "yuyv422".to_string();
    } else if pixel_format_str == "mjpg" {
        pixel_format_str = "mjpeg".to_string();
    }
    let mut ffmpeg_options = ffmpeg_next::Dictionary::new();
    ffmpeg_options.set("video_size", &format!("{}x{}", resolution.0, resolution.1));
    ffmpeg_options.set("framerate", &framerate.to_string());
    ffmpeg_options.set("input_format", &pixel_format_str);
    ffmpeg_options.set("fflags", "nobuffer+discardcorrupt");
    ffmpeg_options.set("probesize", "32");
    ffmpeg_options.set("analyzeduration", "100000");
    ffmpeg_options.set("pixel_format", &pixel_format_str);
    ffmpeg_options.set("color_range", "pc");
    (pixel_format_str, ffmpeg_options)
}

fn open_video_input(
    device: &str,
    options: ffmpeg_next::Dictionary<'_>,
    interrupt_state: &mut VideoInterruptState,
) -> Result<ffmpeg_next::format::context::Input> {
    use ffmpeg_next::sys::{
        avformat_alloc_context, avformat_close_input, avformat_find_stream_info,
        avformat_open_input, AVIOInterruptCB,
    };

    let device = CString::new(device).context("Video device path contains a NUL byte")?;

    unsafe {
        let mut context = avformat_alloc_context();
        if context.is_null() {
            return Err(anyhow!("FFmpeg could not allocate an input context"));
        }

        (*context).interrupt_callback = AVIOInterruptCB {
            callback: Some(video_interrupt_callback),
            opaque: interrupt_state as *mut VideoInterruptState as *mut c_void,
        };

        let mut options_ptr = options.disown();
        let open_result = avformat_open_input(
            &mut context,
            device.as_ptr(),
            std::ptr::null_mut(),
            &mut options_ptr,
        );
        drop(ffmpeg_next::Dictionary::own(options_ptr));

        if open_result < 0 {
            avformat_close_input(&mut context);
            return Err(ffmpeg_next::Error::from(open_result))
                .context("FFmpeg could not open the video input");
        }

        let stream_info_result = avformat_find_stream_info(context, std::ptr::null_mut());
        if stream_info_result < 0 {
            avformat_close_input(&mut context);
            return Err(ffmpeg_next::Error::from(stream_info_result))
                .context("FFmpeg could not read video stream information");
        }

        Ok(ffmpeg_next::format::context::Input::wrap(context))
    }
}

pub fn video_thread_main(
    frame_sender: crossbeam_channel::Sender<Arc<RawFrame>>,
    status_sender: crossbeam_channel::Sender<VideoThreadEvent>,
    config: VideoThreadConfig,
) -> Result<()> {
    let VideoThreadConfig {
        device,
        format,
        resolution,
        framerate,
        scaler_filter,
        color_range,
        stop_requested,
        request_repaint,
    } = config;
    ffmpeg_next::init().context("Failed to initialize FFmpeg")?;

    let mut started = false;
    let mut frame_progress = VideoFrameProgress::new();
    loop {
        let result = run_video_capture_session(
            &frame_sender,
            &status_sender,
            &device,
            &format,
            resolution,
            framerate,
            &scaler_filter,
            &color_range,
            &stop_requested,
            &request_repaint,
            &mut started,
            &mut frame_progress,
        );

        if stop_requested.load(Ordering::Relaxed) {
            return Ok(());
        }

        match result {
            Ok(()) => return Ok(()),
            Err(error) if !started => return Err(error),
            Err(error) if !frame_progress.stalled_for(VIDEO_RECOVERY_TIMEOUT) => {
                tracing::warn!(
                    error = %error,
                    "Video capture stalled; reopening the device"
                );
                sleep_until_video_reconnect(&stop_requested);
            }
            Err(error) => {
                return Err(error).context(format!(
                    "Video capture did not recover for {:?}",
                    VIDEO_RECOVERY_TIMEOUT
                ));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_video_capture_session(
    frame_sender: &crossbeam_channel::Sender<Arc<RawFrame>>,
    status_sender: &crossbeam_channel::Sender<VideoThreadEvent>,
    device: &str,
    format: &VideoFormat,
    resolution: (u32, u32),
    framerate: u32,
    scaler_filter: &Arc<AtomicU8>,
    color_range: &Arc<AtomicU8>,
    stop_requested: &Arc<AtomicBool>,
    request_repaint: &Arc<dyn Fn() + Send + Sync>,
    started: &mut bool,
    frame_progress: &mut VideoFrameProgress,
) -> Result<()> {
    let initial_range = color_range.load(Ordering::Relaxed);
    let mut open_attempt = 1;
    let mut interrupt_state = Box::new(VideoInterruptState::new(Arc::clone(stop_requested)));
    let mut ictx = loop {
        interrupt_state.mark_progress();
        let (_pixel_format, mut ffmpeg_options) =
            setup_ffmpeg_options(format, resolution, framerate);
        ffmpeg_options.set("color_range", if initial_range == 1 { "tv" } else { "pc" });

        tracing::info!(
            device = %device,
            attempt = open_attempt,
            options = ?ffmpeg_options,
            "Starting FFmpeg with options"
        );
        match open_video_input(device, ffmpeg_options, &mut interrupt_state) {
            Ok(input) => break input,
            Err(_) if stop_requested.load(Ordering::Relaxed) => return Ok(()),
            Err(error) if open_attempt < VIDEO_OPEN_ATTEMPTS => {
                tracing::warn!(
                    device = %device,
                    attempt = open_attempt,
                    error = %error,
                    "Failed to open video device; retrying"
                );
                open_attempt += 1;
                thread::sleep(VIDEO_OPEN_RETRY_DELAY);
            }
            Err(error) => {
                return Err(error).context("Failed to open input device with ffmpeg");
            }
        }
    };

    let input = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .context("Could not find best video stream")?;
    let video_stream_index = input.index();

    let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(input.parameters())
        .and_then(|c| c.decoder().video())
        .context("Failed to create software video decoder")?;

    decoder.set_threading(ffmpeg_next::codec::threading::Config::default());
    let mut current_scaler_val = scaler_filter.load(Ordering::Relaxed);
    let output_format = renderable_pixel_format(decoder.format());
    let mut scaler = ffmpeg_next::software::scaling::context::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        output_format,
        decoder.width(),
        decoder.height(),
        crate::video::types::ScalerFilter::from_u8(current_scaler_val).into_ffmpeg_flag(),
    )
    .context("Failed to create software scaler for normalization")?;
    loop {
        let new_scaler_val = scaler_filter.load(Ordering::Relaxed);
        if new_scaler_val != current_scaler_val {
            current_scaler_val = new_scaler_val;
            scaler = ffmpeg_next::software::scaling::context::Context::get(
                decoder.format(),
                decoder.width(),
                decoder.height(),
                output_format,
                decoder.width(),
                decoder.height(),
                crate::video::types::ScalerFilter::from_u8(current_scaler_val).into_ffmpeg_flag(),
            )
            .context("Failed to re-create software scaler")?;
        }

        if stop_requested.load(Ordering::Relaxed) {
            return Ok(());
        }

        interrupt_state.mark_progress();
        let mut packet = ffmpeg_next::Packet::empty();
        match packet.read(&mut ictx) {
            Ok(()) => {}
            Err(ffmpeg_next::Error::Eof) => {
                return Err(anyhow!("Video input reached end of stream"));
            }
            Err(_) if stop_requested.load(Ordering::Relaxed) => return Ok(()),
            Err(ffmpeg_next::Error::Other { errno }) if errno == libc::EAGAIN => {
                if frame_progress.stalled_for(VIDEO_STALL_TIMEOUT) {
                    return Err(anyhow!(
                        "Video input produced no decoded frames for {:?}",
                        VIDEO_STALL_TIMEOUT
                    ));
                }
                thread::sleep(VIDEO_READ_RETRY_DELAY);
                continue;
            }
            Err(error) => return Err(error).context("Failed to read a video packet"),
        }

        if packet.stream() != video_stream_index {
            continue;
        }

        decoder
            .send_packet(&packet)
            .context("Failed to send packet to decoder")?;
        let mut decoded = ffmpeg_next::frame::Video::empty();
        while decoder.receive_frame(&mut decoded).is_ok() {
            // Use the scaler to normalize the frame (removes strides and ensures consistent plane layout)
            let mut normalized = ffmpeg_next::frame::Video::empty();

            let range_val = color_range.load(Ordering::Relaxed);

            scaler
                .run(&decoded, &mut normalized)
                .context("Failed to normalize video frame")?;

            let width = normalized.width();
            let height = normalized.height();
            let format = normalized.format();
            let data = copy_frame_tightly(&normalized)?;

            let raw_frame = Arc::new(RawFrame {
                width,
                height,
                data,
                format,
                color_range: crate::video::types::ColorRange::from_u8(range_val),
            });

            frame_progress.mark_frame();

            match frame_sender.try_send(raw_frame) {
                Ok(()) => request_repaint(),
                Err(crossbeam_channel::TrySendError::Full(_)) => {}
                Err(crossbeam_channel::TrySendError::Disconnected(_)) => return Ok(()),
            }

            if !*started {
                let _ = status_sender.send(VideoThreadEvent::Started);
                request_repaint();
                *started = true;
            }
        }

        // Some failed capture devices continue dequeuing corrupt or empty packets. Packet I/O
        // is not useful progress: only a decoded frame proves that the stream is still alive.
        if frame_progress.stalled_for(VIDEO_STALL_TIMEOUT) {
            return Err(anyhow!(
                "Video input produced no decoded frames for {:?}",
                VIDEO_STALL_TIMEOUT
            ));
        }
    }
}

fn sleep_until_video_reconnect(stop_requested: &AtomicBool) {
    let retry_count = (VIDEO_RECONNECT_DELAY.as_millis() / 10).max(1);
    for _ in 0..retry_count {
        if stop_requested.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_interrupt_honors_stop_requests_and_stall_timeout() {
        let stop_requested = Arc::new(AtomicBool::new(false));
        let state = VideoInterruptState::new(Arc::clone(&stop_requested));

        assert_eq!(
            unsafe {
                video_interrupt_callback(&state as *const VideoInterruptState as *mut c_void)
            },
            0
        );

        *state.last_progress.lock().unwrap() = Instant::now() - VIDEO_STALL_TIMEOUT;
        assert!(state.timed_out());

        state.mark_progress();
        stop_requested.store(true, Ordering::Relaxed);
        assert_eq!(
            unsafe {
                video_interrupt_callback(&state as *const VideoInterruptState as *mut c_void)
            },
            1
        );
    }

    #[test]
    fn decoded_frames_are_the_video_watchdog_progress_signal() {
        let mut progress = VideoFrameProgress {
            last_frame_at: Instant::now() - VIDEO_STALL_TIMEOUT,
        };
        assert!(progress.stalled_for(VIDEO_STALL_TIMEOUT));

        // Reading another packet must not reset this timestamp. Only successfully decoding a
        // frame does, which prevents corrupt packet traffic from hiding a dead capture stream.
        progress.mark_frame();
        assert!(!progress.stalled_for(VIDEO_STALL_TIMEOUT));
    }

    #[test]
    fn test_setup_ffmpeg_options_mjpg() {
        ffmpeg_next::init().unwrap();
        let format = VideoFormat {
            fourcc: "MJPG\0".to_string(),
            description: "MJPEG".to_string(),
            resolutions: vec![],
        };
        let (pix_fmt, options) = setup_ffmpeg_options(&format, (1920, 1080), 60);
        assert_eq!(pix_fmt, "mjpeg");
        assert_eq!(options.get("video_size"), Some("1920x1080"));
        assert_eq!(options.get("framerate"), Some("60"));
        assert_eq!(options.get("input_format"), Some("mjpeg"));
        assert_eq!(options.get("color_range"), Some("pc"));
    }

    #[test]
    fn test_setup_ffmpeg_options_yuyv() {
        ffmpeg_next::init().unwrap();
        let format = VideoFormat {
            fourcc: "YUYV".to_string(),
            description: "YUYV 4:2:2".to_string(),
            resolutions: vec![],
        };
        let (pix_fmt, options) = setup_ffmpeg_options(&format, (1280, 720), 30);
        assert_eq!(pix_fmt, "yuyv422");
        assert_eq!(options.get("video_size"), Some("1280x720"));
        assert_eq!(options.get("framerate"), Some("30"));
        assert_eq!(options.get("input_format"), Some("yuyv422"));
    }

    #[test]
    fn test_setup_ffmpeg_options_other() {
        ffmpeg_next::init().unwrap();
        let format = VideoFormat {
            fourcc: "nv12".to_string(),
            description: "NV12".to_string(),
            resolutions: vec![],
        };
        let (pix_fmt, options) = setup_ffmpeg_options(&format, (800, 600), 120);
        assert_eq!(pix_fmt, "nv12");
        assert_eq!(options.get("video_size"), Some("800x600"));
        assert_eq!(options.get("framerate"), Some("120"));
        assert_eq!(options.get("input_format"), Some("nv12"));
    }
}
