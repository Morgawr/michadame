use crate::video::types::{RawFrame, VideoFormat};
use anyhow::{anyhow, Context, Result};
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use std::thread;

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
pub fn video_thread_main(
    frame_sender: crossbeam_channel::Sender<Arc<RawFrame>>,
    device: String,
    format: VideoFormat,
    resolution: (u32, u32),
    framerate: u32,
    scaler_filter: Arc<AtomicU8>,
    color_range: Arc<AtomicU8>,
) -> Result<()> {
    ffmpeg_next::init().context("Failed to initialize FFmpeg")?;
    let (_pixel_format, mut ffmpeg_options) = setup_ffmpeg_options(&format, resolution, framerate);

    // Initial color range setup
    let initial_range = color_range.load(Ordering::Relaxed);
    if initial_range == 1 {
        ffmpeg_options.set("color_range", "tv");
    } else {
        ffmpeg_options.set("color_range", "pc");
    }

    tracing::info!(device = %device, options = ?ffmpeg_options, "Starting FFmpeg with options");
    let ictx = ffmpeg_next::format::input_with_dictionary(&device, ffmpeg_options)
        .context("Failed to open input device with ffmpeg")?;

    let input = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .context("Could not find best video stream")?;
    let video_stream_index = input.index();

    let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(input.parameters())
        .and_then(|c| c.decoder().video())
        .context("Failed to create software video decoder")?;

    decoder.set_threading(ffmpeg_next::codec::threading::Config::default());
    let (packet_tx, packet_rx) = crossbeam_channel::bounded(1);
    let _reader_thread = thread::spawn(move || {
        let mut ictx = ictx;
        for (stream, packet) in ictx.packets() {
            if stream.index() == video_stream_index {
                if let Err(crossbeam_channel::TrySendError::Disconnected(_)) =
                    packet_tx.try_send(packet)
                {
                    break;
                }
            }
        }
        tracing::debug!("Packet reader thread finished.");
    });

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

        if let Ok(packet) = packet_rx.recv() {
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

                if let Err(crossbeam_channel::TrySendError::Disconnected(_)) =
                    frame_sender.try_send(raw_frame)
                {
                    return Ok(());
                }
            }
        } else {
            // packet_rx disconnected
            break;
        }
    }
    tracing::debug!("Video thread finished.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
