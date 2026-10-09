use anyhow::{Context, Result};
use ffmpeg_next as ff;
use ff::{format, frame};
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    s1: [f32; 2],
    s2: [f32; 2],
}

impl Biquad {
    pub fn notch(f0: f32, q: f32, fs: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * (f0 / fs).clamp(0.0001, 0.499);
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let cos_w0 = w0.cos();

        let b0 = 1.0;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            s1: [0.0; 2],
            s2: [0.0; 2],
        }
    }

    pub fn highpass(f0: f32, q: f32, fs: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * (f0 / fs).clamp(0.0001, 0.499);
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let cos_w0 = w0.cos();

        let b0 = (1.0 + cos_w0) * 0.5;
        let b1 = -(1.0 + cos_w0);
        let b2 = (1.0 + cos_w0) * 0.5;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            s1: [0.0; 2],
            s2: [0.0; 2],
        }
    }

    pub fn lowpass(f0: f32, q: f32, fs: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * (f0 / fs).clamp(0.0001, 0.499);
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let cos_w0 = w0.cos();

        let b0 = (1.0 - cos_w0) * 0.5;
        let b1 = 1.0 - cos_w0;
        let b2 = (1.0 - cos_w0) * 0.5;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            s1: [0.0; 2],
            s2: [0.0; 2],
        }
    }

    #[inline(always)]
    pub fn tick(&mut self, ch: usize, x: f32) -> f32 {
        let c = ch.min(1);
        let y = self.b0 * x + self.s1[c];
        self.s1[c] = self.b1 * x - self.a1 * y + self.s2[c];
        self.s2[c] = self.b2 * x - self.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        self.s1 = [0.0; 2];
        self.s2 = [0.0; 2];
    }
}

/// 4th-Order Linkwitz-Riley (LR4) Crossover Filter
///
/// Splits stereo audio into Low (< fc) and High (>= fc) bands.
/// Composed of two cascaded 2nd-order Butterworth filters (Q = 1/√2).
/// When summed, low + high produces an exact flat magnitude response (0.00 dB)
/// across all frequencies with zero ripple.
#[derive(Clone, Debug)]
pub struct Lr4Crossover {
    lp1: Biquad,
    lp2: Biquad,
    hp1: Biquad,
    hp2: Biquad,
}

impl Lr4Crossover {
    pub fn new(fc: f32, fs: f32) -> Self {
        let q = 1.0 / std::f32::consts::SQRT_2; // 0.70710677
        Self {
            lp1: Biquad::lowpass(fc, q, fs),
            lp2: Biquad::lowpass(fc, q, fs),
            hp1: Biquad::highpass(fc, q, fs),
            hp2: Biquad::highpass(fc, q, fs),
        }
    }

    #[inline(always)]
    pub fn split(&mut self, ch: usize, x: f32) -> (f32, f32) {
        let low = self.lp2.tick(ch, self.lp1.tick(ch, x));
        let high = self.hp2.tick(ch, self.hp1.tick(ch, x));
        (low, high)
    }

    pub fn reset(&mut self) {
        self.lp1.reset();
        self.lp2.reset();
        self.hp1.reset();
        self.hp2.reset();
    }
}

#[derive(Clone, Debug)]
pub struct DownwardExpander {
    threshold_linear: f32,
    ratio: f32,
    attack_coeff: f32,
    release_coeff: f32,
    env: [f32; 2],
}

impl DownwardExpander {
    pub fn new(threshold_db: f32, ratio: f32, attack_ms: f32, release_ms: f32, fs: f32) -> Self {
        let threshold_linear = 10.0f32.powf(threshold_db.clamp(-80.0, -10.0) / 20.0);
        let attack_sec = (attack_ms.max(0.5) * 0.001).max(1.0 / fs);
        let release_sec = (release_ms.max(1.0) * 0.001).max(1.0 / fs);

        let attack_coeff = (-1.0 / (attack_sec * fs)).exp();
        let release_coeff = (-1.0 / (release_sec * fs)).exp();

        Self {
            threshold_linear,
            ratio: ratio.clamp(1.5, 16.0),
            attack_coeff,
            release_coeff,
            env: [0.0; 2],
        }
    }

    #[inline(always)]
    pub fn tick(&mut self, ch: usize, x: f32) -> f32 {
        let c = ch.min(1);
        let level = x.abs();

        if level > self.env[c] {
            self.env[c] = self.attack_coeff * self.env[c] + (1.0 - self.attack_coeff) * level;
        } else {
            self.env[c] = self.release_coeff * self.env[c] + (1.0 - self.release_coeff) * level;
        }

        let env_safe = self.env[c].max(1e-6);
        let gain = if env_safe < self.threshold_linear {
            let diff_db = 20.0 * (env_safe / self.threshold_linear).log10();
            let gain_db = diff_db * (self.ratio - 1.0);
            10.0f32.powf((gain_db / 20.0).clamp(-60.0, 0.0))
        } else {
            1.0
        };

        x * gain
    }

    pub fn reset(&mut self) {
        self.env = [0.0; 2];
    }
}

pub struct RnnoiseStream {
    left: Box<nnnoiseless::DenoiseState<'static>>,
    right: Box<nnnoiseless::DenoiseState<'static>>,
    in_l: VecDeque<f32>,
    in_r: VecDeque<f32>,
    out_l: VecDeque<f32>,
    out_r: VecDeque<f32>,
}

impl RnnoiseStream {
    pub const FRAME_SIZE: usize = nnnoiseless::DenoiseState::FRAME_SIZE;
    pub const DELAY_SAMPLES: usize = Self::FRAME_SIZE * 2;

    pub fn new() -> Self {
        let mut stream = Self {
            left: nnnoiseless::DenoiseState::new(),
            right: nnnoiseless::DenoiseState::new(),
            in_l: VecDeque::with_capacity(Self::FRAME_SIZE * 4),
            in_r: VecDeque::with_capacity(Self::FRAME_SIZE * 4),
            out_l: VecDeque::with_capacity(Self::FRAME_SIZE * 4),
            out_r: VecDeque::with_capacity(Self::FRAME_SIZE * 4),
        };
        stream.prime();
        stream
    }

    fn prime(&mut self) {
        self.in_l.clear();
        self.in_r.clear();
        self.out_l.clear();
        self.out_r.clear();
        self.out_l.resize(Self::FRAME_SIZE, 0.0);
        self.out_r.resize(Self::FRAME_SIZE, 0.0);
    }

    pub fn reset(&mut self) {
        self.left = nnnoiseless::DenoiseState::new();
        self.right = nnnoiseless::DenoiseState::new();
        self.prime();
    }

    pub fn process_interleaved(&mut self, samples: &mut [f32], channels: usize) {
        if channels == 0 || samples.is_empty() {
            return;
        }

        if channels == 1 {
            for &s in samples.iter() {
                let scaled = (s * 32767.0).clamp(-32768.0, 32767.0);
                self.in_l.push_back(scaled);
            }
        } else {
            for frame in samples.chunks_exact(channels) {
                let s_l = (frame[0] * 32767.0).clamp(-32768.0, 32767.0);
                let s_r = (frame[1] * 32767.0).clamp(-32768.0, 32767.0);
                self.in_l.push_back(s_l);
                self.in_r.push_back(s_r);
            }
        }

        let mut block_in_l = [0.0f32; Self::FRAME_SIZE];
        let mut block_in_r = [0.0f32; Self::FRAME_SIZE];
        let mut block_out_l = [0.0f32; Self::FRAME_SIZE];
        let mut block_out_r = [0.0f32; Self::FRAME_SIZE];

        while self.in_l.len() >= Self::FRAME_SIZE {
            for i in 0..Self::FRAME_SIZE {
                block_in_l[i] = self.in_l.pop_front().unwrap_or(0.0);
                if channels > 1 {
                    block_in_r[i] = self.in_r.pop_front().unwrap_or(0.0);
                }
            }

            self.left.process_frame(&mut block_out_l, &block_in_l);
            for &val in &block_out_l {
                self.out_l.push_back(val);
            }

            if channels > 1 {
                self.right.process_frame(&mut block_out_r, &block_in_r);
                for &val in &block_out_r {
                    self.out_r.push_back(val);
                }
            }
        }

        if channels == 1 {
            for s in samples.iter_mut() {
                let val = self.out_l.pop_front().unwrap_or(0.0);
                *s = (val / 32767.0).clamp(-1.0, 1.0);
            }
        } else {
            for frame in samples.chunks_exact_mut(channels) {
                let l = self.out_l.pop_front().unwrap_or(0.0);
                let r = self.out_r.pop_front().unwrap_or(0.0);
                frame[0] = (l / 32767.0).clamp(-1.0, 1.0);
                frame[1] = (r / 32767.0).clamp(-1.0, 1.0);
            }
        }
    }
}

pub struct FfmpegAfftdnStream {
    graph: ff::filter::Graph,
    sample_rate: u32,
    pts: i64,
    frame_size: usize,
    in_l: VecDeque<f32>,
    in_r: VecDeque<f32>,
    out_l: VecDeque<f32>,
    out_r: VecDeque<f32>,
}

impl FfmpegAfftdnStream {
    pub fn frame_size_for(sample_rate: u32) -> usize {
        (sample_rate as usize * 125) / 10000 // exact 12.5 ms (600 at 48000 Hz)
    }

    pub fn delay_samples_for(sample_rate: u32) -> usize {
        // Algorithmic filter delay (2 * frame_size) + FIFO pre-buffer (1 * frame_size) = 3 * frame_size
        Self::frame_size_for(sample_rate) * 3
    }

    #[allow(dead_code)]
    pub fn delay_samples(&self) -> usize {
        Self::delay_samples_for(self.sample_rate)
    }

    pub fn new(
        sample_rate: u32,
        nr_db: f32,
        nf_db: f32,
        track_noise: bool,
        gain_smooth: u32,
    ) -> Result<Self> {
        let _ = ff::init();
        let mut graph = ff::filter::Graph::new();
        let abuffer = ff::filter::find("abuffer").context("abuffer filter not found")?;
        let abuffersink = ff::filter::find("abuffersink").context("abuffersink filter not found")?;

        let args = format!(
            "time_base=1/{}:sample_rate={}:sample_fmt=fltp:channel_layout=stereo",
            sample_rate, sample_rate
        );
        graph.add(&abuffer, "in", &args).context("Failed to add abuffer source")?;
        graph.add(&abuffersink, "out", "").context("Failed to add abuffersink sink")?;

        let nr = nr_db.clamp(1.0, 60.0);
        let nf = nf_db.clamp(-90.0, -20.0);
        let tn = if track_noise { 1 } else { 0 };
        let gs = gain_smooth.min(50);
        let spec = format!("afftdn=nr={:.1}:nf={:.1}:tn={}:tr=0:gs={}", nr, nf, tn, gs);

        graph
            .output("in", 0)
            .context("output in")?
            .input("out", 0)
            .context("input out")?
            .parse(&spec)
            .context("parse afftdn")?;

        graph.validate().context("validate graph")?;

        let frame_size = Self::frame_size_for(sample_rate);
        let mut stream = Self {
            graph,
            sample_rate,
            pts: 0,
            frame_size,
            in_l: VecDeque::with_capacity(frame_size * 4),
            in_r: VecDeque::with_capacity(frame_size * 4),
            out_l: VecDeque::with_capacity(frame_size * 4),
            out_r: VecDeque::with_capacity(frame_size * 4),
        };

        stream.prime();
        Ok(stream)
    }

    fn prime(&mut self) {
        self.in_l.clear();
        self.in_r.clear();
        self.out_l.clear();
        self.out_r.clear();
        // Pre-fill output FIFO with exactly frame_size zeros.
        // in_l/in_r start empty.
        // This ensures every sample experiences a fixed, deterministic delay of 3 * frame_size.
        self.out_l.resize(self.frame_size, 0.0);
        self.out_r.resize(self.frame_size, 0.0);
    }

    fn drain_into_buffers(&mut self) {
        if let Some(mut out_ctx) = self.graph.get("out") {
            let mut sink = out_ctx.sink();
            loop {
                let mut out_frame = frame::Audio::empty();
                match sink.frame(&mut out_frame) {
                    Ok(()) => {
                        let plane_l = out_frame.plane::<f32>(0);
                        let plane_r = if out_frame.channels() > 1 {
                            out_frame.plane::<f32>(1)
                        } else {
                            plane_l
                        };
                        self.out_l.extend(plane_l);
                        self.out_r.extend(plane_r);
                    }
                    Err(_) => break,
                }
            }
        }
    }

    pub fn process_interleaved(&mut self, samples: &mut [f32], channels: usize) {
        if channels == 0 || samples.is_empty() {
            return;
        }

        if channels == 1 {
            for &s in samples.iter() {
                self.in_l.push_back(s);
                self.in_r.push_back(s);
            }
        } else {
            for frame in samples.chunks_exact(channels) {
                self.in_l.push_back(frame[0]);
                self.in_r.push_back(frame[1]);
            }
        }

        while self.in_l.len() >= self.frame_size {
            let mut input_frame = frame::Audio::new(
                format::Sample::F32(format::sample::Type::Planar),
                self.frame_size,
                ff::ChannelLayout::STEREO,
            );
            input_frame.set_rate(self.sample_rate);
            input_frame.set_pts(Some(self.pts));
            self.pts += self.frame_size as i64;

            for i in 0..self.frame_size {
                let l = self.in_l.pop_front().unwrap_or(0.0);
                let r = self.in_r.pop_front().unwrap_or(0.0);
                input_frame.plane_mut::<f32>(0)[i] = l;
                input_frame.plane_mut::<f32>(1)[i] = r;
            }

            if let Some(mut in_ctx) = self.graph.get("in") {
                if in_ctx.source().add(&input_frame).is_ok() {
                    self.drain_into_buffers();
                }
            }
        }

        if channels == 1 {
            for s in samples.iter_mut() {
                let val = self.out_l.pop_front().unwrap_or(0.0);
                *s = val.clamp(-1.0, 1.0);
            }
        } else {
            for frame in samples.chunks_exact_mut(channels) {
                let l = self.out_l.pop_front().unwrap_or(0.0);
                let r = self.out_r.pop_front().unwrap_or(0.0);
                frame[0] = l.clamp(-1.0, 1.0);
                frame[1] = r.clamp(-1.0, 1.0);
            }
        }
    }
}

pub struct LiveAudioDsp {
    pub enabled: bool,
    rumble_filter: Option<Biquad>,
    hum_notches: Vec<Biquad>,
    crossover: Option<Lr4Crossover>,
    high_delay_l: VecDeque<f32>,
    high_delay_r: VecDeque<f32>,
    crt_notch: Option<Biquad>,   // 15.734 kHz flyback whine
    rnnoise: Option<RnnoiseStream>,
    afftdn: Option<FfmpegAfftdnStream>,
    expander: Option<DownwardExpander>,
    sample_rate: u32,
    current_config: crate::replay::config::AudioFilterConfig,
}

impl LiveAudioDsp {
    pub fn new(config: &crate::replay::config::AudioFilterConfig, sample_rate: u32) -> Self {
        let mut dsp = Self {
            enabled: false,
            rumble_filter: None,
            hum_notches: Vec::new(),
            crossover: None,
            high_delay_l: VecDeque::new(),
            high_delay_r: VecDeque::new(),
            crt_notch: None,
            rnnoise: None,
            afftdn: None,
            expander: None,
            sample_rate: 0,
            current_config: crate::replay::config::AudioFilterConfig::default(),
        };
        dsp.reconfigure(config, sample_rate);
        dsp
    }

    pub fn reconfigure(&mut self, config: &crate::replay::config::AudioFilterConfig, sample_rate: u32) {
        if self.enabled == config.enabled
            && self.sample_rate == sample_rate
            && &self.current_config == config
        {
            return;
        }

        self.enabled = config.enabled;
        self.sample_rate = sample_rate;
        self.current_config = config.clone();

        if !self.enabled {
            self.rumble_filter = None;
            self.hum_notches.clear();
            self.crossover = None;
            self.high_delay_l.clear();
            self.high_delay_r.clear();
            self.crt_notch = None;
            self.rnnoise = None;
            self.afftdn = None;
            self.expander = None;
            return;
        }

        let fs = sample_rate as f32;

        // 40 Hz Subsonic Rumble Filter
        self.rumble_filter = if config.rumble_filter {
            Some(Biquad::highpass(40.0, 0.707, fs))
        } else {
            None
        };

        // Hum & Video Ground Return Buzz Notches (high-Q biquad notches, Q=35)
        self.hum_notches = match config.hum_filter {
            crate::replay::config::HumFilterMode::SingleNotch60Hz => {
                vec![Biquad::notch(60.0, 35.0, fs)]
            }
            crate::replay::config::HumFilterMode::SingleNotch50Hz => {
                vec![Biquad::notch(50.0, 35.0, fs)]
            }
            crate::replay::config::HumFilterMode::HarmonicNotch60Hz => {
                // Cascaded narrow constant-bandwidth (3 Hz) notches at Dreamcast NTSC 59.94 Hz harmonics up to 1500 Hz (24 harmonics)
                let f0 = 59.938;
                (1..=24)
                    .map(|k| {
                        let fk = f0 * (k as f32);
                        let q = (fk / 3.0).max(15.0);
                        Biquad::notch(fk, q, fs)
                    })
                    .collect()
            }
            crate::replay::config::HumFilterMode::HarmonicNotch50Hz => {
                // Cascaded narrow constant-bandwidth (3 Hz) notches at PAL 50.00 Hz harmonics up to 1500 Hz (30 harmonics)
                let f0 = 50.0;
                (1..=30)
                    .map(|k| {
                        let fk = f0 * (k as f32);
                        let q = (fk / 3.0).max(15.0);
                        Biquad::notch(fk, q, fs)
                    })
                    .collect()
            }
            crate::replay::config::HumFilterMode::Custom => {
                let freq = config.hum_freq.clamp(20.0, 1000.0);
                vec![Biquad::notch(freq, 35.0, fs)]
            }
            crate::replay::config::HumFilterMode::Disabled => Vec::new(),
        };

        // Crossover for Treble Bypass / High-Frequency Retention
        let has_denoiser = config.denoise_backend != crate::replay::config::DenoiseBackend::Disabled;
        if config.treble_bypass && has_denoiser {
            let fc = config.treble_crossover_hz.clamp(1000.0, 16000.0);
            self.crossover = Some(Lr4Crossover::new(fc, fs));
            let delay_frames = match config.denoise_backend {
                crate::replay::config::DenoiseBackend::Rnnoise => RnnoiseStream::DELAY_SAMPLES,
                crate::replay::config::DenoiseBackend::Afftdn => FfmpegAfftdnStream::delay_samples_for(sample_rate),
                crate::replay::config::DenoiseBackend::Disabled => 0,
            };
            self.high_delay_l.clear();
            self.high_delay_r.clear();
            self.high_delay_l.resize(delay_frames, 0.0);
            self.high_delay_r.resize(delay_frames, 0.0);
        } else {
            self.crossover = None;
            self.high_delay_l.clear();
            self.high_delay_r.clear();
        }

        // 15.734 kHz CRT line whine notch
        self.crt_notch = if config.crt_notch {
            let freq = config.crt_freq.clamp(10000.0, fs * 0.49);
            Some(Biquad::notch(freq, 50.0, fs))
        } else {
            None
        };

        // Realtime Denoising Library
        match config.denoise_backend {
            crate::replay::config::DenoiseBackend::Afftdn => {
                self.rnnoise = None;
                self.afftdn = FfmpegAfftdnStream::new(
                    sample_rate,
                    config.denoise_reduction_db,
                    config.denoise_noise_floor_db,
                    config.denoise_track_noise,
                    config.denoise_gain_smooth,
                )
                .map_err(|e| tracing::warn!("Failed to init afftdn: {}", e))
                .ok();
            }
            crate::replay::config::DenoiseBackend::Rnnoise => {
                self.afftdn = None;
                if self.rnnoise.is_none() {
                    self.rnnoise = Some(RnnoiseStream::new());
                }
            }
            crate::replay::config::DenoiseBackend::Disabled => {
                self.afftdn = None;
                self.rnnoise = None;
            }
        }

        // Noise gate / downward expander
        self.expander = if config.gate {
            Some(DownwardExpander::new(
                config.gate_threshold_db,
                6.0,
                5.0,
                80.0,
                fs,
            ))
        } else {
            None
        };
    }

    #[inline]
    pub fn process_interleaved(&mut self, samples: &mut [f32], channels: usize) {
        if !self.enabled || channels == 0 || samples.is_empty() {
            return;
        }

        let num_ch = channels.min(2);

        // Stage 1: Rumble HPF and Hum / Buzz Notches (both operate in low frequencies)
        for frame in samples.chunks_exact_mut(channels) {
            for (ch, sample) in frame[..num_ch].iter_mut().enumerate() {
                let mut x = *sample;

                if let Some(rf) = &mut self.rumble_filter {
                    x = rf.tick(ch, x);
                }

                for hn in &mut self.hum_notches {
                    x = hn.tick(ch, x);
                }

                *sample = x;
            }
        }

        // Stage 2: Crossover Split (if Treble Bypass is enabled)
        if let Some(xo) = &mut self.crossover {
            for frame in samples.chunks_exact_mut(channels) {
                let (low_l, high_l) = xo.split(0, frame[0]);
                frame[0] = low_l;
                self.high_delay_l.push_back(high_l);

                if num_ch > 1 {
                    let (low_r, high_r) = xo.split(1, frame[1]);
                    frame[1] = low_r;
                    self.high_delay_r.push_back(high_r);
                }
            }
        }

        // Stage 3: Realtime Denoising Library (Afftdn or Rnnoise)
        if let Some(afftdn) = &mut self.afftdn {
            afftdn.process_interleaved(samples, channels);
        } else if let Some(rnnoise) = &mut self.rnnoise {
            rnnoise.process_interleaved(samples, channels);
        }

        // Stage 4: Recombine High Frequencies (if Treble Bypass was active)
        if self.crossover.is_some() {
            for frame in samples.chunks_exact_mut(channels) {
                let high_l = self.high_delay_l.pop_front().unwrap_or(0.0);
                frame[0] += high_l;

                if num_ch > 1 {
                    let high_r = self.high_delay_r.pop_front().unwrap_or(0.0);
                    frame[1] += high_r;
                }
            }
        }

        // Stage 5: 15.7 kHz CRT Flyback Notch and Noise Gate Expander
        for frame in samples.chunks_exact_mut(channels) {
            for (ch, sample) in frame[..num_ch].iter_mut().enumerate() {
                let mut x = *sample;

                if let Some(cn) = &mut self.crt_notch {
                    x = cn.tick(ch, x);
                }

                if let Some(exp) = &mut self.expander {
                    x = exp.tick(ch, x);
                }

                *sample = x;
            }
        }
    }

    pub fn reset(&mut self) {
        if let Some(rf) = &mut self.rumble_filter {
            rf.reset();
        }
        for hn in &mut self.hum_notches {
            hn.reset();
        }
        if let Some(xo) = &mut self.crossover {
            xo.reset();
            let delay_frames = match self.current_config.denoise_backend {
                crate::replay::config::DenoiseBackend::Rnnoise => RnnoiseStream::DELAY_SAMPLES,
                crate::replay::config::DenoiseBackend::Afftdn => FfmpegAfftdnStream::delay_samples_for(self.sample_rate),
                crate::replay::config::DenoiseBackend::Disabled => 0,
            };
            self.high_delay_l.clear();
            self.high_delay_r.clear();
            self.high_delay_l.resize(delay_frames, 0.0);
            self.high_delay_r.resize(delay_frames, 0.0);
        }
        if let Some(cn) = &mut self.crt_notch {
            cn.reset();
        }
        if let Some(rnnoise) = &mut self.rnnoise {
            rnnoise.reset();
        }
        if let Some(exp) = &mut self.expander {
            exp.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::config::{AudioFilterConfig, DenoiseBackend};

    #[test]
    fn test_single_notch_attenuation_and_transparency() {
        let fs = 48000.0;
        let mut notch = Biquad::notch(60.0, 30.0, fs);

        // 1. At 60 Hz: strong notch attenuation (> 25 dB)
        let mut sum_sq_in = 0.0;
        let mut sum_sq_out = 0.0;
        for i in 0..96000 {
            let t = i as f32 / fs;
            let x = (2.0 * std::f32::consts::PI * 60.0 * t).sin();
            let y = notch.tick(0, x);
            if i >= 48000 {
                sum_sq_in += x * x;
                sum_sq_out += y * y;
            }
        }
        let atten_db = 10.0 * (sum_sq_in / sum_sq_out).log10();
        assert!(atten_db > 25.0, "60Hz notch atten was {:.1} dB", atten_db);

        // 2. At 80 Hz: exact transparency (attenuation < 0.1 dB, zero phaser distortion)
        notch.reset();
        let mut sum_sq_in_80 = 0.0;
        let mut sum_sq_out_80 = 0.0;
        for i in 0..96000 {
            let t = i as f32 / fs;
            let x = (2.0 * std::f32::consts::PI * 80.0 * t).sin();
            let y = notch.tick(0, x);
            if i >= 48000 {
                sum_sq_in_80 += x * x;
                sum_sq_out_80 += y * y;
            }
        }
        let delta_db = 10.0 * (sum_sq_in_80 / sum_sq_out_80).log10().abs();
        assert!(delta_db < 0.1, "80Hz was affected by {:.2} dB", delta_db);
    }

    #[test]
    fn test_lr4_crossover_flat_summing() {
        let fs = 48000.0;
        let mut xo = Lr4Crossover::new(4000.0, fs);

        // Test summing across multiple test frequencies (500 Hz, 2 kHz, 4 kHz, 8 kHz, 12 kHz)
        for &freq in &[500.0, 2000.0, 4000.0, 8000.0, 12000.0] {
            xo.reset();
            let mut sum_sq_in = 0.0;
            let mut sum_sq_out = 0.0;
            for i in 0..48000 {
                let t = i as f32 / fs;
                let x = (2.0 * std::f32::consts::PI * freq * t).sin();
                let (low, high) = xo.split(0, x);
                let y = low + high;
                if i >= 4800 {
                    sum_sq_in += x * x;
                    sum_sq_out += y * y;
                }
            }
            let delta_db = 10.0 * (sum_sq_in / sum_sq_out).log10().abs();
            assert!(
                delta_db < 0.05,
                "LR4 crossover at {:.0} Hz had delta {:.3} dB (expected < 0.05 dB)",
                freq, delta_db
            );
        }
    }

    #[test]
    fn test_rnnoise_streaming_exact_frame_sizes() {
        let mut stream = RnnoiseStream::new();
        for sz in [1024, 512, 128, 480, 64] {
            let mut buf = vec![0.1f32; sz * 2];
            stream.process_interleaved(&mut buf, 2);
            assert_eq!(buf.len(), sz * 2);
        }
    }

    #[test]
    fn test_live_audio_dsp_rnnoise_pipeline() {
        let mut cfg = AudioFilterConfig::default();
        cfg.enabled = true;
        cfg.denoise_backend = DenoiseBackend::Rnnoise;
        cfg.treble_bypass = true;
        let mut dsp = LiveAudioDsp::new(&cfg, 48000);
        let mut samples = vec![0.1f32; 1024 * 2];
        dsp.process_interleaved(&mut samples, 2);
        assert_eq!(samples.len(), 1024 * 2);
    }

    #[test]
    fn test_live_dsp_crossover_variable_chunks() {
        for backend in [DenoiseBackend::Afftdn, DenoiseBackend::Rnnoise] {
            let mut cfg = AudioFilterConfig::default();
            cfg.enabled = true;
            cfg.denoise_backend = backend;
            cfg.treble_bypass = true;
            cfg.treble_crossover_hz = 4000.0;
            let mut dsp = LiveAudioDsp::new(&cfg, 48000);

            // Feed arbitrary variable chunk sizes
            let chunk_sizes = [256, 128, 512, 384, 1024, 64, 480, 256, 512];
            for &sz in &chunk_sizes {
                let mut buf = vec![0.1f32; sz * 2];
                dsp.process_interleaved(&mut buf, 2);
                assert_eq!(buf.len(), sz * 2);
                // Verify no NaNs or Infinities
                for &s in &buf {
                    assert!(s.is_finite());
                }
            }
        }
    }

    #[test]
    fn test_harmonic_notch_attenuation_and_transparency() {
        let fs = 48000.0;
        let f0 = 59.938;
        let mut notches: Vec<Biquad> = (1..=24)
            .map(|k| {
                let fk = f0 * (k as f32);
                let q = (fk / 3.0).max(15.0);
                Biquad::notch(fk, q, fs)
            })
            .collect();

        // 1. Check attenuation at harmonics (e.g. 59.94, 179.8, 359.6, 719.3, 1198.8 Hz)
        for &k in &[1, 3, 6, 12, 20] {
            let fk = f0 * (k as f32);
            for n in &mut notches {
                n.reset();
            }
            let mut sum_sq_in = 0.0;
            let mut sum_sq_out = 0.0;
            for i in 0..96000 {
                let t = i as f32 / fs;
                let x = (2.0 * std::f32::consts::PI * fk * t).sin();
                let mut y = x;
                for n in &mut notches {
                    y = n.tick(0, y);
                }
                if i >= 48000 {
                    sum_sq_in += x * x;
                    sum_sq_out += y * y;
                }
            }
            let atten_db = 10.0 * (sum_sq_in / sum_sq_out).log10();
            assert!(
                atten_db > 15.0,
                "Harmonic notch at {:.1} Hz attenuation was {:.1} dB (expected > 15 dB)",
                fk,
                atten_db
            );
        }

        // 2. Check transparency at off-notch frequencies (90 Hz, 150 Hz, 710.5 Hz, 1000 Hz)
        for &f_pass in &[90.0, 150.0, 710.5, 1000.0] {
            for n in &mut notches {
                n.reset();
            }
            let mut sum_sq_in = 0.0;
            let mut sum_sq_out = 0.0;
            for i in 0..96000 {
                let t = i as f32 / fs;
                let x = (2.0 * std::f32::consts::PI * f_pass * t).sin();
                let mut y = x;
                for n in &mut notches {
                    y = n.tick(0, y);
                }
                if i >= 48000 {
                    sum_sq_in += x * x;
                    sum_sq_out += y * y;
                }
            }
            let delta_db = 10.0 * (sum_sq_in / sum_sq_out).log10().abs();
            assert!(
                delta_db < 0.25,
                "Harmonic notch affected passband {:.1} Hz by {:.3} dB (expected < 0.25 dB)",
                f_pass,
                delta_db
            );
        }
    }

    #[test]
    fn test_afftdn_exact_constant_delay_and_streaming() {
        for spike_at in [0, 50, 255, 256, 300, 599, 600, 750, 1000] {
            let mut stream = FfmpegAfftdnStream::new(48000, 12.0, -50.0, false, 0).unwrap();
            let chunk_sizes = [256, 256, 128, 384, 512, 1024, 256, 512, 128, 64, 480, 512];
            let mut total_fed = 0;
            let mut detected_at = None;
            let mut total_out = 0;

            for &sz in &chunk_sizes {
                let mut buf = vec![0.0f32; sz * 2];
                for i in 0..sz {
                    if total_fed + i == spike_at {
                        buf[i * 2] = 1.0;
                        buf[i * 2 + 1] = 1.0;
                    }
                }
                total_fed += sz;
                stream.process_interleaved(&mut buf, 2);
                for i in 0..sz {
                    let val = buf[i * 2];
                    if val.abs() > 0.05 && detected_at.is_none() {
                        detected_at = Some(total_out + i);
                    }
                }
                total_out += sz;
            }

            let delay = detected_at.unwrap() as i64 - spike_at as i64;
            assert_eq!(
                delay,
                stream.delay_samples() as i64,
                "Afftdn spike at {} had delay {} (expected {})",
                spike_at,
                delay,
                stream.delay_samples()
            );
        }
    }

    #[test]
    fn test_rnnoise_exact_constant_delay_and_streaming() {
        for spike_at in [0, 50, 255, 256, 300, 479, 480, 750, 1000] {
            let mut stream = RnnoiseStream::new();
            let chunk_sizes = [256, 256, 128, 384, 512, 1024, 256, 512, 128, 64, 480, 512];
            let mut total_fed = 0;
            let mut max_val = 0.0f32;
            let mut peak_at = 0;
            let mut total_out = 0;

            for &sz in &chunk_sizes {
                let mut buf = vec![0.0f32; sz * 2];
                for i in 0..sz {
                    if total_fed + i == spike_at {
                        buf[i * 2] = 0.5;
                        buf[i * 2 + 1] = 0.5;
                    }
                }
                total_fed += sz;
                stream.process_interleaved(&mut buf, 2);
                for i in 0..sz {
                    let val = buf[i * 2].abs();
                    if val > max_val {
                        max_val = val;
                        peak_at = total_out + i;
                    }
                }
                total_out += sz;
            }

            let delay = peak_at as i64 - spike_at as i64;
            assert_eq!(
                delay,
                RnnoiseStream::DELAY_SAMPLES as i64,
                "RNNoise spike at {} had delay {} (expected {})",
                spike_at,
                delay,
                RnnoiseStream::DELAY_SAMPLES
            );
        }
    }

    fn write_wav_i16(path: &str, samples: &[f32], channels: usize, sample_rate: u32) {
        use std::io::Write;
        let mut f = std::fs::File::create(path).unwrap();
        let num_samples = samples.len();
        let data_bytes = (num_samples * 2) as u32;
        let file_size = 36 + data_bytes;
        f.write_all(b"RIFF").unwrap();
        f.write_all(&file_size.to_le_bytes()).unwrap();
        f.write_all(b"WAVEfmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&(channels as u16).to_le_bytes()).unwrap();
        f.write_all(&sample_rate.to_le_bytes()).unwrap();
        f.write_all(&(sample_rate * channels as u32 * 2).to_le_bytes()).unwrap();
        f.write_all(&(channels as u16 * 2).to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&data_bytes.to_le_bytes()).unwrap();
        for &s in samples {
            let i = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            f.write_all(&i.to_le_bytes()).unwrap();
        }
    }

    #[test]
    fn test_process_test_noise_mp3() {
        use crate::replay::config::FilterPreset;
        let input_path = std::path::Path::new("/tmp/test-noise.mp3");
        if !input_path.exists() {
            return;
        }
        let mut input = ff::format::input(&input_path).unwrap();
        let stream = input.streams().best(ff::media::Type::Audio).unwrap();
        let index = stream.index();
        let mut decoder = ff::codec::context::Context::from_parameters(stream.parameters())
            .unwrap()
            .decoder()
            .audio()
            .unwrap();

        let mut interleaved_samples = Vec::<f32>::new();
        let mut frame = ff::frame::Audio::empty();
        for (s, packet) in input.packets() {
            if s.index() != index {
                continue;
            }
            let _ = decoder.send_packet(&packet);
            while decoder.receive_frame(&mut frame).is_ok() {
                let channels = frame.channels() as usize;
                let num_frames = frame.samples();
                if channels == 2 {
                    if frame.is_planar() {
                        let l = frame.plane::<f32>(0);
                        let r = frame.plane::<f32>(1);
                        for i in 0..num_frames {
                            interleaved_samples.push(l[i]);
                            interleaved_samples.push(r[i]);
                        }
                    } else {
                        interleaved_samples.extend_from_slice(frame.plane::<f32>(0));
                    }
                }
            }
        }
        let _ = decoder.send_eof();
        while decoder.receive_frame(&mut frame).is_ok() {
            let channels = frame.channels() as usize;
            let num_frames = frame.samples();
            if channels == 2 {
                if frame.is_planar() {
                    let l = frame.plane::<f32>(0);
                    let r = frame.plane::<f32>(1);
                    for i in 0..num_frames {
                        interleaved_samples.push(l[i]);
                        interleaved_samples.push(r[i]);
                    }
                } else {
                    interleaved_samples.extend_from_slice(frame.plane::<f32>(0));
                }
            }
        }

        assert!(!interleaved_samples.is_empty());

        // 1. Process with RNNoise + Treble Bypass (4 kHz Crossover)
        let mut cfg_rnnoise = AudioFilterConfig::default();
        cfg_rnnoise.enabled = true;
        cfg_rnnoise.apply_preset(FilterPreset::DreamcastNtsc);
        cfg_rnnoise.denoise_backend = DenoiseBackend::Rnnoise;
        cfg_rnnoise.treble_bypass = true;
        cfg_rnnoise.treble_crossover_hz = 4000.0;
        let mut dsp_rnnoise = LiveAudioDsp::new(&cfg_rnnoise, 48000);

        let mut out_rnnoise = interleaved_samples.clone();
        for chunk in out_rnnoise.chunks_mut(1024 * 2) {
            dsp_rnnoise.process_interleaved(chunk, 2);
        }
        write_wav_i16("/tmp/test-noise-rnnoise-treble-preserved.wav", &out_rnnoise, 2, 48000);

        // 2. Process with FFmpeg afftdn + Treble Bypass (4 kHz Crossover)
        let mut cfg_afftdn = AudioFilterConfig::default();
        cfg_afftdn.enabled = true;
        cfg_afftdn.apply_preset(FilterPreset::DreamcastNtsc);
        cfg_afftdn.denoise_backend = DenoiseBackend::Afftdn;
        cfg_afftdn.treble_bypass = true;
        cfg_afftdn.treble_crossover_hz = 4000.0;
        let mut dsp_afftdn = LiveAudioDsp::new(&cfg_afftdn, 48000);

        let mut out_afftdn = interleaved_samples.clone();
        for chunk in out_afftdn.chunks_mut(1024 * 2) {
            dsp_afftdn.process_interleaved(chunk, 2);
        }
        write_wav_i16("/tmp/test-noise-afftdn-treble-preserved.wav", &out_afftdn, 2, 48000);
    }
}
