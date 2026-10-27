use prost::Message;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
    Arc, Mutex,
};
use std::time::Instant;

/// Selection of OCR engine backend.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OcrBackend {
    #[default]
    GoogleLens,
    LocalScreenAi,
}

impl OcrBackend {
    pub fn as_u8(self) -> u8 {
        match self {
            OcrBackend::GoogleLens => 0,
            OcrBackend::LocalScreenAi => 1,
        }
    }

    pub fn from_u8(val: u8) -> Self {
        match val {
            1 => OcrBackend::LocalScreenAi,
            _ => OcrBackend::GoogleLens,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OcrBackend::GoogleLens => "Google Lens (Remote)",
            OcrBackend::LocalScreenAi => "Chrome Screen AI (Local)",
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            OcrBackend::GoogleLens => "Google Lens",
            OcrBackend::LocalScreenAi => "Chrome Screen AI (Local)",
        }
    }
}

/// The text reading and writing direction detected by OCR.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum WritingDirection {
    #[default]
    LeftToRight = 0,
    RightToLeft = 1,
    TopToBottom = 2,
}

impl WritingDirection {
    #[inline]
    pub fn is_vertical(&self) -> bool {
        matches!(self, WritingDirection::TopToBottom)
    }

    pub fn from_i32(val: i32) -> Self {
        match val {
            1 => WritingDirection::RightToLeft,
            2 => WritingDirection::TopToBottom,
            _ => WritingDirection::LeftToRight,
        }
    }
}

/// Represents an OCR-detected text region with normalized coordinates.
/// All coordinate fields (center_x, center_y, width, height) are in the range [0.0, 1.0],
/// relative to the full game/video rendered area.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrBox {
    pub text: String,
    pub center_x: f32,
    pub center_y: f32,
    pub width: f32,
    pub height: f32,
    pub lines: Vec<ParsedLine>,
    pub writing_direction: WritingDirection,
}

/// Represents an individual OCR-detected line prior to multi-line block grouping.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ParsedLine {
    pub text: String,
    pub center_x: f32,
    pub center_y: f32,
    pub width: f32,
    pub height: f32,
    pub paragraph_idx: usize,
    pub writing_direction: WritingDirection,
    /// Optional normalized `[start, end]` intervals along the reading axis (X for horizontal,
    /// Y for vertical) for each character in `text.chars()`. When empty or when its length does
    /// not match `text.chars().count()`, callers fall back to uniform subdivision of the line box.
    pub char_spans: Vec<(f32, f32)>,
}

/// Encapsulates raw captured screen pixels from OpenGL to be processed by the background OCR worker.
#[derive(Clone, Debug)]
pub struct OcrCaptureRequest {
    pub raw_rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub request_id: u64,
    pub generation: u64,
    pub backend: OcrBackend,
    pub constant_scan: bool,
}

/// Result produced by the background OCR worker tagged with the originating request metadata.
#[derive(Clone, Debug)]
pub struct OcrWorkerResult {
    pub request_id: u64,
    pub generation: u64,
    pub backend: OcrBackend,
    pub constant_scan: bool,
    pub result: Result<Vec<ParsedLine>, String>,
}

/// Cloneable handle used by OpenGL paint callbacks to dispatch tagged OCR capture requests.
#[derive(Clone)]
pub struct OcrDispatchHandle {
    pub capture_requested: Arc<AtomicBool>,
    pub is_processing: Arc<AtomicBool>,
    pub session_generation: Arc<AtomicU64>,
    pub next_request_id: Arc<AtomicU64>,
    pub active_request_id: Arc<AtomicU64>,
    pub backend: OcrBackend,
    pub constant_scan: bool,
    pub sender: Option<crossbeam_channel::Sender<OcrCaptureRequest>>,
}

impl OcrDispatchHandle {
    pub fn take_capture_requested(&self) -> bool {
        self.capture_requested.swap(false, Ordering::AcqRel)
    }

    pub fn dispatch(&self, raw_rgba: Vec<u8>, width: u32, height: u32) -> Option<u64> {
        let req_tx = self.sender.as_ref()?;
        let request_id = self.next_request_id.fetch_add(1, Ordering::AcqRel);
        let generation = self.session_generation.load(Ordering::Acquire);
        self.active_request_id.store(request_id, Ordering::Release);
        self.is_processing.store(true, Ordering::Release);
        if req_tx
            .send(OcrCaptureRequest {
                raw_rgba,
                width,
                height,
                request_id,
                generation,
                backend: self.backend,
                constant_scan: self.constant_scan,
            })
            .is_err()
        {
            if self
                .active_request_id
                .compare_exchange(request_id, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.is_processing.store(false, Ordering::Release);
            }
            return None;
        }
        Some(request_id)
    }
}

/// Minimum base scan interval (in seconds) for constant background scanning.
pub const MIN_SCAN_INTERVAL_SECS: f32 = 0.3;
/// Maximum base scan interval (in seconds) for constant background scanning.
pub const MAX_SCAN_INTERVAL_SECS: f32 = 3.0;
/// Default base scan interval (in seconds) for constant background scanning.
pub const DEFAULT_SCAN_INTERVAL_SECS: f32 = 0.5;
/// Minimum configurable maximum drop-off delay (in seconds).
pub const MIN_MAX_DROPOFF_SECS: f32 = 1.0;
/// Maximum configurable maximum drop-off delay (in seconds).
pub const MAX_MAX_DROPOFF_SECS: f32 = 10.0;
/// Default maximum drop-off delay (in seconds).
pub const DEFAULT_MAX_DROPOFF_SECS: f32 = 3.0;
/// Delay increment (in seconds) per consecutive unchanged/empty scan along the harmonic rate curve.
pub const DROPOFF_STEP_SECS: f32 = 0.2;
/// Minimum configurable bounding-box jitter tolerance (1% of screen dimension).
pub const MIN_JITTER_TOLERANCE: f32 = 0.01;
/// Maximum configurable bounding-box jitter tolerance (50% of screen dimension).
pub const MAX_JITTER_TOLERANCE: f32 = 0.50;
/// Default bounding-box jitter tolerance (15% of screen dimension).
pub const DEFAULT_JITTER_TOLERANCE: f32 = 0.15;

fn is_ignorable_ocr_punctuation(c: char) -> bool {
    c.is_whitespace()
        || c.is_ascii_punctuation()
        || matches!(
            c,
            '、' | '。'
                | '，'
                | '．'
                | '・'
                | '：'
                | '；'
                | '？'
                | '！'
                | '「'
                | '」'
                | '『'
                | '』'
                | '（'
                | '）'
                | '〔'
                | '〕'
                | '［'
                | '］'
                | '｛'
                | '｝'
                | '〈'
                | '〉'
                | '《'
                | '》'
                | '【'
                | '】'
                | '…'
                | '‥'
                | '―'
                | '—'
                | '〜'
                | '～'
                | '“'
                | '”'
                | '‘'
                | '’'
        )
}

fn normalize_box_text_for_comparison(text: &str) -> String {
    let core: String = text
        .chars()
        .filter(|c| !is_ignorable_ocr_punctuation(*c))
        .collect();
    if core.is_empty() {
        text.chars().filter(|c| !c.is_whitespace()).collect()
    } else {
        core
    }
}

/// Returns `true` if two `OcrBox` instances represent the same text block within `jitter_tolerance`.
pub fn is_same_ocr_box(a: &OcrBox, b: &OcrBox, jitter_tolerance: f32) -> bool {
    let safe_jitter = if jitter_tolerance.is_finite() {
        jitter_tolerance
    } else {
        DEFAULT_JITTER_TOLERANCE
    };
    let tol = safe_jitter.clamp(MIN_JITTER_TOLERANCE, MAX_JITTER_TOLERANCE);
    (a.center_x - b.center_x).abs() <= tol
        && (a.center_y - b.center_y).abs() <= tol
        && (a.width - b.width).abs() <= tol
        && (a.height - b.height).abs() <= tol
        && normalize_box_text_for_comparison(&a.text) == normalize_box_text_for_comparison(&b.text)
}

/// Returns `true` if `prev` and `next` represent the same OCR scan outcome within `jitter_tolerance`:
/// - Both are empty ("no text"), OR
/// - Both have the same number of boxes, and each box in `prev` matches a unique box in `next`
///   with identical normalized text and bounding box coordinates (`center_x`, `center_y`,
///   `width`, `height`) within `jitter_tolerance`.
pub fn are_ocr_boxes_equivalent(prev: &[OcrBox], next: &[OcrBox], jitter_tolerance: f32) -> bool {
    if prev.len() != next.len() {
        return false;
    }
    if prev.is_empty() {
        return true;
    }

    let mut matched_next = vec![false; next.len()];
    for a in prev {
        let found = next
            .iter()
            .enumerate()
            .position(|(j, b)| !matched_next[j] && is_same_ocr_box(a, b, jitter_tolerance));
        match found {
            Some(j) => matched_next[j] = true,
            None => return false,
        }
    }
    true
}

/// State of the OCR system in the application.
pub struct OcrState {
    /// Selected OCR backend (remote Google Lens or local Chrome Screen AI).
    pub backend: OcrBackend,
    /// Atomic representation of `backend` shared with the persistent background worker thread.
    pub backend_shared: Arc<AtomicU8>,
    /// Whether persistent background OCR scanning is enabled (only active when `backend` is `LocalScreenAi`).
    pub constant_scanning: bool,
    /// Base interval (in seconds, >= 0.3s in 0.1s steps) between completed constant OCR scans.
    pub scan_interval_secs: f32,
    /// Maximum cooldown delay (in seconds) reached when consecutive scans return identical or empty results.
    pub max_dropoff_secs: f32,
    /// Normalized coordinate/size jitter tolerance (0.01..=0.50) for treating boxes across frames as identical.
    pub jitter_tolerance: f32,
    /// Number of consecutive constant scans that returned the same text/boxes (or no text).
    pub consecutive_same_scans: u32,
    /// Whether at least one constant scan has completed in the current session/backoff cycle.
    pub has_previous_constant_scan: bool,
    /// Scheduled timestamp for triggering the next constant OCR scan after the previous scan completed.
    pub next_constant_scan_time: Option<Instant>,
    /// Whether the mouse cursor is currently hovering inside any rendered OCR box on the video feed.
    pub is_box_hovered: bool,
    /// Whether constant scanning was paused on the previous frame (e.g. popup open, box hovered, or whiteboard open).
    pub was_constant_scan_paused: bool,
    /// Local directory where Chrome Screen AI model files and library are stored.
    pub screen_ai_dir: PathBuf,
    /// Installed local Chrome Screen AI metadata, if present on disk.
    pub screen_ai_installed_metadata: Option<crate::ocr::screen_ai::ScreenAiMetadata>,
    /// Latest remote Chrome Screen AI metadata checked from CIPD.
    pub screen_ai_remote_metadata: Option<crate::ocr::screen_ai::ScreenAiMetadata>,
    /// Whether an update is available (or local model is not yet installed).
    pub screen_ai_update_available: bool,
    /// Atomic flag indicating a Screen AI version check is running.
    pub is_checking_screen_ai_update: Arc<AtomicBool>,
    /// Atomic flag indicating a Screen AI model download/extraction is running.
    pub is_syncing_screen_ai: Arc<AtomicBool>,
    /// Current Screen AI download/sync progress: (Status message, fraction 0.0..1.0).
    pub screen_ai_sync_progress: Arc<Mutex<Option<(String, f32)>>>,
    /// Channel sender for Screen AI sync/update worker events.
    pub screen_ai_event_tx: crossbeam_channel::Sender<crate::ocr::screen_ai::ScreenAiEvent>,
    /// Channel receiver for Screen AI sync/update worker events.
    pub screen_ai_event_rx:
        Option<crossbeam_channel::Receiver<crate::ocr::screen_ai::ScreenAiEvent>>,
    /// Atomic flag set by the UI when an OCR screenshot capture is requested.
    /// Polled and cleared by the OpenGL paint callback.
    pub capture_requested: Arc<AtomicBool>,
    /// Whether an OCR request is currently being processed by the background worker.
    pub is_processing: Arc<AtomicBool>,
    /// Monotonically increasing OCR session generation incremented on clear, backend switch, or source change.
    pub session_generation: Arc<AtomicU64>,
    /// Monotonically increasing request ID generator for correlating in-flight OCR captures.
    pub next_request_id: Arc<AtomicU64>,
    /// Request ID of the currently in-flight OCR request (`0` when idle).
    pub active_request_id: Arc<AtomicU64>,
    /// List of OCR text boxes detected from the last scan.
    pub boxes: Vec<OcrBox>,
    /// Raw unmerged lines from the last OCR scan.
    pub raw_lines: Vec<ParsedLine>,
    /// Maximum distance threshold (as a multiple of line height) for merging
    /// adjacent lines into a single text block.
    pub sticky_distance: f32,
    /// Number of seconds before OCR scan results automatically clear (0 = disabled).
    pub timeout_seconds: u32,
    /// Timestamp when the most recent OCR scan results were received.
    pub last_scan_time: Option<Instant>,
    /// Timestamp of the last frame tick while the OCR timer is paused (e.g. whiteboard open).
    pub last_pause_tick: Option<Instant>,
    /// Whether to hide the visual OCR overlay (boxes and replacement text) on the video feed.
    pub hide_overlay: bool,
    /// Index of the most recently clicked/copied box, for visual feedback.
    pub last_copied_index: Option<usize>,
    /// Timestamp when the box was copied, to expire visual highlight.
    pub copy_feedback_time: Option<Instant>,
    /// Last error message, if any.
    pub last_error: Option<String>,
    /// Channel sender for dispatching raw frame captures to the persistent OCR background worker.
    pub ocr_request_sender: Option<crossbeam_channel::Sender<OcrCaptureRequest>>,
    /// Sender for OCR worker results.
    pub ocr_sender: Option<crossbeam_channel::Sender<OcrWorkerResult>>,
    /// Channel receiver for completed OCR results from the background worker.
    pub result_receiver: Option<crossbeam_channel::Receiver<OcrWorkerResult>>,
}

impl OcrState {
    /// Returns the current OCR session generation.
    #[inline]
    pub fn current_generation(&self) -> u64 {
        self.session_generation.load(Ordering::Acquire)
    }

    /// Advances the OCR session generation and invalidates any in-flight request so stale
    /// completions cannot repopulate boxes, errors, or backoff state.
    pub fn bump_generation(&mut self) -> u64 {
        let next = self.session_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.active_request_id.store(0, Ordering::Release);
        self.is_processing.store(false, Ordering::Release);
        self.capture_requested.store(false, Ordering::Release);
        next
    }

    /// Creates a cloneable dispatch handle capturing the current backend and scan mode.
    pub fn dispatch_handle(&self) -> OcrDispatchHandle {
        OcrDispatchHandle {
            capture_requested: self.capture_requested.clone(),
            is_processing: self.is_processing.clone(),
            session_generation: self.session_generation.clone(),
            next_request_id: self.next_request_id.clone(),
            active_request_id: self.active_request_id.clone(),
            backend: self.backend,
            constant_scan: self.is_constant_scanning_active(),
            sender: self.ocr_request_sender.clone(),
        }
    }

    /// Validates a worker completion against the active request ID and current session generation.
    /// Clears `is_processing` only if `res.request_id` matches `active_request_id`, and returns
    /// `true` only if `res.generation` matches the current session generation.
    pub fn accept_worker_result(&mut self, res: &OcrWorkerResult) -> bool {
        if self
            .active_request_id
            .compare_exchange(res.request_id, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.is_processing.store(false, Ordering::Release);
        }
        res.generation == self.current_generation()
    }

    /// Returns `true` if constant background scanning is currently active.
    /// Constant scanning is only supported when the local Chrome Screen AI model is selected.
    #[inline]
    pub fn is_constant_scanning_active(&self) -> bool {
        self.backend == OcrBackend::LocalScreenAi && self.constant_scanning
    }

    /// Computes the scan delay in seconds using a harmonic frequency drop-off curve
    /// (`rate(n) = 1 / (base + 0.2 * n)` -> `delay(n) = base + 0.2 * n`, clamped to `max_dropoff`).
    /// For a `0.3s` base interval, this progresses as `0.3s -> 0.5s -> 0.7s -> ... -> max_dropoff`.
    pub fn calculate_dropoff_delay_secs(
        scan_interval_secs: f32,
        max_dropoff_secs: f32,
        consecutive_same_scans: u32,
    ) -> f32 {
        let safe_interval = if scan_interval_secs.is_finite() {
            scan_interval_secs
        } else {
            DEFAULT_SCAN_INTERVAL_SECS
        };
        let safe_dropoff = if max_dropoff_secs.is_finite() {
            max_dropoff_secs
        } else {
            DEFAULT_MAX_DROPOFF_SECS
        };
        let base = safe_interval.clamp(MIN_SCAN_INTERVAL_SECS, MAX_SCAN_INTERVAL_SECS);
        let max_delay = safe_dropoff
            .clamp(MIN_MAX_DROPOFF_SECS, MAX_MAX_DROPOFF_SECS)
            .max(base);
        let raw = (base + DROPOFF_STEP_SECS * (consecutive_same_scans as f32)).min(max_delay);
        let rounded = (raw * 1000.0).round() / 1000.0;
        if rounded.is_finite() && rounded >= MIN_SCAN_INTERVAL_SECS {
            rounded
        } else {
            DEFAULT_SCAN_INTERVAL_SECS
        }
    }

    /// Returns the `Duration` to wait before the next constant scan, accounting for drop-off backoff.
    pub fn current_constant_scan_delay(&self) -> std::time::Duration {
        let secs = Self::calculate_dropoff_delay_secs(
            self.scan_interval_secs,
            self.max_dropoff_secs,
            self.consecutive_same_scans,
        );
        std::time::Duration::try_from_secs_f32(secs)
            .unwrap_or_else(|_| std::time::Duration::from_secs_f32(DEFAULT_SCAN_INTERVAL_SECS))
    }

    /// Resets the constant scan drop-off counter and scheduled timer, and invalidates in-flight scans.
    pub fn reset_constant_scan_backoff(&mut self) {
        self.consecutive_same_scans = 0;
        self.has_previous_constant_scan = false;
        self.next_constant_scan_time = None;
        self.was_constant_scan_paused = false;
        self.bump_generation();
    }

    /// Processes a completed constant OCR scan result (`lines`).
    ///
    /// - If the resulting boxes match the previous scan within `self.jitter_tolerance` (or both are
    ///   empty "no text"), the outcome is discarded without updating `boxes`, the drop-off counter
    ///   is incremented, and this method returns `false`.
    /// - If the resulting boxes differ from the previous scan (or this is the first scan), `boxes`
    ///   and `raw_lines` are updated (preserving stationary boxes' coordinates so unchanged regions
    ///   do not flicker), the drop-off counter is reset to `0`, and this method returns `true`.
    pub fn apply_constant_scan_lines(&mut self, lines: Vec<ParsedLine>) -> bool {
        let now = Instant::now();
        let safe_sticky = if self.sticky_distance.is_finite() {
            self.sticky_distance.clamp(0.0, 10.0)
        } else {
            0.6
        };
        let new_boxes = crate::ocr::lens::group_lines_into_blocks(&lines, safe_sticky);
        let tol = self.jitter_tolerance;
        if self.has_previous_constant_scan && are_ocr_boxes_equivalent(&self.boxes, &new_boxes, tol)
        {
            self.consecutive_same_scans = self.consecutive_same_scans.saturating_add(1);
            self.next_constant_scan_time = Some(now + self.current_constant_scan_delay());
            return false;
        }

        let changed = !are_ocr_boxes_equivalent(&self.boxes, &new_boxes, tol);
        let mut used_prev = vec![false; self.boxes.len()];
        let stabilized_boxes: Vec<OcrBox> = new_boxes
            .into_iter()
            .map(|nb| {
                if let Some((idx, prev_box)) = self
                    .boxes
                    .iter()
                    .enumerate()
                    .find(|(i, pb)| !used_prev[*i] && is_same_ocr_box(pb, &nb, tol))
                {
                    used_prev[idx] = true;
                    prev_box.clone()
                } else {
                    nb
                }
            })
            .collect();

        self.has_previous_constant_scan = true;
        self.consecutive_same_scans = 0;
        self.raw_lines = lines;
        self.boxes = stabilized_boxes;
        self.last_copied_index = None;
        self.copy_feedback_time = None;
        self.last_error = None;
        if self.boxes.is_empty() {
            self.last_scan_time = None;
            self.is_box_hovered = false;
        } else {
            self.last_scan_time = Some(now);
        }
        self.next_constant_scan_time = Some(now + self.current_constant_scan_delay());
        changed
    }

    /// Updates the selected OCR backend and synchronizes the atomic value read by the worker thread.
    pub fn set_backend(&mut self, backend: OcrBackend) {
        if self.backend != backend {
            self.reset_constant_scan_backoff();
        }
        self.backend = backend;
        self.backend_shared
            .store(backend.as_u8(), Ordering::Release);
    }

    /// Triggers a background version check for the Chrome Screen AI model on CIPD.
    pub fn trigger_check_screen_ai_version(&mut self) {
        let current_digest = self
            .screen_ai_installed_metadata
            .as_ref()
            .map(|m| m.instance_digest.clone());
        crate::ocr::screen_ai::spawn_check_screen_ai_version(
            current_digest,
            self.is_checking_screen_ai_update.clone(),
            self.screen_ai_event_tx.clone(),
        );
    }

    /// Triggers a background download and installation of the Chrome Screen AI model.
    pub fn trigger_screen_ai_sync(&mut self) {
        if self.is_syncing_screen_ai.load(Ordering::Relaxed) {
            return;
        }
        crate::ocr::screen_ai::spawn_sync_screen_ai(
            self.screen_ai_dir.clone(),
            self.is_syncing_screen_ai.clone(),
            self.screen_ai_event_tx.clone(),
        );
    }

    /// Starts the persistent background worker thread if not already running.
    /// The worker manages connection pooling, warmup keep-alives, single-pass downscaling,
    /// fast JPEG compression (for Google Lens), local Screen AI inference, and direct UI
    /// notification upon result readiness.
    pub fn start_worker(&mut self, egui_ctx: Option<eframe::egui::Context>) {
        self.backend_shared
            .store(self.backend.as_u8(), Ordering::Release);

        if self.ocr_request_sender.is_some() {
            return;
        }

        if egui_ctx.is_some() {
            self.trigger_check_screen_ai_version();
            if self.backend == OcrBackend::LocalScreenAi
                && self.screen_ai_installed_metadata.is_none()
            {
                self.trigger_screen_ai_sync();
            }
        }

        let (req_tx, req_rx) = crossbeam_channel::unbounded::<OcrCaptureRequest>();
        self.ocr_request_sender = Some(req_tx);

        let res_tx = self.ocr_sender.as_ref().unwrap().clone();
        let is_processing = self.is_processing.clone();
        let session_generation = self.session_generation.clone();
        let backend_shared = self.backend_shared.clone();
        let screen_ai_dir = self.screen_ai_dir.clone();
        let is_syncing_screen_ai = self.is_syncing_screen_ai.clone();
        let screen_ai_event_tx = self.screen_ai_event_tx.clone();

        let _ = std::thread::Builder::new()
            .name("ocr-worker".into())
            .spawn(move || {
                let agent = crate::ocr::lens::create_lens_agent();
                let initial_backend =
                    OcrBackend::from_u8(backend_shared.load(Ordering::Acquire));
                match initial_backend {
                    OcrBackend::GoogleLens => {
                        // Initial background warmup to establish TCP+TLS connection early
                        crate::ocr::lens::ping_warmup(&agent);
                    }
                    OcrBackend::LocalScreenAi => {
                        if crate::ocr::screen_ai::is_screen_ai_installed(&screen_ai_dir) {
                            let _ = crate::ocr::screen_ai::warmup_screen_ai(&screen_ai_dir);
                        }
                    }
                }

                let mut rgb_buffer = Vec::new();
                let mut jpeg_buffer = Vec::new();
                let mut bgra_buffer = Vec::new();

                loop {
                    // Wait for capture request or send periodic keep-alive probe after 45s
                    match req_rx.recv_timeout(std::time::Duration::from_secs(45)) {
                        Ok(req) => {
                            if req.generation != session_generation.load(Ordering::Acquire) {
                                let _ = res_tx.send(OcrWorkerResult {
                                    request_id: req.request_id,
                                    generation: req.generation,
                                    backend: req.backend,
                                    constant_scan: req.constant_scan,
                                    result: Ok(Vec::new()),
                                });
                                continue;
                            }
                            is_processing.store(true, Ordering::Release);
                            let active_backend = req.backend;

                            let result = match active_backend {
                                OcrBackend::GoogleLens => {
                                    let (target_w, target_h) =
                                        crate::ocr::lens::calculate_720p_target_dimensions(
                                            req.width, req.height,
                                        );

                                    crate::ocr::lens::convert_and_scale_gl_rgba_to_rgb(
                                        &req.raw_rgba,
                                        req.width,
                                        req.height,
                                        target_w,
                                        target_h,
                                        &mut rgb_buffer,
                                    );

                                    let enc_res = crate::ocr::lens::encode_jpeg_for_ocr(
                                        &rgb_buffer,
                                        target_w,
                                        target_h,
                                        &mut jpeg_buffer,
                                    );

                                    match enc_res {
                                        Ok(()) => crate::ocr::lens::execute_lens_ocr_with_agent(
                                            &agent,
                                            jpeg_buffer.clone(),
                                            target_w,
                                            target_h,
                                        ),
                                        Err(e) => Err(e),
                                    }
                                }
                                OcrBackend::LocalScreenAi => {
                                    // Wait if a background sync is already in progress
                                    while is_syncing_screen_ai.load(Ordering::Acquire) {
                                        std::thread::sleep(std::time::Duration::from_millis(100));
                                    }

                                    // Auto-download on demand if not yet installed
                                    let install_res = if !crate::ocr::screen_ai::is_screen_ai_installed(&screen_ai_dir) {
                                        is_syncing_screen_ai.store(true, Ordering::Release);
                                        let tx_prog = screen_ai_event_tx.clone();
                                        let ctx_prog = egui_ctx.clone();
                                        let dl_res = crate::ocr::screen_ai::download_and_install_screen_ai(
                                            &screen_ai_dir,
                                            move |message, progress| {
                                                let _ = tx_prog.send(
                                                    crate::ocr::screen_ai::ScreenAiEvent::SyncProgress {
                                                        message,
                                                        progress,
                                                    },
                                                );
                                                if let Some(ctx) = &ctx_prog {
                                                    ctx.request_repaint();
                                                }
                                            },
                                        );
                                        is_syncing_screen_ai.store(false, Ordering::Release);
                                        let _ = screen_ai_event_tx.send(
                                            crate::ocr::screen_ai::ScreenAiEvent::SyncFinished(
                                                dl_res.clone(),
                                            ),
                                        );
                                        dl_res.map(|_| ())
                                    } else {
                                        Ok(())
                                    };

                                    match install_res {
                                        Ok(()) => {
                                            let max_dim = crate::ocr::screen_ai::warmup_screen_ai(
                                                &screen_ai_dir,
                                            )
                                            .unwrap_or(
                                                crate::ocr::screen_ai::DEFAULT_MAX_DIMENSION,
                                            );
                                            let (target_w, target_h) =
                                                crate::ocr::screen_ai::calculate_screen_ai_target_dimensions(
                                                    req.width, req.height, max_dim,
                                                );

                                            crate::ocr::screen_ai::convert_and_scale_gl_rgba_to_bgra(
                                                &req.raw_rgba,
                                                req.width,
                                                req.height,
                                                target_w,
                                                target_h,
                                                &mut bgra_buffer,
                                            );

                                            crate::ocr::screen_ai::execute_screen_ai_ocr_bgra(
                                                &screen_ai_dir,
                                                &bgra_buffer,
                                                target_w,
                                                target_h,
                                            )
                                        }
                                        Err(e) => Err(e),
                                    }
                                }
                            };

                            let _ = res_tx.send(OcrWorkerResult {
                                request_id: req.request_id,
                                generation: req.generation,
                                backend: req.backend,
                                constant_scan: req.constant_scan,
                                result,
                            });
                            if let Some(ctx) = &egui_ctx {
                                ctx.request_repaint();
                            }
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                            // Maintain keepalive on idle connection when using remote Google Lens
                            if OcrBackend::from_u8(backend_shared.load(Ordering::Acquire))
                                == OcrBackend::GoogleLens
                            {
                                crate::ocr::lens::ping_warmup(&agent);
                            }
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                            break;
                        }
                    }
                }
            });
    }

    /// Recomputes merged `boxes` from `raw_lines` using the current `sticky_distance`.
    pub fn recompute_boxes(&mut self) {
        let safe_sticky = if self.sticky_distance.is_finite() {
            self.sticky_distance.clamp(0.0, 10.0)
        } else {
            0.6
        };
        self.boxes = crate::ocr::lens::group_lines_into_blocks(&self.raw_lines, safe_sticky);
        self.last_copied_index = None;
    }

    /// Clears all detected OCR boxes, lines, and related state.
    pub fn clear(&mut self) {
        self.boxes.clear();
        self.raw_lines.clear();
        self.last_copied_index = None;
        self.copy_feedback_time = None;
        self.last_scan_time = None;
        self.last_pause_tick = None;
        self.is_box_hovered = false;
        self.reset_constant_scan_backoff();
    }

    /// Returns true if the OCR boxes have expired according to `timeout_seconds`.
    /// When constant scanning is active, boxes do not expire on a timer (they update/clear
    /// automatically when on-screen text changes or disappears).
    pub fn is_expired(&self) -> bool {
        if self.is_constant_scanning_active() || self.timeout_seconds == 0 || self.boxes.is_empty()
        {
            return false;
        }
        if let Some(scan_time) = self.last_scan_time {
            scan_time.elapsed() >= std::time::Duration::from_secs(self.timeout_seconds as u64)
        } else {
            false
        }
    }

    /// Returns the remaining duration until the OCR boxes expire, or None if expired or disabled.
    pub fn remaining_time(&self) -> Option<std::time::Duration> {
        if self.is_constant_scanning_active() || self.timeout_seconds == 0 || self.boxes.is_empty()
        {
            return None;
        }
        let scan_time = self.last_scan_time?;
        let timeout = std::time::Duration::from_secs(self.timeout_seconds as u64);
        timeout.checked_sub(scan_time.elapsed())
    }
}

impl Default for OcrState {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (sa_tx, sa_rx) = crossbeam_channel::unbounded();
        let screen_ai_dir = crate::ocr::screen_ai::resolve_screen_ai_directory();
        let screen_ai_installed_metadata =
            crate::ocr::screen_ai::load_installed_metadata(&screen_ai_dir);
        let screen_ai_update_available = screen_ai_installed_metadata.is_none();

        Self {
            backend: OcrBackend::default(),
            backend_shared: Arc::new(AtomicU8::new(OcrBackend::default().as_u8())),
            constant_scanning: false,
            scan_interval_secs: DEFAULT_SCAN_INTERVAL_SECS,
            max_dropoff_secs: DEFAULT_MAX_DROPOFF_SECS,
            jitter_tolerance: DEFAULT_JITTER_TOLERANCE,
            consecutive_same_scans: 0,
            has_previous_constant_scan: false,
            next_constant_scan_time: None,
            is_box_hovered: false,
            was_constant_scan_paused: false,
            screen_ai_dir,
            screen_ai_installed_metadata,
            screen_ai_remote_metadata: None,
            screen_ai_update_available,
            is_checking_screen_ai_update: Arc::new(AtomicBool::new(false)),
            is_syncing_screen_ai: Arc::new(AtomicBool::new(false)),
            screen_ai_sync_progress: Arc::new(Mutex::new(None)),
            screen_ai_event_tx: sa_tx,
            screen_ai_event_rx: Some(sa_rx),
            capture_requested: Arc::new(AtomicBool::new(false)),
            is_processing: Arc::new(AtomicBool::new(false)),
            session_generation: Arc::new(AtomicU64::new(1)),
            next_request_id: Arc::new(AtomicU64::new(1)),
            active_request_id: Arc::new(AtomicU64::new(0)),
            boxes: Vec::new(),
            raw_lines: Vec::new(),
            sticky_distance: 0.6,
            timeout_seconds: 45,
            last_scan_time: None,
            last_pause_tick: None,
            hide_overlay: false,
            last_copied_index: None,
            copy_feedback_time: None,
            last_error: None,
            ocr_request_sender: None,
            ocr_sender: Some(tx),
            result_receiver: Some(rx),
        }
    }
}

// ============================================================================
// Google Lens Protobuf Schemas
// ============================================================================

#[derive(Clone, PartialEq, Message)]
pub struct CenterRotatedBox {
    #[prost(float, tag = "1")]
    pub center_x: f32,
    #[prost(float, tag = "2")]
    pub center_y: f32,
    #[prost(float, tag = "3")]
    pub width: f32,
    #[prost(float, tag = "4")]
    pub height: f32,
    #[prost(float, tag = "5")]
    pub rotation_z: f32,
    #[prost(int32, tag = "6")]
    pub coordinate_type: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct Geometry {
    #[prost(message, optional, tag = "1")]
    pub bounding_box: Option<CenterRotatedBox>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TextEntityIdentifier {
    #[prost(int64, tag = "1")]
    pub id: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct TextLayoutWord {
    #[prost(message, optional, tag = "1")]
    pub id: Option<TextEntityIdentifier>,
    #[prost(string, tag = "2")]
    pub plain_text: String,
    #[prost(string, optional, tag = "3")]
    pub text_separator: Option<String>,
    #[prost(message, optional, tag = "4")]
    pub geometry: Option<Geometry>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TextLayoutLine {
    #[prost(message, repeated, tag = "1")]
    pub words: Vec<TextLayoutWord>,
    #[prost(message, optional, tag = "2")]
    pub geometry: Option<Geometry>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TextLayoutParagraph {
    #[prost(message, optional, tag = "1")]
    pub id: Option<TextEntityIdentifier>,
    #[prost(message, repeated, tag = "2")]
    pub lines: Vec<TextLayoutLine>,
    #[prost(message, optional, tag = "3")]
    pub geometry: Option<Geometry>,
    #[prost(int32, tag = "4")]
    pub writing_direction: i32,
    #[prost(string, tag = "5")]
    pub content_language: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct TextLayout {
    #[prost(message, repeated, tag = "1")]
    pub paragraphs: Vec<TextLayoutParagraph>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Text {
    #[prost(message, optional, tag = "1")]
    pub text_layout: Option<TextLayout>,
    #[prost(string, tag = "2")]
    pub content_language: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ImageMetadata {
    #[prost(int32, tag = "1")]
    pub width: i32,
    #[prost(int32, tag = "2")]
    pub height: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct ImagePayload {
    #[prost(bytes, tag = "1")]
    pub image_bytes: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ImageData {
    #[prost(message, optional, tag = "1")]
    pub payload: Option<ImagePayload>,
    #[prost(message, optional, tag = "3")]
    pub image_metadata: Option<ImageMetadata>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AppliedFilter {
    #[prost(int32, tag = "1")]
    pub filter_type: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct AppliedFilters {
    #[prost(message, repeated, tag = "1")]
    pub filter: Vec<AppliedFilter>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LocaleContext {
    #[prost(string, tag = "1")]
    pub language: String,
    #[prost(string, tag = "2")]
    pub region: String,
    #[prost(string, tag = "3")]
    pub time_zone: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayClientContext {
    #[prost(int32, tag = "1")]
    pub platform: i32,
    #[prost(int32, tag = "2")]
    pub surface: i32,
    #[prost(message, optional, tag = "4")]
    pub locale_context: Option<LocaleContext>,
    #[prost(string, tag = "6")]
    pub app_id: String,
    #[prost(message, optional, tag = "17")]
    pub client_filters: Option<AppliedFilters>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayRequestId {
    #[prost(uint64, tag = "1")]
    pub uuid: u64,
    #[prost(int32, tag = "2")]
    pub sequence_id: i32,
    #[prost(int32, tag = "3")]
    pub image_sequence_id: i32,
    #[prost(bytes, tag = "4")]
    pub analytics_id: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayRequestContext {
    #[prost(message, optional, tag = "3")]
    pub request_id: Option<LensOverlayRequestId>,
    #[prost(message, optional, tag = "4")]
    pub client_context: Option<LensOverlayClientContext>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayObjectsRequest {
    #[prost(message, optional, tag = "1")]
    pub request_context: Option<LensOverlayRequestContext>,
    #[prost(message, optional, tag = "3")]
    pub image_data: Option<ImageData>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayServerRequest {
    #[prost(message, optional, tag = "1")]
    pub objects_request: Option<LensOverlayObjectsRequest>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayObjectsResponse {
    #[prost(message, optional, tag = "3")]
    pub text: Option<Text>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayServerResponse {
    #[prost(message, optional, tag = "2")]
    pub objects_response: Option<LensOverlayObjectsResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proto_serialization_roundtrip() {
        let req = LensOverlayServerRequest {
            objects_request: Some(LensOverlayObjectsRequest {
                request_context: Some(LensOverlayRequestContext {
                    request_id: Some(LensOverlayRequestId {
                        uuid: 123456789,
                        sequence_id: 0,
                        image_sequence_id: 0,
                        analytics_id: vec![1, 2, 3, 4],
                    }),
                    client_context: Some(LensOverlayClientContext {
                        platform: 3,
                        surface: 4,
                        locale_context: Some(LocaleContext {
                            language: "ja".to_string(),
                            region: "JP".to_string(),
                            time_zone: "Asia/Tokyo".to_string(),
                        }),
                        app_id: String::new(),
                        client_filters: Some(AppliedFilters {
                            filter: vec![AppliedFilter { filter_type: 7 }],
                        }),
                    }),
                }),
                image_data: Some(ImageData {
                    payload: Some(ImagePayload {
                        image_bytes: vec![0x89, 0x50, 0x4E, 0x47],
                    }),
                    image_metadata: Some(ImageMetadata {
                        width: 100,
                        height: 100,
                    }),
                }),
            }),
        };

        let mut buf = Vec::new();
        req.encode(&mut buf).expect("Failed to encode request");
        assert!(!buf.is_empty());

        let decoded = LensOverlayServerRequest::decode(&buf[..]).expect("Failed to decode");
        assert_eq!(req, decoded);
    }

    #[test]
    fn test_ocr_state_clear() {
        let mut state = OcrState::default();
        state.boxes.push(OcrBox {
            text: "Hello".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.2,
            height: 0.1,
            lines: Vec::new(),
            writing_direction: WritingDirection::LeftToRight,
        });
        state.raw_lines.push(ParsedLine {
            text: "Hello".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.2,
            height: 0.1,
            paragraph_idx: 0,
            writing_direction: WritingDirection::LeftToRight,
            char_spans: Vec::new(),
        });
        state.last_copied_index = Some(0);
        state.last_scan_time = Some(Instant::now());

        state.clear();
        assert!(state.boxes.is_empty());
        assert!(state.raw_lines.is_empty());
        assert_eq!(state.last_copied_index, None);
        assert_eq!(state.last_scan_time, None);
    }

    #[test]
    fn test_ocr_state_is_expired_and_remaining_time() {
        let mut state = OcrState::default();
        state.timeout_seconds = 45;

        // When boxes is empty, never expired
        assert!(!state.is_expired());
        assert_eq!(state.remaining_time(), None);

        // Add a box
        state.boxes.push(OcrBox {
            text: "Test".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.1,
            height: 0.1,
            lines: Vec::new(),
            writing_direction: WritingDirection::LeftToRight,
        });

        // If last_scan_time is None, not expired
        assert!(!state.is_expired());
        assert_eq!(state.remaining_time(), None);

        // Just scanned now
        state.last_scan_time = Some(Instant::now());
        assert!(!state.is_expired());
        assert!(state.remaining_time().unwrap().as_secs() <= 45);

        // Simulated scan 50 seconds ago (exceeds 45s)
        state.last_scan_time = Some(Instant::now() - std::time::Duration::from_secs(50));
        assert!(state.is_expired());
        assert_eq!(state.remaining_time(), None);

        // Simulated scan 30 seconds ago (15s remaining)
        state.last_scan_time = Some(Instant::now() - std::time::Duration::from_secs(30));
        assert!(!state.is_expired());
        let rem = state.remaining_time().unwrap();
        assert!(rem.as_secs() >= 14 && rem.as_secs() <= 15);
    }

    #[test]
    fn test_ocr_state_timeout_disabled() {
        let mut state = OcrState::default();
        state.timeout_seconds = 0; // Disabled
        state.boxes.push(OcrBox {
            text: "Permanent".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.1,
            height: 0.1,
            lines: Vec::new(),
            writing_direction: WritingDirection::LeftToRight,
        });
        state.last_scan_time = Some(Instant::now() - std::time::Duration::from_secs(9999));

        assert!(!state.is_expired());
        assert_eq!(state.remaining_time(), None);
    }

    #[test]
    fn test_ocr_worker_lifecycle() {
        let mut state = OcrState::default();
        assert!(state.ocr_request_sender.is_none());

        // Starting worker initializes channel
        state.start_worker(None);
        assert!(state.ocr_request_sender.is_some());

        // Calling start_worker again is idempotent
        state.start_worker(None);
        assert!(state.ocr_request_sender.is_some());
    }

    #[test]
    fn test_constant_scanning_active_only_for_local_backend() {
        let mut state = OcrState::default();
        state.constant_scanning = true;
        state.set_backend(OcrBackend::GoogleLens);
        assert!(!state.is_constant_scanning_active());

        state.set_backend(OcrBackend::LocalScreenAi);
        assert!(state.is_constant_scanning_active());

        state.constant_scanning = false;
        assert!(!state.is_constant_scanning_active());
    }

    #[test]
    fn test_dropoff_delay_progression_and_clamping() {
        assert!(
            (OcrState::calculate_dropoff_delay_secs(0.3, 3.0, 0) - 0.3).abs() < 1e-5,
            "0 consecutive same scans should use base interval 0.3s"
        );
        assert!(
            (OcrState::calculate_dropoff_delay_secs(0.3, 3.0, 1) - 0.5).abs() < 1e-5,
            "1 consecutive same scan -> 0.5s"
        );
        assert!(
            (OcrState::calculate_dropoff_delay_secs(0.3, 3.0, 2) - 0.7).abs() < 1e-5,
            "2 consecutive same scans -> 0.7s"
        );
        assert!(
            (OcrState::calculate_dropoff_delay_secs(0.3, 3.0, 100) - 3.0).abs() < 1e-5,
            "High consecutive count clamps to max_dropoff 3.0s"
        );
    }

    #[test]
    fn test_are_ocr_boxes_equivalent_jitter_and_text() {
        let empty: Vec<OcrBox> = Vec::new();
        assert!(are_ocr_boxes_equivalent(
            &empty,
            &empty,
            DEFAULT_JITTER_TOLERANCE
        ));

        let box_a = OcrBox {
            text: "日本語 テスト".to_string(),
            center_x: 0.50,
            center_y: 0.80,
            width: 0.40,
            height: 0.10,
            lines: Vec::new(),
            writing_direction: WritingDirection::LeftToRight,
        };
        let box_jitter = OcrBox {
            text: "「日本語　テスト！」".to_string(), // full-width space & punctuation normalize the same
            center_x: 0.62,                           // 12% shift (within 15% tolerance)
            center_y: 0.68,                           // 12% shift (within 15% tolerance)
            width: 0.50,                              // 10% shift (within 15% tolerance)
            height: 0.18,                             // 8% shift (within 15% tolerance)
            lines: Vec::new(),
            writing_direction: WritingDirection::LeftToRight,
        };
        assert!(are_ocr_boxes_equivalent(
            &[box_a.clone()],
            &[box_jitter],
            DEFAULT_JITTER_TOLERANCE
        ));

        let box_moved = OcrBox {
            center_x: 0.70, // 20% difference (> 0.15 default tolerance, but <= 0.25 custom tolerance)
            ..box_a.clone()
        };
        assert!(!are_ocr_boxes_equivalent(
            &[box_a.clone()],
            &[box_moved.clone()],
            DEFAULT_JITTER_TOLERANCE
        ));
        assert!(are_ocr_boxes_equivalent(
            &[box_a.clone()],
            &[box_moved],
            0.25
        ));

        let box_diff_text = OcrBox {
            text: "別のテキスト".to_string(),
            ..box_a.clone()
        };
        assert!(!are_ocr_boxes_equivalent(
            &[box_a],
            &[box_diff_text],
            DEFAULT_JITTER_TOLERANCE
        ));
    }

    #[test]
    fn test_apply_constant_scan_lines_same_and_no_text_backoff() {
        let mut state = OcrState::default();
        state.set_backend(OcrBackend::LocalScreenAi);
        state.constant_scanning = true;
        state.scan_interval_secs = 0.3;
        state.max_dropoff_secs = 3.0;

        // 1. First empty scan: establishes baseline (no backoff yet)
        let updated = state.apply_constant_scan_lines(Vec::new());
        assert!(!updated);
        assert!(state.has_previous_constant_scan);
        assert_eq!(state.consecutive_same_scans, 0);

        // 2. Second empty scan ("no text" treated same as "same text"): increments backoff
        let updated = state.apply_constant_scan_lines(Vec::new());
        assert!(!updated);
        assert_eq!(state.consecutive_same_scans, 1);
        assert!((state.current_constant_scan_delay().as_secs_f32() - 0.5).abs() < 1e-4);

        // 3. New Japanese text appears: updates boxes and resets backoff to 0
        let line = ParsedLine {
            text: "こんにちは".to_string(),
            center_x: 0.5,
            center_y: 0.8,
            width: 0.3,
            height: 0.08,
            paragraph_idx: 0,
            writing_direction: WritingDirection::LeftToRight,
            char_spans: Vec::new(),
        };
        let updated = state.apply_constant_scan_lines(vec![line.clone()]);
        assert!(updated);
        assert_eq!(state.boxes.len(), 1);
        assert_eq!(state.consecutive_same_scans, 0);

        // 4. Same text on next scan (with tiny jitter): discarded, boxes untouched, backoff increments
        let jittered_line = ParsedLine {
            center_x: 0.505,
            ..line
        };
        let prev_scan_time = state.last_scan_time;
        let updated = state.apply_constant_scan_lines(vec![jittered_line]);
        assert!(!updated);
        assert_eq!(state.consecutive_same_scans, 1);
        assert_eq!(state.last_scan_time, prev_scan_time);
        assert!((state.boxes[0].center_x - 0.5).abs() < 1e-6);

        // 5. Text disappears (empty scan after non-empty): clears boxes and resets backoff
        let updated = state.apply_constant_scan_lines(Vec::new());
        assert!(updated);
        assert!(state.boxes.is_empty());
        assert_eq!(state.consecutive_same_scans, 0);
    }

    #[test]
    fn test_constant_scanning_disables_box_expiration() {
        let mut state = OcrState::default();
        state.timeout_seconds = 10;
        state.boxes.push(OcrBox {
            text: "テスト".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.2,
            height: 0.1,
            lines: Vec::new(),
            writing_direction: WritingDirection::LeftToRight,
        });
        state.last_scan_time = Some(Instant::now() - std::time::Duration::from_secs(60));

        // Expired when constant scanning is off
        assert!(state.is_expired());

        // Not expired when constant scanning is active with LocalScreenAi
        state.set_backend(OcrBackend::LocalScreenAi);
        state.constant_scanning = true;
        assert!(!state.is_expired());
        assert_eq!(state.remaining_time(), None);
    }

    #[test]
    fn test_f28_nonfinite_timing_and_jitter_do_not_panic() {
        assert_eq!(
            OcrState::calculate_dropoff_delay_secs(f32::NAN, f32::NAN, 0),
            DEFAULT_SCAN_INTERVAL_SECS
        );
        assert_eq!(
            OcrState::calculate_dropoff_delay_secs(f32::INFINITY, f32::NEG_INFINITY, 5),
            DEFAULT_SCAN_INTERVAL_SECS + DROPOFF_STEP_SECS * 5.0
        );

        let mut state = OcrState::default();
        state.scan_interval_secs = f32::NAN;
        state.max_dropoff_secs = f32::NAN;
        state.jitter_tolerance = f32::NAN;
        state.sticky_distance = f32::NAN;
        let delay = state.current_constant_scan_delay();
        assert!((delay.as_secs_f32() - DEFAULT_SCAN_INTERVAL_SECS).abs() < 1e-4);

        let box_a = OcrBox {
            text: "テスト".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.2,
            height: 0.1,
            lines: Vec::new(),
            writing_direction: WritingDirection::LeftToRight,
        };
        assert!(is_same_ocr_box(&box_a, &box_a, f32::NAN));
    }

    #[test]
    fn test_f23_generation_and_request_id_reject_stale_completions() {
        let mut state = OcrState::default();
        let (req_tx, req_rx) = crossbeam_channel::unbounded();
        state.ocr_request_sender = Some(req_tx);

        // Dispatch request 1 in generation 1
        let handle1 = state.dispatch_handle();
        let req1_id = handle1
            .dispatch(vec![0u8; 16], 2, 2)
            .expect("request 1 should dispatch");
        let captured1 = req_rx.try_recv().expect("captured request 1");
        assert_eq!(captured1.request_id, req1_id);
        assert_eq!(captured1.generation, 1);
        assert!(state.is_processing.load(Ordering::Acquire));

        // Clear OCR state while request 1 is in flight -> bumps generation to 2 and clears is_processing
        state.clear();
        assert_eq!(state.current_generation(), 2);
        assert!(!state.is_processing.load(Ordering::Acquire));

        // Dispatch request 2 in generation 2 while request 1 is still running on worker
        state.set_backend(OcrBackend::LocalScreenAi);
        assert_eq!(state.current_generation(), 3);
        let handle2 = state.dispatch_handle();
        let req2_id = handle2
            .dispatch(vec![0u8; 16], 2, 2)
            .expect("request 2 should dispatch");
        let captured2 = req_rx.try_recv().expect("captured request 2");
        assert_eq!(captured2.request_id, req2_id);
        assert_eq!(captured2.generation, 3);
        assert!(state.is_processing.load(Ordering::Acquire));

        // Stale completion for request 1 arrives: must be rejected AND must not clear is_processing for request 2
        let stale_res = OcrWorkerResult {
            request_id: captured1.request_id,
            generation: captured1.generation,
            backend: captured1.backend,
            constant_scan: captured1.constant_scan,
            result: Ok(vec![ParsedLine {
                text: "古い結果".to_string(),
                ..Default::default()
            }]),
        };
        assert!(!state.accept_worker_result(&stale_res));
        assert!(
            state.is_processing.load(Ordering::Acquire),
            "stale completion must not clear is_processing while request 2 is active"
        );

        // Fresh completion for request 2 arrives: accepted and clears is_processing
        let fresh_res = OcrWorkerResult {
            request_id: captured2.request_id,
            generation: captured2.generation,
            backend: captured2.backend,
            constant_scan: captured2.constant_scan,
            result: Ok(vec![ParsedLine {
                text: "新しい結果".to_string(),
                ..Default::default()
            }]),
        };
        assert!(state.accept_worker_result(&fresh_res));
        assert!(!state.is_processing.load(Ordering::Acquire));
    }
}
