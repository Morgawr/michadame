use super::{
    config::{Rate, ReplayConfig},
    ring::Encoded,
};
use anyhow::{bail, ensure, Context, Result};
use ff::{codec, encoder, format, frame, sys as ffi, Packet, Rational};
use ffmpeg_next as ff;
use std::{ffi::CString, ptr};

fn check(code: i32) -> Result<()> {
    if code < 0 {
        bail!("{}", ff::Error::from(code));
    }
    Ok(())
}
struct Buffer(*mut ffi::AVBufferRef);
impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            ffi::av_buffer_unref(&mut self.0);
        }
    }
}

fn conversion_threads() -> usize {
    // Leave CPU capacity for capture/playback. Created on the low-priority
    // recording worker, whose nice level the libswscale threads inherit.
    (std::thread::available_parallelism().map_or(1, |n| n.get()) / 2).clamp(1, 4)
}

/// The legacy sws_scale entry point only uses one slice context even when
/// threads are configured. Initialize a bounded pool and use sws_scale_frame
/// to actually dispatch the conversion across it.
struct RgbaConverter(*mut ffi::SwsContext);
impl RgbaConverter {
    fn new(width: u32, height: u32, threads: usize) -> Result<Self> {
        unsafe {
            let context = Self(ffi::sws_alloc_context());
            ensure!(!context.0.is_null(), "Cannot allocate replay converter");
            (*context.0).src_w = width as i32;
            (*context.0).src_h = height as i32;
            (*context.0).dst_w = width as i32;
            (*context.0).dst_h = height as i32;
            (*context.0).src_format = ffi::AVPixelFormat::AV_PIX_FMT_RGBA as i32;
            (*context.0).dst_format = ffi::AVPixelFormat::AV_PIX_FMT_NV12 as i32;
            (*context.0).flags = ff::software::scaling::flag::Flags::BILINEAR.bits() as u32;
            (*context.0).threads = threads.clamp(1, 4) as i32;
            check(ffi::sws_init_context(
                context.0,
                ptr::null_mut(),
                ptr::null_mut(),
            ))?;
            let coeff = ffi::sws_getCoefficients(ffi::SWS_CS_ITU709);
            check(ffi::sws_setColorspaceDetails(
                context.0,
                coeff,
                1,
                coeff,
                0,
                0,
                1 << 16,
                1 << 16,
            ))?;
            Ok(context)
        }
    }
}
impl Drop for RgbaConverter {
    fn drop(&mut self) {
        unsafe {
            ffi::sws_freeContext(self.0);
        }
    }
}

pub struct VideoEncoder {
    encoder: encoder::Video,
    frames: Buffer,
    // Keep the VAAPI device alive until the encoder and frame pool are gone.
    _device: Buffer,
    sw: frame::Video,
    scaler: RgbaConverter,
    width: u32,
    height: u32,
    pub rate: Rate,
    timings: EncodeTimings,
}
#[derive(Default)]
pub struct EncodeTimings {
    pub frames: u64,
    pub conversion_ms: f64,
    pub hardware_ms: f64,
}
impl VideoEncoder {
    pub fn take_timings(&mut self) -> EncodeTimings {
        std::mem::take(&mut self.timings)
    }
    pub fn conversion_threads(&self) -> usize {
        unsafe { (*self.scaler.0).threads as usize }
    }
    /// Runs only on the recording worker; never opens a capture device.
    pub fn new(config: &ReplayConfig, width: u32, height: u32, rate: Rate) -> Result<Self> {
        ff::init()?;
        let (coded_width, coded_height) = coded_size(width, height);
        let codec = encoder::find_by_name(config.codec.encoder())
            .context("Requested hardware encoder is unavailable in FFmpeg")?;
        let mut encoder = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()?;
        encoder.set_width(coded_width);
        encoder.set_height(coded_height);
        encoder.set_format(format::Pixel::VAAPI);
        encoder.set_time_base((1, 1_000_000));
        encoder.set_frame_rate(Some(Rational(rate.num as i32, rate.den as i32)));
        encoder.set_gop((rate.num / rate.den).max(1));
        encoder.set_max_b_frames(0);
        encoder.set_flags(codec::Flags::GLOBAL_HEADER);
        let mut device = Buffer(ptr::null_mut());
        let device_name = CString::new(config.render_device.as_str())?;
        let frames;
        unsafe {
            check(ffi::av_hwdevice_ctx_create(
                &mut device.0,
                ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                device_name.as_ptr(),
                ptr::null_mut(),
                0,
            ))
            .context("Cannot initialize VAAPI; select a supported render device")?;
            frames = Buffer(ffi::av_hwframe_ctx_alloc(device.0));
            ensure!(!frames.0.is_null(), "Cannot allocate VAAPI frame pool");
            let pool = (*frames.0).data as *mut ffi::AVHWFramesContext;
            (*pool).format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
            (*pool).sw_format = ffi::AVPixelFormat::AV_PIX_FMT_NV12;
            (*pool).width = coded_width as i32;
            (*pool).height = coded_height as i32;
            (*pool).initial_pool_size = 4;
            check(ffi::av_hwframe_ctx_init(frames.0))?;
            (*encoder.as_mut_ptr()).hw_frames_ctx = ffi::av_buffer_ref(frames.0);
            ensure!(
                !(*encoder.as_ptr()).hw_frames_ctx.is_null(),
                "Cannot retain VAAPI pool"
            );
            (*encoder.as_mut_ptr()).color_range = ffi::AVColorRange::AVCOL_RANGE_MPEG;
            (*encoder.as_mut_ptr()).colorspace = ffi::AVColorSpace::AVCOL_SPC_BT709;
            (*encoder.as_mut_ptr()).color_primaries = ffi::AVColorPrimaries::AVCOL_PRI_BT709;
            (*encoder.as_mut_ptr()).color_trc =
                ffi::AVColorTransferCharacteristic::AVCOL_TRC_IEC61966_2_1;
        }
        let mut options = ff::Dictionary::new();
        options.set("rc_mode", "CQP");
        options.set("global_quality", &config.quality.to_string());
        options.set("async_depth", "2");
        let encoder = encoder.open_with(options).context(
            "Hardware encoder rejected these settings; no software fallback was started",
        )?;
        let scaler = RgbaConverter::new(width, height, conversion_threads())?;
        Ok(Self {
            encoder,
            frames,
            _device: device,
            sw: frame::Video::new(format::Pixel::NV12, coded_width, coded_height),
            scaler,
            width,
            height,
            rate,
            timings: EncodeTimings::default(),
        })
    }
    pub fn parameters(&self) -> Result<codec::Parameters> {
        let mut parameters = codec::Parameters::from(&self.encoder);
        set_display_crop(&mut parameters, self.width, self.height)?;
        Ok(parameters)
    }
    pub fn encode(&mut self, rgba: &[u8], pts: i64, key: bool) -> Result<Vec<Encoded>> {
        ensure!(
            rgba.len() == self.width as usize * self.height as usize * 4,
            "Wrong replay surface length"
        );
        let mut hardware = frame::Video::empty();
        let conversion_started = std::time::Instant::now();
        unsafe {
            check(ffi::av_frame_make_writable(self.sw.as_mut_ptr()))?;
            convert_rgba(
                &mut self.scaler,
                &mut self.sw,
                rgba,
                self.width,
                self.height,
            )?;
        }
        self.timings.conversion_ms += conversion_started.elapsed().as_secs_f64() * 1000.;
        let hardware_started = std::time::Instant::now();
        unsafe {
            check(ffi::av_hwframe_get_buffer(
                self.frames.0,
                hardware.as_mut_ptr(),
                0,
            ))?;
            check(ffi::av_hwframe_transfer_data(
                hardware.as_mut_ptr(),
                self.sw.as_ptr(),
                0,
            ))?;
        }
        hardware.set_pts(Some(pts));
        hardware.set_kind(if key {
            ff::picture::Type::I
        } else {
            ff::picture::Type::None
        });
        self.encoder.send_frame(&hardware)?;
        let mut packets = Vec::new();
        loop {
            let mut packet = Packet::empty();
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    packet.set_duration(self.rate.us(1));
                    packets.push(Encoded::new(packet, true));
                }
                Err(ff::Error::Other { errno }) if errno == libc::EAGAIN => break,
                Err(ff::Error::Eof) => break,
                Err(e) => return Err(e.into()),
            }
        }
        self.timings.hardware_ms += hardware_started.elapsed().as_secs_f64() * 1000.;
        self.timings.frames += 1;
        Ok(packets)
    }
}

pub struct AudioEncoder {
    encoder: encoder::Audio,
}
impl AudioEncoder {
    pub fn new() -> Result<Self> {
        let codec =
            encoder::find_by_name("libopus").context("FFmpeg libopus encoder is unavailable")?;
        let mut encoder = codec::context::Context::new_with_codec(codec)
            .encoder()
            .audio()?;
        encoder.set_rate(48000);
        encoder.set_channel_layout(ff::ChannelLayout::STEREO);
        encoder.set_format(format::Sample::F32(format::sample::Type::Packed));
        encoder.set_time_base((1, 48000));
        encoder.set_bit_rate(192000);
        encoder.set_flags(codec::Flags::GLOBAL_HEADER);
        let mut options = ff::Dictionary::new();
        options.set("application", "audio");
        options.set("frame_duration", "20");
        let encoder = encoder.open_with(options)?;
        ensure!(encoder.frame_size() == 960, "Unexpected Opus frame size");
        Ok(Self { encoder })
    }
    pub fn parameters(&self) -> codec::Parameters {
        codec::Parameters::from(&self.encoder)
    }
    pub fn encode(&mut self, samples: &[[f32; 2]], pts: i64) -> Result<Vec<Encoded>> {
        ensure!(samples.len() == 960, "Expected 20ms audio block");
        let mut frame = frame::Audio::new(
            format::Sample::F32(format::sample::Type::Packed),
            960,
            ff::ChannelLayout::STEREO,
        );
        frame.set_rate(48000);
        frame.set_pts(Some(pts));
        frame.data_mut(0)[..960 * 8].copy_from_slice(bytemuck::cast_slice(samples));
        self.encoder.send_frame(&frame)?;
        let mut packets = Vec::new();
        loop {
            let mut packet = Packet::empty();
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    packet.rescale_ts((1, 48000), (1, 1_000_000));
                    packets.push(Encoded::new(packet, false));
                }
                Err(ff::Error::Other { errno }) if errno == libc::EAGAIN => break,
                Err(ff::Error::Eof) => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(packets)
    }
}

impl Drop for AudioEncoder {
    fn drop(&mut self) {
        // Live recording deliberately leaves the final delayed packet unsaved on
        // cancellation. Drain it on the worker so libopus closes cleanly.
        if self.encoder.send_eof().is_ok() {
            let mut packet = Packet::empty();
            while self.encoder.receive_packet(&mut packet).is_ok() {}
        }
    }
}

pub fn export(
    path: &std::path::Path,
    video: codec::Parameters,
    audio: codec::Parameters,
    mut packets: Vec<std::sync::Arc<Encoded>>,
) -> Result<()> {
    ensure!(!packets.is_empty(), "No replay packets");
    let start = packets
        .iter()
        .filter(|p| p.key)
        .map(|p| p.start)
        .min()
        .context("No replay keyframe")?;
    packets.sort_by_key(|p| (p.packet.dts().unwrap_or(p.start), !p.video));
    let mut output = format::output_as(path, "mp4")?;
    for parameters in [video, audio] {
        let mut stream = output.add_stream(encoder::find(parameters.id()))?;
        if parameters.medium() == ff::media::Type::Video {
            let rate = unsafe { (*parameters.as_ptr()).framerate };
            if rate.num > 0 && rate.den > 0 {
                stream.set_rate(Rational(rate.num, rate.den));
                stream.set_avg_frame_rate(Rational(rate.num, rate.den));
            }
        }
        stream.set_parameters(parameters);
        stream.set_time_base((1, 1_000_000));
    }
    output.write_header()?;
    for p in packets {
        let mut packet = p.packet.clone();
        let index = usize::from(!p.video);
        packet.set_stream(index);
        packet.set_position(-1);
        packet.set_pts(packet.pts().map(|t| t - start));
        packet.set_dts(packet.dts().map(|t| t - start));
        packet.rescale_ts((1, 1_000_000), output.stream(index).unwrap().time_base());
        packet.write_interleaved(&mut output)?;
    }
    output.write_trailer()?;
    Ok(())
}

fn coded_size(width: u32, height: u32) -> (u32, u32) {
    // Conservative surface alignment for AMD VCN; no scaling of the visible image.
    (width.div_ceil(64) * 64, height.div_ceil(16) * 16)
}
fn set_display_crop(parameters: &mut codec::Parameters, width: u32, height: u32) -> Result<()> {
    unsafe {
        let p = parameters.as_mut_ptr();
        ensure!(
            (*p).width as u32 >= width && (*p).height as u32 >= height,
            "Invalid display crop"
        );
        let right = (*p).width as u32 - width;
        let bottom = (*p).height as u32 - height;
        if right == 0 && bottom == 0 {
            return Ok(());
        }
        let data = ffi::av_packet_side_data_new(
            &mut (*p).coded_side_data,
            &mut (*p).nb_coded_side_data,
            ffi::AVPacketSideDataType::AV_PKT_DATA_FRAME_CROPPING,
            16,
            0,
        );
        ensure!(!data.is_null(), "Cannot allocate display crop metadata");
        let bytes = std::slice::from_raw_parts_mut((*data).data, 16);
        for (out, n) in bytes.chunks_exact_mut(4).zip([0, bottom, 0, right]) {
            out.copy_from_slice(&n.to_le_bytes());
        }
        // Square pixels in the cropped display rectangle.
        (*p).sample_aspect_ratio = ffi::AVRational { num: 1, den: 1 };
    }
    Ok(())
}
fn convert_rgba(
    scaler: &mut RgbaConverter,
    sw: &mut frame::Video,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<()> {
    ensure!(
        width > 0 && height > 0 && rgba.len() == width as usize * height as usize * 4,
        "Invalid RGBA surface"
    );
    unsafe {
        // Borrow the already-owned queue storage only for this synchronous call.
        // Refcounted headers avoid swscale copying a non-refcounted RGBA frame.
        // sws_frame_end below releases every internal reference before return,
        // including error paths. The callback never frees the borrowed Vec.
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
        ensure!(
            !(*source).buf[0].is_null(),
            "Cannot reference replay RGBA buffer"
        );
        // OpenGL is bottom-up; negative stride flips without a full RGBA copy.
        (*source).data[0] = rgba
            .as_ptr()
            .add((height as usize - 1) * width as usize * 4)
            .cast_mut();
        (*source).linesize[0] = -(width as i32 * 4);
        let mut output = frame::Video::empty();
        check(ffi::av_frame_ref(output.as_mut_ptr(), sw.as_ptr()))?;
        // Convert the visible rectangle into the larger, aligned allocation.
        output.set_width(width);
        output.set_height(height);
        let result = ffi::sws_scale_frame(scaler.0, output.as_mut_ptr(), source);
        ffi::sws_frame_end(scaler.0);
        check(result).context("RGB conversion failed")?;
    }
    // Extend edge pixels into coded padding. MP4 clean-aperture metadata removes it
    // on presentation, including odd window dimensions; content is never resized.
    let coded_height = sw.height() as usize;
    let coded_width = sw.width() as usize;
    for plane in 0..2 {
        let stride = sw.stride(plane);
        let (w, h, full_h, unit) = if plane == 0 {
            (width as usize, height as usize, coded_height, 1)
        } else {
            (
                width.div_ceil(2) as usize * 2,
                height.div_ceil(2) as usize,
                coded_height / 2,
                2,
            )
        };
        let bytes = sw.data_mut(plane);
        for row in 0..h {
            let start = row * stride;
            for x in w..coded_width {
                bytes[start + x] = bytes[start + w - unit + (x - w) % unit];
            }
        }
        for row in h..full_h {
            bytes.copy_within(
                (h - 1) * stride..(h - 1) * stride + coded_width,
                row * stride,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parallel_conversion_matches_original_pixels_including_chroma_and_odd_sizes() {
        for (w, h) in [(63, 47), (321, 241), (3024, 2160)] {
            let (cw, ch) = coded_size(w, h);
            let rgba: Vec<u8> = (0..w as usize * h as usize * 4)
                .map(|i| ((i * 37 + i / 17) % 256) as u8)
                .collect();
            let mut reference = frame::Video::new(format::Pixel::NV12, cw, ch);
            let mut original = ff::software::scaling::context::Context::get(
                format::Pixel::RGBA,
                w,
                h,
                format::Pixel::NV12,
                w,
                h,
                ff::software::scaling::flag::Flags::BILINEAR,
            )
            .unwrap();
            unsafe {
                let coeff = ffi::sws_getCoefficients(ffi::SWS_CS_ITU709);
                check(ffi::sws_setColorspaceDetails(
                    original.as_mut_ptr(),
                    coeff,
                    1,
                    coeff,
                    0,
                    0,
                    1 << 16,
                    1 << 16,
                ))
                .unwrap();
                let source = [
                    rgba.as_ptr().add((h as usize - 1) * w as usize * 4),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                ];
                let stride = [-(w as i32 * 4), 0, 0, 0];
                let out = reference.as_mut_ptr();
                assert_eq!(
                    ffi::sws_scale(
                        original.as_mut_ptr(),
                        source.as_ptr(),
                        stride.as_ptr(),
                        0,
                        h as i32,
                        (*out).data.as_ptr(),
                        (*out).linesize.as_ptr()
                    ),
                    h as i32
                );
            }
            for threads in [1, 2, 4] {
                let mut converter = RgbaConverter::new(w, h, threads).unwrap();
                let mut output = frame::Video::new(format::Pixel::NV12, cw, ch);
                // Reuse the same converter and output buffer, as the worker does.
                for _ in 0..2 {
                    convert_rgba(&mut converter, &mut output, &rgba, w, h).unwrap();
                    assert!(
                        unsafe { ffi::av_frame_is_writable(output.as_mut_ptr()) } > 0,
                        "Conversion retained output references and would force a copy next frame"
                    );
                    for plane in 0..2 {
                        let width = if plane == 0 {
                            w as usize
                        } else {
                            w.div_ceil(2) as usize * 2
                        };
                        let height = if plane == 0 {
                            h as usize
                        } else {
                            h.div_ceil(2) as usize
                        };
                        for row in 0..height {
                            let expected =
                                &reference.data(plane)[row * reference.stride(plane)..][..width];
                            let actual = &output.data(plane)[row * output.stride(plane)..][..width];
                            assert_eq!(
                                actual, expected,
                                "{w}x{h}, {threads} threads, plane {plane}, row {row}"
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn bottom_up_rgba_is_flipped_and_padded_without_scaling() {
        ff::init().unwrap();
        let (w, h) = (63, 47);
        let (cw, ch) = coded_size(w, h);
        let mut rgba = vec![0u8; w as usize * h as usize * 4];
        // GL's bottom row is white, top row black.
        for y in 0..h as usize {
            for x in 0..w as usize {
                let value = if y < h as usize / 2 { 255 } else { 0 };
                rgba[(y * w as usize + x) * 4..(y * w as usize + x) * 4 + 4]
                    .copy_from_slice(&[value, value, value, 255]);
            }
        }
        let mut scaler = RgbaConverter::new(w, h, 2).unwrap();
        let mut output = frame::Video::new(format::Pixel::NV12, cw, ch);
        convert_rgba(&mut scaler, &mut output, &rgba, w, h).unwrap();
        let stride = output.stride(0);
        let luma = output.data(0);
        assert!(luma[0] < 25);
        assert!(luma[(h as usize - 1) * stride] > 225);
        assert_eq!(luma[62], luma[63]);
        assert_eq!(luma[(h as usize - 1) * stride], luma[h as usize * stride]);
    }

    /// A fully local media test: software-generated video fixtures exercise the
    /// actual history/export/Opus path without a display, GPU or capture device.
    #[test]
    fn exported_replay_demuxes_decodes_and_preserves_timing_and_crop() {
        for codec in ["libx264", "libx265", "libaom-av1"] {
            export_fixture(false, codec, false);
        }
    }

    #[test]
    #[ignore = "Local CPU-only conversion benchmark; no GPU or capture device"]
    fn benchmark_replay_conversion() {
        let (w, h) = (3024, 2160);
        let rgba: Vec<u8> = (0..w as usize * h as usize * 4)
            .map(|i| (i % 251) as u8)
            .collect();
        let (cw, ch) = coded_size(w, h);
        let mut out = frame::Video::new(format::Pixel::NV12, cw, ch);
        for threads in [1, 2, 4] {
            let mut scaler = RgbaConverter::new(w, h, threads).unwrap();
            let mut times = Vec::new();
            for i in 0..130 {
                let start = std::time::Instant::now();
                convert_rgba(&mut scaler, &mut out, &rgba, w, h).unwrap();
                if i >= 10 {
                    times.push(start.elapsed().as_secs_f64() * 1000.);
                }
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "Replay CPU conversion {w}x{h}, {threads} threads: mean {:.2} ms, p95 {:.2} ms",
                times.iter().sum::<f64>() / times.len() as f64,
                times[times.len() * 95 / 100]
            );
        }
    }
    #[test]
    fn exported_replay_keeps_audio_and_video_before_and_after_dropped_frame_bursts() {
        for codec in ["libx264", "libx265", "libaom-av1"] {
            export_fixture(true, codec, false);
        }
    }
    #[test]
    fn exported_replay_preserves_subframe_timestamps_without_speeding_up_catchup() {
        for codec in ["libx264", "libx265", "libaom-av1"] {
            export_fixture(false, codec, true);
        }
    }
    fn export_fixture(drop_frames: bool, codec_name: &str, jitter: bool) {
        ff::init().unwrap();
        let codec = encoder::find_by_name(codec_name)
            .expect("Synthetic MP4 fixtures need software H.264/HEVC/AV1 encoders");
        let expected_codec = codec.id();
        let mut video = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .unwrap();
        video.set_width(64);
        video.set_height(48);
        video.set_format(format::Pixel::YUV420P);
        video.set_time_base((1, 1_000_000));
        video.set_frame_rate(Some(Rational(30, 1)));
        video.set_gop(15);
        video.set_flags(codec::Flags::GLOBAL_HEADER);
        video.set_max_b_frames(0);
        video.set_threading(codec::threading::Config {
            count: 1,
            ..Default::default()
        });
        let mut options = ff::Dictionary::new();
        match codec_name {
            "libx264" => {
                options.set("preset", "ultrafast");
                options.set("tune", "zerolatency");
                options.set("qp", "0");
            }
            "libx265" => {
                options.set("preset", "ultrafast");
                options.set("tune", "zerolatency");
                options.set(
                    "x265-params",
                    "lossless=1:pools=none:frame-threads=1:log-level=error",
                );
            }
            "libaom-av1" => {
                options.set("cpu-used", "8");
                options.set("usage", "realtime");
                options.set("lag-in-frames", "0");
                options.set("crf", "0");
            }
            _ => unreachable!(),
        }
        let mut video = video.open_with(options).unwrap();
        let mut audio = AudioEncoder::new().unwrap();
        let mut history = super::super::ring::History::new(10);
        let epoch = 4_000_000i64;
        let mut schedule = super::super::worker::VideoSchedule::new(epoch);
        for tick in 0..90 {
            let missing = drop_frames
                && (tick == 6
                    || (13..16).contains(&tick)
                    || (19..28).contains(&tick)
                    || (30..70).contains(&tick));
            if !missing {
                let at = epoch + tick * 1_000_000 / 30
                    - if jitter && tick % 2 == 1 { 24_000 } else { 0 };
                let (pts, key) = schedule.accept(at).unwrap();
                assert_eq!(pts, at);
                let mut f = frame::Video::new(format::Pixel::YUV420P, 64, 48);
                f.data_mut(0).fill(if tick < 45 { 40 } else { 180 });
                f.data_mut(1).fill(128);
                f.data_mut(2).fill(128);
                f.set_pts(Some(pts));
                f.set_kind(if key {
                    ff::picture::Type::I
                } else {
                    ff::picture::Type::None
                });
                video.send_frame(&f).unwrap();
                let mut p = Packet::empty();
                while video.receive_packet(&mut p).is_ok() {
                    p.set_duration(1_000_000 / 30);
                    history.push(Encoded::new(p, true), 10_000_000);
                    p = Packet::empty();
                }
            }
            if tick % 3 == 0 {
                for j in 0..5 {
                    let at = tick / 3 * 4800 + j * 960;
                    let samples: Vec<[f32; 2]> = (0..960)
                        .map(|i| {
                            let v = (((at + i) as f32 / 48000.) * 440. * std::f32::consts::TAU)
                                .sin()
                                * 0.2;
                            [v, v]
                        })
                        .collect();
                    for mut p in audio.encode(&samples, at).unwrap() {
                        p.packet.set_pts(p.packet.pts().map(|t| t + epoch));
                        p.packet.set_dts(p.packet.dts().map(|t| t + epoch));
                        p.start += epoch;
                        p.end += epoch;
                        history.push(p, 10_000_000);
                    }
                }
            }
        }
        let clip = history
            .clip(if drop_frames { 3 } else { 2 }, epoch + 2_900_000)
            .unwrap();
        let expected_frames = clip.iter().filter(|p| p.video).count();
        if drop_frames {
            assert_eq!(
                clip[0].start, epoch,
                "Earlier history must survive multiple gaps"
            );
            assert!(clip
                .iter()
                .filter(|p| p.video)
                .any(|p| p.packet.duration() > 1_000_000));
            assert_eq!(
                clip.iter().filter(|p| p.video).count(),
                34,
                "Encode only received frames before the cutoff"
            );
        }
        let mut vp = codec::Parameters::from(&video);
        set_display_crop(&mut vp, 63, 47).unwrap();
        let expected_start = clip[0].start;
        let expected_intervals: Vec<_> = clip
            .iter()
            .filter(|p| p.video)
            .map(|p| {
                (
                    (p.start - expected_start) as f64 / 1e6,
                    p.packet.duration() as f64 / 1e6,
                )
            })
            .collect();
        let expected_duration = clip.iter().map(|p| p.end).max().unwrap() - expected_start;
        // Production saves use a temporary suffix before publishing .mp4.
        // The explicit muxer must work independently of the filename extension.
        let path = std::env::temp_dir().join(format!(
            "michadame-replay-test-{}-{}.partial",
            std::process::id(),
            super::super::now_us()
        ));
        export(&path, vp, audio.parameters(), clip).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            &bytes[4..8],
            b"ftyp",
            "Must write an MP4 container, not just rename Matroska"
        );
        let mut input = format::input(&path).unwrap();
        assert_eq!(input.nb_streams(), 2);
        let vs = input.streams().best(ff::media::Type::Video).unwrap();
        let vi = vs.index();
        let vtb = vs.time_base();
        let parameters = vs.parameters();
        assert_eq!(parameters.id(), expected_codec);
        unsafe {
            let p = parameters.as_ptr();
            let crop = ffi::av_packet_side_data_get(
                (*p).coded_side_data,
                (*p).nb_coded_side_data,
                ffi::AVPacketSideDataType::AV_PKT_DATA_FRAME_CROPPING,
            );
            assert!(!crop.is_null(), "MP4 must preserve crop metadata");
            let crop = std::slice::from_raw_parts((*crop).data, 16);
            assert_eq!(&crop[4..8], &1u32.to_le_bytes());
            assert_eq!(&crop[12..16], &1u32.to_le_bytes());
        }
        let mut vd = codec::context::Context::from_parameters(parameters)
            .unwrap()
            .decoder()
            .video()
            .unwrap();
        let audio_stream = input.streams().best(ff::media::Type::Audio).unwrap();
        let ai = audio_stream.index();
        let atb = audio_stream.time_base();
        assert_eq!(audio_stream.parameters().id(), codec::Id::OPUS);
        let mut ad = codec::context::Context::from_parameters(audio_stream.parameters())
            .unwrap()
            .decoder()
            .audio()
            .unwrap();
        unsafe {
            (*ad.as_mut_ptr()).pkt_timebase = atb.into();
        }
        let mut video_frames = 0;
        let mut audio_samples = 0;
        let mut first_v = None;
        let mut first_a = None;
        let mut intervals = Vec::new();
        let mut audio_end = 0f64;
        for (stream, packet) in input.packets() {
            if stream.index() == vi {
                let expected = expected_intervals[intervals.len()];
                assert!(
                    (packet.pts().unwrap() as f64 * f64::from(vtb) - expected.0).abs() < 0.000002
                );
                assert!((packet.duration() as f64 * f64::from(vtb) - expected.1).abs() < 0.000002);
                intervals.push((
                    packet.pts().unwrap() as f64 * f64::from(vtb),
                    packet.duration() as f64 * f64::from(vtb),
                ));
                vd.send_packet(&packet).unwrap();
                let mut f = frame::Video::empty();
                while vd.receive_frame(&mut f).is_ok() {
                    video_frames += 1;
                    first_v.get_or_insert(f.timestamp().unwrap_or(0) as f64 * f64::from(vtb));
                    assert!(f.data(0)[0] < 60 || f.data(0)[0] > 160);
                    if drop_frames {
                        let elapsed = f.timestamp().unwrap() as f64 * f64::from(vtb);
                        assert_eq!(
                            f.data(0)[0],
                            if elapsed < 2.3 { 40 } else { 180 },
                            "Recovered picture appeared at the wrong time"
                        );
                    }
                }
            } else if stream.index() == ai {
                ad.send_packet(&packet).unwrap();
                let mut f = frame::Audio::empty();
                while ad.receive_frame(&mut f).is_ok() {
                    first_a.get_or_insert(f.timestamp().unwrap_or(0) as f64 * f64::from(atb));
                    audio_samples += f.samples();
                    audio_end = f.timestamp().unwrap_or(0) as f64 * f64::from(atb)
                        + f.samples() as f64 / f.rate() as f64;
                }
            }
        }
        assert_eq!(video_frames, expected_frames);
        if drop_frames {
            assert!(
                intervals.iter().any(|(_, duration)| *duration > 1.),
                "Held-frame duration lost during muxing"
            );
            for pair in intervals.windows(2) {
                assert!(
                    (pair[0].0 + pair[0].1 - pair[1].0).abs() < 0.002,
                    "Gap or overlap in recovered video timeline"
                );
            }
            assert!(intervals[0].0 < 0.01);
            assert!(
                intervals.last().unwrap().0 > 2.8,
                "Did not resume after the video stall"
            );
        }
        assert!(audio_samples > 48000);
        let last_video = intervals.last().unwrap();
        assert!(
            (last_video.0 + last_video.1 - audio_end).abs() < 0.03,
            "Audio/video ends drifted after recovery"
        );
        assert!(
            (first_v.unwrap() - first_a.unwrap()).abs() < 0.03,
            "Audio/video start differs by >30ms"
        );
        assert!(
            (input.duration() - expected_duration).abs() < 50_000,
            "Export duration changed"
        );
        drop(input);
        std::fs::remove_file(path).unwrap();
    }
}
