use anyhow::{Context, Result};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

pub const GAMESCOPE_SINK_NAME: &str = "michadame_gamescope";

pub struct GamescopeAudioHandle {
    pub stream: crate::devices::audio::AudioStreamHandle,
    pub module_id: Option<String>,
}

impl Drop for GamescopeAudioHandle {
    fn drop(&mut self) {
        if let Some(ref id) = self.module_id {
            let _ = crate::portable::helper_command("pactl")
                .args(["unload-module", id])
                .output();
        }
    }
}

pub fn ensure_gamescope_sink() -> Result<Option<String>> {
    let output = crate::portable::helper_command("pactl")
        .args(["list", "sinks", "short"])
        .output()
        .context("Failed to run pactl to inspect audio sinks")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.contains(GAMESCOPE_SINK_NAME) {
        tracing::debug!("Gamescope null-sink {GAMESCOPE_SINK_NAME} already exists.");
        return Ok(None);
    }

    let load_output = crate::portable::helper_command("pactl")
        .args([
            "load-module",
            "module-null-sink",
            &format!("sink_name={GAMESCOPE_SINK_NAME}"),
            "sink_properties=device.description=GamescopeAudio",
        ])
        .output()
        .context("Failed to load module-null-sink via pactl")?;

    if !load_output.status.success() {
        let stderr = String::from_utf8_lossy(&load_output.stderr);
        anyhow::bail!("Failed to create Gamescope null-sink: {stderr}");
    }

    let mod_id = String::from_utf8_lossy(&load_output.stdout)
        .trim()
        .to_string();
    tracing::info!("Created Gamescope null-sink with module ID: {mod_id}");
    Ok(Some(mod_id))
}

pub fn start_gamescope_audio(
    peak_amplitude_shared: Arc<AtomicU64>,
    audio_latency_ms: Arc<AtomicU64>,
    buffer_size: u32,
    sample_rate: u32,
    sample_format: String,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    replay: Arc<crate::replay::AudioTap>,
) -> Result<GamescopeAudioHandle> {
    let module_id = ensure_gamescope_sink().unwrap_or(None);

    let monitor_source = format!("{GAMESCOPE_SINK_NAME}.monitor");
    let alsa_device = format!("pulse:DEVICE={monitor_source}");

    let stream = crate::devices::audio::start_audio_stream(
        &alsa_device,
        peak_amplitude_shared,
        audio_latency_ms,
        buffer_size,
        sample_rate,
        sample_format,
        request_repaint,
        replay,
    )
    .context("Failed to start audio capture stream for Gamescope monitor")?;

    Ok(GamescopeAudioHandle { stream, module_id })
}
