//! Twitch live broadcast client: real-time video/audio encoding and RTMP(S) streaming.

use crate::config::models::{StreamingAudioEncoder, StreamingVideoEncoder};
use crate::replay::{
    AudioBlock, AudioClock, AudioRoute, MixSource, TimelineMixer, VideoFrame,
    AUDIO_BLOCKS, AUDIO_SAMPLES,
};
use anyhow::{bail, ensure, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use ff::{codec, encoder, format, frame, sys as ffi, Packet, Rational};
use ffmpeg_next as ff;
use std::ffi::CString;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn check(code: i32) -> Result<()> {
    if code < 0 {
        bail!("{}", ff::Error::from(code));
    }
    Ok(())
}

struct BufferRef(*mut ffi::AVBufferRef);
impl Drop for BufferRef {
    fn drop(&mut self) {
        unsafe {
            ffi::av_buffer_unref(&mut self.0);
        }
    }
}

/// Clamps rendering dimensions to a max 1080p broadcast canvas (1920x1080 max),
/// preserving aspect ratio and ensuring dimensions are even numbers.
pub fn clamp_broadcast_resolution(width: u32, height: u32) -> (u32, u32) {
    let max_w = 1920.0f32;
    let max_h = 1080.0f32;
    let w = width.max(2) as f32;
    let h = height.max(2) as f32;
    let scale = (max_w / w).min(max_h / h).min(1.0);
    let target_w = ((w * scale).round() as u32).max(2) & !1;
    let target_h = ((h * scale).round() as u32).max(2) & !1;
    (target_w, target_h)
}

#[derive(Clone, Debug)]
pub struct BroadcastConfig {
    pub video_bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
    pub video_encoder: StreamingVideoEncoder,
    pub encoder_preset: String,
    #[allow(dead_code)]
    pub audio_encoder: StreamingAudioEncoder,
    pub ingest_server: String,
    pub stream_key: String,
    pub render_device: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BroadcastStatus {
    Offline,
    Connecting,
    Live { started_at: Instant },
    Error(String),
}

#[derive(Default)]
pub struct BroadcastMetrics {
    pub current_fps: AtomicU32,       // scaled x10 (e.g. 600 = 60.0 fps)
    pub current_bitrate_kbps: AtomicU32,
    pub video_dropped: Arc<AtomicU64>,
    pub audio_dropped: Arc<AtomicU64>,
    pub frames_sent: AtomicU64,
    pub bytes_sent: AtomicU64,
}

struct RgbaToYuv420pConverter(*mut ffi::SwsContext);
impl RgbaToYuv420pConverter {
    fn new(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Result<Self> {
        unsafe {
            let context = Self(ffi::sws_alloc_context());
            ensure!(!context.0.is_null(), "Cannot allocate broadcast converter");
            (*context.0).src_w = src_w as i32;
            (*context.0).src_h = src_h as i32;
            (*context.0).dst_w = dst_w as i32;
            (*context.0).dst_h = dst_h as i32;
            (*context.0).src_format = ffi::AVPixelFormat::AV_PIX_FMT_RGBA as i32;
            (*context.0).dst_format = ffi::AVPixelFormat::AV_PIX_FMT_YUV420P as i32;
            (*context.0).flags = ff::software::scaling::flag::Flags::BILINEAR.bits() as u32;
            let threads = (std::thread::available_parallelism().map_or(1, |n| n.get()) / 2).clamp(1, 4);
            (*context.0).threads = threads as i32;
            check(ffi::sws_init_context(context.0, ptr::null_mut(), ptr::null_mut()))?;
            let coeff = ffi::sws_getCoefficients(ffi::SWS_CS_ITU709);
            check(ffi::sws_setColorspaceDetails(
                context.0, coeff, 1, coeff, 0, 0, 1 << 16, 1 << 16,
            ))?;
            Ok(context)
        }
    }
}
impl Drop for RgbaToYuv420pConverter {
    fn drop(&mut self) {
        unsafe {
            ffi::sws_freeContext(self.0);
        }
    }
}

enum LiveVideoEncoder {
    Software {
        encoder: encoder::Video,
        scaler: RgbaToYuv420pConverter,
        yuv_frame: frame::Video,
        src_w: u32,
        src_h: u32,
        dst_w: u32,
        dst_h: u32,
    },
    Hardware {
        encoder: encoder::Video,
        frames: BufferRef,
        _device: BufferRef,
        sw: frame::Video,
        scaler: RgbaToNv12Converter,
        src_w: u32,
        src_h: u32,
        dst_w: u32,
        dst_h: u32,
    },
}

struct RgbaToNv12Converter(*mut ffi::SwsContext);
impl RgbaToNv12Converter {
    fn new(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Result<Self> {
        unsafe {
            let context = Self(ffi::sws_alloc_context());
            ensure!(!context.0.is_null(), "Cannot allocate broadcast NV12 converter");
            (*context.0).src_w = src_w as i32;
            (*context.0).src_h = src_h as i32;
            (*context.0).dst_w = dst_w as i32;
            (*context.0).dst_h = dst_h as i32;
            (*context.0).src_format = ffi::AVPixelFormat::AV_PIX_FMT_RGBA as i32;
            (*context.0).dst_format = ffi::AVPixelFormat::AV_PIX_FMT_NV12 as i32;
            (*context.0).flags = ff::software::scaling::flag::Flags::BILINEAR.bits() as u32;
            let threads = (std::thread::available_parallelism().map_or(1, |n| n.get()) / 2).clamp(1, 4);
            (*context.0).threads = threads as i32;
            check(ffi::sws_init_context(context.0, ptr::null_mut(), ptr::null_mut()))?;
            let coeff = ffi::sws_getCoefficients(ffi::SWS_CS_ITU709);
            check(ffi::sws_setColorspaceDetails(
                context.0, coeff, 1, coeff, 0, 0, 1 << 16, 1 << 16,
            ))?;
            Ok(context)
        }
    }
}
impl Drop for RgbaToNv12Converter {
    fn drop(&mut self) {
        unsafe {
            ffi::sws_freeContext(self.0);
        }
    }
}

impl LiveVideoEncoder {
    fn new(config: &BroadcastConfig, src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Result<Self> {
        ff::init()?;
        match config.video_encoder {
            StreamingVideoEncoder::SoftwareX264 => {
                let codec = encoder::find_by_name("libx264")
                    .context("libx264 encoder is unavailable in FFmpeg")?;
                let mut enc = codec::context::Context::new_with_codec(codec)
                    .encoder()
                    .video()?;
                enc.set_width(dst_w);
                enc.set_height(dst_h);
                enc.set_format(format::Pixel::YUV420P);
                enc.set_time_base((1, 1000));
                enc.set_frame_rate(Some(Rational(60, 1)));
                enc.set_gop(120); // 2s GOP at 60 fps
                enc.set_max_b_frames(0);
                enc.set_flags(codec::Flags::GLOBAL_HEADER);

                let mut options = ff::Dictionary::new();
                options.set("preset", &config.encoder_preset);
                options.set("tune", "zerolatency");
                options.set("profile", "high");
                let bps = (config.video_bitrate_kbps as u64 * 1000).to_string();
                options.set("b", &bps);
                options.set("maxrate", &bps);
                options.set("bufsize", &bps);
                options.set("g", "120");
                options.set("keyint_min", "60");

                let opened = enc.open_with(options).context("Failed to open libx264 encoder")?;
                let scaler = RgbaToYuv420pConverter::new(src_w, src_h, dst_w, dst_h)?;
                let yuv_frame = frame::Video::new(format::Pixel::YUV420P, dst_w, dst_h);
                Ok(Self::Software {
                    encoder: opened,
                    scaler,
                    yuv_frame,
                    src_w,
                    src_h,
                    dst_w,
                    dst_h,
                })
            }
            StreamingVideoEncoder::HardwareVaapi => {
                let codec = encoder::find_by_name("h264_vaapi")
                    .context("h264_vaapi encoder is unavailable in FFmpeg")?;
                let mut enc = codec::context::Context::new_with_codec(codec)
                    .encoder()
                    .video()?;
                enc.set_width(dst_w);
                enc.set_height(dst_h);
                enc.set_format(format::Pixel::VAAPI);
                enc.set_time_base((1, 1000));
                enc.set_frame_rate(Some(Rational(60, 1)));
                enc.set_gop(120);
                enc.set_max_b_frames(0);
                enc.set_flags(codec::Flags::GLOBAL_HEADER);

                let mut device = BufferRef(ptr::null_mut());
                let dev_name = CString::new(config.render_device.as_str())?;
                let frames;
                unsafe {
                    check(ffi::av_hwdevice_ctx_create(
                        &mut device.0,
                        ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                        dev_name.as_ptr(),
                        ptr::null_mut(),
                        0,
                    ))
                    .context("Cannot initialize VAAPI hardware device for broadcast")?;
                    frames = BufferRef(ffi::av_hwframe_ctx_alloc(device.0));
                    ensure!(!frames.0.is_null(), "Cannot allocate broadcast VAAPI frame pool");
                    let pool = (*frames.0).data as *mut ffi::AVHWFramesContext;
                    (*pool).format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
                    (*pool).sw_format = ffi::AVPixelFormat::AV_PIX_FMT_NV12;
                    (*pool).width = dst_w as i32;
                    (*pool).height = dst_h as i32;
                    (*pool).initial_pool_size = 4;
                    check(ffi::av_hwframe_ctx_init(frames.0))?;
                    (*enc.as_mut_ptr()).hw_frames_ctx = ffi::av_buffer_ref(frames.0);
                    ensure!(!(*enc.as_ptr()).hw_frames_ctx.is_null(), "Cannot retain VAAPI pool");
                }

                let mut options = ff::Dictionary::new();
                options.set("async_depth", "1");
                options.set("rc_mode", "CBR");
                let bps = (config.video_bitrate_kbps as u64 * 1000).to_string();
                options.set("b", &bps);
                options.set("maxrate", &bps);
                options.set("bufsize", &bps);

                let opened = enc.open_with(options).context("Hardware VAAPI encoder rejected settings")?;
                let scaler = RgbaToNv12Converter::new(src_w, src_h, dst_w, dst_h)?;
                let sw = frame::Video::new(format::Pixel::NV12, dst_w, dst_h);
                Ok(Self::Hardware {
                    encoder: opened,
                    frames,
                    _device: device,
                    sw,
                    scaler,
                    src_w,
                    src_h,
                    dst_w,
                    dst_h,
                })
            }
        }
    }

    fn parameters(&self) -> codec::Parameters {
        match self {
            Self::Software { encoder, .. } => codec::Parameters::from(encoder),
            Self::Hardware { encoder, .. } => codec::Parameters::from(encoder),
        }
    }

    fn encode(&mut self, rgba: &[u8], src_w: u32, src_h: u32, pts: i64) -> Result<Vec<Packet>> {
        match self {
            Self::Software {
                encoder,
                scaler,
                yuv_frame,
                src_w: cur_w,
                src_h: cur_h,
                dst_w,
                dst_h,
            } => {
                if *cur_w != src_w || *cur_h != src_h {
                    *scaler = RgbaToYuv420pConverter::new(src_w, src_h, *dst_w, *dst_h)?;
                    *cur_w = src_w;
                    *cur_h = src_h;
                }
                unsafe {
                    check(ffi::av_frame_make_writable(yuv_frame.as_mut_ptr()))?;
                    convert_rgba_to_frame(scaler.0, yuv_frame, rgba, src_w, src_h)?;
                }
                yuv_frame.set_pts(Some(pts));
                encoder.send_frame(yuv_frame)?;
                let mut packets = Vec::new();
                loop {
                    let mut packet = Packet::empty();
                    match encoder.receive_packet(&mut packet) {
                        Ok(()) => packets.push(packet),
                        Err(ff::Error::Other { errno }) if errno == libc::EAGAIN => break,
                        Err(ff::Error::Eof) => break,
                        Err(e) => return Err(e.into()),
                    }
                }
                Ok(packets)
            }
            Self::Hardware {
                encoder,
                frames,
                sw,
                scaler,
                src_w: cur_w,
                src_h: cur_h,
                dst_w,
                dst_h,
                ..
            } => {
                if *cur_w != src_w || *cur_h != src_h {
                    *scaler = RgbaToNv12Converter::new(src_w, src_h, *dst_w, *dst_h)?;
                    *cur_w = src_w;
                    *cur_h = src_h;
                }
                unsafe {
                    check(ffi::av_frame_make_writable(sw.as_mut_ptr()))?;
                    convert_rgba_to_frame(scaler.0, sw, rgba, src_w, src_h)?;
                }
                let mut hardware = frame::Video::empty();
                unsafe {
                    check(ffi::av_hwframe_get_buffer(frames.0, hardware.as_mut_ptr(), 0))?;
                    check(ffi::av_hwframe_transfer_data(hardware.as_mut_ptr(), sw.as_ptr(), 0))?;
                }
                hardware.set_pts(Some(pts));
                encoder.send_frame(&hardware)?;
                let mut packets = Vec::new();
                loop {
                    let mut packet = Packet::empty();
                    match encoder.receive_packet(&mut packet) {
                        Ok(()) => packets.push(packet),
                        Err(ff::Error::Other { errno }) if errno == libc::EAGAIN => break,
                        Err(ff::Error::Eof) => break,
                        Err(e) => return Err(e.into()),
                    }
                }
                Ok(packets)
            }
        }
    }
}

unsafe fn convert_rgba_to_frame(
    sws: *mut ffi::SwsContext,
    dst: &mut frame::Video,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<()> {
    unsafe extern "C" fn borrowed_buffer(_: *mut libc::c_void, _: *mut u8) {}
    let mut input = frame::Video::empty();
    input.set_format(format::Pixel::RGBA);
    input.set_width(width);
    input.set_height(height);
    let source = input.as_mut_ptr();
    (*source).buf[0] = ffi::av_buffer_create(
        rgba.as_ptr().cast_mut(),
        rgba.len(),
        Some(borrowed_buffer),
        ptr::null_mut(),
        ffi::AV_BUFFER_FLAG_READONLY,
    );
    ensure!(!(*source).buf[0].is_null(), "Cannot reference RGBA buffer");
    // Flip bottom-up OpenGL via negative stride
    (*source).data[0] = rgba
        .as_ptr()
        .add((height as usize - 1) * width as usize * 4)
        .cast_mut();
    (*source).linesize[0] = -(width as i32 * 4);

    let result = ffi::sws_scale_frame(sws, dst.as_mut_ptr(), source);
    ffi::sws_frame_end(sws);
    check(result).context("SWS RGB scaling failed")
}

struct LiveAudioEncoder {
    encoder: encoder::Audio,
}

impl LiveAudioEncoder {
    fn new(bitrate_kbps: u32) -> Result<Self> {
        let codec = encoder::find_by_name("aac").context("FFmpeg aac encoder is unavailable")?;
        let mut enc = codec::context::Context::new_with_codec(codec)
            .encoder()
            .audio()?;
        enc.set_rate(48000);
        enc.set_channel_layout(ff::ChannelLayout::STEREO);
        enc.set_format(format::Sample::F32(format::sample::Type::Planar));
        enc.set_time_base((1, 48000));
        enc.set_bit_rate(bitrate_kbps as usize * 1000);
        enc.set_flags(codec::Flags::GLOBAL_HEADER);
        let opened = enc.open()?;
        Ok(Self { encoder: opened })
    }

    fn parameters(&self) -> codec::Parameters {
        codec::Parameters::from(&self.encoder)
    }

    fn encode(&mut self, samples: &[[f32; 2]], pts: i64) -> Result<Vec<Packet>> {
        ensure!(samples.len() == 1024, "Expected 1024 audio samples");
        let mut frame = frame::Audio::new(
            format::Sample::F32(format::sample::Type::Planar),
            1024,
            ff::ChannelLayout::STEREO,
        );
        frame.set_rate(48000);
        frame.set_pts(Some(pts));
        for channel in 0..2 {
            for (out, input) in frame.plane_mut::<f32>(channel).iter_mut().zip(samples.iter()) {
                *out = input[channel];
            }
        }
        self.encoder.send_frame(&frame)?;
        let mut packets = Vec::new();
        loop {
            let mut packet = Packet::empty();
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => packets.push(packet),
                Err(ff::Error::Other { errno }) if errno == libc::EAGAIN => break,
                Err(ff::Error::Eof) => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(packets)
    }
}

pub struct BroadcastHandle {
    #[allow(dead_code)]
    pub config: BroadcastConfig,
    pub status: Arc<Mutex<BroadcastStatus>>,
    pub metrics: Arc<BroadcastMetrics>,
    pub stop: Arc<AtomicBool>,
    pub video_tx: Sender<VideoFrame>,
    pub video_free_rx: Receiver<Vec<u8>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl BroadcastHandle {
    pub fn is_live(&self) -> bool {
        matches!(*self.status.lock().unwrap(), BroadcastStatus::Live { .. })
    }

    pub fn status(&self) -> BroadcastStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for BroadcastHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Spawns the background Twitch broadcast worker thread.
pub fn start_broadcast(
    config: BroadcastConfig,
    audio_tap: Arc<crate::replay::AudioTap>,
) -> Result<BroadcastHandle> {
    ensure!(!config.stream_key.trim().is_empty(), "Twitch stream key is required");
    let status = Arc::new(Mutex::new(BroadcastStatus::Connecting));
    let metrics = Arc::new(BroadcastMetrics::default());
    let stop = Arc::new(AtomicBool::new(false));

    // Video frame queue: small capacity of 3 frames to strictly ensure low latency.
    let (vtx, vrx) = bounded::<VideoFrame>(3);
    let (vfree_tx, vfree_rx) = bounded::<Vec<u8>>(4);
    for _ in 0..4 {
        let _ = vfree_tx.send(Vec::new());
    }

    // Audio queues
    let (game_tx, game_rx) = bounded::<AudioBlock>(AUDIO_BLOCKS);
    let (game_free_tx, game_free_rx) = bounded::<AudioBlock>(AUDIO_BLOCKS);
    for _ in 0..AUDIO_BLOCKS {
        let _ = game_free_tx.send(AudioBlock {
            samples: Box::new([0.; AUDIO_SAMPLES]),
            len: 0,
            rate: 48000,
            channels: 2,
            at: 0,
            continuous: false,
        });
    }

    let (mic_tx, mic_rx) = bounded::<AudioBlock>(AUDIO_BLOCKS);
    let (mic_free_tx, mic_free_rx) = bounded::<AudioBlock>(AUDIO_BLOCKS);
    for _ in 0..AUDIO_BLOCKS {
        let _ = mic_free_tx.send(AudioBlock {
            samples: Box::new([0.; AUDIO_SAMPLES]),
            len: 0,
            rate: 48000,
            channels: 2,
            at: 0,
            continuous: false,
        });
    }

    audio_tap.register_broadcast_routes(
        AudioRoute {
            ready: game_tx,
            free: game_free_rx,
            stop: stop.clone(),
            dropped: metrics.audio_dropped.clone(),
            partial: None,
            next_continuous: false,
            format: None,
        },
        AudioRoute {
            ready: mic_tx,
            free: mic_free_rx,
            stop: stop.clone(),
            dropped: metrics.audio_dropped.clone(),
            partial: None,
            next_continuous: false,
            format: None,
        },
    );

    let worker_config = config.clone();
    let worker_status = status.clone();
    let worker_metrics = metrics.clone();
    let worker_stop = stop.clone();
    let worker_audio_tap = audio_tap.clone();

    let thread = std::thread::Builder::new()
        .name("twitch-broadcast".into())
        .spawn(move || {
            let result = run_broadcast_loop(
                worker_config,
                vrx,
                vfree_tx,
                game_rx,
                game_free_tx,
                mic_rx,
                mic_free_tx,
                &worker_status,
                &worker_metrics,
                &worker_stop,
                &worker_audio_tap,
            );
            worker_audio_tap.unregister_broadcast_routes();
            if let Err(e) = result {
                tracing::error!("Twitch broadcast stopped with error: {e:#}");
                *worker_status.lock().unwrap() = BroadcastStatus::Error(format!("{e:#}"));
            } else {
                *worker_status.lock().unwrap() = BroadcastStatus::Offline;
            }
        })?;

    Ok(BroadcastHandle {
        config,
        status,
        metrics,
        stop,
        video_tx: vtx,
        video_free_rx: vfree_rx,
        thread: Some(thread),
    })
}

#[allow(clippy::too_many_arguments)]
fn run_broadcast_loop(
    config: BroadcastConfig,
    video_rx: Receiver<VideoFrame>,
    video_free_tx: Sender<Vec<u8>>,
    game_rx: Receiver<AudioBlock>,
    game_free_tx: Sender<AudioBlock>,
    mic_rx: Receiver<AudioBlock>,
    mic_free_tx: Sender<AudioBlock>,
    status: &Arc<Mutex<BroadcastStatus>>,
    metrics: &Arc<BroadcastMetrics>,
    stop: &Arc<AtomicBool>,
    audio_tap: &Arc<crate::replay::AudioTap>,
) -> Result<()> {
    // Wait for the first video frame to establish stream dimensions
    let mut initial_frame = None;
    while !stop.load(Ordering::Acquire) {
        if let Ok(frame) = video_rx.recv_timeout(Duration::from_millis(100)) {
            initial_frame = Some(frame);
            break;
        }
    }
    let Some(first_frame) = initial_frame else {
        return Ok(());
    };

    let (dst_w, dst_h) = clamp_broadcast_resolution(first_frame.width, first_frame.height);
    let mut video_encoder = LiveVideoEncoder::new(&config, first_frame.width, first_frame.height, dst_w, dst_h)?;
    let mut audio_encoder = LiveAudioEncoder::new(config.audio_bitrate_kbps)?;

    // Format full RTMP/RTMPS URL
    let base = config.ingest_server.trim();
    let url = if base.ends_with('/') {
        format!("{}{}", base, config.stream_key.trim())
    } else {
        format!("{}/{}", base, config.stream_key.trim())
    };

    let mut output = format::output_as(&Path::new(&url), "flv")
        .context("Failed to allocate RTMP FLV output context")?;

    {
        let mut v_stream = output.add_stream(encoder::find(video_encoder.parameters().id()))?;
        v_stream.set_parameters(video_encoder.parameters());
        v_stream.set_time_base((1, 1000)); // FLV standard 1ms timebase

        let mut a_stream = output.add_stream(encoder::find(audio_encoder.parameters().id()))?;
        a_stream.set_parameters(audio_encoder.parameters());
        a_stream.set_time_base((1, 1000));
    }

    let mut muxer_opts = ff::Dictionary::new();
    muxer_opts.set("flvflags", "no_duration_filesize");
    muxer_opts.set("tcp_nodelay", "1");
    muxer_opts.set("rw_timeout", "10000000"); // 10s timeout
    output
        .write_header_with(muxer_opts)
        .context("Failed to connect or send stream header to Twitch ingest")?;

    let started_at = Instant::now();
    *status.lock().unwrap() = BroadcastStatus::Live { started_at };

    let mut audio_clock = AudioClock::new();
    let mut mic_clock = AudioClock::new();
    let mut mixer = TimelineMixer::new();

    let mut last_audio_pts = -1i64;
    let mut last_stat_time = Instant::now();
    let mut frames_this_sec = 0u32;
    let mut bytes_this_sec = 0u64;

    let start_epoch = first_frame.at;

    // Process the first frame
    let first_pts = 0i64;
    let pkts = video_encoder.encode(&first_frame.rgba, first_frame.width, first_frame.height, first_pts)?;
    for mut pkt in pkts {
        pkt.set_stream(0);
        pkt.set_position(-1);
        pkt.set_pts(Some(first_pts));
        pkt.set_dts(Some(first_pts));
        bytes_this_sec += pkt.size() as u64;
        pkt.write_interleaved(&mut output)?;
    }
    let _ = video_free_tx.send(first_frame.rgba);
    frames_this_sec += 1;
    let mut last_video_pts = 0i64;

    while !stop.load(Ordering::Acquire) {
        // Drain incoming audio from game
        while let Ok(block) = game_rx.try_recv() {
            let _ = audio_clock.append_recovering(&block, start_epoch);
            let _ = game_free_tx.send(block);
        }
        let vol = audio_tap.game_volume();
        while let Some((sample_idx, samples)) = audio_clock.take() {
            mixer.push(MixSource::Game, sample_idx, &samples, vol);
        }

        // Drain incoming audio from mic
        while let Ok(block) = mic_rx.try_recv() {
            let _ = mic_clock.append_recovering(&block, start_epoch);
            let _ = mic_free_tx.send(block);
        }
        while let Some((sample_idx, samples)) = mic_clock.take() {
            mixer.push(MixSource::Mic, sample_idx, &samples, 1.0);
        }

        // Encode mixed 48kHz stereo audio frames
        while let Some((sample_idx, samples)) = mixer.take() {
            let audio_ms = (sample_idx as f64 * 1000.0 / 48000.0).round() as i64;
            let audio_pts = audio_ms.max(last_audio_pts + 1);
            last_audio_pts = audio_pts;
            let pkts = audio_encoder.encode(&samples, audio_pts)?;
            for mut pkt in pkts {
                pkt.set_stream(1);
                pkt.set_position(-1);
                pkt.set_pts(Some(audio_pts));
                pkt.set_dts(Some(audio_pts));
                bytes_this_sec += pkt.size() as u64;
                pkt.write_interleaved(&mut output)?;
            }
        }

        // Process incoming video frame
        match video_rx.recv_timeout(Duration::from_millis(5)) {
            Ok(frame) => {
                let frame_ms = ((frame.at - start_epoch) as f64 / 1000.0).round() as i64;
                let pts = frame_ms.max(last_video_pts + 1);
                last_video_pts = pts;
                let pkts = video_encoder.encode(&frame.rgba, frame.width, frame.height, pts)?;
                for mut pkt in pkts {
                    pkt.set_stream(0);
                    pkt.set_position(-1);
                    pkt.set_pts(Some(pts));
                    pkt.set_dts(Some(pts));
                    bytes_this_sec += pkt.size() as u64;
                    pkt.write_interleaved(&mut output)?;
                }
                let _ = video_free_tx.send(frame.rgba);
                frames_this_sec += 1;
                metrics.frames_sent.fetch_add(1, Ordering::Relaxed);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }

        // Update 1-second metrics
        if last_stat_time.elapsed() >= Duration::from_secs(1) {
            let elapsed = last_stat_time.elapsed().as_secs_f32();
            let fps = ((frames_this_sec as f32 / elapsed) * 10.0).round() as u32;
            let kbps = ((bytes_this_sec as f32 * 8.0 / 1000.0) / elapsed).round() as u32;
            metrics.current_fps.store(fps, Ordering::Relaxed);
            metrics.current_bitrate_kbps.store(kbps, Ordering::Relaxed);
            metrics.bytes_sent.fetch_add(bytes_this_sec, Ordering::Relaxed);
            frames_this_sec = 0;
            bytes_this_sec = 0;
            last_stat_time = Instant::now();
        }
    }

    let _ = output.write_trailer();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clamp_broadcast_resolution_4k() {
        // 4K 16:9 downscales to 1080p
        assert_eq!(clamp_broadcast_resolution(3840, 2160), (1920, 1080));
    }

    #[test]
    fn test_clamp_broadcast_resolution_1080p() {
        // 1080p is unchanged
        assert_eq!(clamp_broadcast_resolution(1920, 1080), (1920, 1080));
    }

    #[test]
    fn test_clamp_broadcast_resolution_720p() {
        // Native 720p is kept at 720p (not upscaled)
        assert_eq!(clamp_broadcast_resolution(1280, 720), (1280, 720));
    }

    #[test]
    fn test_clamp_broadcast_resolution_odd_dimensions() {
        // Odd numbers are truncated to even numbers for YUV420P compatibility
        let (w, h) = clamp_broadcast_resolution(1919, 1079);
        assert_eq!(w % 2, 0);
        assert_eq!(h % 2, 0);
        assert!(w <= 1920 && h <= 1080);
    }

    #[test]
    fn test_clamp_broadcast_resolution_ultrawide() {
        // 3440x1440 (21:9) downscaled fits within 1920x1080 with even dimensions
        let (w, h) = clamp_broadcast_resolution(3440, 1440);
        assert_eq!(w, 1920);
        assert_eq!(h % 2, 0);
        assert!(h <= 1080);
    }

    #[test]
    fn test_clamp_broadcast_resolution_vertical() {
        // Vertical 9:16 fits within max 1080 height
        let (w, h) = clamp_broadcast_resolution(1080, 1920);
        assert_eq!(h, 1080);
        assert_eq!(w % 2, 0);
        assert!(w <= 1920);
    }

    #[test]
    fn test_broadcast_default_settings() {
        let config = crate::config::models::TwitchConfig::default();
        assert_eq!(config.video_bitrate_kbps, 6000);
        assert_eq!(config.audio_bitrate_kbps, 160);
        assert_eq!(config.video_encoder, StreamingVideoEncoder::SoftwareX264);
        assert_eq!(config.encoder_preset, "veryfast");
        assert_eq!(config.audio_encoder, StreamingAudioEncoder::Aac);
        assert_eq!(config.ingest_server, "rtmps://live.twitch.tv:443/app/");
        assert!(config.stream_key.is_empty());
    }
}

