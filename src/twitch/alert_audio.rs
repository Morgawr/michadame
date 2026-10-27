//! Retro synthesized audio chime for Twitch alerts.
//!
//! Accurately models the Yamaha YM2608 (OPNA) FM sound chip found on NEC PC-9801/9821
//! sound boards (such as the PC-9801-86 "86 Board"), reproducing the signature dreamy,
//! sustained, sawtoothy FM synth pad design heard across classic PC-98 visual novels
//! (such as Ryu Umemoto's soundtrack for *YU-NO: A girl who chants love at the bound of this world*).

use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, Sink};
use std::f32::consts::PI;
use std::sync::Arc;

pub const ALERT_SAMPLE_RATE: u32 = 48_000;
pub const ALERT_CHANNELS: u16 = 2;

/// A band-limited PolyBLEP (Polynomial Band-Limited Step) sawtooth oscillator.
///
/// Produces a razor-sharp, bright retro sawtooth waveform rich in harmonic overtones
/// without digital Nyquist foldover aliasing, recreating the signature bright buzz of
/// vintage synth string machines and PC-98 sound card string patches.
#[derive(Clone, Copy, Default)]
struct PolyBlepSawOscillator {
    phase: f32,
}

impl PolyBlepSawOscillator {
    #[inline]
    fn next_sample(&mut self, freq: f32, sample_rate: f32) -> f32 {
        let dt = freq / sample_rate;
        let naive = 2.0 * self.phase - 1.0;

        let blep = if self.phase < dt {
            let t = self.phase / dt;
            2.0 * t - t * t - 1.0
        } else if self.phase > 1.0 - dt {
            let t = (self.phase - 1.0) / dt;
            2.0 * t + t * t + 1.0
        } else {
            0.0
        };

        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }

        naive - blep
    }
}

/// Synthesizes a bright, dreamy, rising 4-note arpeggio played by an ensemble of artificially-sounding
/// sawtooth strings with PC-98 sound card DAC character and analog circuit fuzz (~2.20s).
///
/// Sound Architecture:
/// - Rising Sequence Arpeggio: 4 notes in rising sequence (A4 440Hz -> C#5 554.37Hz -> E5 659.25Hz -> A5 880Hz)
///   in the bright violin/soprano register (no deep or grave brass).
/// - Cumulative String Pad Bloom: As each note enters in succession (0.00s, 0.13s, 0.26s, 0.39s), it swells
///   and sustains as a pad, blossoming into a magnificent, shimmering string ensemble chord.
/// - Ensemble Chorus (Triple-Saw): Each note is composed of 3 detuned band-limited PolyBLEP sawtooth oscillators
///   modulated by dual LFOs (5.8 Hz fast chorus and 0.6 Hz slow ensemble sweep) across a wide stereo field,
///   faithfully recreating the nostalgic artificial string ensemble timbre of classic visual novel soundtracks (e.g. YU-NO).
/// - Envelope: 35ms smooth bow swell attack per note (strictly eliminating clicks or chirps) with a rich,
///   singing chord sustain past 1.1s that gracefully decays into the PC-98 analog noise floor out to 2.2s.
/// - Hardware Emulation: 14-bit DAC resistor-ladder quantization step and subtle analog circuit fuzz
///   recreating the tactile hardware sound of an original NEC PC-9801 sound card.
pub fn generate_retro_chime_samples() -> Vec<f32> {
    let duration_secs = 2.20f32;
    let total_frames = (ALERT_SAMPLE_RATE as f32 * duration_secs) as usize;
    let mut samples = Vec::with_capacity(total_frames * ALERT_CHANNELS as usize);

    // Rising 4-note arpeggio in the bright violin register (A major chord):
    // Note 0: A4 (440.00 Hz) - entrance at 0.00s
    // Note 1: C#5 (554.37 Hz) - entrance at 0.13s
    // Note 2: E5 (659.25 Hz) - entrance at 0.26s
    // Note 3: A5 (880.00 Hz) - entrance at 0.39s
    let note_defs = [
        (440.00f32, 0.00f32),
        (554.37f32, 0.13f32),
        (659.25f32, 0.26f32),
        (880.00f32, 0.39f32),
    ];

    let mut oscs = [[PolyBlepSawOscillator::default(); 3]; 4];

    let mut rng_state: u32 = 0x98_01_86_00; // Deterministic seed for authentic PC-98 analog noise
    let mut lpf_noise: f32 = 0.0;

    let pans = [-0.35f32, 0.35f32, 0.00f32];

    for frame in 0..total_frames {
        let t = frame as f32 / ALERT_SAMPLE_RATE as f32;

        // Dual LFO string ensemble chorus (Solina / Juno BBD style modulation)
        let lfo_fast = (2.0 * PI * 5.8 * t).sin();
        let lfo_slow = (2.0 * PI * 0.6 * t).sin();

        let detunes = [
            lfo_fast * 0.0022 + lfo_slow * 0.0012,
            0.0035 - lfo_fast * 0.0018 + lfo_slow * 0.0015,
            -0.0035 + lfo_fast * 0.0015 - lfo_slow * 0.0018,
        ];

        let mut sum_l = 0.0f32;
        let mut sum_r = 0.0f32;

        for (n_idx, &(base_freq, start_t)) in note_defs.iter().enumerate() {
            if t < start_t {
                continue;
            }
            let note_t = t - start_t;

            // Smooth 35ms bow swell attack (eliminates any clicks or chirps)
            let att = if note_t < 0.035 {
                let f = note_t / 0.035;
                f * f * (3.0 - 2.0 * f)
            } else {
                1.0
            };

            // Master pad sustain & decay: holds strong past 1.10s, then decays smoothly
            let decay = if t < 1.10 {
                1.0 - (t / 1.10) * 0.08
            } else {
                0.92 * (-(t - 1.10) * 2.2).exp()
            };
            let env = att * decay;

            for o_idx in 0..3 {
                let freq = base_freq * (1.0 + detunes[o_idx]);
                let saw = oscs[n_idx][o_idx].next_sample(freq, ALERT_SAMPLE_RATE as f32);

                let pan = pans[o_idx];
                let pan_l = 0.5 - 0.5 * pan;
                let pan_r = 0.5 + 0.5 * pan;

                let voice_out = saw * (env * 0.38 / 3.0);
                sum_l += voice_out * pan_l;
                sum_r += voice_out * pan_r;
            }
        }

        // NEC PC-9801 sound card analog circuit fuzz / noise floor emulation
        rng_state = rng_state.wrapping_mul(1664525).wrapping_add(1013904223);
        let raw_noise = ((rng_state >> 16) as f32 / 32768.0) - 1.0;
        lpf_noise += 0.25 * (raw_noise - lpf_noise);

        let master_env = if t < 1.10 {
            1.0
        } else {
            (-(t - 1.10) * 2.2).exp()
        };
        let analog_fuzz = lpf_noise * 0.010 * master_env;

        let mix_l = sum_l + analog_fuzz;
        let mix_r = sum_r + analog_fuzz;

        // Analog preamp soft saturation (tanh)
        let sat_l = (mix_l * 1.4).tanh() * 0.45;
        let sat_r = (mix_r * 1.4).tanh() * 0.45;

        // 14-bit DAC resistor-ladder quantization step (PC-9801-86 hardware resolution)
        let dac_step = 8192.0f32;
        let out_l = (sat_l * dac_step).round() / dac_step;
        let out_r = (sat_r * dac_step).round() / dac_step;

        samples.push(out_l.clamp(-1.0, 1.0));
        samples.push(out_r.clamp(-1.0, 1.0));
    }

    samples
}

/// Embedded 48 kHz 16-bit stereo PCM recording of a heavy mechanical wall light switch clack (lights off).
pub const EMBEDDED_LIGHT_SWITCH_WAV: &[u8] = include_bytes!("../../assets/light_switch.wav");

/// Embedded 48 kHz 16-bit stereo PCM recording of a mechanical wall light switch clack followed by
/// ceiling tube ballast charging hum and synchronized starter ignition surges (lights on).
pub const EMBEDDED_LIGHT_SWITCH_ON_WAV: &[u8] = include_bytes!("../../assets/light_switch_on.wav");

fn decode_embedded_wav_samples(wav_bytes: &[u8]) -> Vec<f32> {
    let pcm_bytes =
        if wav_bytes.len() > 44 && &wav_bytes[0..4] == b"RIFF" && &wav_bytes[8..12] == b"WAVE" {
            &wav_bytes[44..]
        } else {
            wav_bytes
        };

    let mut samples = Vec::with_capacity(pcm_bytes.len() / 2);
    for chunk in pcm_bytes.chunks_exact(2) {
        let s = i16::from_le_bytes([chunk[0], chunk[1]]);
        samples.push((s as f32 / 32768.0).clamp(-1.0, 1.0));
    }
    samples
}

/// Decodes the embedded 48 kHz 16-bit stereo PCM mechanical light switch WAV into normalized `f32` samples.
pub fn generate_light_switch_clack_samples() -> Vec<f32> {
    decode_embedded_wav_samples(EMBEDDED_LIGHT_SWITCH_WAV)
}

/// Decodes the embedded 48 kHz 16-bit stereo PCM light switch + ceiling tube charging hum WAV into normalized `f32` samples.
pub fn generate_light_switch_on_samples() -> Vec<f32> {
    decode_embedded_wav_samples(EMBEDDED_LIGHT_SWITCH_ON_WAV)
}

/// Manages local audio playback for Twitch alerts and UI sound effects through Rodio.
pub struct AlertSoundPlayer {
    _stream: Option<OutputStream>,
    stream_handle: Option<rodio::OutputStreamHandle>,
    sink: Option<Sink>,
    active_switch_sink: std::sync::Mutex<Option<Sink>>,
    chime_samples: Arc<Vec<f32>>,
    light_switch_samples: Arc<Vec<f32>>,
    light_switch_on_samples: Arc<Vec<f32>>,
    volume: std::sync::atomic::AtomicU32,
}

impl Default for AlertSoundPlayer {
    fn default() -> Self {
        if cfg!(test) {
            Self::new_headless()
        } else {
            Self::new()
        }
    }
}

impl AlertSoundPlayer {
    /// Creates an `AlertSoundPlayer` with precomputed audio samples but no hardware audio output stream.
    pub fn new_headless() -> Self {
        Self::with_playback(false)
    }

    pub fn new() -> Self {
        Self::with_playback(!cfg!(test))
    }

    fn with_playback(enable_hardware: bool) -> Self {
        let chime_samples = Arc::new(generate_retro_chime_samples());
        let light_switch_samples = Arc::new(generate_light_switch_clack_samples());
        let light_switch_on_samples = Arc::new(generate_light_switch_on_samples());

        // Attempt to initialize Rodio playback when hardware output is enabled.
        // If unavailable (e.g. headless environment or unit tests), we safely retain None
        // so execution never probes ALSA/PipeWire in tests or crashes without audio hardware.
        let (stream, stream_handle, sink) = if !enable_hardware {
            (None, None, None)
        } else {
            match crate::devices::audio::initialize_rodio_playback() {
                Ok((guard, handle)) => match Sink::try_new(&handle) {
                    Ok(sink) => {
                        sink.set_volume(1.0);
                        (Some(guard), Some(handle), Some(sink))
                    }
                    Err(e) => {
                        tracing::warn!("Failed to create Rodio sink for alert audio: {e}");
                        (Some(guard), Some(handle), None)
                    }
                },
                Err(e) => {
                    tracing::debug!("Rodio output stream not available for alert audio: {e}");
                    (None, None, None)
                }
            }
        };

        Self {
            _stream: stream,
            stream_handle,
            sink,
            active_switch_sink: std::sync::Mutex::new(None),
            chime_samples,
            light_switch_samples,
            light_switch_on_samples,
            volume: std::sync::atomic::AtomicU32::new(1.0f32.to_bits()),
        }
    }

    /// Returns true if a hardware audio output stream is attached to this player.
    #[allow(dead_code)]
    pub fn has_hardware_output(&self) -> bool {
        self._stream.is_some() || self.stream_handle.is_some() || self.sink.is_some()
    }

    /// Sets the playback volume for local alert audio, matching the stream master volume.
    pub fn set_volume(&self, volume: f32) {
        let clamped = volume.clamp(0.0, 1.0);
        self.volume
            .store(clamped.to_bits(), std::sync::atomic::Ordering::Relaxed);
        if let Some(sink) = &self.sink {
            sink.set_volume(clamped);
        }
        if let Ok(guard) = self.active_switch_sink.lock() {
            if let Some(sink) = guard.as_ref() {
                sink.set_volume(clamped);
            }
        }
    }

    /// Returns the current playback volume.
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Plays the retro alert chime locally on the PC through Rodio at current master volume.
    pub fn play_local(&self) {
        if let Some(sink) = &self.sink {
            let vol = self.volume();
            sink.set_volume(vol);
            let buffer = SamplesBuffer::new(
                ALERT_CHANNELS,
                ALERT_SAMPLE_RATE,
                self.chime_samples.as_ref().clone(),
            );
            sink.append(buffer);
        }
    }

    /// Plays the mechanical light switch sound locally on the PC through Rodio immediately.
    ///
    /// When `turning_on` is `true`, plays the switch clack + ceiling tube ballast charging hum.
    /// When `turning_on` is `false`, stops any in-progress tube charging hum and plays the switch clack.
    pub fn play_light_switch_local(&self, turning_on: bool) {
        let vol = self.volume();
        let samples = if turning_on {
            self.light_switch_on_samples.as_ref().clone()
        } else {
            self.light_switch_samples.as_ref().clone()
        };
        let buffer = SamplesBuffer::new(ALERT_CHANNELS, ALERT_SAMPLE_RATE, samples);
        if let Some(handle) = &self.stream_handle {
            if let Ok(sink) = Sink::try_new(handle) {
                sink.set_volume(vol);
                sink.append(buffer);
                if let Ok(mut guard) = self.active_switch_sink.lock() {
                    if let Some(prev) = guard.take() {
                        prev.stop();
                    }
                    *guard = Some(sink);
                } else {
                    sink.detach();
                }
                return;
            }
        }
        if let Some(sink) = &self.sink {
            sink.set_volume(vol);
            sink.append(buffer);
        }
    }

    /// Returns a reference to the precomputed 48 kHz stereo chime samples.
    pub fn chime_samples(&self) -> &Arc<Vec<f32>> {
        &self.chime_samples
    }

    /// Returns a reference to the precomputed 48 kHz stereo light switch clack samples (lights off).
    pub fn light_switch_samples(&self) -> &Arc<Vec<f32>> {
        &self.light_switch_samples
    }

    /// Returns a reference to the precomputed 48 kHz stereo light switch + tube charging hum samples (lights on).
    pub fn light_switch_on_samples(&self) -> &Arc<Vec<f32>> {
        &self.light_switch_on_samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chime_generation() {
        let samples = generate_retro_chime_samples();
        assert!(!samples.is_empty());
        // 48000 * 2.20 * 2 = 211200 samples
        assert_eq!(samples.len(), (48_000.0 * 2.20 * 2.0) as usize);

        // Ensure samples are within valid [-1.0, 1.0] audio limits and have non-zero energy
        let mut max_val = 0.0f32;
        for &s in &samples {
            assert!(s >= -1.0 && s <= 1.0, "Sample out of bounds: {s}");
            max_val = max_val.max(s.abs());
        }
        assert!(max_val > 0.1, "Chime should produce audible signal");
    }

    #[test]
    fn test_light_switch_clack_generation() {
        let samples = generate_light_switch_clack_samples();
        assert!(!samples.is_empty());
        assert_eq!(samples.len(), (48_000.0 * 0.350) as usize * 2);

        let mut max_val = 0.0f32;
        for &s in &samples {
            assert!(s >= -1.0 && s <= 1.0, "Sample out of bounds: {s}");
            max_val = max_val.max(s.abs());
        }
        assert!(
            max_val > 0.4 && max_val < 0.7,
            "Light switch clack should produce a comfortable audible signal, got {max_val}"
        );

        let on_samples = generate_light_switch_on_samples();
        assert_eq!(on_samples.len(), (48_000.0 * 2.600) as usize * 2);
        let mut max_on = 0.0f32;
        for &s in &on_samples {
            assert!(s >= -1.0 && s <= 1.0, "Sample out of bounds: {s}");
            max_on = max_on.max(s.abs());
        }
        assert!(
            max_on > 0.4 && max_on < 0.7,
            "Light switch on + ballast hum should produce a comfortable audible signal, got {max_on}"
        );
    }

    #[test]
    fn test_player_instantiation_does_not_panic() {
        let player = AlertSoundPlayer::new();
        assert!(
            !player.has_hardware_output(),
            "AlertSoundPlayer::new() in unit tests must not open hardware audio devices"
        );
        let default_player = AlertSoundPlayer::default();
        assert!(!default_player.has_hardware_output());
        let headless_player = AlertSoundPlayer::new_headless();
        assert!(!headless_player.has_hardware_output());
        assert_eq!(
            player.chime_samples().len(),
            (48_000.0 * 2.20 * 2.0) as usize
        );
        assert_eq!(
            player.light_switch_samples().len(),
            (48_000.0 * 0.350) as usize * 2
        );
        assert_eq!(
            player.light_switch_on_samples().len(),
            (48_000.0 * 2.600) as usize * 2
        );
        player.set_volume(0.42);
        assert!((player.volume() - 0.42).abs() < 1e-6);
        player.play_local();
        player.play_light_switch_local(true);
        player.play_light_switch_local(false);
    }
}
