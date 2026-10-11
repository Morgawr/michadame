use crate::video::types::VideoFormat;
use anyhow::{anyhow, Context, Result};
use std::ffi::{c_void, CString};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const WEBCAM_OPEN_ATTEMPTS: u8 = 3;
const WEBCAM_OPEN_RETRY_DELAY: Duration = Duration::from_millis(250);
const WEBCAM_STALL_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub struct WebcamFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct WebcamStreamHandle {
    pub device: String,
    pub format_fourcc: String,
    pub resolution: (u32, u32),
    pub framerate: u32,
    pub stop_requested: Arc<AtomicBool>,
    pub thread_handle: Option<thread::JoinHandle<()>>,
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
    last_progress: Mutex<Instant>,
}

impl WebcamInterruptState {
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
            .map(|last_progress| last_progress.elapsed() >= WEBCAM_STALL_TIMEOUT)
            .unwrap_or(true)
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
    let mut pixel_format_str = format.fourcc.trim_end_matches('\0').to_lowercase();
    if pixel_format_str == "yuyv" {
        pixel_format_str = "yuyv422".to_string();
    } else if pixel_format_str == "mjpg" {
        pixel_format_str = "mjpeg".to_string();
    }
    let mut options = ffmpeg_next::Dictionary::new();
    if resolution.0 > 0 && resolution.1 > 0 {
        options.set("video_size", &format!("{}x{}", resolution.0, resolution.1));
    }
    if framerate > 0 {
        options.set("framerate", &framerate.to_string());
    }
    if !pixel_format_str.is_empty() && pixel_format_str != "unkn" {
        options.set("input_format", &pixel_format_str);
    }
    options.set("fflags", "nobuffer+discardcorrupt");
    options.set("probesize", "32");
    options.set("analyzeduration", "100000");
    options
}

fn open_webcam_input(
    device: &str,
    options: ffmpeg_next::Dictionary<'_>,
    interrupt_state: &mut WebcamInterruptState,
) -> Result<ffmpeg_next::format::context::Input> {
    use ffmpeg_next::sys::{
        avformat_alloc_context, avformat_close_input, avformat_find_stream_info,
        avformat_open_input, AVIOInterruptCB,
    };

    let device_c = CString::new(device).context("Webcam device path contains a NUL byte")?;

    unsafe {
        let mut context = avformat_alloc_context();
        if context.is_null() {
            return Err(anyhow!("FFmpeg could not allocate input context for webcam"));
        }

        (*context).interrupt_callback = AVIOInterruptCB {
            callback: Some(webcam_interrupt_callback),
            opaque: interrupt_state as *mut WebcamInterruptState as *mut c_void,
        };

        let mut options_ptr = options.disown();
        let open_result = avformat_open_input(
            &mut context,
            device_c.as_ptr(),
            std::ptr::null_mut(),
            &mut options_ptr,
        );
        drop(ffmpeg_next::Dictionary::own(options_ptr));

        if open_result < 0 {
            avformat_close_input(&mut context);
            return Err(ffmpeg_next::Error::from(open_result))
                .context("FFmpeg could not open webcam input");
        }

        let stream_info_result = avformat_find_stream_info(context, std::ptr::null_mut());
        if stream_info_result < 0 {
            avformat_close_input(&mut context);
            return Err(ffmpeg_next::Error::from(stream_info_result))
                .context("FFmpeg could not read webcam stream info");
        }

        Ok(ffmpeg_next::format::context::Input::wrap(context))
    }
}

pub fn start_webcam_stream(
    device: &str,
    format: VideoFormat,
    resolution: (u32, u32),
    framerate: u32,
    frame_sender: crossbeam_channel::Sender<Arc<WebcamFrame>>,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
) -> Result<WebcamStreamHandle> {
    let stop_requested = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop_requested);
    let device_str = device.to_string();
    let device_clone = device_str.clone();

    let format_fourcc = format.fourcc.clone();
    let thread_handle = thread::Builder::new()
        .name("webcam-capture".to_string())
        .spawn(move || {
            let _ = run_webcam_worker(
                &device_clone,
                &format,
                resolution,
                framerate,
                &frame_sender,
                &request_repaint,
                &stop_clone,
            );
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

fn run_webcam_worker(
    device: &str,
    format: &VideoFormat,
    resolution: (u32, u32),
    framerate: u32,
    frame_sender: &crossbeam_channel::Sender<Arc<WebcamFrame>>,
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
                tracing::warn!("Webcam open attempt {} failed: {}; retrying...", open_attempt, e);
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

    let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(input_stream.parameters())
        .and_then(|c| c.decoder().video())
        .context("Failed to create webcam video decoder")?;

    let mut scaler = ffmpeg_next::software::scaling::context::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        ffmpeg_next::format::Pixel::RGBA,
        decoder.width(),
        decoder.height(),
        ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR,
    )
    .context("Failed to create scaler for webcam RGBA conversion")?;

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

        if packet.stream() != video_stream_index {
            continue;
        }

        if decoder.send_packet(&packet).is_ok() {
            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                if stop_requested.load(Ordering::Relaxed) {
                    return Ok(());
                }

                // If dimensions changed mid-stream, re-create the scaler
                if decoded_frame.width() != scaler.input().width
                    || decoded_frame.height() != scaler.input().height
                    || decoded_frame.format() != scaler.input().format
                {
                    if let Ok(new_scaler) = ffmpeg_next::software::scaling::context::Context::get(
                        decoded_frame.format(),
                        decoded_frame.width(),
                        decoded_frame.height(),
                        ffmpeg_next::format::Pixel::RGBA,
                        decoded_frame.width(),
                        decoded_frame.height(),
                        ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR,
                    ) {
                        scaler = new_scaler;
                    }
                }

                if scaler.run(&decoded_frame, &mut rgba_frame).is_ok() {
                    let w = rgba_frame.width();
                    let h = rgba_frame.height();
                    let stride = rgba_frame.stride(0);
                    let row_bytes = (w * 4) as usize;
                    let plane_data = rgba_frame.data(0);

                    let mut tight_rgba = Vec::with_capacity(row_bytes * h as usize);
                    for y in 0..h as usize {
                        let start = y * stride;
                        let end = start + row_bytes;
                        if let Some(row) = plane_data.get(start..end) {
                            tight_rgba.extend_from_slice(row);
                        }
                    }

                    let frame = Arc::new(WebcamFrame {
                        width: w,
                        height: h,
                        rgba: tight_rgba,
                    });

                    // Drop oldest if UI thread hasn't picked it up yet
                    let _ = frame_sender.try_send(frame);
                    request_repaint();
                }
            }
        }
    }

    Ok(())
}
