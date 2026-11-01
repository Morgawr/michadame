//! Resolution-aware UI scaling and window fitting for the desktop (non Steam Deck) UI.
//!
//! The desktop UI was designed on a 4K monitor at 100% OS scaling, i.e. one physical
//! pixel per egui point. On smaller monitors the same point sizes take up a much larger
//! share of the screen, so fixed-size windows could end up cut off. This module:
//!
//! * picks an automatic egui zoom factor from the monitor's physical resolution (4K is the
//!   100% reference, smaller screens shrink down to a readability floor), on top of the
//!   OS DPI scale that winit already reports,
//! * allows a manual "UI Scale" override,
//! * clamps window sizes to the monitor so they never extend past the screen, falling back
//!   to resizable windows with scrollbars when the designed size does not fit.

use eframe::egui;

/// Short side (in physical pixels) of the monitor the UI was designed for (4K UHD).
pub const REFERENCE_SHORT_SIDE_PX: f32 = 2160.0;
/// Short side (in physical pixels) at which the automatic scale reaches its floor (1080p).
pub const FLOOR_SHORT_SIDE_PX: f32 = 1080.0;
/// Smallest automatic scale; keeps text readable on 1080p and smaller screens.
pub const MIN_AUTO_SCALE: f32 = 0.75;
/// Largest automatic scale (reached on 8K-class monitors).
pub const MAX_AUTO_SCALE: f32 = 2.0;
/// Range for the manual UI scale override.
pub const MIN_MANUAL_SCALE: f32 = 0.75;
pub const MAX_MANUAL_SCALE: f32 = 2.0;
/// Manual scale presets offered in the settings UI.
pub const MANUAL_SCALE_PRESETS: [f32; 8] = [0.75, 0.85, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// Space (in points) kept free around windows for title bars, panels and taskbars.
pub const WINDOW_SCREEN_MARGIN: egui::Vec2 = egui::vec2(48.0, 112.0);
/// Smallest size a window may be shrunk to when its designed size does not fit.
pub const MIN_FALLBACK_WINDOW_SIZE: egui::Vec2 = egui::vec2(320.0, 240.0);

/// Runtime state of the UI scale (the manual override is persisted in the config).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UiScaleState {
    /// Manual override; `None` means automatic (resolution based).
    pub manual: Option<f32>,
    /// Most recent automatic scale, shown in the settings UI.
    pub auto: Option<f32>,
    /// True during the frame in which a new zoom factor took effect. Child viewports
    /// use this to re-apply their sizes, which are expressed in (now resized) points.
    pub zoom_changed: bool,
    /// True once a frame completed without requesting a zoom change.
    pub settled_once: bool,
    /// Zoom factor that was active while the current frame's input (and therefore its
    /// `monitor_size`, which egui-winit reports in points) was gathered.
    measured_zoom: Option<f32>,
}

/// Clamps a (possibly user-edited) manual scale to the supported range.
pub fn sanitize_manual_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(MIN_MANUAL_SCALE, MAX_MANUAL_SCALE)
    } else {
        1.0
    }
}

/// Automatic UI scale for a monitor with the given physical pixel size.
///
/// Uses the monitor's short side so portrait monitors are treated like their landscape
/// equivalent. Scales linearly: 1080p -> 75%, 1440p -> ~83%, 4K -> 100%, 8K -> 200%.
pub fn auto_ui_scale(monitor_physical_px: egui::Vec2) -> f32 {
    // Note: `f32::min` ignores NaN, so validate both components explicitly.
    if !monitor_physical_px.x.is_finite() || !monitor_physical_px.y.is_finite() {
        return 1.0;
    }
    let short_side = monitor_physical_px.x.min(monitor_physical_px.y);
    if short_side <= 0.0 {
        return 1.0;
    }
    let t = (short_side - FLOOR_SHORT_SIDE_PX) / (REFERENCE_SHORT_SIDE_PX - FLOOR_SHORT_SIDE_PX);
    let scale = MIN_AUTO_SCALE + (1.0 - MIN_AUTO_SCALE) * t;
    // Round to whole percents so tiny reported size differences never cause zoom jitter.
    ((scale * 100.0).round() / 100.0).clamp(MIN_AUTO_SCALE, MAX_AUTO_SCALE)
}

/// Applies the UI scale to the egui context. Call once per frame from the root viewport.
pub fn apply_ui_scale(ctx: &egui::Context, state: &mut UiScaleState) {
    let current_zoom = ctx.zoom_factor();
    // egui-winit gathers `monitor_size` with the zoom factor of the *previous* frame
    // (a newly requested zoom only takes effect when the next frame begins), so convert
    // back to physical pixels using that zoom rather than the current one.
    let measured_zoom = state.measured_zoom.unwrap_or(current_zoom);
    state.measured_zoom = Some(current_zoom);
    state.zoom_changed = (measured_zoom - current_zoom).abs() > 0.001;

    let (monitor_points, native_ppp) = ctx.input(|i| {
        (
            i.viewport().monitor_size,
            i.viewport().native_pixels_per_point,
        )
    });
    if let Some(monitor_points) = monitor_points {
        let ppp = measured_zoom * native_ppp.unwrap_or(1.0);
        state.auto = Some(auto_ui_scale(monitor_points * ppp));
    }

    let target = match state.manual {
        Some(manual) => Some(sanitize_manual_scale(manual)),
        // No monitor information yet: keep whatever zoom is active.
        None => state.auto,
    };
    let requested_change = target.is_some_and(|target| (current_zoom - target).abs() > 0.001);
    if let (true, Some(target)) = (requested_change, target) {
        ctx.set_zoom_factor(target);
    }
    state.settled_once |= !requested_change;
}

/// Re-applies a fitted window size to the current (child) viewport after the zoom factor
/// changed. Viewport sizes are specified in points, and egui only sends size commands when
/// the builder values change, so without this a window would keep its old pixel size.
pub fn reapply_window_size_on_zoom_change(
    ctx: &egui::Context,
    scale: &UiScaleState,
    fitted: FittedWindow,
    desired: egui::Vec2,
) {
    if !scale.zoom_changed {
        return;
    }
    if fitted.fits {
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(fitted.size));
        ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(fitted.size));
    } else {
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(
            MIN_FALLBACK_WINDOW_SIZE.min(fitted.size),
        ));
        ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(desired));
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(fitted.size));
}

/// Result of fitting a window's designed size onto the current monitor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FittedWindow {
    /// Size to open the window with (points).
    pub size: egui::Vec2,
    /// Whether the designed size fits on the monitor. When it does not, the window should
    /// be resizable and its contents scrollable.
    pub fits: bool,
}

/// Fits a window of `desired` size (points) onto a monitor of `monitor` size (points).
pub fn fit_window(desired: egui::Vec2, monitor: Option<egui::Vec2>) -> FittedWindow {
    let Some(monitor) = monitor.filter(|m| m.x.is_finite() && m.y.is_finite()) else {
        return FittedWindow {
            size: desired,
            fits: true,
        };
    };
    let available = (monitor - WINDOW_SCREEN_MARGIN).max(MIN_FALLBACK_WINDOW_SIZE);
    let fits = desired.x <= available.x && desired.y <= available.y;
    FittedWindow {
        size: desired.min(available),
        fits,
    }
}

/// Builds a viewport for a window with a designed, fixed size. If the size fits on the
/// monitor the window keeps its fixed, non-resizable size; otherwise it is shrunk to the
/// screen and made resizable so its (scrollable) contents remain reachable.
pub fn fixed_size_viewport(
    builder: egui::ViewportBuilder,
    desired: egui::Vec2,
    monitor: Option<egui::Vec2>,
) -> (egui::ViewportBuilder, FittedWindow) {
    let fitted = fit_window(desired, monitor);
    let builder = builder.with_inner_size(fitted.size);
    let builder = if fitted.fits {
        builder
            .with_min_inner_size(fitted.size)
            .with_max_inner_size(fitted.size)
            .with_resizable(false)
    } else {
        builder
            .with_min_inner_size(MIN_FALLBACK_WINDOW_SIZE.min(fitted.size))
            .with_max_inner_size(desired)
            .with_resizable(true)
    };
    (builder, fitted)
}

/// Monitor size (points) of the viewport `ctx` currently belongs to.
pub fn monitor_size(ctx: &egui::Context) -> Option<egui::Vec2> {
    ctx.input(|i| i.viewport().monitor_size)
}

/// Largest size (points) an in-window popup/dialog may take before it has to scroll.
pub fn max_dialog_size(ctx: &egui::Context) -> egui::Vec2 {
    (ctx.screen_rect().size() - egui::vec2(32.0, 48.0)).max(egui::vec2(200.0, 120.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_scale_reference_points() {
        assert_eq!(auto_ui_scale(egui::vec2(3840.0, 2160.0)), 1.0);
        assert_eq!(auto_ui_scale(egui::vec2(1920.0, 1080.0)), 0.75);
        assert_eq!(auto_ui_scale(egui::vec2(2560.0, 1440.0)), 0.83);
        // Portrait monitors use their short side.
        assert_eq!(auto_ui_scale(egui::vec2(1440.0, 2560.0)), 0.83);
        // Floor and ceiling.
        assert_eq!(auto_ui_scale(egui::vec2(1280.0, 720.0)), MIN_AUTO_SCALE);
        assert_eq!(auto_ui_scale(egui::vec2(15360.0, 8640.0)), MAX_AUTO_SCALE);
        assert_eq!(auto_ui_scale(egui::vec2(0.0, 0.0)), 1.0);
        assert_eq!(auto_ui_scale(egui::vec2(f32::NAN, 100.0)), 1.0);
    }

    #[test]
    fn manual_scale_is_sanitized() {
        assert_eq!(sanitize_manual_scale(5.0), MAX_MANUAL_SCALE);
        assert_eq!(sanitize_manual_scale(0.1), MIN_MANUAL_SCALE);
        assert_eq!(sanitize_manual_scale(f32::NAN), 1.0);
        assert_eq!(sanitize_manual_scale(1.25), 1.25);
    }

    #[test]
    fn fit_window_keeps_designed_size_when_it_fits() {
        let fitted = fit_window(egui::vec2(960.0, 770.0), Some(egui::vec2(2560.0, 1440.0)));
        assert!(fitted.fits);
        assert_eq!(fitted.size, egui::vec2(960.0, 770.0));
        let fitted = fit_window(egui::vec2(960.0, 770.0), None);
        assert!(fitted.fits);
    }

    #[test]
    fn fit_window_clamps_to_monitor() {
        let fitted = fit_window(egui::vec2(960.0, 770.0), Some(egui::vec2(1366.0, 768.0)));
        assert!(!fitted.fits);
        assert_eq!(
            fitted.size,
            egui::vec2(960.0, 768.0 - WINDOW_SCREEN_MARGIN.y)
        );
        // Never smaller than the fallback minimum, even on absurdly small monitors.
        let fitted = fit_window(egui::vec2(960.0, 770.0), Some(egui::vec2(100.0, 100.0)));
        assert!(!fitted.fits);
        assert_eq!(fitted.size, MIN_FALLBACK_WINDOW_SIZE);
    }

    #[test]
    fn apply_ui_scale_uses_manual_override() {
        let ctx = egui::Context::default();
        let mut state = UiScaleState {
            manual: Some(1.5),
            ..Default::default()
        };
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            apply_ui_scale(ctx, &mut state);
        });
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        assert_eq!(ctx.zoom_factor(), 1.5);
    }

    #[test]
    fn apply_ui_scale_is_stable_across_zoom_changes() {
        // Simulates egui-winit reporting the monitor size in points using the zoom factor
        // that was active before the frame started.
        let ctx = egui::Context::default();
        let mut state = UiScaleState::default();
        let physical = egui::vec2(1920.0, 1080.0);
        for _ in 0..5 {
            let mut input = egui::RawInput::default();
            let viewport = input.viewports.entry(egui::ViewportId::ROOT).or_default();
            viewport.native_pixels_per_point = Some(1.0);
            viewport.monitor_size = Some(physical / ctx.zoom_factor());
            let _ = ctx.run(input, |ctx| apply_ui_scale(ctx, &mut state));
            assert_eq!(state.auto, Some(0.75));
        }
        assert_eq!(ctx.zoom_factor(), 0.75);
    }
}
