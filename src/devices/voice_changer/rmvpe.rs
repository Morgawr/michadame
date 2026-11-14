use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;

pub const RMVPE_SR: usize = 16_000;
pub const RMVPE_N_FFT: usize = 1024;
pub const RMVPE_WIN: usize = 1024;
pub const RMVPE_HOP: usize = 160; // 10 ms @ 16 kHz (100 fps)
pub const RMVPE_N_MEL: usize = 128;
pub const RMVPE_N_BINS: usize = RMVPE_N_FFT / 2 + 1; // 513
pub const RMVPE_PITCH_BINS: usize = 360;
const RMVPE_FMIN: f32 = 30.0;
const RMVPE_FMAX: f32 = 8000.0;
const RMVPE_BASE_CENTS: f32 = 1997.379_4; // 10 * 2^(1997.3794/1200) ~= 31.708 Hz (official RMVPE C1-offset)

/// Precomputed STFT + Librosa HTK/Slaney 128-bin log-mel extractor and 360-bin cent decoder
/// matching official `RMVPE` (`lj1995/VoiceConversionWebUI` / `w-okada/voice-changer`).
pub struct RmvpeFrontend {
    fft: Arc<dyn Fft<f32>>,
    hann_window: Vec<f32>,
    /// Row-major `[RMVPE_N_MEL * RMVPE_N_BINS]` Slaney-normalized HTK mel basis
    mel_basis: Vec<f32>,
    padded_audio: Vec<f32>,
    fft_buf: Vec<Complex<f32>>,
    fft_scratch: Vec<Complex<f32>>,
    mag_frame: Vec<f32>,
}

impl Default for RmvpeFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl RmvpeFrontend {
    pub fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(RMVPE_N_FFT);
        let scratch_len = fft.get_inplace_scratch_len();

        // Periodic Hann window matching `torch.hann_window(1024, periodic=True)`
        let hann_window: Vec<f32> = (0..RMVPE_WIN)
            .map(|n| 0.5 - 0.5 * (2.0 * PI * n as f32 / RMVPE_WIN as f32).cos())
            .collect();

        let mel_basis = build_htk_slaney_mel_basis(
            RMVPE_SR as f32,
            RMVPE_N_FFT,
            RMVPE_N_MEL,
            RMVPE_FMIN,
            RMVPE_FMAX,
        );

        Self {
            fft,
            hann_window,
            mel_basis,
            padded_audio: Vec::with_capacity(16_000),
            fft_buf: vec![Complex::new(0.0, 0.0); RMVPE_N_FFT],
            fft_scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            mag_frame: vec![0.0; RMVPE_N_BINS],
        }
    }

    /// Computes `[1, 128, t_padded]` log-mel spectrogram (reflect-padded in time to a multiple of 32).
    /// Returns `(n_frames_unpadded, t_padded)` and fills `mel_out` in row-major `[128, t_padded]`.
    pub fn compute_log_mel(&mut self, audio_16k: &[f32], mel_out: &mut Vec<f32>) -> (usize, usize) {
        let pad = RMVPE_N_FFT / 2; // 512 (`torch.stft(..., center=True)`)
        reflect_pad_1d(audio_16k, pad, pad, &mut self.padded_audio);

        let n_frames = if self.padded_audio.len() >= RMVPE_WIN {
            1 + (self.padded_audio.len() - RMVPE_WIN) / RMVPE_HOP
        } else {
            1
        };
        let t_padded = n_frames.div_ceil(32).max(1) * 32;

        mel_out.clear();
        mel_out.resize(RMVPE_N_MEL * t_padded, (-11.512_925f32).max(1e-5f32.ln()));

        for t in 0..n_frames {
            let start = t * RMVPE_HOP;
            for i in 0..RMVPE_WIN {
                let s = self.padded_audio.get(start + i).copied().unwrap_or(0.0);
                self.fft_buf[i] = Complex::new(s * self.hann_window[i], 0.0);
            }
            self.fft
                .process_with_scratch(&mut self.fft_buf, &mut self.fft_scratch);

            for k in 0..RMVPE_N_BINS {
                let c = self.fft_buf[k];
                self.mag_frame[k] = (c.re * c.re + c.im * c.im).sqrt();
            }

            for m in 0..RMVPE_N_MEL {
                let row = &self.mel_basis[m * RMVPE_N_BINS..(m + 1) * RMVPE_N_BINS];
                let mut acc = 0.0f32;
                for k in 0..RMVPE_N_BINS {
                    acc += row[k] * self.mag_frame[k];
                }
                mel_out[m * t_padded + t] = acc.max(1e-5).ln();
            }
        }

        // Reflect-pad along time axis from `n_frames` to `t_padded`
        if t_padded > n_frames && n_frames > 0 {
            for m in 0..RMVPE_N_MEL {
                let row_off = m * t_padded;
                for extra in n_frames..t_padded {
                    let src_t = reflect_index(extra as isize, n_frames);
                    mel_out[row_off + extra] = mel_out[row_off + src_t];
                }
            }
        }

        (n_frames, t_padded)
    }
}

/// Decodes RMVPE output salience tensor into per-frame F0 in Hz (`0.0` for unvoiced).
/// Supports both `[1, T_pad, 360]` and `[1, 360, T_pad]` layouts automatically.
pub fn decode_rmvpe_salience(
    shape: &[i64],
    data: &[f32],
    n_frames: usize,
    threshold: f32,
    f0_out: &mut Vec<f32>,
) {
    f0_out.clear();
    if n_frames == 0 || data.is_empty() {
        return;
    }

    // Determine layout: `[1, T_pad, 360]` (standard) vs `[1, 360, T_pad]`
    let (time_major, t_pad, n_bins) = if shape.len() == 3 && shape[2] as usize == RMVPE_PITCH_BINS {
        (true, shape[1].max(1) as usize, RMVPE_PITCH_BINS)
    } else if shape.len() == 3 && shape[1] as usize == RMVPE_PITCH_BINS {
        (false, shape[2].max(1) as usize, RMVPE_PITCH_BINS)
    } else {
        let inferred_t = data.len() / RMVPE_PITCH_BINS;
        (true, inferred_t.max(1), RMVPE_PITCH_BINS)
    };

    let frames = n_frames.min(t_pad);
    f0_out.reserve(n_frames);

    for t in 0..frames {
        let mut center = 0usize;
        let mut max_val = f32::NEG_INFINITY;

        if time_major {
            let row = &data[t * n_bins..(t + 1) * n_bins];
            for (b, &v) in row.iter().enumerate() {
                if v > max_val {
                    max_val = v;
                    center = b;
                }
            }
            if !max_val.is_finite() || max_val <= threshold {
                f0_out.push(0.0);
                continue;
            }
            let lo = center.saturating_sub(4);
            let hi = (center + 4).min(n_bins - 1);
            let mut prod_sum = 0.0f32;
            let mut weight_sum = 0.0f32;
            for (b, &v) in row.iter().enumerate().take(hi + 1).skip(lo) {
                let cents = 20.0 * b as f32 + RMVPE_BASE_CENTS;
                prod_sum += v * cents;
                weight_sum += v;
            }
            if weight_sum > 1e-8 {
                let cents = prod_sum / weight_sum;
                f0_out.push(10.0 * 2.0f32.powf(cents / 1200.0));
            } else {
                f0_out.push(0.0);
            }
        } else {
            for b in 0..n_bins {
                let v = data[b * t_pad + t];
                if v > max_val {
                    max_val = v;
                    center = b;
                }
            }
            if !max_val.is_finite() || max_val <= threshold {
                f0_out.push(0.0);
                continue;
            }
            let lo = center.saturating_sub(4);
            let hi = (center + 4).min(n_bins - 1);
            let mut prod_sum = 0.0f32;
            let mut weight_sum = 0.0f32;
            for b in lo..=hi {
                let v = data[b * t_pad + t];
                let cents = 20.0 * b as f32 + RMVPE_BASE_CENTS;
                prod_sum += v * cents;
                weight_sum += v;
            }
            if weight_sum > 1e-8 {
                let cents = prod_sum / weight_sum;
                f0_out.push(10.0 * 2.0f32.powf(cents / 1200.0));
            } else {
                f0_out.push(0.0);
            }
        }
    }

    while f0_out.len() < n_frames {
        f0_out.push(0.0);
    }

    // Bridge isolated 1-frame pitch dropouts inside strongly voiced vowels without
    // polluting true unvoiced consonants (`s`, `sh`, `t`, `k`, `h`) with vocal buzz.
    if f0_out.len() >= 3 {
        for i in 1..f0_out.len() - 1 {
            if f0_out[i] == 0.0 && f0_out[i - 1] > 0.0 && f0_out[i + 1] > 0.0 {
                let ratio = (f0_out[i + 1] / f0_out[i - 1]).max(f0_out[i - 1] / f0_out[i + 1]);
                if ratio < 1.25 {
                    f0_out[i] = 0.5 * (f0_out[i - 1] + f0_out[i + 1]);
                }
            }
        }
    }
}

/// Applies pitch shift (`semitones`) to voiced frames and computes RVC v2 mel-coarse
/// pitch indices in `1..=255` (`1` = unvoiced, matching RVC WebUI & w-okada `emb_pitch`).
pub fn shift_and_quantize_f0(
    f0_hz: &mut [f32],
    pitch_shift_semitones: f32,
    coarse_out: &mut Vec<i64>,
) {
    const F0_MIN: f32 = 50.0;
    const F0_MAX: f32 = 1100.0;
    let f0_mel_min = 1127.0 * (1.0 + F0_MIN / 700.0).ln();
    let f0_mel_max = 1127.0 * (1.0 + F0_MAX / 700.0).ln();
    let pitch_mul = 2.0f32.powf(pitch_shift_semitones / 12.0);

    coarse_out.clear();
    coarse_out.reserve(f0_hz.len());

    for f0 in f0_hz.iter_mut() {
        if *f0 > 0.0 {
            *f0 = (*f0 * pitch_mul).clamp(F0_MIN, F0_MAX);
            let f0_mel = 1127.0 * (1.0 + *f0 / 700.0).ln();
            let norm = (f0_mel - f0_mel_min) * 254.0 / (f0_mel_max - f0_mel_min) + 1.0;
            let idx = norm.round().clamp(1.0, 255.0) as i64;
            coarse_out.push(idx);
        } else {
            *f0 = 0.0;
            coarse_out.push(1);
        }
    }
}

fn hz_to_mel_htk(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

fn mel_to_hz_htk(mel: f32) -> f32 {
    700.0 * (10.0f32.powf(mel / 2595.0) - 1.0)
}

fn build_htk_slaney_mel_basis(
    sr: f32,
    n_fft: usize,
    n_mel: usize,
    fmin: f32,
    fmax: f32,
) -> Vec<f32> {
    let n_bins = n_fft / 2 + 1;
    let fft_freqs: Vec<f32> = (0..n_bins)
        .map(|k| (sr * 0.5) * (k as f32) / ((n_bins - 1) as f32))
        .collect();

    let mel_min = hz_to_mel_htk(fmin);
    let mel_max = hz_to_mel_htk(fmax);
    let mel_pts: Vec<f32> = (0..n_mel + 2)
        .map(|i| mel_min + (mel_max - mel_min) * (i as f32) / ((n_mel + 1) as f32))
        .collect();
    let freq_pts: Vec<f32> = mel_pts.into_iter().map(mel_to_hz_htk).collect();

    let mut basis = vec![0.0f32; n_mel * n_bins];
    for m in 0..n_mel {
        let lo = freq_pts[m];
        let center = freq_pts[m + 1];
        let hi = freq_pts[m + 2];
        // Librosa `norm="slaney"` area normalization (`2.0 / (f[m+2] - f[m])`)
        let enorm = 2.0 / (hi - lo).max(1e-6);

        for (k, &f) in fft_freqs.iter().enumerate() {
            let weight = if f >= lo && f <= center && center > lo {
                (f - lo) / (center - lo)
            } else if f > center && f <= hi && hi > center {
                (hi - f) / (hi - center)
            } else {
                0.0
            };
            basis[m * n_bins + k] = weight * enorm;
        }
    }
    basis
}

fn reflect_index(idx: isize, len: usize) -> usize {
    if len <= 1 {
        return 0;
    }
    let period = 2 * (len as isize - 1);
    let mut m = idx.rem_euclid(period);
    if m >= len as isize {
        m = period - m;
    }
    m as usize
}

fn reflect_pad_1d(src: &[f32], pad_left: usize, pad_right: usize, dst: &mut Vec<f32>) {
    dst.clear();
    if src.is_empty() {
        dst.resize(pad_left + pad_right, 0.0);
        return;
    }
    let n = src.len();
    dst.reserve(pad_left + n + pad_right);
    for i in 0..pad_left {
        let idx = -(pad_left as isize) + i as isize;
        dst.push(src[reflect_index(idx, n)]);
    }
    dst.extend_from_slice(src);
    for i in 0..pad_right {
        let idx = n as isize + i as isize;
        dst.push(src[reflect_index(idx, n)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rmvpe_mel_shape_and_cent_decoder() {
        let mut fe = RmvpeFrontend::new();
        // 3200 samples = 200 ms -> `1 + 3200 / 160 = 21` frames -> padded to 32
        let audio: Vec<f32> = (0..3200)
            .map(|n| (2.0 * PI * 220.0 * n as f32 / 16000.0).sin() * 0.5)
            .collect();
        let mut mel = Vec::new();
        let (n_frames, t_pad) = fe.compute_log_mel(&audio, &mut mel);
        assert_eq!(n_frames, 21);
        assert_eq!(t_pad, 32);
        assert_eq!(mel.len(), 128 * 32);
        assert!(mel.iter().all(|v| v.is_finite()));

        // Construct synthetic RMVPE salience peaking at 440 Hz:
        // 440 = 10 * 2^(cents / 1200) => cents = 1200 * log2(44) = 6551.318
        // bin = (6551.318 - 1997.3794) / 20 = 227.7 -> bin 228 (~441.5 Hz)
        let mut salience = vec![0.001f32; 4 * 360];
        salience[228] = 0.95; // frame 0 voiced @ ~441.5 Hz
        salience[2 * 360 + 228] = 0.92; // frame 2 voiced (frame 1 bridged!)
                                        // frame 3 stays unvoiced (`0.001 <= 0.03`)

        let mut f0 = Vec::new();
        decode_rmvpe_salience(&[1, 4, 360], &salience, 4, 0.03, &mut f0);
        assert_eq!(f0.len(), 4);
        assert!((f0[0] - 441.5).abs() < 3.0, "got f0[0]={}", f0[0]);
        assert!((f0[1] - 441.5).abs() < 3.0, "1-frame dropout should bridge");
        assert_eq!(f0[3], 0.0, "unvoiced frame must remain 0.0");

        let mut coarse = Vec::new();
        shift_and_quantize_f0(&mut f0, 12.0, &mut coarse);
        assert!((f0[0] - 883.0).abs() < 6.0);
        assert!(coarse[0] > 200 && coarse[0] <= 255);
        assert_eq!(coarse[3], 1, "unvoiced frame must map to coarse index 1");
    }
}
