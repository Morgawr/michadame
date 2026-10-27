use super::models::{MichadameConfig, Profile};
use crate::app::models::AppState;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

fn set_private_file_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

pub fn quarantine_corrupted_config(path: &Path) -> Option<PathBuf> {
    if path.exists()
        && std::fs::metadata(path)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    {
        let parent = path.parent()?;
        let backups_dir = parent.join("backups");
        let _ = std::fs::create_dir_all(&backups_dir);
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let quarantine_path = backups_dir.join(format!("corrupted-{}.toml", now_secs));
        if std::fs::copy(path, &quarantine_path).is_ok() {
            set_private_file_permissions(&quarantine_path);
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
    if std::fs::copy(config_path, &bak_path).is_ok() {
        set_private_file_permissions(&bak_path);
    }

    // 2. Timestamped rotating backup in backups/ (rate-limited to at most once every 60 seconds)
    if let Some(parent) = config_path.parent() {
        let backups_dir = parent.join("backups");
        if std::fs::create_dir_all(&backups_dir).is_ok() {
            let now_secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let recent_backup_exists = std::fs::read_dir(&backups_dir)
                .ok()
                .map(|entries| {
                    entries.filter_map(|e| e.ok()).any(|e| {
                        let name = e.file_name();
                        let s = name.to_string_lossy();
                        s.strip_prefix("config-")
                            .and_then(|s| s.strip_suffix(".toml"))
                            .and_then(|s| s.parse::<u64>().ok())
                            .is_some_and(|ts| now_secs.saturating_sub(ts) < 60)
                    })
                })
                .unwrap_or(false);

            if !recent_backup_exists {
                let backup_file = backups_dir.join(format!("config-{}.toml", now_secs));
                if std::fs::copy(config_path, &backup_file).is_ok() {
                    set_private_file_permissions(&backup_file);
                }

                prune_old_backups(&backups_dir, 20);
            }
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
                let mtime = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(UNIX_EPOCH);
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

pub fn store_config_atomic_at(path: &Path, cfg: &MichadameConfig) -> Result<(), confy::ConfyError> {
    create_rotating_backup(path);

    let parent = path.parent().ok_or_else(|| {
        confy::ConfyError::BadConfigDirectory(format!("{:?} is a root or prefix", path))
    })?;
    std::fs::create_dir_all(parent).map_err(confy::ConfyError::DirectoryCreationFailed)?;

    let tmp_path = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config"),
        std::process::id()
    ));

    confy::store_path(&tmp_path, cfg)?;
    set_private_file_permissions(&tmp_path);

    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(confy::ConfyError::WriteConfigurationFileError(e));
    }

    Ok(())
}

pub fn save_replay_config(
    replay: &crate::replay::config::ReplayConfig,
) -> Result<(), confy::ConfyError> {
    if cfg!(test) {
        return Ok(());
    }
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
        realistic_light_switch: Some(state.video.realistic_light_switch),
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
        fft_mask_save_name: if state.fft_mask_save_name.trim().is_empty() {
            None
        } else {
            Some(
                crate::config::fft_masks::sanitize_mask_name(&state.fft_mask_save_name)
                    .unwrap_or_else(|_| state.fft_mask_save_name.clone()),
            )
        },

        audio_source: state.hardware.selected_audio_source_name.clone(),
        audio_buffer_size: Some(state.hardware.audio_buffer_size),
        audio_sample_rate: Some(state.hardware.audio_sample_rate),
        audio_sample_format: Some(state.hardware.audio_sample_format.clone()),
        master_volume: Some(state.hardware.master_volume),

        audio_filter: Some(state.replay.config.audio_filter.clone()),
        popup_under_crt: Some(state.dict.popup_under_crt),
    }
}

pub fn sync_active_profile_in_state(state: &mut AppState) {
    let current_profile_data = build_profile_from_state(state);
    if state.video_player.is_some() {
        state.video_profile = current_profile_data;
    } else if state.gamescope_hook.as_ref().map_or(false, |g| g.is_active) {
        state.gamescope_profile = current_profile_data;
    } else if state.pc98.is_active {
        state.pc98_profile = current_profile_data;
    } else if state.pc88.is_active {
        state.pc88_profile = current_profile_data;
    } else {
        state
            .profiles
            .insert(state.active_profile.clone(), current_profile_data);
    }
}

pub fn save_config(state: &mut AppState) {
    sync_active_profile_in_state(state);
    if state.ui.in_egui_frame {
        state.ui.config_save_pending = true;
        return;
    }
    flush_pending_config(state);
}

pub fn flush_pending_config(state: &mut AppState) {
    state.ui.config_save_pending = false;
    if cfg!(test) {
        return;
    }
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

    sync_active_profile_in_state(state);
    save_global_hardware_config_at(path, state)
}

pub fn save_global_hardware_config(state: &mut AppState) {
    save_config(state);
}

pub fn save_global_hardware_config_at(
    path: &Path,
    state: &AppState,
) -> Result<(), confy::ConfyError> {
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
    if state.video_player.is_none()
        && state.gamescope_hook.is_none()
        && !state.pc98.is_active
        && !state.pc88.is_active
    {
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
    }
    cfg.reset_usb_on_startup = Some(state.ui.reset_usb_on_startup);
    cfg.has_shown_first_run_warning = Some(!state.ui.show_first_run_dialog);
    cfg.audio_source = state.hardware.selected_audio_source_name.clone();
    cfg.audio_buffer_size = Some(state.hardware.audio_buffer_size);
    cfg.audio_sample_rate = Some(state.hardware.audio_sample_rate);
    cfg.audio_sample_format = Some(state.hardware.audio_sample_format.clone());
    cfg.master_volume = Some(state.hardware.master_volume);
    cfg.ocr_backend = Some(state.ocr.backend);
    cfg.ocr_sticky_distance = Some(state.ocr.sticky_distance);
    cfg.ocr_hide_overlay = Some(state.ocr.hide_overlay);
    cfg.ocr_timeout_seconds = Some(state.ocr.timeout_seconds);
    cfg.ocr_constant_scanning = Some(state.ocr.constant_scanning);
    cfg.ocr_scan_interval_secs = Some(state.ocr.scan_interval_secs);
    cfg.ocr_max_dropoff_secs = Some(state.ocr.max_dropoff_secs);
    cfg.ocr_jitter_tolerance = Some(state.ocr.jitter_tolerance);
    cfg.bank_current_tag = crate::bank::tags::normalize_tag(&state.bank.current_tag);
    cfg.bank_compact_mode = Some(state.bank.compact_mode);
    cfg.default_halo = Some(state.halo_defaults.clone());
    cfg.default_cathode_interference = Some(state.cathode_interference_defaults.clone());
    cfg.popup_under_crt = Some(state.dict.popup_under_crt);
    cfg.retro_pc_system = state.retro_pc_system;
    cfg.gamescope_launcher_script = Some(state.gamescope_launcher_script.clone());
    cfg.selected_steam_game = state.selected_steam_game.clone();
    cfg.twitch = state.twitch.config.clone();
    cfg.mic = state.mic.config.clone();
    cfg.webcam = state.webcam.config.clone();
    cfg.pc98 = state.pc98.config.clone();
    cfg.pc88 = state.pc88.config.clone();
    cfg.audio_reactive = state.audio_reactive.clone();

    let current_profile_data = build_profile_from_state(state);
    if state.video_player.is_some() {
        cfg.video_profile = Some(current_profile_data);
        cfg.gamescope_profile = Some(state.gamescope_profile.clone());
        cfg.pc98_profile = Some(state.pc98_profile.clone());
        cfg.pc88_profile = Some(state.pc88_profile.clone());
    } else if state.gamescope_hook.as_ref().map_or(false, |g| g.is_active) {
        cfg.gamescope_profile = Some(current_profile_data);
        cfg.video_profile = Some(state.video_profile.clone());
        cfg.pc98_profile = Some(state.pc98_profile.clone());
        cfg.pc88_profile = Some(state.pc88_profile.clone());
    } else if state.pc98.is_active {
        cfg.pc98_profile = Some(current_profile_data);
        cfg.video_profile = Some(state.video_profile.clone());
        cfg.gamescope_profile = Some(state.gamescope_profile.clone());
        cfg.pc88_profile = Some(state.pc88_profile.clone());
    } else if state.pc88.is_active {
        cfg.pc88_profile = Some(current_profile_data);
        cfg.video_profile = Some(state.video_profile.clone());
        cfg.gamescope_profile = Some(state.gamescope_profile.clone());
        cfg.pc98_profile = Some(state.pc98_profile.clone());
    } else {
        let mut profiles = state.profiles.clone();
        profiles.insert(state.active_profile.clone(), current_profile_data);
        cfg.active_profile = state.active_profile.clone();
        cfg.profiles = profiles;
        cfg.video_profile = Some(state.video_profile.clone());
        cfg.gamescope_profile = Some(state.gamescope_profile.clone());
        cfg.pc98_profile = Some(state.pc98_profile.clone());
        cfg.pc88_profile = Some(state.pc88_profile.clone());
    }

    store_config_atomic_at(path, &cfg)
}

fn sanitize_finite_f32(val: Option<f32>, default: f32, min: f32, max: f32) -> f32 {
    let fallback = if default.is_finite() {
        default.clamp(min, max)
    } else {
        min
    };
    val.filter(|v| v.is_finite())
        .unwrap_or(fallback)
        .clamp(min, max)
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

    state.crt.hard_scan = sanitize_finite_f32(profile.crt_hard_scan, -8.0, -20.0, 0.0);
    state.crt.hard_pix = sanitize_finite_f32(profile.crt_hard_pix, -3.0, -20.0, 0.0);
    state.crt.brightboost = sanitize_finite_f32(profile.crt_brightboost, 1.0, 0.0, 4.0);
    state.crt.warp_x = sanitize_finite_f32(profile.crt_warp_x, 0.031, 0.0, 0.5);
    state.crt.warp_y = sanitize_finite_f32(profile.crt_warp_y, 0.041, 0.0, 0.5);
    state.crt.shadow_mask = sanitize_finite_f32(profile.crt_shadow_mask, 3.0, 0.0, 4.0);
    state.crt.hard_bloom_pix = sanitize_finite_f32(profile.crt_hard_bloom_pix, -1.5, -4.0, 0.0);
    state.crt.hard_bloom_scan = sanitize_finite_f32(profile.crt_hard_bloom_scan, -2.0, -8.0, 0.0);
    state.crt.bloom_amount = sanitize_finite_f32(profile.crt_bloom_amount, 0.15, 0.0, 2.0);
    state.crt.shape = sanitize_finite_f32(profile.crt_shape, 2.0, 0.0, 10.0);

    state.video.use_magenta_background = profile.use_magenta_background.unwrap_or(false);
    state.video.retro_pc_frame = profile.retro_pc_frame.unwrap_or(false);
    state.video.retro_pc_frame_dark_mode = profile.retro_pc_frame_dark_mode.unwrap_or(false);
    state.video.retro_pc_ambient_glow =
        sanitize_finite_f32(profile.retro_pc_ambient_glow, 0.55, 0.0, 1.0);
    state.video.retro_software_mouse = profile.retro_software_mouse.unwrap_or(false);
    state.video.lights_off_night_mode = profile.lights_off_night_mode.unwrap_or(false);
    state.video.realistic_light_switch = profile.realistic_light_switch.unwrap_or(false);
    state.video.light_switch_flicker_start = None;
    state.video.night_mode_glow_intensity =
        sanitize_finite_f32(profile.night_mode_glow_intensity, 0.0, 0.0, 1.0);

    state.video.crt_glass_enabled = profile.crt_glass_enabled.unwrap_or(false);
    state.video.crt_glass_intensity =
        sanitize_finite_f32(profile.crt_glass_intensity, 0.25, 0.0, 1.0);
    state.video.crt_glass_glossiness =
        sanitize_finite_f32(profile.crt_glass_glossiness, 0.50, 0.0, 1.0);
    state.video.crt_glass_ceiling_light_enabled =
        profile.crt_glass_ceiling_light_enabled.unwrap_or(true);
    state.video.crt_glass_photographer_enabled =
        profile.crt_glass_photographer_enabled.unwrap_or(false);
    state.video.crt_glass_photographer_intensity =
        sanitize_finite_f32(profile.crt_glass_photographer_intensity, 0.50, 0.0, 1.0);
    state.video.crt_glass_flash_enabled = profile.crt_glass_flash_enabled.unwrap_or(false);
    state.video.crt_glass_flash_intensity =
        sanitize_finite_f32(profile.crt_glass_flash_intensity, 0.70, 0.0, 1.0);

    state.video.horizontal_stretch = sanitize_finite_f32(profile.horizontal_stretch, 1.0, 0.5, 1.5);
    state.video.median_filter_enabled = profile.median_filter_enabled.unwrap_or(false);
    state.video.median_mix = sanitize_finite_f32(profile.median_mix, 1.0, 0.0, 1.0);
    state.video.deinterlace_filter_enabled = profile.deinterlace_filter_enabled.unwrap_or(false);
    state.video.deinterlace_mode = profile.deinterlace_mode.unwrap_or(0);
    state.video.deinterlace_blend = sanitize_finite_f32(profile.deinterlace_blend, 0.5, 0.0, 1.0);
    state.video.deinterlace_motion_threshold =
        sanitize_finite_f32(profile.deinterlace_motion_threshold, 0.08, 0.01, 0.5);
    state.video.deinterlace_line_spacing =
        sanitize_finite_f32(profile.deinterlace_line_spacing, 1.0, 0.5, 3.0);
    state.video.deinterlace_spatial_mix =
        sanitize_finite_f32(profile.deinterlace_spatial_mix, 0.75, 0.0, 1.0);
    state.video.vibrance = sanitize_finite_f32(profile.vibrance, 1.0, 0.0, 3.0);
    state.video.overscan_x = sanitize_finite_f32(profile.overscan_x, 0.0, -0.2, 0.2);
    state.video.overscan_y = sanitize_finite_f32(profile.overscan_y, 0.0, -0.2, 0.2);
    state.video.underscan_x = sanitize_finite_f32(profile.underscan_x, 0.0, -0.2, 0.3);
    state.video.underscan_y = sanitize_finite_f32(profile.underscan_y, 0.0, -0.2, 0.3);
    state.video.border_crop_left = sanitize_finite_f32(profile.border_crop_left, 0.0, 0.0, 0.2);
    state.video.border_crop_right = sanitize_finite_f32(profile.border_crop_right, 0.0, 0.0, 0.2);
    state.video.border_crop_top = sanitize_finite_f32(profile.border_crop_top, 0.0, 0.0, 0.2);
    state.video.border_crop_bottom = sanitize_finite_f32(profile.border_crop_bottom, 0.0, 0.0, 0.2);

    let def_halo = state.halo_defaults.clone();
    state.halo.brightboost =
        sanitize_finite_f32(profile.halo_brightboost, def_halo.brightboost, 0.5, 3.0);
    state.halo.brightboost1 =
        sanitize_finite_f32(profile.halo_brightboost1, def_halo.brightboost1, 0.5, 3.0);
    state.halo.beam_min = sanitize_finite_f32(profile.halo_beam_min, def_halo.beam_min, 0.5, 3.0);
    state.halo.beam_max = sanitize_finite_f32(profile.halo_beam_max, def_halo.beam_max, 0.2, 2.0);
    state.halo.beam_size =
        sanitize_finite_f32(profile.halo_beam_size, def_halo.beam_size, 0.0, 2.0);
    state.halo.h_sharp = sanitize_finite_f32(profile.halo_h_sharp, def_halo.h_sharp, 1.0, 10.0);
    state.halo.glow = sanitize_finite_f32(profile.halo_glow, def_halo.glow, 0.0, 1.0);
    state.halo.bloom = sanitize_finite_f32(profile.halo_bloom, def_halo.bloom, 0.0, 1.0);
    state.halo.halation = sanitize_finite_f32(profile.halo_halation, def_halo.halation, 0.0, 0.5);
    state.halo.shadow_mask =
        sanitize_finite_f32(profile.halo_shadow_mask, def_halo.shadow_mask, 0.0, 6.0);
    state.halo.masksize = sanitize_finite_f32(profile.halo_masksize, def_halo.masksize, 1.0, 4.0);
    state.halo.maskstr = sanitize_finite_f32(profile.halo_maskstr, def_halo.maskstr, 0.0, 1.0);
    state.halo.mcut = sanitize_finite_f32(profile.halo_mcut, def_halo.mcut, 0.0, 1.0);
    state.halo.slotmask = sanitize_finite_f32(profile.halo_slotmask, def_halo.slotmask, 0.0, 1.0);
    state.halo.slotmask1 =
        sanitize_finite_f32(profile.halo_slotmask1, def_halo.slotmask1, 0.0, 1.0);
    state.halo.double_slot =
        sanitize_finite_f32(profile.halo_double_slot, def_halo.double_slot, 1.0, 4.0);
    state.halo.smoothmask =
        sanitize_finite_f32(profile.halo_smoothmask, def_halo.smoothmask, 0.0, 2.0);
    state.halo.halo_zoom = sanitize_finite_f32(profile.halo_zoom, def_halo.halo_zoom, 50.0, 100.0);
    state.halo.halo_intensity =
        sanitize_finite_f32(profile.halo_intensity, def_halo.halo_intensity, 0.0, 2.0);
    state.halo.corner_size =
        sanitize_finite_f32(profile.halo_corner_size, def_halo.corner_size, 0.0, 0.1);
    state.halo.curvature = profile.halo_curvature.unwrap_or(def_halo.curvature);

    let def_cathode = state.cathode_interference_defaults.clone();
    state.cathode_interference.enabled = profile
        .cathode_interference_enabled
        .unwrap_or(def_cathode.enabled);
    state.cathode_interference.intensity =
        sanitize_finite_f32(profile.cathode_intensity, def_cathode.intensity, 0.0, 1.0);
    state.cathode_interference.frequency =
        sanitize_finite_f32(profile.cathode_frequency, def_cathode.frequency, 0.1, 5.0);
    state.cathode_interference.randomization = sanitize_finite_f32(
        profile.cathode_randomization,
        def_cathode.randomization,
        0.0,
        1.0,
    );
    state.cathode_interference.electricity_glow = sanitize_finite_f32(
        profile.cathode_electricity_glow,
        def_cathode.electricity_glow,
        0.0,
        1.0,
    );
    state.cathode_interference.flicker_depth = sanitize_finite_f32(
        profile.cathode_flicker_depth,
        def_cathode.flicker_depth,
        0.0,
        1.0,
    );
    state.cathode_interference.interference = sanitize_finite_f32(
        profile.cathode_interference,
        def_cathode.interference,
        0.0,
        1.0,
    );
    state.cathode_interference.lightbulb_effect = sanitize_finite_f32(
        profile.cathode_lightbulb_effect,
        def_cathode.lightbulb_effect,
        0.0,
        1.0,
    );

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
        let canonical_name = crate::config::fft_masks::sanitize_mask_name(mask_name)
            .unwrap_or_else(|_| mask_name.clone());
        state.fft_mask_save_name = canonical_name.clone();
        if state.video.fft_filter_enabled && !canonical_name.is_empty() {
            let stream_res = state
                .latest_frame
                .as_ref()
                .map(|f| (f.width, f.height))
                .unwrap_or(state.hardware.selected_resolution);
            if stream_res.0 > 0 && stream_res.1 > 0 {
                match crate::config::fft_masks::load_mask(&canonical_name, stream_res) {
                    Ok((data, fft_res, mask_thresh, black_thresh)) => {
                        state.fft_mask_data = data;
                        state.fft_mask_resolution = fft_res;
                        state.fft_mask_threshold = mask_thresh;
                        state.fft_black_threshold = black_thresh;
                        state.fft_mask_dirty = true;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to restore FFT mask '{}': {}", canonical_name, e);
                        state.error(format!(
                            "Failed to restore FFT mask '{}': {}",
                            canonical_name, e
                        ));
                    }
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
            if state
                .hardware
                .audio_sources
                .iter()
                .any(|(_, name)| name == source)
            {
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

    if let Some(vol) = profile.master_volume {
        state.set_master_volume(sanitize_finite_f32(Some(vol), 1.0, 0.0, 1.0));
    }

    audio_hardware_changed
}

pub fn apply_hardware_devices_from_config(state: &mut AppState, cfg: &MichadameConfig) {
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

    if let Some(saved_mic) = &cfg.mic.device_name {
        if state
            .hardware
            .audio_sources
            .iter()
            .any(|(_, name)| name == saved_mic)
        {
            state.mic.config.device_name = Some(saved_mic.clone());
        }
    }

    if let Some(dev) = &state.webcam.config.device {
        if state.hardware.video_devices.contains(dev) {
            state.webcam.formats_probed_for_device = Some(dev.clone());
            if let Ok(formats) = crate::devices::video::find_video_formats(dev) {
                state.webcam.supported_formats = formats;
                if let Some(saved_fourcc) = &state.webcam.config.format_fourcc {
                    if let Some(idx) = state
                        .webcam
                        .supported_formats
                        .iter()
                        .position(|f| f.fourcc == *saved_fourcc)
                    {
                        state.webcam.selected_format_index = idx;
                    }
                }
            }
        }
    }
}

pub fn apply_config(state: &mut AppState, cfg: &MichadameConfig) {
    state.replay.config = cfg.replay.clone();
    state.replay.sync_audio_filter();
    state.profiles = cfg.profiles.clone();
    state.active_profile = cfg.active_profile.clone();
    state.video_profile = cfg.video_profile.clone().unwrap_or_default();
    state.gamescope_profile = cfg.gamescope_profile.clone().unwrap_or_default();
    state.pc98_profile = cfg
        .pc98_profile
        .clone()
        .unwrap_or_else(super::models::default_pc98_profile);
    state.pc88_profile = cfg
        .pc88_profile
        .clone()
        .or_else(|| cfg.pc98_profile.clone())
        .unwrap_or_else(super::models::default_pc88_profile);
    state.retro_pc_system = cfg.retro_pc_system;
    state.gamescope_launcher_script = cfg
        .gamescope_launcher_script
        .clone()
        .unwrap_or_else(crate::gamescope::steam::default_launcher_script_path);
    state.selected_steam_game = cfg.selected_steam_game.clone();
    state.pc98.config = cfg.pc98.clone();
    if !state.pc98.config.games_dir.trim().is_empty() {
        let _ = state.pc98.refresh_games_list();
    }
    state.pc88.config = cfg.pc88.clone();
    if !state.pc88.config.games_dir.trim().is_empty() {
        let _ = state.pc88.refresh_games_list();
    }

    state.hardware.audio_buffer_size = cfg.audio_buffer_size.unwrap_or(1024);
    state.hardware.audio_sample_rate = cfg.audio_sample_rate.unwrap_or(48000);
    state.hardware.audio_sample_format = cfg
        .audio_sample_format
        .clone()
        .unwrap_or_else(|| "S16LE".to_string());
    if let Some(vol) = cfg.master_volume {
        state.set_master_volume(sanitize_finite_f32(Some(vol), 1.0, 0.0, 1.0));
    }

    if !cfg.has_shown_first_run_warning.unwrap_or(false) {
        state.ui.show_first_run_dialog = true;
    }
    state.ocr.set_backend(cfg.ocr_backend.unwrap_or_default());
    state.ocr.sticky_distance = sanitize_finite_f32(cfg.ocr_sticky_distance, 0.6, 0.0, 10.0);
    state.ocr.hide_overlay = cfg.ocr_hide_overlay.unwrap_or(false);
    state.ocr.timeout_seconds = cfg.ocr_timeout_seconds.unwrap_or(45);
    state.ocr.constant_scanning = cfg.ocr_constant_scanning.unwrap_or(false);
    state.ocr.scan_interval_secs = sanitize_finite_f32(
        cfg.ocr_scan_interval_secs,
        crate::ocr::models::DEFAULT_SCAN_INTERVAL_SECS,
        crate::ocr::models::MIN_SCAN_INTERVAL_SECS,
        crate::ocr::models::MAX_SCAN_INTERVAL_SECS,
    );
    state.ocr.max_dropoff_secs = sanitize_finite_f32(
        cfg.ocr_max_dropoff_secs,
        crate::ocr::models::DEFAULT_MAX_DROPOFF_SECS,
        crate::ocr::models::MIN_MAX_DROPOFF_SECS,
        crate::ocr::models::MAX_MAX_DROPOFF_SECS,
    )
    .max(state.ocr.scan_interval_secs);
    state.ocr.jitter_tolerance = sanitize_finite_f32(
        cfg.ocr_jitter_tolerance,
        crate::ocr::models::DEFAULT_JITTER_TOLERANCE,
        crate::ocr::models::MIN_JITTER_TOLERANCE,
        crate::ocr::models::MAX_JITTER_TOLERANCE,
    );
    state.ocr.reset_constant_scan_backoff();
    state.dict.popup_under_crt = cfg.popup_under_crt.unwrap_or(false);
    state.bank.current_tag = cfg.bank_current_tag.clone().unwrap_or_default();
    state.bank.saved_current_tag = state.bank.current_tag.clone();
    state.bank.compact_mode = cfg.bank_compact_mode.unwrap_or(false);
    state.twitch.apply_config(cfg.twitch.clone());
    state.mic.config = cfg.mic.clone();
    if let Some(defaults) = &cfg.default_halo {
        state.halo_defaults = defaults.clone();
        state.halo = defaults.clone();
    }
    if let Some(defaults) = &cfg.default_cathode_interference {
        state.cathode_interference_defaults = defaults.clone();
        state.cathode_interference = defaults.clone();
    }
    state.audio_reactive = cfg.audio_reactive.clone();
    state.audio_reactive.ensure_builtin_presets();
    state
        .replay
        .audio
        .set_reactive_listening(state.audio_reactive.enabled);

    state.webcam.config = cfg.webcam.clone();
    apply_hardware_devices_from_config(state, cfg);

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
        state.hardware.master_volume = 0.75;

        let profile = build_profile_from_state(&state);
        assert_eq!(profile.master_volume, Some(0.75));
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
            master_volume: Some(0.6),
            ..Default::default()
        };

        apply_profile_to_state(&mut state, &profile);
        assert_eq!(state.hardware.master_volume, 0.6);
        assert_eq!(state.crt.hard_scan, -15.0);
        assert_eq!(state.halo.brightboost, 2.5);
        assert!(state.cathode_interference.enabled);
        assert_eq!(state.cathode_interference.intensity, 0.9);
        assert_eq!(state.cathode_interference.frequency, 2.0);
        assert_eq!(state.cathode_interference.lightbulb_effect, 0.65);
        assert_eq!(
            state.selected_crt_filter,
            crate::devices::filter_type::CrtFilter::Halo
        );
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
            master_volume: Some(0.4),
            ..Default::default()
        };

        apply_config(&mut state, &cfg);
        assert_eq!(state.hardware.master_volume, 0.4);
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
    fn test_ocr_backend_toml_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "michadame-ocr-backend-config-{}-{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));
        let cfg = MichadameConfig {
            ocr_backend: Some(crate::ocr::OcrBackend::LocalScreenAi),
            ..Default::default()
        };
        confy::store_path(&path, &cfg).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(
            loaded.ocr_backend,
            Some(crate::ocr::OcrBackend::LocalScreenAi)
        );

        let mut state = AppState::default();
        apply_config(&mut state, &loaded);
        assert_eq!(state.ocr.backend, crate::ocr::OcrBackend::LocalScreenAi);
        assert_eq!(
            crate::ocr::OcrBackend::from_u8(state.ocr.backend_shared.load(Ordering::Acquire)),
            crate::ocr::OcrBackend::LocalScreenAi
        );
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

        // Realistic light switch persistence roundtrip
        assert!(!state.video.realistic_light_switch);
        state.video.realistic_light_switch = true;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.realistic_light_switch, Some(true));
        apply_profile_to_state(&mut new_state, &profile);
        assert!(new_state.video.realistic_light_switch);

        state.video.realistic_light_switch = false;
        let profile = build_profile_from_state(&state);
        assert_eq!(profile.realistic_light_switch, Some(false));
        apply_profile_to_state(&mut new_state, &profile);
        assert!(!new_state.video.realistic_light_switch);
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
        let dir =
            std::env::temp_dir().join(format!("michadame-test-abort-{}", crate::replay::now_us()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test-config.toml");
        std::fs::write(&path, "original_content = true\n").unwrap();

        let mut state = AppState::default();
        state.config_load_error = Some("Failed to parse TOML".to_string());

        let result = save_config_at(&path, &mut state);
        assert!(
            result.is_err(),
            "save_config_at must return an Err when config_load_error is set"
        );

        // Verify the original file on disk was NOT touched or overwritten
        let current_content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(current_content, "original_content = true\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_aborted_when_existing_file_corrupt() {
        let dir = std::env::temp_dir().join(format!(
            "michadame-test-corrupt-{}",
            crate::replay::now_us()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test-config.toml");
        std::fs::write(&path, "invalid = [[[\n").unwrap();

        let mut state = AppState::default();
        let result = save_config_at(&path, &mut state);
        assert!(
            result.is_err(),
            "save_config_at must refuse to overwrite corrupt config on disk"
        );

        let current_content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(current_content, "invalid = [[[\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_quarantine_corrupted_config() {
        let dir = std::env::temp_dir().join(format!(
            "michadame-test-quarantine-{}",
            crate::replay::now_us()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test-config.toml");
        let corrupted_bytes = b"bad toml content = { [ unclosed";
        std::fs::write(&path, corrupted_bytes).unwrap();

        let quarantined = quarantine_corrupted_config(&path);
        assert!(
            quarantined.is_some(),
            "quarantine_corrupted_config should return a quarantine path"
        );
        let q_path = quarantined.unwrap();
        assert!(q_path.exists(), "quarantine file must exist on disk");
        assert_eq!(std::fs::read(&q_path).unwrap(), corrupted_bytes);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rotating_backup_and_pruning() {
        let dir =
            std::env::temp_dir().join(format!("michadame-test-backup-{}", crate::replay::now_us()));
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
        assert_eq!(
            count, 5,
            "prune_old_backups should reduce count to max_keep"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_legacy_config_missing_fields_deserialization() {
        let dir =
            std::env::temp_dir().join(format!("michadame-test-legacy-{}", crate::replay::now_us()));
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
        let loaded: MichadameConfig =
            confy::load_path(&path).expect("Old TOML should deserialize cleanly");
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
        state
            .profiles
            .insert("Dreamcast".to_string(), dreamcast_profile);

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
        state.profiles.insert("PS2".to_string(), Profile::default());

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
        std::fs::write(
            &path,
            "active_profile = \"Default\"\nocr_timeout_seconds = 30\n",
        )
        .unwrap();
        let old: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(old.twitch, crate::config::TwitchConfig::default());
        assert_eq!(old.ocr_timeout_seconds, Some(30));

        let mut state = AppState::default();
        apply_config(&mut state, &old);
        state.twitch.config.channel = "some_streamer".into();
        state.twitch.config.chat_overlay_enabled = true;
        state.twitch.config.overlay_width_pct = 0.3;
        state.twitch.config.message_lifetime_secs = 0;
        state.twitch.config.preroll_enabled = true;
        state.twitch.config.preroll_video_path = "/path/to/intro.mp4".into();
        state.twitch.config.outro_enabled = true;
        state.twitch.config.outro_video_path = "/path/to/outro.mp4".into();
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

    #[test]
    fn test_video_profile_isolation_in_persistence() {
        let path = std::env::temp_dir().join(format!(
            "michadame-video-profile-test-{}-{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));

        let mut state = AppState::default();
        let retro_profile = Profile {
            crt_hard_scan: Some(-15.0),
            vibrance: Some(1.3),
            video_resolution: Some((640, 480)),
            ..Default::default()
        };
        state.profiles.insert("Retro".to_string(), retro_profile);
        state.active_profile = "Retro".to_string();
        state.crt.hard_scan = -15.0;
        state.video.vibrance = 1.3;
        state.hardware.selected_resolution = (640, 480);

        // 1. Normal save: writes to Retro profile
        save_config_at(&path, &mut state).unwrap();
        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(loaded.active_profile, "Retro");
        assert_eq!(
            loaded.profiles.get("Retro").unwrap().crt_hard_scan,
            Some(-15.0)
        );

        // 2. Switch to video mode
        let (cmd_tx, _cmd_rx) = crossbeam_channel::unbounded();
        state.video_player = Some(crate::app::models::VideoPlayerState {
            file_path: std::path::PathBuf::from("clip.mp4"),
            duration_secs: 10.0,
            current_position_secs: 2.0,
            is_paused: false,
            is_eof: false,
            is_seeking: false,
            was_playing_before_scrub: false,
            command_sender: cmd_tx,
            position_us: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
            is_paused_shared: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            is_eof_shared: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            stop_requested: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            file_width: 1920,
            file_height: 1080,
            player_thread: None,
        });

        // Tweak settings in video mode
        state.crt.hard_scan = -3.0;
        state.video.vibrance = 2.0;

        // Save config in video mode
        save_config_at(&path, &mut state).unwrap();

        // 3. Verify on disk that video_profile received the changes and Retro was untouched
        let reloaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(reloaded.active_profile, "Retro");
        assert_eq!(
            reloaded.profiles.get("Retro").unwrap().crt_hard_scan,
            Some(-15.0)
        );
        assert_eq!(reloaded.profiles.get("Retro").unwrap().vibrance, Some(1.3));

        let vid_prof = reloaded.video_profile.expect("video_profile must be saved");
        assert_eq!(vid_prof.crt_hard_scan, Some(-3.0));
        assert_eq!(vid_prof.vibrance, Some(2.0));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("toml.bak"));
    }

    #[test]
    fn test_gamescope_profile_isolation() {
        let path = std::env::temp_dir().join(format!(
            "michadame_gamescope_test_{}_{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));

        let mut config = MichadameConfig::default();
        let mut retro_profile = Profile::default();
        retro_profile.crt_hard_scan = Some(-15.0);
        retro_profile.vibrance = Some(1.3);
        config
            .profiles
            .insert("Retro".to_string(), retro_profile.clone());
        config.active_profile = "Retro".to_string();
        store_config_atomic_at(&path, &config).unwrap();

        let mut state = AppState::default();
        apply_config(&mut state, &config);
        assert_eq!(state.active_profile, "Retro");
        assert_eq!(state.crt.hard_scan, -15.0);
        assert_eq!(state.video.vibrance, 1.3);

        // Enter Gamescope mode
        state.gamescope_hook = Some(crate::gamescope::GamescopeHookState {
            is_active: true,
            passthrough_enabled: true,
            ..Default::default()
        });

        // Tweak settings in Gamescope mode
        state.crt.hard_scan = -5.0;
        state.video.vibrance = 1.8;

        // Save config in Gamescope mode
        save_config_at(&path, &mut state).unwrap();

        // Verify on disk that gamescope_profile received the changes and Retro was untouched
        let reloaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(reloaded.active_profile, "Retro");
        assert_eq!(
            reloaded.profiles.get("Retro").unwrap().crt_hard_scan,
            Some(-15.0)
        );
        assert_eq!(reloaded.profiles.get("Retro").unwrap().vibrance, Some(1.3));

        let gs_prof = reloaded
            .gamescope_profile
            .expect("gamescope_profile must be saved");
        assert_eq!(gs_prof.crt_hard_scan, Some(-5.0));
        assert_eq!(gs_prof.vibrance, Some(1.8));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("toml.bak"));
    }

    #[test]
    fn test_webcam_hardware_persistence_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "michadame_webcam_test_{}_{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));
        let mut state = AppState::default();
        state.webcam.config.enabled = true;
        state.webcam.config.device = Some("/dev/video2".to_string());
        state.webcam.config.format_fourcc = Some("MJPG".to_string());
        state.webcam.config.resolution = Some((1280, 720));
        state.webcam.config.framerate = Some(30);
        state.webcam.config.pos_x_pct = 0.55;
        state.webcam.config.pos_y_pct = 0.45;
        state.webcam.config.width_pct = 0.30;
        state.webcam.config.cutout.left = 12;
        state.webcam.config.cutout.right = 14;
        state.webcam.config.cutout.top = 16;
        state.webcam.config.cutout.bottom = 18;

        save_global_hardware_config_at(&path, &state).unwrap();

        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(loaded.webcam.enabled, true);
        assert_eq!(loaded.webcam.device, Some("/dev/video2".to_string()));
        assert_eq!(loaded.webcam.format_fourcc, Some("MJPG".to_string()));
        assert_eq!(loaded.webcam.resolution, Some((1280, 720)));
        assert_eq!(loaded.webcam.framerate, Some(30));
        assert!((loaded.webcam.pos_x_pct - 0.55).abs() < 1e-4);
        assert!((loaded.webcam.pos_y_pct - 0.45).abs() < 1e-4);
        assert!((loaded.webcam.width_pct - 0.30).abs() < 1e-4);
        assert_eq!(loaded.webcam.cutout.left, 12);
        assert_eq!(loaded.webcam.cutout.right, 14);
        assert_eq!(loaded.webcam.cutout.top, 16);
        assert_eq!(loaded.webcam.cutout.bottom, 18);

        let mut fresh_state = AppState::default();
        apply_config(&mut fresh_state, &loaded);
        assert_eq!(fresh_state.webcam.config.enabled, true);
        assert_eq!(
            fresh_state.webcam.config.device,
            Some("/dev/video2".to_string())
        );
        assert_eq!(
            fresh_state.webcam.config.format_fourcc,
            Some("MJPG".to_string())
        );
        assert_eq!(fresh_state.webcam.config.resolution, Some((1280, 720)));
        assert_eq!(fresh_state.webcam.config.framerate, Some(30));
        assert!((fresh_state.webcam.config.pos_x_pct - 0.55).abs() < 1e-4);
        assert!((fresh_state.webcam.config.pos_y_pct - 0.45).abs() < 1e-4);
        assert!((fresh_state.webcam.config.width_pct - 0.30).abs() < 1e-4);
        assert_eq!(fresh_state.webcam.config.cutout.left, 12);
        assert_eq!(fresh_state.webcam.config.cutout.right, 14);
        assert_eq!(fresh_state.webcam.config.cutout.top, 16);
        assert_eq!(fresh_state.webcam.config.cutout.bottom, 18);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("toml.bak"));
    }

    #[test]
    fn test_gamescope_launcher_script_persistence_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "michadame_gs_launcher_test_{}_{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));
        let mut state = AppState::default();
        state.gamescope_launcher_script = "/custom/path/michadame-gamescope-steam.sh".to_string();
        state.selected_steam_game = Some("3259780".to_string());

        save_global_hardware_config_at(&path, &state).unwrap();

        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(
            loaded.gamescope_launcher_script.as_deref(),
            Some("/custom/path/michadame-gamescope-steam.sh")
        );
        assert_eq!(loaded.selected_steam_game.as_deref(), Some("3259780"));

        let mut fresh_state = AppState::default();
        apply_config(&mut fresh_state, &loaded);
        assert_eq!(
            fresh_state.gamescope_launcher_script,
            "/custom/path/michadame-gamescope-steam.sh"
        );
        assert_eq!(fresh_state.selected_steam_game.as_deref(), Some("3259780"));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("toml.bak"));
    }

    #[test]
    fn test_ocr_constant_scanning_persistence_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "michadame_ocr_constant_scan_test_{}_{}.toml",
            std::process::id(),
            crate::replay::now_us()
        ));
        let mut state = AppState::default();
        state.ocr.set_backend(crate::ocr::OcrBackend::LocalScreenAi);
        state.ocr.constant_scanning = true;
        state.ocr.scan_interval_secs = 0.7;
        state.ocr.max_dropoff_secs = 4.5;
        state.ocr.jitter_tolerance = 0.25;

        save_global_hardware_config_at(&path, &state).unwrap();

        let loaded: MichadameConfig = confy::load_path(&path).unwrap();
        assert_eq!(
            loaded.ocr_backend,
            Some(crate::ocr::OcrBackend::LocalScreenAi)
        );
        assert_eq!(loaded.ocr_constant_scanning, Some(true));
        assert!((loaded.ocr_scan_interval_secs.unwrap() - 0.7).abs() < 1e-4);
        assert!((loaded.ocr_max_dropoff_secs.unwrap() - 4.5).abs() < 1e-4);
        assert!((loaded.ocr_jitter_tolerance.unwrap() - 0.25).abs() < 1e-4);

        let mut fresh_state = AppState::default();
        apply_config(&mut fresh_state, &loaded);
        assert_eq!(
            fresh_state.ocr.backend,
            crate::ocr::OcrBackend::LocalScreenAi
        );
        assert!(fresh_state.ocr.constant_scanning);
        assert!(fresh_state.ocr.is_constant_scanning_active());
        assert!((fresh_state.ocr.scan_interval_secs - 0.7).abs() < 1e-4);
        assert!((fresh_state.ocr.max_dropoff_secs - 4.5).abs() < 1e-4);
        assert!((fresh_state.ocr.jitter_tolerance - 0.25).abs() < 1e-4);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("toml.bak"));
    }

    #[test]
    fn test_apply_config_sanitizes_non_finite_floats() {
        for non_finite in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut state = AppState::default();
            let cfg = MichadameConfig {
                master_volume: Some(non_finite),
                ocr_sticky_distance: Some(non_finite),
                ocr_scan_interval_secs: Some(non_finite),
                ocr_max_dropoff_secs: Some(non_finite),
                ocr_jitter_tolerance: Some(non_finite),
                ..Default::default()
            };

            apply_config(&mut state, &cfg);

            assert!(state.hardware.master_volume.is_finite());
            assert_eq!(state.hardware.master_volume, 1.0);
            assert!(state.ocr.sticky_distance.is_finite());
            assert!((state.ocr.sticky_distance - 0.6).abs() < 1e-5);
            assert!(state.ocr.scan_interval_secs.is_finite());
            assert_eq!(
                state.ocr.scan_interval_secs,
                crate::ocr::models::DEFAULT_SCAN_INTERVAL_SECS
            );
            assert!(state.ocr.max_dropoff_secs.is_finite());
            assert_eq!(
                state.ocr.max_dropoff_secs,
                crate::ocr::models::DEFAULT_MAX_DROPOFF_SECS
            );
            assert!(state.ocr.jitter_tolerance.is_finite());
            assert_eq!(
                state.ocr.jitter_tolerance,
                crate::ocr::models::DEFAULT_JITTER_TOLERANCE
            );

            for step in 0..=10 {
                let delay_secs = crate::ocr::OcrState::calculate_dropoff_delay_secs(
                    state.ocr.scan_interval_secs,
                    state.ocr.max_dropoff_secs,
                    step,
                );
                assert!(delay_secs.is_finite() && delay_secs > 0.0);
                state.ocr.consecutive_same_scans = step;
                let duration = state.ocr.current_constant_scan_delay();
                assert!(duration > std::time::Duration::ZERO);
            }
        }
    }

    #[test]
    fn test_apply_profile_sanitizes_non_finite_floats_and_normalizes_fft_mask_name() {
        let mut state = AppState::default();
        let profile = Profile {
            crt_hard_scan: Some(f32::NAN),
            crt_brightboost: Some(f32::INFINITY),
            vibrance: Some(f32::NEG_INFINITY),
            horizontal_stretch: Some(f32::NAN),
            halo_brightboost: Some(f32::NAN),
            cathode_intensity: Some(f32::INFINITY),
            master_volume: Some(f32::NAN),
            fft_filter_enabled: Some(false),
            fft_mask_save_name: Some("my custom mask".to_string()),
            ..Default::default()
        };

        apply_profile_to_state(&mut state, &profile);

        assert_eq!(state.crt.hard_scan, -8.0);
        assert_eq!(state.crt.brightboost, 1.0);
        assert_eq!(state.video.vibrance, 1.0);
        assert_eq!(state.video.horizontal_stretch, 1.0);
        assert_eq!(state.halo.brightboost, state.halo_defaults.brightboost);
        assert_eq!(
            state.cathode_interference.intensity,
            state.cathode_interference_defaults.intensity
        );
        assert_eq!(state.hardware.master_volume, 1.0);
        assert_eq!(state.fft_mask_save_name, "my_custom_mask");

        let built = build_profile_from_state(&state);
        assert_eq!(built.fft_mask_save_name.as_deref(), Some("my_custom_mask"));
    }
}
