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
        retro_pc_frame_dark_mode: Some(state.video.retro_pc_frame_dark_mode),
        retro_pc_ambient_glow: Some(state.video.retro_pc_ambient_glow),
        retro_software_mouse: Some(state.video.retro_software_mouse),
        lights_off_night_mode: Some(state.video.lights_off_night_mode),
        night_mode_glow_intensity: Some(state.video.night_mode_glow_intensity),
        crt_glass_enabled: Some(state.video.crt_glass_enabled),
        crt_glass_intensity: Some(state.video.crt_glass_intensity),
        crt_glass_glossiness: Some(state.video.crt_glass_glossiness),
        crt_glass_ceiling_light_enabled: Some(state.video.crt_glass_ceiling_light_enabled),
        crt_glass_photographer_enabled: Some(state.video.crt_glass_photographer_enabled),
        crt_glass_photographer_intensity: Some(state.video.crt_glass_photographer_intensity),
        crt_glass_flash_enabled: Some(state.video.crt_glass_flash_enabled),
        crt_glass_flash_intensity: Some(state.video.crt_glass_flash_intensity),
        horizontal_stretch: Some(state.video.horizontal_stretch),
        median_filter_enabled: Some(state.video.median_filter_enabled),
        median_mix: Some(state.video.median_mix),
        deinterlace_filter_enabled: Some(state.video.deinterlace_filter_enabled),
        deinterlace_mode: Some(state.video.deinterlace_mode),
        deinterlace_blend: Some(state.video.deinterlace_blend),
        deinterlace_motion_threshold: Some(state.video.deinterlace_motion_threshold),
        deinterlace_line_spacing: Some(state.video.deinterlace_line_spacing),
        deinterlace_spatial_mix: Some(state.video.deinterlace_spatial_mix),
        vibrance: Some(state.video.vibrance),
        overscan_x: Some(state.video.overscan_x),
        overscan_y: Some(state.video.overscan_y),
        underscan_x: Some(state.video.underscan_x),
        underscan_y: Some(state.video.underscan_y),
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

        video_resolution: if state.hardware.selected_resolution.0 > 0 {
            Some(state.hardware.selected_resolution)
        } else {
            None
        },
        video_framerate: if state.hardware.selected_framerate > 0 {
            Some(state.hardware.selected_framerate)
        } else {
            None
        },

        fft_filter_enabled: Some(state.video.fft_filter_enabled),
        fft_mask_save_name: if state.fft_mask_save_name.is_empty() {
            None
        } else {
            Some(state.fft_mask_save_name.clone())
        },

        audio_source: state.hardware.selected_audio_source_name.clone(),
        audio_buffer_size: Some(state.hardware.audio_buffer_size),
        audio_sample_rate: Some(state.hardware.audio_sample_rate),
        audio_sample_format: Some(state.hardware.audio_sample_format.clone()),

        audio_filter: Some(state.replay.config.audio_filter.clone()),
        popup_under_crt: Some(state.dict.popup_under_crt),
    }
}

pub fn save_config(state: &mut AppState) {
    let Ok(path) = confy::get_configuration_file_path("michadame", None) else {
        tracing::error!("Failed to get configuration file path for save_config");
        return;
    };
    if let Err(e) = save_config_at(&path, state) {
        tracing::error!("Failed to save configuration: {}", e);
    }
}

pub fn save_config_at(path: &Path, state: &mut AppState) -> Result<(), confy::ConfyError> {
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
    cfg.popup_under_crt = Some(state.dict.popup_under_crt);
    cfg.twitch = state.twitch.config.clone();

    let current_profile_data = build_profile_from_state(state);
    state
        .profiles
        .insert(state.active_profile.clone(), current_profile_data);

    cfg.active_profile = state.active_profile.clone();
    cfg.profiles = state.profiles.clone();

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
    cfg.twitch = state.twitch.config.clone();

    cfg.active_profile = state.active_profile.clone();
    cfg.profiles = state.profiles.clone();

    store_config_atomic_at(path, &cfg)
}

pub fn apply_profile_to_state(state: &mut AppState, profile: &Profile) -> bool {
    state
        .crt_filter
        .store(profile.crt_filter.unwrap_or(0), Ordering::Relaxed);
    state.selected_crt_filter =
        crate::devices::filter_type::CrtFilter::from_u8(profile.selected_crt_filter.unwrap_or(1));
    state.scaler_filter.store(
        profile
            .scaler_filter
            .unwrap_or(crate::video::types::ScalerFilter::Bicubic as u8),
        Ordering::Relaxed,
    );
    state.color_range.store(
        profile
            .color_range
            .unwrap_or(crate::video::types::ColorRange::Full as u8),
        Ordering::Relaxed,
    );
    state.video.pixelate_filter_enabled = profile.pixelate_filter_enabled.unwrap_or(false);

    state.crt.hard_scan = profile.crt_hard_scan.unwrap_or(-8.0);
    state.crt.hard_pix = profile.crt_hard_pix.unwrap_or(-3.0);
    state.crt.brightboost = profile.crt_brightboost.unwrap_or(1.0);
    state.crt.warp_x = profile.crt_warp_x.unwrap_or(0.031);
    state.crt.warp_y = profile.crt_warp_y.unwrap_or(0.041);
    state.crt.shadow_mask = profile.crt_shadow_mask.unwrap_or(3.0);
    state.crt.hard_bloom_pix = profile.crt_hard_bloom_pix.unwrap_or(-1.5);
    state.crt.hard_bloom_scan = profile.crt_hard_bloom_scan.unwrap_or(-2.0);
    state.crt.bloom_amount = profile.crt_bloom_amount.unwrap_or(0.15);
    state.crt.shape = profile.crt_shape.unwrap_or(2.0);

    state.video.use_magenta_background = profile.use_magenta_background.unwrap_or(false);
    state.video.retro_pc_frame = profile.retro_pc_frame.unwrap_or(false);
    state.video.retro_pc_frame_dark_mode = profile.retro_pc_frame_dark_mode.unwrap_or(false);
    state.video.retro_pc_ambient_glow = profile.retro_pc_ambient_glow.unwrap_or(0.55);
    state.video.retro_software_mouse = profile.retro_software_mouse.unwrap_or(false);
    state.video.lights_off_night_mode = profile.lights_off_night_mode.unwrap_or(false);
    state.video.night_mode_glow_intensity = profile.night_mode_glow_intensity.unwrap_or(0.0);

    state.video.crt_glass_enabled = profile.crt_glass_enabled.unwrap_or(false);
    state.video.crt_glass_intensity = profile.crt_glass_intensity.unwrap_or(0.25);
    state.video.crt_glass_glossiness = profile.crt_glass_glossiness.unwrap_or(0.50);
    state.video.crt_glass_ceiling_light_enabled =
        profile.crt_glass_ceiling_light_enabled.unwrap_or(true);
    state.video.crt_glass_photographer_enabled =
        profile.crt_glass_photographer_enabled.unwrap_or(false);
    state.video.crt_glass_photographer_intensity =
        profile.crt_glass_photographer_intensity.unwrap_or(0.50);
    state.video.crt_glass_flash_enabled = profile.crt_glass_flash_enabled.unwrap_or(false);
    state.video.crt_glass_flash_intensity = profile.crt_glass_flash_intensity.unwrap_or(0.70);

    state.video.horizontal_stretch = profile.horizontal_stretch.unwrap_or(1.0);
    state.video.median_filter_enabled = profile.median_filter_enabled.unwrap_or(false);
    state.video.median_mix = profile.median_mix.unwrap_or(1.0);
    state.video.deinterlace_filter_enabled = profile.deinterlace_filter_enabled.unwrap_or(false);
    state.video.deinterlace_mode = profile.deinterlace_mode.unwrap_or(0);
    state.video.deinterlace_blend = profile.deinterlace_blend.unwrap_or(0.5);
    state.video.deinterlace_motion_threshold = profile.deinterlace_motion_threshold.unwrap_or(0.08);
    state.video.deinterlace_line_spacing = profile.deinterlace_line_spacing.unwrap_or(1.0);
    state.video.deinterlace_spatial_mix = profile.deinterlace_spatial_mix.unwrap_or(0.75);
    state.video.vibrance = profile.vibrance.unwrap_or(1.0);
    state.video.overscan_x = profile.overscan_x.unwrap_or(0.0);
    state.video.overscan_y = profile.overscan_y.unwrap_or(0.0);
    state.video.underscan_x = profile.underscan_x.unwrap_or(0.0);
    state.video.underscan_y = profile.underscan_y.unwrap_or(0.0);
    state.video.border_crop_left = profile.border_crop_left.unwrap_or(0.0);
    state.video.border_crop_right = profile.border_crop_right.unwrap_or(0.0);
    state.video.border_crop_top = profile.border_crop_top.unwrap_or(0.0);
    state.video.border_crop_bottom = profile.border_crop_bottom.unwrap_or(0.0);

    let def_halo = state.halo_defaults.clone();
    state.halo.brightboost = profile.halo_brightboost.unwrap_or(def_halo.brightboost);
    state.halo.brightboost1 = profile.halo_brightboost1.unwrap_or(def_halo.brightboost1);
    state.halo.beam_min = profile.halo_beam_min.unwrap_or(def_halo.beam_min);
    state.halo.beam_max = profile.halo_beam_max.unwrap_or(def_halo.beam_max);
    state.halo.beam_size = profile.halo_beam_size.unwrap_or(def_halo.beam_size);
    state.halo.h_sharp = profile.halo_h_sharp.unwrap_or(def_halo.h_sharp);
    state.halo.glow = profile.halo_glow.unwrap_or(def_halo.glow);
    state.halo.bloom = profile.halo_bloom.unwrap_or(def_halo.bloom);
    state.halo.halation = profile.halo_halation.unwrap_or(def_halo.halation);
    state.halo.shadow_mask = profile.halo_shadow_mask.unwrap_or(def_halo.shadow_mask);
    state.halo.masksize = profile.halo_masksize.unwrap_or(def_halo.masksize);
    state.halo.maskstr = profile.halo_maskstr.unwrap_or(def_halo.maskstr);
    state.halo.mcut = profile.halo_mcut.unwrap_or(def_halo.mcut);
    state.halo.slotmask = profile.halo_slotmask.unwrap_or(def_halo.slotmask);
    state.halo.slotmask1 = profile.halo_slotmask1.unwrap_or(def_halo.slotmask1);
    state.halo.double_slot = profile.halo_double_slot.unwrap_or(def_halo.double_slot);
    state.halo.smoothmask = profile.halo_smoothmask.unwrap_or(def_halo.smoothmask);
    state.halo.halo_zoom = profile.halo_zoom.unwrap_or(def_halo.halo_zoom);
    state.halo.halo_intensity = profile.halo_intensity.unwrap_or(def_halo.halo_intensity);
    state.halo.corner_size = profile.halo_corner_size.unwrap_or(def_halo.corner_size);
    state.halo.curvature = profile.halo_curvature.unwrap_or(def_halo.curvature);

    let def_cathode = state.cathode_interference_defaults.clone();
    state.cathode_interference.enabled = profile
        .cathode_interference_enabled
        .unwrap_or(def_cathode.enabled);
    state.cathode_interference.intensity =
        profile.cathode_intensity.unwrap_or(def_cathode.intensity);
    state.cathode_interference.frequency =
        profile.cathode_frequency.unwrap_or(def_cathode.frequency);
    state.cathode_interference.randomization = profile
        .cathode_randomization
        .unwrap_or(def_cathode.randomization);
    state.cathode_interference.electricity_glow = profile
        .cathode_electricity_glow
        .unwrap_or(def_cathode.electricity_glow);
    state.cathode_interference.flicker_depth = profile
        .cathode_flicker_depth
        .unwrap_or(def_cathode.flicker_depth);
    state.cathode_interference.interference = profile
        .cathode_interference
        .unwrap_or(def_cathode.interference);
    state.cathode_interference.lightbulb_effect = profile
        .cathode_lightbulb_effect
        .unwrap_or(def_cathode.lightbulb_effect);

    // Video format / resolution / framerate
    if !state.hardware.supported_formats.is_empty() {
        if let Some(fourcc) = &profile.video_format_fourcc {
            if let Some(idx) = state
                .hardware
                .supported_formats
                .iter()
                .position(|f| f.fourcc == *fourcc)
            {
                state.hardware.selected_format_index = idx;
                if let Some(res) = profile.video_resolution {
                    if state.hardware.supported_formats[idx]
                        .resolutions
                        .iter()
                        .any(|r| (r.width, r.height) == res)
                    {
                        state.hardware.selected_resolution = res;
                        if let Some(fps) = profile.video_framerate {
                            if let Some(res_info) = state.hardware.supported_formats[idx]
                                .resolutions
                                .iter()
                                .find(|r| (r.width, r.height) == res)
                            {
                                if res_info.framerates.contains(&fps) {
                                    state.hardware.selected_framerate = fps;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // FFT mask filter
    state.video.fft_filter_enabled = profile.fft_filter_enabled.unwrap_or(false);
    if let Some(mask_name) = &profile.fft_mask_save_name {
        state.fft_mask_save_name = mask_name.clone();
        if state.video.fft_filter_enabled && !mask_name.is_empty() {
            let stream_res = state
                .latest_frame
                .as_ref()
                .map(|f| (f.width, f.height))
                .unwrap_or(state.hardware.selected_resolution);
            if stream_res.0 > 0 && stream_res.1 > 0 {
                if let Ok((data, fft_res, mask_thresh, black_thresh)) =
                    crate::config::fft_masks::load_mask(mask_name, stream_res)
                {
                    state.fft_mask_data = data;
                    state.fft_mask_resolution = fft_res;
                    state.fft_mask_threshold = mask_thresh;
                    state.fft_black_threshold = black_thresh;
                    state.fft_mask_dirty = true;
                }
            }
        }
    } else {
        state.fft_mask_save_name.clear();
    }

    // Audio filter
    if let Some(ref filter) = profile.audio_filter {
        state.replay.config.audio_filter = filter.clone();
        state.replay.sync_audio_filter();
    }

    // Audio device settings
    let mut audio_hardware_changed = false;
    if let Some(ref source) = profile.audio_source {
        if state.hardware.selected_audio_source_name.as_ref() != Some(source) {
            if state.hardware.audio_sources.iter().any(|(_, name)| name == source) {
                state.hardware.selected_audio_source_name = Some(source.clone());
                audio_hardware_changed = true;
            }
        }
    }
    if let Some(buf) = profile.audio_buffer_size {
        if state.hardware.audio_buffer_size != buf {
            state.hardware.audio_buffer_size = buf;
            audio_hardware_changed = true;
        }
    }
    if let Some(rate) = profile.audio_sample_rate {
        if state.hardware.audio_sample_rate != rate {
            state.hardware.audio_sample_rate = rate;
            audio_hardware_changed = true;
        }
    }
    if let Some(ref fmt) = profile.audio_sample_format {
        if &state.hardware.audio_sample_format != fmt {
            state.hardware.audio_sample_format = fmt.clone();
            audio_hardware_changed = true;
        }
    }

    if let Some(under) = profile.popup_under_crt {
        state.dict.popup_under_crt = under;
    }

    audio_hardware_changed
}

pub fn apply_config(state: &mut AppState, cfg: &MichadameConfig) {
    state.replay.config = cfg.replay.clone();
    state.replay.sync_audio_filter();
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
    state.dict.popup_under_crt = cfg.popup_under_crt.unwrap_or(false);
    state.bank.current_tag = cfg.bank_current_tag.clone().unwrap_or_default();
    state.bank.saved_current_tag = state.bank.current_tag.clone();
    state.bank.compact_mode = cfg.bank_compact_mode.unwrap_or(false);
    state.twitch.apply_config(cfg.twitch.clone());
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
            ..Default::default()
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
        save_config_at(&path, &mut state).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(loaded.bank_current_tag.as_deref(), Some("Grandia 2"));
        state.bank.current_tag.clear();
        save_config_at(&path, &mut state).unwrap();
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
        save_config_at(&path, &mut state).unwrap();
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
    fn test_underscan_profile_roundtrip() {
        let mut state = AppState::default();
        state.video.underscan_x = 0.045;
        state.video.underscan_y = -0.025;

        let profile = build_profile_from_state(&state);
        assert_eq!(profile.underscan_x, Some(0.045));
        assert_eq!(profile.underscan_y, Some(-0.025));

        let mut new_state = AppState::default();
        apply_profile_to_state(&mut new_state, &profile);
        assert_eq!(new_state.video.underscan_x, 0.045);
        assert_eq!(new_state.video.underscan_y, -0.025);
    }

    #[test]
    fn test_retro_pc_frame_profile_roundtrip() {
        let mut state = AppState::default();
        assert!(!state.video.retro_pc_frame);
        assert!(!state.video.retro_pc_frame_dark_mode);
        assert_eq!(state.video.retro_pc_ambient_glow, 0.55);
        assert!(!state.video.lights_off_night_mode);
        assert_eq!(state.video.night_mode_glow_intensity, 0.0);

        state.video.retro_pc_frame = true;
        state.video.retro_pc_frame_dark_mode = true;
        state.video.retro_pc_ambient_glow = 0.80;
        state.video.lights_off_night_mode = true;
        state.video.night_mode_glow_intensity = 0.75;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.retro_pc_frame, Some(true));
        assert_eq!(profile.retro_pc_frame_dark_mode, Some(true));
        assert_eq!(profile.retro_pc_ambient_glow, Some(0.80));
        assert_eq!(profile.lights_off_night_mode, Some(true));
        assert_eq!(profile.night_mode_glow_intensity, Some(0.75));

        let mut new_state = AppState::default();
        apply_profile_to_state(&mut new_state, &profile);
        assert!(new_state.video.retro_pc_frame);
        assert!(new_state.video.retro_pc_frame_dark_mode);
        assert_eq!(new_state.video.retro_pc_ambient_glow, 0.80);
        assert!(new_state.video.lights_off_night_mode);
        assert_eq!(new_state.video.night_mode_glow_intensity, 0.75);

        state.video.retro_pc_frame = false;
        state.video.retro_pc_frame_dark_mode = false;
        state.video.retro_pc_ambient_glow = 0.0;
        state.video.lights_off_night_mode = false;
        state.video.night_mode_glow_intensity = 0.0;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.retro_pc_frame, Some(false));
        assert_eq!(profile.retro_pc_frame_dark_mode, Some(false));
        assert_eq!(profile.retro_pc_ambient_glow, Some(0.0));
        assert_eq!(profile.lights_off_night_mode, Some(false));
        assert_eq!(profile.night_mode_glow_intensity, Some(0.0));

        apply_profile_to_state(&mut new_state, &profile);
        assert!(!new_state.video.retro_pc_frame);
        assert!(!new_state.video.retro_pc_frame_dark_mode);
        assert_eq!(new_state.video.retro_pc_ambient_glow, 0.0);
        assert!(!new_state.video.lights_off_night_mode);
        assert_eq!(new_state.video.night_mode_glow_intensity, 0.0);
    }

    #[test]
    fn test_retro_software_mouse_profile_roundtrip() {
        let mut state = AppState::default();
        assert!(!state.video.retro_software_mouse);

        state.video.retro_software_mouse = true;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.retro_software_mouse, Some(true));

        let mut new_state = AppState::default();
        apply_profile_to_state(&mut new_state, &profile);
        assert!(new_state.video.retro_software_mouse);

        state.video.retro_software_mouse = false;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.retro_software_mouse, Some(false));

        apply_profile_to_state(&mut new_state, &profile);
        assert!(!new_state.video.retro_software_mouse);
    }

    #[test]
    fn test_crt_glass_profile_roundtrip() {
        let mut state = AppState::default();
        assert!(!state.video.crt_glass_enabled);
        assert_eq!(state.video.crt_glass_intensity, 0.25);
        assert_eq!(state.video.crt_glass_glossiness, 0.50);
        assert!(state.video.crt_glass_ceiling_light_enabled);
        assert!(!state.video.crt_glass_photographer_enabled);
        assert_eq!(state.video.crt_glass_photographer_intensity, 0.50);
        assert!(!state.video.crt_glass_flash_enabled);
        assert_eq!(state.video.crt_glass_flash_intensity, 0.70);

        state.video.crt_glass_enabled = true;
        state.video.crt_glass_intensity = 0.65;
        state.video.crt_glass_glossiness = 0.85;
        state.video.crt_glass_ceiling_light_enabled = true;
        state.video.crt_glass_photographer_enabled = true;
        state.video.crt_glass_photographer_intensity = 0.80;
        state.video.crt_glass_flash_enabled = true;
        state.video.crt_glass_flash_intensity = 0.90;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.crt_glass_enabled, Some(true));
        assert_eq!(profile.crt_glass_intensity, Some(0.65));
        assert_eq!(profile.crt_glass_glossiness, Some(0.85));
        assert_eq!(profile.crt_glass_ceiling_light_enabled, Some(true));
        assert_eq!(profile.crt_glass_photographer_enabled, Some(true));
        assert_eq!(profile.crt_glass_photographer_intensity, Some(0.80));
        assert_eq!(profile.crt_glass_flash_enabled, Some(true));
        assert_eq!(profile.crt_glass_flash_intensity, Some(0.90));

        let mut new_state = AppState::default();
        apply_profile_to_state(&mut new_state, &profile);
        assert!(new_state.video.crt_glass_enabled);
        assert_eq!(new_state.video.crt_glass_intensity, 0.65);
        assert_eq!(new_state.video.crt_glass_glossiness, 0.85);
        assert!(new_state.video.crt_glass_ceiling_light_enabled);
        assert!(new_state.video.crt_glass_photographer_enabled);
        assert_eq!(new_state.video.crt_glass_photographer_intensity, 0.80);
        assert!(new_state.video.crt_glass_flash_enabled);
        assert_eq!(new_state.video.crt_glass_flash_intensity, 0.90);

        state.video.crt_glass_enabled = false;
        state.video.crt_glass_intensity = 0.10;
        state.video.crt_glass_glossiness = 0.20;
        state.video.crt_glass_ceiling_light_enabled = false;
        state.video.crt_glass_photographer_enabled = false;
        state.video.crt_glass_photographer_intensity = 0.30;
        state.video.crt_glass_flash_enabled = false;
        state.video.crt_glass_flash_intensity = 0.40;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.crt_glass_enabled, Some(false));
        assert_eq!(profile.crt_glass_intensity, Some(0.10));
        assert_eq!(profile.crt_glass_glossiness, Some(0.20));
        assert_eq!(profile.crt_glass_ceiling_light_enabled, Some(false));
        assert_eq!(profile.crt_glass_photographer_enabled, Some(false));
        assert_eq!(profile.crt_glass_photographer_intensity, Some(0.30));
        assert_eq!(profile.crt_glass_flash_enabled, Some(false));
        assert_eq!(profile.crt_glass_flash_intensity, Some(0.40));

        apply_profile_to_state(&mut new_state, &profile);
        assert!(!new_state.video.crt_glass_enabled);
        assert_eq!(new_state.video.crt_glass_intensity, 0.10);
        assert_eq!(new_state.video.crt_glass_glossiness, 0.20);
        assert!(!new_state.video.crt_glass_ceiling_light_enabled);
        assert!(!new_state.video.crt_glass_photographer_enabled);
        assert_eq!(new_state.video.crt_glass_photographer_intensity, 0.30);
        assert!(!new_state.video.crt_glass_flash_enabled);
        assert_eq!(new_state.video.crt_glass_flash_intensity, 0.40);
    }

    #[test]
    fn test_popup_under_crt_profile_roundtrip() {
        let mut state = AppState::default();
        assert!(!state.dict.popup_under_crt);

        state.dict.popup_under_crt = true;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.popup_under_crt, Some(true));

        let mut new_state = AppState::default();
        apply_profile_to_state(&mut new_state, &profile);
        assert!(new_state.dict.popup_under_crt);

        state.dict.popup_under_crt = false;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.popup_under_crt, Some(false));

        apply_profile_to_state(&mut new_state, &profile);
        assert!(!new_state.dict.popup_under_crt);
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

        let result = save_config_at(&path, &mut state);
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

        let mut state = AppState::default();
        let result = save_config_at(&path, &mut state);
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

    #[test]
    fn test_audio_filter_config_synced_on_startup_and_apply() {
        let mut state = AppState::default();
        assert!(!state.replay.audio.filter.read().unwrap().enabled);

        let mut cfg = MichadameConfig::default();
        cfg.replay.audio_filter.enabled = true;
        cfg.replay.audio_filter.denoise_reduction_db = 18.0;
        cfg.replay.audio_filter.hum_freq = 60.0;

        apply_config(&mut state, &cfg);

        // Verify that state.replay.audio.filter (read by ALSA thread) is immediately enabled and synced!
        let audio_filter = state.replay.audio.filter.read().unwrap();
        assert!(audio_filter.enabled);
        assert_eq!(audio_filter.denoise_reduction_db, 18.0);
        assert_eq!(audio_filter.hum_freq, 60.0);
    }

    #[test]
    fn test_profile_switch_preserves_distinct_crt_and_glass_settings() {
        let mut state = AppState::default();

        // 1. Configure Dreamcast profile with CRT Lottes, Glass ON, custom glass intensity
        state.active_profile = "Dreamcast".to_string();
        state.crt_filter.store(1, Ordering::Relaxed);
        state.selected_crt_filter = crate::devices::filter_type::CrtFilter::Lottes;
        state.video.crt_glass_enabled = true;
        state.video.crt_glass_intensity = 0.85;

        let dreamcast_profile = build_profile_from_state(&state);
        state.profiles.insert("Dreamcast".to_string(), dreamcast_profile);

        // 2. Configure PS2 profile with CRT Off, Glass OFF
        let mut ps2_profile = Profile::default();
        ps2_profile.crt_filter = Some(0);
        ps2_profile.crt_glass_enabled = Some(false);
        state.profiles.insert("PS2".to_string(), ps2_profile);

        // 3. Switch to PS2 profile
        let outgoing = build_profile_from_state(&state);
        state.profiles.insert("Dreamcast".to_string(), outgoing);
        state.active_profile = "PS2".to_string();
        let ps2_data = state.profiles.get("PS2").unwrap().clone();
        apply_profile_to_state(&mut state, &ps2_data);

        // Verify PS2 settings
        assert_eq!(state.crt_filter.load(Ordering::Relaxed), 0);
        assert!(!state.video.crt_glass_enabled);

        // 4. Switch back to Dreamcast
        let outgoing_ps2 = build_profile_from_state(&state);
        state.profiles.insert("PS2".to_string(), outgoing_ps2);
        state.active_profile = "Dreamcast".to_string();
        let dreamcast_data = state.profiles.get("Dreamcast").unwrap().clone();
        apply_profile_to_state(&mut state, &dreamcast_data);

        // Verify Dreamcast settings restored
        assert_eq!(state.crt_filter.load(Ordering::Relaxed), 1);
        assert_eq!(
            state.selected_crt_filter,
            crate::devices::filter_type::CrtFilter::Lottes
        );
        assert!(state.video.crt_glass_enabled);
        assert_eq!(state.video.crt_glass_intensity, 0.85);
    }

    #[test]
    fn test_profile_switch_preserves_audio_filter_and_settings() {
        let mut state = AppState::default();

        // Dreamcast audio filter settings
        let mut dc_filter = crate::replay::config::AudioFilterConfig::default();
        dc_filter.enabled = true;
        dc_filter.apply_preset(crate::replay::config::FilterPreset::DreamcastNtsc);

        let mut dc_profile = Profile::default();
        dc_profile.audio_filter = Some(dc_filter.clone());
        dc_profile.audio_buffer_size = Some(512);
        dc_profile.audio_sample_rate = Some(48000);
        state.profiles.insert("Dreamcast".to_string(), dc_profile);

        // PS2 audio filter settings
        let mut ps2_filter = crate::replay::config::AudioFilterConfig::default();
        ps2_filter.enabled = false;
        ps2_filter.preset = crate::replay::config::FilterPreset::ConsolePal;

        let mut ps2_profile = Profile::default();
        ps2_profile.audio_filter = Some(ps2_filter.clone());
        ps2_profile.audio_buffer_size = Some(2048);
        ps2_profile.audio_sample_rate = Some(44100);
        state.profiles.insert("PS2".to_string(), ps2_profile);

        // Switch to Dreamcast
        let dc_data = state.profiles.get("Dreamcast").unwrap().clone();
        apply_profile_to_state(&mut state, &dc_data);

        assert!(state.replay.config.audio_filter.enabled);
        assert_eq!(
            state.replay.config.audio_filter.preset,
            crate::replay::config::FilterPreset::DreamcastNtsc
        );
        assert_eq!(state.hardware.audio_buffer_size, 512);
        assert_eq!(state.hardware.audio_sample_rate, 48000);
        assert!(state.replay.audio.filter.read().unwrap().enabled);

        // Switch to PS2
        let ps2_data = state.profiles.get("PS2").unwrap().clone();
        apply_profile_to_state(&mut state, &ps2_data);

        assert!(!state.replay.config.audio_filter.enabled);
        assert_eq!(
            state.replay.config.audio_filter.preset,
            crate::replay::config::FilterPreset::ConsolePal
        );
        assert_eq!(state.hardware.audio_buffer_size, 2048);
        assert_eq!(state.hardware.audio_sample_rate, 44100);
        assert!(!state.replay.audio.filter.read().unwrap().enabled);
    }

    #[test]
    fn test_profile_switch_resets_unspecified_settings_to_prevent_leakage() {
        let mut state = AppState::default();

        // Profile A has glass and pixelate enabled
        state.video.crt_glass_enabled = true;
        state.video.pixelate_filter_enabled = true;
        state.crt_filter.store(2, Ordering::Relaxed);

        // Profile B is default (all None)
        let profile_b = Profile::default();
        apply_profile_to_state(&mut state, &profile_b);

        // Settings should be reset to defaults and not leak from Profile A
        assert_eq!(state.crt_filter.load(Ordering::Relaxed), 0);
        assert!(!state.video.crt_glass_enabled);
        assert!(!state.video.pixelate_filter_enabled);
    }

    #[test]
    fn test_profile_switching_without_explicit_save_persists_modifications() {
        let dir = std::env::temp_dir().join(format!(
            "michadame-test-prof-switch-{}",
            crate::replay::now_us()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test-config.toml");

        let mut state = AppState::default();
        state.active_profile = "Dreamcast".to_string();
        state
            .profiles
            .insert("Dreamcast".to_string(), Profile::default());
        state
            .profiles
            .insert("PS2".to_string(), Profile::default());

        save_config_at(&path, &mut state).unwrap();

        // 1. User tweaks settings on Dreamcast (toggles CRT filter and glass shader)
        state.crt_filter.store(1, Ordering::Relaxed);
        state.video.crt_glass_enabled = true;
        state.replay.config.audio_filter.enabled = true;
        state.replay.config.audio_filter.preset =
            crate::replay::config::FilterPreset::DreamcastNtsc;

        // 2. User switches to PS2 WITHOUT clicking "Save Config"
        let pre_selected = state.active_profile.clone();
        let outgoing = build_profile_from_state(&state);
        state.profiles.insert(pre_selected, outgoing);

        state.active_profile = "PS2".to_string();
        let ps2_profile = state.profiles.get("PS2").unwrap().clone();
        apply_profile_to_state(&mut state, &ps2_profile);
        save_config_at(&path, &mut state).unwrap();

        // On PS2, CRT filter is 0, glass is false, audio filter is disabled
        assert_eq!(state.crt_filter.load(Ordering::Relaxed), 0);
        assert!(!state.video.crt_glass_enabled);

        // 3. User tweaks settings on PS2
        state.crt_filter.store(2, Ordering::Relaxed);
        state.selected_crt_filter = crate::devices::filter_type::CrtFilter::Halo;

        // 4. User switches back to Dreamcast WITHOUT clicking "Save Config"
        let pre_selected = state.active_profile.clone();
        let outgoing = build_profile_from_state(&state);
        state.profiles.insert(pre_selected, outgoing);

        state.active_profile = "Dreamcast".to_string();
        let dc_profile = state.profiles.get("Dreamcast").unwrap().clone();
        apply_profile_to_state(&mut state, &dc_profile);
        save_config_at(&path, &mut state).unwrap();

        // Verify Dreamcast retained its tweaked settings
        assert_eq!(state.crt_filter.load(Ordering::Relaxed), 1);
        assert!(state.video.crt_glass_enabled);
        assert!(state.replay.config.audio_filter.enabled);
        assert_eq!(
            state.replay.config.audio_filter.preset,
            crate::replay::config::FilterPreset::DreamcastNtsc
        );

        // 5. User switches back to PS2
        let pre_selected = state.active_profile.clone();
        let outgoing = build_profile_from_state(&state);
        state.profiles.insert(pre_selected, outgoing);

        state.active_profile = "PS2".to_string();
        let ps2_profile = state.profiles.get("PS2").unwrap().clone();
        apply_profile_to_state(&mut state, &ps2_profile);

        // Verify PS2 retained its tweaked settings
        assert_eq!(state.crt_filter.load(Ordering::Relaxed), 2);
        assert_eq!(
            state.selected_crt_filter,
            crate::devices::filter_type::CrtFilter::Halo
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_deinterlace_filter_profile_roundtrip() {
        let mut state = AppState::default();
        state.video.deinterlace_filter_enabled = true;
        state.video.deinterlace_mode = 2;
        state.video.deinterlace_blend = 0.65;
        state.video.deinterlace_motion_threshold = 0.25;
        state.video.deinterlace_line_spacing = 2.0;
        state.video.deinterlace_spatial_mix = 0.95;

        let profile = build_profile_from_state(&state);
        assert_eq!(profile.deinterlace_filter_enabled, Some(true));
        assert_eq!(profile.deinterlace_mode, Some(2));
        assert_eq!(profile.deinterlace_blend, Some(0.65));
        assert_eq!(profile.deinterlace_motion_threshold, Some(0.25));
        assert_eq!(profile.deinterlace_line_spacing, Some(2.0));
        assert_eq!(profile.deinterlace_spatial_mix, Some(0.95));

        let mut loaded_state = AppState::default();
        apply_profile_to_state(&mut loaded_state, &profile);
        assert!(loaded_state.video.deinterlace_filter_enabled);
        assert_eq!(loaded_state.video.deinterlace_mode, 2);
        assert_eq!(loaded_state.video.deinterlace_blend, 0.65);
        assert_eq!(loaded_state.video.deinterlace_motion_threshold, 0.25);
        assert_eq!(loaded_state.video.deinterlace_line_spacing, 2.0);
        assert_eq!(loaded_state.video.deinterlace_spatial_mix, 0.95);
    }

    #[test]
    fn twitch_settings_round_trip_and_old_configs_get_defaults() {
        let path = std::env::temp_dir().join(format!(
            "michadame-twitch-config-{}-{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));
        // An existing config written before the Twitch table existed.
        std::fs::write(&path, "active_profile = \"Default\"\nocr_timeout_seconds = 30\n").unwrap();
        let old: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(old.twitch, crate::config::TwitchConfig::default());
        assert_eq!(old.ocr_timeout_seconds, Some(30));

        let mut state = AppState::default();
        apply_config(&mut state, &old);
        state.twitch.config.channel = "some_streamer".into();
        state.twitch.config.chat_overlay_enabled = true;
        state.twitch.config.overlay_width_pct = 0.3;
        state.twitch.config.message_lifetime_secs = 0;
        save_config_at(&path, &mut state).unwrap();

        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(loaded.twitch, state.twitch.config);
        let mut restarted = AppState::default();
        apply_config(&mut restarted, &loaded);
        assert_eq!(restarted.twitch.config, state.twitch.config);
        assert_eq!(restarted.twitch.channel_input, "some_streamer");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("toml.bak"));
    }
}
