use crate::app::models::{AppState, PendingAudioStream};
use crate::{devices, video};
use eframe::egui;
use std::sync::Arc;
use std::thread;

use video::decoder::VideoThreadEvent;

impl AppState {
    pub fn start_stream(&mut self, ctx: &egui::Context) {
        if self.hardware.active_audio_stream.is_some() || self.video_thread.is_some() {
            tracing::warn!("Stream already active, ignoring start request.");
            return;
        }

        let mic = if let Some(mic) = &self.hardware.selected_audio_source_name {
            mic.clone()
        } else {
            self.error("Cannot start: Missing audio input device.");
            return;
        };

        let format = if let Some(f) = self
            .hardware
            .supported_formats
            .get(self.hardware.selected_format_index)
        {
            f.clone()
        } else {
            self.error("Cannot start: No video format selected.");
            return;
        };

        let resolution = self.hardware.selected_resolution;
        if resolution.0 == 0 || resolution.1 == 0 {
            self.error("Cannot start: No video resolution selected.");
            return;
        }

        let framerate = self.hardware.selected_framerate;
        if framerate == 0 {
            self.error("Cannot start: No video framerate selected.");
            return;
        }

        self.pending_audio_stream = Some(PendingAudioStream {
            source_name: mic,
            buffer_size: self.hardware.audio_buffer_size,
            sample_rate: self.hardware.audio_sample_rate,
            sample_format: self.hardware.audio_sample_format.clone(),
        });
        self.latest_frame = None;

        let new_size = egui::vec2(resolution.0 as f32, resolution.1 as f32);
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::InnerSize(new_size),
        );
        ctx.request_repaint();

        let device = self.hardware.selected_video_device.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (status_tx, status_rx) = crossbeam_channel::unbounded();
        self.frame_receiver = Some(rx);
        self.video_status_receiver = Some(status_rx);

        let scaler_filter = self.scaler_filter.clone();
        let color_range = self.color_range.clone();

        let video_config = video::decoder::VideoThreadConfig {
            device,
            format,
            resolution,
            framerate,
            scaler_filter,
            color_range,
        };
        let handle = thread::spawn(move || {
            let result = video::decoder::video_thread_main(tx, status_tx.clone(), video_config);

            match result {
                Ok(()) => {
                    let _ = status_tx.send(VideoThreadEvent::Stopped);
                }
                Err(e) => {
                    let error = format!("{e:#}");
                    tracing::error!("Video thread error: {}", error);
                    let _ = status_tx.send(VideoThreadEvent::Failed(error));
                }
            }
        });
        self.video_thread = Some(handle);
        self.ui.video_window_open = true;
        self.ui.control_window_open = false;
    }

    pub fn handle_video_thread_events(&mut self, ctx: &egui::Context) -> bool {
        let mut handled_event = false;

        while let Some(rx) = self.video_status_receiver.as_ref() {
            let event = match rx.try_recv() {
                Ok(event) => event,
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.fail_stream_start(
                        ctx,
                        "Video thread ended without reporting its status.".to_string(),
                    );
                    handled_event = true;
                    break;
                }
            };

            handled_event = true;
            match event {
                VideoThreadEvent::Started => self.finish_stream_start(ctx),
                VideoThreadEvent::Failed(error) => {
                    self.fail_stream_start(ctx, format!("Failed to start video stream: {error}"));
                }
                VideoThreadEvent::Stopped => {
                    self.fail_stream_start(ctx, "Video stream stopped unexpectedly.".to_string());
                }
            }

            if self.video_status_receiver.is_none() {
                break;
            }
        }

        handled_event
    }

    fn finish_stream_start(&mut self, ctx: &egui::Context) {
        let Some(audio) = self.pending_audio_stream.take() else {
            return;
        };

        match devices::audio::start_audio_stream(
            &audio.source_name,
            Arc::clone(&self.hardware.audio_peak_amplitude),
            Arc::clone(&self.hardware.audio_latency_ms),
            audio.buffer_size,
            audio.sample_rate,
            audio.sample_format,
        ) {
            Ok(handle) => {
                self.hardware.active_audio_stream = Some(handle);
                self.info("Stream started.");
                self.fullscreen_toggle_frame_count = Some(0);
            }
            Err(e) => {
                self.fail_stream_start(ctx, format!("Failed to start audio stream: {e}"));
            }
        }
    }

    fn fail_stream_start(&mut self, ctx: &egui::Context, error: String) {
        self.frame_receiver = None;
        self.video_status_receiver = None;
        self.pending_audio_stream = None;
        self.hardware.active_audio_stream = None;

        if let Some(handle) = self.video_thread.take() {
            let _ = handle.join();
        }

        self.latest_frame = None;
        self.fullscreen_toggle_frame_count = None;
        self.ui.video_window_open = false;
        self.ui.control_window_open = true;
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Fullscreen(false),
        );
        self.error(error);
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
        self.video_status_receiver = None;
        self.pending_audio_stream = None;

        if let Some(handle) = self.video_thread.take() {
            let _ = handle.join();
        }

        if self.hardware.active_audio_stream.take().is_some() {
            self.info("Stream stopped and audio stream dropped.");
        } else {
            self.info("Stream stopped.");
        }

        self.frame_receiver = None;
        self.latest_frame = None;
        self.fullscreen_toggle_frame_count = None;
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
                    self.info("Audio stream restarted with new buffer size.");
                }
                Err(e) => {
                    self.error(format!("Failed to restart audio: {}", e));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_audio() -> PendingAudioStream {
        PendingAudioStream {
            source_name: "test-source".to_string(),
            buffer_size: 1024,
            sample_rate: 48_000,
            sample_format: "S16LE".to_string(),
        }
    }

    #[test]
    fn video_start_failure_restores_retryable_state() {
        let mut state = AppState::default();
        let (status_tx, status_rx) = crossbeam_channel::unbounded();
        let (_frame_tx, frame_rx) = crossbeam_channel::bounded(1);
        state.video_status_receiver = Some(status_rx);
        state.frame_receiver = Some(frame_rx);
        state.pending_audio_stream = Some(pending_audio());
        state.fullscreen_toggle_frame_count = Some(1);
        state.ui.video_window_open = true;
        state.ui.control_window_open = false;

        status_tx
            .send(VideoThreadEvent::Failed("capture unavailable".to_string()))
            .unwrap();

        assert!(state.handle_video_thread_events(&egui::Context::default()));
        assert!(state.video_status_receiver.is_none());
        assert!(state.frame_receiver.is_none());
        assert!(state.pending_audio_stream.is_none());
        assert!(state.hardware.active_audio_stream.is_none());
        assert!(state.video_thread.is_none());
        assert!(state.fullscreen_toggle_frame_count.is_none());
        assert!(!state.ui.video_window_open);
        assert!(state.ui.control_window_open);
    }

    #[test]
    fn disconnected_video_status_channel_restores_retryable_state() {
        let mut state = AppState::default();
        let (status_tx, status_rx) = crossbeam_channel::unbounded();
        state.video_status_receiver = Some(status_rx);
        state.pending_audio_stream = Some(pending_audio());
        state.ui.video_window_open = true;
        state.ui.control_window_open = false;
        drop(status_tx);

        assert!(state.handle_video_thread_events(&egui::Context::default()));
        assert!(state.video_status_receiver.is_none());
        assert!(state.pending_audio_stream.is_none());
        assert!(!state.ui.video_window_open);
        assert!(state.ui.control_window_open);
    }
}
