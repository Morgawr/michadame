use crate::app::models::AppState;
use crate::{devices, video};
use eframe::egui;
use std::sync::{Arc};
use std::thread;

impl AppState {
    pub fn start_stream(&mut self, ctx: &egui::Context) {
        if self.hardware.active_audio_stream.is_some() {
            tracing::warn!("Stream already active, ignoring start request.");
            return;
        }

        if let Some(mic) = &self.hardware.selected_audio_source_name {
            match devices::audio::start_audio_stream(
                mic, 
                Arc::clone(&self.hardware.audio_peak_amplitude),
                Arc::clone(&self.hardware.audio_latency_ms),
                self.hardware.audio_buffer_size,
                self.hardware.audio_sample_rate,
                self.hardware.audio_sample_format.clone(),
            ) {
                Ok(handle) => {
                    self.hardware.active_audio_stream = Some(handle);
                    self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(3)), text: "Audio stream started.".to_string().into() });
                }
                Err(e) => {
                    self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(3)), text: format!("Failed to start audio stream: {}", e).into() });
                    return;
                }
            }
        } else {
            self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(3)), text: "Cannot start: Missing audio input device.".to_string().into() });
            return;
        }

        let format = if let Some(f) = self.hardware.supported_formats.get(self.hardware.selected_format_index) {
            f
        } else {
            self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(3)), text: "Cannot start: No video format selected.".to_string().into() });
            return;
        };

        let resolution = self.hardware.selected_resolution;

        let new_size = egui::vec2(resolution.0 as f32, resolution.1 as f32);
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::InnerSize(new_size),
        );
        ctx.request_repaint();

        let device = self.hardware.selected_video_device.clone();
        let format = format.clone();
        let framerate = self.hardware.selected_framerate;
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.frame_receiver = Some(rx);

        let scaler_filter = self.scaler_filter.clone();
        let color_range = self.color_range.clone();

        let handle = thread::spawn(move || {
            if let Err(e) = video::decoder::video_thread_main(
                tx,
                device,
                format,
                resolution,
                framerate,
                scaler_filter,
                color_range,
            ) {
                tracing::error!("Video thread error: {}", e);
            }
        });
        self.video_thread = Some(handle);
        self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(3)), text: "Stream started.".to_string().into() });
        self.ui.video_window_open = true;
        self.ui.control_window_open = false;

        self.fullscreen_toggle_frame_count = Some(0);
    }

    pub fn stop_stream(&mut self, ctx: &egui::Context) {
        if self.ui.is_fullscreen {
            self.ui.is_fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        self.stop_stream_resources();
        if let Some(texture) = &mut self.video_texture {
            texture.set(
                egui::ImageData::Color(egui::ColorImage::new([1, 1], egui::Color32::BLACK).into()),
                egui::TextureOptions::LINEAR,
            );
        }
        self.ui.video_window_open = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    pub fn stop_stream_resources(&mut self) {
        self.frame_receiver = None;

        if let Some(handle) = self.video_thread.take() {
            let _ = handle.join();
        }

        if self.hardware.active_audio_stream.take().is_some() {
            self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(3)), text: "Stream stopped and audio stream dropped.".to_string().into() });
        } else {
            self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(3)), text: "Stream stopped.".to_string().into() });
        }

        self.frame_receiver = None;
        self.ui.video_window_open = false;
    }

    pub fn restart_audio_stream(&mut self) {
        self.hardware.active_audio_stream = None;
        
        if let Some(mic) = &self.hardware.selected_audio_source_name {
            match devices::audio::start_audio_stream(
                mic, 
                Arc::clone(&self.hardware.audio_peak_amplitude),
                Arc::clone(&self.hardware.audio_latency_ms),
                self.hardware.audio_buffer_size,
                self.hardware.audio_sample_rate,
                self.hardware.audio_sample_format.clone(),
            ) {
                Ok(handle) => {
                    self.hardware.active_audio_stream = Some(handle);
                    self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(3)), text: "Audio stream restarted with new buffer size.".to_string().into() });
                }
                Err(e) => {
                    self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Error, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(3)), text: format!("Failed to restart audio: {}", e).into() });
                }
            }
        }
    }
}
