use crate::app::models::{AppState, PendingAudioStream};
use crate::{devices, video};
use eframe::egui;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
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
        // Give each stream its own counter: a retired worker may still be exiting.
        self.hardware.video_queue_drops = Arc::new(std::sync::atomic::AtomicU64::new(0));
        self.gui_fps = 0.0;
        self.video_fps = 0.0;
        self.frames_since_last_check = 0;
        self.video_frames_since_last_check = 0;
        self.last_fps_check = std::time::Instant::now();
        self.last_video_fps_check = self.last_fps_check;

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
        let stop_requested = Arc::new(AtomicBool::new(false));
        self.video_stop_requested = Some(Arc::clone(&stop_requested));
        let repaint_context = ctx.clone();
        let request_repaint: Arc<dyn Fn() + Send + Sync> =
            Arc::new(move || repaint_context.request_repaint());
        let notify_video_event = Arc::clone(&request_repaint);

        let video_config = video::decoder::VideoThreadConfig {
            queue_drops: self.hardware.video_queue_drops.clone(),
            device,
            format,
            resolution,
            framerate,
            scaler_filter,
            color_range,
            stop_requested,
            request_repaint,
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
            notify_video_event();
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
                    self.fail_stream_start(ctx, format!("Video stream failed: {error}"));
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
        self.replay.sync_audio_filter();
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
            repaint_request(ctx),
            self.replay.audio.clone(),
        ) {
            Ok(handle) => {
                handle.set_volume(self.hardware.master_volume);
                self.replay.set_game_volume(self.hardware.master_volume);
                self.hardware.active_audio_stream = Some(handle);
                self.info("Stream started.");
                self.fullscreen_toggle_frame_count = Some(0);
            }
            Err(e) => {
                self.fail_stream_start(ctx, format!("Failed to start audio stream: {e}"));
            }
        }
    }

    pub fn handle_audio_thread_events(&mut self, ctx: &egui::Context) -> bool {
        let failure = self
            .hardware
            .active_audio_stream
            .as_ref()
            .and_then(devices::audio::AudioStreamHandle::take_capture_failure);

        if let Some(error) = failure {
            self.fail_stream_start(ctx, format!("Audio stream stopped: {error}"));
            true
        } else {
            false
        }
    }

    fn fail_stream_start(&mut self, ctx: &egui::Context, error: String) {
        self.replay.disable();
        if let Some(stop_requested) = self.video_stop_requested.take() {
            stop_requested.store(true, Ordering::Relaxed);
        }
        self.frame_receiver = None;
        self.video_status_receiver = None;
        self.pending_audio_stream = None;
        self.hardware.active_audio_stream = None;

        if let Some(handle) = self.video_thread.take() {
            reap_video_thread(handle);
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
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
        ctx.set_cursor_icon(egui::CursorIcon::Default);
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
        self.ui.control_window_open = true;
        self.ui.show_stop_stream_dialog = false;
        ctx.request_repaint();
    }

    pub fn stop_stream_resources(&mut self) {
        self.replay.disable();
        if let Some(stop_requested) = self.video_stop_requested.take() {
            stop_requested.store(true, Ordering::Relaxed);
        }
        self.frame_receiver = None;
        self.video_status_receiver = None;
        self.pending_audio_stream = None;

        let audio_was_active = self.hardware.active_audio_stream.take().is_some();

        if let Some(handle) = self.video_thread.take() {
            reap_video_thread(handle);
        }

        self.stop_video_file();
        self.stop_gamescope_hook();
        self.stop_pc98_mode();
        self.stop_pc88_mode();

        if audio_was_active {
            self.info("Stream stopped and audio stream dropped.");
        } else {
            self.info("Stream stopped.");
        }

        self.frame_receiver = None;
        self.latest_frame = None;
        self.video_fps = 0.0;
        self.video_frames_since_last_check = 0;
        self.fullscreen_toggle_frame_count = None;
        self.ui.video_window_open = false;
    }

    pub fn stop_video_file(&mut self) {
        if self.video_player.is_some() {
            // Save video profile changes while video_player is still active
            self.video_profile = crate::config::build_profile_from_state(self);
            crate::config::save_config(self);
            crate::config::flush_pending_config(self);
        }

        if let Some(mut player) = self.video_player.take() {
            player.stop_requested.store(true, Ordering::Relaxed);
            let _ = player
                .command_sender
                .send(crate::video::player::PlayerCommand::Stop);
            if let Some(handle) = player.player_thread.take() {
                reap_video_thread(handle);
            }

            // Restore capture card resolution & framerate
            if self.previous_capture_resolution.0 > 0 {
                self.hardware.selected_resolution = self.previous_capture_resolution;
                self.hardware.selected_framerate = self.previous_capture_framerate;
            }

            // Restore normal profile
            if let Some(prev_name) = self.previous_normal_profile.take() {
                self.active_profile = prev_name;
            }
            let profile_to_apply = self.profiles.get(&self.active_profile).cloned();
            if let Some(profile) = profile_to_apply {
                crate::config::apply_profile_to_state(self, &profile);
            }
        }
        self.video_player_event_receiver = None;
        self.frame_receiver = None;
        self.latest_frame = None;
    }

    pub fn stop_gamescope_hook(&mut self) {
        if self.gamescope_hook.as_ref().map_or(false, |g| g.is_active) {
            // Save gamescope profile changes while gamescope_hook is still active
            self.gamescope_profile = crate::config::build_profile_from_state(self);
            crate::config::save_config(self);
            crate::config::flush_pending_config(self);
        }

        if let Some(mut hook) = self.gamescope_hook.take() {
            hook.is_active = false;
            drop(hook.dma_handle.take());
            drop(hook.egl_importer.take());
            if let Some(mut video_handle) = hook.video_handle.take() {
                video_handle.stop();
            }
            drop(hook.audio_handle.take());
            drop(hook.input_client.take());
            drop(hook.x11_cursor_monitor.take());

            // Restore normal profile
            if let Some(prev_name) = self.previous_gamescope_normal_profile.take() {
                self.active_profile = prev_name;
            }
            let profile_to_apply = self.profiles.get(&self.active_profile).cloned();
            if let Some(profile) = profile_to_apply {
                crate::config::apply_profile_to_state(self, &profile);
            }
            crate::gamescope::steam::restore_steam_launch_wrappers();
        }
        self.frame_receiver = None;
        self.latest_frame = None;
        self.ui.video_window_open = false;
    }

    pub fn refresh_steam_games(&mut self) {
        self.steam_games = crate::gamescope::steam::scan_steam_games();
        if let Some(ref selected) = self.selected_steam_game {
            if !self.steam_games.iter().any(|g| &g.appid == selected) {
                self.selected_steam_game = None;
            }
        }
        self.info(format!(
            "Found {} installed Steam games.",
            self.steam_games.len()
        ));
    }

    pub fn open_gamescope_script_dialog(&mut self, ctx: &egui::Context) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.gamescope_script_dialog_receiver = Some(rx);
        let ctx_clone = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("gamescope-script-picker".to_string())
            .spawn(move || {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Shell scripts", &["sh"])
                    .add_filter("All files", &["*"])
                    .set_title("Select Gamescope Launcher Script")
                    .pick_file()
                {
                    let _ = tx.send(path);
                    ctx_clone.request_repaint();
                }
            });
    }

    pub fn start_gamescope_hook(&mut self, ctx: &egui::Context) {
        self.stop_stream_resources();

        if let Some(selected_appid) = self.selected_steam_game.clone() {
            if let Some(game) = self
                .steam_games
                .iter()
                .find(|g| g.appid == selected_appid)
                .cloned()
            {
                let script = self.gamescope_launcher_script.clone();
                if let Err(e) = crate::gamescope::steam::launch_steam_game(&game, &script) {
                    self.error(format!("Failed to launch Steam game '{}': {e}", game.name));
                    return;
                }
                self.info(format!("Launching '{}' via Steam...", game.name));
            }
        }

        // Stash current normal profile and capture card parameters
        let current_normal = crate::config::build_profile_from_state(self);
        self.profiles
            .insert(self.active_profile.clone(), current_normal);
        self.previous_gamescope_normal_profile = Some(self.active_profile.clone());
        // Apply dedicated gamescope profile
        crate::config::apply_profile_to_state(self, &self.gamescope_profile.clone());

        let (frame_tx, frame_rx) = crossbeam_channel::bounded(2);
        self.frame_receiver = Some(frame_rx);
        self.latest_frame = None;
        self.hardware.video_queue_drops = Arc::new(std::sync::atomic::AtomicU64::new(0));
        self.gui_fps = 0.0;
        self.video_fps = 0.0;
        self.frames_since_last_check = 0;
        self.video_frames_since_last_check = 0;
        self.last_fps_check = std::time::Instant::now();
        self.last_video_fps_check = self.last_fps_check;
        self.ui.video_window_open = true;
        self.ui.control_window_open = false;
        self.fullscreen_toggle_frame_count = Some(0);

        let repaint_ctx = ctx.clone();
        let request_repaint = Arc::new(move || {
            repaint_ctx.request_repaint();
        });

        // 1. Start Gamescope video reader:
        // Try native zero-copy PipeWire DMA-BUF + EGL stream first.
        let dma_handle = crate::gamescope::pipewire_dma::start_gamescope_dma_stream(
            None,
            request_repaint.clone(),
            self.hardware.video_queue_drops.clone(),
        );
        let egl_importer = if dma_handle.is_some() {
            crate::gamescope::egl::EglDmaBufImporter::new()
                .map(|imp| std::sync::Arc::new(std::sync::Mutex::new(imp)))
        } else {
            None
        };

        let video_handle = if dma_handle.is_none() || egl_importer.is_none() {
            tracing::info!("Using fallback GStreamer pipewiresrc pipeline for Gamescope video");
            self.color_range.store(
                crate::video::types::ColorRange::Limited as u8,
                std::sync::atomic::Ordering::Relaxed,
            );
            match crate::gamescope::video::start_gamescope_video(
                frame_tx,
                request_repaint.clone(),
                self.color_range.clone(),
            ) {
                Ok(handle) => Some(handle),
                Err(e) => {
                    self.error(format!("Failed to connect to Gamescope video stream: {e}"));
                    None
                }
            }
        } else {
            tracing::info!(
                "Native PipeWire DMA-BUF zero-copy pipeline initialized for Gamescope video"
            );
            None
        };

        // 2. Start Gamescope audio monitor capture
        let audio_handle = match crate::gamescope::audio::start_gamescope_audio(
            self.hardware.audio_peak_amplitude.clone(),
            self.hardware.audio_latency_ms.clone(),
            self.hardware.audio_buffer_size,
            self.hardware.audio_sample_rate,
            self.hardware.audio_sample_format.clone(),
            request_repaint,
            self.replay.audio.clone(),
        ) {
            Ok(handle) => {
                handle.stream.set_volume(self.hardware.master_volume);
                self.replay.set_game_volume(self.hardware.master_volume);
                self.info("Gamescope audio hooked successfully.");
                Some(handle)
            }
            Err(e) => {
                self.error(format!("Failed to hook Gamescope audio: {e}"));
                None
            }
        };

        // 3. Connect EIS input client
        let input_client = match crate::gamescope::input::GamescopeInputClient::connect() {
            Ok(client) => Some(client),
            Err(e) => {
                tracing::warn!("Failed to initialize Gamescope EIS input client: {e}");
                None
            }
        };

        // 4. Initialize X11 cursor monitor for automatic camera capture
        let x11_cursor_monitor = if let Some(disp) =
            crate::gamescope::x11_cursor::find_gamescope_x_display()
        {
            tracing::info!("Found Gamescope X11 display: {disp}, starting cursor monitor");
            crate::gamescope::x11_cursor::GamescopeX11CursorMonitor::start(&disp)
        } else {
            tracing::warn!("Gamescope X11 display not detected; automatic cursor capture disabled");
            None
        };

        let initial_resolution = crate::gamescope::video::detect_gamescope_resolution();

        self.gamescope_hook = Some(crate::gamescope::GamescopeHookState {
            is_active: true,
            passthrough_enabled: true,
            dma_handle,
            egl_importer,
            current_dmabuf_frame: None,
            video_handle,
            audio_handle,
            input_client,
            x11_cursor_monitor,
            cursor_texture: None,
            prev_modifiers: eframe::egui::Modifiers::default(),
            initial_resolution,
            last_cursor_retry: Some(std::time::Instant::now()),
        });

        self.info("Gamescope hook started (4K 60Hz). Mouse/keyboard passthrough active (Shift+Tab to release).");
        ctx.request_repaint();
    }

    pub fn restart_audio_stream(&mut self, ctx: &egui::Context) {
        self.replay.disable();
        self.replay.sync_audio_filter();
        self.hardware.active_audio_stream = None;

        if let Some(mic) = &self.hardware.selected_audio_source_name {
            match devices::audio::start_audio_stream(
                mic,
                Arc::clone(&self.hardware.audio_peak_amplitude),
                Arc::clone(&self.hardware.audio_latency_ms),
                self.hardware.audio_buffer_size,
                self.hardware.audio_sample_rate,
                self.hardware.audio_sample_format.clone(),
                repaint_request(ctx),
                self.replay.audio.clone(),
            ) {
                Ok(handle) => {
                    handle.set_volume(self.hardware.master_volume);
                    self.replay.set_game_volume(self.hardware.master_volume);
                    self.hardware.active_audio_stream = Some(handle);
                    self.info("Audio stream restarted with new buffer size.");
                }
                Err(e) => {
                    self.error(format!("Failed to restart audio: {}", e));
                }
            }
        }
    }

    pub fn sync_mic_stream(&mut self) {
        if !self.mic.config.enabled {
            if self.mic.active_stream.is_some() {
                self.info("Microphone stream stopped.");
                self.mic.active_stream = None;
            }
            return;
        }

        let device_name = match &self.mic.config.device_name {
            Some(name) if !name.is_empty() => name.clone(),
            _ => {
                self.mic.active_stream = None;
                return;
            }
        };

        if let Some(stream) = &self.mic.active_stream {
            if stream.device_name == device_name {
                if let Ok(mut cfg) = stream.config.write() {
                    *cfg = self.mic.config.clone();
                }
                return;
            }
        }

        self.mic.active_stream = None;

        match crate::devices::mic::start_mic_stream(
            &device_name,
            self.mic.config.clone(),
            self.mic.current_level_db.clone(),
            self.mic.peak_level_db.clone(),
            self.mic.is_clipping.clone(),
            self.replay.audio.clone(),
        ) {
            Ok(handle) => {
                self.info(format!("Microphone capture started for {device_name}."));
                self.mic.active_stream = Some(handle);
            }
            Err(e) => {
                self.error(format!("Failed to start microphone ({device_name}): {e}"));
            }
        }
    }

    pub fn sync_webcam_stream(&mut self, ctx: Option<&egui::Context>) {
        if !self.webcam.config.enabled {
            if self.webcam.active_stream.is_some() {
                self.info("Webcam stream stopped.");
                self.webcam.active_stream = None;
                self.webcam.frame_receiver = None;
                self.webcam.latest_frame = None;
            }
            return;
        }

        let device_name = match &self.webcam.config.device {
            Some(dev) if !dev.is_empty() => dev.clone(),
            _ => {
                self.webcam.active_stream = None;
                self.webcam.frame_receiver = None;
                self.webcam.latest_frame = None;
                return;
            }
        };

        if let Some(ref saved_fourcc) = self.webcam.config.format_fourcc {
            if let Some(idx) = self
                .webcam
                .supported_formats
                .iter()
                .position(|f| &f.fourcc == saved_fourcc)
            {
                self.webcam.selected_format_index = idx;
            }
        }

        let format = if let Some(fmt) = self
            .webcam
            .supported_formats
            .get(self.webcam.selected_format_index)
        {
            fmt.clone()
        } else if let Some(ref saved_fourcc) = self.webcam.config.format_fourcc {
            crate::video::types::VideoFormat {
                fourcc: saved_fourcc.clone(),
                description: saved_fourcc.clone(),
                resolutions: Vec::new(),
            }
        } else {
            crate::video::types::VideoFormat::default()
        };

        let resolution = self.webcam.config.resolution.unwrap_or((640, 480));
        let framerate = self.webcam.config.framerate.unwrap_or(30);

        if let Some(stream) = &self.webcam.active_stream {
            if stream.device == device_name
                && stream.format_fourcc == format.fourcc
                && stream.resolution == resolution
                && stream.framerate == framerate
            {
                return;
            }
        }

        let prev_thread = self
            .webcam
            .active_stream
            .as_mut()
            .and_then(|s| s.take_stop_handle());
        self.webcam.active_stream = None;
        self.webcam.frame_receiver = None;
        self.webcam.latest_frame = None;

        let (tx, rx) = crossbeam_channel::bounded(1);
        self.webcam.frame_receiver = Some(rx);

        let repaint_ctx = ctx.cloned();
        let request_repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let Some(ctx) = &repaint_ctx {
                ctx.request_repaint();
            }
        });

        match crate::devices::webcam::start_webcam_stream(
            &device_name,
            format,
            resolution,
            framerate,
            tx,
            request_repaint,
            prev_thread,
        ) {
            Ok(handle) => {
                self.info(format!("Webcam capture started for {device_name}."));
                self.webcam.active_stream = Some(handle);
            }
            Err(e) => {
                self.error(format!("Failed to start webcam ({device_name}): {e}"));
            }
        }
    }

    pub fn start_video_file(&mut self, path: &std::path::Path, ctx: &egui::Context) {
        self.stop_stream_resources();

        // Probe video file container for native dimensions, duration, and framerate
        let probe = crate::video::player::probe_video_file(path).ok();
        let (file_width, file_height, file_duration, file_framerate) = if let Some(ref p) = probe {
            (p.width, p.height, p.duration_secs, p.framerate)
        } else {
            (640, 480, 0.0, 30.0)
        };

        // Stash current normal profile and capture card parameters
        let current_normal = crate::config::build_profile_from_state(self);
        self.profiles
            .insert(self.active_profile.clone(), current_normal);
        self.previous_normal_profile = Some(self.active_profile.clone());
        self.previous_capture_resolution = self.hardware.selected_resolution;
        self.previous_capture_framerate = self.hardware.selected_framerate;

        // Apply dedicated video profile (starts with default settings or previously saved video profile)
        crate::config::apply_profile_to_state(self, &self.video_profile.clone());

        // Base rendering resolution on the actual resolution of the file itself
        if file_width > 0 && file_height > 0 {
            self.hardware.selected_resolution = (file_width, file_height);
            self.hardware.selected_framerate = (file_framerate.round() as u32).max(1);
            let new_size = egui::vec2(file_width as f32, file_height as f32);
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::InnerSize(new_size),
            );
        }

        let (frame_tx, frame_rx) = crossbeam_channel::bounded(2);
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();

        let position_us = Arc::new(std::sync::atomic::AtomicI64::new(0));
        let is_paused = Arc::new(AtomicBool::new(false));
        let is_eof = Arc::new(AtomicBool::new(false));
        let stop_requested = Arc::new(AtomicBool::new(false));

        self.latest_frame = None;
        self.gui_fps = 0.0;
        self.video_fps = 0.0;
        self.frames_since_last_check = 0;
        self.video_frames_since_last_check = 0;
        self.last_fps_check = std::time::Instant::now();
        self.last_video_fps_check = self.last_fps_check;

        self.frame_receiver = Some(frame_rx);
        self.video_player_event_receiver = Some(event_rx);

        let repaint_context = ctx.clone();
        let request_repaint: Arc<dyn Fn() + Send + Sync> =
            Arc::new(move || repaint_context.request_repaint());
        let notify_event = Arc::clone(&request_repaint);

        self.replay.set_game_volume(self.hardware.master_volume);

        let config = crate::video::player::VideoFileConfig {
            file_path: path.to_path_buf(),
            scaler_filter: self.scaler_filter.clone(),
            color_range: self.color_range.clone(),
            stop_requested: Arc::clone(&stop_requested),
            request_repaint,
            peak_amplitude: Arc::clone(&self.hardware.audio_peak_amplitude),
            audio_latency_ms: Arc::clone(&self.hardware.audio_latency_ms),
            replay_audio: self.replay.audio.clone(),
            master_volume: self.hardware.master_volume,
        };

        let pos_clone = Arc::clone(&position_us);
        let paused_clone = Arc::clone(&is_paused);
        let eof_clone = Arc::clone(&is_eof);

        let event_tx_clone = event_tx.clone();
        let handle = thread::spawn(move || {
            let result = crate::video::player::video_file_thread_main(
                frame_tx,
                event_tx_clone.clone(),
                cmd_rx,
                pos_clone,
                paused_clone,
                eof_clone,
                config,
            );
            match result {
                Ok(()) => {
                    let _ = event_tx_clone.send(crate::video::player::PlayerEvent::Stopped);
                }
                Err(e) => {
                    let error = format!("{e:#}");
                    tracing::error!("Video file player error: {}", error);
                    let _ = event_tx_clone.send(crate::video::player::PlayerEvent::Failed(error));
                }
            }
            notify_event();
        });

        self.video_player = Some(crate::app::models::VideoPlayerState {
            file_path: path.to_path_buf(),
            duration_secs: file_duration,
            current_position_secs: 0.0,
            is_paused: false,
            is_eof: false,
            is_seeking: false,
            was_playing_before_scrub: false,
            command_sender: cmd_tx,
            position_us,
            is_paused_shared: is_paused,
            is_eof_shared: is_eof,
            stop_requested,
            file_width,
            file_height,
            player_thread: Some(handle),
        });

        self.replay.sync_audio_filter();
        self.ui.video_window_open = true;
        self.ui.control_window_open = false;
        self.fullscreen_toggle_frame_count = Some(0);
    }

    pub fn open_video_file_dialog(&mut self, ctx: &egui::Context) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.video_file_dialog_receiver = Some(rx);
        let ctx_clone = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("video-file-picker".to_string())
            .spawn(move || {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter(
                        "Video files",
                        &[
                            "mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "m4v", "ts", "mpg",
                            "mpeg", "vob", "ogv", "3gp",
                        ],
                    )
                    .add_filter("All files", &["*"])
                    .set_title("Open Video File")
                    .pick_file()
                {
                    let _ = tx.send(path);
                    ctx_clone.request_repaint();
                }
            });
    }

    pub fn handle_video_file_player_events(&mut self, ctx: &egui::Context) -> bool {
        let mut handled = false;
        while let Some(rx) = self.video_player_event_receiver.as_ref() {
            let event = match rx.try_recv() {
                Ok(event) => event,
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            };
            handled = true;
            match event {
                crate::video::player::PlayerEvent::Started {
                    width,
                    height,
                    duration_secs,
                    framerate,
                } => {
                    if let Some(ref mut p) = self.video_player {
                        p.duration_secs = duration_secs;
                        if width > 0 && height > 0 {
                            p.file_width = width;
                            p.file_height = height;
                        }
                    }
                    if width > 0 && height > 0 {
                        self.hardware.selected_resolution = (width, height);
                        self.hardware.selected_framerate = (framerate.round() as u32).max(1);
                        let new_size = egui::vec2(width as f32, height as f32);
                        ctx.send_viewport_cmd_to(
                            egui::ViewportId::ROOT,
                            egui::ViewportCommand::InnerSize(new_size),
                        );
                    }
                    self.fullscreen_toggle_frame_count = Some(0);
                    self.info("Video file playback started.");
                }
                crate::video::player::PlayerEvent::Failed(err) => {
                    self.error(format!("Video file playback failed: {err}"));
                    self.stop_video_file();
                    self.fullscreen_toggle_frame_count = None;
                    self.ui.is_fullscreen = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    self.ui.video_window_open = false;
                    self.ui.control_window_open = true;
                }
                crate::video::player::PlayerEvent::Eof => {
                    if let Some(ref mut p) = self.video_player {
                        p.is_eof = true;
                        p.is_paused = true;
                    }
                }
                crate::video::player::PlayerEvent::Stopped => {}
            }
        }
        handled
    }

    pub fn set_master_volume(&mut self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        self.hardware.master_volume = volume;
        if let Some(ref audio_stream) = self.hardware.active_audio_stream {
            audio_stream.set_volume(volume);
        }
        if let Some(ref gs) = self.gamescope_hook {
            if let Some(ref ah) = gs.audio_handle {
                ah.stream.set_volume(volume);
            }
        }
        if let Some(ref player) = self.video_player {
            let _ = player
                .command_sender
                .send(crate::video::player::PlayerCommand::SetVolume(volume));
        }
        if let Some(ref tx) = self.pc98.command_tx {
            let _ = tx.send(crate::pc98::Pc98Command::SetVolume(volume));
        }
        if let Some(ref tx) = self.pc88.command_tx {
            let _ = tx.send(crate::pc88::Pc88Command::SetVolume(volume));
        }
        self.replay.set_game_volume(volume);
        self.twitch.set_master_volume(volume);
    }

    pub fn start_pc98_mode(&mut self, ctx: &egui::Context) {
        self.stop_stream_resources();

        // Stash current normal profile and capture card parameters
        let current_normal = crate::config::build_profile_from_state(self);
        self.profiles
            .insert(self.active_profile.clone(), current_normal);
        self.previous_pc98_normal_profile = Some(self.active_profile.clone());
        self.previous_capture_resolution = self.hardware.selected_resolution;
        self.previous_capture_framerate = self.hardware.selected_framerate;

        // Apply dedicated PC-98 profile (defaults to Halo CRT + Retro PC Bezel)
        crate::config::apply_profile_to_state(self, &self.pc98_profile.clone());

        self.hardware.selected_resolution = (640, 400);
        self.hardware.selected_framerate = 56;
        let new_size = egui::vec2(800.0, 600.0);
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::InnerSize(new_size),
        );

        let (frame_tx, frame_rx) = crossbeam_channel::bounded(2);
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let stop_requested = Arc::new(AtomicBool::new(false));
        let input_state = Arc::new(std::sync::Mutex::new(crate::pc98::Pc98InputState::default()));

        self.latest_frame = None;
        self.gui_fps = 0.0;
        self.video_fps = 0.0;
        self.frames_since_last_check = 0;
        self.video_frames_since_last_check = 0;
        self.last_fps_check = std::time::Instant::now();
        self.last_video_fps_check = self.last_fps_check;

        self.frame_receiver = Some(frame_rx);
        self.pc98.event_rx = Some(event_rx);
        self.pc98.command_tx = Some(cmd_tx);
        self.pc98.stop_requested = Arc::clone(&stop_requested);
        self.pc98.input_state = Arc::clone(&input_state);
        self.pc98.is_active = true;
        self.pc98.passthrough_enabled = true;
        self.pc98.status_message = Some("Starting NEC PC-98 emulator...".to_string());
        self.pc98.resolution = (640, 400);

        let repaint_ctx = ctx.clone();
        let request_repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            repaint_ctx.request_repaint();
        });

        self.replay.set_game_volume(self.hardware.master_volume);
        self.replay.sync_audio_filter();

        let worker_params = crate::pc98::libretro::Pc98WorkerParams {
            config: self.pc98.config.clone(),
            frame_tx,
            event_tx: event_tx.clone(),
            command_rx: cmd_rx,
            input_state,
            stop_requested,
            request_repaint: request_repaint.clone(),
            peak_amplitude: Arc::clone(&self.hardware.audio_peak_amplitude),
            audio_latency_ms: Arc::clone(&self.hardware.audio_latency_ms),
            replay_audio: self.replay.audio.clone(),
            master_volume: self.hardware.master_volume,
        };

        let handle = thread::Builder::new()
            .name("pc98-libretro-worker".to_string())
            .spawn(move || {
                match crate::pc98::libretro::run_pc98_worker(worker_params) {
                    Ok(()) => {
                        let _ = event_tx.send(crate::pc98::Pc98Event::Stopped);
                    }
                    Err(e) => {
                        let _ = event_tx.send(crate::pc98::Pc98Event::Failed(format!("{e:#}")));
                    }
                }
                request_repaint();
            })
            .ok();

        self.pc98.worker_thread = handle;
        self.ui.video_window_open = true;
        self.ui.control_window_open = false;
        self.fullscreen_toggle_frame_count = Some(0);
        ctx.request_repaint();
    }

    pub fn stop_pc98_mode(&mut self) {
        if self.pc98.is_active {
            // Save PC-98 profile changes while PC-98 mode is still active
            self.pc98_profile = crate::config::build_profile_from_state(self);
            crate::config::save_config(self);
            crate::config::flush_pending_config(self);
        }

        self.pc98.request_stop();
        if let Some(tx) = self.pc98.command_tx.take() {
            let _ = tx.send(crate::pc98::Pc98Command::Stop);
        }
        if let Some(handle) = self.pc98.worker_thread.take() {
            reap_video_thread(handle);
        }

        if self.pc98.is_active {
            self.pc98.is_active = false;
            self.pc98.status_message = None;

            if self.previous_capture_resolution.0 > 0 {
                self.hardware.selected_resolution = self.previous_capture_resolution;
                self.hardware.selected_framerate = self.previous_capture_framerate;
            }
            if let Some(prev_name) = self.previous_pc98_normal_profile.take() {
                self.active_profile = prev_name;
            }
            let profile_to_apply = self.profiles.get(&self.active_profile).cloned();
            if let Some(profile) = profile_to_apply {
                crate::config::apply_profile_to_state(self, &profile);
            }
        }

        self.pc98.event_rx = None;
        self.frame_receiver = None;
        self.latest_frame = None;
    }

    pub fn handle_pc98_events(&mut self, ctx: &egui::Context) -> bool {
        let mut handled = false;
        if let Some(rx) = &self.pc98.file_picker_rx {
            if let Ok((slot, raw_path)) = rx.try_recv() {
                if slot == crate::pc98::Pc98MediaSlot::GamesDir {
                    self.pc98.config.games_dir = raw_path.to_string_lossy().to_string();
                    match self.pc98.refresh_games_list() {
                        Ok(count) => {
                            self.info(format!("Scanned PC-98 games folder: found {count} games."));
                        }
                        Err(e) => {
                            self.error(format!("Failed to scan PC-98 games folder: {e}"));
                        }
                    }
                    crate::config::save_config(self);
                    handled = true;
                } else {
                    let path = raw_path;
                    let path_str = path.to_string_lossy().to_string();
                    match slot {
                        crate::pc98::Pc98MediaSlot::Fdd1 => {
                            self.pc98.config.fdd1_path = path_str.clone();
                            self.pc98.send_command(crate::pc98::Pc98Command::MountFdd {
                                drive: 0,
                                path: Some(path),
                            });
                        }
                        crate::pc98::Pc98MediaSlot::Fdd2 => {
                            self.pc98.config.fdd2_path = path_str.clone();
                            self.pc98.send_command(crate::pc98::Pc98Command::MountFdd {
                                drive: 1,
                                path: Some(path),
                            });
                        }
                        crate::pc98::Pc98MediaSlot::Hdd1 => {
                            self.pc98.config.hdd1_path = path_str;
                            if self.pc98.is_active {
                                self.info("HDD1 updated. Reboot PC-98 mode to apply HDD changes.");
                            }
                        }
                        crate::pc98::Pc98MediaSlot::Hdd2 => {
                            self.pc98.config.hdd2_path = path_str;
                            if self.pc98.is_active {
                                self.info("HDD2 updated. Reboot PC-98 mode to apply HDD changes.");
                            }
                        }
                        crate::pc98::Pc98MediaSlot::CdRom => {
                            self.pc98.config.cdrom_path = path_str;
                            if self.pc98.is_active {
                                self.info(
                                    "CD-ROM updated. Reboot PC-98 mode to apply CD-ROM changes.",
                                );
                            }
                        }
                        crate::pc98::Pc98MediaSlot::DosHdd => {
                            self.pc98.config.dos_hdd_path = path_str.clone();
                            if let Some(sel_idx) = self
                                .pc98
                                .games_list
                                .iter()
                                .position(|g| g.id == self.pc98.config.selected_game)
                            {
                                if self.pc98.games_list[sel_idx].boot_mode.requires_dos_hdd() {
                                    self.pc98.config.hdd1_path = path_str;
                                }
                            }
                            self.info("Updated base bootable MS-DOS hard disk image.");
                        }
                        crate::pc98::Pc98MediaSlot::GamesDir => {}
                    }
                    crate::config::save_config(self);
                    handled = true;
                }
            }
        }

        while let Some(rx) = self.pc98.event_rx.as_ref() {
            let event = match rx.try_recv() {
                Ok(ev) => ev,
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            };
            handled = true;
            match event {
                crate::pc98::Pc98Event::SyncProgress(msg) => {
                    self.pc98.status_message = Some(msg.clone());
                    self.info(msg);
                }
                crate::pc98::Pc98Event::Started { width, height, fps } => {
                    self.pc98.status_message = None;
                    self.pc98.resolution = (width, height);
                    self.hardware.selected_resolution = (width, height);
                    self.hardware.selected_framerate = (fps.round() as u32).max(1);
                    if let Some(slot) = self.pc98.pending_boot_load_slot.take() {
                        self.pc98
                            .send_command(crate::pc98::Pc98Command::LoadState { slot });
                    } else {
                        self.info(
                            "NEC PC-98 booted! Keyboard/mouse passthrough active (Shift+Tab to release, F12 for NP2 menu).",
                        );
                    }
                }
                crate::pc98::Pc98Event::ResolutionChanged { width, height } => {
                    self.pc98.resolution = (width, height);
                    self.hardware.selected_resolution = (width, height);
                }
                crate::pc98::Pc98Event::StateSaved { slot, meta } => {
                    self.pc98
                        .slot_textures
                        .remove(&(meta.game_key.clone(), slot));
                    self.pc98.refresh_slots_cache();
                    self.info(format!(
                        "💾 Saved PC-98 state to Slot {slot} ({})",
                        meta.game_title
                    ));
                }
                crate::pc98::Pc98Event::StateLoaded { slot, meta } => {
                    if !meta.fdd1_path.is_empty() {
                        self.pc98.config.fdd1_path = meta.fdd1_path.clone();
                    }
                    if !meta.fdd2_path.is_empty() {
                        self.pc98.config.fdd2_path = meta.fdd2_path.clone();
                    }
                    self.info(format!(
                        "📂 Loaded PC-98 state from Slot {slot} ({})",
                        meta.game_title
                    ));
                }
                crate::pc98::Pc98Event::Info(msg) => {
                    self.info(msg);
                }
                crate::pc98::Pc98Event::Failed(err) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(
                        egui::CursorGrab::None,
                    ));
                    ctx.set_cursor_icon(egui::CursorIcon::Default);
                    self.fullscreen_toggle_frame_count = None;
                    self.ui.is_fullscreen = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    self.error(format!("PC-98 emulator error: {err}"));
                    self.stop_pc98_mode();
                    self.ui.video_window_open = false;
                    self.ui.control_window_open = true;
                }
                crate::pc98::Pc98Event::Stopped => {}
            }
        }
        if handled {
            ctx.request_repaint();
        }
        handled
    }

    pub fn open_pc98_media_dialog(
        &mut self,
        slot: crate::pc98::Pc98MediaSlot,
        ctx: &egui::Context,
    ) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.pc98.file_picker_rx = Some(rx);
        let ctx_clone = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("pc98-media-picker".to_string())
            .spawn(move || {
                let picked = match slot {
                    crate::pc98::Pc98MediaSlot::GamesDir => rfd::FileDialog::new()
                        .set_title("Select PC-98 Root Games Directory")
                        .pick_folder(),
                    crate::pc98::Pc98MediaSlot::Fdd1 | crate::pc98::Pc98MediaSlot::Fdd2 => {
                        rfd::FileDialog::new()
                            .add_filter(
                                "PC-98 Floppy Disk Images (.d88/.fdi/.hdm/.nfd)",
                                &[
                                    "d88", "88d", "fdi", "hdm", "xdf", "dup", "2hd", "tfd", "nfd",
                                    "flp", "fdd", "fim", "bin",
                                ],
                            )
                            .add_filter("All files", &["*"])
                            .set_title("Select PC-98 Floppy Disk Image")
                            .pick_file()
                    }
                    crate::pc98::Pc98MediaSlot::Hdd1
                    | crate::pc98::Pc98MediaSlot::Hdd2
                    | crate::pc98::Pc98MediaSlot::DosHdd => {
                        let title = if slot == crate::pc98::Pc98MediaSlot::DosHdd {
                            "Select Base Bootable MS-DOS Hard Disk Image"
                        } else {
                            "Select PC-98 Hard Disk Image"
                        };
                        rfd::FileDialog::new()
                            .add_filter(
                                "PC-98 Hard Disk Images (.hdi/.nhd/.thd)",
                                &["hdi", "thd", "nhd", "vhd", "slh", "hdn"],
                            )
                            .add_filter("All files", &["*"])
                            .set_title(title)
                            .pick_file()
                    }
                    crate::pc98::Pc98MediaSlot::CdRom => rfd::FileDialog::new()
                        .add_filter(
                            "CD-ROM Images (.cue/.ccd/.iso)",
                            &["cue", "ccd", "cdm", "mds", "nrg", "iso"],
                        )
                        .add_filter("All files", &["*"])
                        .set_title("Select PC-98 CD-ROM Image")
                        .pick_file(),
                };
                if let Some(path) = picked {
                    let _ = tx.send((slot, path));
                    ctx_clone.request_repaint();
                }
            });
    }

    pub fn start_pc88_mode(&mut self, ctx: &egui::Context) {
        self.stop_stream_resources();

        // Stash current normal profile and capture card parameters
        let current_normal = crate::config::build_profile_from_state(self);
        self.profiles
            .insert(self.active_profile.clone(), current_normal);
        self.previous_pc88_normal_profile = Some(self.active_profile.clone());
        self.previous_capture_resolution = self.hardware.selected_resolution;
        self.previous_capture_framerate = self.hardware.selected_framerate;

        // Apply dedicated PC-88 profile (defaults to Halo CRT + Retro PC Bezel)
        crate::config::apply_profile_to_state(self, &self.pc88_profile.clone());

        self.hardware.selected_resolution = (640, 400);
        self.hardware.selected_framerate = 56;
        let new_size = egui::vec2(800.0, 600.0);
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::InnerSize(new_size),
        );

        let (frame_tx, frame_rx) = crossbeam_channel::bounded(2);
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let stop_requested = Arc::new(AtomicBool::new(false));
        let input_state = Arc::new(std::sync::Mutex::new(crate::pc98::Pc98InputState::default()));

        self.latest_frame = None;
        self.gui_fps = 0.0;
        self.video_fps = 0.0;
        self.frames_since_last_check = 0;
        self.video_frames_since_last_check = 0;
        self.last_fps_check = std::time::Instant::now();
        self.last_video_fps_check = self.last_fps_check;

        self.frame_receiver = Some(frame_rx);
        self.pc88.event_rx = Some(event_rx);
        self.pc88.command_tx = Some(cmd_tx);
        self.pc88.stop_requested = Arc::clone(&stop_requested);
        self.pc88.input_state = Arc::clone(&input_state);
        self.pc88.is_active = true;
        self.pc88.passthrough_enabled = true;
        self.pc88.status_message = Some("Starting NEC PC-88 emulator...".to_string());
        self.pc88.resolution = (640, 400);

        let repaint_ctx = ctx.clone();
        let request_repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            repaint_ctx.request_repaint();
        });

        self.replay.set_game_volume(self.hardware.master_volume);
        self.replay.sync_audio_filter();

        let worker_params = crate::pc88::libretro::Pc88WorkerParams {
            config: self.pc88.config.clone(),
            active_game_floppies: self.pc88.active_game_floppies.clone(),
            frame_tx,
            event_tx: event_tx.clone(),
            command_rx: cmd_rx,
            input_state,
            stop_requested,
            request_repaint: request_repaint.clone(),
            peak_amplitude: Arc::clone(&self.hardware.audio_peak_amplitude),
            audio_latency_ms: Arc::clone(&self.hardware.audio_latency_ms),
            replay_audio: self.replay.audio.clone(),
            master_volume: self.hardware.master_volume,
        };

        let handle = thread::Builder::new()
            .name("pc88-libretro-worker".to_string())
            .spawn(move || {
                match crate::pc88::libretro::run_pc88_worker(worker_params) {
                    Ok(()) => {
                        let _ = event_tx.send(crate::pc88::Pc88Event::Stopped);
                    }
                    Err(e) => {
                        let _ = event_tx.send(crate::pc88::Pc88Event::Failed(format!("{e:#}")));
                    }
                }
                request_repaint();
            })
            .ok();

        self.pc88.worker_thread = handle;
        self.ui.video_window_open = true;
        self.ui.control_window_open = false;
        self.fullscreen_toggle_frame_count = Some(0);
        ctx.request_repaint();
    }

    pub fn stop_pc88_mode(&mut self) {
        if self.pc88.is_active {
            // Save PC-88 profile changes while PC-88 mode is still active
            self.pc88_profile = crate::config::build_profile_from_state(self);
            crate::config::save_config(self);
            crate::config::flush_pending_config(self);
        }

        self.pc88.request_stop();
        if let Some(tx) = self.pc88.command_tx.take() {
            let _ = tx.send(crate::pc88::Pc88Command::Stop);
        }
        if let Some(handle) = self.pc88.worker_thread.take() {
            reap_video_thread(handle);
        }

        if self.pc88.is_active {
            self.pc88.is_active = false;
            self.pc88.status_message = None;

            if self.previous_capture_resolution.0 > 0 {
                self.hardware.selected_resolution = self.previous_capture_resolution;
                self.hardware.selected_framerate = self.previous_capture_framerate;
            }
            if let Some(prev_name) = self.previous_pc88_normal_profile.take() {
                self.active_profile = prev_name;
            }
            let profile_to_apply = self.profiles.get(&self.active_profile).cloned();
            if let Some(profile) = profile_to_apply {
                crate::config::apply_profile_to_state(self, &profile);
            }
        }

        self.pc88.event_rx = None;
        self.frame_receiver = None;
        self.latest_frame = None;
    }

    pub fn handle_pc88_events(&mut self, ctx: &egui::Context) -> bool {
        let mut handled = false;
        if let Some(rx) = &self.pc88.file_picker_rx {
            if let Ok((slot, raw_path)) = rx.try_recv() {
                if slot == crate::pc88::Pc88MediaSlot::GamesDir {
                    self.pc88.config.games_dir = raw_path.to_string_lossy().to_string();
                    match self.pc88.refresh_games_list() {
                        Ok(count) => {
                            self.info(format!("Scanned PC-88 games folder: found {count} games."));
                        }
                        Err(e) => {
                            self.error(format!("Failed to scan PC-88 games folder: {e}"));
                        }
                    }
                    crate::config::save_config(self);
                    handled = true;
                } else {
                    let path = raw_path;
                    let path_str = path.to_string_lossy().to_string();
                    match slot {
                        crate::pc88::Pc88MediaSlot::Fdd1 => {
                            self.pc88.config.fdd1_path = path_str.clone();
                            self.pc88.send_command(crate::pc88::Pc88Command::MountFdd {
                                drive: 0,
                                path: Some(path),
                            });
                        }
                        crate::pc88::Pc88MediaSlot::Fdd2 => {
                            self.pc88.config.fdd2_path = path_str.clone();
                            self.pc88.send_command(crate::pc88::Pc88Command::MountFdd {
                                drive: 1,
                                path: Some(path),
                            });
                        }
                        crate::pc88::Pc88MediaSlot::Tape => {
                            self.pc88.config.tape_path = path_str;
                            if self.pc88.is_active {
                                self.info(
                                    "Tape image updated. Reboot PC-88 mode to apply tape changes.",
                                );
                            }
                        }
                        crate::pc88::Pc88MediaSlot::GamesDir => {}
                    }
                    crate::config::save_config(self);
                    handled = true;
                }
            }
        }

        while let Some(rx) = self.pc88.event_rx.as_ref() {
            let event = match rx.try_recv() {
                Ok(ev) => ev,
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            };
            handled = true;
            match event {
                crate::pc88::Pc88Event::SyncProgress(msg) => {
                    self.pc88.status_message = Some(msg.clone());
                    self.info(msg);
                }
                crate::pc88::Pc88Event::Started { width, height, fps } => {
                    self.pc88.status_message = None;
                    self.pc88.resolution = (width, height);
                    self.hardware.selected_resolution = (width, height);
                    self.hardware.selected_framerate = (fps.round() as u32).max(1);
                    if let Some(slot) = self.pc88.pending_boot_load_slot.take() {
                        self.pc88
                            .send_command(crate::pc88::Pc88Command::LoadState { slot });
                    } else {
                        self.info(
                            "NEC PC-88 booted! Keyboard/mouse passthrough active (Shift+Tab to release, F12 for Quasi88 menu).",
                        );
                    }
                }
                crate::pc88::Pc88Event::ResolutionChanged { width, height } => {
                    self.pc88.resolution = (width, height);
                    self.hardware.selected_resolution = (width, height);
                }
                crate::pc88::Pc88Event::StateSaved { slot, meta } => {
                    self.pc88
                        .slot_textures
                        .remove(&(meta.game_key.clone(), slot));
                    self.pc88.refresh_slots_cache();
                    self.info(format!(
                        "💾 Saved PC-88 state to Slot {slot} ({})",
                        meta.game_title
                    ));
                }
                crate::pc88::Pc88Event::StateLoaded { slot, meta } => {
                    if !meta.fdd1_path.is_empty() {
                        self.pc88.config.fdd1_path = meta.fdd1_path.clone();
                    }
                    if !meta.fdd2_path.is_empty() {
                        self.pc88.config.fdd2_path = meta.fdd2_path.clone();
                    }
                    self.info(format!(
                        "📂 Loaded PC-88 state from Slot {slot} ({})",
                        meta.game_title
                    ));
                }
                crate::pc88::Pc88Event::Info(msg) => {
                    self.info(msg);
                }
                crate::pc88::Pc88Event::Failed(err) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(
                        egui::CursorGrab::None,
                    ));
                    ctx.set_cursor_icon(egui::CursorIcon::Default);
                    self.fullscreen_toggle_frame_count = None;
                    self.ui.is_fullscreen = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    self.error(format!("PC-88 emulator error: {err}"));
                    self.stop_pc88_mode();
                    self.ui.video_window_open = false;
                    self.ui.control_window_open = true;
                }
                crate::pc88::Pc88Event::Stopped => {}
            }
        }
        if handled {
            ctx.request_repaint();
        }
        handled
    }

    pub fn open_pc88_media_dialog(
        &mut self,
        slot: crate::pc88::Pc88MediaSlot,
        ctx: &egui::Context,
    ) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.pc88.file_picker_rx = Some(rx);
        let ctx_clone = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("pc88-media-picker".to_string())
            .spawn(move || {
                let picked = match slot {
                    crate::pc88::Pc88MediaSlot::GamesDir => rfd::FileDialog::new()
                        .set_title("Select PC-88 Root Games Directory")
                        .pick_folder(),
                    crate::pc88::Pc88MediaSlot::Fdd1 | crate::pc88::Pc88MediaSlot::Fdd2 => {
                        rfd::FileDialog::new()
                            .add_filter(
                                "PC-88 Floppy Disk Images (.d88/.88d)",
                                &["d88", "88d", "d77", "2d"],
                            )
                            .add_filter("All files", &["*"])
                            .set_title("Select PC-88 Floppy Disk Image")
                            .pick_file()
                    }
                    crate::pc88::Pc88MediaSlot::Tape => rfd::FileDialog::new()
                        .add_filter("PC-88 Tape Images (.t88/.cmt)", &["t88", "cmt"])
                        .add_filter("All files", &["*"])
                        .set_title("Select PC-88 Tape Image")
                        .pick_file(),
                };
                if let Some(path) = picked {
                    let _ = tx.send((slot, path));
                    ctx_clone.request_repaint();
                }
            });
    }
}

fn repaint_request(ctx: &egui::Context) -> Arc<dyn Fn() + Send + Sync> {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

fn reap_video_thread(handle: thread::JoinHandle<()>) {
    if handle.is_finished() {
        if let Err(error) = handle.join() {
            tracing::warn!("Video thread join failed: {:?}", error);
        }
        return;
    }

    // A capture driver can remain stuck after the stop flag is set. Joining it on the UI thread
    // made the application itself unclosable during the exact failure we are trying to recover.
    if let Err(error) = thread::Builder::new()
        .name("video-capture-reaper".to_string())
        .spawn(move || {
            if let Err(error) = handle.join() {
                tracing::warn!("Video thread join failed: {:?}", error);
            }
        })
    {
        tracing::warn!("Could not start video capture reaper: {}", error);
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
    fn reaping_a_stuck_video_worker_does_not_block_the_caller() {
        let handle = thread::spawn(|| thread::sleep(std::time::Duration::from_secs(1)));
        let started = std::time::Instant::now();

        reap_video_thread(handle);

        assert!(started.elapsed() < std::time::Duration::from_millis(500));
    }

    #[test]
    fn video_start_failure_restores_retryable_state() {
        let mut state = AppState::default();
        let (status_tx, status_rx) = crossbeam_channel::unbounded();
        let (_frame_tx, frame_rx) = crossbeam_channel::bounded(1);
        let stop_requested = Arc::new(AtomicBool::new(false));
        state.video_status_receiver = Some(status_rx);
        state.frame_receiver = Some(frame_rx);
        state.video_stop_requested = Some(Arc::clone(&stop_requested));
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
        assert!(state.video_stop_requested.is_none());
        assert!(stop_requested.load(Ordering::Relaxed));
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

    #[test]
    fn stop_video_file_cleans_resources_and_resets_state() {
        let mut state = AppState::default();
        let (cmd_tx, _cmd_rx) = crossbeam_channel::unbounded();
        let (_frame_tx, frame_rx) = crossbeam_channel::bounded(1);
        let stop_requested = Arc::new(AtomicBool::new(false));

        state.frame_receiver = Some(frame_rx);
        state.video_player = Some(crate::app::models::VideoPlayerState {
            file_path: std::path::PathBuf::from("test.mp4"),
            duration_secs: 120.0,
            current_position_secs: 15.0,
            is_paused: false,
            is_eof: false,
            is_seeking: false,
            was_playing_before_scrub: false,
            command_sender: cmd_tx,
            position_us: Arc::new(std::sync::atomic::AtomicI64::new(15_000_000)),
            is_paused_shared: Arc::new(AtomicBool::new(false)),
            is_eof_shared: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::clone(&stop_requested),
            file_width: 640,
            file_height: 480,
            player_thread: None,
        });

        state.stop_video_file();

        assert!(state.video_player.is_none());
        assert!(state.frame_receiver.is_none());
        assert!(state.latest_frame.is_none());
        assert!(stop_requested.load(Ordering::Relaxed));
    }

    #[test]
    fn video_file_player_events_update_state() {
        let mut state = AppState::default();
        let (cmd_tx, _cmd_rx) = crossbeam_channel::unbounded();
        let (event_tx, event_rx) = crossbeam_channel::unbounded();

        state.video_player_event_receiver = Some(event_rx);
        state.video_player = Some(crate::app::models::VideoPlayerState {
            file_path: std::path::PathBuf::from("movie.mkv"),
            duration_secs: 0.0,
            current_position_secs: 0.0,
            is_paused: false,
            is_eof: false,
            is_seeking: false,
            was_playing_before_scrub: false,
            command_sender: cmd_tx,
            position_us: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            is_paused_shared: Arc::new(AtomicBool::new(false)),
            is_eof_shared: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            file_width: 640,
            file_height: 480,
            player_thread: None,
        });

        event_tx
            .send(crate::video::player::PlayerEvent::Started {
                width: 1280,
                height: 720,
                duration_secs: 300.0,
                framerate: 60.0,
            })
            .unwrap();

        assert!(state.handle_video_file_player_events(&egui::Context::default()));
        assert_eq!(state.video_player.as_ref().unwrap().duration_secs, 300.0);

        event_tx
            .send(crate::video::player::PlayerEvent::Eof)
            .unwrap();
        assert!(state.handle_video_file_player_events(&egui::Context::default()));
        assert!(state.video_player.as_ref().unwrap().is_eof);
        assert!(state.video_player.as_ref().unwrap().is_paused);
    }

    #[test]
    fn test_video_mode_profile_and_resolution_restoration() {
        let mut state = AppState::default();
        let dreamcast_profile = crate::config::Profile {
            crt_hard_scan: Some(-12.0),
            vibrance: Some(1.2),
            video_resolution: Some((640, 480)),
            ..Default::default()
        };
        state
            .profiles
            .insert("Dreamcast".to_string(), dreamcast_profile);
        state.active_profile = "Dreamcast".to_string();
        state.hardware.selected_resolution = (640, 480);
        state.hardware.selected_framerate = 60;
        let dreamcast = state.profiles.get("Dreamcast").unwrap().clone();
        crate::config::apply_profile_to_state(&mut state, &dreamcast);
        assert_eq!(state.crt.hard_scan, -12.0);
        assert_eq!(state.video.vibrance, 1.2);

        // Simulate entering video mode
        let current_normal = crate::config::build_profile_from_state(&mut state);
        state
            .profiles
            .insert(state.active_profile.clone(), current_normal);
        state.previous_normal_profile = Some(state.active_profile.clone());
        state.previous_capture_resolution = state.hardware.selected_resolution;
        state.previous_capture_framerate = state.hardware.selected_framerate;

        // Apply dedicated video profile (starts with defaults)
        let vid_prof = state.video_profile.clone();
        crate::config::apply_profile_to_state(&mut state, &vid_prof);
        assert_eq!(state.crt.hard_scan, -8.0); // Default CRT scanline
        assert_eq!(state.video.vibrance, 1.0); // Default vibrance

        // Native video resolution (e.g., 1920x1080)
        state.hardware.selected_resolution = (1920, 1080);
        state.hardware.selected_framerate = 30;

        let (cmd_tx, _cmd_rx) = crossbeam_channel::unbounded();
        state.video_player = Some(crate::app::models::VideoPlayerState {
            file_path: std::path::PathBuf::from("hd_video.mp4"),
            duration_secs: 60.0,
            current_position_secs: 0.0,
            is_paused: false,
            is_eof: false,
            is_seeking: false,
            was_playing_before_scrub: false,
            command_sender: cmd_tx,
            position_us: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            is_paused_shared: Arc::new(AtomicBool::new(false)),
            is_eof_shared: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            file_width: 1920,
            file_height: 1080,
            player_thread: None,
        });

        // User tweaks a setting while watching video
        state.video.vibrance = 1.6;

        // Stop video file
        state.stop_video_file();

        // 1. Video profile should have captured the video tweak
        assert_eq!(state.video_profile.vibrance, Some(1.6));

        // 2. Active profile should be restored to Dreamcast
        assert_eq!(state.active_profile, "Dreamcast");

        // 3. Hardware capture resolution and framerate should be restored
        assert_eq!(state.hardware.selected_resolution, (640, 480));
        assert_eq!(state.hardware.selected_framerate, 60);

        // 4. Dreamcast profile settings should be intact and restored
        assert_eq!(state.crt.hard_scan, -12.0);
        assert_eq!(state.video.vibrance, 1.2);
    }

    #[test]
    fn test_set_master_volume_clamps_and_syncs_replay() {
        let mut state = AppState::default();
        assert_eq!(state.hardware.master_volume, 1.0);
        state.set_master_volume(0.5);
        assert_eq!(state.hardware.master_volume, 0.5);
        assert_eq!(state.replay.audio.game_volume(), 0.5);
        assert_eq!(state.twitch.alert_player.volume(), 0.5);

        // Clamping bounds
        state.set_master_volume(-0.5);
        assert_eq!(state.hardware.master_volume, 0.0);
        assert_eq!(state.replay.audio.game_volume(), 0.0);
        assert_eq!(state.twitch.alert_player.volume(), 0.0);

        state.set_master_volume(1.5);
        assert_eq!(state.hardware.master_volume, 1.0);
        assert_eq!(state.replay.audio.game_volume(), 1.0);
        assert_eq!(state.twitch.alert_player.volume(), 1.0);
    }

    #[test]
    fn stop_gamescope_hook_cleans_resources_and_closes_video_window() {
        let mut state = AppState::default();
        let (_frame_tx, frame_rx) = crossbeam_channel::bounded(1);

        state.ui.video_window_open = true;
        state.frame_receiver = Some(frame_rx);
        state.gamescope_hook = Some(crate::gamescope::GamescopeHookState {
            is_active: true,
            ..Default::default()
        });

        state.stop_gamescope_hook();

        assert!(state.gamescope_hook.is_none());
        assert!(state.frame_receiver.is_none());
        assert!(state.latest_frame.is_none());
        assert!(!state.ui.video_window_open);
    }

    #[test]
    fn test_pc98_mode_profile_and_resolution_restoration() {
        let mut state = AppState::default();
        // Default PC-98 profile has Halo CRT (2) and retro_pc_frame enabled
        assert_eq!(state.pc98_profile.crt_filter, Some(2));
        assert_eq!(state.pc98_profile.retro_pc_frame, Some(true));

        let saturn_profile = crate::config::Profile {
            crt_hard_scan: Some(-10.0),
            vibrance: Some(1.3),
            video_resolution: Some((720, 480)),
            ..Default::default()
        };
        state.profiles.insert("Saturn".to_string(), saturn_profile);
        state.active_profile = "Saturn".to_string();
        state.hardware.selected_resolution = (720, 480);
        state.hardware.selected_framerate = 60;
        let saturn = state.profiles.get("Saturn").unwrap().clone();
        crate::config::apply_profile_to_state(&mut state, &saturn);

        // Simulate entering PC-98 mode
        let current_normal = crate::config::build_profile_from_state(&state);
        state
            .profiles
            .insert(state.active_profile.clone(), current_normal);
        state.previous_pc98_normal_profile = Some(state.active_profile.clone());
        state.previous_capture_resolution = state.hardware.selected_resolution;
        state.previous_capture_framerate = state.hardware.selected_framerate;

        let pc98_prof = state.pc98_profile.clone();
        crate::config::apply_profile_to_state(&mut state, &pc98_prof);
        assert!(state.video.retro_pc_frame);

        state.pc98.is_active = true;
        state.hardware.selected_resolution = (640, 400);
        state.hardware.selected_framerate = 56;

        // User tweaks vibrance while in PC-98 mode
        state.video.vibrance = 1.45;

        state.stop_pc98_mode();

        assert!(!state.pc98.is_active);
        assert_eq!(state.pc98_profile.vibrance, Some(1.45));
        assert_eq!(state.active_profile, "Saturn");
        assert_eq!(state.hardware.selected_resolution, (720, 480));
        assert_eq!(state.hardware.selected_framerate, 60);
        assert_eq!(state.crt.hard_scan, -10.0);
        assert_eq!(state.video.vibrance, 1.3);
    }
}
