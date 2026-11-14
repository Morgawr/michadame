use std::f64::consts::PI;

const HALF_TAPS: usize = 8;
const TAPS: usize = HALF_TAPS * 2; // 16-tap windowed-sinc kernel

/// Streaming zero-allocation windowed-sinc fractional resampler.
///
/// Designed for real-time voice conversion rate conversions:
/// - Input decimation (`48 kHz -> 16 kHz * formant_ratio`) with anti-aliasing low-pass cutoff
/// - Output interpolation (`(40 kHz | 48 kHz) * formant_ratio -> 48 kHz`)
pub struct StreamingResampler {
    in_rate: f64,
    out_rate: f64,
    step: f64, // in_samples per out_sample = in_rate / out_rate
    phase: f64,
    history: Vec<f32>,
    work_buf: Vec<f32>,
}

impl StreamingResampler {
    pub fn new(in_rate: f64, out_rate: f64) -> Self {
        let mut s = Self {
            in_rate: 48_000.0,
            out_rate: 48_000.0,
            step: 1.0,
            phase: 0.0,
            history: vec![0.0; TAPS],
            work_buf: Vec::with_capacity(8192),
        };
        s.set_rates(in_rate, out_rate);
        s
    }

    pub fn set_rates(&mut self, in_rate: f64, out_rate: f64) {
        let ir = in_rate.max(1000.0);
        let or = out_rate.max(1000.0);
        self.in_rate = ir;
        self.out_rate = or;
        self.step = ir / or;
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.history.fill(0.0);
    }

    /// Resamples `input` and appends output samples into `output`.
    /// Does not allocate once `work_buf` and `output` reach steady-state capacity.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }

        self.work_buf.clear();
        self.work_buf.extend_from_slice(&self.history);
        self.work_buf.extend_from_slice(input);

        // Anti-aliasing normalized cutoff ( <= 0.46 of Nyquist of whichever rate is lower ).
        let cutoff = (self.out_rate / self.in_rate).min(1.0) * 0.46;

        // Center index inside `work_buf`: history occupies `[0..TAPS]`, where `HALF_TAPS - 1`
        // is the oldest active center when `phase == 0.0`, leaving `HALF_TAPS` lookahead taps.
        // Available valid center positions satisfy `center + HALF_TAPS < work_buf.len()`.
        let max_pos = (self.work_buf.len() - TAPS) as f64;
        let mut pos = self.phase;

        while pos <= max_pos {
            let base = pos.floor() as usize;
            let frac = pos - base as f64;

            let mut sum = 0.0f64;
            let mut weight_sum = 0.0f64;

            for k in 0..TAPS {
                // Tap offset relative to fractional sample point:
                // k = HALF_TAPS - 1 corresponds to `-frac`, k = HALF_TAPS corresponds to `1 - frac`.
                let x = (k as isize - (HALF_TAPS as isize - 1)) as f64 - frac;
                let sinc_arg = 2.0 * cutoff * x;
                let sinc = if sinc_arg.abs() < 1e-9 {
                    1.0
                } else {
                    (PI * sinc_arg).sin() / (PI * sinc_arg)
                };

                // Blackman window over [-HALF_TAPS, +HALF_TAPS]
                let win_pos = (x + HALF_TAPS as f64) / (TAPS as f64);
                let window = if (0.0..=1.0).contains(&win_pos) {
                    0.42 - 0.5 * (2.0 * PI * win_pos).cos() + 0.08 * (4.0 * PI * win_pos).cos()
                } else {
                    0.0
                };

                let w = sinc * window;
                weight_sum += w;
                sum += self.work_buf[base + k] as f64 * w;
            }

            let sample = if weight_sum.abs() > 1e-12 {
                (sum / weight_sum) as f32
            } else {
                0.0
            };
            output.push(sample);
            pos += self.step;
        }

        let consumed = input.len();
        self.phase = (pos - consumed as f64).max(0.0);

        let tail_start = self.work_buf.len() - TAPS;
        self.history
            .copy_from_slice(&self.work_buf[tail_start..tail_start + TAPS]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resampler_preserves_duration_and_sine_fidelity() {
        let in_rate = 48_000.0;
        let out_rate = 16_000.0;
        let mut down = StreamingResampler::new(in_rate, out_rate);
        let mut up = StreamingResampler::new(out_rate, in_rate);

        // 0.5 seconds of 440 Hz sine wave fed in 10 ms chunks
        let total_in = 24_000usize;
        let mut input = Vec::with_capacity(total_in);
        for n in 0..total_in {
            let t = n as f64 / in_rate;
            input.push((2.0 * PI * 440.0 * t).sin() as f32 * 0.8);
        }

        let mut mid = Vec::new();
        let mut out = Vec::new();
        for chunk in input.chunks(480) {
            let prev_mid = mid.len();
            down.process(chunk, &mut mid);
            up.process(&mid[prev_mid..], &mut out);
        }

        // Allow filter group-delay startup tolerance
        assert!(
            (mid.len() as isize - 8000).abs() <= 16,
            "expected ~8000 decimated samples, got {}",
            mid.len()
        );
        assert!(
            (out.len() as isize - total_in as isize).abs() <= 32,
            "expected ~{total_in} reconstructed samples, got {}",
            out.len()
        );

        // Steady-state amplitude in middle of signal should match ~0.8 closely
        let peak = out[4000..20000]
            .iter()
            .fold(0.0f32, |acc, &v| acc.max(v.abs()));
        assert!(
            (peak - 0.8).abs() < 0.03,
            "expected ~0.8 amplitude, got {peak}"
        );
    }
}
