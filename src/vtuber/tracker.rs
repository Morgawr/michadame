use super::model_sync::{
    are_vtuber_models_installed, detector_model_path, landmarks_model_path,
    VTUBER_MODELS_READY_PINNED,
};
use super::rig::{solve_rig_from_mediapipe_478, TrackingRigFrame};
use crate::devices::webcam::WebcamFrame;
use crate::video::decoder::monotonic_nanos;
use glam::Vec3;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;
use tract_onnx::prelude::*;

type RunnableOnnxModel =
    SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VtuberTrackerStatus {
    IdleNoCamera,
    ModelsMissing,
    LoadingModels,
    AcquiringFace,
    Tracking,
    Error(String),
}

impl VtuberTrackerStatus {
    pub fn short_label(&self) -> &str {
        match self {
            Self::IdleNoCamera => "Idle (No Camera)",
            Self::ModelsMissing => "Tracker Models Not Installed",
            Self::LoadingModels => "Loading Tracker...",
            Self::AcquiringFace => "Searching for Face...",
            Self::Tracking => "Tracking Active",
            Self::Error(_) => "Tracker Error",
        }
    }
}

#[derive(Default)]
pub struct SharedTrackingState {
    pub latest_frame: Option<TrackingRigFrame>,
    pub status: Option<VtuberTrackerStatus>,
}

pub struct VtuberTrackerHandle {
    pub frame_tx: crossbeam_channel::Sender<Arc<WebcamFrame>>,
    pub frame_drop_rx: crossbeam_channel::Receiver<Arc<WebcamFrame>>,
    pub stop_requested: Arc<AtomicBool>,
    pub reload_models: Arc<AtomicBool>,
    pub thread_handle: Option<thread::JoinHandle<()>>,
}

impl Drop for VtuberTrackerHandle {
    fn drop(&mut self) {
        self.stop_requested.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread_handle.take() {
            if handle.is_finished() {
                let _ = handle.join();
            } else {
                let _ = thread::Builder::new()
                    .name("vtuber-tracker-reaper".to_string())
                    .spawn(move || {
                        let _ = handle.join();
                    });
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FaceRoi {
    cx: f32,
    cy: f32,
    size: f32,
    angle_rad: f32,
}

struct LoadedTrackerModels {
    detector: RunnableOnnxModel,
    landmarks: RunnableOnnxModel,
}

fn load_onnx_models(models_dir: &Path) -> Result<LoadedTrackerModels, String> {
    let det_path = detector_model_path(models_dir);
    let lm_path = landmarks_model_path(models_dir);

    let detector = tract_onnx::onnx()
        .model_for_path(&det_path)
        .map_err(|e| format!("Failed to parse {}: {e}", det_path.display()))?
        .into_optimized()
        .map_err(|e| format!("Failed to optimize BlazeFace ONNX: {e}"))?
        .into_runnable()
        .map_err(|e| format!("Failed to prepare BlazeFace ONNX: {e}"))?;

    let mut lm_model = tract_onnx::onnx()
        .model_for_path(&lm_path)
        .map_err(|e| format!("Failed to parse {}: {e}", lm_path.display()))?;
    lm_model
        .set_input_fact(
            0,
            InferenceFact::dt_shape(f32::datum_type(), &[1, 256, 256, 3]),
        )
        .map_err(|e| format!("Failed to set FaceMesh input shape: {e}"))?;
    let landmarks = lm_model
        .into_optimized()
        .map_err(|e| format!("Failed to optimize FaceMesh ONNX: {e}"))?
        .into_runnable()
        .map_err(|e| format!("Failed to prepare FaceMesh ONNX: {e}"))?;

    VTUBER_MODELS_READY_PINNED.store(true, Ordering::Relaxed);
    Ok(LoadedTrackerModels {
        detector,
        landmarks,
    })
}

pub fn start_vtuber_tracker(
    models_dir: PathBuf,
    shared: Arc<Mutex<SharedTrackingState>>,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
) -> Result<VtuberTrackerHandle, String> {
    let (frame_tx, frame_rx) = crossbeam_channel::bounded::<Arc<WebcamFrame>>(1);
    let frame_drop_rx = frame_rx.clone();
    let stop_requested = Arc::new(AtomicBool::new(false));
    let reload_models = Arc::new(AtomicBool::new(false));

    let stop_clone = Arc::clone(&stop_requested);
    let reload_clone = Arc::clone(&reload_models);

    let thread_handle = thread::Builder::new()
        .name("vtuber-tracker".to_string())
        .spawn(move || {
            run_tracker_loop(
                &models_dir,
                frame_rx,
                shared,
                request_repaint,
                stop_clone,
                reload_clone,
            );
        })
        .map_err(|e| format!("Failed to spawn VTuber tracker thread: {e}"))?;

    Ok(VtuberTrackerHandle {
        frame_tx,
        frame_drop_rx,
        stop_requested,
        reload_models,
        thread_handle: Some(thread_handle),
    })
}

fn set_status(shared: &Arc<Mutex<SharedTrackingState>>, status: VtuberTrackerStatus) {
    if let Ok(mut guard) = shared.lock() {
        guard.status = Some(status);
    }
}

fn run_tracker_loop(
    models_dir: &Path,
    frame_rx: crossbeam_channel::Receiver<Arc<WebcamFrame>>,
    shared: Arc<Mutex<SharedTrackingState>>,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    stop_requested: Arc<AtomicBool>,
    reload_models: Arc<AtomicBool>,
) {
    let mut loaded: Option<LoadedTrackerModels> = None;
    let mut load_failed = false;
    let mut current_roi: Option<FaceRoi> = None;
    let mut crop_buf = vec![0.0f32; 256 * 256 * 3];
    let mut det_buf = vec![0.0f32; 3 * 128 * 128];
    let mut landmarks_norm = Vec::with_capacity(478);

    while !stop_requested.load(Ordering::Relaxed) {
        if reload_models.swap(false, Ordering::AcqRel) {
            loaded = None;
            load_failed = false;
            current_roi = None;
        }

        if loaded.is_none() && !load_failed {
            if are_vtuber_models_installed(models_dir) {
                set_status(&shared, VtuberTrackerStatus::LoadingModels);
                request_repaint();
                match load_onnx_models(models_dir) {
                    Ok(m) => {
                        loaded = Some(m);
                        set_status(&shared, VtuberTrackerStatus::AcquiringFace);
                        request_repaint();
                    }
                    Err(e) => {
                        tracing::error!("VTuber tracker model load failed: {}", e);
                        load_failed = true;
                        set_status(&shared, VtuberTrackerStatus::Error(e));
                        request_repaint();
                    }
                }
            } else {
                set_status(&shared, VtuberTrackerStatus::ModelsMissing);
            }
        }

        let frame = match frame_rx.recv_timeout(Duration::from_millis(60)) {
            Ok(f) => f,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        let Some(models) = loaded.as_ref() else {
            continue;
        };

        if frame.width < 16
            || frame.height < 16
            || frame.rgba.len() < (frame.width * frame.height * 4) as usize
        {
            continue;
        }

        // Stage 1: Acquire face ROI via BlazeFace if we don't have an active tracked ROI.
        if current_roi.is_none() {
            current_roi = detect_face_roi_blazeface(&models.detector, &frame, &mut det_buf);
            if current_roi.is_none() {
                let now_secs = monotonic_nanos() as f64 * 1e-9;
                if let Ok(mut guard) = shared.lock() {
                    if let Some(prev) = &mut guard.latest_frame {
                        prev.tracked = false;
                        prev.confidence = 0.0;
                        prev.timestamp_secs = now_secs;
                    }
                    guard.status = Some(VtuberTrackerStatus::AcquiringFace);
                }
                request_repaint();
                continue;
            }
        }

        let Some(roi) = current_roi else {
            continue;
        };

        // Stage 2: Oriented 256x256 bilinear crop -> MediaPipe FaceMesh V2 (478 3D landmarks).
        sample_aligned_rgb_crop_nhwc(&frame, roi, 256, &mut crop_buf);
        let Ok(input_tensor) = Tensor::from_shape(&[1, 256, 256, 3], &crop_buf) else {
            continue;
        };
        let Ok(outputs) = models.landmarks.run(tvec!(input_tensor.into())) else {
            current_roi = None;
            continue;
        };

        if outputs.len() < 2 {
            current_roi = None;
            continue;
        }

        let presence_logit = outputs[1]
            .as_slice::<f32>()
            .ok()
            .and_then(|s| s.first().copied())
            .unwrap_or(-10.0);
        let confidence = 1.0 / (1.0 + (-presence_logit).exp());

        if confidence < 0.50 {
            current_roi = None;
            let now_secs = monotonic_nanos() as f64 * 1e-9;
            if let Ok(mut guard) = shared.lock() {
                if let Some(prev) = &mut guard.latest_frame {
                    prev.tracked = false;
                    prev.confidence = confidence;
                    prev.timestamp_secs = now_secs;
                }
                guard.status = Some(VtuberTrackerStatus::AcquiringFace);
            }
            request_repaint();
            continue;
        }

        let Ok(coords_raw) = outputs[0].as_slice::<f32>() else {
            current_roi = None;
            continue;
        };
        let num_pts = coords_raw.len() / 3;
        if num_pts < 468 {
            current_roi = None;
            continue;
        }

        // Map landmarks back from aligned 256x256 crop space to isotropic normalized image space.
        let cos_a = roi.angle_rad.cos();
        let sin_a = roi.angle_rad.sin();
        let w_f = frame.width as f32;
        let h_f = frame.height as f32;
        let inv_scale = 1.0 / w_f.max(1.0);

        landmarks_norm.clear();
        let mut min_px = f32::INFINITY;
        let mut max_px = f32::NEG_INFINITY;
        let mut min_py = f32::INFINITY;
        let mut max_py = f32::NEG_INFINITY;
        let mut p33_px = (0.0f32, 0.0f32);
        let mut p263_px = (0.0f32, 0.0f32);

        for idx in 0..num_pts.min(478) {
            let lx = coords_raw[idx * 3];
            let ly = coords_raw[idx * 3 + 1];
            let lz = coords_raw[idx * 3 + 2];

            let nx = lx * (1.0 / 256.0) - 0.5;
            let ny = ly * (1.0 / 256.0) - 0.5;
            let nz = lz * (1.0 / 256.0);

            let px = roi.cx + (nx * cos_a - ny * sin_a) * roi.size;
            let py = roi.cy + (nx * sin_a + ny * cos_a) * roi.size;
            let pz = nz * roi.size;

            if idx < 468 {
                min_px = min_px.min(px);
                max_px = max_px.max(px);
                min_py = min_py.min(py);
                max_py = max_py.max(py);
            }
            if idx == 33 {
                p33_px = (px, py);
            } else if idx == 263 {
                p263_px = (px, py);
            }

            // Isotropic normalization centered vertically at 0.5 so 16:9 aspect ratios never skew 3D angles.
            let iso_x = px * inv_scale;
            let iso_y = (py - 0.5 * h_f) * inv_scale + 0.5;
            let iso_z = pz * inv_scale;
            landmarks_norm.push(Vec3::new(iso_x, iso_y, iso_z));
        }

        // Update steady-state ROI for next frame directly from the 468 face mesh bounds.
        let span = (max_px - min_px).max(max_py - min_py).max(32.0);
        let next_angle = (p263_px.1 - p33_px.1).atan2(p263_px.0 - p33_px.0);
        current_roi = Some(FaceRoi {
            cx: 0.5 * (min_px + max_px),
            cy: 0.5 * (min_py + max_py),
            size: span * 1.48,
            angle_rad: next_angle,
        });

        let now_secs = monotonic_nanos() as f64 * 1e-9;
        let rig_frame = solve_rig_from_mediapipe_478(&landmarks_norm, confidence, now_secs);

        if let Ok(mut guard) = shared.lock() {
            guard.latest_frame = Some(rig_frame);
            guard.status = Some(VtuberTrackerStatus::Tracking);
        }
        request_repaint();
    }
}

fn detect_face_roi_blazeface(
    detector: &RunnableOnnxModel,
    frame: &WebcamFrame,
    det_buf_nchw: &mut [f32],
) -> Option<FaceRoi> {
    // Resize RGB into [1, 3, 128, 128] NCHW normalized [0..1].
    let plane = 128 * 128;
    let src_w = frame.width as usize;
    let src_h = frame.height as usize;
    let rgba = &frame.rgba;

    for dy in 0..128usize {
        let sy = ((dy * src_h) + src_h / 2) / 128;
        let sy = sy.min(src_h - 1);
        for dx in 0..128usize {
            let sx = ((dx * src_w) + src_w / 2) / 128;
            let sx = sx.min(src_w - 1);
            let off = (sy * src_w + sx) * 4;
            let idx = dy * 128 + dx;
            det_buf_nchw[idx] = rgba[off] as f32 * (1.0 / 255.0);
            det_buf_nchw[plane + idx] = rgba[off + 1] as f32 * (1.0 / 255.0);
            det_buf_nchw[2 * plane + idx] = rgba[off + 2] as f32 * (1.0 / 255.0);
        }
    }

    let img_tensor = Tensor::from_shape(&[1, 3, 128, 128], det_buf_nchw).ok()?;
    let conf_t = tensor1(&[0.45f32]);
    let max_det_t = tensor1(&[4i64]);
    let iou_t = tensor1(&[0.3f32]);

    let outs = detector
        .run(tvec!(
            img_tensor.into(),
            conf_t.into(),
            max_det_t.into(),
            iou_t.into()
        ))
        .ok()?;
    let boxes = outs.first()?.as_slice::<f32>().ok()?;
    let count = boxes.len() / 16;
    if count == 0 {
        return None;
    }

    let w_f = frame.width as f32;
    let h_f = frame.height as f32;

    let mut best: Option<(f32, FaceRoi)> = None;
    for i in 0..count {
        let d = &boxes[i * 16..(i + 1) * 16];
        let (top_y, top_x, bot_y, bot_x) = (d[0], d[1], d[2], d[3]);
        let bw = (bot_x - top_x) * w_f;
        let bh = (bot_y - top_y) * h_f;
        if bw < 16.0 || bh < 16.0 {
            continue;
        }
        let area = bw * bh;

        let (mut e1x, mut e1y) = (d[4] * w_f, d[5] * h_f);
        let (mut e2x, mut e2y) = (d[6] * w_f, d[7] * h_f);
        if e1x > e2x {
            std::mem::swap(&mut e1x, &mut e2x);
            std::mem::swap(&mut e1y, &mut e2y);
        }
        let angle_rad = (e2y - e1y).atan2(e2x - e1x);
        let roi = FaceRoi {
            cx: 0.5 * (top_x + bot_x) * w_f,
            cy: 0.5 * (top_y + bot_y) * h_f,
            size: bw.max(bh) * 1.55,
            angle_rad,
        };
        if best.is_none_or(|(best_area, _)| area > best_area) {
            best = Some((area, roi));
        }
    }

    best.map(|(_, roi)| roi)
}

/// Samples an oriented `dst_dim x dst_dim` NHWC RGB `[0..1]` tensor from `frame` using bilinear filtering.
fn sample_aligned_rgb_crop_nhwc(
    frame: &WebcamFrame,
    roi: FaceRoi,
    dst_dim: usize,
    out_nhwc: &mut [f32],
) {
    let cos_a = roi.angle_rad.cos();
    let sin_a = roi.angle_rad.sin();
    let inv_dim = 1.0 / dst_dim as f32;
    let max_x = (frame.width.saturating_sub(1)) as f32;
    let max_y = (frame.height.saturating_sub(1)) as f32;
    let stride = frame.width as usize * 4;
    let rgba = &frame.rgba;

    for v in 0..dst_dim {
        let ny = (v as f32 + 0.5) * inv_dim - 0.5;
        for u in 0..dst_dim {
            let nx = (u as f32 + 0.5) * inv_dim - 0.5;
            let sx = (roi.cx + (nx * cos_a - ny * sin_a) * roi.size).clamp(0.0, max_x);
            let sy = (roi.cy + (nx * sin_a + ny * cos_a) * roi.size).clamp(0.0, max_y);

            let x0 = sx.floor() as usize;
            let y0 = sy.floor() as usize;
            let x1 = (x0 + 1).min(frame.width as usize - 1);
            let y1 = (y0 + 1).min(frame.height as usize - 1);
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;

            let i00 = y0 * stride + x0 * 4;
            let i10 = y0 * stride + x1 * 4;
            let i01 = y1 * stride + x0 * 4;
            let i11 = y1 * stride + x1 * 4;

            let w00 = (1.0 - fx) * (1.0 - fy) * (1.0 / 255.0);
            let w10 = fx * (1.0 - fy) * (1.0 / 255.0);
            let w01 = (1.0 - fx) * fy * (1.0 / 255.0);
            let w11 = fx * fy * (1.0 / 255.0);

            let dst_off = (v * dst_dim + u) * 3;
            for c in 0..3 {
                out_nhwc[dst_off + c] = rgba[i00 + c] as f32 * w00
                    + rgba[i10 + c] as f32 * w10
                    + rgba[i01 + c] as f32 * w01
                    + rgba[i11 + c] as f32 * w11;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_aligned_rgb_crop_nhwc_preserves_center_pixel() {
        let mut rgba = vec![0u8; 64 * 64 * 4];
        for px in rgba.chunks_exact_mut(4) {
            px.copy_from_slice(&[50, 100, 200, 255]);
        }
        let frame = WebcamFrame {
            width: 64,
            height: 64,
            rgba,
        };
        let mut out = vec![0.0f32; 16 * 16 * 3];
        let roi = FaceRoi {
            cx: 32.0,
            cy: 32.0,
            size: 32.0,
            angle_rad: 0.4,
        };
        sample_aligned_rgb_crop_nhwc(&frame, roi, 16, &mut out);
        assert!((out[0] - 50.0 / 255.0).abs() < 1e-4);
        assert!((out[1] - 100.0 / 255.0).abs() < 1e-4);
        assert!((out[2] - 200.0 / 255.0).abs() < 1e-4);
    }
}
