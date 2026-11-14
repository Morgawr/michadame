use std::f32::consts::PI;

/// Synchronous Overlap-Add (SOLA) phase-aligned streaming stitcher.
///
/// Each synthesis step passes the tail segment of length `hop + overlap + search`
/// at output sample rate. `SolaStitcher` searches `0..=search` for the offset
/// maximizing normalized cross-correlation with `prev_tail`, crossfades over
/// `overlap` samples with raised-cosine weights, emits `hop` click-free samples,
/// and updates `prev_tail`.
pub struct SolaStitcher {
    overlap: usize,
    search: usize,
    prev_tail: Vec<f32>,
    fade_in: Vec<f32>,
    fade_out: Vec<f32>,
    primed: bool,
}

impl SolaStitcher {
    pub fn new(sample_rate: u32, overlap_ms: f32, search_ms: f32) -> Self {
        let overlap = ((sample_rate as f32 * overlap_ms / 1000.0).round() as usize).max(32);
        let search = ((sample_rate as f32 * search_ms / 1000.0).round() as usize).max(16);

        let mut fade_in = Vec::with_capacity(overlap);
        let mut fade_out = Vec::with_capacity(overlap);
        for i in 0..overlap {
            // Hann half-window complementary crossfade (`fade_in[i] + fade_out[i] == 1.0`)
            let t = (i as f32 + 0.5) / overlap as f32;
            let w_in = 0.5 * (1.0 - (PI * t).cos());
            fade_in.push(w_in);
            fade_out.push(1.0 - w_in);
        }

        Self {
            overlap,
            search,
            prev_tail: vec![0.0; overlap],
            fade_in,
            fade_out,
            primed: false,
        }
    }

    #[allow(dead_code)]
    pub fn overlap_frames(&self) -> usize {
        self.overlap
    }

    #[allow(dead_code)]
    pub fn search_frames(&self) -> usize {
        self.search
    }

    /// Required tail segment length for a given `hop` size: `hop + overlap + search`.
    pub fn required_segment_len(&self, hop: usize) -> usize {
        hop + self.overlap + self.search
    }

    /// Nominal lookahead delay introduced by SOLA (`overlap + search / 2`).
    #[allow(dead_code)]
    pub fn algorithmic_delay_frames(&self) -> usize {
        self.overlap + self.search / 2
    }

    pub fn reset(&mut self) {
        self.prev_tail.fill(0.0);
        self.primed = false;
    }

    /// Stitches a synthesized tail into `out` (producing exactly `hop` samples).
    /// If `segment` is shorter than `hop + overlap + search`, left-pads with zeros safely.
    pub fn process(&mut self, segment: &[f32], hop: usize, out: &mut Vec<f32>) {
        if hop == 0 {
            return;
        }
        let req = self.required_segment_len(hop);
        let padded_storage;
        let seg: &[f32] = if segment.len() >= req {
            &segment[segment.len() - req..]
        } else {
            let mut tmp = vec![0.0f32; req];
            let offset = req - segment.len();
            tmp[offset..].copy_from_slice(segment);
            padded_storage = tmp;
            &padded_storage
        };

        let best_offset = if !self.primed {
            self.primed = true;
            self.search / 2
        } else {
            self.find_best_offset(seg)
        };

        out.reserve(hop);
        let fade_len = self.overlap.min(hop);
        for i in 0..fade_len {
            let blended =
                self.prev_tail[i] * self.fade_out[i] + seg[best_offset + i] * self.fade_in[i];
            out.push(blended);
        }

        if hop > self.overlap {
            out.extend_from_slice(&seg[best_offset + self.overlap..best_offset + hop]);
            self.prev_tail
                .copy_from_slice(&seg[best_offset + hop..best_offset + hop + self.overlap]);
        } else {
            // Very small hop (< overlap): advance remaining crossfaded tail smoothly
            let rem = self.overlap - hop;
            for i in 0..rem {
                let blended = self.prev_tail[hop + i] * self.fade_out[hop + i]
                    + seg[best_offset + hop + i] * self.fade_in[hop + i];
                self.prev_tail[i] = blended;
            }
            self.prev_tail[rem..].copy_from_slice(
                &seg[best_offset + self.overlap..best_offset + self.overlap + hop],
            );
        }
    }

    /// Smoothly fades `prev_tail` to silence when voice gate closes, avoiding abrupt cutoffs.
    pub fn process_silence(&mut self, hop: usize, out: &mut Vec<f32>) {
        if hop == 0 {
            return;
        }
        out.reserve(hop);
        let fade_len = self.overlap.min(hop);
        for i in 0..fade_len {
            out.push(self.prev_tail[i] * self.fade_out[i]);
        }
        for _ in fade_len..hop {
            out.push(0.0);
        }
        self.prev_tail.fill(0.0);
        self.primed = false;
    }

    fn find_best_offset(&self, seg: &[f32]) -> usize {
        let mut prev_energy = 0.0f64;
        for &v in &self.prev_tail {
            prev_energy += (v as f64) * (v as f64);
        }
        if prev_energy < 1e-8 {
            return self.search / 2;
        }

        let nominal = self.search / 2;
        let mut best_offset = nominal;
        let mut best_score = f64::NEG_INFINITY;

        // Running window energy over `seg[offset .. offset + overlap]`
        let mut seg_energy = 0.0f64;
        for &v in &seg[0..self.overlap] {
            seg_energy += (v as f64) * (v as f64);
        }

        for offset in 0..=self.search {
            if offset > 0 {
                let leaving = seg[offset - 1] as f64;
                let entering = seg[offset + self.overlap - 1] as f64;
                seg_energy += entering * entering - leaving * leaving;
                seg_energy = seg_energy.max(0.0);
            }

            let mut dot = 0.0f64;
            let cand = &seg[offset..offset + self.overlap];
            for i in 0..self.overlap {
                dot += (self.prev_tail[i] as f64) * (cand[i] as f64);
            }

            let denom = (prev_energy * seg_energy).sqrt() + 1e-9;
            // Mild centering bias (`0.005`) keeps steady silence/noise near nominal offset
            let dist_frac = (offset as f64 - nominal as f64).abs() / (self.search.max(1) as f64);
            let score = (dot / denom) - 0.005 * dist_frac;

            if score > best_score {
                best_score = score;
                best_offset = offset;
            }
        }

        best_offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sola_eliminates_phase_jump_discontinuity() {
        let sr = 48_000u32;
        let hop = 2880usize; // 60 ms
        let mut sola = SolaStitcher::new(sr, 10.0, 6.0);
        let req = sola.required_segment_len(hop);

        let freq = 220.0f32; // ~218 samples period (< 288 search window)
        let mut stitched = Vec::new();

        for chunk_idx in 0..8 {
            // Inject deliberate random-like phase jitter (+/- 60 samples) per chunk,
            // simulating independent neural NSF vocoder phase between chunks.
            let phase_jitter_samples = ((chunk_idx * 73) % 120) as f32 - 60.0;
            let mut seg = Vec::with_capacity(req);
            for i in 0..req {
                let t = (i as f32 + phase_jitter_samples) / sr as f32;
                seg.push((2.0 * PI * freq * t).sin() * 0.7);
            }
            sola.process(&seg, hop, &mut stitched);
        }

        assert_eq!(stitched.len(), 8 * hop);

        // Verify zero boundary clicks: max sample-to-sample delta for 220 Hz @ 48k is
        // `2 * pi * 220 / 48000 * 0.7 ~= 0.0202`. Any unaligned phase jump would spike > 0.4!
        let mut max_step = 0.0f32;
        for w in stitched[hop..].windows(2) {
            max_step = max_step.max((w[1] - w[0]).abs());
        }
        assert!(
            max_step < 0.045,
            "SOLA left a phase discontinuity step of {max_step}"
        );
    }

    #[test]
    fn test_sola_12ms_search_aligns_low_fundamental_110hz() {
        let sr = 48_000u32;
        let hop = 2880usize; // 60 ms
        let mut sola = SolaStitcher::new(sr, 28.0, 12.0);
        let req = sola.required_segment_len(hop);

        let freq = 110.0f32; // ~436 samples period (9.09 ms, requires >9.1 ms search window)
        let mut stitched = Vec::new();

        for chunk_idx in 0..8 {
            // Arbitrary 0..436 sample phase jump per chunk (full 360-degree phase randomization)
            let phase_jitter_samples = ((chunk_idx * 211) % 436) as f32;
            let mut seg = Vec::with_capacity(req);
            for i in 0..req {
                let t = (i as f32 + phase_jitter_samples) / sr as f32;
                seg.push((2.0 * PI * freq * t).sin() * 0.7);
            }
            sola.process(&seg, hop, &mut stitched);
        }

        assert_eq!(stitched.len(), 8 * hop);
        let mut max_step = 0.0f32;
        for w in stitched[hop..].windows(2) {
            max_step = max_step.max((w[1] - w[0]).abs());
        }
        assert!(
            max_step < 0.025,
            "SOLA 12ms search failed to phase-align 110 Hz waveform (max_step={max_step})"
        );
    }
}
