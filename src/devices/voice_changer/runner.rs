use crate::config::{VcExecutionProvider, VoiceChangerPreset, VoiceModelId, VoiceModelKind};
use crate::devices::voice_changer::model_sync::{
    builtin_voice_model_path, contentvec_model_path, rmvpe_model_path, runtime_dir,
    runtime_dylib_path, ORT_LIB_LINKNAME,
};
use crate::devices::voice_changer::rmvpe::{
    decode_rmvpe_salience, shift_and_quantize_f0, RmvpeFrontend, RMVPE_N_MEL,
};
use half::f16;
use ort::ep::{self, ExecutionProvider};
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::{DynValue, Tensor, TensorElementType, ValueType};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const CONTENTVEC_DIM: usize = 768;

static ORT_LOADED_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

fn default_cpu_threads() -> usize {
    let total = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    // Use up to 4 spinning threads for CPU ContentVec so video/CRT rendering,
    // Vulkan submission, and ALSA capture always have dedicated uncontended cores.
    (total / 2).clamp(2, 4)
}

/// Ensures `libonnxruntime` is dynamically loaded into `ort` once per process.
/// Never panics if no library is installed; returns a descriptive `Err` instead,
/// and allows retrying later after the user finishes a 1-click runtime download.
pub fn ensure_ort_initialized(
    vc_dir: &Path,
    preferred_ep: VcExecutionProvider,
) -> Result<PathBuf, String> {
    let mut guard = ORT_LOADED_PATH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = guard.as_ref() {
        return Ok(existing.clone());
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(env_path) = std::env::var("ORT_DYLIB_PATH") {
        if !env_path.trim().is_empty() {
            candidates.push(PathBuf::from(env_path.trim()));
        }
    }

    if preferred_ep == VcExecutionProvider::RocmMiGraphX {
        candidates.push(PathBuf::from("/opt/rocm/lib/libonnxruntime.so"));
    }

    candidates.push(runtime_dylib_path(vc_dir));
    candidates.push(runtime_dir(vc_dir).join(ORT_LIB_LINKNAME));
    candidates.push(PathBuf::from("/opt/rocm/lib/libonnxruntime.so"));
    candidates.push(PathBuf::from("/usr/lib64/libonnxruntime.so"));
    candidates.push(PathBuf::from("/usr/lib/x86_64-linux-gnu/libonnxruntime.so"));
    candidates.push(PathBuf::from("/usr/lib/libonnxruntime.so"));

    let mut last_err = String::from("ONNX Runtime library not found");
    for cand in candidates {
        if !cand.exists() {
            continue;
        }
        match ort::init_from(&cand) {
            Ok(builder) => {
                let _ = builder.with_name("michadame-vc").commit();
                if let Ok(env) = ort::environment::Environment::current() {
                    env.set_log_level(ort::logging::LogLevel::Error);
                }
                tracing::info!(
                    "Voice Changer initialized ONNX Runtime from {}",
                    cand.display()
                );
                *guard = Some(cand.clone());
                return Ok(cand);
            }
            Err(e) => {
                last_err = format!("{}: {e}", cand.display());
            }
        }
    }

    Err(format!(
        "Could not load ONNX Runtime ({last_err}). Click Download in the Voice Changer tab."
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EpChoice {
    WebGpuVulkan,
    MiGraphX,
    Rocm,
    Cuda,
    Cpu,
}

impl EpChoice {
    fn label(self) -> &'static str {
        match self {
            Self::WebGpuVulkan => "GPU (Vulkan / WebGPU)",
            Self::MiGraphX => "GPU (AMD MIGraphX)",
            Self::Rocm => "GPU (AMD ROCm)",
            Self::Cuda => "GPU (NVIDIA CUDA)",
            Self::Cpu => "CPU (Multi-Thread)",
        }
    }
}

fn candidate_eps(pref: VcExecutionProvider) -> &'static [EpChoice] {
    match pref {
        VcExecutionProvider::AutoGpu => &[
            EpChoice::WebGpuVulkan,
            EpChoice::MiGraphX,
            EpChoice::Rocm,
            EpChoice::Cuda,
            EpChoice::Cpu,
        ],
        VcExecutionProvider::WebGpuVulkan => &[EpChoice::WebGpuVulkan, EpChoice::Cpu],
        VcExecutionProvider::RocmMiGraphX => &[EpChoice::MiGraphX, EpChoice::Rocm, EpChoice::Cpu],
        VcExecutionProvider::Cuda => &[EpChoice::Cuda, EpChoice::Cpu],
        VcExecutionProvider::CpuOnly => &[EpChoice::Cpu],
    }
}

fn build_session_with_ep(
    model_path: &Path,
    pref: VcExecutionProvider,
) -> Result<(Session, &'static str), String> {
    if !model_path.is_file() {
        return Err(format!("Model file missing: {}", model_path.display()));
    }

    let cpu_threads = default_cpu_threads();
    let mut last_err = String::new();
    for &choice in candidate_eps(pref) {
        // Keep GPU session host helper threads minimal (1, non-spinning) so CPU-side Stage 1
        // ContentVec runs at full AVX2 speed without OpenMP/thread-pool contention.
        let is_cpu = choice == EpChoice::Cpu;
        let intra_threads = if is_cpu { cpu_threads } else { 1 };

        let mut builder =
            Session::builder().map_err(|e| format!("ORT SessionBuilder failed: {e}"))?;
        builder = builder
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .unwrap_or_else(|e| e.recover());
        builder = builder
            .with_intra_threads(intra_threads)
            .unwrap_or_else(|e| e.recover())
            .with_intra_op_spinning(is_cpu)
            .unwrap_or_else(|e| e.recover())
            .with_inter_threads(1)
            .unwrap_or_else(|e| e.recover())
            .with_inter_op_spinning(false)
            .unwrap_or_else(|e| e.recover());

        let reg_ok = match choice {
            EpChoice::WebGpuVulkan => {
                let ep = ep::WebGPU::default()
                    .with_dawn_backend_type(ep::webgpu::DawnBackendType::Vulkan);
                ep.is_available().unwrap_or(false) && ep.register(&mut builder).is_ok()
            }
            EpChoice::MiGraphX => {
                let ep = ep::MIGraphX::default();
                ep.is_available().unwrap_or(false) && ep.register(&mut builder).is_ok()
            }
            EpChoice::Rocm => {
                let ep = ep::ROCm::default();
                ep.is_available().unwrap_or(false) && ep.register(&mut builder).is_ok()
            }
            EpChoice::Cuda => {
                let ep = ep::CUDA::default();
                ep.is_available().unwrap_or(false) && ep.register(&mut builder).is_ok()
            }
            EpChoice::Cpu => true,
        };

        if !reg_ok {
            continue;
        }

        match builder.commit_from_file(model_path) {
            Ok(sess) => return Ok((sess, choice.label())),
            Err(e) => {
                last_err = format!("{} failed on {}: {e}", choice.label(), model_path.display());
                tracing::warn!("{last_err}");
            }
        }
    }

    Err(format!(
        "Failed to load ONNX session {}: {last_err}",
        model_path.display()
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContentVecSchema {
    /// `input_values: f32 [1, N]`, optional `attention_mask: i64 [1, N]` (`TigreGotico/voiceclonnx-rvc`)
    InputValues { has_attention_mask: bool },
    /// `source: f32 [1, 1, N]` (`vec-768-layer-12.onnx` / `ozada` format)
    Source3D,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NetGSchema {
    /// `wok000/vcclient_model` (`VC_CLIENT` v2 / v3): `feats`, `p_len`, `pitch`, `pitchf`, `sid`
    VcClient { has_pitch: bool, feats_f16: bool },
    /// Standard RVC WebUI / Applio / voiceclonnx export: `phone`, `phone_lengths`, `pitch`, `pitchf`, `ds`, `rnd`
    StandardRvc {
        has_pitch: bool,
        has_rnd: bool,
        feats_f16: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum LoadedVoiceKey {
    BuiltIn(VoiceModelId),
    Custom(PathBuf),
}

struct VoiceSynthesizer {
    session: Session,
    schema: NetGSchema,
    output_f16: bool,
    sample_rate: u32,
    upsample_feats_2x: bool,
}

impl VoiceSynthesizer {
    fn load(path: &Path, pref: VcExecutionProvider) -> Result<(Self, &'static str), String> {
        let (session, ep_label) = build_session_with_ep(path, pref)?;

        let mut input_names = Vec::new();
        let mut feats_f16 = false;
        for inp in session.inputs() {
            let name = inp.name().to_string();
            if (name == "feats" || name == "phone")
                && matches!(
                    inp.dtype(),
                    ValueType::Tensor {
                        ty: TensorElementType::Float16,
                        ..
                    }
                )
            {
                feats_f16 = true;
            }
            input_names.push(name);
        }

        let has_name = |target: &str| input_names.iter().any(|n| n == target);
        let schema = if has_name("feats") && has_name("p_len") {
            NetGSchema::VcClient {
                has_pitch: has_name("pitch") && has_name("pitchf"),
                feats_f16,
            }
        } else if has_name("phone") && has_name("phone_lengths") {
            NetGSchema::StandardRvc {
                has_pitch: has_name("pitch") && has_name("pitchf"),
                has_rnd: has_name("rnd"),
                feats_f16,
            }
        } else {
            return Err(format!(
                "Unsupported RVC ONNX input signature {:?} in {}",
                input_names,
                path.display()
            ));
        };

        let output_f16 = session.outputs().first().is_some_and(|o| {
            matches!(
                o.dtype(),
                ValueType::Tensor {
                    ty: TensorElementType::Float16,
                    ..
                }
            )
        });

        let sample_rate = detect_model_sample_rate(&session, path);

        let mut synth = Self {
            session,
            schema,
            output_f16,
            sample_rate,
            upsample_feats_2x: true,
        };

        // Warm up GPU compute pipelines across all streaming chunk tail sizes (40/60/80/120 ms)
        // and auto-detect whether the ONNX graph expects 100 Hz (`upsample_feats_2x = true`)
        // vs 50 Hz features.
        let mut f16_scratch = Vec::new();
        let mut rnd_scratch = Vec::new();
        let mut probe_out = Vec::new();
        for &probe_t in &[18usize, 20, 22, 26] {
            let dummy_feats = vec![0.0f32; probe_t * CONTENTVEC_DIM];
            let dummy_pitch = vec![1i64; probe_t];
            let dummy_pitchf = vec![0.0f32; probe_t];
            if synth
                .run_generator(
                    &dummy_feats,
                    &dummy_pitch,
                    &dummy_pitchf,
                    probe_t,
                    &mut f16_scratch,
                    &mut rnd_scratch,
                    &mut probe_out,
                )
                .is_ok()
                && !probe_out.is_empty()
                && probe_t == 22
            {
                let samples_per_frame = probe_out.len() / probe_t;
                // At 40k/48k, 100 Hz yields 400/480 samples/frame; 50 Hz yields 800/960.
                let threshold_50hz = (sample_rate as usize) / 70;
                synth.upsample_feats_2x = samples_per_frame < threshold_50hz;
            }
        }

        Ok((synth, ep_label))
    }

    #[allow(clippy::too_many_arguments)]
    fn run_generator(
        &mut self,
        feats: &[f32],
        pitch: &[i64],
        pitchf: &[f32],
        t_frames: usize,
        f16_buf: &mut Vec<f16>,
        rnd_buf: &mut Vec<f32>,
        audio_out: &mut Vec<f32>,
    ) -> Result<(), String> {
        let mut inputs: Vec<(&str, DynValue)> = Vec::with_capacity(6);
        let feat_len = t_frames * CONTENTVEC_DIM;

        match self.schema {
            NetGSchema::VcClient {
                has_pitch,
                feats_f16,
            } => {
                if feats_f16 {
                    f16_buf.clear();
                    f16_buf.extend(feats[..feat_len].iter().map(|&x| f16::from_f32(x)));
                    let v = Tensor::from_array((
                        [1usize, t_frames, CONTENTVEC_DIM],
                        f16_buf.clone().into_boxed_slice(),
                    ))
                    .map_err(|e| format!("feats f16 tensor error: {e}"))?
                    .into_dyn();
                    inputs.push(("feats", v));
                } else {
                    let v = Tensor::from_array((
                        [1usize, t_frames, CONTENTVEC_DIM],
                        feats[..feat_len].to_vec().into_boxed_slice(),
                    ))
                    .map_err(|e| format!("feats f32 tensor error: {e}"))?
                    .into_dyn();
                    inputs.push(("feats", v));
                }

                let p_len =
                    Tensor::from_array(([1usize], vec![t_frames as i64].into_boxed_slice()))
                        .map_err(|e| format!("p_len tensor error: {e}"))?
                        .into_dyn();
                inputs.push(("p_len", p_len));

                if has_pitch {
                    let p = Tensor::from_array((
                        [1usize, t_frames],
                        pitch[..t_frames].to_vec().into_boxed_slice(),
                    ))
                    .map_err(|e| format!("pitch tensor error: {e}"))?
                    .into_dyn();
                    let pf = Tensor::from_array((
                        [1usize, t_frames],
                        pitchf[..t_frames].to_vec().into_boxed_slice(),
                    ))
                    .map_err(|e| format!("pitchf tensor error: {e}"))?
                    .into_dyn();
                    inputs.push(("pitch", p));
                    inputs.push(("pitchf", pf));
                }

                let sid = Tensor::from_array(([1usize], vec![0i64].into_boxed_slice()))
                    .map_err(|e| format!("sid tensor error: {e}"))?
                    .into_dyn();
                inputs.push(("sid", sid));
            }
            NetGSchema::StandardRvc {
                has_pitch,
                has_rnd,
                feats_f16,
            } => {
                if feats_f16 {
                    f16_buf.clear();
                    f16_buf.extend(feats[..feat_len].iter().map(|&x| f16::from_f32(x)));
                    let v = Tensor::from_array((
                        [1usize, t_frames, CONTENTVEC_DIM],
                        f16_buf.clone().into_boxed_slice(),
                    ))
                    .map_err(|e| format!("phone f16 tensor error: {e}"))?
                    .into_dyn();
                    inputs.push(("phone", v));
                } else {
                    let v = Tensor::from_array((
                        [1usize, t_frames, CONTENTVEC_DIM],
                        feats[..feat_len].to_vec().into_boxed_slice(),
                    ))
                    .map_err(|e| format!("phone f32 tensor error: {e}"))?
                    .into_dyn();
                    inputs.push(("phone", v));
                }

                let p_len =
                    Tensor::from_array(([1usize], vec![t_frames as i64].into_boxed_slice()))
                        .map_err(|e| format!("phone_lengths tensor error: {e}"))?
                        .into_dyn();
                inputs.push(("phone_lengths", p_len));

                if has_pitch {
                    let p = Tensor::from_array((
                        [1usize, t_frames],
                        pitch[..t_frames].to_vec().into_boxed_slice(),
                    ))
                    .map_err(|e| format!("pitch tensor error: {e}"))?
                    .into_dyn();
                    let pf = Tensor::from_array((
                        [1usize, t_frames],
                        pitchf[..t_frames].to_vec().into_boxed_slice(),
                    ))
                    .map_err(|e| format!("pitchf tensor error: {e}"))?
                    .into_dyn();
                    inputs.push(("pitch", p));
                    inputs.push(("pitchf", pf));
                }

                let ds = Tensor::from_array(([1usize], vec![0i64].into_boxed_slice()))
                    .map_err(|e| format!("ds tensor error: {e}"))?
                    .into_dyn();
                inputs.push(("ds", ds));

                if has_rnd {
                    let rnd_len = 192 * t_frames;
                    if rnd_buf.len() < rnd_len {
                        // Deterministic low-variance pseudo-Gaussian excitation vector
                        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
                        rnd_buf.resize_with(rnd_len, || {
                            seed = seed
                                .wrapping_mul(6364136223846793005)
                                .wrapping_add(1442695040888963407);
                            let u = ((seed >> 40) as f32) / ((1u32 << 24) as f32);
                            (u - 0.5) * 0.6
                        });
                    }
                    if feats_f16 {
                        let rnd_f16: Vec<f16> = rnd_buf[..rnd_len]
                            .iter()
                            .map(|&x| f16::from_f32(x))
                            .collect();
                        let rnd_t = Tensor::from_array((
                            [1usize, 192, t_frames],
                            rnd_f16.into_boxed_slice(),
                        ))
                        .map_err(|e| format!("rnd f16 tensor error: {e}"))?
                        .into_dyn();
                        inputs.push(("rnd", rnd_t));
                    } else {
                        let rnd_t = Tensor::from_array((
                            [1usize, 192, t_frames],
                            rnd_buf[..rnd_len].to_vec().into_boxed_slice(),
                        ))
                        .map_err(|e| format!("rnd tensor error: {e}"))?
                        .into_dyn();
                        inputs.push(("rnd", rnd_t));
                    }
                }
            }
        }

        let outputs = self
            .session
            .run(inputs)
            .map_err(|e| format!("RVC generator inference failed: {e}"))?;

        if outputs.len() == 0 {
            return Err("RVC generator returned zero outputs".to_string());
        }

        let first = &outputs[0];
        audio_out.clear();
        if self.output_f16 {
            let (_shape, slice) = first
                .try_extract_tensor::<f16>()
                .map_err(|e| format!("Failed to extract f16 audio output: {e}"))?;
            audio_out.reserve(slice.len());
            for &v in slice {
                let s = v.to_f32();
                audio_out.push(if s.is_finite() {
                    s.clamp(-1.0, 1.0)
                } else {
                    0.0
                });
            }
        } else {
            let (_shape, slice) = first
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("Failed to extract f32 audio output: {e}"))?;
            audio_out.reserve(slice.len());
            for &s in slice {
                audio_out.push(if s.is_finite() {
                    s.clamp(-1.0, 1.0)
                } else {
                    0.0
                });
            }
        }

        Ok(())
    }
}

fn detect_model_sample_rate(session: &Session, path: &Path) -> u32 {
    if let Ok(meta) = session.metadata() {
        for key in ["sample_rate", "sr", "samplingRate"] {
            if let Some(val) = meta.custom(key) {
                if let Ok(sr) = val.trim().parse::<u32>() {
                    if (16_000..=96_000).contains(&sr) {
                        return sr;
                    }
                }
            }
        }
        // `VC_CLIENT` stores JSON inside custom key `"metadata"`: `{"samplingRate": 40000, ...}`
        if let Some(json_str) = meta.custom("metadata") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_str) {
                for field in ["samplingRate", "sample_rate", "sr"] {
                    if let Some(sr) = v.get(field).and_then(|x| x.as_u64()) {
                        let sr = sr as u32;
                        if (16_000..=96_000).contains(&sr) {
                            return sr;
                        }
                    }
                }
            }
        }
    }

    let fname = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if fname.contains("48k") || fname.contains("48000") {
        48_000
    } else if fname.contains("32k") || fname.contains("32000") {
        32_000
    } else {
        40_000
    }
}

/// Tail-sliced phonetic features + RMVPE log-mel spectrogram passed from Stage 1 (`FeatureExtractor`)
/// to Stage 2 (`VoiceVocoder`) in the pipelined real-time worker.
#[derive(Clone, Default)]
pub struct ExtractedFeatures {
    pub cv_tail_feats: Vec<f32>,
    pub tail_cv_frames: usize,
    pub mel_buf: Vec<f32>,
    pub n_mel_frames: usize,
    pub t_mel_padded: usize,
    pub tail_audio_16k: Vec<f32>,
}

/// Stage 1 of the real-time RVC pipeline: runs full-context `ContentVec` 768d extraction
/// (so `GroupNorm` and `pos_conv` see 400+ ms of past speech + lookahead) and computes
/// the tail-sliced 128-bin RMVPE log-mel spectrogram (`t_padded == 32`).
///
/// Under `AutoGpu` / `WebGpuVulkan`, `ContentVec` is bound to multi-thread CPU so Stage 1
/// executes completely in parallel with Stage 2 (`RMVPE` + `NetG` on Vulkan/WebGPU), avoiding
/// `HubertModel` dynamic-mask CPU/GPU sync stalls and Dawn pipeline switching.
pub struct FeatureExtractor {
    ep_pref: VcExecutionProvider,
    cv_session: Session,
    cv_schema: ContentVecSchema,
    rmvpe_frontend: RmvpeFrontend,
    cv_padded: Vec<f32>,
    cv_full_feats: Vec<f32>,
    attn_mask_buf: Vec<i64>,
}

impl FeatureExtractor {
    pub fn load(vc_dir: &Path, ep_pref: VcExecutionProvider) -> Result<Self, String> {
        ensure_ort_initialized(vc_dir, ep_pref)?;
        let cv_path = contentvec_model_path(vc_dir);

        let cv_ep_pref = match ep_pref {
            VcExecutionProvider::AutoGpu
            | VcExecutionProvider::WebGpuVulkan
            | VcExecutionProvider::CpuOnly => VcExecutionProvider::CpuOnly,
            VcExecutionProvider::RocmMiGraphX | VcExecutionProvider::Cuda => ep_pref,
        };

        let (mut cv_session, _cv_ep) = build_session_with_ep(&cv_path, cv_ep_pref)?;
        let cv_schema = {
            let mut has_source = false;
            let mut has_attn = false;
            for inp in cv_session.inputs() {
                match inp.name() {
                    "source" => has_source = true,
                    "attention_mask" => has_attn = true,
                    _ => {}
                }
            }
            if has_source {
                ContentVecSchema::Source3D
            } else {
                ContentVecSchema::InputValues {
                    has_attention_mask: has_attn,
                }
            }
        };

        // Warm up ContentVec session once at load time
        let warm_n = 8_720usize;
        let warm_wav = vec![0.0f32; warm_n];
        let mut warm_inputs: Vec<(&str, DynValue)> = Vec::with_capacity(2);
        match cv_schema {
            ContentVecSchema::Source3D => {
                if let Ok(t) =
                    Tensor::from_array(([1usize, 1, warm_n], warm_wav.into_boxed_slice()))
                {
                    warm_inputs.push(("source", t.into_dyn()));
                    let _ = cv_session.run(warm_inputs);
                }
            }
            ContentVecSchema::InputValues { has_attention_mask } => {
                if let Ok(iv) = Tensor::from_array(([1usize, warm_n], warm_wav.into_boxed_slice()))
                {
                    warm_inputs.push(("input_values", iv.into_dyn()));
                    if has_attention_mask {
                        if let Ok(mk) = Tensor::from_array((
                            [1usize, warm_n],
                            vec![1i64; warm_n].into_boxed_slice(),
                        )) {
                            warm_inputs.push(("attention_mask", mk.into_dyn()));
                        }
                    }
                    let _ = cv_session.run(warm_inputs);
                }
            }
        }

        Ok(Self {
            ep_pref,
            cv_session,
            cv_schema,
            rmvpe_frontend: RmvpeFrontend::new(),
            cv_padded: Vec::with_capacity(16_384),
            cv_full_feats: Vec::with_capacity(128 * CONTENTVEC_DIM),
            attn_mask_buf: Vec::with_capacity(16_384),
        })
    }

    pub fn execution_provider_pref(&self) -> VcExecutionProvider {
        self.ep_pref
    }

    /// Runs full-window `ContentVec` for accurate phonetic normalization, then slices only
    /// the last `max_tail_cv_frames` (50 Hz) frames + matching 16 kHz audio tail for `RMVPE`
    /// and `NetG` synthesis.
    pub fn extract_tail(
        &mut self,
        window_16k: &[f32],
        max_tail_cv_frames: usize,
        out: &mut ExtractedFeatures,
    ) -> Result<(), String> {
        if window_16k.len() < 640 {
            out.cv_tail_feats.clear();
            out.tail_cv_frames = 0;
            out.mel_buf.clear();
            out.n_mel_frames = 0;
            out.t_mel_padded = 0;
            out.tail_audio_16k.clear();
            return Ok(());
        }

        // Reflect-pad `(40, 40)` around ContentVec's 400-sample receptive field so `N` input
        // samples at 16 kHz yield `N / 320` exact 50 Hz phonetic frames.
        self.cv_padded.clear();
        self.cv_padded.reserve(window_16k.len() + 80);
        for i in 0..40 {
            let idx = (40 - i).min(window_16k.len() - 1);
            self.cv_padded.push(window_16k[idx]);
        }
        self.cv_padded.extend_from_slice(window_16k);
        for i in 0..40 {
            let idx = window_16k.len().saturating_sub(2 + i);
            self.cv_padded.push(window_16k[idx]);
        }

        let n_cv_samples = self.cv_padded.len();
        let mut cv_inputs: Vec<(&str, DynValue)> = Vec::with_capacity(2);
        match self.cv_schema {
            ContentVecSchema::Source3D => {
                let src = Tensor::from_array((
                    [1usize, 1, n_cv_samples],
                    self.cv_padded.clone().into_boxed_slice(),
                ))
                .map_err(|e| format!("ContentVec source tensor error: {e}"))?
                .into_dyn();
                cv_inputs.push(("source", src));
            }
            ContentVecSchema::InputValues { has_attention_mask } => {
                let iv = Tensor::from_array((
                    [1usize, n_cv_samples],
                    self.cv_padded.clone().into_boxed_slice(),
                ))
                .map_err(|e| format!("ContentVec input_values error: {e}"))?
                .into_dyn();
                cv_inputs.push(("input_values", iv));
                if has_attention_mask {
                    if self.attn_mask_buf.len() < n_cv_samples {
                        self.attn_mask_buf.resize(n_cv_samples, 1i64);
                    }
                    let mask = Tensor::from_array((
                        [1usize, n_cv_samples],
                        self.attn_mask_buf[..n_cv_samples]
                            .to_vec()
                            .into_boxed_slice(),
                    ))
                    .map_err(|e| format!("ContentVec attention_mask error: {e}"))?
                    .into_dyn();
                    cv_inputs.push(("attention_mask", mask));
                }
            }
        }

        let cv_t = {
            let cv_outputs = self
                .cv_session
                .run(cv_inputs)
                .map_err(|e| format!("ContentVec inference failed: {e}"))?;
            let (shape, slice) = cv_outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("ContentVec output extract error: {e}"))?;
            let frames = if shape.len() == 3 {
                shape[1].max(1) as usize
            } else {
                slice.len() / CONTENTVEC_DIM
            };
            self.cv_full_feats.clear();
            self.cv_full_feats
                .extend_from_slice(&slice[..frames * CONTENTVEC_DIM]);
            frames
        };

        let tail_cv = max_tail_cv_frames.clamp(2, cv_t);
        let cv_start = (cv_t - tail_cv) * CONTENTVEC_DIM;
        let cv_end = cv_t * CONTENTVEC_DIM;
        out.cv_tail_feats.clear();
        out.cv_tail_feats
            .extend_from_slice(&self.cv_full_feats[cv_start..cv_end]);
        out.tail_cv_frames = tail_cv;

        let tail_samples = (tail_cv * 320).min(window_16k.len());
        let tail_audio = &window_16k[window_16k.len() - tail_samples..];
        out.tail_audio_16k.clear();
        out.tail_audio_16k.extend_from_slice(tail_audio);

        let (n_mel_frames, t_mel_padded) = self
            .rmvpe_frontend
            .compute_log_mel(tail_audio, &mut out.mel_buf);
        out.n_mel_frames = n_mel_frames;
        out.t_mel_padded = t_mel_padded;

        Ok(())
    }
}

/// Stage 2 of the real-time RVC pipeline: runs GPU-accelerated `RMVPE` F0 decoding (`t_padded = 32`)
/// and tail-sliced `SynthesizerTrnMs768NSFsid` neural vocoding (`~18-26` frames at 100 Hz).
pub struct VoiceVocoder {
    vc_dir: PathBuf,
    ep_pref: VcExecutionProvider,
    active_ep_label: &'static str,
    rmvpe_session: Session,
    voice: Option<(LoadedVoiceKey, VoiceSynthesizer)>,
    upsampled_feats: Vec<f32>,
    f0_100hz: Vec<f32>,
    f0_aligned: Vec<f32>,
    coarse_pitch: Vec<i64>,
    f16_buf: Vec<f16>,
    rnd_buf: Vec<f32>,
}

impl VoiceVocoder {
    pub fn load_base(vc_dir: &Path, ep_pref: VcExecutionProvider) -> Result<Self, String> {
        ensure_ort_initialized(vc_dir, ep_pref)?;
        let rm_path = rmvpe_model_path(vc_dir);
        let (mut rmvpe_session, rm_ep) = build_session_with_ep(&rm_path, ep_pref)?;

        // Warm up RMVPE shaders at the exact streaming pad size (`t_padded = 32`) so the
        // first spoken syllable never stalls compiling Vulkan/WebGPU compute pipelines.
        if let Ok(dummy_mel) = Tensor::from_array((
            [1usize, RMVPE_N_MEL, 32usize],
            vec![-11.5f32; RMVPE_N_MEL * 32].into_boxed_slice(),
        )) {
            let _ = rmvpe_session.run(vec![("input", dummy_mel.into_dyn())]);
        }

        Ok(Self {
            vc_dir: vc_dir.to_path_buf(),
            ep_pref,
            active_ep_label: rm_ep,
            rmvpe_session,
            voice: None,
            upsampled_feats: Vec::with_capacity(64 * CONTENTVEC_DIM),
            f0_100hz: Vec::with_capacity(64),
            f0_aligned: Vec::with_capacity(64),
            coarse_pitch: Vec::with_capacity(64),
            f16_buf: Vec::new(),
            rnd_buf: Vec::new(),
        })
    }

    pub fn execution_provider_pref(&self) -> VcExecutionProvider {
        self.ep_pref
    }

    pub fn active_ep_label(&self) -> &'static str {
        self.active_ep_label
    }

    fn desired_voice_key(
        &self,
        preset: &VoiceChangerPreset,
    ) -> Result<(LoadedVoiceKey, PathBuf), String> {
        match preset.model_kind {
            VoiceModelKind::BuiltIn => {
                let p = builtin_voice_model_path(&self.vc_dir, preset.builtin_model);
                if !p.is_file() {
                    return Err(format!(
                        "Voice model '{}' is not downloaded yet.",
                        preset.builtin_model.short_label()
                    ));
                }
                Ok((LoadedVoiceKey::BuiltIn(preset.builtin_model), p))
            }
            VoiceModelKind::CustomOnnx => {
                let trimmed = preset.custom_model_path.trim();
                if trimmed.is_empty() {
                    return Err("Select a custom .onnx voice model file.".to_string());
                }
                let p = PathBuf::from(trimmed);
                if !p.is_file() {
                    return Err(format!("Custom .onnx model not found: {}", p.display()));
                }
                Ok((LoadedVoiceKey::Custom(p.clone()), p))
            }
        }
    }

    pub fn needs_voice_load(&self, preset: &VoiceChangerPreset) -> bool {
        let Ok((desired, _)) = self.desired_voice_key(preset) else {
            return false;
        };
        self.voice.as_ref().is_none_or(|(k, _)| *k != desired)
    }

    pub fn ensure_voice_loaded(&mut self, preset: &VoiceChangerPreset) -> Result<u32, String> {
        let (desired_key, model_path) = self.desired_voice_key(preset)?;

        if let Some((loaded_key, synth)) = &self.voice {
            if *loaded_key == desired_key {
                return Ok(synth.sample_rate);
            }
        }

        let (synth, voice_ep) = VoiceSynthesizer::load(&model_path, self.ep_pref)?;
        self.active_ep_label = voice_ep;
        let sr = synth.sample_rate;
        self.voice = Some((desired_key, synth));
        Ok(sr)
    }

    /// Synthesizes target voice audio at generator sample rate (`40000` or `48000` Hz)
    /// from Stage 1's tail-sliced features.
    pub fn synthesize_tail(
        &mut self,
        feats: &ExtractedFeatures,
        preset: &VoiceChangerPreset,
        out_gen_audio: &mut Vec<f32>,
    ) -> Result<u32, String> {
        let gen_sr = self.ensure_voice_loaded(preset)?;
        let tail_cv = feats.tail_cv_frames;
        if tail_cv < 2 || feats.t_mel_padded == 0 {
            out_gen_audio.clear();
            return Ok(gen_sr);
        }

        let upsample_2x = self.voice.as_ref().is_none_or(|(_, s)| s.upsample_feats_2x);
        let target_frames = if upsample_2x { tail_cv * 2 } else { tail_cv };

        // 1. Exact nearest-neighbor 2x upsampling (`F.interpolate(scale_factor=2, mode='nearest')`)
        // matching RVC v2 `SynthesizerTrnMs768NSFsid` training so plosive consonants stay crisp.
        self.upsampled_feats.clear();
        if upsample_2x {
            self.upsampled_feats.reserve(target_frames * CONTENTVEC_DIM);
            for i in 0..tail_cv {
                let cur = &feats.cv_tail_feats[i * CONTENTVEC_DIM..(i + 1) * CONTENTVEC_DIM];
                self.upsampled_feats.extend_from_slice(cur);
                self.upsampled_feats.extend_from_slice(cur);
            }
        } else {
            self.upsampled_feats
                .extend_from_slice(&feats.cv_tail_feats[..tail_cv * CONTENTVEC_DIM]);
        }

        // 2. RMVPE neural F0 decoding (`100 Hz` hop = 160 samples @ 16 kHz, `t_padded = 32`)
        let mel_tensor = Tensor::from_array((
            [1usize, RMVPE_N_MEL, feats.t_mel_padded],
            feats.mel_buf.clone().into_boxed_slice(),
        ))
        .map_err(|e| format!("RMVPE mel tensor error: {e}"))?
        .into_dyn();

        // `protect` (`0.0..=0.5`) raises unvoiced sibilant/consonant protection threshold
        // (`0.03 .. 0.085`) so breathy `s`/`sh`/`t`/`k`/`h` stay unvoiced instead of whistling.
        let voicing_threshold = 0.03 + preset.protect.clamp(0.0, 0.5) * 0.11;
        {
            let rm_outputs = self
                .rmvpe_session
                .run(vec![("input", mel_tensor)])
                .map_err(|e| format!("RMVPE inference failed: {e}"))?;
            let (rm_shape, rm_slice) = rm_outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("RMVPE output extract error: {e}"))?;
            decode_rmvpe_salience(
                rm_shape,
                rm_slice,
                feats.n_mel_frames,
                voicing_threshold,
                &mut self.f0_100hz,
            );
        }

        // 3. Head-align F0 (`f0_100hz[0..target_frames]`) matching `center=True` STFT frame 0
        // with upsampled ContentVec frame 0 (avoids 10 ms off-by-one pitch lag from `2*N + 1`).
        self.f0_aligned.clear();
        if upsample_2x {
            let take = target_frames.min(self.f0_100hz.len());
            self.f0_aligned.extend_from_slice(&self.f0_100hz[..take]);
            if take < target_frames {
                self.f0_aligned.resize(target_frames, 0.0);
            }
        } else {
            for i in 0..target_frames {
                let src_idx = (i * 2).min(self.f0_100hz.len().saturating_sub(1));
                self.f0_aligned
                    .push(self.f0_100hz.get(src_idx).copied().unwrap_or(0.0));
            }
        }

        // 4. Apply pitch shift + RVC v2 mel-coarse quantization (`1..=255`)
        shift_and_quantize_f0(
            &mut self.f0_aligned,
            preset.pitch_shift_semitones,
            &mut self.coarse_pitch,
        );

        // Mild unvoiced consonant feature damping when `protect` is active so high-pitched
        // models don't inject metallic vocal-cord artifacts into sibilants/breaths.
        let protect_blend = preset.protect.clamp(0.0, 0.5) * 0.25;
        if protect_blend > 0.01 && target_frames >= 2 {
            for t in 1..target_frames {
                if self.f0_aligned[t] == 0.0 {
                    let prev_off = (t - 1) * CONTENTVEC_DIM;
                    let cur_off = t * CONTENTVEC_DIM;
                    for d in 0..CONTENTVEC_DIM {
                        let sm = self.upsampled_feats[prev_off + d] * protect_blend
                            + self.upsampled_feats[cur_off + d] * (1.0 - protect_blend);
                        self.upsampled_feats[cur_off + d] = sm;
                    }
                }
            }
        }

        // 5. Synthesize target voice waveform on the compact tail (`~18-26` frames)
        let (_, synth) = self
            .voice
            .as_mut()
            .ok_or_else(|| "Voice synthesizer not loaded".to_string())?;
        synth.run_generator(
            &self.upsampled_feats,
            &self.coarse_pitch,
            &self.f0_aligned,
            target_frames,
            &mut self.f16_buf,
            &mut self.rnd_buf,
            out_gen_audio,
        )?;

        Ok(gen_sr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_candidate_ep_ordering_includes_cpu_fallback() {
        for pref in [
            VcExecutionProvider::AutoGpu,
            VcExecutionProvider::WebGpuVulkan,
            VcExecutionProvider::RocmMiGraphX,
            VcExecutionProvider::Cuda,
            VcExecutionProvider::CpuOnly,
        ] {
            let eps = candidate_eps(pref);
            assert!(!eps.is_empty());
            assert_eq!(eps.last().copied(), Some(EpChoice::Cpu));
        }
    }

    #[test]
    fn test_ort_vulkan_and_vc_client_synthesizer_smoke() {
        let Ok(smoke_dir) = std::env::var("MICHADAME_VC_SMOKE_DIR") else {
            return;
        };
        let smoke_path = Path::new(&smoke_dir);
        let dylib_src = smoke_path.join("libonnxruntime.so.1.27.0");
        let model_path = smoke_path.join("tsukuyomi_simple.onnx");
        if !dylib_src.exists() || !model_path.exists() {
            return;
        }

        let temp_vc_dir = std::env::temp_dir().join(format!(
            "michadame-vc-ort-smoke-{}-{}",
            std::process::id(),
            crate::replay::now_us()
        ));
        let rt_dir = runtime_dir(&temp_vc_dir);
        std::fs::create_dir_all(&rt_dir).unwrap();
        std::fs::copy(&dylib_src, runtime_dylib_path(&temp_vc_dir)).unwrap();
        let shared_src = smoke_path.join("libonnxruntime_providers_shared.so");
        if shared_src.exists() {
            std::fs::copy(
                &shared_src,
                rt_dir.join("libonnxruntime_providers_shared.so"),
            )
            .unwrap();
        }

        let loaded_lib =
            ensure_ort_initialized(&temp_vc_dir, VcExecutionProvider::AutoGpu).unwrap();
        assert!(loaded_lib.exists());

        let (mut synth, ep_label) =
            VoiceSynthesizer::load(&model_path, VcExecutionProvider::AutoGpu).unwrap();
        assert!(!ep_label.is_empty());
        assert_eq!(synth.sample_rate, 40_000);
        assert!(synth.upsample_feats_2x);

        let t_frames = 22usize;
        let feats = vec![0.02f32; t_frames * CONTENTVEC_DIM];
        let mut pitchf = vec![220.0f32; t_frames];
        let mut pitch = Vec::new();
        shift_and_quantize_f0(&mut pitchf, 12.0, &mut pitch);

        let mut f16_buf = Vec::new();
        let mut rnd_buf = Vec::new();
        let mut audio_out = Vec::new();
        synth
            .run_generator(
                &feats,
                &pitch,
                &pitchf,
                t_frames,
                &mut f16_buf,
                &mut rnd_buf,
                &mut audio_out,
            )
            .unwrap();

        assert_eq!(audio_out.len(), t_frames * 400);
        assert!(audio_out.iter().all(|s| s.is_finite()));

        let _ = std::fs::remove_dir_all(&temp_vc_dir);
    }
}
