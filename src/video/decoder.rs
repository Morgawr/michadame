use crate::video::types::{VideoFormat, RawFrame};
use anyhow::{Context, Result};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
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
    stop_flag: Arc<AtomicBool>,
    device: String,
    format: VideoFormat,
    resolution: (u32, u32),
    framerate: u32,
) -> Result<()> {
    ffmpeg_next::init().context("Failed to initialize FFmpeg")?;
    let (_pixel_format, ffmpeg_options) = setup_ffmpeg_options(&format, resolution, framerate);

    tracing::info!(device = %device, options = ?ffmpeg_options, "Starting FFmpeg with options");
    let ictx = ffmpeg_next::format::input_with_dictionary(&device, ffmpeg_options)
        .context("Failed to open input device with ffmpeg")?;

    let input = ictx.streams().best(ffmpeg_next::media::Type::Video).context("Could not find best video stream")?;
    let video_stream_index = input.index();

    let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(input.parameters())
        .and_then(|c| c.decoder().video())
        .context("Failed to create software video decoder")?;

    decoder.set_threading(ffmpeg_next::codec::threading::Config::default());
    let (packet_tx, packet_rx) = crossbeam_channel::bounded(1);
    let reader_stop_flag = stop_flag.clone();
    let _reader_thread = thread::spawn(move || {
        let mut ictx = ictx;
        for (stream, packet) in ictx.packets() {
            if reader_stop_flag.load(Ordering::Relaxed) { break; }
            if stream.index() == video_stream_index {
                let _ = packet_tx.try_send(packet);
            }
        }
        tracing::info!("Packet reader thread finished.");
    });

    let mut scaler = ffmpeg_next::software::scaling::context::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        decoder.format(),
        decoder.width(),
        decoder.height(),
        ffmpeg_next::software::scaling::flag::Flags::BILINEAR,
    ).context("Failed to create software scaler for normalization")?;

    while !stop_flag.load(Ordering::Relaxed) {
        if let Ok(packet) = packet_rx.recv() {
            decoder.send_packet(&packet).context("Failed to send packet to decoder")?;
            let mut decoded = ffmpeg_next::frame::Video::empty();
            while decoder.receive_frame(&mut decoded).is_ok() {
                let width = decoded.width();
                let height = decoded.height();
                let format = decoded.format();
                
                // Use the scaler to normalize the frame (removes strides and ensures consistent plane layout)
                let mut normalized = ffmpeg_next::frame::Video::empty();
                scaler.run(&decoded, &mut normalized).context("Failed to normalize video frame")?;

                let mut data = Vec::new();
                for i in 0..normalized.planes() {
                    data.extend_from_slice(normalized.data(i));
                }

                let raw_frame = Arc::new(RawFrame {
                    width,
                    height,
                    data,
                    format,
                });

                if frame_sender.try_send(raw_frame).is_err() {
                    break;
                }
            }
        }
    }
    tracing::info!("Video thread finished.");
    Ok(())
}