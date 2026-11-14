pub mod model_sync;
pub mod resampler;
pub mod rmvpe;
pub mod runner;
pub mod sola;
pub mod worker;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum VoiceChangerStatus {
    Disabled = 0,
    MissingModels = 1,
    Loading = 2,
    Active = 3,
    Error = 4,
}

impl VoiceChangerStatus {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::MissingModels,
            2 => Self::Loading,
            3 => Self::Active,
            4 => Self::Error,
            _ => Self::Disabled,
        }
    }
}

/// Lock-free / low-contention telemetry shared between the voice changer worker,
/// the ALSA capture thread, and the egui UI.
pub struct VoiceChangerTelemetry {
    status: AtomicU8,
    inference_ms_bits: AtomicU32,
    latency_ms_bits: AtomicU32,
    delay_frames: AtomicU32,
    xrun: AtomicBool,
    reload_requested: AtomicBool,
    active_ep: Mutex<String>,
    last_error: Mutex<Option<String>>,
}

impl Default for VoiceChangerTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceChangerTelemetry {
    pub fn new() -> Self {
        Self {
            status: AtomicU8::new(VoiceChangerStatus::Disabled as u8),
            inference_ms_bits: AtomicU32::new(0.0f32.to_bits()),
            latency_ms_bits: AtomicU32::new(0.0f32.to_bits()),
            delay_frames: AtomicU32::new(0),
            xrun: AtomicBool::new(false),
            reload_requested: AtomicBool::new(false),
            active_ep: Mutex::new("Idle".to_string()),
            last_error: Mutex::new(None),
        }
    }

    pub fn status(&self) -> VoiceChangerStatus {
        VoiceChangerStatus::from_u8(self.status.load(Ordering::Relaxed))
    }

    pub fn set_status(&self, status: VoiceChangerStatus) {
        self.status.store(status as u8, Ordering::Relaxed);
    }

    pub fn inference_ms(&self) -> f32 {
        f32::from_bits(self.inference_ms_bits.load(Ordering::Relaxed))
    }

    pub fn set_inference_ms(&self, ms: f32) {
        self.inference_ms_bits
            .store(ms.to_bits(), Ordering::Relaxed);
    }

    pub fn latency_ms(&self) -> f32 {
        f32::from_bits(self.latency_ms_bits.load(Ordering::Relaxed))
    }

    pub fn set_latency_ms(&self, ms: f32) {
        self.latency_ms_bits.store(ms.to_bits(), Ordering::Relaxed);
    }

    pub fn delay_frames(&self) -> u32 {
        self.delay_frames.load(Ordering::Relaxed)
    }

    pub fn set_delay_frames(&self, frames: u32) {
        self.delay_frames.store(frames, Ordering::Relaxed);
    }

    pub fn is_xrun(&self) -> bool {
        self.xrun.load(Ordering::Relaxed)
    }

    pub fn set_xrun(&self, xrun: bool) {
        self.xrun.store(xrun, Ordering::Relaxed);
    }

    pub fn request_reload(&self) {
        self.reload_requested.store(true, Ordering::Relaxed);
    }

    pub fn take_reload_request(&self) -> bool {
        self.reload_requested.swap(false, Ordering::Relaxed)
    }

    pub fn active_ep_label(&self) -> String {
        self.active_ep
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "Unknown".to_string())
    }

    pub fn set_active_ep_label(&self, label: impl Into<String>) {
        if let Ok(mut g) = self.active_ep.lock() {
            *g = label.into();
        }
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|g| g.clone())
    }

    pub fn set_last_error(&self, err: Option<String>) {
        if let Ok(mut g) = self.last_error.lock() {
            *g = err;
        }
    }
}
