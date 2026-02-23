use crate::{app::AppState, config};

pub fn init_app_state(cc: &eframe::CreationContext) -> AppState {
    let mut state = AppState::default();

    // Load fonts, styles, logo
    let ctx_clone = cc.egui_ctx.clone();
    // Use fallback styles since fonts aren't actually in the repo based on previous compilation errors.
    crate::ui::setup_style(&ctx_clone);
    // Load UI Logo Texture
    let logo_image =
        image::load_from_memory(include_bytes!("../assets/logo.png")).expect("Failed to load logo");
    let logo_size = [logo_image.width() as _, logo_image.height() as _];
    let logo_rgba = logo_image.to_rgba8();
    let logo_pixels = logo_rgba.as_flat_samples();
    let logo_color_image =
        eframe::egui::ColorImage::from_rgba_unmultiplied(logo_size, logo_pixels.as_slice());
    let logo_texture = cc
        .egui_ctx
        .load_texture("logo", logo_color_image, Default::default());
    state.logo_texture = Some(logo_texture);

    // Apply config
    let cfg = config::MichadameConfig::default();
    let loaded_cfg = confy::load::<config::MichadameConfig>("michadame", None).unwrap_or(cfg);
    config::apply_config(&mut state, &loaded_cfg);

    let (tx, _rx) = crossbeam_channel::unbounded();
    let (video_devices, pulse_sources, pulse_sinks, usb_devices) =
        crate::devices::scan_devices(None, None, tx);
    state.video_devices = video_devices;
    state.usb_devices = usb_devices;
    state.pulse_sources = pulse_sources;
    state.pulse_sinks = pulse_sinks;

    if !state.selected_video_device.is_empty() {
        match crate::devices::video::find_video_formats(&state.selected_video_device) {
            Ok(formats) => {
                state.supported_formats = formats;
                crate::video::types::apply_saved_format_config(&mut state, &loaded_cfg);
            }
            Err(e) => {
                tracing::error!("Device scan failed: {:?}", e);
            }
        }
    }

    let egui_ctx = cc.egui_ctx.clone();
    let (tx, rx) = crossbeam_channel::unbounded();
    state.device_scan_receiver = Some(rx);
    std::thread::spawn(move || {
        let video_result = crate::devices::video::find_video_devices();
        let pulse_result = crate::devices::audio::find_pulse_devices();
        let usb_result = crate::devices::usb::find_usb_devices();

        let result: crate::devices::DeviceScanResult = (|| {
            let video_devices = video_result?;
            let (pulse_sources, pulse_sinks) = pulse_result?;
            let usb_devices = usb_result?;
            Ok((video_devices, pulse_sources, pulse_sinks, usb_devices))
        })();

        if let Err(e) = &result {
            tracing::error!("Device scan failed: {:?}", e);
        };
        let _ = tx.send(result);
        egui_ctx.request_repaint();
    });

    // Pre-allocate the video texture
    let video_texture = {
        let tex_manager = cc.egui_ctx.tex_manager();
        let tex_id = tex_manager.write().alloc(
            "video_stream".to_string(),
            eframe::egui::ImageData::Color(
                eframe::egui::ColorImage::new([1, 1], eframe::egui::Color32::BLACK).into(),
            ),
            eframe::egui::TextureOptions::LINEAR,
        );
        eframe::egui::TextureHandle::new(tex_manager, tex_id)
    };
    state.video_texture = Some(video_texture);

    if let Some(gl) = cc.gl.as_ref() {
        state.crt_renderer = Some(std::sync::Arc::new(std::sync::Mutex::new(
            crate::video::gpu_filter::CrtFilterRenderer::new(gl),
        )));
    }

    state
}
