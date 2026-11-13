use std::ptr;

use ffmpeg_next::sys::{
    av_buffer_ref, av_buffer_unref, av_frame_copy_props, av_frame_move_ref, av_frame_unref,
    av_hwdevice_ctx_create, av_hwframe_transfer_data, av_pix_fmt_desc_get, avcodec_get_hw_config,
    AVBufferRef, AVCodecContext, AVHWDeviceType, AVPixelFormat,
    AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX, AV_PIX_FMT_FLAG_HWACCEL,
};

const MAX_STARTUP_FALLBACK_PACKETS: usize = 32;

const HW_DEVICE_CANDIDATES: &[(AVHWDeviceType, &str)] = &[
    (AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI, "VAAPI"),
    (AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA, "NVDEC"),
    (AVHWDeviceType::AV_HWDEVICE_TYPE_DRM, "DRM"),
];

/// Returns `true` if `codec_id` is compressed video eligible for GPU hardware decoding,
/// skipping uncompressed V4L2/raw formats (`RAWVIDEO`, etc.).
pub fn is_hwdec_eligible_codec(codec_id: ffmpeg_next::codec::Id) -> bool {
    matches!(
        codec_id,
        ffmpeg_next::codec::Id::H264
            | ffmpeg_next::codec::Id::HEVC
            | ffmpeg_next::codec::Id::AV1
            | ffmpeg_next::codec::Id::VP9
            | ffmpeg_next::codec::Id::VP8
            | ffmpeg_next::codec::Id::MPEG2VIDEO
            | ffmpeg_next::codec::Id::VC1
            | ffmpeg_next::codec::Id::MJPEG
    )
}

unsafe fn select_hw_or_sw_format(
    target_hw_fmt: AVPixelFormat,
    pix_fmts: *const AVPixelFormat,
) -> AVPixelFormat {
    if pix_fmts.is_null() {
        return AVPixelFormat::AV_PIX_FMT_NONE;
    }

    if target_hw_fmt != AVPixelFormat::AV_PIX_FMT_NONE {
        let mut idx = 0usize;
        loop {
            let candidate = unsafe { *pix_fmts.add(idx) };
            if candidate == AVPixelFormat::AV_PIX_FMT_NONE {
                break;
            }
            if candidate == target_hw_fmt {
                return candidate;
            }
            idx += 1;
        }
    }

    // Fall back to the first non-hwaccel software format offered by FFmpeg.
    let mut idx = 0usize;
    loop {
        let candidate = unsafe { *pix_fmts.add(idx) };
        if candidate == AVPixelFormat::AV_PIX_FMT_NONE {
            break;
        }
        let desc = unsafe { av_pix_fmt_desc_get(candidate) };
        if !desc.is_null() && (unsafe { (*desc).flags } & (AV_PIX_FMT_FLAG_HWACCEL as u64)) == 0 {
            return candidate;
        }
        idx += 1;
    }

    unsafe { *pix_fmts }
}

unsafe extern "C" fn get_hw_format(
    ctx: *mut AVCodecContext,
    pix_fmts: *const AVPixelFormat,
) -> AVPixelFormat {
    let target_hw_fmt = if !ctx.is_null() && !unsafe { (*ctx).opaque }.is_null() {
        unsafe { *((*ctx).opaque as *const AVPixelFormat) }
    } else {
        AVPixelFormat::AV_PIX_FMT_NONE
    };
    unsafe { select_hw_or_sw_format(target_hw_fmt, pix_fmts) }
}

/// Video decoder session wrapping `ffmpeg_next::decoder::Video` with optional VAAPI / NVDEC / DRM
/// hardware acceleration and automatic runtime fallback to CPU software decoding.
pub struct VideoDecoderSession {
    decoder: Option<ffmpeg_next::decoder::Video>,
    hw_device_ctx: *mut AVBufferRef,
    hw_pix_fmt: AVPixelFormat,
    opaque_pix_fmt: *mut AVPixelFormat,
    raw_decoded: ffmpeg_next::frame::Video,
    sw_transfer_frame: ffmpeg_next::frame::Video,
    hw_backend_name: Option<&'static str>,
    saved_params: ffmpeg_next::codec::Parameters,
    confirmed_working: bool,
    saw_eof: bool,
    startup_packets: Vec<ffmpeg_next::Packet>,
    #[cfg(test)]
    sim_fail_on_send: bool,
    #[cfg(test)]
    sim_fail_on_transfer: bool,
}

// SAFETY: `VideoDecoderSession` exclusively owns its `AVCodecContext`, `AVBufferRef` hardware
// device handle, heap-allocated `opaque_pix_fmt`, and internal `AVFrame`s, and is only accessed
// via `&mut self` on a single decoding thread at a time.
unsafe impl Send for VideoDecoderSession {}

impl VideoDecoderSession {
    pub fn open(
        params: ffmpeg_next::codec::Parameters,
        allow_hwdec: bool,
    ) -> Result<Self, ffmpeg_next::Error> {
        let codec_id = params.id();
        if allow_hwdec && is_hwdec_eligible_codec(codec_id) {
            if let Some(hw_session) = Self::try_open_hardware(params.clone(), codec_id) {
                return Ok(hw_session);
            }
        }

        let decoder = Self::open_software_decoder(params.clone())?;
        Ok(Self {
            decoder: Some(decoder),
            hw_device_ctx: ptr::null_mut(),
            hw_pix_fmt: AVPixelFormat::AV_PIX_FMT_NONE,
            opaque_pix_fmt: ptr::null_mut(),
            raw_decoded: ffmpeg_next::frame::Video::empty(),
            sw_transfer_frame: ffmpeg_next::frame::Video::empty(),
            hw_backend_name: None,
            saved_params: params,
            confirmed_working: true,
            saw_eof: false,
            startup_packets: Vec::new(),
            #[cfg(test)]
            sim_fail_on_send: false,
            #[cfg(test)]
            sim_fail_on_transfer: false,
        })
    }

    fn open_software_decoder(
        params: ffmpeg_next::codec::Parameters,
    ) -> Result<ffmpeg_next::decoder::Video, ffmpeg_next::Error> {
        ffmpeg_next::codec::context::Context::from_parameters(params)
            .and_then(|c| c.decoder().video())
    }

    fn try_open_hardware(
        params: ffmpeg_next::codec::Parameters,
        codec_id: ffmpeg_next::codec::Id,
    ) -> Option<Self> {
        let codec = ffmpeg_next::decoder::find(codec_id)?;
        let codec_ptr = unsafe { codec.as_ptr() };
        if codec_ptr.is_null() {
            return None;
        }

        for &(dev_type, backend_name) in HW_DEVICE_CANDIDATES {
            let mut matched_pix_fmt = None;
            let mut cfg_idx: libc::c_int = 0;
            loop {
                let hw_cfg = unsafe { avcodec_get_hw_config(codec_ptr, cfg_idx) };
                if hw_cfg.is_null() {
                    break;
                }
                let methods = unsafe { (*hw_cfg).methods as u32 };
                let cfg_dev_type = unsafe { (*hw_cfg).device_type };
                if (methods & (AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as u32)) != 0
                    && cfg_dev_type == dev_type
                {
                    matched_pix_fmt = Some(unsafe { (*hw_cfg).pix_fmt });
                    break;
                }
                cfg_idx += 1;
            }

            let Some(hw_pix_fmt) = matched_pix_fmt else {
                continue;
            };
            if hw_pix_fmt == AVPixelFormat::AV_PIX_FMT_NONE {
                continue;
            }

            let mut hw_device_ctx: *mut AVBufferRef = ptr::null_mut();
            let create_ret = unsafe {
                av_hwdevice_ctx_create(
                    &mut hw_device_ctx,
                    dev_type,
                    ptr::null(),
                    ptr::null_mut(),
                    0,
                )
            };
            if create_ret < 0 || hw_device_ctx.is_null() {
                if !hw_device_ctx.is_null() {
                    unsafe { av_buffer_unref(&mut hw_device_ctx) };
                }
                continue;
            }

            let Ok(mut ctx) = ffmpeg_next::codec::context::Context::from_parameters(params.clone())
            else {
                unsafe { av_buffer_unref(&mut hw_device_ctx) };
                continue;
            };

            let ctx_hw_ref = unsafe { av_buffer_ref(hw_device_ctx) };
            if ctx_hw_ref.is_null() {
                unsafe { av_buffer_unref(&mut hw_device_ctx) };
                continue;
            }

            let opaque_pix_fmt = Box::into_raw(Box::new(hw_pix_fmt));
            unsafe {
                let ctx_ptr = ctx.as_mut_ptr();
                (*ctx_ptr).opaque = opaque_pix_fmt as *mut libc::c_void;
                (*ctx_ptr).get_format = Some(get_hw_format);
                (*ctx_ptr).hw_device_ctx = ctx_hw_ref;
                (*ctx_ptr).thread_count = 1;
            }

            match ctx.decoder().open_as(codec).and_then(|o| o.video()) {
                Ok(decoder) => {
                    tracing::info!(
                        codec = ?codec_id,
                        backend = backend_name,
                        hw_pix_fmt = ?hw_pix_fmt,
                        "Opened hardware-accelerated FFmpeg video decoder"
                    );
                    return Some(Self {
                        decoder: Some(decoder),
                        hw_device_ctx,
                        hw_pix_fmt,
                        opaque_pix_fmt,
                        raw_decoded: ffmpeg_next::frame::Video::empty(),
                        sw_transfer_frame: ffmpeg_next::frame::Video::empty(),
                        hw_backend_name: Some(backend_name),
                        saved_params: params,
                        confirmed_working: false,
                        saw_eof: false,
                        startup_packets: Vec::new(),
                        #[cfg(test)]
                        sim_fail_on_send: false,
                        #[cfg(test)]
                        sim_fail_on_transfer: false,
                    });
                }
                Err(err) => {
                    tracing::debug!(
                        codec = ?codec_id,
                        backend = backend_name,
                        error = %err,
                        "Hardware decoder open failed; probing next backend"
                    );
                    // `ctx` was dropped inside `open_as`, so `AVCodecContext` no longer references
                    // `opaque_pix_fmt`.
                    unsafe {
                        drop(Box::from_raw(opaque_pix_fmt));
                        av_buffer_unref(&mut hw_device_ctx);
                    }
                }
            }
        }

        None
    }

    pub fn is_hardware(&self) -> bool {
        self.hw_backend_name.is_some()
    }

    #[allow(dead_code)]
    pub fn hw_backend_name(&self) -> Option<&'static str> {
        self.hw_backend_name
    }

    pub fn width(&self) -> u32 {
        self.decoder.as_ref().map_or(0, |d| d.width())
    }

    pub fn height(&self) -> u32 {
        self.decoder.as_ref().map_or(0, |d| d.height())
    }

    #[allow(dead_code)]
    pub fn format(&self) -> ffmpeg_next::format::Pixel {
        self.decoder
            .as_ref()
            .map_or(ffmpeg_next::format::Pixel::None, |d| d.format())
    }

    #[allow(dead_code)]
    pub fn color_range(&self) -> ffmpeg_next::color::Range {
        self.decoder
            .as_ref()
            .map_or(ffmpeg_next::color::Range::Unspecified, |d| d.color_range())
    }

    pub fn flush(&mut self) {
        self.saw_eof = false;
        if !self.confirmed_working {
            self.startup_packets.clear();
        }
        unsafe {
            av_frame_unref(self.raw_decoded.as_mut_ptr());
            av_frame_unref(self.sw_transfer_frame.as_mut_ptr());
        }
        if let Some(decoder) = self.decoder.as_mut() {
            decoder.flush();
        }
    }

    fn release_hw_resources(&mut self) {
        // Drop the `AVCodecContext` BEFORE freeing `opaque_pix_fmt` or `hw_device_ctx`.
        self.decoder = None;
        unsafe {
            av_frame_unref(self.raw_decoded.as_mut_ptr());
            av_frame_unref(self.sw_transfer_frame.as_mut_ptr());
            if !self.opaque_pix_fmt.is_null() {
                drop(Box::from_raw(self.opaque_pix_fmt));
                self.opaque_pix_fmt = ptr::null_mut();
            }
            if !self.hw_device_ctx.is_null() {
                av_buffer_unref(&mut self.hw_device_ctx);
                self.hw_device_ctx = ptr::null_mut();
            }
        }
        self.hw_pix_fmt = AVPixelFormat::AV_PIX_FMT_NONE;
        self.hw_backend_name = None;
    }

    fn fallback_to_software_and_replay(&mut self) -> Result<(), ffmpeg_next::Error> {
        self.release_hw_resources();
        let mut sw_decoder = Self::open_software_decoder(self.saved_params.clone())?;
        let queued = std::mem::take(&mut self.startup_packets);
        for pkt in &queued {
            let _ = sw_decoder.send_packet(pkt);
        }
        self.decoder = Some(sw_decoder);
        self.confirmed_working = true;
        #[cfg(test)]
        {
            self.sim_fail_on_send = false;
            self.sim_fail_on_transfer = false;
        }
        Ok(())
    }

    pub fn send_packet(&mut self, packet: &ffmpeg_next::Packet) -> Result<(), ffmpeg_next::Error> {
        self.saw_eof = false;

        if self.is_hardware() && !self.confirmed_working {
            #[cfg(test)]
            let send_res = if self.sim_fail_on_send {
                Err(ffmpeg_next::Error::InvalidData)
            } else {
                self.decoder
                    .as_mut()
                    .expect("decoder present")
                    .send_packet(packet)
            };
            #[cfg(not(test))]
            let send_res = self
                .decoder
                .as_mut()
                .expect("decoder present")
                .send_packet(packet);

            match send_res {
                Ok(()) => {
                    if self.startup_packets.len() < MAX_STARTUP_FALLBACK_PACKETS {
                        self.startup_packets.push(packet.clone());
                    }
                    return Ok(());
                }
                Err(ffmpeg_next::Error::Other { errno }) if errno == libc::EAGAIN => {
                    return Err(ffmpeg_next::Error::Other { errno });
                }
                Err(err) => {
                    tracing::warn!(
                        backend = self.hw_backend_name.unwrap_or("hwdec"),
                        error = %err,
                        "Hardware video decoder rejected initial packet; falling back to software decoding"
                    );
                    self.fallback_to_software_and_replay()?;
                    return self
                        .decoder
                        .as_mut()
                        .expect("software fallback decoder present")
                        .send_packet(packet);
                }
            }
        }

        self.decoder
            .as_mut()
            .expect("decoder present")
            .send_packet(packet)
    }

    pub fn send_eof(&mut self) -> Result<(), ffmpeg_next::Error> {
        self.saw_eof = true;
        self.decoder.as_mut().expect("decoder present").send_eof()
    }

    /// Receives the next decoded software-accessible video frame into `out`.
    /// Returns `Ok(true)` when a frame was produced, or `Ok(false)` on `EAGAIN` / `EOF`.
    pub fn receive_decoded_frame(
        &mut self,
        out: &mut ffmpeg_next::frame::Video,
    ) -> Result<bool, ffmpeg_next::Error> {
        if !self.is_hardware() {
            return match self
                .decoder
                .as_mut()
                .expect("decoder present")
                .receive_frame(out)
            {
                Ok(()) => Ok(true),
                Err(ffmpeg_next::Error::Other { errno }) if errno == libc::EAGAIN => Ok(false),
                Err(ffmpeg_next::Error::Eof) => Ok(false),
                Err(err) => Err(err),
            };
        }

        let recv_res = self
            .decoder
            .as_mut()
            .expect("decoder present")
            .receive_frame(&mut self.raw_decoded);

        match recv_res {
            Err(ffmpeg_next::Error::Other { errno }) if errno == libc::EAGAIN => return Ok(false),
            Err(ffmpeg_next::Error::Eof) => return Ok(false),
            Err(err) => {
                if !self.confirmed_working {
                    tracing::warn!(
                        backend = self.hw_backend_name.unwrap_or("hwdec"),
                        error = %err,
                        "Hardware video decoder failed on initial frame; falling back to software decoding"
                    );
                    self.fallback_to_software_and_replay()?;
                    if self.saw_eof {
                        let _ = self.send_eof();
                    }
                    return self.receive_decoded_frame(out);
                }
                return Err(err);
            }
            Ok(()) => {}
        }

        let decoded_av_fmt = AVPixelFormat::from(self.raw_decoded.format());
        #[cfg(test)]
        let needs_hw_transfer = decoded_av_fmt == self.hw_pix_fmt || self.sim_fail_on_transfer;
        #[cfg(not(test))]
        let needs_hw_transfer = decoded_av_fmt == self.hw_pix_fmt;

        if needs_hw_transfer {
            unsafe {
                av_frame_unref(self.sw_transfer_frame.as_mut_ptr());
                #[cfg(test)]
                let ret = if self.sim_fail_on_transfer {
                    -libc::ENOSYS
                } else {
                    av_hwframe_transfer_data(
                        self.sw_transfer_frame.as_mut_ptr(),
                        self.raw_decoded.as_ptr(),
                        0,
                    )
                };
                #[cfg(not(test))]
                let ret = av_hwframe_transfer_data(
                    self.sw_transfer_frame.as_mut_ptr(),
                    self.raw_decoded.as_ptr(),
                    0,
                );

                if ret < 0 {
                    av_frame_unref(self.raw_decoded.as_mut_ptr());
                    av_frame_unref(self.sw_transfer_frame.as_mut_ptr());
                    if !self.confirmed_working {
                        tracing::warn!(
                            backend = self.hw_backend_name.unwrap_or("hwdec"),
                            error = %ffmpeg_next::Error::from(ret),
                            "Hardware frame transfer failed on initial frame; falling back to software decoding"
                        );
                        self.fallback_to_software_and_replay()?;
                        if self.saw_eof {
                            let _ = self.send_eof();
                        }
                        return self.receive_decoded_frame(out);
                    }
                    return Err(ffmpeg_next::Error::from(ret));
                }

                let _ = av_frame_copy_props(
                    self.sw_transfer_frame.as_mut_ptr(),
                    self.raw_decoded.as_ptr(),
                );
                (*self.sw_transfer_frame.as_mut_ptr()).width = (*self.raw_decoded.as_ptr()).width;
                (*self.sw_transfer_frame.as_mut_ptr()).height = (*self.raw_decoded.as_ptr()).height;
                av_frame_unref(self.raw_decoded.as_mut_ptr());
                av_frame_unref(out.as_mut_ptr());
                av_frame_move_ref(out.as_mut_ptr(), self.sw_transfer_frame.as_mut_ptr());
            }
        } else {
            // FFmpeg negotiated a software pixel format inside `get_hw_format`.
            unsafe {
                av_frame_unref(out.as_mut_ptr());
                av_frame_move_ref(out.as_mut_ptr(), self.raw_decoded.as_mut_ptr());
            }
        }

        if !self.confirmed_working {
            self.confirmed_working = true;
            self.startup_packets.clear();
            self.startup_packets.shrink_to_fit();
        }
        Ok(true)
    }
}

impl Drop for VideoDecoderSession {
    fn drop(&mut self) {
        self.release_hw_resources();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_test_video_packets() -> (ffmpeg_next::codec::Parameters, Vec<ffmpeg_next::Packet>) {
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
        enc_ctx.set_gop(30);
        enc_ctx.set_max_b_frames(0);
        let mut encoder = enc_ctx.open_as(codec).unwrap();

        let mut packets = Vec::new();
        for i in 0..4i64 {
            let mut frame =
                ffmpeg_next::frame::Video::new(ffmpeg_next::format::Pixel::YUV420P, 64, 64);
            frame.set_pts(Some(i));
            frame.data_mut(0).fill(32 + (i as u8) * 30);
            frame.data_mut(1).fill(128);
            frame.data_mut(2).fill(128);
            encoder.send_frame(&frame).unwrap();
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
        assert!(!packets.is_empty());
        let params = ffmpeg_next::codec::Parameters::from(encoder);
        (params, packets)
    }

    #[test]
    fn test_is_hwdec_eligible_codec() {
        use ffmpeg_next::codec::Id;
        assert!(is_hwdec_eligible_codec(Id::H264));
        assert!(is_hwdec_eligible_codec(Id::HEVC));
        assert!(is_hwdec_eligible_codec(Id::AV1));
        assert!(is_hwdec_eligible_codec(Id::VP9));
        assert!(is_hwdec_eligible_codec(Id::VP8));
        assert!(is_hwdec_eligible_codec(Id::MPEG2VIDEO));
        assert!(is_hwdec_eligible_codec(Id::VC1));
        assert!(is_hwdec_eligible_codec(Id::MJPEG));

        assert!(!is_hwdec_eligible_codec(Id::RAWVIDEO));
        assert!(!is_hwdec_eligible_codec(Id::PNG));
        assert!(!is_hwdec_eligible_codec(Id::PCM_S16LE));
    }

    #[test]
    fn test_get_hw_format_selects_matching_hw_or_first_software_format() {
        ffmpeg_next::init().unwrap();
        let offered = [
            AVPixelFormat::AV_PIX_FMT_CUDA,
            AVPixelFormat::AV_PIX_FMT_VAAPI,
            AVPixelFormat::AV_PIX_FMT_YUV420P,
            AVPixelFormat::AV_PIX_FMT_NONE,
        ];

        let chosen_hw =
            unsafe { select_hw_or_sw_format(AVPixelFormat::AV_PIX_FMT_VAAPI, offered.as_ptr()) };
        assert_eq!(chosen_hw, AVPixelFormat::AV_PIX_FMT_VAAPI);

        // When target hardware format (DRM_PRIME) is not offered, foreign hwaccel formats
        // (CUDA, VAAPI) must be skipped in favor of the first software format (YUV420P).
        let chosen_sw = unsafe {
            select_hw_or_sw_format(AVPixelFormat::AV_PIX_FMT_DRM_PRIME, offered.as_ptr())
        };
        assert_eq!(chosen_sw, AVPixelFormat::AV_PIX_FMT_YUV420P);
    }

    #[test]
    fn test_video_decoder_session_software_and_hwdec_probe() {
        let (params, packets) = encode_test_video_packets();

        for allow_hwdec in [false, true] {
            let mut session = VideoDecoderSession::open(params.clone(), allow_hwdec).unwrap();
            if !allow_hwdec {
                assert!(!session.is_hardware());
                assert!(session.hw_backend_name().is_none());
            }

            let mut out = ffmpeg_next::frame::Video::empty();
            let mut decoded_count = 0usize;
            for pkt in &packets {
                session.send_packet(pkt).unwrap();
                while session.receive_decoded_frame(&mut out).unwrap() {
                    assert_eq!((out.width(), out.height()), (64, 64));
                    decoded_count += 1;
                }
            }
            session.send_eof().unwrap();
            while session.receive_decoded_frame(&mut out).unwrap() {
                assert_eq!((out.width(), out.height()), (64, 64));
                decoded_count += 1;
            }
            assert_eq!(decoded_count, 4);
        }
    }

    #[test]
    fn test_seamless_runtime_software_fallback_recovers_initial_frames() {
        let (params, packets) = encode_test_video_packets();

        // 1. Simulate hardware decoder rejecting the very first packet in `send_packet`.
        let mut session_send_fail = VideoDecoderSession::open(params.clone(), false).unwrap();
        session_send_fail.hw_backend_name = Some("SIMULATED_VAAPI");
        session_send_fail.confirmed_working = false;
        session_send_fail.sim_fail_on_send = true;
        assert!(session_send_fail.is_hardware());

        let mut out = ffmpeg_next::frame::Video::empty();
        let mut count_send_fail = 0usize;
        for pkt in &packets {
            session_send_fail.send_packet(pkt).unwrap();
            while session_send_fail.receive_decoded_frame(&mut out).unwrap() {
                count_send_fail += 1;
            }
        }
        session_send_fail.send_eof().unwrap();
        while session_send_fail.receive_decoded_frame(&mut out).unwrap() {
            count_send_fail += 1;
        }
        assert!(!session_send_fail.is_hardware());
        assert_eq!(count_send_fail, 4);

        // 2. Simulate hardware surface transfer (`av_hwframe_transfer_data`) failing on frame 0
        // after packets were already queued into the hardware decoder.
        let mut session_xfer_fail = VideoDecoderSession::open(params, false).unwrap();
        session_xfer_fail.hw_backend_name = Some("SIMULATED_VAAPI");
        session_xfer_fail.confirmed_working = false;
        session_xfer_fail.sim_fail_on_transfer = true;

        let mut count_xfer_fail = 0usize;
        for pkt in &packets {
            session_xfer_fail.send_packet(pkt).unwrap();
            while session_xfer_fail.receive_decoded_frame(&mut out).unwrap() {
                count_xfer_fail += 1;
            }
        }
        session_xfer_fail.send_eof().unwrap();
        while session_xfer_fail.receive_decoded_frame(&mut out).unwrap() {
            count_xfer_fail += 1;
        }
        assert!(!session_xfer_fail.is_hardware());
        assert_eq!(
            count_xfer_fail, 4,
            "all 4 frames including initial IDR keyframe must be replayed on software fallback"
        );
    }
}
