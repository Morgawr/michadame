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

pub struct VideoEncoder {
    encoder: encoder::Video,
    frames: Buffer,
    // Keep the VAAPI device alive until the encoder and frame pool are gone.
    _device: Buffer,
    sw: frame::Video,
    scaler: ff::software::scaling::context::Context,
    width: u32,
    height: u32,
    pub rate: Rate,
}
impl VideoEncoder {
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
        let mut scaler = ff::software::scaling::context::Context::get(
            format::Pixel::RGBA,
            width,
            height,
            format::Pixel::NV12,
            width,
            height,
            ff::software::scaling::flag::Flags::BILINEAR,
        )?;
        unsafe {
            let coeff = ffi::sws_getCoefficients(ffi::SWS_CS_ITU709);
            check(ffi::sws_setColorspaceDetails(
                scaler.as_mut_ptr(),
                coeff,
                1,
                coeff,
                0,
                0,
                1 << 16,
                1 << 16,
            ))?;
        }
        Ok(Self {
            encoder,
            frames,
            _device: device,
            sw: frame::Video::new(format::Pixel::NV12, coded_width, coded_height),
            scaler,
            width,
            height,
            rate,
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
        unsafe {
            check(ffi::av_frame_make_writable(self.sw.as_mut_ptr()))?;
            convert_rgba(
                &mut self.scaler,
                &mut self.sw,
                rgba,
                self.width,
                self.height,
            )?;
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
    let mut output = format::output_as(path, "matroska")?;
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
    scaler: &mut ff::software::scaling::context::Context,
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
        // OpenGL is bottom-up; negative stride flips without a full RGBA copy.
        let stride = width as i32 * 4;
        let data = [
            rgba.as_ptr().add((height as usize - 1) * stride as usize),
            ptr::null(),
            ptr::null(),
            ptr::null(),
        ];
        let strides = [-stride, 0, 0, 0];
        let out = sw.as_mut_ptr();
        let rows = ffi::sws_scale(
            scaler.as_mut_ptr(),
            data.as_ptr(),
            strides.as_ptr(),
            0,
            height as i32,
            (*out).data.as_ptr(),
            (*out).linesize.as_ptr(),
        );
        ensure!(rows == height as i32, "RGB conversion failed");
    }
    // Extend edge pixels into coded padding. Matroska crop metadata removes it
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
        let mut scaler = ff::software::scaling::context::Context::get(
            format::Pixel::RGBA,
            w,
            h,
            format::Pixel::NV12,
            w,
            h,
            ff::software::scaling::flag::Flags::BILINEAR,
        )
        .unwrap();
        let mut output = frame::Video::new(format::Pixel::NV12, cw, ch);
        convert_rgba(&mut scaler, &mut output, &rgba, w, h).unwrap();
        let stride = output.stride(0);
        let luma = output.data(0);
        assert!(luma[0] < 25);
        assert!(luma[(h as usize - 1) * stride] > 225);
        assert_eq!(luma[62], luma[63]);
        assert_eq!(luma[(h as usize - 1) * stride], luma[h as usize * stride]);
    }

    /// A fully local media test: software-generated FFV1 fixtures exercise the
    /// actual history/export/Opus path without a display, GPU or capture device.
    #[test]
    fn exported_replay_demuxes_decodes_and_preserves_timing_and_crop() {
        ff::init().unwrap();
        let codec = encoder::find(codec::Id::FFV1).unwrap();
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
        let mut video = video.open().unwrap();
        let mut audio = AudioEncoder::new().unwrap();
        let mut history = super::super::ring::History::new(10);
        let epoch = 4_000_000i64;
        for tick in 0..90 {
            let mut f = frame::Video::new(format::Pixel::YUV420P, 64, 48);
            f.data_mut(0).fill(if tick < 45 { 40 } else { 180 });
            f.data_mut(1).fill(128);
            f.data_mut(2).fill(128);
            f.set_pts(Some(epoch + tick * 1_000_000 / 30));
            video.send_frame(&f).unwrap();
            let mut p = Packet::empty();
            while video.receive_packet(&mut p).is_ok() {
                p.set_duration(1_000_000 / 30);
                history.push(Encoded::new(p, true), 10_000_000);
                p = Packet::empty();
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
        let clip = history.clip(2, epoch + 2_900_000).unwrap();
        let mut vp = codec::Parameters::from(&video);
        set_display_crop(&mut vp, 63, 47).unwrap();
        let expected_start = clip[0].start;
        let expected_duration = clip.iter().map(|p| p.end).max().unwrap() - expected_start;
        let path = std::env::temp_dir().join(format!(
            "michadame-replay-test-{}-{}.mkv",
            std::process::id(),
            super::super::now_us()
        ));
        export(&path, vp, audio.parameters(), clip).unwrap();
        let mut input = format::input(&path).unwrap();
        assert_eq!(input.nb_streams(), 2);
        let vs = input.streams().best(ff::media::Type::Video).unwrap();
        let vi = vs.index();
        let vtb = vs.time_base();
        let parameters = vs.parameters();
        unsafe {
            let p = parameters.as_ptr();
            let crop = ffi::av_packet_side_data_get(
                (*p).coded_side_data,
                (*p).nb_coded_side_data,
                ffi::AVPacketSideDataType::AV_PKT_DATA_FRAME_CROPPING,
            );
            assert!(!crop.is_null(), "Matroska must preserve crop metadata");
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
        for (stream, packet) in input.packets() {
            if stream.index() == vi {
                vd.send_packet(&packet).unwrap();
                let mut f = frame::Video::empty();
                while vd.receive_frame(&mut f).is_ok() {
                    video_frames += 1;
                    first_v.get_or_insert(f.timestamp().unwrap_or(0) as f64 * f64::from(vtb));
                    assert!(f.data(0)[0] < 60 || f.data(0)[0] > 160);
                }
            } else if stream.index() == ai {
                ad.send_packet(&packet).unwrap();
                let mut f = frame::Audio::empty();
                while ad.receive_frame(&mut f).is_ok() {
                    first_a.get_or_insert(f.timestamp().unwrap_or(0) as f64 * f64::from(atb));
                    audio_samples += f.samples();
                }
            }
        }
        assert!(video_frames >= 40);
        assert!(audio_samples > 48000);
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
