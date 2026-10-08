use crate::{
    app::AppState,
    devices::filter_type::CrtFilter,
    video::types::{ColorRange, ScalerFilter},
};
use eframe::egui;
use std::sync::atomic::Ordering;

/// Called only for the root video viewport, after its video paint callback.
/// Floating UI leaves the rendering surface unchanged and out of replay readback.
pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    if ctx.viewport_id() != egui::ViewportId::ROOT {
        return;
    }
    if !ctx.wants_keyboard_input()
        && ctx.input(|i| i.focused)
        && ctx.input_mut(|i| {
            let pressed = i.events.iter().any(|event| matches!(event,
                egui::Event::Key { key: egui::Key::D, pressed: true, repeat: false, modifiers, .. }
                if modifiers.is_none()
            ));
            pressed && i.consume_key(egui::Modifiers::NONE, egui::Key::D)
        })
    {
        state.ui.debug_open = !state.ui.debug_open;
    }
    if !state.ui.debug_open {
        return;
    }
    // Also refresh when capture stalls or has stopped. Never forces capture work.
    ctx.request_repaint_after(std::time::Duration::from_millis(250));
    let mut open = state.ui.debug_open;
    egui::Window::new("Debug")
        .id(egui::Id::new("stream-debug"))
        .open(&mut open)
        .collapsible(false)
        .default_pos([12.0, 40.0])
        .default_width(480.0)
        .max_height((ctx.screen_rect().height() - 64.0).max(100.0))
        .vscroll(true)
        .show(ctx, |ui| {
            ui.strong("Live playback");
            ui.label(format!(
                "UI: {:.1} FPS · received video: {:.1} FPS",
                state.gui_fps, state.video_fps
            ));
            ui.label(format!(
                "Video queue drops: {}",
                state.hardware.video_queue_drops.load(Ordering::Relaxed)
            ));
            if state.ui.video_window_open {
                if let Some(frame) = &state.latest_frame {
                    ui.label(format!(
                        "Decoded: {} × {} · {:?} · {:.3} FPS nominal",
                        frame.width,
                        frame.height,
                        frame.format,
                        frame.rate.num as f64 / frame.rate.den as f64
                    ));
                    ui.label(format!(
                        "Decoded frame: {:.2} MiB · input queue: {} frames",
                        frame.data.len() as f64 / 1_048_576.0,
                        state.frame_receiver.as_ref().map_or(0, |rx| rx.len())
                    ));
                } else {
                    ui.label("Waiting for video");
                }
            } else {
                ui.label("Stream stopped");
            }
            let size = ctx.screen_rect().size() * ctx.pixels_per_point();
            ui.label(format!("Video window: {:.0} × {:.0} px", size.x, size.y));
            if let Some(audio) = &state.hardware.active_audio_stream {
                let stats = &audio.statistics;
                let ms = 1000.0 / audio.sample_rate.max(1) as f64;
                ui.label(format!(
                    "Audio: {} Hz · {} channels · queued: {:.1} ms",
                    audio.sample_rate,
                    audio.channels,
                    stats.queued_frames.load(Ordering::Relaxed) as f64 * ms
                ));
                ui.label(format!(
                    "Estimated audio latency: {} ms",
                    state.hardware.audio_latency_ms.load(Ordering::Relaxed)
                ));
                ui.label(format!(
                    "Playback underrun silence: {:.1} ms · drift drops: {} audio frames",
                    stats.silence_frames.load(Ordering::Relaxed) as f64 * ms,
                    stats.drift_dropped_frames.load(Ordering::Relaxed)
                ));
            } else {
                ui.label("Audio inactive");
            }

            ui.separator();
            ui.strong("Configured stream / filters");
            let hw = &state.hardware;
            let format = hw
                .supported_formats
                .get(hw.selected_format_index)
                .map_or("—", |f| f.fourcc.as_str());
            ui.label(format!(
                "{} · {} · {} × {} @ {} FPS",
                hw.selected_video_device,
                format,
                hw.selected_resolution.0,
                hw.selected_resolution.1,
                hw.selected_framerate
            ));
            ui.label(format!(
                "Audio: {} · {} Hz · {} · buffer {} frames",
                hw.selected_audio_source_name.as_deref().unwrap_or("—"),
                hw.audio_sample_rate,
                hw.audio_sample_format,
                hw.audio_buffer_size
            ));
            ui.label(format!(
                "Profile: {} · CRT: {} · scaler: {}",
                state.active_profile,
                CrtFilter::from_u8(state.crt_filter.load(Ordering::Relaxed)),
                ScalerFilter::from_u8(state.scaler_filter.load(Ordering::Relaxed))
            ));
            ui.label(format!(
                "Color range: {} · stretch: {:.3} · overscan X/Y: {:.3} / {:.3} · underscan X/Y: {:.3} / {:.3}",
                ColorRange::from_u8(state.color_range.load(Ordering::Relaxed)),
                state.video.horizontal_stretch,
                state.video.overscan_x,
                state.video.overscan_y,
                state.video.underscan_x,
                state.video.underscan_y,
            ));
            ui.label(format!(
                "Border cut-off (T/B/L/R): {:.1}% / {:.1}% / {:.1}% / {:.1}%",
                state.video.border_crop_top * 100.0,
                state.video.border_crop_bottom * 100.0,
                state.video.border_crop_left * 100.0,
                state.video.border_crop_right * 100.0,
            ));
            ui.label(format!(
                "Pixelate: {} · FFT: {} · median: {} ({:.2}) · vibrance: {:.2}",
                state.video.pixelate_filter_enabled,
                state.video.fft_filter_enabled,
                state.video.median_filter_enabled,
                state.video.median_mix,
                state.video.vibrance
            ));

            ui.separator();
            ui.strong(if state.replay.runtime.is_some() {
                "Replay · enabled"
            } else {
                "Replay · disabled"
            });
            crate::replay::ui::draw_debug(&state.replay, ui);
        });
    state.ui.debug_open = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exercise input routing with egui only; no window, renderer or devices.
    #[test]
    fn debug_shortcut_is_limited_to_focused_video_viewport() {
        for (viewport, focused, expected) in [
            (egui::ViewportId::ROOT, true, true),
            (egui::ViewportId::ROOT, false, false),
            (
                egui::ViewportId::from_hash_of("control_window"),
                true,
                false,
            ),
        ] {
            let ctx = egui::Context::default();
            let mut state = AppState::default();
            let mut input = egui::RawInput {
                viewport_id: viewport,
                focused,
                ..Default::default()
            };
            input.viewports.entry(viewport).or_default();
            input.events.push(egui::Event::Key {
                key: egui::Key::D,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
            let _ = ctx.run(input, |ctx| draw(&mut state, ctx));
            assert_eq!(state.ui.debug_open, expected);
            if expected {
                // A held key must not repeatedly close/open the overlay.
                let mut input = egui::RawInput::default();
                input.events.push(egui::Event::Key {
                    key: egui::Key::D,
                    physical_key: None,
                    pressed: true,
                    repeat: true,
                    modifiers: egui::Modifiers::NONE,
                });
                let _ = ctx.run(input, |ctx| draw(&mut state, ctx));
                assert!(state.ui.debug_open);
            }
        }
    }
}
