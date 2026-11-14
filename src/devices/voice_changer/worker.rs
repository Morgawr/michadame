use crate::config::{VcExecutionProvider, VoiceChangerConfig, VoiceChangerPreset};
use crate::devices::voice_changer::model_sync::{
    query_vc_install_status, resolve_voice_changer_directory,
};
use crate::devices::voice_changer::resampler::StreamingResampler;
use crate::devices::voice_changer::runner::{ExtractedFeatures, FeatureExtractor, VoiceVocoder};
use crate::devices::voice_changer::sola::SolaStitcher;
use crate::devices::voice_changer::{VoiceChangerStatus, VoiceChangerTelemetry};
use crossbeam_channel::{Receiver, SendTimeoutError, Sender};
use rtrb::{Consumer, Producer, RingBuffer};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const SOLA_OVERLAP_MS: f32 = 28.0;
const SOLA_SEARCH_MS: f32 = 12.0;
const RIGHT_MARGIN_MS: usize = 20;
const SILENCE_HANGOVER_CHUNKS: u32 = 3;

#[derive(Clone, Debug, PartialEq)]
struct WorkerParams {
    execution_provider: VcExecutionProvider,
    preset: VoiceChangerPreset,
}

impl WorkerParams {
    fn from_config(cfg: &VoiceChangerConfig) -> Self {
        Self {
            execution_provider: cfg.execution_provider,
            preset: cfg.current.sanitized(),
        }
    }
}

enum Stage1Action {
    MissingModels,
    Error,
    Silence,
    Synthesize {
        feats: Box<ExtractedFeatures>,
        stage1_ms: f32,
        formant_ratio: f64,
    },
}

struct Stage1Msg {
    params: WorkerParams,
    chunk_frames: usize,
    target_prime: usize,
    force_reload: bool,
    action: Stage1Action,
}

/// Real-time lock-free bridge owned by `MicDspPipeline` on the ALSA capture thread.
/// Communicates with the 2-stage pipelined inference workers (`michadame-vc-feat` and
/// `michadame-vc-synth`) via lock-free `rtrb` ring buffers.
pub struct VoiceChangerBridge {
    sample_rate: u32,
    ring_cap: usize,
    in_prod: Producer<f32>,
    out_cons: Consumer<f32>,
    dry_delay: VecDeque<f32>,
    shared_params: Arc<RwLock<WorkerParams>>,
    last_params: WorkerParams,
    stop: Arc<AtomicBool>,
    underrun_deficit: Arc<AtomicUsize>,
    feat_thread: Option<JoinHandle<()>>,
    synth_thread: Option<JoinHandle<()>>,
    telemetry: Arc<VoiceChangerTelemetry>,
    last_wet_sample: f32,
    primed_nominal_delay: usize,
}

impl VoiceChangerBridge {
    pub fn spawn(
        sample_rate: u32,
        initial_cfg: &VoiceChangerConfig,
        telemetry: Arc<VoiceChangerTelemetry>,
    ) -> Self {
        let sr = sample_rate.max(8_000);
        // 2.0 seconds lock-free ring capacity at capture rate
        let ring_cap = (sr as usize) * 2;
        let (in_prod, in_cons) = RingBuffer::<f32>::new(ring_cap);
        let (mut out_prod, out_cons) = RingBuffer::<f32>::new(ring_cap);

        let initial_params = WorkerParams::from_config(initial_cfg);
        let initial_prime = compute_ring_prime_frames(sr, initial_params.preset.chunk_ms);
        let initial_delay = compute_nominal_delay_frames(sr, initial_params.preset.chunk_ms);

        // Pre-prime output ring with pipeline cushion so speech onset + parallel Stage 1/2
        // inference never underrun `out_cons`.
        for _ in 0..initial_prime {
            let _ = out_prod.push(0.0);
        }
        // `dry_delay` holds the full acoustic delay (`prime + right_margin + sola`) so
        // dry/wet blending and Replay/Twitch/VTuber sync are sample-aligned.
        let mut dry_delay = VecDeque::with_capacity(initial_delay + 4096);
        dry_delay.resize(initial_delay, 0.0);

        telemetry.set_delay_frames(initial_delay as u32);
        telemetry.set_latency_ms(initial_delay as f32 * 1000.0 / sr as f32);

        let shared_params = Arc::new(RwLock::new(initial_params.clone()));
        let stop = Arc::new(AtomicBool::new(false));
        let underrun_deficit = Arc::new(AtomicUsize::new(0));

        let (stage_tx, stage_rx) = crossbeam_channel::bounded::<Stage1Msg>(1);
        let (recycle_tx, recycle_rx) = crossbeam_channel::bounded::<Box<ExtractedFeatures>>(3);
        for _ in 0..3 {
            let _ = recycle_tx.try_send(Box::default());
        }

        let stop_for_synth = Arc::clone(&stop);
        let telemetry_for_synth = Arc::clone(&telemetry);
        let deficit_for_synth = Arc::clone(&underrun_deficit);

        let synth_thread = thread::Builder::new()
            .name("michadame-vc-synth".to_string())
            .spawn(move || {
                run_vc_synth_worker(
                    sr,
                    ring_cap,
                    initial_prime,
                    stage_rx,
                    recycle_tx,
                    out_prod,
                    stop_for_synth,
                    deficit_for_synth,
                    telemetry_for_synth,
                );
            })
            .ok();

        let params_for_feat = Arc::clone(&shared_params);
        let stop_for_feat = Arc::clone(&stop);
        let telemetry_for_feat = Arc::clone(&telemetry);

        let feat_thread = thread::Builder::new()
            .name("michadame-vc-feat".to_string())
            .spawn(move || {
                run_vc_feat_worker(
                    sr,
                    in_cons,
                    stage_tx,
                    recycle_rx,
                    params_for_feat,
                    stop_for_feat,
                    telemetry_for_feat,
                );
            })
            .ok();

        Self {
            sample_rate: sr,
            ring_cap,
            in_prod,
            out_cons,
            dry_delay,
            shared_params,
            last_params: initial_params,
            stop,
            underrun_deficit,
            feat_thread,
            synth_thread,
            telemetry,
            last_wet_sample: 0.0,
            primed_nominal_delay: initial_delay,
        }
    }

    pub fn delay_frames(&self) -> u32 {
        self.telemetry.delay_frames()
    }

    /// Processes `samples` (interleaved `channels`) through the lock-free voice changer bridge,
    /// applying post-VC output gain trim and phase-aligned dry/wet mix.
    pub fn process_interleaved(
        &mut self,
        samples: &mut [f32],
        channels: usize,
        cfg: &VoiceChangerConfig,
    ) {
        if samples.is_empty() || channels == 0 {
            return;
        }

        // Push parameter updates without locking when nothing changed
        if self.last_params.execution_provider != cfg.execution_provider
            || self.last_params.preset != cfg.current
        {
            let next = WorkerParams::from_config(cfg);
            if let Ok(mut w) = self.shared_params.try_write() {
                *w = next.clone();
                self.last_params = next;
            }
        }

        let target_delay =
            compute_nominal_delay_frames(self.sample_rate, self.last_params.preset.chunk_ms);
        if target_delay != self.primed_nominal_delay {
            self.primed_nominal_delay = target_delay;
            while self.dry_delay.len() < target_delay {
                self.dry_delay.push_front(0.0);
            }
            while self.dry_delay.len() > target_delay {
                self.dry_delay.pop_front();
            }
        }

        // Bound output queue backlog so lowering Chunk Size or recovering from an OS stall
        // trims excess latency immediately.
        let chunk_frames =
            ((self.sample_rate as usize * self.last_params.preset.chunk_ms as usize) / 1000)
                .max(160);
        let target_prime =
            compute_ring_prime_frames(self.sample_rate, self.last_params.preset.chunk_ms);
        let max_backlog = target_prime + chunk_frames + (self.sample_rate as usize * 35) / 1000;
        while self.out_cons.slots() > max_backlog {
            let _ = self.out_cons.pop();
        }

        let worker_active = self.telemetry.status() == VoiceChangerStatus::Active;
        let out_gain = 10.0f32.powf(self.last_params.preset.output_gain_db / 20.0);
        let wet_mix = self.last_params.preset.dry_wet.clamp(0.0, 1.0);
        let dry_mix = 1.0 - wet_mix;

        let mut missing_out = 0usize;

        for frame in samples.chunks_exact_mut(channels) {
            let mono_in = if channels == 1 {
                frame[0]
            } else {
                0.5 * (frame[0] + frame[1])
            };

            let _ = self.in_prod.push(mono_in);
            self.dry_delay.push_back(mono_in);
            let delayed_dry = self.dry_delay.pop_front().unwrap_or(mono_in);

            let wet_raw = match self.out_cons.pop() {
                Ok(s) => {
                    self.last_wet_sample = s;
                    s
                }
                Err(_) => {
                    missing_out += 1;
                    self.last_wet_sample *= 0.992;
                    self.last_wet_sample
                }
            };

            let mixed = if worker_active {
                (wet_raw * out_gain) * wet_mix + delayed_dry * dry_mix
            } else {
                // Keep microphone audible while models download or warm up
                delayed_dry
            };

            for ch in frame.iter_mut() {
                *ch = mixed;
            }
        }

        if missing_out > 0 {
            self.underrun_deficit
                .fetch_add(missing_out, Ordering::Relaxed);
            if worker_active {
                self.telemetry.set_xrun(true);
            }
        }

        // Wake Stage 1 immediately as soon as a full chunk is ready in `in_prod`
        if self.ring_cap.saturating_sub(self.in_prod.slots()) >= chunk_frames {
            if let Some(h) = &self.feat_thread {
                h.thread().unpark();
            }
        }
    }
}

impl Drop for VoiceChangerBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = &self.feat_thread {
            h.thread().unpark();
        }
        if let Some(handle) = self.feat_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.synth_thread.take() {
            let _ = handle.join();
        }
        self.telemetry.set_status(VoiceChangerStatus::Disabled);
        self.telemetry.set_delay_frames(0);
        self.telemetry.set_latency_ms(0.0);
        self.telemetry.set_inference_ms(0.0);
        self.telemetry.set_xrun(false);
    }
}

/// Ring-buffer pre-prime frames needed so 2-stage pipeline fill (`Stage 1 + Stage 2`)
/// on cold speech onset plus parallel steady-state chunks never underrun `out_cons`.
pub fn compute_ring_prime_frames(sample_rate: u32, chunk_ms: u32) -> usize {
    let sr = sample_rate.max(8_000) as usize;
    let chunk_frames = ((sr * chunk_ms as usize) / 1000).max(160);
    let cushion_frames = (chunk_frames * 2).max((sr * 145) / 1000);
    chunk_frames + cushion_frames
}

/// Total nominal acoustic delay (`ring_prime + right_margin + sola_delay`) used for
/// phase-aligned dry/wet mixing and Replay / Twitch / VTuber lip-sync compensation.
pub fn compute_nominal_delay_frames(sample_rate: u32, chunk_ms: u32) -> usize {
    let sr = sample_rate.max(8_000);
    let prime_frames = compute_ring_prime_frames(sr, chunk_ms);
    let overlap_frames = ((sr as f32 * SOLA_OVERLAP_MS / 1000.0).round() as usize).max(32);
    let search_frames = ((sr as f32 * SOLA_SEARCH_MS / 1000.0).round() as usize).max(16);
    let right_margin_frames = (sr as usize * RIGHT_MARGIN_MS) / 1000;
    prime_frames + right_margin_frames + overlap_frames + search_frames / 2
}

fn send_stage1_msg(tx: &Sender<Stage1Msg>, stop: &AtomicBool, mut msg: Stage1Msg) -> bool {
    while !stop.load(Ordering::Relaxed) {
        match tx.send_timeout(msg, Duration::from_millis(20)) {
            Ok(()) => return true,
            Err(SendTimeoutError::Timeout(returned)) => {
                msg = returned;
            }
            Err(SendTimeoutError::Disconnected(_)) => return false,
        }
    }
    false
}

/// Stage 1 worker (`michadame-vc-feat`): resamples mic chunks to 16 kHz, maintains the
/// sliding phonetic context window, runs `ContentVec` + tail `MelSTFT`, and feeds Stage 2.
fn run_vc_feat_worker(
    sample_rate: u32,
    mut in_cons: Consumer<f32>,
    stage_tx: Sender<Stage1Msg>,
    recycle_rx: Receiver<Box<ExtractedFeatures>>,
    shared_params: Arc<RwLock<WorkerParams>>,
    stop: Arc<AtomicBool>,
    telemetry: Arc<VoiceChangerTelemetry>,
) {
    let vc_dir = resolve_voice_changer_directory();
    let mut params = shared_params
        .read()
        .map(|g| g.clone())
        .unwrap_or_else(|_| WorkerParams::from_config(&VoiceChangerConfig::default()));

    let mut extractor: Option<FeatureExtractor> = None;
    let mut failed_ep: Option<VcExecutionProvider> = None;

    let mut in_resampler = StreamingResampler::new(sample_rate as f64, 16_000.0);
    let overlap_frames = ((sample_rate as f32 * SOLA_OVERLAP_MS / 1000.0).round() as usize).max(32);
    let search_frames = ((sample_rate as f32 * SOLA_SEARCH_MS / 1000.0).round() as usize).max(16);

    let mut chunk_in_buf: Vec<f32> = Vec::with_capacity(8192);
    let mut resampled_16k_chunk: Vec<f32> = Vec::with_capacity(4096);
    let mut context_16k: Vec<f32> = Vec::with_capacity(16_000);
    let mut hangover_chunks = 0u32;

    while !stop.load(Ordering::Relaxed) {
        if let Ok(latest) = shared_params.try_read() {
            if *latest != params {
                params = latest.clone();
                failed_ep = None;
            }
        }

        let mut force_reload = false;
        if telemetry.take_reload_request() {
            extractor = None;
            failed_ep = None;
            context_16k.clear();
            hangover_chunks = 0;
            force_reload = true;
        }

        let chunk_ms = params.preset.chunk_ms;
        let chunk_frames = ((sample_rate as usize * chunk_ms as usize) / 1000).max(160);
        let target_prime = compute_ring_prime_frames(sample_rate, chunk_ms);
        let nominal_delay = compute_nominal_delay_frames(sample_rate, chunk_ms);

        telemetry.set_delay_frames(nominal_delay as u32);
        telemetry.set_latency_ms(nominal_delay as f32 * 1000.0 / sample_rate as f32);

        if in_cons.slots() < chunk_frames {
            thread::park_timeout(Duration::from_millis(2));
            continue;
        }

        // Drop stale input backlog if a model load or desktop stall accumulated old mic data
        let max_stale_in = chunk_frames * 2 + (sample_rate as usize * 40) / 1000;
        if in_cons.slots() > max_stale_in {
            while in_cons.slots() > chunk_frames {
                let _ = in_cons.pop();
            }
        }

        chunk_in_buf.clear();
        for _ in 0..chunk_frames {
            if let Ok(s) = in_cons.pop() {
                chunk_in_buf.push(s);
            }
        }

        if extractor
            .as_ref()
            .is_some_and(|e| e.execution_provider_pref() != params.execution_provider)
        {
            extractor = None;
            failed_ep = None;
            force_reload = true;
        }

        let install_status = query_vc_install_status(&vc_dir);
        let voice_ready = match params.preset.model_kind {
            crate::config::VoiceModelKind::BuiltIn => {
                install_status.is_voice_installed(params.preset.builtin_model)
            }
            crate::config::VoiceModelKind::CustomOnnx => {
                let p = params.preset.custom_model_path.trim();
                !p.is_empty() && std::path::Path::new(p).is_file()
            }
        };

        if !install_status.is_engine_ready() || !voice_ready {
            hangover_chunks = 0;
            if !send_stage1_msg(
                &stage_tx,
                &stop,
                Stage1Msg {
                    params: params.clone(),
                    chunk_frames,
                    target_prime,
                    force_reload,
                    action: Stage1Action::MissingModels,
                },
            ) {
                break;
            }
            continue;
        }

        if failed_ep == Some(params.execution_provider) {
            if !send_stage1_msg(
                &stage_tx,
                &stop,
                Stage1Msg {
                    params: params.clone(),
                    chunk_frames,
                    target_prime,
                    force_reload,
                    action: Stage1Action::Error,
                },
            ) {
                break;
            }
            continue;
        }

        if extractor.is_none() {
            telemetry.set_status(VoiceChangerStatus::Loading);
            telemetry.set_active_ep_label("Loading ONNX Engine...");
            match FeatureExtractor::load(&vc_dir, params.execution_provider) {
                Ok(ext) => {
                    telemetry.set_last_error(None);
                    extractor = Some(ext);
                }
                Err(err) => {
                    tracing::error!("Voice Changer Stage 1 feature extractor load failed: {err}");
                    telemetry.set_last_error(Some(err));
                    telemetry.set_status(VoiceChangerStatus::Error);
                    failed_ep = Some(params.execution_provider);
                    if !send_stage1_msg(
                        &stage_tx,
                        &stop,
                        Stage1Msg {
                            params: params.clone(),
                            chunk_frames,
                            target_prime,
                            force_reload,
                            action: Stage1Action::Error,
                        },
                    ) {
                        break;
                    }
                    continue;
                }
            }
        }

        // Compute chunk RMS in dB for soft silence gate + hangover
        let mut sum_sq = 0.0f64;
        for &s in &chunk_in_buf {
            sum_sq += (s as f64) * (s as f64);
        }
        let rms = (sum_sq / chunk_in_buf.len().max(1) as f64).sqrt() as f32;
        let rms_db = if rms > 1e-6 {
            20.0 * rms.log10()
        } else {
            -100.0
        };

        // Independent vocal-tract formant shift ratio: `2^(formant_st / 12)`
        let formant_ratio = 2.0f64.powf(params.preset.formant_shift_semitones as f64 / 12.0);
        let eff_in_16k = 16_000.0 * formant_ratio;
        in_resampler.set_rates(sample_rate as f64, eff_in_16k);

        resampled_16k_chunk.clear();
        in_resampler.process(&chunk_in_buf, &mut resampled_16k_chunk);

        // Scale max context cap gently on short 40ms/60ms/80ms chunks so CPU ContentVec
        // always finishes well inside the chunk budget alongside video/CRT rendering.
        let max_ctx_for_chunk = match chunk_ms {
            0..=45 => 180,
            46..=65 => 220,
            66..=90 => 280,
            _ => 640,
        };
        let effective_extra_ms = params.preset.extra_context_ms.min(max_ctx_for_chunk);
        let total_window_ms = chunk_ms + effective_extra_ms + 60;
        let raw_target_16k = ((total_window_ms as f64 / 1000.0) * eff_in_16k).round() as usize;
        let target_ctx_len = raw_target_16k.div_ceil(320).max(8) * 320;

        if context_16k.len() < target_ctx_len {
            let missing = target_ctx_len - context_16k.len();
            let mut grown = vec![0.0f32; missing];
            grown.extend_from_slice(&context_16k);
            context_16k = grown;
        }
        context_16k.extend_from_slice(&resampled_16k_chunk);
        if context_16k.len() > target_ctx_len {
            let drain_n = context_16k.len() - target_ctx_len;
            context_16k.drain(0..drain_n);
        }

        let above_gate = rms_db >= params.preset.silence_threshold_db;
        if above_gate {
            hangover_chunks = SILENCE_HANGOVER_CHUNKS;
        }
        let should_synthesize = if above_gate {
            true
        } else if hangover_chunks > 0 {
            hangover_chunks -= 1;
            true
        } else {
            false
        };

        if !should_synthesize {
            if !send_stage1_msg(
                &stage_tx,
                &stop,
                Stage1Msg {
                    params: params.clone(),
                    chunk_frames,
                    target_prime,
                    force_reload,
                    action: Stage1Action::Silence,
                },
            ) {
                break;
            }
            continue;
        }

        let req_out = chunk_frames + overlap_frames + search_frames;
        let usable_cv =
            ((req_out as f64 * formant_ratio * 50.0 / sample_rate as f64) + 0.15).ceil() as usize;
        // `usable_cv` + 1 frame right-edge trim (20 ms) + 3 frames NetG conv warmup (60 ms),
        // clamped <= 15 so RMVPE `t_padded` is always 32.
        let tail_cv_frames = (usable_cv + 1 + 3).clamp(8, 15);

        let mut feats_box = recycle_rx.try_recv().unwrap_or_default();
        let t0 = Instant::now();
        if let Some(ext) = extractor.as_mut() {
            match ext.extract_tail(&context_16k, tail_cv_frames, &mut feats_box) {
                Ok(()) => {
                    let stage1_ms = t0.elapsed().as_secs_f32() * 1000.0;
                    if !send_stage1_msg(
                        &stage_tx,
                        &stop,
                        Stage1Msg {
                            params: params.clone(),
                            chunk_frames,
                            target_prime,
                            force_reload,
                            action: Stage1Action::Synthesize {
                                feats: feats_box,
                                stage1_ms,
                                formant_ratio,
                            },
                        },
                    ) {
                        break;
                    }
                }
                Err(err) => {
                    tracing::error!("Voice Changer Stage 1 extraction error: {err}");
                    telemetry.set_last_error(Some(err));
                    telemetry.set_status(VoiceChangerStatus::Error);
                    failed_ep = Some(params.execution_provider);
                    if !send_stage1_msg(
                        &stage_tx,
                        &stop,
                        Stage1Msg {
                            params: params.clone(),
                            chunk_frames,
                            target_prime,
                            force_reload,
                            action: Stage1Action::Error,
                        },
                    ) {
                        break;
                    }
                }
            }
        }
    }
}

/// Stage 2 worker (`michadame-vc-synth`): runs GPU `RMVPE` + tail `NetG` synthesis concurrently
/// with Stage 1, trims corrupted right-edge frames, tracks input RMS envelope, stitches via
/// SOLA, and maintains exact output ring-buffer priming.
#[allow(clippy::too_many_arguments)]
fn run_vc_synth_worker(
    sample_rate: u32,
    ring_cap: usize,
    initial_prime: usize,
    stage_rx: Receiver<Stage1Msg>,
    recycle_tx: Sender<Box<ExtractedFeatures>>,
    mut out_prod: Producer<f32>,
    stop: Arc<AtomicBool>,
    underrun_deficit: Arc<AtomicUsize>,
    telemetry: Arc<VoiceChangerTelemetry>,
) {
    let vc_dir = resolve_voice_changer_directory();
    let mut vocoder: Option<VoiceVocoder> = None;
    let mut failed_signature: Option<WorkerParams> = None;

    let mut out_resampler = StreamingResampler::new(40_000.0, sample_rate as f64);
    let mut sola = SolaStitcher::new(sample_rate, SOLA_OVERLAP_MS, SOLA_SEARCH_MS);

    let mut gen_audio: Vec<f32> = Vec::with_capacity(16_384);
    let mut resampled_out_seg: Vec<f32> = Vec::with_capacity(16_384);
    let mut env_seg: Vec<f32> = Vec::with_capacity(16_384);
    let mut stitched_out: Vec<f32> = Vec::with_capacity(8192);

    let mut worker_primed_frames = initial_prime;
    let mut prev_env_gain = 0.0f32;
    let mut smoothed_infer_ms = 0.0f32;
    let mut xrun_hold_chunks = 0u32;

    while !stop.load(Ordering::Relaxed) {
        let msg = match stage_rx.recv_timeout(Duration::from_millis(20)) {
            Ok(m) => m,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        if msg.force_reload
            || vocoder
                .as_ref()
                .is_some_and(|v| v.execution_provider_pref() != msg.params.execution_provider)
        {
            vocoder = None;
            failed_signature = None;
            sola.reset();
            prev_env_gain = 0.0;
        }

        if failed_signature.as_ref().is_some_and(|f| *f != msg.params) {
            failed_signature = None;
        }

        match msg.action {
            Stage1Action::MissingModels => {
                telemetry.set_status(VoiceChangerStatus::MissingModels);
                telemetry.set_active_ep_label("Waiting for Model Download");
                telemetry.set_inference_ms(0.0);
                telemetry.set_xrun(false);
                xrun_hold_chunks = 0;
                sola.reset();
                prev_env_gain = 0.0;
                let _ = replenish_ring_cushion(
                    &mut out_prod,
                    ring_cap,
                    &mut worker_primed_frames,
                    msg.target_prime,
                    msg.chunk_frames,
                    &underrun_deficit,
                );
                for _ in 0..msg.chunk_frames {
                    let _ = out_prod.push(0.0);
                }
                continue;
            }
            Stage1Action::Error => {
                sola.reset();
                prev_env_gain = 0.0;
                let _ = replenish_ring_cushion(
                    &mut out_prod,
                    ring_cap,
                    &mut worker_primed_frames,
                    msg.target_prime,
                    msg.chunk_frames,
                    &underrun_deficit,
                );
                for _ in 0..msg.chunk_frames {
                    let _ = out_prod.push(0.0);
                }
                continue;
            }
            Stage1Action::Silence | Stage1Action::Synthesize { .. } => {}
        }

        if failed_signature.as_ref() == Some(&msg.params) {
            if let Stage1Action::Synthesize { feats, .. } = msg.action {
                let _ = recycle_tx.try_send(feats);
            }
            let _ = replenish_ring_cushion(
                &mut out_prod,
                ring_cap,
                &mut worker_primed_frames,
                msg.target_prime,
                msg.chunk_frames,
                &underrun_deficit,
            );
            for _ in 0..msg.chunk_frames {
                let _ = out_prod.push(0.0);
            }
            continue;
        }

        // Load Stage 2 base / voice model under `Loading` status so ALSA cleanly passes
        // dry audio without false XRUNs while GPU shaders compile.
        let mut just_loaded = false;
        if vocoder.is_none() {
            just_loaded = true;
            telemetry.set_status(VoiceChangerStatus::Loading);
            telemetry.set_active_ep_label("Loading ONNX Vocoder...");
            match VoiceVocoder::load_base(&vc_dir, msg.params.execution_provider) {
                Ok(v) => {
                    telemetry.set_active_ep_label(v.active_ep_label());
                    vocoder = Some(v);
                }
                Err(err) => {
                    tracing::error!("Voice Changer Stage 2 vocoder load failed: {err}");
                    telemetry.set_last_error(Some(err));
                    telemetry.set_status(VoiceChangerStatus::Error);
                    failed_signature = Some(msg.params.clone());
                    if let Stage1Action::Synthesize { feats, .. } = msg.action {
                        let _ = recycle_tx.try_send(feats);
                    }
                    let _ = replenish_ring_cushion(
                        &mut out_prod,
                        ring_cap,
                        &mut worker_primed_frames,
                        msg.target_prime,
                        msg.chunk_frames,
                        &underrun_deficit,
                    );
                    for _ in 0..msg.chunk_frames {
                        let _ = out_prod.push(0.0);
                    }
                    continue;
                }
            }
        }

        if let Some(voc) = vocoder.as_mut() {
            if voc.needs_voice_load(&msg.params.preset) {
                just_loaded = true;
                telemetry.set_status(VoiceChangerStatus::Loading);
                telemetry.set_active_ep_label("Compiling Voice Model...");
                sola.reset();
                prev_env_gain = 0.0;
            }
            if let Err(err) = voc.ensure_voice_loaded(&msg.params.preset) {
                tracing::error!("Voice Changer voice model load failed: {err}");
                telemetry.set_last_error(Some(err));
                telemetry.set_status(VoiceChangerStatus::Error);
                failed_signature = Some(msg.params.clone());
                if let Stage1Action::Synthesize { feats, .. } = msg.action {
                    let _ = recycle_tx.try_send(feats);
                }
                let _ = replenish_ring_cushion(
                    &mut out_prod,
                    ring_cap,
                    &mut worker_primed_frames,
                    msg.target_prime,
                    msg.chunk_frames,
                    &underrun_deficit,
                );
                for _ in 0..msg.chunk_frames {
                    let _ = out_prod.push(0.0);
                }
                continue;
            }
        }

        stitched_out.clear();
        let was_active = !just_loaded && telemetry.status() == VoiceChangerStatus::Active;

        match msg.action {
            Stage1Action::Silence => {
                sola.process_silence(msg.chunk_frames, &mut stitched_out);
                prev_env_gain = 0.0;
                smoothed_infer_ms *= 0.85;
                telemetry.set_inference_ms(smoothed_infer_ms);
                if let Some(voc) = vocoder.as_ref() {
                    telemetry.set_active_ep_label(voc.active_ep_label());
                }
                telemetry.set_last_error(None);

                let deficit = replenish_ring_cushion(
                    &mut out_prod,
                    ring_cap,
                    &mut worker_primed_frames,
                    msg.target_prime,
                    msg.chunk_frames,
                    &underrun_deficit,
                );
                if was_active && deficit > 0 {
                    xrun_hold_chunks = 8;
                    telemetry.set_xrun(true);
                } else if xrun_hold_chunks > 0 {
                    xrun_hold_chunks -= 1;
                } else {
                    telemetry.set_xrun(false);
                }

                for &s in &stitched_out {
                    let _ = out_prod.push(s.clamp(-1.0, 1.0));
                }
                // After `process_silence` has faded `prev_tail` to 0.0, top up any drift back
                // to full `target_prime` so the next speech onset always has 100% cushion.
                let cur_queued = ring_cap.saturating_sub(out_prod.slots());
                if cur_queued < msg.target_prime {
                    for _ in 0..(msg.target_prime - cur_queued) {
                        let _ = out_prod.push(0.0);
                    }
                }
                telemetry.set_status(VoiceChangerStatus::Active);
            }
            Stage1Action::Synthesize {
                feats,
                stage1_ms,
                formant_ratio,
            } => {
                if let Some(voc) = vocoder.as_mut() {
                    let t0 = Instant::now();
                    match voc.synthesize_tail(&feats, &msg.params.preset, &mut gen_audio) {
                        Ok(gen_sr) => {
                            let stage2_ms = t0.elapsed().as_secs_f32() * 1000.0;
                            let bottleneck_ms = stage1_ms.max(stage2_ms);
                            smoothed_infer_ms = if smoothed_infer_ms < 0.1 {
                                bottleneck_ms
                            } else {
                                0.7 * smoothed_infer_ms + 0.3 * bottleneck_ms
                            };
                            telemetry.set_inference_ms(smoothed_infer_ms);
                            telemetry.set_active_ep_label(voc.active_ep_label());
                            telemetry.set_last_error(None);

                            // Trim 20 ms (`gen_sr / 50`) right-edge lookahead where ContentVec /
                            // HiFi-GAN lack right-side receptive context.
                            let right_margin_gen =
                                ((gen_sr as usize) / 50).min(gen_audio.len() / 4);
                            let usable_end = gen_audio.len().saturating_sub(right_margin_gen);

                            let req_out = sola.required_segment_len(msg.chunk_frames);
                            let eff_gen_sr = (gen_sr as f64) * formant_ratio;
                            let needed_gen = ((req_out as f64 * eff_gen_sr / sample_rate as f64)
                                .ceil() as usize)
                                + 32;
                            let start_gen = usable_end.saturating_sub(needed_gen);
                            let tail_slice = &gen_audio[start_gen..usable_end];

                            out_resampler.reset();
                            out_resampler.set_rates(eff_gen_sr, sample_rate as f64);
                            resampled_out_seg.clear();
                            out_resampler.process(tail_slice, &mut resampled_out_seg);

                            env_seg.clear();
                            if resampled_out_seg.len() >= req_out {
                                env_seg.extend_from_slice(
                                    &resampled_out_seg[resampled_out_seg.len() - req_out..],
                                );
                            } else {
                                env_seg.resize(req_out - resampled_out_seg.len(), 0.0);
                                env_seg.extend_from_slice(&resampled_out_seg);
                            }

                            apply_input_envelope(
                                &mut env_seg,
                                &feats.tail_audio_16k,
                                sample_rate,
                                formant_ratio,
                                msg.params.preset.silence_threshold_db,
                                &mut prev_env_gain,
                            );

                            let _ = recycle_tx.try_send(feats);

                            sola.process(&env_seg, msg.chunk_frames, &mut stitched_out);

                            let deficit = replenish_ring_cushion(
                                &mut out_prod,
                                ring_cap,
                                &mut worker_primed_frames,
                                msg.target_prime,
                                msg.chunk_frames,
                                &underrun_deficit,
                            );

                            let over_budget =
                                smoothed_infer_ms > msg.params.preset.chunk_ms as f32 * 1.02;
                            if over_budget || (was_active && deficit > 0) {
                                xrun_hold_chunks = 8;
                                telemetry.set_xrun(true);
                            } else if xrun_hold_chunks > 0 {
                                xrun_hold_chunks -= 1;
                            } else {
                                telemetry.set_xrun(false);
                            }

                            for &s in &stitched_out {
                                let _ = out_prod.push(s.clamp(-1.0, 1.0));
                            }
                            telemetry.set_status(VoiceChangerStatus::Active);
                        }
                        Err(err) => {
                            let _ = recycle_tx.try_send(feats);
                            tracing::error!("Voice Changer Stage 2 synthesis error: {err}");
                            telemetry.set_last_error(Some(err));
                            telemetry.set_status(VoiceChangerStatus::Error);
                            failed_signature = Some(msg.params.clone());
                            sola.process_silence(msg.chunk_frames, &mut stitched_out);
                            prev_env_gain = 0.0;
                            let _ = replenish_ring_cushion(
                                &mut out_prod,
                                ring_cap,
                                &mut worker_primed_frames,
                                msg.target_prime,
                                msg.chunk_frames,
                                &underrun_deficit,
                            );
                            for &s in &stitched_out {
                                let _ = out_prod.push(s.clamp(-1.0, 1.0));
                            }
                        }
                    }
                } else {
                    let _ = recycle_tx.try_send(feats);
                }
            }
            Stage1Action::MissingModels | Stage1Action::Error => {}
        }
    }
}

/// Restores output ring cushion when `chunk_ms` increases or after any transient underrun,
/// returning the underrun deficit observed since the previous chunk.
fn replenish_ring_cushion(
    out_prod: &mut Producer<f32>,
    ring_cap: usize,
    worker_primed_frames: &mut usize,
    target_prime: usize,
    chunk_frames: usize,
    underrun_deficit: &AtomicUsize,
) -> usize {
    if target_prime > *worker_primed_frames {
        let grow = target_prime - *worker_primed_frames;
        for _ in 0..grow {
            let _ = out_prod.push(0.0);
        }
    }
    *worker_primed_frames = target_prime;

    let deficit = underrun_deficit.swap(0, Ordering::Relaxed);
    if deficit > 0 {
        let cur_queued = ring_cap.saturating_sub(out_prod.slots());
        let max_refill = target_prime.saturating_sub(cur_queued + chunk_frames);
        let refill = deficit.min(max_refill);
        for _ in 0..refill {
            let _ = out_prod.push(0.0);
        }
    }
    deficit
}

/// Tracks 10 ms input-to-output RMS envelope + smoothstep soft-knee silence gate so
/// normalized `ContentVec` vowels preserve natural microphone dynamics, whispers, and
/// word endings without abrupt gating cutoffs.
fn apply_input_envelope(
    seg_out: &mut [f32],
    tail_audio_16k: &[f32],
    sample_rate: u32,
    formant_ratio: f64,
    silence_threshold_db: f32,
    prev_gain: &mut f32,
) {
    if seg_out.is_empty() || tail_audio_16k.len() <= 320 {
        return;
    }

    let eff_in_16k = 16_000.0 * formant_ratio;
    let in_end = tail_audio_16k.len() - 320; // Exclude 20 ms right-edge lookahead
    let in_seg_len = ((seg_out.len() as f64 * eff_in_16k) / sample_rate as f64)
        .round()
        .max(16.0) as usize;
    let in_start = in_end.saturating_sub(in_seg_len);
    let in_seg = &tail_audio_16k[in_start..in_end];
    if in_seg.is_empty() {
        return;
    }

    let step_out = (sample_rate as usize / 100).max(32); // ~10 ms sub-frames
    let n_sub = (seg_out.len() / step_out).max(2);

    for b in 0..n_sub {
        let o0 = (b * seg_out.len()) / n_sub;
        let o1 = ((b + 1) * seg_out.len()) / n_sub;
        let i0 = (b * in_seg.len()) / n_sub;
        let i1 = (((b + 1) * in_seg.len()) / n_sub)
            .max(i0 + 1)
            .min(in_seg.len());

        let mut in_sq = 0.0f64;
        for &x in &in_seg[i0..i1] {
            in_sq += (x as f64) * (x as f64);
        }
        let rms_in = (in_sq / (i1 - i0).max(1) as f64).sqrt() as f32;

        let mut out_sq = 0.0f64;
        for &y in &seg_out[o0..o1] {
            out_sq += (y as f64) * (y as f64);
        }
        let rms_out = (out_sq / (o1 - o0).max(1) as f64).sqrt() as f32;

        let in_db = if rms_in > 1e-6 {
            20.0 * rms_in.log10()
        } else {
            -100.0
        };
        let knee_t = ((in_db - (silence_threshold_db - 4.0)) / 12.0).clamp(0.0, 1.0);
        let gate = knee_t * knee_t * (3.0 - 2.0 * knee_t);

        let denom = rms_out.max(0.04);
        let target_gain =
            ((rms_in / denom).clamp(0.0, 2.5).powf(0.65) * 1.25 * gate).clamp(0.0, 2.2);
        let g1 = 0.35 * (*prev_gain) + 0.65 * target_gain;
        let g0 = *prev_gain;
        let span = (o1 - o0).max(1) as f32;

        for (k, sample) in seg_out[o0..o1].iter_mut().enumerate() {
            let frac = (k as f32 + 1.0) / span;
            *sample *= g0 + (g1 - g0) * frac;
        }
        *prev_gain = g1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_prime_and_nominal_delay_invariants() {
        for sr in [44_100u32, 48_000] {
            let mut prev_prime = 0usize;
            let mut prev_delay = 0usize;
            for chunk_ms in [40u32, 60, 80, 120] {
                let prime = compute_ring_prime_frames(sr, chunk_ms);
                let delay = compute_nominal_delay_frames(sr, chunk_ms);
                let chunk_frames = (sr as usize * chunk_ms as usize) / 1000;
                // Prime must always exceed 1 full chunk + >= 100 ms inference cushion
                assert!(prime >= chunk_frames + (sr as usize * 100) / 1000);
                // Nominal acoustic delay strictly exceeds ring prime by lookahead
                assert!(delay > prime);
                assert!(prime >= prev_prime);
                assert!(delay >= prev_delay);
                prev_prime = prime;
                prev_delay = delay;
            }
        }
    }

    #[test]
    fn test_input_envelope_follower_attenuates_quiet_tails_smoothly() {
        let sr = 48_000u32;
        let mut seg_out = vec![0.25f32; 4800]; // Loud normalized NetG output (100 ms)
                                               // Quiet whisper input (-58 dB, below -52 dB threshold) + 320-sample right margin
        let tail_16k = vec![0.0012f32; 1600 + 320];
        let mut prev_gain = 1.0f32;
        apply_input_envelope(&mut seg_out, &tail_16k, sr, 1.0, -52.0, &mut prev_gain);
        // Tail end must fade smoothly to near-silence without discontinuities
        assert!(seg_out.last().copied().unwrap_or(1.0).abs() < 0.01);
        let max_step = seg_out
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_step < 0.02, "Envelope introduced click step {max_step}");
    }

    #[test]
    #[ignore]
    fn verify_live_realtime_streaming_zero_xruns() {
        let vc_dir = resolve_voice_changer_directory();
        let status = query_vc_install_status(&vc_dir);
        if !status.is_engine_ready()
            || !status.is_voice_installed(crate::config::VoiceModelId::TsukuyomiChan)
        {
            println!(
                "Skipping live VC benchmark: models not installed in {}",
                vc_dir.display()
            );
            return;
        }

        let sr = 48_000u32;
        let period = 480usize; // 10 ms real-time ALSA period
        let mut cfg = VoiceChangerConfig::default();
        cfg.enabled = true;
        cfg.execution_provider = VcExecutionProvider::AutoGpu;

        let telemetry = Arc::new(VoiceChangerTelemetry::new());
        let mut bridge = VoiceChangerBridge::spawn(sr, &cfg, Arc::clone(&telemetry));

        // Wait up to 4s while feeding real-time 10 ms silent ALSA callbacks until Active
        let start_wait = Instant::now();
        let mut period_buf = vec![0.0f32; period];
        let mut next_tick = Instant::now();
        while telemetry.status() != VoiceChangerStatus::Active {
            assert!(
                start_wait.elapsed() < Duration::from_secs(6),
                "Timed out waiting for VoiceChangerStatus::Active (got {:?}, err={:?})",
                telemetry.status(),
                telemetry.last_error()
            );
            period_buf.fill(0.0);
            bridge.process_interleaved(&mut period_buf, 1, &cfg);
            assert!(
                !telemetry.is_xrun(),
                "False XRUN reported during initial model compilation"
            );
            next_tick += Duration::from_millis(10);
            let now = Instant::now();
            if next_tick > now {
                thread::sleep(next_tick - now);
            }
        }
        println!(
            "Engine Active on '{}' in {:.0} ms",
            telemetry.active_ep_label(),
            start_wait.elapsed().as_secs_f64() * 1000.0
        );

        // Synthesize realistic harmonic vowel+formant test speech with silence transitions
        let gen_speech_sample = |sample_idx: usize| -> f32 {
            let t = sample_idx as f32 / sr as f32;
            // Alternate 0.25s silence -> 1.3s speech -> 0.25s silence
            let phase_in_cycle = t % 1.8;
            let env = if !(0.20..1.50).contains(&phase_in_cycle) {
                0.0
            } else if phase_in_cycle < 0.24 {
                (phase_in_cycle - 0.20) / 0.04
            } else if phase_in_cycle > 1.42 {
                (1.50 - phase_in_cycle) / 0.08
            } else {
                1.0
            };
            if env <= 0.0 {
                return 0.0;
            }
            let f0 = 135.0 + 18.0 * (2.0 * std::f32::consts::PI * 2.3 * t).sin();
            let mut s = 0.0f32;
            for h in 1..=8 {
                let amp = 1.0 / (h as f32).powf(0.85);
                s += amp * (2.0 * std::f32::consts::PI * f0 * (h as f32) * t).sin();
            }
            s * 0.12 * env
        };

        for &chunk_ms in &[60u32, 80, 120, 60] {
            cfg.current.chunk_ms = chunk_ms;
            let mut xrun_periods = 0usize;
            let mut max_infer_ms = 0.0f32;
            let mut out_rms_acc = 0.0f64;
            let mut out_samples = 0usize;
            let mut max_step = 0.0f32;
            let mut prev_s = 0.0f32;

            // Stream 1.8 seconds (180 x 10ms real-time ALSA periods) per chunk setting
            next_tick = Instant::now();
            for p in 0..180usize {
                let base_idx = p * period;
                for i in 0..period {
                    period_buf[i] = gen_speech_sample(base_idx + i);
                }
                bridge.process_interleaved(&mut period_buf, 1, &cfg);

                if telemetry.is_xrun() {
                    xrun_periods += 1;
                }
                max_infer_ms = max_infer_ms.max(telemetry.inference_ms());
                for &y in &period_buf {
                    assert!(y.is_finite(), "Non-finite output sample");
                    if out_samples > 0 {
                        max_step = max_step.max((y - prev_s).abs());
                    }
                    prev_s = y;
                    out_rms_acc += (y as f64) * (y as f64);
                    out_samples += 1;
                }

                next_tick += Duration::from_millis(10);
                let now = Instant::now();
                if next_tick > now {
                    thread::sleep(next_tick - now);
                }
            }

            let out_rms = (out_rms_acc / out_samples.max(1) as f64).sqrt();
            println!(
                "[chunk_ms={chunk_ms:>3}] xrun_periods={xrun_periods}, max_infer_ms={max_infer_ms:.1} ms, nominal_latency={:.0} ms, out_rms={out_rms:.4}, max_step={max_step:.4}",
                telemetry.latency_ms()
            );
            assert_eq!(
                xrun_periods, 0,
                "Expected 0 xruns at chunk_ms={chunk_ms}, got {xrun_periods}"
            );
            assert!(
                max_infer_ms < chunk_ms as f32,
                "Expected max_infer_ms < {chunk_ms} ms, got {max_infer_ms:.1} ms"
            );
            assert!(
                out_rms > 0.01,
                "Expected audible converted voice output at chunk_ms={chunk_ms}, got rms={out_rms}"
            );
        }
    }
}
