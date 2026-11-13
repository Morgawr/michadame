pub mod gpu;
pub mod model_sync;
pub mod rig;
pub mod tracker;
pub mod vrm;

use crate::config::VtuberConfig;
use eframe::egui;
use gpu::VtuberGpuRenderer;
use model_sync::{
    are_vtuber_models_installed, download_and_install_vtuber_models,
    resolve_vtuber_tracker_directory, VtuberModelSyncEvent,
};
use rig::RigAnimator;
use std::path::Path;
use std::sync::{atomic::Ordering, Arc, Mutex};
use std::thread;
use tracker::{
    start_vtuber_tracker, SharedTrackingState, VtuberTrackerHandle, VtuberTrackerStatus,
};
use vrm::loader::{load_vrm_from_path, DecodedRgbaTexture, LoadedVrmAsset};

#[derive(Debug, Clone)]
pub enum VtuberNotification {
    Info(String),
    Success(String),
    Error(String),
}

pub struct VtuberState {
    pub gpu: Arc<Mutex<VtuberGpuRenderer>>,
    pub rig_animator: RigAnimator,
    pub shared_tracking: Arc<Mutex<SharedTrackingState>>,
    pub tracker_handle: Option<VtuberTrackerHandle>,

    pub active_model_path: Option<String>,
    pub loading_model_path: Option<String>,
    pub avatar_title: Option<String>,
    pub model_error: Option<String>,
    vrm_load_rx: Option<crossbeam_channel::Receiver<(String, Result<LoadedVrmAsset, String>)>>,

    pub active_bg_path: Option<String>,
    pub loading_bg_path: Option<String>,
    pub bg_error: Option<String>,
    bg_load_rx: Option<crossbeam_channel::Receiver<(String, Result<DecodedRgbaTexture, String>)>>,

    pub sync_in_progress: bool,
    pub sync_progress: Option<(String, f32)>,
    pub sync_error: Option<String>,
    sync_event_rx: Option<crossbeam_channel::Receiver<VtuberModelSyncEvent>>,

    last_render_time_secs: Option<f64>,
}

impl Default for VtuberState {
    fn default() -> Self {
        Self::new()
    }
}

impl VtuberState {
    pub fn new() -> Self {
        Self {
            gpu: Arc::new(Mutex::new(VtuberGpuRenderer::default())),
            rig_animator: RigAnimator::new(),
            shared_tracking: Arc::new(Mutex::new(SharedTrackingState::default())),
            tracker_handle: None,
            active_model_path: None,
            loading_model_path: None,
            avatar_title: None,
            model_error: None,
            vrm_load_rx: None,
            active_bg_path: None,
            loading_bg_path: None,
            bg_error: None,
            bg_load_rx: None,
            sync_in_progress: false,
            sync_progress: None,
            sync_error: None,
            sync_event_rx: None,
            last_render_time_secs: None,
        }
    }

    /// Synchronizes asynchronous `.vrm` avatar and custom background image loading with `cfg`.
    pub fn sync_assets_with_config(
        &mut self,
        cfg: &VtuberConfig,
        repaint_ctx: Option<egui::Context>,
    ) {
        // 1. VRM Model Asset
        match cfg.model_path.as_deref().filter(|p| !p.trim().is_empty()) {
            None => {
                if self.active_model_path.is_some() || self.loading_model_path.is_some() {
                    self.active_model_path = None;
                    self.loading_model_path = None;
                    self.avatar_title = None;
                    self.model_error = None;
                    self.vrm_load_rx = None;
                    if let Ok(mut gpu) = self.gpu.lock() {
                        gpu.queue_clear_vrm();
                    }
                }
            }
            Some(path_str) => {
                let already_active = self.active_model_path.as_deref() == Some(path_str);
                let already_loading = self.loading_model_path.as_deref() == Some(path_str);
                if !already_active && !already_loading {
                    self.loading_model_path = Some(path_str.to_string());
                    self.model_error = None;
                    let (tx, rx) = crossbeam_channel::bounded(1);
                    self.vrm_load_rx = Some(rx);
                    let owned_path = path_str.to_string();
                    let ctx_clone = repaint_ctx.clone();
                    let _ = thread::Builder::new()
                        .name("vtuber-vrm-loader".to_string())
                        .spawn(move || {
                            let res = load_vrm_from_path(Path::new(&owned_path))
                                .map_err(|e| format!("{e:#}"));
                            let _ = tx.send((owned_path, res));
                            if let Some(ctx) = ctx_clone {
                                ctx.request_repaint();
                            }
                        });
                }
            }
        }

        // 2. Custom Static Background Image
        match cfg
            .background_image_path
            .as_deref()
            .filter(|p| !p.trim().is_empty())
        {
            None => {
                if self.active_bg_path.is_some() || self.loading_bg_path.is_some() {
                    self.active_bg_path = None;
                    self.loading_bg_path = None;
                    self.bg_error = None;
                    self.bg_load_rx = None;
                    if let Ok(mut gpu) = self.gpu.lock() {
                        gpu.queue_clear_bg();
                    }
                }
            }
            Some(bg_str) => {
                let already_active = self.active_bg_path.as_deref() == Some(bg_str);
                let already_loading = self.loading_bg_path.as_deref() == Some(bg_str);
                if !already_active && !already_loading {
                    self.loading_bg_path = Some(bg_str.to_string());
                    self.bg_error = None;
                    let (tx, rx) = crossbeam_channel::bounded(1);
                    self.bg_load_rx = Some(rx);
                    let owned_bg = bg_str.to_string();
                    let ctx_clone = repaint_ctx;
                    let _ = thread::Builder::new()
                        .name("vtuber-bg-loader".to_string())
                        .spawn(move || {
                            let res = decode_background_image_file(Path::new(&owned_bg));
                            let _ = tx.send((owned_bg, res));
                            if let Some(ctx) = ctx_clone {
                                ctx.request_repaint();
                            }
                        });
                }
            }
        }
    }

    /// Polls background asset loaders and ONNX tracker download events on the UI thread.
    pub fn poll_background_tasks(&mut self, cfg: &VtuberConfig) -> Vec<VtuberNotification> {
        let mut notes = Vec::new();

        if let Some(rx) = &self.vrm_load_rx {
            if let Ok((loaded_path, res)) = rx.try_recv() {
                self.vrm_load_rx = None;
                if self.loading_model_path.as_deref() == Some(loaded_path.as_str()) {
                    self.loading_model_path = None;
                }
                if cfg.model_path.as_deref() == Some(loaded_path.as_str()) {
                    match res {
                        Ok(asset) => {
                            let title = asset.title.clone();
                            let spec = if asset.is_vrm0 { "VRM 0.x" } else { "VRM 1.0" };
                            self.avatar_title = Some(title.clone());
                            self.active_model_path = Some(loaded_path);
                            self.model_error = None;
                            if let Ok(mut gpu) = self.gpu.lock() {
                                gpu.queue_vrm_upload(asset);
                            }
                            notes.push(VtuberNotification::Success(format!(
                                "Loaded VTuber avatar: {title} ({spec})"
                            )));
                        }
                        Err(err) => {
                            self.active_model_path = Some(loaded_path);
                            self.avatar_title = None;
                            self.model_error = Some(err.clone());
                            if let Ok(mut gpu) = self.gpu.lock() {
                                gpu.queue_clear_vrm();
                            }
                            notes.push(VtuberNotification::Error(format!(
                                "Failed to load VRM avatar: {err}"
                            )));
                        }
                    }
                }
            }
        }

        if let Some(rx) = &self.bg_load_rx {
            if let Ok((loaded_bg, res)) = rx.try_recv() {
                self.bg_load_rx = None;
                if self.loading_bg_path.as_deref() == Some(loaded_bg.as_str()) {
                    self.loading_bg_path = None;
                }
                if cfg.background_image_path.as_deref() == Some(loaded_bg.as_str()) {
                    match res {
                        Ok(tex) => {
                            self.active_bg_path = Some(loaded_bg);
                            self.bg_error = None;
                            if let Ok(mut gpu) = self.gpu.lock() {
                                gpu.queue_bg_upload(tex);
                            }
                            notes.push(VtuberNotification::Info(
                                "Loaded VTuber background image.".to_string(),
                            ));
                        }
                        Err(err) => {
                            self.active_bg_path = Some(loaded_bg);
                            self.bg_error = Some(err.clone());
                            if let Ok(mut gpu) = self.gpu.lock() {
                                gpu.queue_clear_bg();
                            }
                            notes.push(VtuberNotification::Error(format!(
                                "Failed to load VTuber background image: {err}"
                            )));
                        }
                    }
                }
            }
        }

        if let Ok(mut gpu) = self.gpu.lock() {
            if let Some(gpu_err) = gpu.take_gpu_error() {
                self.model_error = Some(gpu_err.clone());
                notes.push(VtuberNotification::Error(format!(
                    "VTuber GPU upload error: {gpu_err}"
                )));
            }
        }

        let mut clear_sync_rx = false;
        if let Some(rx) = &self.sync_event_rx {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    VtuberModelSyncEvent::Progress { message, progress } => {
                        self.sync_progress = Some((message, progress));
                    }
                    VtuberModelSyncEvent::Finished(Ok(_)) => {
                        self.sync_in_progress = false;
                        self.sync_progress = None;
                        self.sync_error = None;
                        clear_sync_rx = true;
                        if let Some(tracker) = &self.tracker_handle {
                            tracker.reload_models.store(true, Ordering::Relaxed);
                        }
                        notes.push(VtuberNotification::Success(
                            "VTuber neural face tracker installed and active.".to_string(),
                        ));
                    }
                    VtuberModelSyncEvent::Finished(Err(err)) => {
                        self.sync_in_progress = false;
                        self.sync_progress = None;
                        self.sync_error = Some(err.clone());
                        clear_sync_rx = true;
                        notes.push(VtuberNotification::Error(format!(
                            "VTuber tracker download failed: {err}"
                        )));
                    }
                }
            }
        }
        if clear_sync_rx {
            self.sync_event_rx = None;
        }

        notes
    }

    pub fn start_model_download(&mut self, repaint_ctx: Option<egui::Context>) {
        if self.sync_in_progress {
            return;
        }
        self.sync_in_progress = true;
        self.sync_error = None;
        self.sync_progress = Some(("Connecting to model repository...".to_string(), 0.0));

        let (tx, rx) = crossbeam_channel::unbounded();
        self.sync_event_rx = Some(rx);
        let dir = resolve_vtuber_tracker_directory();

        let _ = thread::Builder::new()
            .name("vtuber-model-download".to_string())
            .spawn(move || {
                let tx_prog = tx.clone();
                let ctx_prog = repaint_ctx.clone();
                let res = download_and_install_vtuber_models(&dir, move |message, progress| {
                    let _ = tx_prog.send(VtuberModelSyncEvent::Progress { message, progress });
                    if let Some(ctx) = &ctx_prog {
                        ctx.request_repaint();
                    }
                });
                let _ = tx.send(VtuberModelSyncEvent::Finished(res));
                if let Some(ctx) = repaint_ctx {
                    ctx.request_repaint();
                }
            });
    }

    pub fn ensure_tracker_running(
        &mut self,
        repaint_ctx: Option<egui::Context>,
    ) -> Result<(), String> {
        let needs_spawn = self
            .tracker_handle
            .as_ref()
            .and_then(|h| h.thread_handle.as_ref())
            .is_none_or(|t| t.is_finished());
        if !needs_spawn {
            return Ok(());
        }

        self.tracker_handle = None;
        let models_dir = resolve_vtuber_tracker_directory();
        let shared = Arc::clone(&self.shared_tracking);
        let repaint_cb: Arc<dyn Fn() + Send + Sync> = if let Some(ctx) = repaint_ctx {
            Arc::new(move || ctx.request_repaint())
        } else {
            Arc::new(|| {})
        };
        let handle = start_vtuber_tracker(models_dir, shared, repaint_cb)?;
        self.tracker_handle = Some(handle);
        Ok(())
    }

    pub fn stop_tracker(&mut self) {
        self.tracker_handle = None;
        if let Ok(mut shared) = self.shared_tracking.lock() {
            shared.latest_frame = None;
            shared.status = Some(VtuberTrackerStatus::IdleNoCamera);
        }
    }

    pub fn are_models_installed(&self) -> bool {
        are_vtuber_models_installed(&resolve_vtuber_tracker_directory())
    }

    pub fn tracker_status(&self, has_camera_selected: bool) -> VtuberTrackerStatus {
        if !has_camera_selected {
            return VtuberTrackerStatus::IdleNoCamera;
        }
        if let Ok(shared) = self.shared_tracking.lock() {
            if let Some(st) = &shared.status {
                return st.clone();
            }
        }
        if self.are_models_installed() {
            VtuberTrackerStatus::LoadingModels
        } else {
            VtuberTrackerStatus::ModelsMissing
        }
    }

    pub fn next_render_dt(&mut self, now_secs: f64) -> f32 {
        let dt = self
            .last_render_time_secs
            .map(|prev| (now_secs - prev) as f32)
            .unwrap_or(1.0 / 60.0)
            .clamp(1.0 / 500.0, 0.1);
        self.last_render_time_secs = Some(now_secs);
        dt
    }
}

fn decode_background_image_file(path: &Path) -> Result<DecodedRgbaTexture, String> {
    let dyn_img =
        image::open(path).map_err(|e| format!("Failed to decode {}: {e}", path.display()))?;
    let (w, h) = (dyn_img.width(), dyn_img.height());
    if w == 0 || h == 0 {
        return Err("Background image has zero dimensions".to_string());
    }
    let capped = if w > 2048 || h > 2048 {
        dyn_img.resize(2048, 2048, image::imageops::FilterType::Triangle)
    } else {
        dyn_img
    };
    let rgba8 = capped.to_rgba8();
    Ok(DecodedRgbaTexture {
        width: rgba8.width(),
        height: rgba8.height(),
        rgba: rgba8.into_raw(),
    })
}
