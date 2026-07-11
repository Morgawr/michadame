use crate::app::models::AppState;
use crate::config;
use eframe::egui;
use std::sync::{Arc, Mutex};

pub fn init_app_state(cc: &eframe::CreationContext) -> AppState {
    let mut state = AppState::default();

    let ctx_clone = cc.egui_ctx.clone();
    crate::ui::setup_style(&ctx_clone);

    let logo_image = image::load_from_memory(include_bytes!("../../assets/logo.png"))
        .expect("Failed to load logo");
    let logo_size = [logo_image.width() as _, logo_image.height() as _];
    let logo_rgba = logo_image.to_rgba8();
    let logo_pixels = logo_rgba.as_flat_samples();
    let logo_color_image =
        egui::ColorImage::from_rgba_unmultiplied(logo_size, logo_pixels.as_slice());
    let logo_texture = cc
        .egui_ctx
        .load_texture("logo", logo_color_image, Default::default());
    state.logo_texture = Some(logo_texture);

    let cfg = config::MichadameConfig::default();
    let loaded_cfg = confy::load::<config::MichadameConfig>("michadame", None).unwrap_or(cfg);
    config::apply_config(&mut state, &loaded_cfg);

    let egui_ctx = cc.egui_ctx.clone();
    let (tx, rx) = crossbeam_channel::unbounded();
    state.device_scan_receiver = Some(rx);
    std::thread::spawn(move || {
        let video_result = crate::devices::video::find_video_devices();
        let audio_result = crate::devices::audio::find_audio_devices();
        let usb_result = crate::devices::usb::find_usb_devices();

        let result: crate::devices::DeviceScanResult = (|| {
            let video_devices = video_result?;
            let audio_sources = audio_result?;
            let usb_devices = usb_result?;
            Ok((video_devices, audio_sources, usb_devices))
        })();

        if let Err(e) = &result {
            tracing::error!("Device scan failed: {:?}", e);
        };
        let _ = tx.send(result);
        egui_ctx.request_repaint();
    });

    let video_texture = {
        let tex_manager = cc.egui_ctx.tex_manager();
        let tex_id = tex_manager.write().alloc(
            "video_stream".to_string(),
            egui::ImageData::Color(egui::ColorImage::new([1, 1], egui::Color32::BLACK).into()),
            egui::TextureOptions::LINEAR,
        );
        egui::TextureHandle::new(tex_manager, tex_id)
    };
    state.video_texture = Some(video_texture);

    if let Some(gl) = cc.gl.as_ref() {
        state.crt_renderer = Some(Arc::new(Mutex::new(
            crate::video::gpu::CrtFilterRenderer::new(gl),
        )));
        state.fft_filter = Some(Arc::new(Mutex::new(crate::video::gpu::FftFilter::new(gl))));
    }

    state
}
