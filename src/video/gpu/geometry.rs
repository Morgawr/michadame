/// Bounds of the final video image in physical framebuffer pixels, with an
/// OpenGL bottom-left origin. Outer aspect-fit padding is excluded; shader
/// effects and overscan offsets inside the image are preserved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderedArea {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Shared with the Anime4K renderer and UI geometry; only integer 2x steps are used.
pub fn anime4k_size(source: (u32, u32), target: (u32, u32)) -> (u32, u32) {
    let (mut w, mut h) = source;
    if w == 0 || h == 0 || target.0 == 0 || target.1 == 0 {
        return source;
    }
    while w as u64 * 6 < target.0 as u64 * 5 && h as u64 * 6 < target.1 as u64 * 5 {
        w *= 2;
        h *= 2;
    }
    (w, h)
}

pub fn pixelate_size(source: (u32, u32)) -> (u32, u32) {
    if source.0 == 0 || source.1 == 0 {
        return (1, 1);
    }
    let height = source.1.clamp(1, 480);
    let width = (source.0 as u64 * height as u64)
        .div_ceil(source.1 as u64)
        .max(1) as u32;
    (width, height)
}

/// Geometry for the clean OCR pass: the same source sampling coordinates as
/// the displayed CRT image, without scanlines, masks, bloom, or color effects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OcrProjection {
    pub warp: [f32; 2],
    pub zoom: f32,
}

impl Default for OcrProjection {
    fn default() -> Self {
        Self {
            warp: [0.0; 2],
            zoom: 1.0,
        }
    }
}

impl OcrProjection {
    pub fn lottes(warp_x: f32, warp_y: f32) -> Self {
        Self {
            warp: [warp_x, warp_y],
            zoom: 1.0,
        }
    }

    pub fn halo(curvature: bool, zoom_percent: f32) -> Self {
        Self {
            warp: if curvature { [0.031, 0.041] } else { [0.0; 2] },
            zoom: zoom_percent / 100.0,
        }
    }
}

impl RenderedArea {
    /// Convert the bottom-origin readback inset to a top-origin UI inset.
    /// Odd letterbox padding need not have equal upper and lower halves.
    pub fn top(self, framebuffer_height: u32) -> u32 {
        framebuffer_height.saturating_sub(self.y.saturating_add(self.height))
    }

    pub fn fit(source: (u32, u32), output: (f32, f32), horizontal_stretch: f32) -> Self {
        if source.0 == 0
            || source.1 == 0
            || !output.0.is_finite()
            || !output.1.is_finite()
            || output.0 < 1.
            || output.1 < 1.
            || !horizontal_stretch.is_finite()
            || horizontal_stretch <= 0.
        {
            return Self::default();
        }
        // Match fs_final/fs_passthrough's aspect fit, using the actual final
        // texture size (including upscaling/pixelation), not the capture format.
        let video_aspect = source.0 as f32 * horizontal_stretch / source.1 as f32;
        let output_aspect = output.0 / output.1;
        let (scale_x, scale_y) = if video_aspect > output_aspect {
            (1., output_aspect / video_aspect)
        } else {
            (video_aspect / output_aspect, 1.)
        };
        // The renderer truncates its GL viewport to physical integer pixels.
        // Readback must use that same viewport; round the fitted image once.
        let viewport = (output.0 as u32, output.1 as u32);
        let width = ((viewport.0 as f32 * scale_x).round() as u32).clamp(1, viewport.0);
        let height = ((viewport.1 as f32 * scale_y).round() as u32).clamp(1, viewport.1);
        Self {
            x: (viewport.0 - width) / 2,
            y: (viewport.1 - height) / 2,
            width,
            height,
        }
    }

    pub fn full(output: (f32, f32)) -> Self {
        if !output.0.is_finite() || !output.1.is_finite() || output.0 < 1. || output.1 < 1. {
            return Self::default();
        }
        Self {
            x: 0,
            y: 0,
            width: output.0 as u32,
            height: output.1 as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixelated_and_upscaled_image_bounds_use_the_final_integer_texture() {
        assert_eq!(pixelate_size((1920, 1080)), (854, 480));
        let fitted = RenderedArea::fit(pixelate_size((1920, 1080)), (1280.0, 801.0), 1.0);
        assert_eq!((fitted.width, fitted.height), (1280, 719));
        let upscaled = anime4k_size((301, 241), (1280, 801));
        assert_eq!(upscaled, (1204, 964));
        assert_eq!(pixelate_size(upscaled), (600, 480));
    }

    #[test]
    fn ui_top_matches_the_flipped_readback_with_odd_padding() {
        for height in [800, 801, 803, 1081] {
            let area = RenderedArea::fit((1920, 1080), (1280.0, height as f32), 1.0);
            assert_eq!(area.top(height) + area.height + area.y, height);
            for ppp in [1.0, 1.25, 1.5, 13.0 / 6.0] {
                let ui_top = area.top(height) as f32 / ppp;
                assert!((ui_top * ppp - (height - area.y - area.height) as f32).abs() < 0.001);
            }
        }
        let odd = RenderedArea::fit((1920, 1080), (1280.0, 801.0), 1.0);
        assert_eq!((odd.y, odd.top(801)), (40, 41));
    }

    #[test]
    fn four_by_three_uses_rendered_pixels_without_pillarboxes() {
        assert_eq!(
            RenderedArea::fit((640, 480), (1920., 1080.), 1.),
            RenderedArea {
                x: 240,
                y: 0,
                width: 1440,
                height: 1080
            }
        );
        assert_eq!(
            RenderedArea::fit((1920, 1440), (1920., 1080.), 1.),
            RenderedArea::fit((640, 480), (1920., 1080.), 1.),
            "Upscaling must not change the final aspect-fitted bounds"
        );
    }

    #[test]
    fn widescreen_and_portrait_remove_only_outer_padding() {
        assert_eq!(
            RenderedArea::fit((1920, 1080), (1280., 1024.), 1.),
            RenderedArea {
                x: 0,
                y: 152,
                width: 1280,
                height: 720
            }
        );
        assert_eq!(
            RenderedArea::fit((480, 640), (1920., 1080.), 1.),
            RenderedArea {
                x: 555,
                y: 0,
                width: 810,
                height: 1080
            }
        );
    }

    #[test]
    fn stretch_changes_the_recorded_aspect_ratio_in_both_fit_directions() {
        assert_eq!(
            RenderedArea::fit((1920, 1080), (1920., 1080.), 0.75),
            RenderedArea::fit((640, 480), (1920., 1080.), 1.),
            "A widescreen feed corrected to 4:3 must record at 4:3"
        );
        assert_eq!(
            RenderedArea::fit((640, 480), (1920., 1080.), 1.25),
            RenderedArea {
                x: 60,
                y: 0,
                width: 1800,
                height: 1080
            }
        );
        assert_eq!(
            RenderedArea::fit((640, 480), (1920., 1080.), 1.5),
            RenderedArea {
                x: 0,
                y: 60,
                width: 1920,
                height: 960
            }
        );
    }

    #[test]
    fn physical_pixels_odd_sizes_and_fractional_dpi_stay_inside_the_viewport() {
        for output in [
            (1280., 720.),
            (1920., 1080.),
            (1921., 1081.),
            (1500.75, 1000.25),
        ] {
            let area = RenderedArea::fit((640, 480), output, 1.);
            assert!(area.x + area.width <= output.0 as u32);
            assert!(area.y + area.height <= output.1 as u32);
            assert!((area.width as f32 - area.height as f32 * 4. / 3.).abs() <= 1.);
            assert!((output.0 as i64 - area.width as i64 - 2 * area.x as i64).abs() <= 1);
            assert!((output.1 as i64 - area.height as i64 - 2 * area.y as i64).abs() <= 1);
        }
        assert_eq!(
            RenderedArea::fit((1920, 1080), (1920., 1080.), 1.),
            RenderedArea {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
    }

    #[test]
    fn missing_or_invalid_geometry_has_no_readback_area() {
        for (source, output, stretch) in [
            ((0, 480), (1920., 1080.), 1.),
            ((640, 480), (0., 0.), 1.),
            ((640, 480), (1920., f32::NAN), 1.),
            ((640, 480), (1920., 1080.), 0.),
            ((640, 480), (1920., 1080.), f32::INFINITY),
        ] {
            assert_eq!(
                RenderedArea::fit(source, output, stretch),
                RenderedArea::default()
            );
        }
    }

    #[test]
    fn full_window_uses_entire_output_dimensions() {
        assert_eq!(
            RenderedArea::full((1920., 1080.)),
            RenderedArea {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }
        );
        assert_eq!(
            RenderedArea::full((1280.7, 720.2)),
            RenderedArea {
                x: 0,
                y: 0,
                width: 1280,
                height: 720,
            }
        );
        assert_eq!(RenderedArea::full((0., 1080.)), RenderedArea::default());
        assert_eq!(
            RenderedArea::full((1920., f32::NAN)),
            RenderedArea::default()
        );
    }
}
