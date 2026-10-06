use super::models::{MichadameConfig, Profile};
use crate::app::models::AppState;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn quarantine_corrupted_config(path: &Path) -> Option<PathBuf> {
    if path.exists() && std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false) {
        let parent = path.parent()?;
        let backups_dir = parent.join("backups");
        let _ = std::fs::create_dir_all(&backups_dir);
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let quarantine_path = backups_dir.join(format!("corrupted-{}.toml", now_secs));
        if std::fs::copy(path, &quarantine_path).is_ok() {
            tracing::warn!("Quarantined unreadable config to {:?}", quarantine_path);
            return Some(quarantine_path);
        }
    }
    None
}

pub fn create_rotating_backup(config_path: &Path) {
    if !config_path.exists() {
        return;
    }
    let size = std::fs::metadata(config_path).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return;
    }

    // 1. Direct .bak file alongside config
    let bak_path = config_path.with_extension("toml.bak");
    let _ = std::fs::copy(config_path, &bak_path);

    // 2. Timestamped rotating backup in backups/
    if let Some(parent) = config_path.parent() {
        let backups_dir = parent.join("backups");
        if std::fs::create_dir_all(&backups_dir).is_ok() {
            let now_secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let backup_file = backups_dir.join(format!("config-{}.toml", now_secs));
            let _ = std::fs::copy(config_path, &backup_file);

            prune_old_backups(&backups_dir, 20);
        }
    }
}

pub fn prune_old_backups(dir: &Path, max_keep: usize) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut backups: Vec<(SystemTime, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let s = name.to_string_lossy();
                s.starts_with("config-") && s.ends_with(".toml")
            })
            .map(|e| {
                let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
                (mtime, e.path())
            })
            .collect();

        if backups.len() > max_keep {
            backups.sort_by_key(|b| b.0); // oldest first
            let to_remove = backups.len() - max_keep;
            for (_, path) in backups.into_iter().take(to_remove) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[allow(dead_code)]
pub fn store_config_atomic(cfg: &MichadameConfig) -> Result<(), confy::ConfyError> {
    let path = confy::get_configuration_file_path("michadame", None)?;
    store_config_atomic_at(&path, cfg)
}

pub fn store_config_atomic_at(path: &Path, cfg: &MichadameConfig) -> Result<(), confy::ConfyError> {
    create_rotating_backup(path);

    let parent = path.parent().ok_or_else(|| {
        confy::ConfyError::BadConfigDirectory(format!("{:?} is a root or prefix", path))
    })?;
    std::fs::create_dir_all(parent).map_err(confy::ConfyError::DirectoryCreationFailed)?;

    let tmp_path = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("config"),
        std::process::id()
    ));

    confy::store_path(&tmp_path, cfg)?;

    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(confy::ConfyError::WriteConfigurationFileError(e));
    }

    Ok(())
}

pub fn save_replay_config(
    replay: &crate::replay::config::ReplayConfig,
) -> Result<(), confy::ConfyError> {
    let path = confy::get_configuration_file_path("michadame", None)?;
    save_replay_config_at(&path, replay)
}

fn save_replay_config_at(
    path: &std::path::Path,
    replay: &crate::replay::config::ReplayConfig,
) -> Result<(), confy::ConfyError> {
    // Preserve hardware/profile settings and report read failures rather than
    // replacing an unreadable configuration with defaults.
    let mut cfg: MichadameConfig = confy::load_path(path)?;
    cfg.replay = replay.clone();
    store_config_atomic_at(path, &cfg)
}

pub fn build_profile_from_state(state: &AppState) -> Profile {
    Profile {
        video_format_fourcc: state
            .hardware
            .supported_formats
            .get(state.hardware.selected_format_index)
            .map(|f| f.fourcc.clone()),
        crt_filter: Some(state.crt_filter.load(Ordering::Relaxed)),
        scaler_filter: Some(state.scaler_filter.load(Ordering::Relaxed)),
        color_range: Some(state.color_range.load(Ordering::Relaxed)),
        pixelate_filter_enabled: Some(state.video.pixelate_filter_enabled),

        crt_hard_scan: Some(state.crt.hard_scan),
        crt_warp_x: Some(state.crt.warp_x),
        crt_warp_y: Some(state.crt.warp_y),
        crt_shadow_mask: Some(state.crt.shadow_mask),
        crt_brightboost: Some(state.crt.brightboost),
        crt_hard_bloom_pix: Some(state.crt.hard_bloom_pix),
        crt_hard_bloom_scan: Some(state.crt.hard_bloom_scan),
        crt_bloom_amount: Some(state.crt.bloom_amount),
        crt_shape: Some(state.crt.shape),
        crt_hard_pix: Some(state.crt.hard_pix),
        use_magenta_background: Some(state.video.use_magenta_background),
        retro_pc_frame: Some(state.video.retro_pc_frame),
        crt_glass_enabled: Some(state.video.crt_glass_enabled),
        crt_glass_intensity: Some(state.video.crt_glass_intensity),
        crt_glass_glossiness: Some(state.video.crt_glass_glossiness),
        crt_glass_photographer_enabled: Some(state.video.crt_glass_photographer_enabled),
        crt_glass_photographer_intensity: Some(state.video.crt_glass_photographer_intensity),
        crt_glass_flash_enabled: Some(state.video.crt_glass_flash_enabled),
        crt_glass_flash_intensity: Some(state.video.crt_glass_flash_intensity),
        horizontal_stretch: Some(state.video.horizontal_stretch),
        median_filter_enabled: Some(state.video.median_filter_enabled),
        median_mix: Some(state.video.median_mix),
        vibrance: Some(state.video.vibrance),
        overscan_x: Some(state.video.overscan_x),
        overscan_y: Some(state.video.overscan_y),
        border_crop_left: Some(state.video.border_crop_left),
        border_crop_right: Some(state.video.border_crop_right),
        border_crop_top: Some(state.video.border_crop_top),
        border_crop_bottom: Some(state.video.border_crop_bottom),

        selected_crt_filter: Some(state.selected_crt_filter as u8),
        halo_brightboost: Some(state.halo.brightboost),
        halo_brightboost1: Some(state.halo.brightboost1),
        halo_beam_min: Some(state.halo.beam_min),
        halo_beam_max: Some(state.halo.beam_max),
        halo_beam_size: Some(state.halo.beam_size),
        halo_h_sharp: Some(state.halo.h_sharp),
        halo_glow: Some(state.halo.glow),
        halo_bloom: Some(state.halo.bloom),
        halo_halation: Some(state.halo.halation),
        halo_shadow_mask: Some(state.halo.shadow_mask),
        halo_masksize: Some(state.halo.masksize),
        halo_maskstr: Some(state.halo.maskstr),
        halo_mcut: Some(state.halo.mcut),
        halo_slotmask: Some(state.halo.slotmask),
        halo_slotmask1: Some(state.halo.slotmask1),
        halo_double_slot: Some(state.halo.double_slot),
        halo_smoothmask: Some(state.halo.smoothmask),
        halo_zoom: Some(state.halo.halo_zoom),
        halo_intensity: Some(state.halo.halo_intensity),
        halo_corner_size: Some(state.halo.corner_size),
        halo_curvature: Some(state.halo.curvature),

        cathode_interference_enabled: Some(state.cathode_interference.enabled),
        cathode_intensity: Some(state.cathode_interference.intensity),
        cathode_frequency: Some(state.cathode_interference.frequency),
        cathode_randomization: Some(state.cathode_interference.randomization),
        cathode_electricity_glow: Some(state.cathode_interference.electricity_glow),
        cathode_flicker_depth: Some(state.cathode_interference.flicker_depth),
        cathode_interference: Some(state.cathode_interference.interference),
        cathode_lightbulb_effect: Some(state.cathode_interference.lightbulb_effect),
    }
}

pub fn save_config(state: &AppState) {
    let Ok(path) = confy::get_configuration_file_path("michadame", None) else {
        tracing::error!("Failed to get configuration file path for save_config");
        return;
    };
    if let Err(e) = save_config_at(&path, state) {
        tracing::error!("Failed to save configuration: {}", e);
    }
}

pub fn save_config_at(path: &Path, state: &AppState) -> Result<(), confy::ConfyError> {
    if let Some(err) = &state.config_load_error {
        tracing::error!(
            "ABORTING save_config: Configuration loading failed at startup ({}). Overwriting is blocked to protect existing data.",
            err
        );
        return Err(confy::ConfyError::GeneralLoadError(std::io::Error::other(
            format!("Configuration loading failed at startup: {}", err),
        )));
    }

    let mut cfg = confy::load_path::<MichadameConfig>(path)?;

    cfg.replay = state.replay.config.clone();
    cfg.video_device = Some(state.hardware.selected_video_device.clone());
    cfg.usb_device = state.hardware.selected_usb_device.clone();
    cfg.video_resolution = if state.hardware.selected_resolution.0 > 0 {
        Some(state.hardware.selected_resolution)
    } else {
        None
    };
    cfg.video_framerate = if state.hardware.selected_framerate > 0 {
        Some(state.hardware.selected_framerate)
    } else {
        None
    };
    cfg.reset_usb_on_startup = Some(state.ui.reset_usb_on_startup);
    cfg.has_shown_first_run_warning = Some(!state.ui.show_first_run_dialog);
    cfg.audio_source = state.hardware.selected_audio_source_name.clone();
    cfg.audio_buffer_size = Some(state.hardware.audio_buffer_size);
    cfg.audio_sample_rate = Some(state.hardware.audio_sample_rate);
    cfg.audio_sample_format = Some(state.hardware.audio_sample_format.clone());
    cfg.ocr_sticky_distance = Some(state.ocr.sticky_distance);
    cfg.ocr_hide_overlay = Some(state.ocr.hide_overlay);
    cfg.ocr_timeout_seconds = Some(state.ocr.timeout_seconds);
    cfg.bank_current_tag = crate::bank::tags::normalize_tag(&state.bank.current_tag);
    cfg.bank_compact_mode = Some(state.bank.compact_mode);
    cfg.default_halo = Some(state.halo_defaults.clone());
    cfg.default_cathode_interference = Some(state.cathode_interference_defaults.clone());

    cfg.active_profile = state.active_profile.clone();
    cfg.profiles = state.profiles.clone();

    let current_profile_data = build_profile_from_state(state);
    cfg.profiles
        .insert(state.active_profile.clone(), current_profile_data);

    store_config_atomic_at(path, &cfg)
}

pub fn save_global_hardware_config(state: &AppState) {
    let Ok(path) = confy::get_configuration_file_path("michadame", None) else {
        tracing::error!("Failed to get configuration file path for save_global_hardware_config");
        return;
    };
    if let Err(e) = save_global_hardware_config_at(&path, state) {
        tracing::error!("Failed to save global hardware configuration: {}", e);
    }
}

pub fn save_global_hardware_config_at(path: &Path, state: &AppState) -> Result<(), confy::ConfyError> {
    if let Some(err) = &state.config_load_error {
        tracing::error!(
            "ABORTING save_global_hardware_config: Configuration loading failed at startup ({}). Overwriting is blocked to protect existing data.",
            err
        );
        return Err(confy::ConfyError::GeneralLoadError(std::io::Error::other(
            format!("Configuration loading failed at startup: {}", err),
        )));
    }

    let mut cfg = confy::load_path::<MichadameConfig>(path)?;

    cfg.replay = state.replay.config.clone();
    cfg.video_device = Some(state.hardware.selected_video_device.clone());
    cfg.usb_device = state.hardware.selected_usb_device.clone();
    cfg.video_resolution = if state.hardware.selected_resolution.0 > 0 {
        Some(state.hardware.selected_resolution)
    } else {
        None
    };
    cfg.video_framerate = if state.hardware.selected_framerate > 0 {
        Some(state.hardware.selected_framerate)
    } else {
        None
    };
    cfg.reset_usb_on_startup = Some(state.ui.reset_usb_on_startup);
    cfg.has_shown_first_run_warning = Some(!state.ui.show_first_run_dialog);
    cfg.audio_source = state.hardware.selected_audio_source_name.clone();
    cfg.audio_buffer_size = Some(state.hardware.audio_buffer_size);
    cfg.audio_sample_rate = Some(state.hardware.audio_sample_rate);
    cfg.audio_sample_format = Some(state.hardware.audio_sample_format.clone());
    cfg.ocr_sticky_distance = Some(state.ocr.sticky_distance);
    cfg.ocr_hide_overlay = Some(state.ocr.hide_overlay);
    cfg.ocr_timeout_seconds = Some(state.ocr.timeout_seconds);
    cfg.bank_current_tag = crate::bank::tags::normalize_tag(&state.bank.current_tag);
    cfg.bank_compact_mode = Some(state.bank.compact_mode);
    cfg.default_halo = Some(state.halo_defaults.clone());

    cfg.active_profile = state.active_profile.clone();
    cfg.profiles = state.profiles.clone();

    store_config_atomic_at(path, &cfg)
}

pub fn apply_profile_to_state(state: &mut AppState, profile: &Profile) {
    if let Some(filter) = profile.crt_filter {
        state.crt_filter.store(filter, Ordering::Relaxed);
    }
    if let Some(s) = profile.scaler_filter {
        state.scaler_filter.store(s, Ordering::Relaxed);
    }
    if let Some(c) = profile.color_range {
        state.color_range.store(c, Ordering::Relaxed);
    }
    if let Some(val) = profile.pixelate_filter_enabled {
        state.video.pixelate_filter_enabled = val;
    }
    if let Some(val) = profile.crt_hard_scan {
        state.crt.hard_scan = val;
    }
    if let Some(val) = profile.crt_hard_pix {
        state.crt.hard_pix = val;
    }
    if let Some(val) = profile.crt_brightboost {
        state.crt.brightboost = val;
    }
    if let Some(val) = profile.crt_warp_x {
        state.crt.warp_x = val;
    }
    if let Some(val) = profile.crt_warp_y {
        state.crt.warp_y = val;
    }
    if let Some(val) = profile.crt_shadow_mask {
        state.crt.shadow_mask = val;
    }
    if let Some(val) = profile.crt_hard_bloom_pix {
        state.crt.hard_bloom_pix = val;
    }
    if let Some(val) = profile.crt_hard_bloom_scan {
        state.crt.hard_bloom_scan = val;
    }
    if let Some(val) = profile.crt_bloom_amount {
        state.crt.bloom_amount = val;
    }
    if let Some(val) = profile.crt_shape {
        state.crt.shape = val;
    }
    if let Some(val) = profile.use_magenta_background {
        state.video.use_magenta_background = val;
    }
    if let Some(val) = profile.retro_pc_frame {
        state.video.retro_pc_frame = val;
    }
    if let Some(val) = profile.crt_glass_enabled {
        state.video.crt_glass_enabled = val;
    }
    if let Some(val) = profile.crt_glass_intensity {
        state.video.crt_glass_intensity = val;
    }
    if let Some(val) = profile.crt_glass_glossiness {
        state.video.crt_glass_glossiness = val;
    }
    if let Some(val) = profile.crt_glass_photographer_enabled {
        state.video.crt_glass_photographer_enabled = val;
    }
    if let Some(val) = profile.crt_glass_photographer_intensity {
        state.video.crt_glass_photographer_intensity = val;
    }
    if let Some(val) = profile.crt_glass_flash_enabled {
        state.video.crt_glass_flash_enabled = val;
    }
    if let Some(val) = profile.crt_glass_flash_intensity {
        state.video.crt_glass_flash_intensity = val;
    }
    if let Some(val) = profile.horizontal_stretch {
        state.video.horizontal_stretch = val;
    }
    if let Some(val) = profile.median_filter_enabled {
        state.video.median_filter_enabled = val;
    }
    if let Some(val) = profile.median_mix {
        state.video.median_mix = val;
    }
    if let Some(val) = profile.vibrance {
        state.video.vibrance = val;
    }
    if let Some(val) = profile.overscan_x {
        state.video.overscan_x = val;
    }
    if let Some(val) = profile.overscan_y {
        state.video.overscan_y = val;
    }
    if let Some(val) = profile.border_crop_left {
        state.video.border_crop_left = val;
    }
    if let Some(val) = profile.border_crop_right {
        state.video.border_crop_right = val;
    }
    if let Some(val) = profile.border_crop_top {
        state.video.border_crop_top = val;
    }
    if let Some(val) = profile.border_crop_bottom {
        state.video.border_crop_bottom = val;
    }
    if let Some(val) = profile.selected_crt_filter {
        state.selected_crt_filter = crate::devices::filter_type::CrtFilter::from_u8(val);
    }
    if let Some(val) = profile.halo_brightboost {
        state.halo.brightboost = val;
    }
    if let Some(val) = profile.halo_brightboost1 {
        state.halo.brightboost1 = val;
    }
    if let Some(val) = profile.halo_beam_min {
        state.halo.beam_min = val;
    }
    if let Some(val) = profile.halo_beam_max {
        state.halo.beam_max = val;
    }
    if let Some(val) = profile.halo_beam_size {
        state.halo.beam_size = val;
    }
    if let Some(val) = profile.halo_h_sharp {
        state.halo.h_sharp = val;
    }
    if let Some(val) = profile.halo_glow {
        state.halo.glow = val;
    }
    if let Some(val) = profile.halo_bloom {
        state.halo.bloom = val;
    }
    if let Some(val) = profile.halo_halation {
        state.halo.halation = val;
    }
    if let Some(val) = profile.halo_shadow_mask {
        state.halo.shadow_mask = val;
    }
    if let Some(val) = profile.halo_masksize {
        state.halo.masksize = val;
    }
    if let Some(val) = profile.halo_maskstr {
        state.halo.maskstr = val;
    }
    if let Some(val) = profile.halo_mcut {
        state.halo.mcut = val;
    }
    if let Some(val) = profile.halo_slotmask {
        state.halo.slotmask = val;
    }
    if let Some(val) = profile.halo_slotmask1 {
        state.halo.slotmask1 = val;
    }
    if let Some(val) = profile.halo_double_slot {
        state.halo.double_slot = val;
    }
    if let Some(val) = profile.halo_smoothmask {
        state.halo.smoothmask = val;
    }
    if let Some(val) = profile.halo_zoom {
        state.halo.halo_zoom = val;
    }
    if let Some(val) = profile.halo_intensity {
        state.halo.halo_intensity = val;
    }
    if let Some(val) = profile.halo_corner_size {
        state.halo.corner_size = val;
    }
    if let Some(val) = profile.halo_curvature {
        state.halo.curvature = val;
    }
    if let Some(val) = profile.cathode_interference_enabled {
        state.cathode_interference.enabled = val;
    }
    if let Some(val) = profile.cathode_intensity {
        state.cathode_interference.intensity = val;
    }
    if let Some(val) = profile.cathode_frequency {
        state.cathode_interference.frequency = val;
    }
    if let Some(val) = profile.cathode_randomization {
        state.cathode_interference.randomization = val;
    }
    if let Some(val) = profile.cathode_electricity_glow {
        state.cathode_interference.electricity_glow = val;
    }
    if let Some(val) = profile.cathode_flicker_depth {
        state.cathode_interference.flicker_depth = val;
    }
    if let Some(val) = profile.cathode_interference {
        state.cathode_interference.interference = val;
    }
    if let Some(val) = profile.cathode_lightbulb_effect {
        state.cathode_interference.lightbulb_effect = val;
    }
}

pub fn apply_config(state: &mut AppState, cfg: &MichadameConfig) {
    state.replay.config = cfg.replay.clone();
    state.profiles = cfg.profiles.clone();
    state.active_profile = cfg.active_profile.clone();

    if let Some(saved_device) = &cfg.video_device {
        if state.hardware.video_devices.contains(saved_device) {
            state.hardware.selected_video_device = saved_device.clone();
        }
    }
    if let Some(saved_usb) = &cfg.usb_device {
        if state
            .hardware
            .usb_devices
            .iter()
            .any(|(id, _)| id == saved_usb)
        {
            state.hardware.selected_usb_device = Some(saved_usb.clone());
        }
    }
    if let Some(saved_source) = &cfg.audio_source {
        if state
            .hardware
            .audio_sources
            .iter()
            .any(|(_, name)| name == saved_source)
        {
            state.hardware.selected_audio_source_name = Some(saved_source.clone());
        }
    }
    state.hardware.audio_buffer_size = cfg.audio_buffer_size.unwrap_or(1024);
    state.hardware.audio_sample_rate = cfg.audio_sample_rate.unwrap_or(48000);
    state.hardware.audio_sample_format = cfg
        .audio_sample_format
        .clone()
        .unwrap_or_else(|| "S16LE".to_string());

    if !state.hardware.selected_video_device.is_empty() {
        crate::video::types::apply_saved_format_config(state, cfg);
    }
    state.ui.reset_usb_on_startup = cfg.reset_usb_on_startup.unwrap_or(false);
    if state.ui.reset_usb_on_startup {
        if let Some(device_to_reset) = &state.hardware.selected_usb_device {
            let msg = match crate::devices::usb::reset_usb_device(device_to_reset) {
                Ok(_) => "Auto-reset USB device successfully.".to_string(),
                Err(e) => format!("Failed to auto-reset USB: {}", e),
            };
            state.info(msg);
            tracing::info!("USB device reset on startup as requested.");
        }
    }
    if !cfg.has_shown_first_run_warning.unwrap_or(false) {
        state.ui.show_first_run_dialog = true;
    }
    state.ocr.sticky_distance = cfg.ocr_sticky_distance.unwrap_or(0.6);
    state.ocr.hide_overlay = cfg.ocr_hide_overlay.unwrap_or(false);
    state.ocr.timeout_seconds = cfg.ocr_timeout_seconds.unwrap_or(45);
    state.bank.current_tag = cfg.bank_current_tag.clone().unwrap_or_default();
    state.bank.saved_current_tag = state.bank.current_tag.clone();
    state.bank.compact_mode = cfg.bank_compact_mode.unwrap_or(false);
    if let Some(defaults) = &cfg.default_halo {
        state.halo_defaults = defaults.clone();
    }
    if let Some(defaults) = &cfg.default_cathode_interference {
        state.cathode_interference_defaults = defaults.clone();
    }

    state.active_profile = cfg.active_profile.clone();

    let profile_to_apply = state.profiles.get(&state.active_profile).cloned();
    if let Some(profile) = profile_to_apply {
        apply_profile_to_state(state, &profile);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::models::AppState;
    use crate::config::models::{MichadameConfig, Profile};

    #[test]
    fn replay_settings_survive_disk_reload_without_changing_hardware_or_profiles() {
        let path = std::env::temp_dir().join(format!(
            "michadame-replay-config-{}-{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));
        let mut existing = MichadameConfig {
            audio_buffer_size: Some(4096),
            audio_sample_rate: Some(44100),
            ..Default::default()
        };
        existing.profiles.insert(
            "Saved CRT".into(),
            Profile {
                crt_hard_scan: Some(-12.),
                ..Default::default()
            },
        );
        confy::store_path(&path, &existing).unwrap();
        let settings = crate::replay::config::ReplayConfig {
            history_seconds: 600,
            memory_mib: 2048,
            work_queue_mib: 768,
            custom_seconds: 45,
            keys: [1, 2, 3, 4, 5, 10],
            capture_overlays: true,
            directory: "/tmp/my replays".into(),
            render_device: "/dev/dri/renderD129".into(),
        };
        save_replay_config_at(&path, &settings).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        existing.replay = settings.clone();
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            serde_json::to_value(&existing).unwrap()
        );
        // Applying settings with no enumerated devices cannot access hardware.
        let mut restarted = AppState::default();
        apply_config(&mut restarted, &loaded);
        assert_eq!(
            serde_json::to_value(&restarted.replay.config).unwrap(),
            serde_json::to_value(&settings).unwrap()
        );
        assert!(restarted.replay.runtime.is_none());

        // A malformed existing file must be reported, not overwritten with defaults.
        std::fs::write(&path, "invalid = [").unwrap();
        assert!(save_replay_config_at(&path, &settings).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid = [");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_build_profile_from_state() {
        let mut state = AppState::default();
        state.crt.hard_scan = -12.0;
        state.halo.brightboost = 2.1;
        state.selected_crt_filter = crate::devices::filter_type::CrtFilter::Halo;
        state.video.pixelate_filter_enabled = true;
        state.cathode_interference.enabled = true;
        state.cathode_interference.intensity = 0.75;
        state.crt_filter.store(
            crate::devices::filter_type::CrtFilter::Lottes as u8,
            Ordering::Relaxed,
        );

        let profile = build_profile_from_state(&state);
        assert_eq!(profile.crt_hard_scan, Some(-12.0));
        assert_eq!(profile.halo_brightboost, Some(2.1));
        assert_eq!(profile.cathode_interference_enabled, Some(true));
        assert_eq!(profile.cathode_intensity, Some(0.75));
        assert_eq!(profile.cathode_lightbulb_effect, Some(0.35));
        assert_eq!(
            profile.selected_crt_filter,
            Some(crate::devices::filter_type::CrtFilter::Halo as u8)
        );
        assert_eq!(profile.pixelate_filter_enabled, Some(true));
        assert_eq!(
            profile.crt_filter,
            Some(crate::devices::filter_type::CrtFilter::Lottes as u8)
        );
    }

    #[test]
    fn test_apply_profile_to_state() {
        let mut state = AppState::default();
        let profile = Profile {
            crt_hard_scan: Some(-15.0),
            halo_brightboost: Some(2.5),
            selected_crt_filter: Some(crate::devices::filter_type::CrtFilter::Halo as u8),
            pixelate_filter_enabled: Some(true),
            cathode_interference_enabled: Some(true),
            cathode_intensity: Some(0.9),
            cathode_frequency: Some(2.0),
            cathode_lightbulb_effect: Some(0.65),
            crt_filter: Some(crate::devices::filter_type::CrtFilter::Lottes as u8),
            ..Default::default()
        };

        apply_profile_to_state(&mut state, &profile);
        assert_eq!(state.crt.hard_scan, -15.0);
        assert_eq!(state.halo.brightboost, 2.5);
        assert!(state.cathode_interference.enabled);
        assert_eq!(state.cathode_interference.intensity, 0.9);
        assert_eq!(state.cathode_interference.frequency, 2.0);
        assert_eq!(state.cathode_interference.lightbulb_effect, 0.65);
        assert_eq!(state.selected_crt_filter, crate::devices::filter_type::CrtFilter::Halo);
        assert!(state.video.pixelate_filter_enabled);
        assert_eq!(
            state.crt_filter.load(Ordering::Relaxed),
            crate::devices::filter_type::CrtFilter::Lottes as u8
        );
    }

    #[test]
    fn test_apply_config_hardware_settings() {
        let mut state = AppState::default();
        state.hardware.video_devices = vec!["/dev/video0".to_string()];
        state.hardware.audio_sources = vec![("id".to_string(), "Mic".to_string())];

        let cfg = MichadameConfig {
            video_device: Some("/dev/video0".to_string()),
            audio_source: Some("Mic".to_string()),
            audio_buffer_size: Some(2048),
            ..Default::default()
        };

        apply_config(&mut state, &cfg);
        assert_eq!(state.hardware.selected_video_device, "/dev/video0");
        assert_eq!(
            state.hardware.selected_audio_source_name,
            Some("Mic".to_string())
        );
        assert_eq!(state.hardware.audio_buffer_size, 2048);
    }

    #[test]
    fn test_apply_config_with_missing_hardware() {
        let mut state = AppState::default();
        state.hardware.video_devices = vec![]; // No devices found during scan

        let cfg = MichadameConfig {
            video_device: Some("/dev/video0".to_string()), // Saved device not present
            ..Default::default()
        };

        apply_config(&mut state, &cfg);
        // Should NOT update if not in list
        assert_eq!(state.hardware.selected_video_device, "");
    }

    #[test]
    fn test_apply_config_ocr_sticky_distance() {
        let mut state = AppState::default();
        assert_eq!(state.ocr.sticky_distance, 0.6);

        let cfg = MichadameConfig {
            ocr_sticky_distance: Some(1.25),
            ..Default::default()
        };
        apply_config(&mut state, &cfg);
        assert!((state.ocr.sticky_distance - 1.25).abs() < f32::EPSILON);
    }

    #[test]
    fn test_ocr_sticky_distance_toml_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "michadame-ocr-config-{}-{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));
        let cfg = MichadameConfig {
            ocr_sticky_distance: Some(0.85),
            ..Default::default()
        };
        confy::store_path(&path, &cfg).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(loaded.ocr_sticky_distance, Some(0.85));

        let mut state = AppState::default();
        apply_config(&mut state, &loaded);
        assert!((state.ocr.sticky_distance - 0.85).abs() < f32::EPSILON);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_ocr_hide_overlay_toml_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "michadame-ocr-hide-config-{}-{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));
        let cfg = MichadameConfig {
            ocr_hide_overlay: Some(true),
            ..Default::default()
        };
        confy::store_path(&path, &cfg).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(loaded.ocr_hide_overlay, Some(true));

        let mut state = AppState::default();
        apply_config(&mut state, &loaded);
        assert!(state.ocr.hide_overlay);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_apply_config_ocr_timeout_seconds() {
        let mut state = AppState::default();
        assert_eq!(state.ocr.timeout_seconds, 45);

        let cfg = MichadameConfig {
            ocr_timeout_seconds: Some(60),
            ..Default::default()
        };
        apply_config(&mut state, &cfg);
        assert_eq!(state.ocr.timeout_seconds, 60);

        // Test disable (0)
        let cfg_disabled = MichadameConfig {
            ocr_timeout_seconds: Some(0),
            ..Default::default()
        };
        apply_config(&mut state, &cfg_disabled);
        assert_eq!(state.ocr.timeout_seconds, 0);
    }

    #[test]
    fn test_bank_current_tag_toml_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "michadame-bank-tag-config-{}-{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));
        let cfg = MichadameConfig {
            bank_current_tag: Some("Final Fantasy 7".into()),
            default_halo: Some(Default::default()),
            ..Default::default()
        };
        confy::store_path(&path, &cfg).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(loaded.bank_current_tag.as_deref(), Some("Final Fantasy 7"));

        let mut state = AppState::default();
        apply_config(&mut state, &loaded);
        assert_eq!(state.bank.current_tag, "Final Fantasy 7");

        // Saving from state writes the (trimmed) tag back; an empty tag is omitted.
        state.bank.current_tag = "  Grandia 2 ".into();
        save_config_at(&path, &state).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(loaded.bank_current_tag.as_deref(), Some("Grandia 2"));
        state.bank.current_tag.clear();
        save_config_at(&path, &state).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(loaded.bank_current_tag, None);

        // Older configs without the field load with no tag.
        let mut state = AppState::default();
        apply_config(&mut state, &MichadameConfig::default());
        assert_eq!(state.bank.current_tag, "");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("toml.bak"));
    }

    #[test]
    fn test_bank_compact_mode_toml_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "michadame-bank-compact-config-{}-{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));
        let cfg = MichadameConfig {
            bank_compact_mode: Some(true),
            default_halo: Some(Default::default()),
            ..Default::default()
        };
        confy::store_path(&path, &cfg).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(loaded.bank_compact_mode, Some(true));

        let mut state = AppState::default();
        apply_config(&mut state, &loaded);
        assert!(state.bank.compact_mode);

        state.bank.compact_mode = false;
        save_config_at(&path, &state).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(loaded.bank_compact_mode, Some(false));

        let mut state = AppState::default();
        apply_config(&mut state, &loaded);
        assert!(!state.bank.compact_mode);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("toml.bak"));
    }

    #[test]
    fn test_ocr_timeout_seconds_toml_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "michadame-ocr-timeout-config-{}-{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));
        let cfg = MichadameConfig {
            ocr_timeout_seconds: Some(30),
            ..Default::default()
        };
        confy::store_path(&path, &cfg).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(loaded.ocr_timeout_seconds, Some(30));

        let mut state = AppState::default();
        apply_config(&mut state, &loaded);
        assert_eq!(state.ocr.timeout_seconds, 30);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_apply_profile_halo_settings() {
        let mut state = AppState::default();
        let mut profile = Profile::default();
        profile.selected_crt_filter = Some(crate::devices::filter_type::CrtFilter::Halo as u8);
        profile.halo_brightboost = Some(2.2);
        profile.halo_zoom = Some(75.0);
        profile.halo_curvature = Some(false);

        apply_profile_to_state(&mut state, &profile);
        assert_eq!(
            state.selected_crt_filter,
            crate::devices::filter_type::CrtFilter::Halo
        );
        assert_eq!(state.halo.brightboost, 2.2);
        assert_eq!(state.halo.halo_zoom, 75.0);
        assert_eq!(state.halo.curvature, false);
    }

    #[test]
    fn test_border_crop_profile_roundtrip() {
        let mut state = AppState::default();
        state.video.border_crop_left = 0.05;
        state.video.border_crop_right = 0.06;
        state.video.border_crop_top = 0.07;
        state.video.border_crop_bottom = 0.08;

        let profile = build_profile_from_state(&state);
        assert_eq!(profile.border_crop_left, Some(0.05));
        assert_eq!(profile.border_crop_right, Some(0.06));
        assert_eq!(profile.border_crop_top, Some(0.07));
        assert_eq!(profile.border_crop_bottom, Some(0.08));

        let mut new_state = AppState::default();
        apply_profile_to_state(&mut new_state, &profile);
        assert_eq!(new_state.video.border_crop_left, 0.05);
        assert_eq!(new_state.video.border_crop_right, 0.06);
        assert_eq!(new_state.video.border_crop_top, 0.07);
        assert_eq!(new_state.video.border_crop_bottom, 0.08);
    }

    #[test]
    fn test_retro_pc_frame_profile_roundtrip() {
        let mut state = AppState::default();
        assert!(!state.video.retro_pc_frame);

        state.video.retro_pc_frame = true;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.retro_pc_frame, Some(true));

        let mut new_state = AppState::default();
        apply_profile_to_state(&mut new_state, &profile);
        assert!(new_state.video.retro_pc_frame);

        state.video.retro_pc_frame = false;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.retro_pc_frame, Some(false));

        apply_profile_to_state(&mut new_state, &profile);
        assert!(!new_state.video.retro_pc_frame);
    }

    #[test]
    fn test_crt_glass_profile_roundtrip() {
        let mut state = AppState::default();
        assert!(!state.video.crt_glass_enabled);
        assert_eq!(state.video.crt_glass_intensity, 0.25);
        assert_eq!(state.video.crt_glass_glossiness, 0.50);
        assert!(!state.video.crt_glass_photographer_enabled);
        assert_eq!(state.video.crt_glass_photographer_intensity, 0.50);
        assert!(!state.video.crt_glass_flash_enabled);
        assert_eq!(state.video.crt_glass_flash_intensity, 0.70);

        state.video.crt_glass_enabled = true;
        state.video.crt_glass_intensity = 0.65;
        state.video.crt_glass_glossiness = 0.85;
        state.video.crt_glass_photographer_enabled = true;
        state.video.crt_glass_photographer_intensity = 0.80;
        state.video.crt_glass_flash_enabled = true;
        state.video.crt_glass_flash_intensity = 0.90;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.crt_glass_enabled, Some(true));
        assert_eq!(profile.crt_glass_intensity, Some(0.65));
        assert_eq!(profile.crt_glass_glossiness, Some(0.85));
        assert_eq!(profile.crt_glass_photographer_enabled, Some(true));
        assert_eq!(profile.crt_glass_photographer_intensity, Some(0.80));
        assert_eq!(profile.crt_glass_flash_enabled, Some(true));
        assert_eq!(profile.crt_glass_flash_intensity, Some(0.90));

        let mut new_state = AppState::default();
        apply_profile_to_state(&mut new_state, &profile);
        assert!(new_state.video.crt_glass_enabled);
        assert_eq!(new_state.video.crt_glass_intensity, 0.65);
        assert_eq!(new_state.video.crt_glass_glossiness, 0.85);
        assert!(new_state.video.crt_glass_photographer_enabled);
        assert_eq!(new_state.video.crt_glass_photographer_intensity, 0.80);
        assert!(new_state.video.crt_glass_flash_enabled);
        assert_eq!(new_state.video.crt_glass_flash_intensity, 0.90);

        state.video.crt_glass_enabled = false;
        state.video.crt_glass_intensity = 0.10;
        state.video.crt_glass_glossiness = 0.20;
        state.video.crt_glass_photographer_enabled = false;
        state.video.crt_glass_photographer_intensity = 0.30;
        state.video.crt_glass_flash_enabled = false;
        state.video.crt_glass_flash_intensity = 0.40;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.crt_glass_enabled, Some(false));
        assert_eq!(profile.crt_glass_intensity, Some(0.10));
        assert_eq!(profile.crt_glass_glossiness, Some(0.20));
        assert_eq!(profile.crt_glass_photographer_enabled, Some(false));
        assert_eq!(profile.crt_glass_photographer_intensity, Some(0.30));
        assert_eq!(profile.crt_glass_flash_enabled, Some(false));
        assert_eq!(profile.crt_glass_flash_intensity, Some(0.40));

        apply_profile_to_state(&mut new_state, &profile);
        assert!(!new_state.video.crt_glass_enabled);
        assert_eq!(new_state.video.crt_glass_intensity, 0.10);
        assert_eq!(new_state.video.crt_glass_glossiness, 0.20);
        assert!(!new_state.video.crt_glass_photographer_enabled);
        assert_eq!(new_state.video.crt_glass_photographer_intensity, 0.30);
        assert!(!new_state.video.crt_glass_flash_enabled);
        assert_eq!(new_state.video.crt_glass_flash_intensity, 0.40);
    }

    #[test]
    fn test_replay_capture_overlays_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "michadame-replay-overlay-{}-{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));
        let mut cfg = MichadameConfig::default();
        assert!(!cfg.replay.capture_overlays);
        cfg.replay.capture_overlays = true;

        confy::store_path(&path, &cfg).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert!(loaded.replay.capture_overlays);

        let mut state = AppState::default();
        apply_config(&mut state, &loaded);
        assert!(state.replay.config.capture_overlays);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_default_halo_toml_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "michadame-halo-defaults-{}-{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));
        let mut custom_defaults = crate::app::models::HaloSettings::default();
        custom_defaults.brightboost = 2.45;
        custom_defaults.h_sharp = 4.8;
        custom_defaults.curvature = false;

        let cfg = MichadameConfig {
            default_halo: Some(custom_defaults.clone()),
            ..Default::default()
        };
        confy::store_path(&path, &cfg).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(loaded.default_halo, Some(custom_defaults.clone()));

        let mut state = AppState::default();
        apply_config(&mut state, &loaded);
        assert_eq!(state.halo_defaults, custom_defaults);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_save_aborted_when_config_load_error_set() {
        let dir = std::env::temp_dir().join(format!("michadame-test-abort-{}", crate::replay::now_us()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test-config.toml");
        std::fs::write(&path, "original_content = true\n").unwrap();

        let mut state = AppState::default();
        state.config_load_error = Some("Failed to parse TOML".to_string());

        let result = save_config_at(&path, &state);
        assert!(result.is_err(), "save_config_at must return an Err when config_load_error is set");

        // Verify the original file on disk was NOT touched or overwritten
        let current_content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(current_content, "original_content = true\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_aborted_when_existing_file_corrupt() {
        let dir = std::env::temp_dir().join(format!("michadame-test-corrupt-{}", crate::replay::now_us()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test-config.toml");
        std::fs::write(&path, "invalid = [[[\n").unwrap();

        let state = AppState::default();
        let result = save_config_at(&path, &state);
        assert!(result.is_err(), "save_config_at must refuse to overwrite corrupt config on disk");

        let current_content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(current_content, "invalid = [[[\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_quarantine_corrupted_config() {
        let dir = std::env::temp_dir().join(format!("michadame-test-quarantine-{}", crate::replay::now_us()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test-config.toml");
        let corrupted_bytes = b"bad toml content = { [ unclosed";
        std::fs::write(&path, corrupted_bytes).unwrap();

        let quarantined = quarantine_corrupted_config(&path);
        assert!(quarantined.is_some(), "quarantine_corrupted_config should return a quarantine path");
        let q_path = quarantined.unwrap();
        assert!(q_path.exists(), "quarantine file must exist on disk");
        assert_eq!(std::fs::read(&q_path).unwrap(), corrupted_bytes);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rotating_backup_and_pruning() {
        let dir = std::env::temp_dir().join(format!("michadame-test-backup-{}", crate::replay::now_us()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("default-config.toml");
        std::fs::write(&path, "test = 1\n").unwrap();

        create_rotating_backup(&path);
        let bak_path = dir.join("default-config.toml.bak");
        assert!(bak_path.exists(), ".bak backup should be created");
        assert_eq!(std::fs::read_to_string(&bak_path).unwrap(), "test = 1\n");

        let backups_dir = dir.join("backups");
        assert!(backups_dir.exists(), "backups/ dir should be created");

        // Create 25 dummy backup files in backups_dir
        for i in 0..25 {
            let f = backups_dir.join(format!("config-{}.toml", 1000 + i));
            std::fs::write(&f, format!("backup {}", i)).unwrap();
        }

        prune_old_backups(&backups_dir, 5);

        let count = std::fs::read_dir(&backups_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with("config-") && name.ends_with(".toml")
            })
            .count();
        assert_eq!(count, 5, "prune_old_backups should reduce count to max_keep");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_legacy_config_missing_fields_deserialization() {
        let dir = std::env::temp_dir().join(format!("michadame-test-legacy-{}", crate::replay::now_us()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test-config.toml");
        let old_toml = r#"
active_profile = "Saturn"

[profiles.Saturn]
crt_hard_scan = -8.0
crt_brightboost = 1.4

[profiles.Dreamcast]
crt_hard_scan = -10.0
crt_brightboost = 1.6
"#;
        std::fs::write(&path, old_toml).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).expect("Old TOML should deserialize cleanly");
        assert_eq!(loaded.active_profile, "Saturn");
        assert!(loaded.profiles.contains_key("Saturn"));
        assert!(loaded.profiles.contains_key("Dreamcast"));

        let saturn = loaded.profiles.get("Saturn").unwrap();
        assert_eq!(saturn.crt_hard_scan, Some(-8.0));
        assert_eq!(saturn.crt_brightboost, Some(1.4));
        assert_eq!(saturn.cathode_lightbulb_effect, None);
        assert_eq!(saturn.cathode_interference_enabled, None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
