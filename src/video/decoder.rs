use crate::video::types::{RawFrame, VideoFormat};
use anyhow::{anyhow, Context, Result};
use std::ffi::{c_void, CString};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
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
    pub queue_drops: Arc<AtomicU64>,
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

pub(crate) fn renderable_pixel_format(
    format: ffmpeg_next::format::Pixel,
) -> ffmpeg_next::format::Pixel {
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

pub(crate) fn copy_frame_tightly(frame: &ffmpeg_next::frame::Video) -> Result<Vec<u8>> {
    let planes = frame.planes();
    let mut total_bytes = 0usize;
    for plane in 0..planes {
        let row_bytes = plane_tight_row_bytes(frame, plane)?;
        let row_count = frame.plane_height(plane) as usize;
        total_bytes += row_bytes * row_count;
    }

    let mut data = Vec::with_capacity(total_bytes);
    for plane in 0..planes {
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

pub(crate) fn normalize_v4l2_input_format(fourcc: &str) -> String {
    let lower = fourcc.trim_end_matches('\0').to_lowercase();
    match lower.as_str() {
        "yuyv" => "yuyv422".to_string(),
        "mjpg" => "mjpeg".to_string(),
        _ => lower,
    }
}

pub(crate) fn setup_v4l2_ffmpeg_options(
    format: &VideoFormat,
    resolution: (u32, u32),
    framerate: u32,
    include_capture_pixel_and_range_defaults: bool,
) -> (String, ffmpeg_next::Dictionary<'static>) {
    let pixel_format_str = normalize_v4l2_input_format(&format.fourcc);
    let mut ffmpeg_options = ffmpeg_next::Dictionary::new();
    if resolution.0 > 0 && resolution.1 > 0 {
        ffmpeg_options.set("video_size", &format!("{}x{}", resolution.0, resolution.1));
    }
    if framerate > 0 {
        ffmpeg_options.set("framerate", &framerate.to_string());
    }
    if !pixel_format_str.is_empty() && pixel_format_str != "unkn" {
        ffmpeg_options.set("input_format", &pixel_format_str);
    }
    // Do NOT set `nobuffer` in `fflags`: if `avformat_find_stream_info` runs, `nobuffer`
    // discards the initial probed packet (which on hardware H.264 V4L2 devices contains
    // the only IDR keyframe + SPS/PPS for an entire GOP).
    ffmpeg_options.set("fflags", "discardcorrupt");
    ffmpeg_options.set("probesize", "32");
    if include_capture_pixel_and_range_defaults && pixel_format_str != "h264" {
        ffmpeg_options.set("analyzeduration", "100000");
    } else {
        ffmpeg_options.set("analyzeduration", "0");
    }
    if include_capture_pixel_and_range_defaults {
        ffmpeg_options.set("pixel_format", &pixel_format_str);
        ffmpeg_options.set("color_range", "pc");
    }
    (pixel_format_str, ffmpeg_options)
}

fn setup_ffmpeg_options(
    format: &VideoFormat,
    resolution: (u32, u32),
    framerate: u32,
) -> (String, ffmpeg_next::Dictionary<'_>) {
    setup_v4l2_ffmpeg_options(format, resolution, framerate, true)
}

pub(crate) unsafe fn v4l2_context_needs_stream_info(
    context: *const ffmpeg_next::sys::AVFormatContext,
) -> bool {
    if context.is_null() {
        return true;
    }
    let nb_streams = unsafe { (*context).nb_streams as usize };
    let streams_ptr = unsafe { (*context).streams };
    if nb_streams == 0 || streams_ptr.is_null() {
        return true;
    }
    let streams = unsafe { std::slice::from_raw_parts(streams_ptr, nb_streams) };
    !streams.iter().any(|&st| {
        if st.is_null() {
            return false;
        }
        let codecpar = unsafe { (*st).codecpar };
        if codecpar.is_null() {
            return false;
        }
        let codec_type = unsafe { (*codecpar).codec_type };
        let codec_id = unsafe { (*codecpar).codec_id };
        let width = unsafe { (*codecpar).width };
        let height = unsafe { (*codecpar).height };
        let format = unsafe { (*codecpar).format };
        let has_pixel_format =
            format != ffmpeg_next::sys::AVPixelFormat::AV_PIX_FMT_NONE as libc::c_int;
        codec_type == ffmpeg_next::sys::AVMediaType::AVMEDIA_TYPE_VIDEO
            && codec_id != ffmpeg_next::sys::AVCodecID::AV_CODEC_ID_NONE
            && width > 0
            && height > 0
            && (codec_id == ffmpeg_next::sys::AVCodecID::AV_CODEC_ID_H264 || has_pixel_format)
    })
}

pub(crate) unsafe fn probe_v4l2_stream_info_if_needed(
    context: *mut ffmpeg_next::sys::AVFormatContext,
) -> std::result::Result<bool, ffmpeg_next::Error> {
    if unsafe { v4l2_context_needs_stream_info(context) } {
        let stream_info_result =
            unsafe { ffmpeg_next::sys::avformat_find_stream_info(context, std::ptr::null_mut()) };
        if stream_info_result < 0 {
            return Err(ffmpeg_next::Error::from(stream_info_result));
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

pub(crate) fn normalize_decoded_video_frame(
    decoded: &ffmpeg_next::frame::Video,
    normalized: &mut ffmpeg_next::frame::Video,
    scaler: &mut Option<ffmpeg_next::software::scaling::context::Context>,
    active_scaler_filter: &mut u8,
    desired_scaler_filter: u8,
) -> Result<Option<(u32, u32, ffmpeg_next::format::Pixel, Vec<u8>)>> {
    if decoded.width() == 0
        || decoded.height() == 0
        || decoded.format() == ffmpeg_next::format::Pixel::None
    {
        return Ok(None);
    }

    let output_format = renderable_pixel_format(decoded.format());
    if decoded.format() == output_format {
        let data = copy_frame_tightly(decoded)?;
        return Ok(Some((
            decoded.width(),
            decoded.height(),
            decoded.format(),
            data,
        )));
    }

    let filter_changed = *active_scaler_filter != desired_scaler_filter;
    let needs_reinit = filter_changed
        || scaler.as_ref().is_none_or(|s| {
            s.input().format != decoded.format()
                || s.input().width != decoded.width()
                || s.input().height != decoded.height()
                || s.output().format != output_format
                || s.output().width != decoded.width()
                || s.output().height != decoded.height()
        });

    if needs_reinit {
        let new_scaler = ffmpeg_next::software::scaling::context::Context::get(
            decoded.format(),
            decoded.width(),
            decoded.height(),
            output_format,
            decoded.width(),
            decoded.height(),
            crate::video::types::ScalerFilter::from_u8(desired_scaler_filter).into_ffmpeg_flag(),
        )
        .context("Failed to create software scaler for normalization")?;
        *scaler = Some(new_scaler);
        *active_scaler_filter = desired_scaler_filter;
    }

    let active_scaler = scaler
        .as_mut()
        .expect("scaler initialized before frame normalization");
    active_scaler
        .run(decoded, normalized)
        .context("Failed to normalize video frame")?;
    let data = copy_frame_tightly(normalized)?;
    Ok(Some((
        normalized.width(),
        normalized.height(),
        normalized.format(),
        data,
    )))
}

fn open_video_input(
    device: &str,
    options: ffmpeg_next::Dictionary<'_>,
    interrupt_state: &mut VideoInterruptState,
) -> Result<ffmpeg_next::format::context::Input> {
    use ffmpeg_next::sys::{
        av_find_input_format, avformat_alloc_context, avformat_close_input, avformat_open_input,
        AVIOInterruptCB,
    };

    let is_v4l2_device = device.starts_with("/dev/video");
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

        let input_fmt = if is_v4l2_device {
            av_find_input_format(c"v4l2".as_ptr())
        } else {
            std::ptr::null_mut()
        };
        let mut options_ptr = options.disown();
        let open_result =
            avformat_open_input(&mut context, device.as_ptr(), input_fmt, &mut options_ptr);
        drop(ffmpeg_next::Dictionary::own(options_ptr));

        if open_result < 0 {
            avformat_close_input(&mut context);
            return Err(ffmpeg_next::Error::from(open_result))
                .context("FFmpeg could not open the video input");
        }

        if let Err(err) = probe_v4l2_stream_info_if_needed(context) {
            avformat_close_input(&mut context);
            return Err(err).context("FFmpeg could not read video stream information");
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
        queue_drops,
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
            &queue_drops,
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
    queue_drops: &AtomicU64,
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
    let input_time_base = input.time_base();
    let negotiated_rate = input.avg_frame_rate();
    let negotiated_rate = if negotiated_rate.numerator() > 0 && negotiated_rate.denominator() > 0 {
        negotiated_rate
    } else {
        input.rate()
    };
    let replay_rate = if negotiated_rate.numerator() > 0 && negotiated_rate.denominator() > 0 {
        crate::replay::config::Rate::new(
            negotiated_rate.numerator() as u32,
            negotiated_rate.denominator() as u32,
        )
    } else {
        crate::replay::config::Rate::new(framerate, 1)
    };

    let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(input.parameters())
        .and_then(|c| c.decoder().video())
        .context("Failed to create software video decoder")?;

    decoder.set_threading(ffmpeg_next::codec::threading::Config::default());
    let mut current_scaler_val = scaler_filter.load(Ordering::Relaxed);
    let mut scaler: Option<ffmpeg_next::software::scaling::context::Context> = None;
    let mut decoded = ffmpeg_next::frame::Video::empty();
    let mut normalized = ffmpeg_next::frame::Video::empty();
    loop {
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
        while decoder.receive_frame(&mut decoded).is_ok() {
            let desired_scaler_val = scaler_filter.load(Ordering::Relaxed);
            let Some((width, height, format, data)) = normalize_decoded_video_frame(
                &decoded,
                &mut normalized,
                &mut scaler,
                &mut current_scaler_val,
                desired_scaler_val,
            )?
            else {
                continue;
            };
            let range_val = color_range.load(Ordering::Relaxed);

            let now = crate::replay::now_us();
            let timestamp = decoded
                .timestamp()
                .map(|pts| unsafe {
                    ffmpeg_next::sys::av_rescale_q(
                        pts,
                        input_time_base.into(),
                        ffmpeg_next::Rational(1, 1_000_000).into(),
                    )
                })
                .filter(|pts| now.abs_diff(*pts) < 10_000_000)
                .unwrap_or(now);
            let raw_frame = Arc::new(RawFrame {
                captured_at: timestamp,
                rate: replay_rate,
                width,
                height,
                data,
                format,
                color_range: crate::video::types::ColorRange::from_u8(range_val),
            });

            frame_progress.mark_frame();

            match frame_sender.try_send(raw_frame) {
                Ok(()) => request_repaint(),
                Err(crossbeam_channel::TrySendError::Full(_)) => {
                    queue_drops.fetch_add(1, Ordering::Relaxed);
                }
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

    #[test]
    fn test_setup_ffmpeg_options_omits_nobuffer_for_h264_capture() {
        ffmpeg_next::init().unwrap();
        let format = VideoFormat {
            fourcc: "H264".to_string(),
            description: "H.264".to_string(),
            resolutions: vec![],
        };
        let (pix_fmt, options) = setup_ffmpeg_options(&format, (1920, 1080), 60);
        assert_eq!(pix_fmt, "h264");
        let fflags = options.get("fflags").unwrap_or("");
        assert!(
            !fflags.contains("nobuffer"),
            "fflags ({fflags}) must not include nobuffer so initial H.264 IDR keyframes are preserved"
        );
        assert_eq!(options.get("analyzeduration"), Some("0"));
    }

    #[test]
    fn test_v4l2_context_needs_stream_info_skips_when_header_populated_codec_and_dimensions() {
        ffmpeg_next::init().unwrap();
        unsafe {
            assert!(v4l2_context_needs_stream_info(std::ptr::null()));

            let ctx = ffmpeg_next::sys::avformat_alloc_context();
            assert!(!ctx.is_null());
            assert!(
                v4l2_context_needs_stream_info(ctx),
                "empty context without streams must require stream info"
            );

            let stream = ffmpeg_next::sys::avformat_new_stream(ctx, std::ptr::null());
            assert!(!stream.is_null());
            (*(*stream).codecpar).codec_type = ffmpeg_next::sys::AVMediaType::AVMEDIA_TYPE_VIDEO;
            (*(*stream).codecpar).codec_id = ffmpeg_next::sys::AVCodecID::AV_CODEC_ID_NONE;
            (*(*stream).codecpar).width = 1920;
            (*(*stream).codecpar).height = 1080;
            assert!(
                v4l2_context_needs_stream_info(ctx),
                "unknown codec_id must still probe stream info"
            );

            // Raw video (e.g. YUYV) with AV_PIX_FMT_NONE must still run avformat_find_stream_info
            // so FFmpeg populates codecpar->format before opening rawdec.
            (*(*stream).codecpar).codec_id = ffmpeg_next::sys::AVCodecID::AV_CODEC_ID_RAWVIDEO;
            (*(*stream).codecpar).format =
                ffmpeg_next::sys::AVPixelFormat::AV_PIX_FMT_NONE as libc::c_int;
            assert!(
                v4l2_context_needs_stream_info(ctx),
                "rawvideo with AV_PIX_FMT_NONE must probe stream info"
            );

            // Simulate v4l2_read_header on H.264 capture: codec_id and dimensions are known,
            // while pixel format is still AV_PIX_FMT_NONE (-1).
            (*(*stream).codecpar).codec_id = ffmpeg_next::sys::AVCodecID::AV_CODEC_ID_H264;
            (*(*stream).codecpar).format =
                ffmpeg_next::sys::AVPixelFormat::AV_PIX_FMT_NONE as libc::c_int;
            assert!(
                !v4l2_context_needs_stream_info(ctx),
                "known H.264 codec_id + dimensions should skip avformat_find_stream_info"
            );

            ffmpeg_next::sys::avformat_free_context(ctx);
        }
    }

    #[test]
    fn test_lazy_scaler_decodes_first_keyframe_when_initial_decoder_format_is_none() {
        ffmpeg_next::init().unwrap();
        let codec = ffmpeg_next::encoder::find(ffmpeg_next::codec::Id::H264)
            .or_else(|| ffmpeg_next::encoder::find(ffmpeg_next::codec::Id::MPEG4))
            .expect("H264 or MPEG4 encoder must be available");

        let mut enc_ctx = ffmpeg_next::codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .unwrap();
        enc_ctx.set_width(64);
        enc_ctx.set_height(64);
        enc_ctx.set_format(ffmpeg_next::format::Pixel::YUV420P);
        enc_ctx.set_time_base(ffmpeg_next::Rational(1, 30));
        enc_ctx.set_gop(300);
        enc_ctx.set_max_b_frames(0);
        let mut encoder = enc_ctx.open_as(codec).unwrap();

        let mut packets = Vec::new();
        for i in 0..4i64 {
            let mut f = ffmpeg_next::frame::Video::new(ffmpeg_next::format::Pixel::YUV420P, 64, 64);
            f.set_pts(Some(i));
            f.data_mut(0).fill(64 + i as u8 * 20);
            f.data_mut(1).fill(128);
            f.data_mut(2).fill(128);
            encoder.send_frame(&f).unwrap();
            let mut pkt = ffmpeg_next::Packet::empty();
            while encoder.receive_packet(&mut pkt).is_ok() {
                packets.push(pkt.clone());
            }
        }
        encoder.send_eof().unwrap();
        let mut pkt = ffmpeg_next::Packet::empty();
        while encoder.receive_packet(&mut pkt).is_ok() {
            packets.push(pkt.clone());
        }
        assert!(packets.len() >= 2);

        // Simulate V4L2 compressed stream parameters before any probing decode pass:
        // codec_id, width, height are set, but pixel format is AV_PIX_FMT_NONE.
        let mut params = ffmpeg_next::codec::Parameters::new();
        unsafe {
            let raw = params.as_mut_ptr();
            (*raw).codec_type = ffmpeg_next::sys::AVMediaType::AVMEDIA_TYPE_VIDEO;
            (*raw).codec_id = codec.id().into();
            (*raw).width = 64;
            (*raw).height = 64;
            (*raw).format = ffmpeg_next::sys::AVPixelFormat::AV_PIX_FMT_NONE as libc::c_int;
        }

        let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(params)
            .unwrap()
            .decoder()
            .video()
            .unwrap();
        assert_eq!(decoder.format(), ffmpeg_next::format::Pixel::None);

        let mut scaler = None;
        let mut active_filter = 0u8;
        let mut decoded = ffmpeg_next::frame::Video::empty();
        let mut normalized = ffmpeg_next::frame::Video::empty();
        let mut out_frames = 0usize;

        for packet in &packets {
            decoder.send_packet(packet).unwrap();
            while decoder.receive_frame(&mut decoded).is_ok() {
                if let Some((w, h, fmt, data)) = normalize_decoded_video_frame(
                    &decoded,
                    &mut normalized,
                    &mut scaler,
                    &mut active_filter,
                    0,
                )
                .unwrap()
                {
                    assert_eq!((w, h), (64, 64));
                    assert_eq!(fmt, ffmpeg_next::format::Pixel::YUV420P);
                    assert!(!data.is_empty());
                    out_frames += 1;
                }
            }
        }
        decoder.send_eof().unwrap();
        while decoder.receive_frame(&mut decoded).is_ok() {
            if normalize_decoded_video_frame(
                &decoded,
                &mut normalized,
                &mut scaler,
                &mut active_filter,
                0,
            )
            .unwrap()
            .is_some()
            {
                out_frames += 1;
            }
        }
        assert_eq!(
            out_frames, 4,
            "all 4 frames including initial IDR keyframe must decode"
        );

        // Also verify lazy scaler initialization when decoded format requires swscale conversion (e.g. YUV444P -> YUV420P).
        let mut yuv444_frame =
            ffmpeg_next::frame::Video::new(ffmpeg_next::format::Pixel::YUV444P, 64, 64);
        yuv444_frame.data_mut(0).fill(100);
        yuv444_frame.data_mut(1).fill(128);
        yuv444_frame.data_mut(2).fill(128);
        let (w, h, fmt, data) = normalize_decoded_video_frame(
            &yuv444_frame,
            &mut normalized,
            &mut scaler,
            &mut active_filter,
            0,
        )
        .unwrap()
        .expect("YUV444P frame should be scaled to YUV420P");
        assert_eq!((w, h), (64, 64));
        assert_eq!(fmt, ffmpeg_next::format::Pixel::YUV420P);
        assert_eq!(data.len(), 64 * 64 * 3 / 2);
        assert!(scaler.is_some());
    }
}
