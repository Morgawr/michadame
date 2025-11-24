use crate::video::types::VideoFormat;
use anyhow::{Context, Result};
use eframe::egui;
use ffmpeg_next::format::Pixel;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;

pub fn video_thread_main(
    frame_sender: crossbeam_channel::Sender<Arc<egui::ColorImage>>,
    stop_flag: Arc<AtomicBool>,
    device: String,
    format: VideoFormat,
    resolution: (u32, u32),
    framerate: u32,
) -> Result<()> {
    ffmpeg_next::init().context("Failed to initialize FFmpeg")?;
    
    let mut pixel_format_str = format.fourcc.trim_end_matches('\0').to_lowercase();
    match pixel_format_str.as_str() {
        "yuyv" => pixel_format_str = "yuyv422".to_string(),
        "mjpg" => pixel_format_str = "mjpeg".to_string(),
        _ => {}
    }
    
    let mut ffmpeg_options = ffmpeg_next::Dictionary::new();
    ffmpeg_options.set("video_size", &format!("{}x{}", resolution.0, resolution.1));
    ffmpeg_options.set("framerate", &framerate.to_string());
    ffmpeg_options.set("f", "v4l2");
    ffmpeg_options.set("fflags", "nobuffer+discardcorrupt");
    ffmpeg_options.set("probesize", "32");
    ffmpeg_options.set("analyzeduration", "100000");

    if pixel_format_str == "mjpeg" {
        ffmpeg_options.set("input_format", "mjpeg");
    } else {
        ffmpeg_options.set("input_format", "rawvideo");
        ffmpeg_options.set("pixel_format", &pixel_format_str);
    }

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

    let mut scaler = None;
    while !stop_flag.load(Ordering::Relaxed) {
        if let Ok(packet) = packet_rx.try_recv() {
            decoder.send_packet(&packet).context("Failed to send packet to decoder")?;
            let mut decoded = ffmpeg_next::frame::Video::empty();
            while decoder.receive_frame(&mut decoded).is_ok() {
                let frame_to_process = &decoded;

                let scaler = scaler.get_or_insert_with(|| {
                    ffmpeg_next::software::scaling::context::Context::get(
                        frame_to_process.format(), 
                        frame_to_process.width(), 
                        frame_to_process.height(),
                        Pixel::RGB24, decoded.width(), decoded.height(),
                        ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR,
                    ).unwrap()
                });
                let mut rgb_frame = ffmpeg_next::frame::Video::empty();
                scaler.run(frame_to_process, &mut rgb_frame).context("Scaler failed")?;
                
                let image_data = rgb_frame.data(0);
                let image = Arc::new(egui::ColorImage::from_rgb([rgb_frame.width() as usize, rgb_frame.height() as usize], image_data));

                if frame_sender.try_send(image).is_err() {
                    break;
                }
            }
        } else {
            thread::yield_now();
        }
    }
    tracing::info!("Video thread finished.");
    Ok(())
}