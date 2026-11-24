use super::anime4k::{Anime4kUpscaler, Anime4kVariant};
use super::beam::BeamRenderer;
use super::fft_filter::FftFilter;
use super::framegen::{FfxFrameGen, FfxFrameGenRunInfo, FrameGenPaintRequest};
use super::fsr1::{Fsr1RunInfo, Fsr1Upscaler};
use super::geometry::{OcrProjection, RenderedArea};
use super::halo::HaloRenderer;
use super::params::{
    AudioReactiveShaderParams, BeamShaderParams, CathodeInterferenceShaderParams,
    GlassShaderParams, HaloShaderParams, ShaderParams,
};
use super::programs::*;
use crate::video::types::{RawFrame, ScalerFilter};
use eframe::egui;
use eframe::glow::{self, HasContext};
use ffmpeg_next::format::Pixel;
use std::num::NonZero;
use std::sync::{Arc, Mutex};

pub const RETRO_CURSOR_WIDTH: u32 = 16;
pub const RETRO_CURSOR_HEIGHT: u32 = 24;

const PASS_HIST_1: usize = 0;
const PASS_DEINTERLACE: usize = 1;
const PASS_HIST_2: usize = 2;
const PASS_PIXELATE: usize = 3;
const PASS_MEDIAN: usize = 4;
const PASS_INPUT: usize = 5;
const NUM_INTERMEDIATE_PASSES: usize = 6;
const HISTORY_SLOTS: [usize; 3] = [PASS_INPUT, PASS_HIST_1, PASS_HIST_2];

const RETRO_CURSOR_SPRITE: [&str; 24] = [
    "B...............",
    "BB..............",
    "BWB.............",
    "BWWBS...........",
    "BWWWWBS.........",
    "BWWWWWBS........",
    "BWWWWWWBS.......",
    "BWWWWWWWBS......",
    "BWWWWWWWWBS.....",
    "BWWWWWWWWWBS....",
    "BWWWWWWWWWWBS...",
    "BWWWWWWWWWWWBS..",
    "BWWWWWWBBBBBSS..",
    "BWWWBWWBS.......",
    "BWWB.BWWB.......",
    "BWB..BWWB.......",
    "BB....BWWB......",
    "B.....BWWB......",
    ".......BWWB.....",
    ".......BWWB.....",
    "........BB......",
    "................",
    "................",
    "................",
];

fn generate_retro_cursor_pixels() -> [u8; (RETRO_CURSOR_WIDTH * RETRO_CURSOR_HEIGHT * 4) as usize] {
    let mut pixels = [0u8; (RETRO_CURSOR_WIDTH * RETRO_CURSOR_HEIGHT * 4) as usize];
    for (y, row) in RETRO_CURSOR_SPRITE.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            if x >= RETRO_CURSOR_WIDTH as usize || y >= RETRO_CURSOR_HEIGHT as usize {
                continue;
            }
            let idx = (y * RETRO_CURSOR_WIDTH as usize + x) * 4;
            match ch {
                'B' => {
                    pixels[idx] = 0;
                    pixels[idx + 1] = 0;
                    pixels[idx + 2] = 0;
                    pixels[idx + 3] = 255;
                }
                'W' => {
                    pixels[idx] = 255;
                    pixels[idx + 1] = 255;
                    pixels[idx + 2] = 255;
                    pixels[idx + 3] = 255;
                }
                'S' => {
                    pixels[idx] = 0;
                    pixels[idx + 1] = 0;
                    pixels[idx + 2] = 0;
                    pixels[idx + 3] = 75;
                }
                _ => {
                    pixels[idx] = 0;
                    pixels[idx + 1] = 0;
                    pixels[idx + 2] = 0;
                    pixels[idx + 3] = 0;
                }
            }
        }
    }
    pixels
}

pub struct CrtFilterRenderer {
    passthrough_prog: glow::Program,
    pixelate_prog: glow::Program,
    median_prog: glow::Program,
    final_prog: glow::Program,
    yuv_planar_prog: glow::Program,
    yuyv_packed_prog: glow::Program,
    yuv_range_loc: glow::UniformLocation,
    yuyv_range_loc: glow::UniformLocation,
    yuv_overscan_loc: glow::UniformLocation,
    yuyv_overscan_loc: glow::UniformLocation,
    yuv_underscan_loc: glow::UniformLocation,
    yuyv_underscan_loc: glow::UniformLocation,
    yuv_brightness_loc: glow::UniformLocation,
    yuyv_brightness_loc: glow::UniformLocation,
    yuv_gamma_loc: glow::UniformLocation,
    yuyv_gamma_loc: glow::UniformLocation,
    median_mix_loc: glow::UniformLocation,
    deinterlace_prog: glow::Program,
    deinterlace_current_frame_loc: Option<glow::UniformLocation>,
    deinterlace_prev_frame_loc: Option<glow::UniformLocation>,
    deinterlace_prev_frame_2_loc: Option<glow::UniformLocation>,
    deinterlace_has_prev2_loc: Option<glow::UniformLocation>,
    deinterlace_mode_loc: glow::UniformLocation,
    deinterlace_blend_loc: glow::UniformLocation,
    deinterlace_motion_thresh_loc: glow::UniformLocation,
    deinterlace_line_spacing_loc: glow::UniformLocation,
    deinterlace_spatial_mix_loc: glow::UniformLocation,
    anime4k_small: Anime4kUpscaler,
    anime4k_medium: Anime4kUpscaler,
    anime4k_large: Anime4kUpscaler,
    fsr1: Fsr1Upscaler,
    framegen: FfxFrameGen,
    pub halo: HaloRenderer,
    pub beam: BeamRenderer,

    fbos: [glow::Framebuffer; NUM_INTERMEDIATE_PASSES],
    pass_textures: [glow::Texture; NUM_INTERMEDIATE_PASSES],
    yuv_planes: [glow::Texture; 3],
    pbos: [glow::Buffer; 3],
    vertex_array: glow::VertexArray,
    vbo: glow::Buffer,

    p_passthrough_output_res_loc: glow::UniformLocation,
    p_pixelate_target_res_loc: glow::UniformLocation,

    final_output_res_loc: glow::UniformLocation,
    final_hard_scan_loc: glow::UniformLocation,
    final_hard_pix_loc: glow::UniformLocation,
    final_warp_x_loc: glow::UniformLocation,
    final_warp_y_loc: glow::UniformLocation,
    final_shadow_mask_loc: glow::UniformLocation,
    final_brightboost_loc: glow::UniformLocation,
    final_hard_bloom_pix_loc: glow::UniformLocation,
    final_hard_bloom_scan_loc: glow::UniformLocation,
    final_bloom_amount_loc: glow::UniformLocation,
    final_shape_loc: glow::UniformLocation,
    final_background_color_loc: glow::UniformLocation,
    passthrough_background_color_loc: glow::UniformLocation,
    final_horizontal_stretch_loc: glow::UniformLocation,
    passthrough_horizontal_stretch_loc: glow::UniformLocation,
    final_vibrance_loc: glow::UniformLocation,
    passthrough_vibrance_loc: glow::UniformLocation,
    passthrough_scaler_filter_loc: glow::UniformLocation,
    final_border_crop_loc: glow::UniformLocation,
    passthrough_border_crop_loc: glow::UniformLocation,
    passthrough_input_is_srgb_loc: Option<glow::UniformLocation>,
    passthrough_ocr_warp_loc: glow::UniformLocation,
    passthrough_ocr_zoom_loc: glow::UniformLocation,
    passthrough_image_aspect_loc: glow::UniformLocation,

    cathode_prog: glow::Program,
    cathode_output_res_loc: glow::UniformLocation,
    cathode_source_size_loc: glow::UniformLocation,
    cathode_horizontal_stretch_loc: glow::UniformLocation,
    cathode_time_loc: glow::UniformLocation,
    cathode_intensity_loc: glow::UniformLocation,
    cathode_frequency_loc: glow::UniformLocation,
    cathode_randomization_loc: glow::UniformLocation,
    cathode_electricity_glow_loc: glow::UniformLocation,
    cathode_flicker_depth_loc: glow::UniformLocation,
    cathode_interference_loc: glow::UniformLocation,
    cathode_lightbulb_effect_loc: glow::UniformLocation,
    cathode_border_crop_loc: glow::UniformLocation,

    audio_reactive_prog: glow::Program,
    ar_output_res_loc: Option<glow::UniformLocation>,
    ar_source_size_loc: Option<glow::UniformLocation>,
    ar_horizontal_stretch_loc: Option<glow::UniformLocation>,
    ar_border_crop_loc: Option<glow::UniformLocation>,
    ar_warp_loc: Option<glow::UniformLocation>,
    ar_corner_size_loc: Option<glow::UniformLocation>,
    ar_filter_type_loc: Option<glow::UniformLocation>,
    ar_time_loc: Option<glow::UniformLocation>,
    ar_master_intensity_loc: Option<glow::UniformLocation>,
    ar_low_env_loc: Option<glow::UniformLocation>,
    ar_mid_env_loc: Option<glow::UniformLocation>,
    ar_high_env_loc: Option<glow::UniformLocation>,
    ar_transient_env_loc: Option<glow::UniformLocation>,
    ar_stereo_balance_loc: Option<glow::UniformLocation>,
    ar_spring_pos_loc: Option<glow::UniformLocation>,
    ar_phase_accum_loc: Option<glow::UniformLocation>,
    ar_deflection_shake_loc: Option<glow::UniformLocation>,
    ar_stereo_tug_loc: Option<glow::UniformLocation>,
    ar_flyback_pump_loc: Option<glow::UniformLocation>,
    ar_rgb_deconvergence_loc: Option<glow::UniformLocation>,
    ar_scanline_wave_loc: Option<glow::UniformLocation>,
    ar_phosphor_surge_loc: Option<glow::UniformLocation>,
    ar_sync_tear_loc: Option<glow::UniformLocation>,

    retro_frame_prog: glow::Program,
    bezel_texture: glow::Texture,
    retro_output_res_loc: Option<glow::UniformLocation>,
    retro_source_size_loc: Option<glow::UniformLocation>,
    retro_horizontal_stretch_loc: Option<glow::UniformLocation>,
    retro_border_crop_loc: Option<glow::UniformLocation>,
    retro_warp_loc: Option<glow::UniformLocation>,
    retro_corner_size_loc: Option<glow::UniformLocation>,
    retro_filter_type_loc: Option<glow::UniformLocation>,
    retro_time_loc: Option<glow::UniformLocation>,
    retro_ambient_glow_loc: Option<glow::UniformLocation>,
    retro_dark_mode_loc: Option<glow::UniformLocation>,
    last_video_texture: Option<glow::Texture>,
    last_video_texture_res: (u32, u32),
    last_video_tex_base_filter: i32,
    current_source_frame_id: i64,
    last_mipmapped_frame: std::cell::Cell<Option<(glow::Texture, i64, (u32, u32))>>,
    mipmap_generated_this_paint: std::cell::Cell<bool>,
    last_pixelate_key: Option<(glow::Texture, i64, (u32, u32))>,

    crt_glass_prog: glow::Program,
    glass_output_res_loc: Option<glow::UniformLocation>,
    glass_source_size_loc: Option<glow::UniformLocation>,
    glass_horizontal_stretch_loc: Option<glow::UniformLocation>,
    glass_border_crop_loc: Option<glow::UniformLocation>,
    glass_warp_loc: Option<glow::UniformLocation>,
    glass_corner_size_loc: Option<glow::UniformLocation>,
    glass_filter_type_loc: Option<glow::UniformLocation>,
    glass_intensity_loc: Option<glow::UniformLocation>,
    glass_glossiness_loc: Option<glow::UniformLocation>,
    glass_time_loc: Option<glow::UniformLocation>,
    glass_ceiling_light_enabled_loc: Option<glow::UniformLocation>,
    glass_photographer_enabled_loc: Option<glow::UniformLocation>,
    glass_photographer_intensity_loc: Option<glow::UniformLocation>,
    glass_flash_enabled_loc: Option<glow::UniformLocation>,
    glass_flash_intensity_loc: Option<glow::UniformLocation>,
    silhouette_texture: glow::Texture,

    night_glow_prog: glow::Program,
    night_glow_output_res_loc: Option<glow::UniformLocation>,
    night_glow_source_size_loc: Option<glow::UniformLocation>,
    night_glow_horizontal_stretch_loc: Option<glow::UniformLocation>,
    night_glow_border_crop_loc: Option<glow::UniformLocation>,
    night_glow_warp_loc: Option<glow::UniformLocation>,
    night_glow_corner_size_loc: Option<glow::UniformLocation>,
    night_glow_filter_type_loc: Option<glow::UniformLocation>,
    night_glow_intensity_loc: Option<glow::UniformLocation>,

    pub popup_primitives: Vec<egui::ClippedPrimitive>,
    /// Niconico chat comments, drawn under the CRT post effects and kept in
    /// replays/mining captures.
    pub comment_primitives: Vec<egui::ClippedPrimitive>,
    /// Twitch video alerts, drawn under the CRT post effects and kept in
    /// replays/mining captures.
    pub alert_primitives: Vec<egui::ClippedPrimitive>,
    popup_prog: glow::Program,
    popup_vao: glow::VertexArray,
    popup_vbo: glow::Buffer,
    popup_ebo: glow::Buffer,
    overlay_vertices_scratch: Vec<egui::epaint::Vertex>,
    overlay_indices_scratch: Vec<u32>,
    overlay_draws_scratch: Vec<OverlayDrawCmd>,
    popup_screen_size_loc: Option<glow::UniformLocation>,
    popup_offset_loc: Option<glow::UniformLocation>,
    popup_sampler_loc: Option<glow::UniformLocation>,
    popup_shadow_mask_loc: Option<glow::UniformLocation>,
    popup_scanline_strength_loc: Option<glow::UniformLocation>,
    popup_scanline_freq_loc: Option<glow::UniformLocation>,
    popup_brightboost_loc: Option<glow::UniformLocation>,
    blit_prog: glow::Program,
    retro_mouse_prog: glow::Program,
    retro_mouse_texture: glow::Texture,
    retro_mouse_screen_size_loc: Option<glow::UniformLocation>,
    retro_mouse_cursor_pos_loc: Option<glow::UniformLocation>,
    retro_mouse_cursor_size_loc: Option<glow::UniformLocation>,
    retro_mouse_sampler_loc: Option<glow::UniformLocation>,
    retro_mouse_shadow_mask_loc: Option<glow::UniformLocation>,
    retro_mouse_scanline_strength_loc: Option<glow::UniformLocation>,
    retro_mouse_scanline_freq_loc: Option<glow::UniformLocation>,
    retro_mouse_brightboost_loc: Option<glow::UniformLocation>,
    linearize_prog: glow::Program,
    linearize_sampler_loc: Option<glow::UniformLocation>,
    linearize_overscan_loc: Option<glow::UniformLocation>,
    linearize_underscan_loc: Option<glow::UniformLocation>,
    linearize_brightness_loc: Option<glow::UniformLocation>,
    linearize_gamma_loc: Option<glow::UniformLocation>,

    post_fbos: [glow::Framebuffer; 2],
    post_textures: [glow::Texture; 2],
    last_post_size: (u32, u32),

    last_size: (u32, u32),
    last_scaler_filter: Option<u8>,
    last_pass_res: (u32, u32),
    last_frame_size: (u32, u32),
    last_frame_format: Option<Pixel>,
    last_frame_captured_at: i64,
    prev_frame_captured_at: i64,
    last_input_color_range: Option<crate::video::types::ColorRange>,
    last_input_scan: (f32, f32, f32, f32),
    last_input_color_adj: (f32, f32),
    last_fft_enabled: bool,
    last_fft_thresholds: (f32, f32),
    last_fft_mask_gen: u64,
    last_fft_spectrum_rendered: bool,
    last_input_tex: Option<glow::Texture>,
    last_input_slot: Option<usize>,
    last_source_kind: Option<InputSourceKind>,
    last_fallback_tex: Option<glow::Texture>,
    last_preprocess_sig: Option<PreprocessCacheSig>,
    last_preprocess_out: Option<(glow::Texture, (u32, u32), bool)>,
    /// Un-interpolated pre-framegen texture `(tex, res, is_srgb)` so async/constant OCR always
    /// captures the crisp real PS5 frame without interrupting 60 FPS framegen on display/Replay.
    last_real_video_texture: Option<(glow::Texture, (u32, u32), bool)>,
    last_fsr1_active: bool,
    last_framegen_active: bool,
    history_ring: DeinterlaceHistoryRing,
    pub fallback_captured_at_next_paint: Option<i64>,
    pub framegen_next_paint: Option<FrameGenPaintRequest>,
    pub render_fft_spectrum_next_paint: bool,
    pub capture_ocr_next_paint: bool,
    pub capture_ocr_async_next_paint: bool,
    pub ocr_recycled_buffer: Option<Vec<u8>>,
    pub last_ocr_capture: Option<(Vec<u8>, u32, u32)>,
    ocr_fbo: glow::Framebuffer,
    ocr_texture: glow::Texture,
    last_ocr_fbo_size: (u32, u32),
    ocr_pbo: glow::Buffer,
    ocr_pbo_size: usize,
    ocr_pending_readback: Option<PendingOcrReadback>,
}

struct PendingOcrReadback {
    fence: glow::Fence,
    width: u32,
    height: u32,
}

// SAFETY: `glow::Fence` is an opaque GL sync handle only accessed on the main thread
// while the owning GL context is current (in `paint`, `draw_passthrough`, and `destroy`).
unsafe impl Send for PendingOcrReadback {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputSourceKind {
    Raw,
    Fallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreprocessSourceSig {
    Raw {
        captured_at: i64,
    },
    Fallback {
        tex: glow::Texture,
        captured_at: i64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreprocessCacheSig {
    source: PreprocessSourceSig,
    resolution: (u32, u32),
    effective_res: (u32, u32),
    scaler_filter: u8,
    fsr1_sharpness: u32,
    allow_srgb_passthrough_fastpath: bool,
    scan_and_color: [u32; 6],
    deinterlace_enabled: bool,
    deinterlace_mode: u8,
    deinterlace_params: [u32; 4],
    framegen_enabled: bool,
    framegen_show_interpolated: bool,
    framegen_params: [u32; 3],
    median_enabled: bool,
    median_mix: u32,
    fft_enabled: bool,
    fft_thresholds: [u32; 2],
    fft_gen: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DeinterlaceHistoryRing {
    slots: [usize; 3],
    head: usize,
    count: u32,
}

impl DeinterlaceHistoryRing {
    const fn new(slots: [usize; 3]) -> Self {
        Self {
            slots,
            head: 0,
            count: 0,
        }
    }

    fn reset(&mut self) {
        self.head = 0;
        self.count = 0;
    }

    fn select_write_slot(&mut self, deinterlace_enabled: bool, is_new_frame: bool) -> usize {
        if !deinterlace_enabled {
            self.reset();
            return self.slots[0];
        }
        if is_new_frame {
            if self.count >= 1 {
                self.head = (self.head + 1) % 3;
            }
            self.count = self.count.saturating_add(1);
        } else if self.count == 0 {
            self.count = 1;
        }
        self.slots[self.head]
    }

    fn active_slots(&self) -> (usize, usize, usize, bool) {
        let curr = self.slots[self.head];
        if self.count <= 1 {
            (curr, curr, curr, false)
        } else if self.count == 2 {
            let prev1 = self.slots[(self.head + 2) % 3];
            (curr, prev1, prev1, false)
        } else {
            let prev1 = self.slots[(self.head + 2) % 3];
            let prev2 = self.slots[(self.head + 1) % 3];
            (curr, prev1, prev2, true)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OverlayDrawCmd {
    texture_id: egui::TextureId,
    scissor: [i32; 4],
    first_index: usize,
    index_count: i32,
}

fn intersect_scissor_rects(a: [i32; 4], b: [i32; 4]) -> Option<[i32; 4]> {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = a[0].saturating_add(a[2]).min(b[0].saturating_add(b[2]));
    let y1 = a[1].saturating_add(a[3]).min(b[1].saturating_add(b[3]));
    if x1 > x0 && y1 > y0 {
        Some([x0, y0, x1 - x0, y1 - y0])
    } else {
        None
    }
}

fn check_and_record_mipmap(
    last_mipmapped: &mut Option<(glow::Texture, i64, (u32, u32))>,
    generated_this_paint: &mut bool,
    tex: glow::Texture,
    source_frame_id: i64,
    res: (u32, u32),
) -> bool {
    if source_frame_id > 0 {
        let key = (tex, source_frame_id, res);
        if *last_mipmapped == Some(key) {
            return false;
        }
        *last_mipmapped = Some(key);
        *generated_this_paint = true;
        true
    } else {
        if *generated_this_paint {
            return false;
        }
        *generated_this_paint = true;
        *last_mipmapped = None;
        true
    }
}

#[allow(clippy::too_many_arguments)]
fn build_preprocess_cache_sig(
    raw_frame: Option<&RawFrame>,
    fallback_texture: Option<glow::Texture>,
    fallback_captured_at: Option<i64>,
    resolution: (u32, u32),
    effective_res: (u32, u32),
    scaler_filter: u8,
    fsr1_sharpness: u32,
    allow_srgb_passthrough_fastpath: bool,
    scan_and_color: [u32; 6],
    deinterlace_enabled: bool,
    deinterlace_mode: u8,
    deinterlace_params: [u32; 4],
    framegen_enabled: bool,
    framegen_show_interpolated: bool,
    framegen_params: [u32; 3],
    median_enabled: bool,
    median_mix: u32,
    fft_enabled: bool,
    fft_thresholds: [u32; 2],
    fft_gen: u64,
) -> Option<PreprocessCacheSig> {
    let source = if let Some(f) = raw_frame {
        if f.captured_at > 0 {
            PreprocessSourceSig::Raw {
                captured_at: f.captured_at,
            }
        } else {
            return None;
        }
    } else if let (Some(tex), Some(captured_at)) = (fallback_texture, fallback_captured_at) {
        if captured_at > 0 {
            PreprocessSourceSig::Fallback { tex, captured_at }
        } else {
            return None;
        }
    } else {
        return None;
    };

    Some(PreprocessCacheSig {
        source,
        resolution,
        effective_res,
        scaler_filter,
        fsr1_sharpness,
        allow_srgb_passthrough_fastpath,
        scan_and_color,
        deinterlace_enabled,
        deinterlace_mode,
        deinterlace_params,
        framegen_enabled,
        framegen_show_interpolated,
        framegen_params,
        median_enabled,
        median_mix,
        fft_enabled,
        fft_thresholds,
        fft_gen,
    })
}

fn assemble_overlay_batch(
    primitive_groups: &[&[egui::ClippedPrimitive]],
    widget_rect: egui::Rect,
    output_size: (f32, f32),
    clip_limit: Option<[i32; 4]>,
    vertices_out: &mut Vec<egui::epaint::Vertex>,
    indices_out: &mut Vec<u32>,
    draws_out: &mut Vec<OverlayDrawCmd>,
) {
    vertices_out.clear();
    indices_out.clear();
    draws_out.clear();

    if widget_rect.width() <= 0.0 || widget_rect.height() <= 0.0 {
        return;
    }

    let ppp = output_size.0 / widget_rect.width();
    let out_w = output_size.0 as i32;
    let out_h = output_size.1 as i32;

    for primitives in primitive_groups {
        for primitive in *primitives {
            if let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive {
                if mesh.vertices.is_empty() || mesh.indices.is_empty() {
                    continue;
                }

                let clip_rect = primitive.clip_rect;
                let clip_min_x = ((clip_rect.min.x - widget_rect.min.x) * ppp).round() as i32;
                let clip_min_y = ((clip_rect.min.y - widget_rect.min.y) * ppp).round() as i32;
                let clip_max_x = ((clip_rect.max.x - widget_rect.min.x) * ppp).round() as i32;
                let clip_max_y = ((clip_rect.max.y - widget_rect.min.y) * ppp).round() as i32;

                let clip_min_x = clip_min_x.clamp(0, out_w);
                let clip_min_y = clip_min_y.clamp(0, out_h);
                let clip_max_x = clip_max_x.clamp(clip_min_x, out_w);
                let clip_max_y = clip_max_y.clamp(clip_min_y, out_h);

                let scissor_w = clip_max_x - clip_min_x;
                let scissor_h = clip_max_y - clip_min_y;
                if scissor_w <= 0 || scissor_h <= 0 {
                    continue;
                }

                let scissor_y = out_h - clip_max_y;
                let raw_scissor = [clip_min_x, scissor_y, scissor_w, scissor_h];
                let Some(scissor) = (match clip_limit {
                    Some(limit) => intersect_scissor_rects(raw_scissor, limit),
                    None => Some(raw_scissor),
                }) else {
                    continue;
                };

                let base_vertex = vertices_out.len() as u32;
                let first_index = indices_out.len();
                let index_count = mesh.indices.len() as i32;

                vertices_out.extend_from_slice(&mesh.vertices);
                indices_out.extend(mesh.indices.iter().map(|&idx| idx + base_vertex));

                if let Some(last) = draws_out.last_mut() {
                    if last.texture_id == mesh.texture_id && last.scissor == scissor {
                        last.index_count += index_count;
                        continue;
                    }
                }

                draws_out.push(OverlayDrawCmd {
                    texture_id: mesh.texture_id,
                    scissor,
                    first_index,
                    index_count,
                });
            }
        }
    }
}

pub fn read_framebuffer_pixels_raw(
    gl: &glow::Context,
    area: RenderedArea,
) -> Option<(Vec<u8>, u32, u32)> {
    if area.width == 0 || area.height == 0 {
        return None;
    }
    let mut raw_pixels = vec![0u8; (area.width * area.height * 4) as usize];
    unsafe {
        let old_pbo = gl.get_parameter_i32(glow::PIXEL_PACK_BUFFER_BINDING);
        let alignment = gl.get_parameter_i32(glow::PACK_ALIGNMENT);
        let row_length = gl.get_parameter_i32(glow::PACK_ROW_LENGTH);
        let skip_rows = gl.get_parameter_i32(glow::PACK_SKIP_ROWS);
        let skip_pixels = gl.get_parameter_i32(glow::PACK_SKIP_PIXELS);

        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
        gl.pixel_store_i32(glow::PACK_ROW_LENGTH, 0);
        gl.pixel_store_i32(glow::PACK_SKIP_ROWS, 0);
        gl.pixel_store_i32(glow::PACK_SKIP_PIXELS, 0);
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);

        gl.read_pixels(
            area.x as i32,
            area.y as i32,
            area.width as i32,
            area.height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(&mut raw_pixels),
        );

        gl.bind_buffer(
            glow::PIXEL_PACK_BUFFER,
            std::num::NonZeroU32::new(old_pbo as u32).map(glow::NativeBuffer),
        );
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, alignment);
        gl.pixel_store_i32(glow::PACK_ROW_LENGTH, row_length);
        gl.pixel_store_i32(glow::PACK_SKIP_ROWS, skip_rows);
        gl.pixel_store_i32(glow::PACK_SKIP_PIXELS, skip_pixels);
    }

    Some((raw_pixels, area.width, area.height))
}

fn intermediate_texture_internal_format(pass_index: usize) -> u32 {
    match pass_index {
        // All 6 active intermediate passes store linear RGB sampled before final
        // linear->sRGB presentation; keep them in half-float to avoid dark-area banding.
        0..NUM_INTERMEDIATE_PASSES => glow::RGBA16F,
        _ => glow::RGBA8,
    }
}

fn pixelate_subrender_size(width: u32, height: u32) -> (u32, u32) {
    super::geometry::pixelate_size((width, height))
}

fn pass_texture_dimensions(
    pass_index: usize,
    source_size: (u32, u32),
    effective_size: (u32, u32),
) -> (u32, u32) {
    match pass_index {
        // PASS_PIXELATE is the low-scale pixelate subrender. The final pass samples it
        // back to the output surface, preserving the existing shader order.
        PASS_PIXELATE => pixelate_subrender_size(effective_size.0, effective_size.1),
        // History (0: t-1, 2: t-2), Deinterlace (1), Median (4), and Input (5) happen before any upscaling.
        PASS_HIST_1 | PASS_DEINTERLACE | PASS_HIST_2 | PASS_MEDIAN | PASS_INPUT => source_size,
        _ => effective_size,
    }
}

impl CrtFilterRenderer {
    pub fn new(gl: &glow::Context) -> Self {
        unsafe {
            let passthrough_prog = compile_program(gl, VS_SRC, FS_PASSTHROUGH);
            let pixelate_prog = compile_program(gl, VS_SRC, FS_PIXELATE);
            let median_prog = compile_program(gl, VS_SRC, FS_MEDIAN_3X1);
            let deinterlace_prog = compile_program(gl, VS_SRC, FS_DEINTERLACE);
            let final_prog = compile_program(gl, VS_SRC, FS_FINAL);
            let yuv_planar_prog = compile_program(gl, VS_SRC, FS_YUV_PLANAR);
            let yuyv_packed_prog = compile_program(gl, VS_SRC, FS_YUYV_PACKED);
            let cathode_prog = compile_program(gl, VS_SRC, FS_CATHODE_INTERFERENCE);
            let retro_frame_prog = compile_program(gl, VS_SRC, FS_RETRO_FRAME);

            let retro_output_res_loc =
                gl.get_uniform_location(retro_frame_prog, "outputResolution");
            let retro_source_size_loc = gl.get_uniform_location(retro_frame_prog, "source_size");
            let retro_horizontal_stretch_loc =
                gl.get_uniform_location(retro_frame_prog, "horizontal_stretch");
            let retro_border_crop_loc = gl.get_uniform_location(retro_frame_prog, "border_crop");
            let retro_warp_loc = gl.get_uniform_location(retro_frame_prog, "warp");
            let retro_corner_size_loc = gl.get_uniform_location(retro_frame_prog, "corner_size");
            let retro_filter_type_loc = gl.get_uniform_location(retro_frame_prog, "filter_type");
            let retro_time_loc = gl.get_uniform_location(retro_frame_prog, "time");
            let retro_ambient_glow_loc = gl.get_uniform_location(retro_frame_prog, "ambient_glow");
            let retro_dark_mode_loc = gl.get_uniform_location(retro_frame_prog, "dark_mode");

            gl.use_program(Some(retro_frame_prog));
            if let Some(loc) = gl.get_uniform_location(retro_frame_prog, "bezel_texture") {
                gl.uniform_1_i32(Some(&loc), 0);
            }
            if let Some(loc) = gl.get_uniform_location(retro_frame_prog, "video_texture") {
                gl.uniform_1_i32(Some(&loc), 1);
            }
            gl.use_program(None);

            let crt_glass_prog = compile_program(gl, VS_SRC, FS_CRT_GLASS);
            let glass_output_res_loc = gl.get_uniform_location(crt_glass_prog, "outputResolution");
            let glass_source_size_loc = gl.get_uniform_location(crt_glass_prog, "source_size");
            let glass_horizontal_stretch_loc =
                gl.get_uniform_location(crt_glass_prog, "horizontal_stretch");
            let glass_border_crop_loc = gl.get_uniform_location(crt_glass_prog, "border_crop");
            let glass_warp_loc = gl.get_uniform_location(crt_glass_prog, "warp");
            let glass_corner_size_loc = gl.get_uniform_location(crt_glass_prog, "corner_size");
            let glass_filter_type_loc = gl.get_uniform_location(crt_glass_prog, "filter_type");
            let glass_intensity_loc = gl.get_uniform_location(crt_glass_prog, "intensity");
            let glass_glossiness_loc = gl.get_uniform_location(crt_glass_prog, "glossiness");
            let glass_time_loc = gl.get_uniform_location(crt_glass_prog, "time");
            let glass_ceiling_light_enabled_loc =
                gl.get_uniform_location(crt_glass_prog, "ceiling_light_enabled");
            let glass_photographer_enabled_loc =
                gl.get_uniform_location(crt_glass_prog, "photographer_enabled");
            let glass_photographer_intensity_loc =
                gl.get_uniform_location(crt_glass_prog, "photographer_intensity");
            let glass_flash_enabled_loc = gl.get_uniform_location(crt_glass_prog, "flash_enabled");
            let glass_flash_intensity_loc =
                gl.get_uniform_location(crt_glass_prog, "flash_intensity");

            gl.use_program(Some(crt_glass_prog));
            if let Some(loc) = gl.get_uniform_location(crt_glass_prog, "video_texture") {
                gl.uniform_1_i32(Some(&loc), 0);
            }
            if let Some(loc) = gl.get_uniform_location(crt_glass_prog, "silhouette_texture") {
                gl.uniform_1_i32(Some(&loc), 1);
            }
            gl.use_program(None);

            let night_glow_prog = compile_program(gl, VS_SRC, FS_NIGHT_GLOW);
            let night_glow_output_res_loc =
                gl.get_uniform_location(night_glow_prog, "outputResolution");
            let night_glow_source_size_loc =
                gl.get_uniform_location(night_glow_prog, "source_size");
            let night_glow_horizontal_stretch_loc =
                gl.get_uniform_location(night_glow_prog, "horizontal_stretch");
            let night_glow_border_crop_loc =
                gl.get_uniform_location(night_glow_prog, "border_crop");
            let night_glow_warp_loc = gl.get_uniform_location(night_glow_prog, "warp");
            let night_glow_corner_size_loc =
                gl.get_uniform_location(night_glow_prog, "corner_size");
            let night_glow_filter_type_loc =
                gl.get_uniform_location(night_glow_prog, "filter_type");
            let night_glow_intensity_loc =
                gl.get_uniform_location(night_glow_prog, "glow_intensity");

            gl.use_program(Some(night_glow_prog));
            if let Some(loc) = gl.get_uniform_location(night_glow_prog, "video_texture") {
                gl.uniform_1_i32(Some(&loc), 0);
            }
            gl.use_program(None);

            let popup_prog = compile_program(gl, VS_POPUP, FS_POPUP);
            let popup_screen_size_loc = gl.get_uniform_location(popup_prog, "u_screen_size");
            let popup_offset_loc = gl.get_uniform_location(popup_prog, "u_offset");
            let popup_sampler_loc = gl.get_uniform_location(popup_prog, "u_sampler");
            let popup_shadow_mask_loc = gl.get_uniform_location(popup_prog, "u_shadow_mask");
            let popup_scanline_strength_loc =
                gl.get_uniform_location(popup_prog, "u_scanline_strength");
            let popup_scanline_freq_loc = gl.get_uniform_location(popup_prog, "u_scanline_freq");
            let popup_brightboost_loc = gl.get_uniform_location(popup_prog, "u_brightboost");

            gl.use_program(Some(popup_prog));
            if let Some(ref loc) = popup_sampler_loc {
                gl.uniform_1_i32(Some(loc), 0);
            }
            gl.use_program(None);

            let blit_prog = compile_program(gl, VS_SRC, FS_BLIT);
            gl.use_program(Some(blit_prog));
            if let Some(loc) = gl.get_uniform_location(blit_prog, "u_texture") {
                gl.uniform_1_i32(Some(&loc), 0);
            }
            gl.use_program(None);

            let retro_mouse_prog = compile_program(gl, VS_RETRO_MOUSE, FS_RETRO_MOUSE);
            let retro_mouse_screen_size_loc =
                gl.get_uniform_location(retro_mouse_prog, "u_screen_size");
            let retro_mouse_cursor_pos_loc =
                gl.get_uniform_location(retro_mouse_prog, "u_cursor_pos");
            let retro_mouse_cursor_size_loc =
                gl.get_uniform_location(retro_mouse_prog, "u_cursor_size");
            let retro_mouse_sampler_loc =
                gl.get_uniform_location(retro_mouse_prog, "u_cursor_sampler");
            let retro_mouse_shadow_mask_loc =
                gl.get_uniform_location(retro_mouse_prog, "u_shadow_mask");
            let retro_mouse_scanline_strength_loc =
                gl.get_uniform_location(retro_mouse_prog, "u_scanline_strength");
            let retro_mouse_scanline_freq_loc =
                gl.get_uniform_location(retro_mouse_prog, "u_scanline_freq");
            let retro_mouse_brightboost_loc =
                gl.get_uniform_location(retro_mouse_prog, "u_brightboost");

            gl.use_program(Some(retro_mouse_prog));
            if let Some(ref loc) = retro_mouse_sampler_loc {
                gl.uniform_1_i32(Some(loc), 0);
            }
            gl.use_program(None);

            let linearize_prog = compile_program(gl, VS_SRC, FS_LINEARIZE);
            let linearize_sampler_loc = gl.get_uniform_location(linearize_prog, "srgb_tex");
            let linearize_overscan_loc = gl.get_uniform_location(linearize_prog, "overscan_offset");
            let linearize_underscan_loc =
                gl.get_uniform_location(linearize_prog, "underscan_stretch");
            let linearize_brightness_loc = gl.get_uniform_location(linearize_prog, "brightness");
            let linearize_gamma_loc = gl.get_uniform_location(linearize_prog, "gamma");
            gl.use_program(Some(linearize_prog));
            if let Some(ref loc) = linearize_sampler_loc {
                gl.uniform_1_i32(Some(loc), 0);
            }
            if let Some(ref loc) = linearize_overscan_loc {
                gl.uniform_2_f32(Some(loc), 0.0, 0.0);
            }
            if let Some(ref loc) = linearize_underscan_loc {
                gl.uniform_2_f32(Some(loc), 1.0, 1.0);
            }
            if let Some(ref loc) = linearize_brightness_loc {
                gl.uniform_1_f32(Some(loc), 1.0);
            }
            if let Some(ref loc) = linearize_gamma_loc {
                gl.uniform_1_f32(Some(loc), 1.0);
            }
            gl.use_program(None);

            let retro_mouse_texture = gl
                .create_texture()
                .expect("Cannot create retro mouse texture");
            gl.bind_texture(glow::TEXTURE_2D, Some(retro_mouse_texture));
            let cursor_pixels = generate_retro_cursor_pixels();
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                RETRO_CURSOR_WIDTH as i32,
                RETRO_CURSOR_HEIGHT as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                Some(&cursor_pixels),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);

            let p_passthrough_output_res_loc = gl
                .get_uniform_location(passthrough_prog, "outputResolution")
                .unwrap();
            let passthrough_scaler_filter_loc = gl
                .get_uniform_location(passthrough_prog, "scaler_filter")
                .unwrap();
            let p_pixelate_target_res_loc = gl
                .get_uniform_location(pixelate_prog, "target_resolution")
                .unwrap();
            let median_mix_loc = gl.get_uniform_location(median_prog, "mix_amount").unwrap();

            gl.use_program(Some(median_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(median_prog, "video_texture")
                        .unwrap(),
                ),
                0,
            );
            gl.use_program(None);

            let deinterlace_mode_loc = gl.get_uniform_location(deinterlace_prog, "mode").unwrap();
            let deinterlace_blend_loc = gl
                .get_uniform_location(deinterlace_prog, "blend_amount")
                .unwrap();
            let deinterlace_motion_thresh_loc = gl
                .get_uniform_location(deinterlace_prog, "motion_threshold")
                .unwrap();
            let deinterlace_line_spacing_loc = gl
                .get_uniform_location(deinterlace_prog, "line_spacing")
                .unwrap();
            let deinterlace_spatial_mix_loc = gl
                .get_uniform_location(deinterlace_prog, "spatial_mix")
                .unwrap();
            let deinterlace_current_frame_loc =
                gl.get_uniform_location(deinterlace_prog, "current_frame");
            let deinterlace_prev_frame_loc =
                gl.get_uniform_location(deinterlace_prog, "prev_frame");
            let deinterlace_prev_frame_2_loc =
                gl.get_uniform_location(deinterlace_prog, "prev_frame_2");
            let deinterlace_has_prev2_loc = gl.get_uniform_location(deinterlace_prog, "has_prev2");

            gl.use_program(Some(deinterlace_prog));
            if let Some(loc) = &deinterlace_current_frame_loc {
                gl.uniform_1_i32(Some(loc), 0);
            }
            if let Some(loc) = &deinterlace_prev_frame_loc {
                gl.uniform_1_i32(Some(loc), 1);
            }
            if let Some(loc) = &deinterlace_prev_frame_2_loc {
                gl.uniform_1_i32(Some(loc), 2);
            }
            gl.use_program(None);

            let final_output_res_loc = gl
                .get_uniform_location(final_prog, "outputResolution")
                .unwrap();
            let final_hard_scan_loc = gl.get_uniform_location(final_prog, "hardScan").unwrap();
            let final_hard_pix_loc = gl.get_uniform_location(final_prog, "hardPix").unwrap();
            let final_warp_x_loc = gl.get_uniform_location(final_prog, "warpX").unwrap();
            let final_warp_y_loc = gl.get_uniform_location(final_prog, "warpY").unwrap();
            let final_shadow_mask_loc = gl.get_uniform_location(final_prog, "shadowMask").unwrap();
            let final_brightboost_loc = gl.get_uniform_location(final_prog, "brightboost").unwrap();
            let final_hard_bloom_pix_loc =
                gl.get_uniform_location(final_prog, "hardBloomPix").unwrap();
            let final_hard_bloom_scan_loc = gl
                .get_uniform_location(final_prog, "hardBloomScan")
                .unwrap();
            let final_bloom_amount_loc =
                gl.get_uniform_location(final_prog, "bloomAmount").unwrap();
            let final_shape_loc = gl.get_uniform_location(final_prog, "shape").unwrap();
            let final_background_color_loc = gl
                .get_uniform_location(final_prog, "background_color")
                .unwrap();
            let passthrough_background_color_loc = gl
                .get_uniform_location(passthrough_prog, "background_color")
                .unwrap();
            let final_horizontal_stretch_loc = gl
                .get_uniform_location(final_prog, "horizontal_stretch")
                .unwrap();
            let passthrough_horizontal_stretch_loc = gl
                .get_uniform_location(passthrough_prog, "horizontal_stretch")
                .unwrap();
            let final_vibrance_loc = gl.get_uniform_location(final_prog, "vibrance").unwrap();
            let passthrough_vibrance_loc = gl
                .get_uniform_location(passthrough_prog, "vibrance")
                .unwrap();
            let final_border_crop_loc = gl.get_uniform_location(final_prog, "border_crop").unwrap();
            let passthrough_border_crop_loc = gl
                .get_uniform_location(passthrough_prog, "border_crop")
                .unwrap();
            let passthrough_input_is_srgb_loc =
                gl.get_uniform_location(passthrough_prog, "input_is_srgb");
            let passthrough_ocr_warp_loc = gl
                .get_uniform_location(passthrough_prog, "ocr_warp")
                .unwrap();
            let passthrough_ocr_zoom_loc = gl
                .get_uniform_location(passthrough_prog, "ocr_zoom")
                .unwrap();
            let passthrough_image_aspect_loc = gl
                .get_uniform_location(passthrough_prog, "image_aspect")
                .unwrap();

            let cathode_output_res_loc = gl
                .get_uniform_location(cathode_prog, "outputResolution")
                .unwrap();
            let cathode_source_size_loc = gl
                .get_uniform_location(cathode_prog, "source_size")
                .unwrap();
            let cathode_horizontal_stretch_loc = gl
                .get_uniform_location(cathode_prog, "horizontal_stretch")
                .unwrap();
            let cathode_time_loc = gl.get_uniform_location(cathode_prog, "time").unwrap();
            let cathode_intensity_loc = gl.get_uniform_location(cathode_prog, "intensity").unwrap();
            let cathode_frequency_loc = gl.get_uniform_location(cathode_prog, "frequency").unwrap();
            let cathode_randomization_loc = gl
                .get_uniform_location(cathode_prog, "randomization")
                .unwrap();
            let cathode_electricity_glow_loc = gl
                .get_uniform_location(cathode_prog, "electricity_glow")
                .unwrap();
            let cathode_flicker_depth_loc = gl
                .get_uniform_location(cathode_prog, "flicker_depth")
                .unwrap();
            let cathode_interference_loc = gl
                .get_uniform_location(cathode_prog, "interference")
                .unwrap();
            let cathode_lightbulb_effect_loc = gl
                .get_uniform_location(cathode_prog, "lightbulb_effect")
                .unwrap();
            let cathode_border_crop_loc = gl
                .get_uniform_location(cathode_prog, "border_crop")
                .unwrap();

            gl.use_program(Some(cathode_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(cathode_prog, "input_texture")
                        .unwrap(),
                ),
                0,
            );

            let audio_reactive_prog = compile_program(gl, VS_SRC, FS_AUDIO_REACTIVE);
            let ar_output_res_loc =
                gl.get_uniform_location(audio_reactive_prog, "outputResolution");
            let ar_source_size_loc = gl.get_uniform_location(audio_reactive_prog, "source_size");
            let ar_horizontal_stretch_loc =
                gl.get_uniform_location(audio_reactive_prog, "horizontal_stretch");
            let ar_border_crop_loc = gl.get_uniform_location(audio_reactive_prog, "border_crop");
            let ar_warp_loc = gl.get_uniform_location(audio_reactive_prog, "warp");
            let ar_corner_size_loc = gl.get_uniform_location(audio_reactive_prog, "corner_size");
            let ar_filter_type_loc = gl.get_uniform_location(audio_reactive_prog, "filter_type");
            let ar_time_loc = gl.get_uniform_location(audio_reactive_prog, "time");
            let ar_master_intensity_loc =
                gl.get_uniform_location(audio_reactive_prog, "master_intensity");
            let ar_low_env_loc = gl.get_uniform_location(audio_reactive_prog, "low_env");
            let ar_mid_env_loc = gl.get_uniform_location(audio_reactive_prog, "mid_env");
            let ar_high_env_loc = gl.get_uniform_location(audio_reactive_prog, "high_env");
            let ar_transient_env_loc =
                gl.get_uniform_location(audio_reactive_prog, "transient_env");
            let ar_stereo_balance_loc =
                gl.get_uniform_location(audio_reactive_prog, "stereo_balance");
            let ar_spring_pos_loc = gl.get_uniform_location(audio_reactive_prog, "spring_pos");
            let ar_phase_accum_loc = gl.get_uniform_location(audio_reactive_prog, "phase_accum");
            let ar_deflection_shake_loc =
                gl.get_uniform_location(audio_reactive_prog, "deflection_shake");
            let ar_stereo_tug_loc = gl.get_uniform_location(audio_reactive_prog, "stereo_tug");
            let ar_flyback_pump_loc = gl.get_uniform_location(audio_reactive_prog, "flyback_pump");
            let ar_rgb_deconvergence_loc =
                gl.get_uniform_location(audio_reactive_prog, "rgb_deconvergence");
            let ar_scanline_wave_loc =
                gl.get_uniform_location(audio_reactive_prog, "scanline_wave");
            let ar_phosphor_surge_loc =
                gl.get_uniform_location(audio_reactive_prog, "phosphor_surge");
            let ar_sync_tear_loc = gl.get_uniform_location(audio_reactive_prog, "sync_tear");

            gl.use_program(Some(audio_reactive_prog));
            if let Some(loc) = gl.get_uniform_location(audio_reactive_prog, "input_texture") {
                gl.uniform_1_i32(Some(&loc), 0);
            }

            gl.use_program(Some(passthrough_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(passthrough_prog, "video_texture")
                        .unwrap(),
                ),
                0,
            );
            gl.use_program(Some(pixelate_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(pixelate_prog, "video_texture")
                        .unwrap(),
                ),
                0,
            );
            gl.use_program(Some(median_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(median_prog, "video_texture")
                        .unwrap(),
                ),
                0,
            );
            gl.use_program(Some(final_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(final_prog, "video_texture")
                        .unwrap(),
                ),
                0,
            );

            gl.use_program(Some(yuv_planar_prog));
            gl.uniform_1_i32(
                Some(&gl.get_uniform_location(yuv_planar_prog, "y_tex").unwrap()),
                0,
            );
            gl.uniform_1_i32(
                Some(&gl.get_uniform_location(yuv_planar_prog, "u_tex").unwrap()),
                1,
            );
            gl.uniform_1_i32(
                Some(&gl.get_uniform_location(yuv_planar_prog, "v_tex").unwrap()),
                2,
            );

            gl.use_program(Some(yuyv_packed_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(yuyv_packed_prog, "raw_tex")
                        .unwrap(),
                ),
                0,
            );
            let yuyv_range_loc = gl
                .get_uniform_location(yuyv_packed_prog, "input_range")
                .unwrap();
            let yuyv_overscan_loc = gl
                .get_uniform_location(yuyv_packed_prog, "overscan_offset")
                .unwrap();
            let yuyv_underscan_loc = gl
                .get_uniform_location(yuyv_packed_prog, "underscan_stretch")
                .unwrap();
            let yuyv_brightness_loc = gl
                .get_uniform_location(yuyv_packed_prog, "brightness")
                .unwrap();
            let yuyv_gamma_loc = gl.get_uniform_location(yuyv_packed_prog, "gamma").unwrap();

            gl.use_program(Some(yuv_planar_prog));
            let yuv_range_loc = gl
                .get_uniform_location(yuv_planar_prog, "input_range")
                .unwrap();
            let yuv_overscan_loc = gl
                .get_uniform_location(yuv_planar_prog, "overscan_offset")
                .unwrap();
            let yuv_underscan_loc = gl
                .get_uniform_location(yuv_planar_prog, "underscan_stretch")
                .unwrap();
            let yuv_brightness_loc = gl
                .get_uniform_location(yuv_planar_prog, "brightness")
                .unwrap();
            let yuv_gamma_loc = gl.get_uniform_location(yuv_planar_prog, "gamma").unwrap();

            gl.use_program(None);

            let fbos = [
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
            ];
            let pass_textures = [
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
            ];
            let yuv_planes = [
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
            ];
            let pbos = [
                gl.create_buffer().unwrap(),
                gl.create_buffer().unwrap(),
                gl.create_buffer().unwrap(),
            ];

            let post_fbos = [
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
            ];
            let post_textures = [gl.create_texture().unwrap(), gl.create_texture().unwrap()];
            let ocr_fbo = gl.create_framebuffer().unwrap();
            let ocr_texture = gl.create_texture().unwrap();
            let ocr_pbo = gl.create_buffer().unwrap();

            let vertex_array = gl
                .create_vertex_array()
                .expect("Cannot create vertex array");
            let vertices: [f32; 16] = [
                -1.0, -1.0, 0.0, 0.0, 1.0, -1.0, 1.0, 0.0, -1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0,
            ];
            let vbo = gl.create_buffer().unwrap();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&vertices),
                glow::STATIC_DRAW,
            );

            gl.bind_vertex_array(Some(vertex_array));
            gl.vertex_attrib_pointer_f32(
                0,
                2,
                glow::FLOAT,
                false,
                4 * std::mem::size_of::<f32>() as i32,
                0,
            );
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(
                1,
                2,
                glow::FLOAT,
                false,
                4 * std::mem::size_of::<f32>() as i32,
                (2 * std::mem::size_of::<f32>()) as i32,
            );
            gl.enable_vertex_attrib_array(1);

            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_vertex_array(None);

            let popup_vao = gl.create_vertex_array().expect("Cannot create popup VAO");
            let popup_vbo = gl.create_buffer().unwrap();
            let popup_ebo = gl.create_buffer().unwrap();
            gl.bind_vertex_array(Some(popup_vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(popup_vbo));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(popup_ebo));
            let popup_stride = std::mem::size_of::<egui::epaint::Vertex>() as i32;
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, popup_stride, 0);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, popup_stride, 8);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(2, 4, glow::UNSIGNED_BYTE, false, popup_stride, 16);
            gl.enable_vertex_attrib_array(2);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);

            let mut bezel_img =
                image::load_from_memory(include_bytes!("../../../assets/nec_pc98_bezel.png"))
                    .expect("Failed to load nec_pc98_bezel.png")
                    .to_rgba8();
            image::imageops::flip_vertical_in_place(&mut bezel_img);
            let (bw, bh) = (bezel_img.width(), bezel_img.height());
            let bezel_texture = gl.create_texture().unwrap();
            gl.bind_texture(glow::TEXTURE_2D, Some(bezel_texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                bw as i32,
                bh as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                Some(&bezel_img.into_raw()),
            );
            gl.generate_mipmap(glow::TEXTURE_2D);
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR_MIPMAP_LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
            gl.bind_texture(glow::TEXTURE_2D, None);

            let mut sil_img = image::load_from_memory(include_bytes!(
                "../../../assets/crt_reflection_silhouette.png"
            ))
            .expect("Failed to load crt_reflection_silhouette.png")
            .to_rgba8();
            image::imageops::flip_vertical_in_place(&mut sil_img);
            let (sw, sh) = (sil_img.width(), sil_img.height());
            let silhouette_texture = gl.create_texture().unwrap();
            gl.bind_texture(glow::TEXTURE_2D, Some(silhouette_texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                sw as i32,
                sh as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                Some(&sil_img.into_raw()),
            );
            gl.generate_mipmap(glow::TEXTURE_2D);
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR_MIPMAP_LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);

            Self {
                passthrough_prog,
                retro_frame_prog,
                bezel_texture,
                retro_output_res_loc,
                retro_source_size_loc,
                retro_horizontal_stretch_loc,
                retro_border_crop_loc,
                retro_warp_loc,
                retro_corner_size_loc,
                retro_filter_type_loc,
                retro_time_loc,
                retro_ambient_glow_loc,
                retro_dark_mode_loc,
                last_video_texture: None,
                last_video_texture_res: (0, 0),
                last_video_tex_base_filter: glow::LINEAR as i32,
                current_source_frame_id: 0,
                last_mipmapped_frame: std::cell::Cell::new(None),
                mipmap_generated_this_paint: std::cell::Cell::new(false),
                last_pixelate_key: None,
                pixelate_prog,
                final_prog,
                yuv_planar_prog,
                yuyv_packed_prog,
                yuv_range_loc,
                yuyv_range_loc,
                yuv_overscan_loc,
                yuyv_overscan_loc,
                yuv_underscan_loc,
                yuyv_underscan_loc,
                yuv_brightness_loc,
                yuyv_brightness_loc,
                yuv_gamma_loc,
                yuyv_gamma_loc,
                fbos,
                pass_textures,
                yuv_planes,
                pbos,
                vertex_array,
                vbo,
                p_passthrough_output_res_loc,
                p_pixelate_target_res_loc,
                final_output_res_loc,
                final_hard_scan_loc,
                final_hard_pix_loc,
                final_warp_x_loc,
                final_warp_y_loc,
                final_shadow_mask_loc,
                final_brightboost_loc,
                final_hard_bloom_pix_loc,
                final_hard_bloom_scan_loc,
                final_bloom_amount_loc,
                final_shape_loc,
                final_background_color_loc,
                passthrough_background_color_loc,
                final_horizontal_stretch_loc,
                passthrough_horizontal_stretch_loc,
                final_vibrance_loc,
                passthrough_vibrance_loc,
                passthrough_scaler_filter_loc,
                final_border_crop_loc,
                passthrough_border_crop_loc,
                passthrough_input_is_srgb_loc,
                passthrough_ocr_warp_loc,
                passthrough_ocr_zoom_loc,
                passthrough_image_aspect_loc,
                median_prog,
                median_mix_loc,
                deinterlace_prog,
                deinterlace_current_frame_loc,
                deinterlace_prev_frame_loc,
                deinterlace_prev_frame_2_loc,
                deinterlace_has_prev2_loc,
                deinterlace_mode_loc,
                deinterlace_blend_loc,
                deinterlace_motion_thresh_loc,
                deinterlace_line_spacing_loc,
                deinterlace_spatial_mix_loc,
                cathode_prog,
                cathode_output_res_loc,
                cathode_source_size_loc,
                cathode_horizontal_stretch_loc,
                cathode_time_loc,
                cathode_intensity_loc,
                cathode_frequency_loc,
                cathode_randomization_loc,
                cathode_electricity_glow_loc,
                cathode_flicker_depth_loc,
                cathode_interference_loc,
                cathode_lightbulb_effect_loc,
                cathode_border_crop_loc,
                audio_reactive_prog,
                ar_output_res_loc,
                ar_source_size_loc,
                ar_horizontal_stretch_loc,
                ar_border_crop_loc,
                ar_warp_loc,
                ar_corner_size_loc,
                ar_filter_type_loc,
                ar_time_loc,
                ar_master_intensity_loc,
                ar_low_env_loc,
                ar_mid_env_loc,
                ar_high_env_loc,
                ar_transient_env_loc,
                ar_stereo_balance_loc,
                ar_spring_pos_loc,
                ar_phase_accum_loc,
                ar_deflection_shake_loc,
                ar_stereo_tug_loc,
                ar_flyback_pump_loc,
                ar_rgb_deconvergence_loc,
                ar_scanline_wave_loc,
                ar_phosphor_surge_loc,
                ar_sync_tear_loc,
                crt_glass_prog,
                glass_output_res_loc,
                glass_source_size_loc,
                glass_horizontal_stretch_loc,
                glass_border_crop_loc,
                glass_warp_loc,
                glass_corner_size_loc,
                glass_filter_type_loc,
                glass_intensity_loc,
                glass_glossiness_loc,
                glass_time_loc,
                glass_ceiling_light_enabled_loc,
                glass_photographer_enabled_loc,
                glass_photographer_intensity_loc,
                glass_flash_enabled_loc,
                glass_flash_intensity_loc,
                silhouette_texture,
                night_glow_prog,
                night_glow_output_res_loc,
                night_glow_source_size_loc,
                night_glow_horizontal_stretch_loc,
                night_glow_border_crop_loc,
                night_glow_warp_loc,
                night_glow_corner_size_loc,
                night_glow_filter_type_loc,
                night_glow_intensity_loc,
                popup_primitives: Vec::new(),
                comment_primitives: Vec::new(),
                alert_primitives: Vec::new(),
                popup_prog,
                popup_vao,
                popup_vbo,
                popup_ebo,
                overlay_vertices_scratch: Vec::new(),
                overlay_indices_scratch: Vec::new(),
                overlay_draws_scratch: Vec::new(),
                popup_screen_size_loc,
                popup_offset_loc,
                popup_sampler_loc,
                popup_shadow_mask_loc,
                popup_scanline_strength_loc,
                popup_scanline_freq_loc,
                popup_brightboost_loc,
                blit_prog,
                retro_mouse_prog,
                retro_mouse_texture,
                retro_mouse_screen_size_loc,
                retro_mouse_cursor_pos_loc,
                retro_mouse_cursor_size_loc,
                retro_mouse_sampler_loc,
                retro_mouse_shadow_mask_loc,
                retro_mouse_scanline_strength_loc,
                retro_mouse_scanline_freq_loc,
                retro_mouse_brightboost_loc,
                linearize_prog,
                linearize_sampler_loc,
                linearize_overscan_loc,
                linearize_underscan_loc,
                linearize_brightness_loc,
                linearize_gamma_loc,
                post_fbos,
                post_textures,
                last_post_size: (0, 0),
                anime4k_small: Anime4kUpscaler::new(gl, Anime4kVariant::Small),
                anime4k_medium: Anime4kUpscaler::new(gl, Anime4kVariant::Medium),
                anime4k_large: Anime4kUpscaler::new(gl, Anime4kVariant::Large),
                fsr1: Fsr1Upscaler::new(gl),
                framegen: FfxFrameGen::new(gl),
                halo: HaloRenderer::new(gl),
                beam: BeamRenderer::new(gl),
                last_size: (0, 0),
                last_scaler_filter: None,
                last_pass_res: (0, 0),
                last_frame_size: (0, 0),
                last_frame_format: None,
                last_frame_captured_at: -1,
                prev_frame_captured_at: -1,
                last_input_color_range: None,
                last_input_scan: (0.0, 0.0, 0.0, 0.0),
                last_input_color_adj: (1.0, 1.0),
                last_fft_enabled: false,
                last_fft_thresholds: (0.0, 0.0),
                last_fft_mask_gen: 0,
                last_fft_spectrum_rendered: false,
                last_input_tex: None,
                last_input_slot: None,
                last_source_kind: None,
                last_fallback_tex: None,
                last_preprocess_sig: None,
                last_preprocess_out: None,
                last_real_video_texture: None,
                last_fsr1_active: false,
                last_framegen_active: false,
                history_ring: DeinterlaceHistoryRing::new(HISTORY_SLOTS),
                fallback_captured_at_next_paint: None,
                framegen_next_paint: None,
                render_fft_spectrum_next_paint: false,
                capture_ocr_next_paint: false,
                capture_ocr_async_next_paint: false,
                ocr_recycled_buffer: None,
                last_ocr_capture: None,
                ocr_fbo,
                ocr_texture,
                last_ocr_fbo_size: (0, 0),
                ocr_pbo,
                ocr_pbo_size: 0,
                ocr_pending_readback: None,
            }
        }
    }

    /// Returns telemetry from the most recent FSR 1.0 GPU pass if FSR 1.0 produced the active frame.
    pub fn last_fsr1_run(&self) -> Option<Fsr1RunInfo> {
        if self.last_fsr1_active {
            self.fsr1.last_run()
        } else {
            None
        }
    }

    /// Returns telemetry from the most recent AMD FidelityFX Frame Gen pass when active.
    pub fn last_framegen_run(&self) -> Option<FfxFrameGenRunInfo> {
        if self.last_framegen_active {
            self.framegen.last_run()
        } else {
            None
        }
    }

    pub fn last_framegen_error(&self) -> Option<String> {
        self.framegen.init_error().map(str::to_string)
    }

    #[cfg(test)]
    pub unsafe fn flush_framegen_scd_sync(&mut self, gl: &glow::Context) {
        self.framegen.flush_scd_telemetry_sync(gl);
    }

    /// Private helper to decode a raw YUV frame into an RGB texture and optionally apply the FFT filter.
    /// Returns the texture containing the result (`self.pass_textures[target_slot]` or the FFT output).
    #[allow(clippy::too_many_arguments)]
    unsafe fn prepare_input_texture(
        &mut self,
        gl: &glow::Context,
        frame: &RawFrame,
        is_new_frame: bool,
        target_slot: usize,
        deinterlace_filter_enabled: bool,
        target_width: u32,
        target_height: u32,
        overscan_x: f32,
        overscan_y: f32,
        underscan_x: f32,
        underscan_y: f32,
        brightness: f32,
        gamma: f32,
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
        render_fft_spectrum: bool,
    ) -> glow::Texture {
        let scan_tuple = (overscan_x, overscan_y, underscan_x, underscan_y);
        let color_adj_tuple = (brightness, gamma);
        let fft_params_changed = if let Some(fft_arc) = fft_filter {
            let fft = fft_arc.lock().unwrap();
            let mask_gen = fft.mask_generation();
            let thresholds = (fft_mask_threshold, fft_black_threshold);
            !self.last_fft_enabled
                || thresholds != self.last_fft_thresholds
                || mask_gen != self.last_fft_mask_gen
                || (render_fft_spectrum && !self.last_fft_spectrum_rendered)
        } else {
            false
        };
        let need_yuv_upload = is_new_frame
            || self.last_input_slot != Some(target_slot)
            || self.last_input_tex.is_none()
            || (self.last_fft_enabled && fft_filter.is_none())
            || (self.last_fft_enabled && deinterlace_filter_enabled && fft_params_changed)
            || frame.width != self.last_frame_size.0
            || frame.height != self.last_frame_size.1
            || Some(frame.format) != self.last_frame_format
            || Some(frame.color_range) != self.last_input_color_range
            || scan_tuple != self.last_input_scan
            || color_adj_tuple != self.last_input_color_adj;

        if need_yuv_upload {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[target_slot]));
            gl.viewport(0, 0, target_width as i32, target_height as i32);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.clear(glow::COLOR_BUFFER_BIT);

            let mut rendered_input = false;

            if frame.format == Pixel::YUV422P
                || frame.format == Pixel::YUV420P
                || frame.format == Pixel::YUVJ422P
                || frame.format == Pixel::YUVJ420P
            {
                gl.use_program(Some(self.yuv_planar_prog));
                let y_end = (frame.width * frame.height) as usize;
                let chroma_width = frame.width.div_ceil(2);
                let chroma_height =
                    if frame.format == Pixel::YUV422P || frame.format == Pixel::YUVJ422P {
                        frame.height
                    } else {
                        frame.height.div_ceil(2)
                    };
                let chroma_plane_len = (chroma_width * chroma_height) as usize;
                let u_end = y_end + chroma_plane_len;
                let expected_len = y_end + 2 * chroma_plane_len;

                if frame.data.len() != expected_len {
                    tracing::warn!(
                        "Skipping frame with invalid planar data length: expected {}, got {}",
                        expected_len,
                        frame.data.len()
                    );
                    return self.pass_textures[target_slot];
                }

                let y_data = &frame.data[0..y_end];
                let u_data = &frame.data[y_end..u_end];
                let v_data = &frame.data[u_end..];

                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[0]));
                let needs_realloc = frame.width != self.last_frame_size.0
                    || frame.height != self.last_frame_size.1
                    || Some(frame.format) != self.last_frame_format;
                if needs_realloc {
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::R8 as i32,
                        frame.width as i32,
                        frame.height as i32,
                        0,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        None,
                    );
                }
                gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(self.pbos[0]));
                gl.buffer_data_size(
                    glow::PIXEL_UNPACK_BUFFER,
                    y_data.len() as i32,
                    glow::STREAM_DRAW,
                );
                gl.buffer_sub_data_u8_slice(glow::PIXEL_UNPACK_BUFFER, 0, y_data);
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    frame.width as i32,
                    frame.height as i32,
                    glow::RED,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::BufferOffset(0),
                );

                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[1]));
                if needs_realloc {
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::R8 as i32,
                        chroma_width as i32,
                        chroma_height as i32,
                        0,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        None,
                    );
                }
                gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(self.pbos[1]));
                gl.buffer_data_size(
                    glow::PIXEL_UNPACK_BUFFER,
                    u_data.len() as i32,
                    glow::STREAM_DRAW,
                );
                gl.buffer_sub_data_u8_slice(glow::PIXEL_UNPACK_BUFFER, 0, u_data);
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    chroma_width as i32,
                    chroma_height as i32,
                    glow::RED,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::BufferOffset(0),
                );

                gl.active_texture(glow::TEXTURE2);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[2]));
                if needs_realloc {
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::R8 as i32,
                        chroma_width as i32,
                        chroma_height as i32,
                        0,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        None,
                    );
                }
                gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(self.pbos[2]));
                gl.buffer_data_size(
                    glow::PIXEL_UNPACK_BUFFER,
                    v_data.len() as i32,
                    glow::STREAM_DRAW,
                );
                gl.buffer_sub_data_u8_slice(glow::PIXEL_UNPACK_BUFFER, 0, v_data);
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    chroma_width as i32,
                    chroma_height as i32,
                    glow::RED,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::BufferOffset(0),
                );

                gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
                gl.uniform_1_i32(Some(&self.yuv_range_loc), frame.color_range as i32);
                gl.uniform_2_f32(Some(&self.yuv_overscan_loc), overscan_x, overscan_y);
                gl.uniform_2_f32(
                    Some(&self.yuv_underscan_loc),
                    (1.0 + underscan_x).max(0.01),
                    (1.0 + underscan_y).max(0.01),
                );
                gl.uniform_1_f32(Some(&self.yuv_brightness_loc), brightness);
                gl.uniform_1_f32(Some(&self.yuv_gamma_loc), gamma);
                if needs_realloc {
                    self.last_frame_size = (frame.width, frame.height);
                    self.last_frame_format = Some(frame.format);
                }
                rendered_input = true;
            } else if frame.format == Pixel::YUYV422 {
                let expected_len = (frame.width * frame.height * 2) as usize;
                if frame.data.len() != expected_len {
                    tracing::warn!(
                        "Skipping frame with invalid YUYV data length: expected {}, got {}",
                        expected_len,
                        frame.data.len()
                    );
                    return self.pass_textures[target_slot];
                }

                gl.use_program(Some(self.yuyv_packed_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[0]));
                let needs_realloc = frame.width != self.last_frame_size.0
                    || frame.height != self.last_frame_size.1
                    || Some(frame.format) != self.last_frame_format;
                if needs_realloc {
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA8 as i32,
                        (frame.width / 2) as i32,
                        frame.height as i32,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        None,
                    );
                }
                gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(self.pbos[0]));
                gl.buffer_data_size(
                    glow::PIXEL_UNPACK_BUFFER,
                    frame.data.len() as i32,
                    glow::STREAM_DRAW,
                );
                gl.buffer_sub_data_u8_slice(glow::PIXEL_UNPACK_BUFFER, 0, &frame.data);
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    (frame.width / 2) as i32,
                    frame.height as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::BufferOffset(0),
                );
                gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
                if needs_realloc {
                    self.last_frame_size = (frame.width, frame.height);
                    self.last_frame_format = Some(frame.format);
                }
                gl.uniform_1_i32(Some(&self.yuyv_range_loc), frame.color_range as i32);
                gl.uniform_2_f32(Some(&self.yuyv_overscan_loc), overscan_x, overscan_y);
                gl.uniform_2_f32(
                    Some(&self.yuyv_underscan_loc),
                    (1.0 + underscan_x).max(0.01),
                    (1.0 + underscan_y).max(0.01),
                );
                gl.uniform_1_f32(Some(&self.yuyv_brightness_loc), brightness);
                gl.uniform_1_f32(Some(&self.yuyv_gamma_loc), gamma);
                rendered_input = true;
            } else if frame.format == Pixel::RGBA {
                let expected_len = (frame.width * frame.height * 4) as usize;
                if frame.data.len() != expected_len {
                    tracing::warn!(
                        "Skipping frame with invalid RGBA data length: expected {}, got {}",
                        expected_len,
                        frame.data.len()
                    );
                    return self.pass_textures[target_slot];
                }

                gl.use_program(Some(self.linearize_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[0]));
                let needs_realloc = frame.width != self.last_frame_size.0
                    || frame.height != self.last_frame_size.1
                    || Some(frame.format) != self.last_frame_format;
                if needs_realloc {
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA8 as i32,
                        frame.width as i32,
                        frame.height as i32,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        None,
                    );
                }
                gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(self.pbos[0]));
                gl.buffer_data_size(
                    glow::PIXEL_UNPACK_BUFFER,
                    frame.data.len() as i32,
                    glow::STREAM_DRAW,
                );
                gl.buffer_sub_data_u8_slice(glow::PIXEL_UNPACK_BUFFER, 0, &frame.data);
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    frame.width as i32,
                    frame.height as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::BufferOffset(0),
                );
                gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
                if needs_realloc {
                    self.last_frame_size = (frame.width, frame.height);
                    self.last_frame_format = Some(frame.format);
                }
                if let Some(ref loc) = self.linearize_sampler_loc {
                    gl.uniform_1_i32(Some(loc), 0);
                }
                if let Some(ref loc) = self.linearize_overscan_loc {
                    gl.uniform_2_f32(Some(loc), overscan_x, overscan_y);
                }
                if let Some(ref loc) = self.linearize_underscan_loc {
                    gl.uniform_2_f32(
                        Some(loc),
                        (1.0 + underscan_x).max(0.01),
                        (1.0 + underscan_y).max(0.01),
                    );
                }
                if let Some(ref loc) = self.linearize_brightness_loc {
                    gl.uniform_1_f32(Some(loc), brightness);
                }
                if let Some(ref loc) = self.linearize_gamma_loc {
                    gl.uniform_1_f32(Some(loc), gamma);
                }
                rendered_input = true;
            } else {
                tracing::warn!("Skipping unsupported pixel format: {:?}", frame.format);
            }

            if rendered_input {
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                self.last_input_slot = Some(target_slot);
                self.last_input_color_range = Some(frame.color_range);
                self.last_input_scan = scan_tuple;
                self.last_input_color_adj = color_adj_tuple;
            }
        }

        let mut tex = self.pass_textures[target_slot];

        // Apply FFT filter to raw frame data (at capture card resolution, before any scaling)
        if let Some(fft_arc) = fft_filter {
            let mut fft = fft_arc.lock().unwrap();
            let mask_gen = fft.mask_generation();
            let thresholds = (fft_mask_threshold, fft_black_threshold);
            let need_fft = need_yuv_upload || fft_params_changed;

            if need_fft {
                tex = fft.apply(
                    gl,
                    tex,
                    frame.width,
                    frame.height,
                    fft_mask_threshold,
                    fft_black_threshold,
                    render_fft_spectrum,
                );

                // Set filtering mode dynamically based on current params.scaler_filter
                let current_filter = if self.last_scaler_filter
                    == Some(crate::video::types::ScalerFilter::Point as u8)
                {
                    glow::NEAREST as i32
                } else {
                    glow::LINEAR as i32
                };
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, current_filter);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, current_filter);
                gl.bind_texture(glow::TEXTURE_2D, None);

                // Restore our VAO after FFT used its own
                gl.bind_vertex_array(Some(self.vertex_array));

                // If deinterlace history rotation is active, persist the post-FFT frame into
                // `target_slot` so future frames read the FFT-filtered `prev1`/`prev2` slot.
                if deinterlace_filter_enabled && tex != self.pass_textures[target_slot] {
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[target_slot]));
                    gl.viewport(0, 0, target_width as i32, target_height as i32);
                    gl.use_program(Some(self.blit_prog));
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                    gl.bind_texture(glow::TEXTURE_2D, None);
                    tex = self.pass_textures[target_slot];
                }

                self.last_fft_enabled = true;
                self.last_fft_thresholds = thresholds;
                self.last_fft_mask_gen = mask_gen;
                self.last_fft_spectrum_rendered = render_fft_spectrum;
            } else if let Some(cached) = self.last_input_tex {
                tex = cached;
            }
        } else {
            self.last_fft_enabled = false;
            self.last_fft_spectrum_rendered = false;
        }

        self.last_input_tex = Some(tex);
        tex
    }

    unsafe fn linearize_fallback_texture_into(
        &mut self,
        gl: &glow::Context,
        fallback_texture: glow::Texture,
        target_slot: usize,
        target_width: u32,
        target_height: u32,
        brightness: f32,
        gamma: f32,
    ) -> glow::Texture {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[target_slot]));
        gl.viewport(0, 0, target_width as i32, target_height as i32);
        gl.bind_vertex_array(Some(self.vertex_array));
        gl.use_program(Some(self.linearize_prog));
        if let Some(loc) = &self.linearize_sampler_loc {
            gl.uniform_1_i32(Some(loc), 0);
        }
        if let Some(loc) = &self.linearize_overscan_loc {
            gl.uniform_2_f32(Some(loc), 0.0, 0.0);
        }
        if let Some(loc) = &self.linearize_underscan_loc {
            gl.uniform_2_f32(Some(loc), 1.0, 1.0);
        }
        if let Some(loc) = &self.linearize_brightness_loc {
            gl.uniform_1_f32(Some(loc), brightness);
        }
        if let Some(loc) = &self.linearize_gamma_loc {
            gl.uniform_1_f32(Some(loc), gamma);
        }
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(fallback_texture));
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        gl.bind_texture(glow::TEXTURE_2D, None);
        self.pass_textures[target_slot]
    }

    /// Linearizes an incoming sRGB framebuffer texture (e.g. from Gamescope DMA-BUF)
    /// into `self.pass_textures[PASS_INPUT]` (half-float RGBA16F) so that all CRT filters,
    /// deinterlace, and passthrough presentation shaders receive linear RGB.
    #[allow(dead_code)]
    pub unsafe fn linearize_fallback_texture(
        &mut self,
        gl: &glow::Context,
        fallback_texture: glow::Texture,
        target_width: u32,
        target_height: u32,
        brightness: f32,
        gamma: f32,
    ) -> glow::Texture {
        self.history_ring.reset();
        self.last_frame_captured_at = -1;
        self.last_input_tex = None;
        self.last_input_slot = None;
        self.last_source_kind = Some(InputSourceKind::Fallback);
        self.last_fallback_tex = Some(fallback_texture);
        let tex = self.linearize_fallback_texture_into(
            gl,
            fallback_texture,
            PASS_INPUT,
            target_width,
            target_height,
            brightness,
            gamma,
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        tex
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn update_frame_history_and_prepare_input(
        &mut self,
        gl: &glow::Context,
        raw_frame: Option<&RawFrame>,
        keep_history: bool,
        target_width: u32,
        target_height: u32,
        overscan_x: f32,
        overscan_y: f32,
        underscan_x: f32,
        underscan_y: f32,
        brightness: f32,
        gamma: f32,
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
        render_fft_spectrum: bool,
    ) -> Option<glow::Texture> {
        let frame = raw_frame?;
        if self.last_source_kind != Some(InputSourceKind::Raw) {
            self.history_ring.reset();
            self.last_frame_captured_at = -1;
            self.prev_frame_captured_at = -1;
            self.last_input_tex = None;
            self.last_input_slot = None;
            self.last_fallback_tex = None;
            self.last_source_kind = Some(InputSourceKind::Raw);
        }

        let is_new_frame = frame.captured_at != self.last_frame_captured_at;
        let target_slot = self
            .history_ring
            .select_write_slot(keep_history, is_new_frame);

        let tex = self.prepare_input_texture(
            gl,
            frame,
            is_new_frame,
            target_slot,
            keep_history,
            target_width,
            target_height,
            overscan_x,
            overscan_y,
            underscan_x,
            underscan_y,
            brightness,
            gamma,
            fft_filter,
            fft_mask_threshold,
            fft_black_threshold,
            render_fft_spectrum,
        );

        if is_new_frame {
            self.prev_frame_captured_at = self.last_frame_captured_at;
            self.last_frame_captured_at = frame.captured_at;
        }

        gl.viewport(0, 0, target_width as i32, target_height as i32);
        Some(tex)
    }

    unsafe fn setup_post_framebuffer(
        &mut self,
        gl: &glow::Context,
        target_width: u32,
        target_height: u32,
    ) {
        let w = target_width.max(1);
        let h = target_height.max(1);
        if (w, h) != self.last_post_size {
            for i in 0..2 {
                gl.bind_texture(glow::TEXTURE_2D, Some(self.post_textures[i]));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    w as i32,
                    h as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    None,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_S,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_T,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.post_fbos[i]));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(self.post_textures[i]),
                    0,
                );
                gl.bind_texture(glow::TEXTURE_2D, None);
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            }
            self.last_post_size = (w, h);
        }
    }

    pub fn has_pending_ocr_readback(&self) -> bool {
        self.ocr_pending_readback.is_some()
    }

    unsafe fn setup_ocr_framebuffer(&mut self, gl: &glow::Context, w: u32, h: u32) {
        let w = w.max(1);
        let h = h.max(1);
        if (w, h) != self.last_ocr_fbo_size {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.ocr_texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                w as i32,
                h as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                None,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.ocr_fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(self.ocr_texture),
                0,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.last_ocr_fbo_size = (w, h);
        }
    }

    unsafe fn poll_async_ocr_readback(&mut self, gl: &glow::Context) {
        if let Some(pending) = &self.ocr_pending_readback {
            let status = gl.client_wait_sync(pending.fence, 0, 0);
            if status == glow::ALREADY_SIGNALED || status == glow::CONDITION_SATISFIED {
                let pending = self.ocr_pending_readback.take().unwrap();
                gl.delete_sync(pending.fence);
                let byte_len = (pending.width as usize) * (pending.height as usize) * 4;
                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(self.ocr_pbo));
                let mut buf = self.ocr_recycled_buffer.take().unwrap_or_default();
                if buf.len() != byte_len {
                    buf.resize(byte_len, 0);
                }
                let ptr = gl.map_buffer_range(
                    glow::PIXEL_PACK_BUFFER,
                    0,
                    byte_len as i32,
                    glow::MAP_READ_BIT,
                );
                if !ptr.is_null() {
                    std::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr(), byte_len);
                    gl.unmap_buffer(glow::PIXEL_PACK_BUFFER);
                    gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                    self.last_ocr_capture = Some((buf, pending.width, pending.height));
                } else {
                    gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                    self.ocr_recycled_buffer = Some(buf);
                }
            } else if status == glow::WAIT_FAILED {
                let pending = self.ocr_pending_readback.take().unwrap();
                gl.delete_sync(pending.fence);
            }
        }
    }

    unsafe fn start_async_ocr_readback(&mut self, gl: &glow::Context, area: RenderedArea) {
        if area.width == 0 || area.height == 0 || self.ocr_pending_readback.is_some() {
            return;
        }
        let byte_len = (area.width as usize) * (area.height as usize) * 4;
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(self.ocr_pbo));
        if self.ocr_pbo_size != byte_len {
            gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, byte_len as i32, glow::STREAM_READ);
            self.ocr_pbo_size = byte_len;
        }
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(
            area.x as i32,
            area.y as i32,
            area.width as i32,
            area.height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::BufferOffset(0),
        );
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
        if let Ok(fence) = gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
            self.ocr_pending_readback = Some(PendingOcrReadback {
                fence,
                width: area.width,
                height: area.height,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn preprocess_video_chain(
        &mut self,
        gl: &glow::Context,
        raw_frame: Option<&RawFrame>,
        fallback_texture: Option<glow::Texture>,
        fallback_captured_at: Option<i64>,
        resolution: (u32, u32),
        output_size: (f32, f32),
        scaler_filter: u8,
        fsr1_sharpness: f32,
        overscan_x: f32,
        overscan_y: f32,
        underscan_x: f32,
        underscan_y: f32,
        brightness: f32,
        gamma: f32,
        deinterlace_filter_enabled: bool,
        deinterlace_mode: u8,
        deinterlace_blend: f32,
        deinterlace_motion_threshold: f32,
        deinterlace_line_spacing: f32,
        deinterlace_spatial_mix: f32,
        framegen_req: FrameGenPaintRequest,
        median_filter_enabled: bool,
        median_mix: f32,
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
        render_fft_spectrum: bool,
        allow_srgb_passthrough_fastpath: bool,
    ) -> (glow::Texture, (u32, u32), bool) {
        let mut video_texture = fallback_texture;
        let framegen_enabled = framegen_req.enabled;
        let framegen_show_interpolated = framegen_req.enabled && framegen_req.show_interpolated;
        let keep_history = deinterlace_filter_enabled || framegen_enabled;
        let allow_srgb_passthrough_fastpath = allow_srgb_passthrough_fastpath && !framegen_enabled;

        let scaler = ScalerFilter::from_u8(scaler_filter);
        let is_anime4k = matches!(
            scaler,
            ScalerFilter::Anime4kSmall | ScalerFilter::Anime4kMedium | ScalerFilter::Anime4kLarge
        );
        let is_fsr1 = scaler == ScalerFilter::Fsr1;
        let is_gpu_upscaler = is_anime4k || is_fsr1;

        let effective_res = if is_anime4k {
            match scaler {
                ScalerFilter::Anime4kSmall => self.anime4k_small.get_upscaled_size(
                    resolution.0,
                    resolution.1,
                    output_size.0 as u32,
                    output_size.1 as u32,
                ),
                ScalerFilter::Anime4kMedium => self.anime4k_medium.get_upscaled_size(
                    resolution.0,
                    resolution.1,
                    output_size.0 as u32,
                    output_size.1 as u32,
                ),
                ScalerFilter::Anime4kLarge => self.anime4k_large.get_upscaled_size(
                    resolution.0,
                    resolution.1,
                    output_size.0 as u32,
                    output_size.1 as u32,
                ),
                _ => unreachable!(),
            }
        } else if is_fsr1 {
            self.fsr1.get_upscaled_size(
                resolution.0,
                resolution.1,
                output_size.0 as u32,
                output_size.1 as u32,
            )
        } else {
            resolution
        };

        if self.last_size != resolution
            || self.last_scaler_filter != Some(scaler_filter)
            || self.last_pass_res != effective_res
        {
            self.setup_framebuffers(
                gl,
                resolution.0,
                resolution.1,
                output_size.0 as u32,
                output_size.1 as u32,
                scaler_filter,
            );
            self.last_size = resolution;
            self.last_scaler_filter = Some(scaler_filter);
            self.last_pass_res = effective_res;
            self.last_preprocess_sig = None;
            self.last_preprocess_out = None;
            self.last_real_video_texture = None;
            self.last_mipmapped_frame.set(None);
            self.last_pixelate_key = None;
            // Reallocating pass textures invalidates prior GPU history slots.
            self.history_ring.reset();
            self.prev_frame_captured_at = -1;
            self.last_frame_captured_at = -1;
            self.last_input_tex = None;
            self.last_input_slot = None;
            self.framegen.reset_history();
        }

        let fft_gen = if raw_frame.is_some() {
            fft_filter
                .and_then(|arc| arc.lock().ok().map(|fft| fft.mask_generation()))
                .unwrap_or(0)
        } else {
            0
        };
        let current_sig = build_preprocess_cache_sig(
            raw_frame,
            fallback_texture,
            fallback_captured_at,
            resolution,
            effective_res,
            scaler_filter,
            if is_fsr1 { fsr1_sharpness.to_bits() } else { 0 },
            allow_srgb_passthrough_fastpath,
            [
                overscan_x.to_bits(),
                overscan_y.to_bits(),
                underscan_x.to_bits(),
                underscan_y.to_bits(),
                brightness.to_bits(),
                gamma.to_bits(),
            ],
            deinterlace_filter_enabled,
            deinterlace_mode,
            [
                deinterlace_blend.to_bits(),
                deinterlace_motion_threshold.to_bits(),
                deinterlace_line_spacing.to_bits(),
                deinterlace_spatial_mix.to_bits(),
            ],
            framegen_enabled,
            framegen_show_interpolated,
            if framegen_enabled {
                framegen_req.params.cache_bits()
            } else {
                [0; 3]
            },
            median_filter_enabled,
            median_mix.to_bits(),
            raw_frame.is_some() && fft_filter.is_some(),
            [fft_mask_threshold.to_bits(), fft_black_threshold.to_bits()],
            fft_gen,
        );
        gl.bind_vertex_array(Some(self.vertex_array));
        gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);

        let need_spectrum_refresh = raw_frame.is_some()
            && fft_filter.is_some()
            && render_fft_spectrum
            && !self.last_fft_spectrum_rendered;
        if !need_spectrum_refresh {
            if let (Some(sig), Some(cached_out)) = (current_sig, self.last_preprocess_out) {
                if self.last_preprocess_sig == Some(sig) {
                    return cached_out;
                }
            }
        }

        self.last_mipmapped_frame.set(None);
        self.last_pixelate_key = None;

        let mut input_is_srgb = false;
        if let Some(tex) = self.update_frame_history_and_prepare_input(
            gl,
            raw_frame,
            keep_history,
            resolution.0,
            resolution.1,
            overscan_x,
            overscan_y,
            underscan_x,
            underscan_y,
            brightness,
            gamma,
            fft_filter,
            fft_mask_threshold,
            fft_black_threshold,
            render_fft_spectrum,
        ) {
            video_texture = Some(tex);
        } else if let Some(tex) = fallback_texture {
            if allow_srgb_passthrough_fastpath
                && !deinterlace_filter_enabled
                && !framegen_enabled
                && !median_filter_enabled
                && !is_gpu_upscaler
                && (brightness - 1.0).abs() <= 0.0001
                && (gamma - 1.0).abs() <= 0.0001
            {
                self.history_ring.reset();
                self.last_source_kind = Some(InputSourceKind::Fallback);
                self.last_fallback_tex = Some(tex);
                self.last_input_slot = None;
                self.last_input_tex = None;
                self.prev_frame_captured_at = -1;
                self.last_frame_captured_at =
                    fallback_captured_at.filter(|&ts| ts > 0).unwrap_or(-1);
                video_texture = Some(tex);
                input_is_srgb = true;
            } else {
                if self.last_source_kind != Some(InputSourceKind::Fallback) {
                    self.history_ring.reset();
                    self.prev_frame_captured_at = -1;
                    self.last_frame_captured_at = -1;
                    self.last_input_slot = None;
                    self.last_input_tex = None;
                    self.last_fallback_tex = None;
                    self.last_source_kind = Some(InputSourceKind::Fallback);
                }
                let is_new_fallback_frame = match fallback_captured_at.filter(|&ts| ts > 0) {
                    Some(ts) => {
                        ts != self.last_frame_captured_at || self.last_fallback_tex != Some(tex)
                    }
                    None => true,
                };
                let target_slot = self
                    .history_ring
                    .select_write_slot(keep_history, is_new_fallback_frame);
                video_texture = Some(self.linearize_fallback_texture_into(
                    gl,
                    tex,
                    target_slot,
                    resolution.0,
                    resolution.1,
                    brightness,
                    gamma,
                ));
                self.last_fallback_tex = Some(tex);
                self.last_input_slot = Some(target_slot);
                self.last_input_tex = None;
                if is_new_fallback_frame {
                    self.prev_frame_captured_at = self.last_frame_captured_at;
                }
                self.last_frame_captured_at =
                    fallback_captured_at.filter(|&ts| ts > 0).unwrap_or(-1);
            }
        }

        let input_texture = video_texture.expect("No video texture available");
        let mut current_video_texture = input_texture;
        let mut current_res = resolution;

        // Apply Deinterlace filter before median / upscaling
        if deinterlace_filter_enabled {
            let (_, prev1_slot, prev2_slot, has_prev2) = self.history_ring.active_slots();
            let prev1_tex = if self.history_ring.count >= 2 {
                self.pass_textures[prev1_slot]
            } else {
                current_video_texture
            };
            let prev2_tex = if has_prev2 {
                self.pass_textures[prev2_slot]
            } else {
                prev1_tex
            };

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[PASS_DEINTERLACE]));
            gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);
            gl.use_program(Some(self.deinterlace_prog));
            if let Some(loc) = &self.deinterlace_current_frame_loc {
                gl.uniform_1_i32(Some(loc), 0);
            }
            if let Some(loc) = &self.deinterlace_prev_frame_loc {
                gl.uniform_1_i32(Some(loc), 1);
            }
            if let Some(loc) = &self.deinterlace_prev_frame_2_loc {
                gl.uniform_1_i32(Some(loc), 2);
            }
            if let Some(loc) = &self.deinterlace_has_prev2_loc {
                gl.uniform_1_i32(Some(loc), if has_prev2 { 1 } else { 0 });
            }
            gl.uniform_1_i32(Some(&self.deinterlace_mode_loc), deinterlace_mode as i32);
            gl.uniform_1_f32(Some(&self.deinterlace_blend_loc), deinterlace_blend);
            gl.uniform_1_f32(
                Some(&self.deinterlace_motion_thresh_loc),
                deinterlace_motion_threshold,
            );
            gl.uniform_1_f32(
                Some(&self.deinterlace_line_spacing_loc),
                deinterlace_line_spacing,
            );
            gl.uniform_1_f32(
                Some(&self.deinterlace_spatial_mix_loc),
                deinterlace_spatial_mix,
            );

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(current_video_texture));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(prev1_tex));
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, Some(prev2_tex));

            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);

            current_video_texture = self.pass_textures[PASS_DEINTERLACE];
        }

        // Snapshot the crisp un-interpolated real video texture before framegen so async/constant
        // OCR can always read the exact real frame without dropping an interpolated beat on screen.
        self.last_real_video_texture = Some((current_video_texture, current_res, input_is_srgb));

        // Apply AMD FidelityFX Optical Flow frame interpolation before median / upscaling / CRT.
        if framegen_enabled {
            let curr_captured_at = raw_frame
                .map(|f| f.captured_at)
                .or(fallback_captured_at)
                .unwrap_or(0);
            let (_, prev1_slot, _, _) = self.history_ring.active_slots();
            let has_prev = self.history_ring.count >= 2 && self.prev_frame_captured_at > 0;
            let prev_tex = if has_prev {
                self.pass_textures[prev1_slot]
            } else {
                current_video_texture
            };
            let interp_out = self.framegen.interpolate(
                gl,
                prev_tex,
                current_video_texture,
                self.prev_frame_captured_at,
                curr_captured_at,
                current_res.0,
                current_res.1,
                framegen_show_interpolated && has_prev,
                framegen_req.params,
            );
            if interp_out != current_video_texture {
                let filter_mode = if scaler == ScalerFilter::Point {
                    glow::NEAREST as i32
                } else {
                    glow::LINEAR as i32
                };
                gl.bind_texture(glow::TEXTURE_2D, Some(interp_out));
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter_mode);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter_mode);
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
            gl.bind_vertex_array(Some(self.vertex_array));
            gl.viewport(0, 0, current_res.0 as i32, current_res.1 as i32);
            current_video_texture = interp_out;
            self.last_framegen_active = true;
        } else {
            if self.last_framegen_active {
                self.framegen.reset_history();
            }
            self.last_framegen_active = false;
        }

        // Apply Median filter before upscaling
        if median_filter_enabled {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[PASS_MEDIAN]));
            gl.viewport(0, 0, current_res.0 as i32, current_res.1 as i32);
            gl.use_program(Some(self.median_prog));
            gl.uniform_1_f32(Some(&self.median_mix_loc), median_mix);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(current_video_texture));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            current_video_texture = self.pass_textures[PASS_MEDIAN];
        }

        if is_anime4k {
            let upscaled = match scaler {
                ScalerFilter::Anime4kSmall => self.anime4k_small.upscale(
                    gl,
                    current_video_texture,
                    current_res.0,
                    current_res.1,
                    output_size.0 as u32,
                    output_size.1 as u32,
                ),
                ScalerFilter::Anime4kMedium => self.anime4k_medium.upscale(
                    gl,
                    current_video_texture,
                    current_res.0,
                    current_res.1,
                    output_size.0 as u32,
                    output_size.1 as u32,
                ),
                ScalerFilter::Anime4kLarge => self.anime4k_large.upscale(
                    gl,
                    current_video_texture,
                    current_res.0,
                    current_res.1,
                    output_size.0 as u32,
                    output_size.1 as u32,
                ),
                _ => unreachable!(),
            };
            current_video_texture = upscaled.0;
            current_res = (upscaled.1, upscaled.2);
        } else if is_fsr1 {
            let upscaled = self.fsr1.upscale(
                gl,
                current_video_texture,
                current_res.0,
                current_res.1,
                output_size.0 as u32,
                output_size.1 as u32,
                fsr1_sharpness,
            );
            current_video_texture = upscaled.0;
            current_res = (upscaled.1, upscaled.2);
        }

        let out = (current_video_texture, current_res, input_is_srgb);
        self.last_fsr1_active = is_fsr1;
        self.last_preprocess_sig = current_sig;
        self.last_preprocess_out = Some(out);
        out
    }

    unsafe fn capture_caller_gl_state(
        gl: &glow::Context,
    ) -> (
        Option<glow::Framebuffer>,
        Option<glow::VertexArray>,
        bool,
        [i32; 4],
        bool,
    ) {
        let caller_fbo = NonZero::new(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32)
            .map(glow::NativeFramebuffer);
        let old_vao = NonZero::new(gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING) as u32)
            .map(glow::NativeVertexArray);
        let scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);
        let mut caller_scissor = [0i32; 4];
        if scissor_enabled {
            gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut caller_scissor);
            gl.disable(glow::SCISSOR_TEST);
        }
        let blend_enabled = gl.is_enabled(glow::BLEND);
        if blend_enabled {
            gl.disable(glow::BLEND);
        }
        (
            caller_fbo,
            old_vao,
            scissor_enabled,
            caller_scissor,
            blend_enabled,
        )
    }

    unsafe fn prepare_postprocess_targets(
        &mut self,
        gl: &glow::Context,
        output_size: (f32, f32),
        caller_fbo: Option<glow::Framebuffer>,
        cathode_params: Option<&CathodeInterferenceShaderParams>,
        audio_reactive_params: Option<&AudioReactiveShaderParams>,
        glass_params: Option<&GlassShaderParams>,
    ) -> (Option<glow::Framebuffer>, bool) {
        let run_cathode = cathode_params
            .map(|c| c.enabled && c.intensity > 0.001)
            .unwrap_or(false);
        let run_audio_reactive = audio_reactive_params
            .map(|a| a.enabled && a.master_intensity > 0.001)
            .unwrap_or(false);
        let run_glass = glass_params
            .map(|g| g.enabled && g.intensity > 0.001)
            .unwrap_or(false);
        let needs_post_fbo = run_cathode || run_audio_reactive || run_glass;

        let upstream_target_fbo = if needs_post_fbo {
            self.setup_post_framebuffer(gl, output_size.0 as u32, output_size.1 as u32);
            Some(self.post_fbos[0])
        } else {
            caller_fbo
        };

        (upstream_target_fbo, needs_post_fbo)
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn postprocess_and_present(
        &mut self,
        gl: &glow::Context,
        painter: &egui_glow::Painter,
        widget_rect: egui::Rect,
        output_size: (f32, f32),
        final_input_res: (u32, u32),
        horizontal_stretch: f32,
        s_mask: f32,
        scan_str: f32,
        b_boost: f32,
        cathode_params: Option<&CathodeInterferenceShaderParams>,
        audio_reactive_params: Option<&AudioReactiveShaderParams>,
        glass_params: Option<&GlassShaderParams>,
        time: f32,
        skip_popup: bool,
        software_mouse_pos: Option<(f32, f32)>,
        software_mouse_clip: Option<[i32; 4]>,
        caller_fbo: Option<glow::Framebuffer>,
        needs_post_fbo: bool,
        scissor_enabled: bool,
        caller_scissor: [i32; 4],
        blend_enabled: bool,
        old_vao: Option<glow::VertexArray>,
    ) {
        let run_cathode = cathode_params
            .map(|c| c.enabled && c.intensity > 0.001)
            .unwrap_or(false);
        let run_audio_reactive = audio_reactive_params
            .map(|a| a.enabled && a.master_intensity > 0.001)
            .unwrap_or(false);
        let run_glass = glass_params
            .map(|g| g.enabled && g.intensity > 0.001)
            .unwrap_or(false);
        let has_popup = !skip_popup && !self.popup_primitives.is_empty();
        let has_comments = !self.comment_primitives.is_empty();
        let has_alerts = !self.alert_primitives.is_empty();
        let has_overlay = has_popup || has_comments || has_alerts;

        // Ensure no stale scissor remains enabled if an upstream pass turned it on.
        gl.disable(glow::SCISSOR_TEST);

        let overlay_target_fbo = if needs_post_fbo {
            Some(self.post_fbos[0])
        } else {
            caller_fbo
        };
        let overlay_clip_limit = if !needs_post_fbo && scissor_enabled {
            Some(caller_scissor)
        } else {
            None
        };
        let mut scissor_dirty = false;
        let mut blend_dirty = false;

        // Niconico comments and alerts sit on the CRT image (below the dictionary
        // popup) and are kept in mining captures, unlike the popup. Batch all active
        // overlay groups into one VBO upload and one EBO upload while preserving order.
        if has_overlay {
            let mut groups: [&[egui::ClippedPrimitive]; 3] = [&[], &[], &[]];
            let mut group_count = 0;
            if has_comments {
                groups[group_count] = &self.comment_primitives;
                group_count += 1;
            }
            if has_alerts {
                groups[group_count] = &self.alert_primitives;
                group_count += 1;
            }
            if has_popup {
                groups[group_count] = &self.popup_primitives;
                group_count += 1;
            }

            assemble_overlay_batch(
                &groups[..group_count],
                widget_rect,
                output_size,
                overlay_clip_limit,
                &mut self.overlay_vertices_scratch,
                &mut self.overlay_indices_scratch,
                &mut self.overlay_draws_scratch,
            );

            if !self.overlay_draws_scratch.is_empty() {
                gl.bind_framebuffer(glow::FRAMEBUFFER, overlay_target_fbo);
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
                self.flush_overlay_batch_gl(
                    gl,
                    painter,
                    output_size,
                    widget_rect,
                    s_mask,
                    scan_str,
                    b_boost,
                );
                scissor_dirty = true;
                blend_dirty = true;
            }

            if has_comments {
                self.comment_primitives.clear();
            }
            if has_alerts {
                self.alert_primitives.clear();
            }
            if has_popup {
                self.popup_primitives.clear();
            }
        }

        if let Some(mouse_pos) = software_mouse_pos {
            let has_clip_constraint = software_mouse_clip.is_some() || overlay_clip_limit.is_some();
            let effective_mouse_clip = match (software_mouse_clip, overlay_clip_limit) {
                (Some(a), Some(b)) => intersect_scissor_rects(a, b),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };
            if !has_clip_constraint || effective_mouse_clip.is_some() {
                gl.bind_framebuffer(glow::FRAMEBUFFER, overlay_target_fbo);
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
                self.draw_software_mouse_internal(
                    gl,
                    mouse_pos,
                    output_size,
                    s_mask,
                    scan_str,
                    b_boost,
                    output_size.0 / widget_rect.width(),
                    effective_mouse_clip,
                );
                if effective_mouse_clip.is_some() {
                    scissor_dirty = true;
                }
                blend_dirty = true;
            }
        }

        if scissor_enabled && scissor_dirty {
            gl.scissor(
                caller_scissor[0],
                caller_scissor[1],
                caller_scissor[2],
                caller_scissor[3],
            );
        }

        let mut current_tex_idx: usize = 0;

        if run_cathode {
            let cp = cathode_params.unwrap();
            let cathode_input = self.post_textures[current_tex_idx];
            let has_more_passes = run_audio_reactive || run_glass;
            let target_fbo = if has_more_passes {
                Some(self.post_fbos[1 - current_tex_idx])
            } else {
                caller_fbo
            };
            let scissor_cathode = scissor_enabled && !has_more_passes;
            self.draw_cathode_pass(
                gl,
                cathode_input,
                output_size,
                final_input_res,
                horizontal_stretch,
                time,
                cp,
                target_fbo,
                scissor_cathode,
            );
            if has_more_passes {
                current_tex_idx = 1 - current_tex_idx;
            }
        }

        if run_audio_reactive {
            let arp = audio_reactive_params.unwrap();
            let ar_input = self.post_textures[current_tex_idx];
            let has_more_passes = run_glass;
            let target_fbo = if has_more_passes {
                Some(self.post_fbos[1 - current_tex_idx])
            } else {
                caller_fbo
            };
            let scissor_ar = scissor_enabled && !has_more_passes;
            self.draw_audio_reactive_pass(
                gl,
                ar_input,
                output_size,
                final_input_res,
                arp,
                time,
                target_fbo,
                scissor_ar,
            );
            if has_more_passes {
                current_tex_idx = 1 - current_tex_idx;
            }
        }

        if run_glass {
            let gp = glass_params.unwrap();
            let glass_input = self.post_textures[current_tex_idx];
            let scissor_glass = scissor_enabled;
            self.draw_glass_pass(
                gl,
                glass_input,
                output_size,
                final_input_res,
                gp,
                time,
                caller_fbo,
                scissor_glass,
            );
        }

        self.mipmap_generated_this_paint.set(false);

        if scissor_enabled {
            gl.enable(glow::SCISSOR_TEST);
        } else {
            gl.disable(glow::SCISSOR_TEST);
        }

        if blend_dirty && blend_enabled {
            gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE_MINUS_DST_ALPHA,
                glow::ONE,
            );
        }
        if blend_enabled {
            gl.enable(glow::BLEND);
        } else {
            gl.disable(glow::BLEND);
        }

        gl.bind_vertex_array(old_vao);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        gl: &glow::Context,
        painter: &egui_glow::Painter,
        widget_rect: egui::Rect,
        raw_frame: Option<&RawFrame>,
        fallback_texture: Option<glow::Texture>,
        resolution: (u32, u32),
        output_size: (f32, f32),
        params: &ShaderParams,
        halo_params: &HaloShaderParams,
        beam_params: &BeamShaderParams,
        cathode_params: &CathodeInterferenceShaderParams,
        audio_reactive_params: Option<&AudioReactiveShaderParams>,
        glass_params: Option<&GlassShaderParams>,
        time: f32,
        run_pixelate: bool,
        run_lottes: bool,
        run_halo: bool,
        run_beam: bool,
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
        software_mouse_pos: Option<(f32, f32)>,
        software_mouse_clip: Option<[i32; 4]>,
        skip_popup: bool,
    ) -> RenderedArea {
        unsafe {
            let (caller_fbo, old_vao, scissor_enabled, caller_scissor, blend_enabled) =
                Self::capture_caller_gl_state(gl);

            self.current_source_frame_id = raw_frame
                .map(|f| f.captured_at)
                .or(params.fallback_captured_at)
                .unwrap_or(0);

            let framegen_req = self.framegen_next_paint.take().unwrap_or_default();
            let allow_srgb_fastpath =
                !run_lottes && !run_halo && !run_beam && !run_pixelate && !framegen_req.enabled;
            let (current_video_texture, current_res, input_is_srgb) = self.preprocess_video_chain(
                gl,
                raw_frame,
                fallback_texture,
                params.fallback_captured_at,
                resolution,
                output_size,
                params.scaler_filter,
                params.fsr1_sharpness,
                params.overscan_x,
                params.overscan_y,
                params.underscan_x,
                params.underscan_y,
                params.brightness,
                params.gamma,
                params.deinterlace_filter_enabled,
                params.deinterlace_mode,
                params.deinterlace_blend,
                params.deinterlace_motion_threshold,
                params.deinterlace_line_spacing,
                params.deinterlace_spatial_mix,
                framegen_req,
                params.median_filter_enabled,
                params.median_mix,
                fft_filter,
                fft_mask_threshold,
                fft_black_threshold,
                params.render_fft_spectrum,
                allow_srgb_fastpath,
            );

            self.poll_async_ocr_readback(gl);

            if self.capture_ocr_async_next_paint {
                self.capture_ocr_async_next_paint = false;
                if self.ocr_pending_readback.is_none() && self.last_ocr_capture.is_none() {
                    let (ocr_tex, ocr_base_res, ocr_input_is_srgb) =
                        if framegen_req.enabled && framegen_req.show_interpolated {
                            self.last_real_video_texture.unwrap_or((
                                current_video_texture,
                                current_res,
                                input_is_srgb,
                            ))
                        } else {
                            (current_video_texture, current_res, input_is_srgb)
                        };
                    let ocr_src_res = if run_pixelate {
                        pixelate_subrender_size(ocr_base_res.0, ocr_base_res.1)
                    } else {
                        ocr_base_res
                    };
                    let full_clean_area =
                        RenderedArea::fit(ocr_src_res, output_size, params.horizontal_stretch);
                    if full_clean_area.width > 0 && full_clean_area.height > 0 {
                        let ocr_max_dim = ocr_src_res.0.max(ocr_src_res.1).clamp(1280, 1920);
                        let (target_w, target_h) =
                            crate::ocr::screen_ai::calculate_screen_ai_target_dimensions(
                                full_clean_area.width,
                                full_clean_area.height,
                                ocr_max_dim,
                            );
                        let target_w = target_w.max(1);
                        let target_h = target_h.max(1);
                        self.setup_ocr_framebuffer(gl, target_w, target_h);
                        let stretch_for_exact_fit = if ocr_src_res.0 > 0 && ocr_src_res.1 > 0 {
                            (target_w as f32 * ocr_src_res.1 as f32)
                                / (target_h as f32 * ocr_src_res.0 as f32)
                        } else {
                            params.horizontal_stretch
                        };
                        let _ = self.draw_passthrough_internal(
                            gl,
                            ocr_tex,
                            ocr_src_res,
                            (target_w as f32, target_h as f32),
                            [0.0, 0.0, 0.0],
                            stretch_for_exact_fit,
                            1.0,
                            params.scaler_filter,
                            params.border_crop,
                            Some(self.ocr_fbo),
                            ocr_input_is_srgb,
                            if run_lottes {
                                OcrProjection::lottes(params.warp_x, params.warp_y)
                            } else if run_halo {
                                OcrProjection::halo(halo_params.curvature, halo_params.halo_zoom)
                            } else if run_beam {
                                OcrProjection::beam(
                                    beam_params.curvature,
                                    beam_params.warp_x,
                                    beam_params.warp_y,
                                    beam_params.screen_scale,
                                )
                            } else {
                                OcrProjection::default()
                            },
                        );
                        self.start_async_ocr_readback(
                            gl,
                            RenderedArea {
                                x: 0,
                                y: 0,
                                width: target_w,
                                height: target_h,
                            },
                        );
                        gl.bind_framebuffer(glow::FRAMEBUFFER, caller_fbo);
                    }
                }
            }

            if self.capture_ocr_next_paint {
                self.capture_ocr_next_paint = false;
                let (ocr_tex, ocr_base_res, ocr_input_is_srgb) =
                    if framegen_req.enabled && framegen_req.show_interpolated {
                        self.last_real_video_texture.unwrap_or((
                            current_video_texture,
                            current_res,
                            input_is_srgb,
                        ))
                    } else {
                        (current_video_texture, current_res, input_is_srgb)
                    };
                self.setup_post_framebuffer(gl, output_size.0 as u32, output_size.1 as u32);
                let clean_area = self.draw_passthrough_internal(
                    gl,
                    ocr_tex,
                    if run_pixelate {
                        pixelate_subrender_size(ocr_base_res.0, ocr_base_res.1)
                    } else {
                        ocr_base_res
                    },
                    output_size,
                    [0.0, 0.0, 0.0],
                    params.horizontal_stretch,
                    1.0,
                    params.scaler_filter,
                    params.border_crop,
                    Some(self.post_fbos[0]),
                    ocr_input_is_srgb,
                    if run_lottes {
                        OcrProjection::lottes(params.warp_x, params.warp_y)
                    } else if run_halo {
                        OcrProjection::halo(halo_params.curvature, halo_params.halo_zoom)
                    } else if run_beam {
                        OcrProjection::beam(
                            beam_params.curvature,
                            beam_params.warp_x,
                            beam_params.warp_y,
                            beam_params.screen_scale,
                        )
                    } else {
                        OcrProjection::default()
                    },
                );
                self.last_ocr_capture = read_framebuffer_pixels_raw(gl, clean_area);
                gl.bind_framebuffer(glow::FRAMEBUFFER, caller_fbo);
            }

            let mut final_input_texture = current_video_texture;
            let mut final_input_res = current_res;

            if run_pixelate {
                let pixelate_res = pixelate_subrender_size(current_res.0, current_res.1);
                let pixelate_key = (
                    current_video_texture,
                    self.current_source_frame_id,
                    pixelate_res,
                );
                let need_pixelate = self.current_source_frame_id <= 0
                    || self.last_pixelate_key != Some(pixelate_key);
                if need_pixelate {
                    gl.bind_vertex_array(Some(self.vertex_array));
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[PASS_PIXELATE]));
                    gl.viewport(0, 0, pixelate_res.0 as i32, pixelate_res.1 as i32);
                    gl.use_program(Some(self.pixelate_prog));
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(current_video_texture));
                    gl.uniform_2_f32(
                        Some(&self.p_pixelate_target_res_loc),
                        pixelate_res.0 as f32,
                        pixelate_res.1 as f32,
                    );
                    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                    self.last_pixelate_key = if self.current_source_frame_id > 0 {
                        Some(pixelate_key)
                    } else {
                        None
                    };
                    self.last_mipmapped_frame.set(None);
                }
                final_input_texture = self.pass_textures[PASS_PIXELATE];
                final_input_res = pixelate_res;
            }
            self.last_video_texture = Some(final_input_texture);
            self.last_video_texture_res = final_input_res;
            self.last_video_tex_base_filter = if final_input_texture
                == self.pass_textures[PASS_PIXELATE]
                || (!input_is_srgb && params.scaler_filter == ScalerFilter::Point as u8)
            {
                glow::NEAREST as i32
            } else {
                glow::LINEAR as i32
            };

            let (upstream_target_fbo, needs_post_fbo) = self.prepare_postprocess_targets(
                gl,
                output_size,
                caller_fbo,
                Some(cathode_params),
                audio_reactive_params,
                glass_params,
            );

            if run_lottes {
                gl.bind_vertex_array(Some(self.vertex_array));
                gl.bind_framebuffer(glow::FRAMEBUFFER, upstream_target_fbo);
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
                gl.use_program(Some(self.final_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(final_input_texture));

                gl.uniform_2_f32(
                    Some(&self.final_output_res_loc),
                    output_size.0,
                    output_size.1,
                );
                gl.uniform_1_f32(Some(&self.final_hard_scan_loc), params.hard_scan);
                gl.uniform_1_f32(Some(&self.final_hard_pix_loc), params.hard_pix);
                gl.uniform_1_f32(Some(&self.final_warp_x_loc), params.warp_x);
                gl.uniform_1_f32(Some(&self.final_warp_y_loc), params.warp_y);
                gl.uniform_1_f32(Some(&self.final_shadow_mask_loc), params.shadow_mask);
                gl.uniform_1_f32(Some(&self.final_brightboost_loc), params.brightboost);
                gl.uniform_1_f32(Some(&self.final_hard_bloom_pix_loc), params.hard_bloom_pix);
                gl.uniform_1_f32(
                    Some(&self.final_hard_bloom_scan_loc),
                    params.hard_bloom_scan,
                );
                gl.uniform_1_f32(Some(&self.final_bloom_amount_loc), params.bloom_amount);
                gl.uniform_1_f32(Some(&self.final_shape_loc), params.shape);
                gl.uniform_3_f32(
                    Some(&self.final_background_color_loc),
                    params.background_color[0],
                    params.background_color[1],
                    params.background_color[2],
                );
                gl.uniform_1_f32(
                    Some(&self.final_horizontal_stretch_loc),
                    params.horizontal_stretch,
                );
                gl.uniform_1_f32(Some(&self.final_vibrance_loc), params.vibrance);
                gl.uniform_4_f32(
                    Some(&self.final_border_crop_loc),
                    params.border_crop[0],
                    params.border_crop[1],
                    params.border_crop[2],
                    params.border_crop[3],
                );
                if scissor_enabled && !needs_post_fbo {
                    gl.enable(glow::SCISSOR_TEST);
                }
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            } else if run_halo {
                self.halo.paint(
                    gl,
                    final_input_texture,
                    final_input_res,
                    output_size,
                    halo_params,
                    scissor_enabled && !needs_post_fbo,
                    upstream_target_fbo,
                    !run_pixelate,
                );
            } else if run_beam {
                self.beam.paint(
                    gl,
                    final_input_texture,
                    final_input_res,
                    output_size,
                    beam_params,
                    scissor_enabled && !needs_post_fbo,
                    upstream_target_fbo,
                );
            } else if run_pixelate {
                gl.bind_vertex_array(Some(self.vertex_array));
                gl.bind_framebuffer(glow::FRAMEBUFFER, upstream_target_fbo);
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
                gl.use_program(Some(self.passthrough_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(final_input_texture));
                gl.uniform_2_f32(
                    Some(&self.p_passthrough_output_res_loc),
                    output_size.0,
                    output_size.1,
                );
                gl.uniform_3_f32(
                    Some(&self.passthrough_background_color_loc),
                    params.background_color[0],
                    params.background_color[1],
                    params.background_color[2],
                );
                gl.uniform_1_f32(
                    Some(&self.passthrough_horizontal_stretch_loc),
                    params.horizontal_stretch,
                );
                gl.uniform_1_f32(Some(&self.passthrough_vibrance_loc), params.vibrance);
                // This program may have just rendered a curved OCR capture.
                gl.uniform_2_f32(Some(&self.passthrough_ocr_warp_loc), 0.0, 0.0);
                gl.uniform_1_f32(Some(&self.passthrough_ocr_zoom_loc), 1.0);
                gl.uniform_1_f32(
                    Some(&self.passthrough_image_aspect_loc),
                    final_input_res.0 as f32 / final_input_res.1 as f32,
                );
                gl.uniform_1_i32(
                    Some(&self.passthrough_scaler_filter_loc),
                    params.scaler_filter as i32,
                );
                gl.uniform_4_f32(
                    Some(&self.passthrough_border_crop_loc),
                    params.border_crop[0],
                    params.border_crop[1],
                    params.border_crop[2],
                    params.border_crop[3],
                );
                if let Some(loc) = &self.passthrough_input_is_srgb_loc {
                    gl.uniform_1_i32(Some(loc), 0);
                }
                if scissor_enabled && !needs_post_fbo {
                    gl.enable(glow::SCISSOR_TEST);
                }
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            } else {
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
                if scissor_enabled && !needs_post_fbo {
                    gl.enable(glow::SCISSOR_TEST);
                }
                self.draw_passthrough_internal(
                    gl,
                    current_video_texture,
                    current_res,
                    output_size,
                    params.background_color,
                    params.horizontal_stretch,
                    params.vibrance,
                    params.scaler_filter,
                    params.border_crop,
                    upstream_target_fbo,
                    input_is_srgb,
                    OcrProjection::default(),
                );
            }

            let (s_mask, scan_str, b_boost) = if run_lottes {
                (
                    params.shadow_mask,
                    (params.hard_scan / 30.0).clamp(0.0, 1.0),
                    params.brightboost.max(1.0),
                )
            } else if run_halo {
                (
                    halo_params.shadow_mask,
                    0.5,
                    halo_params.brightboost.max(1.0),
                )
            } else if run_beam {
                (
                    beam_params.shadow_mask,
                    beam_params.scanline_strength.clamp(0.0, 1.0),
                    beam_params.brightboost.max(1.0),
                )
            } else {
                (0.0, 0.0, 1.0)
            };

            self.postprocess_and_present(
                gl,
                painter,
                widget_rect,
                output_size,
                final_input_res,
                params.horizontal_stretch,
                s_mask,
                scan_str,
                b_boost,
                Some(cathode_params),
                audio_reactive_params,
                glass_params,
                time,
                skip_popup,
                software_mouse_pos,
                software_mouse_clip,
                caller_fbo,
                needs_post_fbo,
                scissor_enabled,
                caller_scissor,
                blend_enabled,
                old_vao,
            );

            RenderedArea::fit(final_input_res, output_size, params.horizontal_stretch)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_passthrough(
        &mut self,
        gl: &glow::Context,
        painter: &egui_glow::Painter,
        widget_rect: egui::Rect,
        raw_frame: Option<&RawFrame>,
        fallback_texture: Option<glow::Texture>,
        resolution: (u32, u32),
        output_size: (f32, f32),
        background_color: [f32; 3],
        horizontal_stretch: f32,
        median_filter_enabled: bool,
        median_mix: f32,
        deinterlace_filter_enabled: bool,
        deinterlace_mode: u8,
        deinterlace_blend: f32,
        deinterlace_motion_threshold: f32,
        deinterlace_line_spacing: f32,
        deinterlace_spatial_mix: f32,
        vibrance: f32,
        brightness: f32,
        gamma: f32,
        scaler_filter: u8,
        fsr1_sharpness: f32,
        overscan_x: f32,
        overscan_y: f32,
        underscan_x: f32,
        underscan_y: f32,
        border_crop: [f32; 4],
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
        cathode_params: Option<&CathodeInterferenceShaderParams>,
        audio_reactive_params: Option<&AudioReactiveShaderParams>,
        glass_params: Option<&GlassShaderParams>,
        time: f32,
        software_mouse_pos: Option<(f32, f32)>,
        software_mouse_clip: Option<[i32; 4]>,
        skip_popup: bool,
    ) -> RenderedArea {
        unsafe {
            let (caller_fbo, old_vao, scissor_enabled, caller_scissor, blend_enabled) =
                Self::capture_caller_gl_state(gl);

            let fallback_captured_at = self.fallback_captured_at_next_paint.take();
            let framegen_req = self.framegen_next_paint.take().unwrap_or_default();
            let render_fft_spectrum = self.render_fft_spectrum_next_paint;
            self.render_fft_spectrum_next_paint = false;
            self.current_source_frame_id = raw_frame
                .map(|f| f.captured_at)
                .or(fallback_captured_at)
                .unwrap_or(0);

            let (current_video_texture, current_res, input_is_srgb) = self.preprocess_video_chain(
                gl,
                raw_frame,
                fallback_texture,
                fallback_captured_at,
                resolution,
                output_size,
                scaler_filter,
                fsr1_sharpness,
                overscan_x,
                overscan_y,
                underscan_x,
                underscan_y,
                brightness,
                gamma,
                deinterlace_filter_enabled,
                deinterlace_mode,
                deinterlace_blend,
                deinterlace_motion_threshold,
                deinterlace_line_spacing,
                deinterlace_spatial_mix,
                framegen_req,
                median_filter_enabled,
                median_mix,
                fft_filter,
                fft_mask_threshold,
                fft_black_threshold,
                render_fft_spectrum,
                true,
            );
            self.last_video_texture = Some(current_video_texture);
            self.last_video_texture_res = current_res;
            self.last_video_tex_base_filter =
                if !input_is_srgb && scaler_filter == ScalerFilter::Point as u8 {
                    glow::NEAREST as i32
                } else {
                    glow::LINEAR as i32
                };

            let (upstream_target_fbo, needs_post_fbo) = self.prepare_postprocess_targets(
                gl,
                output_size,
                caller_fbo,
                cathode_params,
                audio_reactive_params,
                glass_params,
            );

            if scissor_enabled && !needs_post_fbo {
                gl.enable(glow::SCISSOR_TEST);
            }
            let rendered_area = self.draw_passthrough_internal(
                gl,
                current_video_texture,
                current_res,
                output_size,
                background_color,
                horizontal_stretch,
                vibrance,
                scaler_filter,
                border_crop,
                upstream_target_fbo,
                input_is_srgb,
                OcrProjection::default(),
            );

            self.poll_async_ocr_readback(gl);

            if self.capture_ocr_async_next_paint {
                self.capture_ocr_async_next_paint = false;
                if self.ocr_pending_readback.is_none()
                    && self.last_ocr_capture.is_none()
                    && rendered_area.width > 0
                    && rendered_area.height > 0
                {
                    let (ocr_tex, ocr_res, ocr_input_is_srgb) =
                        if framegen_req.enabled && framegen_req.show_interpolated {
                            self.last_real_video_texture.unwrap_or((
                                current_video_texture,
                                current_res,
                                input_is_srgb,
                            ))
                        } else {
                            (current_video_texture, current_res, input_is_srgb)
                        };
                    let ocr_max_dim = ocr_res.0.max(ocr_res.1).clamp(1280, 1920);
                    let (target_w, target_h) =
                        crate::ocr::screen_ai::calculate_screen_ai_target_dimensions(
                            rendered_area.width,
                            rendered_area.height,
                            ocr_max_dim,
                        );
                    let target_w = target_w.max(1);
                    let target_h = target_h.max(1);
                    self.setup_ocr_framebuffer(gl, target_w, target_h);
                    let stretch_for_exact_fit = if ocr_res.0 > 0 && ocr_res.1 > 0 {
                        (target_w as f32 * ocr_res.1 as f32) / (target_h as f32 * ocr_res.0 as f32)
                    } else {
                        horizontal_stretch
                    };
                    if scissor_enabled && !needs_post_fbo {
                        gl.disable(glow::SCISSOR_TEST);
                    }
                    let _ = self.draw_passthrough_internal(
                        gl,
                        ocr_tex,
                        ocr_res,
                        (target_w as f32, target_h as f32),
                        [0.0, 0.0, 0.0],
                        stretch_for_exact_fit,
                        1.0,
                        scaler_filter,
                        border_crop,
                        Some(self.ocr_fbo),
                        ocr_input_is_srgb,
                        OcrProjection::default(),
                    );
                    self.start_async_ocr_readback(
                        gl,
                        RenderedArea {
                            x: 0,
                            y: 0,
                            width: target_w,
                            height: target_h,
                        },
                    );
                    gl.bind_framebuffer(glow::FRAMEBUFFER, upstream_target_fbo);
                }
            }

            if self.capture_ocr_next_paint {
                self.capture_ocr_next_paint = false;
                self.last_ocr_capture = read_framebuffer_pixels_raw(gl, rendered_area);
            }

            self.postprocess_and_present(
                gl,
                painter,
                widget_rect,
                output_size,
                current_res,
                horizontal_stretch,
                0.0,
                0.0,
                1.0,
                cathode_params,
                audio_reactive_params,
                glass_params,
                time,
                skip_popup,
                software_mouse_pos,
                software_mouse_clip,
                caller_fbo,
                needs_post_fbo,
                scissor_enabled,
                caller_scissor,
                blend_enabled,
                old_vao,
            );

            rendered_area
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_passthrough_internal(
        &self,
        gl: &glow::Context,
        video_texture: glow::Texture,
        resolution: (u32, u32),
        output_size: (f32, f32),
        background_color: [f32; 3],
        horizontal_stretch: f32,
        vibrance: f32,
        scaler_filter: u8,
        border_crop: [f32; 4],
        target_fbo: Option<glow::Framebuffer>,
        input_is_srgb: bool,
        ocr_projection: OcrProjection,
    ) -> RenderedArea {
        let final_input_texture = video_texture;

        gl.bind_vertex_array(Some(self.vertex_array));
        gl.bind_framebuffer(glow::FRAMEBUFFER, target_fbo);
        gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
        gl.use_program(Some(self.passthrough_prog));
        gl.uniform_1_f32(
            Some(&self.passthrough_image_aspect_loc),
            resolution.0 as f32 / resolution.1 as f32,
        );
        gl.uniform_2_f32(
            Some(&self.passthrough_ocr_warp_loc),
            ocr_projection.warp[0],
            ocr_projection.warp[1],
        );
        gl.uniform_1_f32(Some(&self.passthrough_ocr_zoom_loc), ocr_projection.zoom);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(final_input_texture));

        if let Some(loc) = &self.passthrough_input_is_srgb_loc {
            gl.uniform_1_i32(Some(loc), if input_is_srgb { 1 } else { 0 });
        }

        gl.uniform_2_f32(
            Some(&self.p_passthrough_output_res_loc),
            output_size.0,
            output_size.1,
        );
        gl.uniform_3_f32(
            Some(&self.passthrough_background_color_loc),
            background_color[0],
            background_color[1],
            background_color[2],
        );
        gl.uniform_1_f32(
            Some(&self.passthrough_horizontal_stretch_loc),
            horizontal_stretch,
        );
        gl.uniform_1_f32(Some(&self.passthrough_vibrance_loc), vibrance);
        gl.uniform_1_i32(
            Some(&self.passthrough_scaler_filter_loc),
            scaler_filter as i32,
        );
        gl.uniform_4_f32(
            Some(&self.passthrough_border_crop_loc),
            border_crop[0],
            border_crop[1],
            border_crop[2],
            border_crop[3],
        );

        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        RenderedArea::fit(resolution, output_size, horizontal_stretch)
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_cathode_pass(
        &self,
        gl: &glow::Context,
        input_texture: glow::Texture,
        output_size: (f32, f32),
        resolution: (u32, u32),
        horizontal_stretch: f32,
        time: f32,
        cp: &CathodeInterferenceShaderParams,
        target_fbo: Option<glow::Framebuffer>,
        scissor_enabled: bool,
    ) {
        gl.bind_framebuffer(glow::FRAMEBUFFER, target_fbo);
        gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
        gl.bind_vertex_array(Some(self.vertex_array));
        gl.use_program(Some(self.cathode_prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));

        gl.uniform_2_f32(
            Some(&self.cathode_output_res_loc),
            output_size.0,
            output_size.1,
        );
        gl.uniform_2_f32(
            Some(&self.cathode_source_size_loc),
            resolution.0 as f32,
            resolution.1 as f32,
        );
        gl.uniform_1_f32(
            Some(&self.cathode_horizontal_stretch_loc),
            horizontal_stretch,
        );
        gl.uniform_1_f32(Some(&self.cathode_time_loc), time);
        gl.uniform_1_f32(Some(&self.cathode_intensity_loc), cp.intensity);
        gl.uniform_1_f32(Some(&self.cathode_frequency_loc), cp.frequency);
        gl.uniform_1_f32(Some(&self.cathode_randomization_loc), cp.randomization);
        gl.uniform_1_f32(
            Some(&self.cathode_electricity_glow_loc),
            cp.electricity_glow,
        );
        gl.uniform_1_f32(Some(&self.cathode_flicker_depth_loc), cp.flicker_depth);
        gl.uniform_1_f32(Some(&self.cathode_interference_loc), cp.interference);
        gl.uniform_1_f32(
            Some(&self.cathode_lightbulb_effect_loc),
            cp.lightbulb_effect,
        );
        gl.uniform_4_f32(
            Some(&self.cathode_border_crop_loc),
            cp.border_crop[0],
            cp.border_crop[1],
            cp.border_crop[2],
            cp.border_crop[3],
        );

        if scissor_enabled {
            gl.enable(glow::SCISSOR_TEST);
        }
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.use_program(None);
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_audio_reactive_pass(
        &self,
        gl: &glow::Context,
        input_texture: glow::Texture,
        output_size: (f32, f32),
        resolution: (u32, u32),
        arp: &AudioReactiveShaderParams,
        time: f32,
        target_fbo: Option<glow::Framebuffer>,
        scissor_enabled: bool,
    ) {
        gl.bind_framebuffer(glow::FRAMEBUFFER, target_fbo);
        gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
        gl.bind_vertex_array(Some(self.vertex_array));
        gl.use_program(Some(self.audio_reactive_prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));

        gl.uniform_2_f32(
            self.ar_output_res_loc.as_ref(),
            output_size.0,
            output_size.1,
        );
        gl.uniform_2_f32(
            self.ar_source_size_loc.as_ref(),
            resolution.0 as f32,
            resolution.1 as f32,
        );
        gl.uniform_1_f32(
            self.ar_horizontal_stretch_loc.as_ref(),
            arp.horizontal_stretch,
        );
        gl.uniform_4_f32(
            self.ar_border_crop_loc.as_ref(),
            arp.border_crop[0],
            arp.border_crop[1],
            arp.border_crop[2],
            arp.border_crop[3],
        );
        gl.uniform_2_f32(self.ar_warp_loc.as_ref(), arp.warp[0], arp.warp[1]);
        gl.uniform_1_f32(self.ar_corner_size_loc.as_ref(), arp.corner_size);
        gl.uniform_1_i32(self.ar_filter_type_loc.as_ref(), arp.filter_type);
        gl.uniform_1_f32(self.ar_time_loc.as_ref(), time);

        gl.uniform_1_f32(self.ar_master_intensity_loc.as_ref(), arp.master_intensity);
        gl.uniform_1_f32(self.ar_low_env_loc.as_ref(), arp.low_env);
        gl.uniform_1_f32(self.ar_mid_env_loc.as_ref(), arp.mid_env);
        gl.uniform_1_f32(self.ar_high_env_loc.as_ref(), arp.high_env);
        gl.uniform_1_f32(self.ar_transient_env_loc.as_ref(), arp.transient_env);
        gl.uniform_1_f32(self.ar_stereo_balance_loc.as_ref(), arp.stereo_balance);
        gl.uniform_2_f32(
            self.ar_spring_pos_loc.as_ref(),
            arp.spring_pos[0],
            arp.spring_pos[1],
        );
        gl.uniform_1_f32(self.ar_phase_accum_loc.as_ref(), arp.phase_accum);

        gl.uniform_1_f32(self.ar_deflection_shake_loc.as_ref(), arp.deflection_shake);
        gl.uniform_1_f32(self.ar_stereo_tug_loc.as_ref(), arp.stereo_tug);
        gl.uniform_1_f32(self.ar_flyback_pump_loc.as_ref(), arp.flyback_pump);
        gl.uniform_1_f32(
            self.ar_rgb_deconvergence_loc.as_ref(),
            arp.rgb_deconvergence,
        );
        gl.uniform_1_f32(self.ar_scanline_wave_loc.as_ref(), arp.scanline_wave);
        gl.uniform_1_f32(self.ar_phosphor_surge_loc.as_ref(), arp.phosphor_surge);
        gl.uniform_1_f32(self.ar_sync_tear_loc.as_ref(), arp.sync_tear);

        if scissor_enabled {
            gl.enable(glow::SCISSOR_TEST);
        }
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.use_program(None);
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_glass_pass(
        &self,
        gl: &glow::Context,
        input_texture: glow::Texture,
        output_size: (f32, f32),
        resolution: (u32, u32),
        glass_params: &GlassShaderParams,
        time: f32,
        target_fbo: Option<glow::Framebuffer>,
        scissor_enabled: bool,
    ) {
        gl.bind_framebuffer(glow::FRAMEBUFFER, target_fbo);
        gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
        gl.bind_vertex_array(Some(self.vertex_array));
        gl.use_program(Some(self.crt_glass_prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));

        gl.uniform_2_f32(
            self.glass_output_res_loc.as_ref(),
            output_size.0,
            output_size.1,
        );
        gl.uniform_2_f32(
            self.glass_source_size_loc.as_ref(),
            resolution.0 as f32,
            resolution.1 as f32,
        );
        gl.uniform_1_f32(
            self.glass_horizontal_stretch_loc.as_ref(),
            glass_params.horizontal_stretch,
        );
        gl.uniform_4_f32(
            self.glass_border_crop_loc.as_ref(),
            glass_params.border_crop[0],
            glass_params.border_crop[1],
            glass_params.border_crop[2],
            glass_params.border_crop[3],
        );
        gl.uniform_2_f32(
            self.glass_warp_loc.as_ref(),
            glass_params.warp[0],
            glass_params.warp[1],
        );
        gl.uniform_1_f32(
            self.glass_corner_size_loc.as_ref(),
            glass_params.corner_size,
        );
        gl.uniform_1_i32(
            self.glass_filter_type_loc.as_ref(),
            glass_params.filter_type,
        );
        gl.uniform_1_f32(self.glass_intensity_loc.as_ref(), glass_params.intensity);
        gl.uniform_1_f32(self.glass_glossiness_loc.as_ref(), glass_params.glossiness);
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.silhouette_texture));

        gl.uniform_1_f32(self.glass_time_loc.as_ref(), time);
        gl.uniform_1_i32(
            self.glass_ceiling_light_enabled_loc.as_ref(),
            if glass_params.ceiling_light_enabled {
                1
            } else {
                0
            },
        );
        gl.uniform_1_i32(
            self.glass_photographer_enabled_loc.as_ref(),
            if glass_params.photographer_enabled {
                1
            } else {
                0
            },
        );
        gl.uniform_1_f32(
            self.glass_photographer_intensity_loc.as_ref(),
            glass_params.photographer_intensity,
        );
        gl.uniform_1_i32(
            self.glass_flash_enabled_loc.as_ref(),
            if glass_params.flash_enabled { 1 } else { 0 },
        );
        gl.uniform_1_f32(
            self.glass_flash_intensity_loc.as_ref(),
            glass_params.flash_intensity,
        );

        if scissor_enabled {
            gl.enable(glow::SCISSOR_TEST);
        }
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.use_program(None);
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn flush_overlay_batch_gl(
        &self,
        gl: &glow::Context,
        painter: &egui_glow::Painter,
        output_size: (f32, f32),
        widget_rect: egui::Rect,
        shadow_mask: f32,
        scanline_strength: f32,
        brightboost: f32,
    ) {
        if self.overlay_draws_scratch.is_empty() {
            return;
        }

        gl.enable(glow::BLEND);
        gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        gl.enable(glow::SCISSOR_TEST);

        gl.use_program(Some(self.popup_prog));

        gl.uniform_2_f32(
            self.popup_screen_size_loc.as_ref(),
            widget_rect.width(),
            widget_rect.height(),
        );
        gl.uniform_2_f32(
            self.popup_offset_loc.as_ref(),
            widget_rect.min.x,
            widget_rect.min.y,
        );
        gl.uniform_1_i32(self.popup_sampler_loc.as_ref(), 0);

        gl.uniform_1_f32(self.popup_shadow_mask_loc.as_ref(), shadow_mask);
        gl.uniform_1_f32(self.popup_scanline_strength_loc.as_ref(), scanline_strength);
        let scanline_freq = (output_size.1 / 240.0).max(2.0);
        gl.uniform_1_f32(self.popup_scanline_freq_loc.as_ref(), scanline_freq);
        gl.uniform_1_f32(self.popup_brightboost_loc.as_ref(), brightboost);

        gl.bind_vertex_array(Some(self.popup_vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.popup_vbo));
        gl.buffer_data_u8_slice(
            glow::ARRAY_BUFFER,
            bytemuck::cast_slice(&self.overlay_vertices_scratch),
            glow::STREAM_DRAW,
        );
        gl.buffer_data_u8_slice(
            glow::ELEMENT_ARRAY_BUFFER,
            bytemuck::cast_slice(&self.overlay_indices_scratch),
            glow::STREAM_DRAW,
        );

        gl.active_texture(glow::TEXTURE0);
        let mut last_tex_id: Option<egui::TextureId> = None;
        let mut last_scissor: Option<[i32; 4]> = None;

        for cmd in &self.overlay_draws_scratch {
            let Some(texture) = painter.texture(cmd.texture_id) else {
                continue;
            };
            if last_scissor != Some(cmd.scissor) {
                gl.scissor(
                    cmd.scissor[0],
                    cmd.scissor[1],
                    cmd.scissor[2],
                    cmd.scissor[3],
                );
                last_scissor = Some(cmd.scissor);
            }
            if last_tex_id != Some(cmd.texture_id) {
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                last_tex_id = Some(cmd.texture_id);
            }
            gl.draw_elements(
                glow::TRIANGLES,
                cmd.index_count,
                glow::UNSIGNED_INT,
                (cmd.first_index * std::mem::size_of::<u32>()) as i32,
            );
        }

        // Unbind VAO BEFORE unbinding buffers so `popup_vao` retains its `ELEMENT_ARRAY_BUFFER`.
        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.use_program(None);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::BLEND);
    }

    #[allow(dead_code)]
    pub fn draw_popup_primitives(
        &mut self,
        gl: &glow::Context,
        painter: &egui_glow::Painter,
        output_size: (f32, f32),
        widget_rect: egui::Rect,
        shadow_mask: f32,
        scanline_strength: f32,
        brightboost: f32,
    ) {
        if self.popup_primitives.is_empty()
            || widget_rect.width() <= 0.0
            || widget_rect.height() <= 0.0
        {
            return;
        }
        unsafe {
            let scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);
            let mut caller_scissor = [0i32; 4];
            if scissor_enabled {
                gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut caller_scissor);
            }
            let blend_enabled = gl.is_enabled(glow::BLEND);
            assemble_overlay_batch(
                &[&self.popup_primitives],
                widget_rect,
                output_size,
                if scissor_enabled {
                    Some(caller_scissor)
                } else {
                    None
                },
                &mut self.overlay_vertices_scratch,
                &mut self.overlay_indices_scratch,
                &mut self.overlay_draws_scratch,
            );
            self.flush_overlay_batch_gl(
                gl,
                painter,
                output_size,
                widget_rect,
                shadow_mask,
                scanline_strength,
                brightboost,
            );
            if scissor_enabled {
                gl.scissor(
                    caller_scissor[0],
                    caller_scissor[1],
                    caller_scissor[2],
                    caller_scissor[3],
                );
                gl.enable(glow::SCISSOR_TEST);
            }
            if blend_enabled {
                gl.blend_func_separate(
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                    glow::ONE_MINUS_DST_ALPHA,
                    glow::ONE,
                );
                gl.enable(glow::BLEND);
            }
        }
    }

    /// Draw tessellated egui primitives into the current framebuffer with the
    /// simulated shadow mask / scanlines, so they look part of the CRT image.
    #[allow(dead_code, clippy::too_many_arguments)]
    pub fn draw_overlay_primitives(
        &mut self,
        gl: &glow::Context,
        painter: &egui_glow::Painter,
        primitives: &[egui::ClippedPrimitive],
        output_size: (f32, f32),
        widget_rect: egui::Rect,
        shadow_mask: f32,
        scanline_strength: f32,
        brightboost: f32,
    ) {
        if primitives.is_empty() || widget_rect.width() <= 0.0 || widget_rect.height() <= 0.0 {
            return;
        }
        unsafe {
            let scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);
            let mut caller_scissor = [0i32; 4];
            if scissor_enabled {
                gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut caller_scissor);
            }
            let blend_enabled = gl.is_enabled(glow::BLEND);
            assemble_overlay_batch(
                &[primitives],
                widget_rect,
                output_size,
                if scissor_enabled {
                    Some(caller_scissor)
                } else {
                    None
                },
                &mut self.overlay_vertices_scratch,
                &mut self.overlay_indices_scratch,
                &mut self.overlay_draws_scratch,
            );
            self.flush_overlay_batch_gl(
                gl,
                painter,
                output_size,
                widget_rect,
                shadow_mask,
                scanline_strength,
                brightboost,
            );
            if scissor_enabled {
                gl.scissor(
                    caller_scissor[0],
                    caller_scissor[1],
                    caller_scissor[2],
                    caller_scissor[3],
                );
                gl.enable(glow::SCISSOR_TEST);
            }
            if blend_enabled {
                gl.blend_func_separate(
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                    glow::ONE_MINUS_DST_ALPHA,
                    glow::ONE,
                );
                gl.enable(glow::BLEND);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_software_mouse_internal(
        &self,
        gl: &glow::Context,
        cursor_pos: (f32, f32),
        output_size: (f32, f32),
        shadow_mask: f32,
        scanline_strength: f32,
        brightboost: f32,
        ppp: f32,
        clip_rect: Option<[i32; 4]>,
    ) {
        if let Some([cx, cy, cw, ch]) = clip_rect {
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(cx, cy, cw, ch);
        } else {
            gl.disable(glow::SCISSOR_TEST);
        }

        gl.enable(glow::BLEND);
        gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

        gl.use_program(Some(self.retro_mouse_prog));

        let scale = (1.5 * ppp).round().max(1.0);
        let cursor_w = RETRO_CURSOR_WIDTH as f32 * scale;
        let cursor_h = RETRO_CURSOR_HEIGHT as f32 * scale;

        gl.uniform_2_f32(
            self.retro_mouse_screen_size_loc.as_ref(),
            output_size.0,
            output_size.1,
        );
        gl.uniform_2_f32(
            self.retro_mouse_cursor_pos_loc.as_ref(),
            cursor_pos.0,
            cursor_pos.1,
        );
        gl.uniform_2_f32(
            self.retro_mouse_cursor_size_loc.as_ref(),
            cursor_w,
            cursor_h,
        );
        gl.uniform_1_i32(self.retro_mouse_sampler_loc.as_ref(), 0);

        gl.uniform_1_f32(self.retro_mouse_shadow_mask_loc.as_ref(), shadow_mask);
        gl.uniform_1_f32(
            self.retro_mouse_scanline_strength_loc.as_ref(),
            scanline_strength,
        );
        let scanline_freq = (output_size.1 / 240.0).max(2.0);
        gl.uniform_1_f32(self.retro_mouse_scanline_freq_loc.as_ref(), scanline_freq);
        gl.uniform_1_f32(self.retro_mouse_brightboost_loc.as_ref(), brightboost);

        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.retro_mouse_texture));

        gl.bind_vertex_array(Some(self.vertex_array));
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

        gl.bind_vertex_array(None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.use_program(None);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::BLEND);
    }

    #[allow(dead_code, clippy::too_many_arguments)]
    pub fn draw_software_mouse(
        &self,
        gl: &glow::Context,
        cursor_pos: (f32, f32),
        output_size: (f32, f32),
        shadow_mask: f32,
        scanline_strength: f32,
        brightboost: f32,
        ppp: f32,
        clip_rect: Option<[i32; 4]>,
    ) {
        unsafe {
            let scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);
            let mut caller_scissor = [0i32; 4];
            if scissor_enabled {
                gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut caller_scissor);
            }
            let blend_enabled = gl.is_enabled(glow::BLEND);
            self.draw_software_mouse_internal(
                gl,
                cursor_pos,
                output_size,
                shadow_mask,
                scanline_strength,
                brightboost,
                ppp,
                clip_rect,
            );
            if scissor_enabled {
                if clip_rect.is_some() {
                    gl.scissor(
                        caller_scissor[0],
                        caller_scissor[1],
                        caller_scissor[2],
                        caller_scissor[3],
                    );
                }
                gl.enable(glow::SCISSOR_TEST);
            }
            if blend_enabled {
                gl.blend_func_separate(
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                    glow::ONE_MINUS_DST_ALPHA,
                    glow::ONE,
                );
                gl.enable(glow::BLEND);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_retro_frame(
        &self,
        gl: &glow::Context,
        resolution: (u32, u32),
        output_size: (f32, f32),
        horizontal_stretch: f32,
        border_crop: [f32; 4],
        warp: [f32; 2],
        corner_size: f32,
        filter_type: i32,
        time: f32,
        ambient_glow: f32,
        dark_mode: bool,
    ) {
        unsafe {
            let blend_enabled = gl.is_enabled(glow::BLEND);
            gl.enable(glow::BLEND);
            gl.blend_func_separate(
                glow::SRC_ALPHA,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ZERO,
                glow::ONE,
            );

            gl.bind_vertex_array(Some(self.vertex_array));
            gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);

            gl.use_program(Some(self.retro_frame_prog));

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.bezel_texture));

            gl.active_texture(glow::TEXTURE1);
            let video_tex = self
                .last_video_texture
                .unwrap_or(self.pass_textures[PASS_INPUT]);
            let video_res = if self.last_video_texture_res.0 > 0 {
                self.last_video_texture_res
            } else {
                resolution
            };
            gl.bind_texture(glow::TEXTURE_2D, Some(video_tex));
            let mut last_mipmapped = self.last_mipmapped_frame.get();
            let mut generated_this_paint = self.mipmap_generated_this_paint.get();
            if check_and_record_mipmap(
                &mut last_mipmapped,
                &mut generated_this_paint,
                video_tex,
                self.current_source_frame_id,
                video_res,
            ) {
                gl.generate_mipmap(glow::TEXTURE_2D);
            }
            self.last_mipmapped_frame.set(last_mipmapped);
            self.mipmap_generated_this_paint.set(generated_this_paint);
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR_MIPMAP_LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );

            gl.uniform_2_f32(
                self.retro_output_res_loc.as_ref(),
                output_size.0,
                output_size.1,
            );
            gl.uniform_2_f32(
                self.retro_source_size_loc.as_ref(),
                resolution.0 as f32,
                resolution.1 as f32,
            );
            gl.uniform_1_f32(
                self.retro_horizontal_stretch_loc.as_ref(),
                horizontal_stretch,
            );
            gl.uniform_4_f32(
                self.retro_border_crop_loc.as_ref(),
                border_crop[0],
                border_crop[1],
                border_crop[2],
                border_crop[3],
            );
            gl.uniform_2_f32(self.retro_warp_loc.as_ref(), warp[0], warp[1]);
            gl.uniform_1_f32(self.retro_corner_size_loc.as_ref(), corner_size);
            gl.uniform_1_i32(self.retro_filter_type_loc.as_ref(), filter_type);
            gl.uniform_1_f32(self.retro_time_loc.as_ref(), time);
            gl.uniform_1_f32(self.retro_ambient_glow_loc.as_ref(), ambient_glow);
            gl.uniform_1_i32(
                self.retro_dark_mode_loc.as_ref(),
                if dark_mode { 1 } else { 0 },
            );

            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                self.last_video_tex_base_filter,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                self.last_video_tex_base_filter,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);

            if blend_enabled {
                gl.blend_func_separate(
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                    glow::ONE_MINUS_DST_ALPHA,
                    glow::ONE,
                );
            } else {
                gl.disable(glow::BLEND);
            }

            gl.bind_vertex_array(None);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_night_mode_glow(
        &self,
        gl: &glow::Context,
        resolution: (u32, u32),
        output_size: (f32, f32),
        horizontal_stretch: f32,
        border_crop: [f32; 4],
        warp: [f32; 2],
        corner_size: f32,
        filter_type: i32,
        glow_intensity: f32,
    ) {
        if glow_intensity <= 0.001 {
            return;
        }
        unsafe {
            let blend_enabled = gl.is_enabled(glow::BLEND);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE);

            gl.bind_vertex_array(Some(self.vertex_array));
            gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);

            gl.use_program(Some(self.night_glow_prog));

            gl.active_texture(glow::TEXTURE0);
            let video_tex = self
                .last_video_texture
                .unwrap_or(self.pass_textures[PASS_INPUT]);
            let video_res = if self.last_video_texture_res.0 > 0 {
                self.last_video_texture_res
            } else {
                resolution
            };
            gl.bind_texture(glow::TEXTURE_2D, Some(video_tex));
            let mut last_mipmapped = self.last_mipmapped_frame.get();
            let mut generated_this_paint = self.mipmap_generated_this_paint.get();
            if check_and_record_mipmap(
                &mut last_mipmapped,
                &mut generated_this_paint,
                video_tex,
                self.current_source_frame_id,
                video_res,
            ) {
                gl.generate_mipmap(glow::TEXTURE_2D);
            }
            self.last_mipmapped_frame.set(last_mipmapped);
            self.mipmap_generated_this_paint.set(generated_this_paint);
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR_MIPMAP_LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );

            gl.uniform_2_f32(
                self.night_glow_output_res_loc.as_ref(),
                output_size.0,
                output_size.1,
            );
            gl.uniform_2_f32(
                self.night_glow_source_size_loc.as_ref(),
                resolution.0 as f32,
                resolution.1 as f32,
            );
            gl.uniform_1_f32(
                self.night_glow_horizontal_stretch_loc.as_ref(),
                horizontal_stretch,
            );
            gl.uniform_4_f32(
                self.night_glow_border_crop_loc.as_ref(),
                border_crop[0],
                border_crop[1],
                border_crop[2],
                border_crop[3],
            );
            gl.uniform_2_f32(self.night_glow_warp_loc.as_ref(), warp[0], warp[1]);
            gl.uniform_1_f32(self.night_glow_corner_size_loc.as_ref(), corner_size);
            gl.uniform_1_i32(self.night_glow_filter_type_loc.as_ref(), filter_type);
            gl.uniform_1_f32(self.night_glow_intensity_loc.as_ref(), glow_intensity);

            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                self.last_video_tex_base_filter,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                self.last_video_tex_base_filter,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);

            if blend_enabled {
                gl.blend_func_separate(
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                    glow::ONE_MINUS_DST_ALPHA,
                    glow::ONE,
                );
            } else {
                gl.disable(glow::BLEND);
            }

            gl.bind_vertex_array(None);
        }
    }

    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.passthrough_prog);
            gl.delete_program(self.retro_frame_prog);
            gl.delete_texture(self.bezel_texture);
            gl.delete_texture(self.silhouette_texture);
            gl.delete_program(self.pixelate_prog);
            gl.delete_program(self.median_prog);
            gl.delete_program(self.deinterlace_prog);
            gl.delete_program(self.final_prog);
            gl.delete_program(self.yuv_planar_prog);
            gl.delete_program(self.yuyv_packed_prog);
            gl.delete_program(self.cathode_prog);
            gl.delete_program(self.audio_reactive_prog);
            gl.delete_program(self.crt_glass_prog);
            gl.delete_program(self.night_glow_prog);
            gl.delete_program(self.popup_prog);
            gl.delete_program(self.blit_prog);
            gl.delete_program(self.retro_mouse_prog);
            gl.delete_program(self.linearize_prog);
            gl.delete_texture(self.retro_mouse_texture);
            gl.delete_vertex_array(self.popup_vao);
            gl.delete_buffer(self.popup_vbo);
            gl.delete_buffer(self.popup_ebo);
            for fbo in self.post_fbos {
                gl.delete_framebuffer(fbo);
            }
            for tex in self.post_textures {
                gl.delete_texture(tex);
            }
            self.anime4k_small.destroy(gl);
            self.anime4k_medium.destroy(gl);
            self.anime4k_large.destroy(gl);
            self.fsr1.destroy(gl);
            self.framegen.destroy(gl);
            self.halo.destroy(gl);
            self.beam.destroy(gl);
            gl.delete_vertex_array(self.vertex_array);
            gl.delete_buffer(self.vbo);
            for fbo in self.fbos {
                gl.delete_framebuffer(fbo);
            }
            for texture in self.pass_textures {
                gl.delete_texture(texture);
            }
            for texture in self.yuv_planes {
                gl.delete_texture(texture);
            }
            for pbo in self.pbos {
                gl.delete_buffer(pbo);
            }
            if let Some(pending) = &self.ocr_pending_readback {
                gl.delete_sync(pending.fence);
            }
            gl.delete_framebuffer(self.ocr_fbo);
            gl.delete_texture(self.ocr_texture);
            gl.delete_buffer(self.ocr_pbo);
        }
    }

    fn setup_framebuffers(
        &mut self,
        gl: &glow::Context,
        width: u32,
        height: u32,
        target_width: u32,
        target_height: u32,
        scaler_filter: u8,
    ) {
        let scaler = ScalerFilter::from_u8(scaler_filter);
        let is_anime4k = matches!(
            scaler,
            ScalerFilter::Anime4kSmall | ScalerFilter::Anime4kMedium | ScalerFilter::Anime4kLarge
        );

        let (effective_width, effective_height) = if is_anime4k {
            match scaler {
                ScalerFilter::Anime4kSmall => {
                    self.anime4k_small
                        .get_upscaled_size(width, height, target_width, target_height)
                }
                ScalerFilter::Anime4kMedium => self.anime4k_medium.get_upscaled_size(
                    width,
                    height,
                    target_width,
                    target_height,
                ),
                ScalerFilter::Anime4kLarge => {
                    self.anime4k_large
                        .get_upscaled_size(width, height, target_width, target_height)
                }
                _ => unreachable!(),
            }
        } else if scaler == ScalerFilter::Fsr1 {
            self.fsr1
                .get_upscaled_size(width, height, target_width, target_height)
        } else {
            (width, height)
        };

        let filter_mode = if scaler_filter == crate::video::types::ScalerFilter::Point as u8 {
            glow::NEAREST as i32
        } else {
            glow::LINEAR as i32
        };

        unsafe {
            for i in 0..self.pass_textures.len() {
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[i]));
                let (w, h) = pass_texture_dimensions(
                    i,
                    (width, height),
                    (effective_width, effective_height),
                );
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    intermediate_texture_internal_format(i) as i32,
                    w as i32,
                    h as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    None,
                );
                let pass_filter_mode = if i == PASS_PIXELATE {
                    glow::NEAREST as i32
                } else {
                    filter_mode
                };
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, pass_filter_mode);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, pass_filter_mode);
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_S,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_T,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[i]));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(self.pass_textures[i]),
                    0,
                );
            }
            for texture in self.yuv_planes {
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter_mode);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter_mode);
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_S,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_T,
                    glow::CLAMP_TO_EDGE as i32,
                );
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.history_ring.reset();
            self.last_frame_captured_at = -1;
            self.last_input_tex = None;
            self.last_input_slot = None;
            self.last_source_kind = None;
            self.last_fallback_tex = None;
            self.last_mipmapped_frame.set(None);
            self.last_pixelate_key = None;
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/support/software_gl.rs"]
mod software_gl;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "opt-in Mesa software EGL readback; set LIBGL_ALWAYS_SOFTWARE=1 EGL_PLATFORM=surfaceless"]
    fn clean_ocr_capture_matches_crt_display_geometry() {
        let egl = software_gl::SoftwareGl::new(1280, 801);
        // egui_glow's painter API takes Arc even for a single-threaded context.
        #[allow(clippy::arc_with_non_send_sync)]
        let gl = Arc::new(egl.context());
        println!("GL renderer: {}", unsafe {
            gl.get_parameter_string(glow::RENDERER)
        });
        let mut painter = egui_glow::Painter::new(gl.clone(), "", None).unwrap();
        let mut renderer = CrtFilterRenderer::new(&gl);
        for (source_width, source_height) in
            [(1280u32, 800u32), (1280, 720), (1920, 1080), (301, 241)]
        {
            let marker_x = source_width * 9 / 10;
            let marker_y = source_height * 4 / 5;
            let mut pixels = vec![0u8; (source_width * source_height * 4) as usize];
            for pixel in pixels.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
            // A colored marker near the edge exposes the old 5-6px curvature error.
            for y in marker_y - 8..marker_y + 8 {
                for x in marker_x - 8..marker_x + 8 {
                    let i = ((y * source_width + x) * 4) as usize;
                    pixels[i] = 255;
                }
            }
            let mut frame = RawFrame {
                captured_at: 1,
                rate: crate::replay::config::Rate { num: 60, den: 1 },
                width: source_width,
                height: source_height,
                data: pixels,
                format: Pixel::RGBA,
                color_range: crate::video::types::ColorRange::Full,
            };
            let params = ShaderParams {
                shadow_mask: 0.0,
                bloom_amount: 0.0,
                ..Default::default()
            };
            let cathode = CathodeInterferenceShaderParams::default();
            fn marker(pixels: &[u8], width: u32, height: u32) -> (f64, f64) {
                let mut sum = (0.0, 0.0, 0.0);
                for (i, p) in pixels.chunks_exact(4).enumerate() {
                    if p[0] as i32 > p[1] as i32 * 2 + 16 && p[0] as i32 > p[2] as i32 * 2 + 16 {
                        let weight = p[0] as f64;
                        sum.0 += ((i as u32 % width) as f64 + 0.5) * weight;
                        sum.1 += (height as f64 - (i as u32 / width) as f64 - 0.5) * weight;
                        sum.2 += weight;
                    }
                }
                assert!(sum.2 > 100.0, "Missing marker in readback");
                (sum.0 / sum.2, sum.1 / sum.2)
            }
            for (name, lottes, halo, beam, curvature, zoom, pixelate) in [
                ("Lottes", true, false, false, true, 100.0, false),
                ("Halo", false, true, false, true, 100.0, false),
                ("Halo zoom", false, true, false, true, 95.0, false),
                ("Halo zoom flat", false, true, false, false, 95.0, false),
                ("Beam", false, false, true, false, 100.0, false),
                ("Beam curved zoom", false, false, true, true, 95.0, false),
                (
                    "Off after capture",
                    false,
                    false,
                    false,
                    false,
                    100.0,
                    false,
                ),
                (
                    "Pixelate after capture",
                    false,
                    false,
                    false,
                    false,
                    100.0,
                    true,
                ),
            ] {
                frame.captured_at += 1;
                let halo_params = HaloShaderParams {
                    curvature,
                    halo_zoom: zoom,
                    shadow_mask: 0.0,
                    bloom: 0.0,
                    glow: 0.0,
                    halation: 0.0,
                    halo_intensity: 0.0,
                    ..Default::default()
                };
                let output = (1280.0, 801.0);
                let beam_params = BeamShaderParams {
                    frames_per_hz: 1.0,
                    scan_direction: 0,
                    lcd_anti_retention: false,
                    curvature,
                    screen_scale: zoom,
                    shadow_mask: 0.0,
                    scanline_strength: 0.0,
                    halation: 0.0,
                    ..Default::default()
                };
                renderer.capture_ocr_next_paint = true;
                let area = renderer.paint(
                    &gl,
                    &painter,
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(output.0, output.1)),
                    Some(&frame),
                    None,
                    (source_width, source_height),
                    output,
                    &params,
                    &halo_params,
                    &beam_params,
                    &cathode,
                    None,
                    None,
                    0.0,
                    pixelate,
                    lottes,
                    halo,
                    beam,
                    None,
                    0.0,
                    0.0,
                    None,
                    None,
                    false,
                );
                let (clean, w, h) = renderer.last_ocr_capture.take().expect("OCR capture");
                assert_eq!(
                    (w, h),
                    (area.width, area.height),
                    "Capture/display fit differs for {name}"
                );
                let (display, dw, dh) = read_framebuffer_pixels_raw(&gl, area).unwrap();
                let a = marker(&clean, w, h);
                let b = marker(&display, dw, dh);
                println!("{source_width}x{source_height} {name}: clean={a:?} display={b:?}");
                assert!(
                    (a.0 - b.0).abs() < 1.0 && (a.1 - b.1).abs() < 1.0,
                    "{name}: misaligned OCR"
                );
                if !lottes && !halo && !(beam && (curvature || zoom != 100.0)) {
                    let fitted = if pixelate {
                        super::super::geometry::pixelate_size((source_width, source_height))
                    } else {
                        (source_width, source_height)
                    };
                    let aspect = fitted.0 as f64 / fitted.1 as f64;
                    let image_width = (output.0 as f64).min(output.1 as f64 * aspect);
                    let image_height = (output.1 as f64).min(output.0 as f64 / aspect);
                    let expected_x = (output.0 as f64 - image_width) * 0.5
                        + marker_x as f64 / source_width as f64 * image_width
                        - area.x as f64;
                    let expected_y = (output.1 as f64 - image_height) * 0.5
                        + marker_y as f64 / source_height as f64 * image_height
                        - area.top(output.1 as u32) as f64;
                    assert!(
                        (a.0 - expected_x).abs() < 1.0 && (a.1 - expected_y).abs() < 1.0,
                        "Leaked OCR warp/zoom"
                    );
                }
                assert_eq!(unsafe { gl.get_error() }, glow::NO_ERROR, "{name}");
            }
        }
        renderer.destroy(&gl);
        painter.destroy();
    }

    #[test]
    #[ignore = "opt-in Mesa software EGL readback; set LIBGL_ALWAYS_SOFTWARE=1 EGL_PLATFORM=surfaceless"]
    fn beam_crt_steady_state_capture_matches_subframe_average_without_disturbing_ring_buffer() {
        let egl = software_gl::SoftwareGl::new(640, 480);
        #[allow(clippy::arc_with_non_send_sync)]
        let gl = Arc::new(egl.context());
        let mut painter = egui_glow::Painter::new(gl.clone(), "", None).unwrap();
        let mut renderer = CrtFilterRenderer::new(&gl);

        let mut pixels = vec![0u8; 640 * 480 * 4];
        for p in pixels.chunks_exact_mut(4) {
            p[0] = 200;
            p[1] = 200;
            p[2] = 200;
            p[3] = 255;
        }
        let frame = RawFrame {
            captured_at: 1,
            rate: crate::replay::config::Rate { num: 60, den: 1 },
            width: 640,
            height: 480,
            data: pixels,
            format: Pixel::RGBA,
            color_range: crate::video::types::ColorRange::Full,
        };
        let params = ShaderParams::default();
        let halo_params = HaloShaderParams::default();
        let cathode = CathodeInterferenceShaderParams::default();
        let output = (640.0, 480.0);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(output.0, output.1));
        let top_row_idx = (420 * 640 + 320) * 4;
        let bot_row_idx = (60 * 640 + 320) * 4;

        for gain_vs_blur in [0.75f32, 0.45f32] {
            let gamma = 2.4f32;
            let live_beam = BeamShaderParams {
                frames_per_hz: 2.0,
                gamma,
                gain_vs_blur,
                scan_direction: 1,
                lcd_anti_retention: false,
                curvature: false,
                corner_size: 0.0,
                ..Default::default()
            };
            let capture_beam = live_beam.as_steady_state_capture();

            // Subframe 0 (top half illuminated)
            let mut sub0_beam = live_beam.clone();
            sub0_beam.i_frame_override = Some(0);
            let area0 = renderer.paint(
                &gl,
                &painter,
                rect,
                Some(&frame),
                None,
                (640, 480),
                output,
                &params,
                &halo_params,
                &sub0_beam,
                &cathode,
                None,
                None,
                0.0,
                false,
                false,
                false,
                true,
                None,
                0.0,
                0.0,
                None,
                None,
                false,
            );
            let (sub0, _, _) = read_framebuffer_pixels_raw(&gl, area0).unwrap();

            // Mid-sweep one-shot steady-state screenshot capture (`as_steady_state_capture`)
            let area_cap = renderer.paint(
                &gl,
                &painter,
                rect,
                Some(&frame),
                None,
                (640, 480),
                output,
                &params,
                &halo_params,
                &capture_beam,
                &cathode,
                None,
                None,
                0.0,
                false,
                false,
                false,
                true,
                None,
                0.0,
                0.0,
                None,
                None,
                false,
            );
            let (cap_px, _, _) = read_framebuffer_pixels_raw(&gl, area_cap).unwrap();

            // Subframe 1 immediately after screenshot capture (bottom half must still illuminate cleanly)
            let mut sub1_beam = live_beam.clone();
            sub1_beam.i_frame_override = Some(1);
            let area1 = renderer.paint(
                &gl,
                &painter,
                rect,
                Some(&frame),
                None,
                (640, 480),
                output,
                &params,
                &halo_params,
                &sub1_beam,
                &cathode,
                None,
                None,
                0.0,
                false,
                false,
                false,
                true,
                None,
                0.0,
                0.0,
                None,
                None,
                false,
            );
            let (sub1, _, _) = read_framebuffer_pixels_raw(&gl, area1).unwrap();
            assert!(
                sub1[bot_row_idx] > sub1[top_row_idx],
                "Live subframe 1 after one-shot capture must still illuminate bottom half (bot={}, top={})",
                sub1[bot_row_idx],
                sub1[top_row_idx]
            );

            let srgb_to_linear = |c: f32| -> f32 {
                if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(gamma)
                }
            };
            let linear_to_srgb = |c: f32| -> f32 {
                if c <= 0.0031308 {
                    c * 12.92
                } else {
                    1.055 * c.powf(1.0 / gamma) - 0.055
                }
            };

            // Average over a 16x16 patch in top & bottom halves to average out fine scanline/mask phase
            let avg_patch_linear = |buf: &[u8], center_y: usize| -> f32 {
                let mut sum = 0.0f32;
                let mut count = 0.0f32;
                for dy in 0..16 {
                    for dx in 0..16 {
                        let idx = (((center_y + dy) * 640 + (312 + dx)) * 4) as usize;
                        let r = srgb_to_linear(buf[idx] as f32 / 255.0);
                        let g = srgb_to_linear(buf[idx + 1] as f32 / 255.0);
                        let b = srgb_to_linear(buf[idx + 2] as f32 / 255.0);
                        sum += (r + g + b) / 3.0;
                        count += 1.0;
                    }
                }
                sum / count
            };

            let cap_top_lin = avg_patch_linear(&cap_px, 412);
            let cap_bot_lin = avg_patch_linear(&cap_px, 52);
            assert!(
                (cap_top_lin - cap_bot_lin).abs() < 0.01,
                "Steady-state capture must have uniform top/bottom brightness: top={cap_top_lin:.4}, bot={cap_bot_lin:.4}"
            );

            let live_talbot_top_lin =
                0.5 * (avg_patch_linear(&sub0, 412) + avg_patch_linear(&sub1, 412));
            let live_talbot_srgb = linear_to_srgb(live_talbot_top_lin) * 255.0;
            let cap_srgb = linear_to_srgb(cap_top_lin) * 255.0;
            assert!(
                (cap_srgb - live_talbot_srgb).abs() <= 1.5,
                "Steady-state Beam capture ({cap_srgb:.1}) must match Talbot-Plateau perceived brightness ({live_talbot_srgb:.1}) for gain_vs_blur={gain_vs_blur}"
            );
        }

        renderer.destroy(&gl);
        painter.destroy();
    }

    #[test]
    #[ignore = "opt-in Mesa software EGL readback; set LIBGL_ALWAYS_SOFTWARE=1 EGL_PLATFORM=surfaceless"]
    fn beam_crt_sweeps_top_then_bottom_across_subframes() {
        let egl = software_gl::SoftwareGl::new(640, 480);
        #[allow(clippy::arc_with_non_send_sync)]
        let gl = Arc::new(egl.context());
        let mut painter = egui_glow::Painter::new(gl.clone(), "", None).unwrap();
        let mut renderer = CrtFilterRenderer::new(&gl);

        let mut pixels = vec![0u8; 640 * 480 * 4];
        for p in pixels.chunks_exact_mut(4) {
            p[0] = 200;
            p[1] = 200;
            p[2] = 200;
            p[3] = 255;
        }
        let frame = RawFrame {
            captured_at: 1,
            rate: crate::replay::config::Rate { num: 60, den: 1 },
            width: 640,
            height: 480,
            data: pixels,
            format: Pixel::RGBA,
            color_range: crate::video::types::ColorRange::Full,
        };
        let params = ShaderParams::default();
        let halo_params = HaloShaderParams::default();
        let beam_params = BeamShaderParams {
            frames_per_hz: 2.0,
            scan_direction: 1, // Top to Bottom
            lcd_anti_retention: false,
            ..Default::default()
        };
        let cathode = CathodeInterferenceShaderParams::default();
        let output = (640.0, 480.0);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(output.0, output.1));

        // Subframe 0: crtRasterPos = 0.0 -> top half illuminated
        let area0 = renderer.paint(
            &gl,
            &painter,
            rect,
            Some(&frame),
            None,
            (640, 480),
            output,
            &params,
            &halo_params,
            &beam_params,
            &cathode,
            None,
            None,
            0.0,
            false,
            false,
            false,
            true,
            None,
            0.0,
            0.0,
            None,
            None,
            false,
        );
        let (sub0, _, _) = read_framebuffer_pixels_raw(&gl, area0).unwrap();
        // OpenGL read_pixels y=0 is bottom, y=479 is top
        let top_row_idx = (420 * 640 + 320) * 4;
        let bot_row_idx = (60 * 640 + 320) * 4;
        assert!(
            sub0[top_row_idx] > 150,
            "Subframe 0 top half should be bright, got {}",
            sub0[top_row_idx]
        );
        assert!(
            sub0[bot_row_idx] < sub0[top_row_idx],
            "Subframe 0 bottom half ({}) should be dimmer than top half ({})",
            sub0[bot_row_idx],
            sub0[top_row_idx]
        );

        // Subframe 1: crtRasterPos = 0.5 -> bottom half illuminated
        let area1 = renderer.paint(
            &gl,
            &painter,
            rect,
            Some(&frame),
            None,
            (640, 480),
            output,
            &params,
            &halo_params,
            &beam_params,
            &cathode,
            None,
            None,
            0.0,
            false,
            false,
            false,
            true,
            None,
            0.0,
            0.0,
            None,
            None,
            false,
        );
        let (sub1, _, _) = read_framebuffer_pixels_raw(&gl, area1).unwrap();
        assert!(
            sub1[bot_row_idx] > 120,
            "Subframe 1 bottom half should be bright, got {}",
            sub1[bot_row_idx]
        );
        assert!(
            sub1[top_row_idx] < sub1[bot_row_idx],
            "Subframe 1 top half ({}) should be dimmer than bottom half ({})",
            sub1[top_row_idx],
            sub1[bot_row_idx]
        );

        // Subframe 2 (Cycle 1, crtRasterPos = 0.0): supply a brand new GREEN source frame
        // and verify zero-latency ring-buffer latching on integer frames_per_hz = 2.0!
        let mut green_pixels = vec![0u8; 640 * 480 * 4];
        for p in green_pixels.chunks_exact_mut(4) {
            p[0] = 0;
            p[1] = 220;
            p[2] = 0;
            p[3] = 255;
        }
        let green_frame = RawFrame {
            captured_at: 2,
            rate: crate::replay::config::Rate { num: 60, den: 1 },
            width: 640,
            height: 480,
            data: green_pixels,
            format: Pixel::RGBA,
            color_range: crate::video::types::ColorRange::Full,
        };
        let area2 = renderer.paint(
            &gl,
            &painter,
            rect,
            Some(&green_frame),
            None,
            (640, 480),
            output,
            &params,
            &halo_params,
            &beam_params,
            &cathode,
            None,
            None,
            0.0,
            false,
            false,
            false,
            true,
            None,
            0.0,
            0.0,
            None,
            None,
            false,
        );
        let (sub2, _, _) = read_framebuffer_pixels_raw(&gl, area2).unwrap();
        assert!(
            sub2[top_row_idx + 1] > 160 && sub2[top_row_idx] < 20,
            "Subframe 2 top half must immediately display the new green frame without 1-cycle lag: R={}, G={}",
            sub2[top_row_idx],
            sub2[top_row_idx + 1]
        );

        // Now turn the CRT shader completely OFF (`draw_passthrough`) and render the same frame
        // across repeated UI repaints after unbinding VAO and clearing the default FBO to black.
        for repaint_idx in 0..2 {
            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.clear_color(0.0, 0.0, 0.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                gl.bind_vertex_array(None);
            }
            let pass_area = renderer.draw_passthrough(
                &gl,
                &painter,
                rect,
                Some(&green_frame),
                None,
                (640, 480),
                output,
                [0.0, 0.0, 0.0],
                1.0,
                false,
                0.0,
                false,
                0,
                0.0,
                0.0,
                1.0,
                0.0,
                0.0,
                1.0,
                1.0,
                0,
                crate::video::gpu::DEFAULT_FSR1_SHARPNESS,
                0.0,
                0.0,
                0.0,
                0.0,
                [0.0, 0.0, 0.0, 0.0],
                None,
                0.0,
                0.0,
                None,
                None,
                None,
                0.0,
                None,
                None,
                false,
            );
            let (pass_pixels, _, _) = read_framebuffer_pixels_raw(&gl, pass_area).unwrap();
            assert!(
                pass_pixels[top_row_idx + 1] > 160 && pass_pixels[bot_row_idx + 1] > 160,
                "Passthrough repaint {} must not go black when CRT shader is off! top G={}, bot G={}",
                repaint_idx,
                pass_pixels[top_row_idx + 1],
                pass_pixels[bot_row_idx + 1]
            );
        }

        renderer.destroy(&gl);
        painter.destroy();
    }

    #[test]
    fn linear_prepasses_use_high_precision_targets() {
        assert_eq!(NUM_INTERMEDIATE_PASSES, 6);
        assert_eq!(
            intermediate_texture_internal_format(PASS_HIST_1),
            glow::RGBA16F
        );
        assert_eq!(
            intermediate_texture_internal_format(PASS_DEINTERLACE),
            glow::RGBA16F
        );
        assert_eq!(
            intermediate_texture_internal_format(PASS_HIST_2),
            glow::RGBA16F
        );
        assert_eq!(
            intermediate_texture_internal_format(PASS_PIXELATE),
            glow::RGBA16F
        );
        assert_eq!(
            intermediate_texture_internal_format(PASS_MEDIAN),
            glow::RGBA16F
        );
        assert_eq!(
            intermediate_texture_internal_format(PASS_INPUT),
            glow::RGBA16F
        );
    }

    #[test]
    fn pixelate_pass_uses_low_scale_surface() {
        assert_eq!(pixelate_subrender_size(1920, 1080), (854, 480));
        assert_eq!(pixelate_subrender_size(1280, 720), (854, 480));
        assert_eq!(pixelate_subrender_size(640, 480), (640, 480));
        assert_eq!(pixelate_subrender_size(320, 240), (320, 240));
    }

    #[test]
    fn pass_dimensions_preserve_pipeline_surfaces() {
        let source_size = (720, 480);
        let effective_size = (1440, 960);

        assert_eq!(
            pass_texture_dimensions(PASS_HIST_1, source_size, effective_size),
            source_size
        );
        assert_eq!(
            pass_texture_dimensions(PASS_DEINTERLACE, source_size, effective_size),
            source_size
        );
        assert_eq!(
            pass_texture_dimensions(PASS_HIST_2, source_size, effective_size),
            source_size
        );
        assert_eq!(
            pass_texture_dimensions(PASS_PIXELATE, source_size, effective_size),
            (720, 480)
        );
        assert_eq!(
            pass_texture_dimensions(PASS_MEDIAN, source_size, effective_size),
            source_size
        );
        assert_eq!(
            pass_texture_dimensions(PASS_INPUT, source_size, effective_size),
            source_size
        );
    }

    #[test]
    fn other_intermediate_passes_keep_existing_format() {
        // Dead full-resolution RGBA8 slot was removed (`NUM_INTERMEDIATE_PASSES == 6`);
        // verify out-of-range fallback remains `RGBA8`.
        assert_eq!(NUM_INTERMEDIATE_PASSES, 6);
        assert_eq!(
            intermediate_texture_internal_format(NUM_INTERMEDIATE_PASSES),
            glow::RGBA8
        );
    }

    #[test]
    fn deinterlace_shader_contains_three_frame_uniforms() {
        use crate::video::gpu::programs::FS_DEINTERLACE;
        assert!(FS_DEINTERLACE.contains("uniform sampler2D current_frame;"));
        assert!(FS_DEINTERLACE.contains("uniform sampler2D prev_frame;"));
        assert!(FS_DEINTERLACE.contains("uniform sampler2D prev_frame_2;"));
        assert!(FS_DEINTERLACE.contains("uniform int has_prev2;"));
        assert!(FS_DEINTERLACE.contains("uniform int mode;"));
        assert!(FS_DEINTERLACE.contains("uniform float blend_amount;"));
        assert!(FS_DEINTERLACE.contains("uniform float motion_threshold;"));
        assert!(FS_DEINTERLACE.contains("uniform float line_spacing;"));
        assert!(FS_DEINTERLACE.contains("uniform float spatial_mix;"));
    }

    #[test]
    fn deinterlace_bob_dejitter_math() {
        // Simulate a high-contrast horizontal line (e.g. text/HUD edge) bobbing between Field 0 and Field 1:
        // On Frame N (Even field t): scanline y has intensity 1.0
        // On Frame N+1 (Odd field t+1): scanline y has intensity 0.0 (bobbed by 1 field line)
        // On Frame N+2 (Even field t+2): scanline y has intensity 1.0 (same parity as Frame N)
        let frame_t = 1.0f32;
        let frame_t1 = 0.0f32;
        let frame_t2 = 1.0f32;

        // Same-parity difference (t vs t-2):
        let parity_diff = (frame_t - frame_t2).abs();
        assert_eq!(
            parity_diff, 0.0,
            "Same-parity difference must be zero on static bobbing edges"
        );

        // 50% temporal weave on Frame N (curr = t, prev = t-1):
        let weave_n = frame_t * 0.5 + frame_t1 * 0.5;
        // 50% temporal weave on Frame N+1 (curr = t+1, prev = t):
        let weave_n1 = frame_t1 * 0.5 + frame_t * 0.5;

        assert_eq!(weave_n, 0.5);
        assert_eq!(weave_n1, 0.5);
        assert_eq!(
            weave_n, weave_n1,
            "50% weave must produce identical values across consecutive frames, eliminating bob flicker"
        );
    }

    #[test]
    fn history_shift_preserves_distinct_frames_across_repeat_paints() {
        // Exercises the zero-blit 3-slot `DeinterlaceHistoryRing` across new frames,
        // repeat paints (Beam sub-frames / UI repaints), and `!deinterlace_enabled` resets.
        let mut ring = DeinterlaceHistoryRing::new(HISTORY_SLOTS);
        let mut slot_contents = [0u32; NUM_INTERMEDIATE_PASSES];
        let mut last_captured_at = 0u64;

        let arrivals = [
            (100u64, 1u32), // Frame 1 arrival
            (100u64, 1u32), // Frame 1 repeat paint 1
            (100u64, 1u32), // Frame 1 repeat paint 2
            (200u64, 2u32), // Frame 2 arrival
            (200u64, 2u32), // Frame 2 repeat paint 1
            (200u64, 2u32), // Frame 2 repeat paint 2
            (300u64, 3u32), // Frame 3 arrival
            (300u64, 3u32), // Frame 3 repeat paint 1
            (300u64, 3u32), // Frame 3 repeat paint 2
            (400u64, 4u32), // Frame 4 arrival (ring wraps around cleanly)
            (400u64, 4u32), // Frame 4 repeat paint
        ];

        for (step, (pts, frame_id)) in arrivals.iter().enumerate() {
            let is_new_frame = *pts != last_captured_at;
            let write_slot = ring.select_write_slot(true, is_new_frame);
            if is_new_frame {
                slot_contents[write_slot] = *frame_id;
                last_captured_at = *pts;
            }
            let (curr_slot, prev1_slot, prev2_slot, has_prev2) = ring.active_slots();
            assert_eq!(curr_slot, write_slot);

            match step {
                0..=2 => {
                    assert_eq!(slot_contents[curr_slot], 1);
                    assert_eq!(slot_contents[prev1_slot], 1);
                    assert!(!has_prev2);
                    assert_eq!(ring.count, 1);
                }
                3..=5 => {
                    assert_eq!(slot_contents[curr_slot], 2);
                    assert_eq!(slot_contents[prev1_slot], 1);
                    assert!(!has_prev2);
                    assert_eq!(ring.count, 2);
                }
                6..=8 => {
                    assert_eq!(slot_contents[curr_slot], 3, "Current frame must remain 3");
                    assert_eq!(
                        slot_contents[prev1_slot], 2,
                        "prev1 must remain Frame 2 across repeat paints"
                    );
                    assert_eq!(
                        slot_contents[prev2_slot], 1,
                        "prev2 must remain Frame 1 across repeat paints"
                    );
                    assert!(has_prev2);
                    assert_eq!(ring.count, 3);
                }
                9..=10 => {
                    assert_eq!(slot_contents[curr_slot], 4);
                    assert_eq!(slot_contents[prev1_slot], 3);
                    assert_eq!(slot_contents[prev2_slot], 2);
                    assert!(has_prev2);
                    assert_eq!(ring.count, 4);
                }
                _ => unreachable!(),
            }
        }

        // Disabling deinterlace resets the ring and writes straight to `PASS_INPUT` with zero blits.
        let non_deint_slot = ring.select_write_slot(false, true);
        assert_eq!(non_deint_slot, PASS_INPUT);
        assert_eq!(ring.count, 0);
    }

    #[test]
    fn preprocess_cache_sig_fallback_hits_and_invalidates() {
        let tex1 = glow::NativeTexture(NonZero::new(1).unwrap());
        let tex2 = glow::NativeTexture(NonZero::new(2).unwrap());
        let scan_and_color = [
            0.0f32.to_bits(),
            0.0f32.to_bits(),
            0.0f32.to_bits(),
            0.0f32.to_bits(),
            1.0f32.to_bits(),
            1.0f32.to_bits(),
        ];
        let deint_params = [
            0.5f32.to_bits(),
            0.08f32.to_bits(),
            1.0f32.to_bits(),
            0.5f32.to_bits(),
        ];
        let fft_thresh = [0.0f32.to_bits(), 0.0f32.to_bits()];

        let make_fallback_sig = |tex: Option<glow::Texture>,
                                 ts: Option<i64>,
                                 fastpath: bool|
         -> Option<PreprocessCacheSig> {
            build_preprocess_cache_sig(
                None,
                tex,
                ts,
                (1280, 800),
                (1280, 800),
                0,
                0,
                fastpath,
                scan_and_color,
                false,
                0,
                deint_params,
                false,
                false,
                [0; 3],
                false,
                0.0f32.to_bits(),
                false,
                fft_thresh,
                0,
            )
        };

        // Unversioned fallback (`None` or `<= 0`) must not falsely hit cache
        assert_eq!(make_fallback_sig(Some(tex1), None, false), None);
        assert_eq!(make_fallback_sig(Some(tex1), Some(0), false), None);

        // Repeat paint of identical `(tex1, 100, fastpath=false)` hits cache
        let sig_a1 = make_fallback_sig(Some(tex1), Some(100), false).expect("valid fallback sig");
        let sig_a2 = make_fallback_sig(Some(tex1), Some(100), false).expect("valid fallback sig");
        assert_eq!(sig_a1, sig_a2);

        // Changing `captured_at`, `fallback_texture`, or `allow_srgb_passthrough_fastpath` invalidates
        let sig_new_ts = make_fallback_sig(Some(tex1), Some(101), false).unwrap();
        let sig_new_tex = make_fallback_sig(Some(tex2), Some(100), false).unwrap();
        let sig_fastpath = make_fallback_sig(Some(tex1), Some(100), true).unwrap();
        assert_ne!(sig_a1, sig_new_ts);
        assert_ne!(sig_a1, sig_new_tex);
        assert_ne!(sig_a1, sig_fastpath);

        // Switching from `Fallback` to `Raw` with the same timestamp also invalidates
        let raw_frame = RawFrame {
            captured_at: 100,
            rate: crate::replay::config::Rate { num: 60, den: 1 },
            width: 1280,
            height: 800,
            data: Vec::new(),
            format: Pixel::RGBA,
            color_range: crate::video::types::ColorRange::Full,
        };
        let sig_raw = build_preprocess_cache_sig(
            Some(&raw_frame),
            Some(tex1),
            Some(100),
            (1280, 800),
            (1280, 800),
            0,
            0,
            false,
            scan_and_color,
            false,
            0,
            deint_params,
            false,
            false,
            [0; 3],
            false,
            0.0f32.to_bits(),
            false,
            fft_thresh,
            0,
        )
        .unwrap();
        assert_ne!(sig_a1, sig_raw);

        // Switching framegen between interpolated (t=0.5) and real (t=1.0) invalidates cache
        let sig_fg_interp = build_preprocess_cache_sig(
            Some(&raw_frame),
            None,
            None,
            (1280, 800),
            (1280, 800),
            0,
            0,
            false,
            scan_and_color,
            false,
            0,
            deint_params,
            true,
            true,
            [0; 3],
            false,
            0.0f32.to_bits(),
            false,
            fft_thresh,
            0,
        )
        .unwrap();
        let sig_fg_real = build_preprocess_cache_sig(
            Some(&raw_frame),
            None,
            None,
            (1280, 800),
            (1280, 800),
            0,
            0,
            false,
            scan_and_color,
            false,
            0,
            deint_params,
            true,
            false,
            [0; 3],
            false,
            0.0f32.to_bits(),
            false,
            fft_thresh,
            0,
        )
        .unwrap();
        assert_ne!(sig_fg_interp, sig_fg_real);

        // Changing FSR 1.0 sharpness invalidates the preprocess cache signature
        let sig_fsr_lo = build_preprocess_cache_sig(
            Some(&raw_frame),
            None,
            None,
            (1280, 800),
            (2560, 1600),
            ScalerFilter::Fsr1 as u8,
            0.25f32.to_bits(),
            false,
            scan_and_color,
            false,
            0,
            deint_params,
            false,
            false,
            [0; 3],
            false,
            0.0f32.to_bits(),
            false,
            fft_thresh,
            0,
        )
        .unwrap();
        let sig_fsr_hi = build_preprocess_cache_sig(
            Some(&raw_frame),
            None,
            None,
            (1280, 800),
            (2560, 1600),
            ScalerFilter::Fsr1 as u8,
            0.85f32.to_bits(),
            false,
            scan_and_color,
            false,
            0,
            deint_params,
            false,
            false,
            [0; 3],
            false,
            0.0f32.to_bits(),
            false,
            fft_thresh,
            0,
        )
        .unwrap();
        assert_ne!(sig_fsr_lo, sig_fsr_hi);
    }

    #[test]
    fn mipmap_generation_tracks_source_frame_and_invalidates() {
        let tex1 = glow::NativeTexture(NonZero::new(10).unwrap());
        let tex2 = glow::NativeTexture(NonZero::new(11).unwrap());
        let mut last_mipmapped = None;
        let mut generated_this_paint = false;

        // First consumer (`draw_retro_frame`) on frame 50 generates mipmaps
        assert!(check_and_record_mipmap(
            &mut last_mipmapped,
            &mut generated_this_paint,
            tex1,
            50,
            (640, 480),
        ));
        // Second consumer (`draw_night_mode_glow`) in the same paint skips regeneration
        assert!(!check_and_record_mipmap(
            &mut last_mipmapped,
            &mut generated_this_paint,
            tex1,
            50,
            (640, 480),
        ));

        // Next Beam sub-frame / idle UI repaint (`generated_this_paint` reset to `false`, same frame 50)
        generated_this_paint = false;
        assert!(!check_and_record_mipmap(
            &mut last_mipmapped,
            &mut generated_this_paint,
            tex1,
            50,
            (640, 480),
        ));

        // New video frame 51 regenerates mipmaps
        generated_this_paint = false;
        assert!(check_and_record_mipmap(
            &mut last_mipmapped,
            &mut generated_this_paint,
            tex1,
            51,
            (640, 480),
        ));

        // Changing texture (`run_pixelate` toggled) or resolution on frame 51 regenerates
        generated_this_paint = false;
        assert!(check_and_record_mipmap(
            &mut last_mipmapped,
            &mut generated_this_paint,
            tex2,
            51,
            (640, 480),
        ));
        assert!(check_and_record_mipmap(
            &mut last_mipmapped,
            &mut generated_this_paint,
            tex2,
            51,
            (320, 240),
        ));

        // Explicit invalidation (`preprocess_video_chain` re-rendered `video_tex` on slider change)
        last_mipmapped = None;
        generated_this_paint = false;
        assert!(check_and_record_mipmap(
            &mut last_mipmapped,
            &mut generated_this_paint,
            tex2,
            51,
            (320, 240),
        ));

        // Unversioned source (`frame_id <= 0`) falls back to once-per-paint
        generated_this_paint = false;
        assert!(check_and_record_mipmap(
            &mut last_mipmapped,
            &mut generated_this_paint,
            tex1,
            0,
            (640, 480),
        ));
        assert!(!check_and_record_mipmap(
            &mut last_mipmapped,
            &mut generated_this_paint,
            tex1,
            0,
            (640, 480),
        ));
        generated_this_paint = false;
        assert!(check_and_record_mipmap(
            &mut last_mipmapped,
            &mut generated_this_paint,
            tex1,
            0,
            (640, 480),
        ));
    }

    #[test]
    fn overlay_batch_concatenates_meshes_and_offsets_indices() {
        let mk_mesh =
            |tex_id: egui::TextureId, clip: egui::Rect, v_count: usize, tri_indices: &[u32]| {
                let mut mesh = egui::Mesh::with_texture(tex_id);
                for i in 0..v_count {
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: egui::pos2(i as f32 * 10.0, 10.0),
                        uv: egui::pos2(0.0, 0.0),
                        color: egui::Color32::WHITE,
                    });
                }
                mesh.indices.extend_from_slice(tri_indices);
                egui::ClippedPrimitive {
                    clip_rect: clip,
                    primitive: egui::epaint::Primitive::Mesh(mesh),
                }
            };

        let widget_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0));
        let full_clip = widget_rect;
        let popup_clip =
            egui::Rect::from_min_max(egui::pos2(100.0, 80.0), egui::pos2(300.0, 280.0));

        let comments = vec![
            mk_mesh(
                egui::TextureId::Managed(0),
                full_clip,
                4,
                &[0, 1, 2, 0, 2, 3],
            ),
            mk_mesh(egui::TextureId::Managed(0), full_clip, 3, &[0, 1, 2]),
        ];
        let popup = vec![mk_mesh(
            egui::TextureId::User(7),
            popup_clip,
            4,
            &[0, 1, 2, 2, 3, 0],
        )];

        let mut verts = Vec::new();
        let mut indices = Vec::new();
        let mut draws = Vec::new();

        assemble_overlay_batch(
            &[&comments, &popup],
            widget_rect,
            (1280.0, 960.0),
            Some([0, 0, 1000, 960]),
            &mut verts,
            &mut indices,
            &mut draws,
        );

        assert_eq!(verts.len(), 11);
        assert_eq!(indices, vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 7, 8, 9, 9, 10, 7]);
        // First two meshes share `(Managed(0), scissor)` and coalesce into a single draw command!
        assert_eq!(draws.len(), 2);
        assert_eq!(
            draws[0],
            OverlayDrawCmd {
                texture_id: egui::TextureId::Managed(0),
                scissor: [0, 0, 1000, 960],
                first_index: 0,
                index_count: 9,
            }
        );
        assert_eq!(
            draws[1],
            OverlayDrawCmd {
                texture_id: egui::TextureId::User(7),
                scissor: [200, 400, 400, 400],
                first_index: 9,
                index_count: 6,
            }
        );
    }

    #[test]
    fn test_srgb_linearization_roundtrip() {
        fn to_linear(c: f32) -> f32 {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }

        fn to_srgb(c: f32) -> f32 {
            if c < 0.0031308 {
                c * 12.92
            } else {
                1.055 * c.powf(0.41666) - 0.055
            }
        }

        // Test near-black values (e.g. 0.05, which was washed out to ~0.24 without linearization)
        for i in 0..=255 {
            let srgb = i as f32 / 255.0;
            let linear = to_linear(srgb);
            let roundtrip = to_srgb(linear);
            assert!(
                (srgb - roundtrip).abs() < 0.005,
                "Linearization roundtrip failed for {srgb}: got {roundtrip}"
            );
        }

        // Verify that without linearization (double gamma), near-black is severely elevated
        let near_black = 0.05;
        let double_gamma = to_srgb(near_black);
        assert!(
            double_gamma > 0.20,
            "Double gamma must elevate near-black: got {double_gamma}"
        );
    }

    #[test]
    fn audio_reactive_shader_contains_all_renderer_uniforms() {
        let required_uniforms = [
            "input_texture",
            "outputResolution",
            "source_size",
            "horizontal_stretch",
            "border_crop",
            "warp",
            "corner_size",
            "filter_type",
            "time",
            "master_intensity",
            "low_env",
            "mid_env",
            "high_env",
            "transient_env",
            "stereo_balance",
            "spring_pos",
            "phase_accum",
            "deflection_shake",
            "stereo_tug",
            "flyback_pump",
            "rgb_deconvergence",
            "scanline_wave",
            "phosphor_surge",
            "sync_tear",
        ];
        for name in required_uniforms {
            assert!(
                FS_AUDIO_REACTIVE.contains(name),
                "FS_AUDIO_REACTIVE is missing uniform declaration or usage for `{name}`"
            );
        }
    }

    #[test]
    fn input_shaders_contain_brightness_and_gamma_uniforms() {
        for (shader_name, src) in [
            ("FS_YUV_PLANAR", FS_YUV_PLANAR),
            ("FS_YUYV_PACKED", FS_YUYV_PACKED),
            ("FS_LINEARIZE", FS_LINEARIZE),
        ] {
            assert!(
                src.contains("uniform float brightness;"),
                "{shader_name} is missing `uniform float brightness;`"
            );
            assert!(
                src.contains("uniform float gamma;"),
                "{shader_name} is missing `uniform float gamma;`"
            );
            assert!(
                src.contains("ApplyBrightnessGamma"),
                "{shader_name} is missing `ApplyBrightnessGamma`"
            );
        }
    }

    #[test]
    fn test_brightness_and_gamma_adjustment_math() {
        fn apply_brightness_gamma(mut c: f32, brightness: f32, gamma: f32) -> f32 {
            if (gamma - 1.0).abs() > 0.0001 {
                c = c.max(0.0).powf(1.0 / gamma.max(0.01));
            }
            c *= brightness.max(0.0);
            c.clamp(0.0, 1.0)
        }

        // Neutral settings preserve signal identically
        assert_eq!(apply_brightness_gamma(0.5, 1.0, 1.0), 0.5);

        // Higher gamma (> 1.0) lifts midtones while keeping 0 and 1 anchored
        let lifted = apply_brightness_gamma(0.5, 1.0, 2.0);
        assert!(lifted > 0.70 && lifted < 0.71, "got {lifted}");
        assert_eq!(apply_brightness_gamma(0.0, 1.0, 2.0), 0.0);
        assert_eq!(apply_brightness_gamma(1.0, 1.0, 2.0), 1.0);

        // Lower gamma (< 1.0) darkens midtones
        let darkened = apply_brightness_gamma(0.5, 1.0, 0.5);
        assert!((darkened - 0.25).abs() < 1e-5, "got {darkened}");

        // Brightness scales signal and clamps to [0, 1]
        assert!((apply_brightness_gamma(0.4, 1.5, 1.0) - 0.6).abs() < 1e-5);
        assert_eq!(apply_brightness_gamma(0.8, 2.0, 1.0), 1.0);
    }

    #[test]
    #[ignore = "opt-in Mesa software EGL readback; set LIBGL_ALWAYS_SOFTWARE=1 EGL_PLATFORM=surfaceless"]
    fn fsr1_upscales_and_records_telemetry_and_sharpness() {
        let egl = software_gl::SoftwareGl::new(320, 240);
        #[allow(clippy::arc_with_non_send_sync)]
        let gl = Arc::new(egl.context());
        let mut painter = egui_glow::Painter::new(gl.clone(), "", None).unwrap();
        let mut renderer = CrtFilterRenderer::new(&gl);

        // 160x120 source with a blurred S-curve vertical step edge at x=79..80 (40 -> 65 -> 175 -> 200)
        let mut small_pixels = vec![0u8; 160 * 120 * 4];
        for y in 0..120 {
            for x in 0..160 {
                let v = if x < 79 {
                    40u8
                } else if x == 79 {
                    65u8
                } else if x == 80 {
                    175u8
                } else {
                    200u8
                };
                let idx = (y * 160 + x) * 4;
                small_pixels[idx] = v;
                small_pixels[idx + 1] = v;
                small_pixels[idx + 2] = v;
                small_pixels[idx + 3] = 255;
            }
        }
        let small_frame = RawFrame {
            captured_at: 1,
            rate: crate::replay::config::Rate { num: 60, den: 1 },
            width: 160,
            height: 120,
            data: small_pixels,
            format: Pixel::RGBA,
            color_range: crate::video::types::ColorRange::Full,
        };

        let output = (320.0, 240.0);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(output.0, output.1));

        let render = |renderer: &mut CrtFilterRenderer,
                      frame: &RawFrame,
                      scaler: ScalerFilter,
                      sharpness: f32| {
            let area = renderer.draw_passthrough(
                &gl,
                &painter,
                rect,
                Some(frame),
                None,
                (frame.width, frame.height),
                output,
                [0.0, 0.0, 0.0],
                1.0,
                false,
                0.0,
                false,
                0,
                0.0,
                0.0,
                1.0,
                0.0,
                1.0,
                1.0,
                1.0,
                scaler as u8,
                sharpness,
                0.0,
                0.0,
                0.0,
                0.0,
                [0.0, 0.0, 0.0, 0.0],
                None,
                0.0,
                0.0,
                None,
                None,
                None,
                0.0,
                None,
                None,
                false,
            );
            read_framebuffer_pixels_raw(&gl, area).unwrap().0
        };

        // 1. 2x upscale with sharpness = 0.0 (EASU active, RCAS neutral)
        let px_soft = render(&mut renderer, &small_frame, ScalerFilter::Fsr1, 0.0);
        assert_eq!(
            renderer.last_fsr1_run(),
            Some(Fsr1RunInfo {
                input_size: (160, 120),
                output_size: (320, 240),
                easu_ran: true,
                rcas_sharpness: 0.0,
                run_count: 1,
            })
        );
        let mid_y = 120usize;
        let sample_r = |buf: &[u8], x: usize| buf[(mid_y * 320 + x) * 4] as i32;
        assert!(
            (sample_r(&px_soft, 40) - 40).abs() <= 2,
            "Left interior should preserve dark gray (~40), got {}",
            sample_r(&px_soft, 40)
        );
        assert!(
            (sample_r(&px_soft, 280) - 200).abs() <= 2,
            "Right interior should preserve bright gray (~200), got {}",
            sample_r(&px_soft, 280)
        );

        // 2. Same frame with sharpness = 1.0 invalidates cache and steepens edge contrast
        let px_sharp = render(&mut renderer, &small_frame, ScalerFilter::Fsr1, 1.0);
        assert_eq!(
            renderer.last_fsr1_run(),
            Some(Fsr1RunInfo {
                input_size: (160, 120),
                output_size: (320, 240),
                easu_ran: true,
                rcas_sharpness: 1.0,
                run_count: 2,
            })
        );
        let soft_edge_step = sample_r(&px_soft, 161) - sample_r(&px_soft, 158);
        let sharp_edge_step = sample_r(&px_sharp, 161) - sample_r(&px_sharp, 158);
        assert!(
            sharp_edge_step > soft_edge_step,
            "RCAS sharpness=1.0 must steepen edge contrast over sharpness=0.0 (sharp={sharp_edge_step}, soft={soft_edge_step})"
        );

        // 3. 1x native frame skips EASU and runs RCAS only
        let native_frame = RawFrame {
            captured_at: 2,
            rate: crate::replay::config::Rate { num: 60, den: 1 },
            width: 320,
            height: 240,
            data: vec![128u8; 320 * 240 * 4],
            format: Pixel::RGBA,
            color_range: crate::video::types::ColorRange::Full,
        };
        let _ = render(&mut renderer, &native_frame, ScalerFilter::Fsr1, 0.85);
        assert_eq!(
            renderer.last_fsr1_run(),
            Some(Fsr1RunInfo {
                input_size: (320, 240),
                output_size: (320, 240),
                easu_ran: false,
                rcas_sharpness: 0.85,
                run_count: 3,
            })
        );

        // 4. Switching away from FSR 1.0 clears active telemetry
        let _ = render(&mut renderer, &native_frame, ScalerFilter::Bilinear, 0.85);
        assert_eq!(renderer.last_fsr1_run(), None);

        renderer.destroy(&gl);
        painter.destroy();
    }

    #[test]
    #[ignore = "opt-in Mesa software EGL readback; set LIBGL_ALWAYS_SOFTWARE=1 EGL_PLATFORM=surfaceless"]
    fn ffx_framegen_interpolates_motion_and_handles_scene_cuts() {
        let width = 160u32;
        let height = 128u32;
        let egl = software_gl::SoftwareGl::new(width as i32, height as i32);
        #[allow(clippy::arc_with_non_send_sync)]
        let gl = Arc::new(egl.context());
        let mut painter = egui_glow::Painter::new(gl.clone(), "", None).unwrap();
        let mut renderer = CrtFilterRenderer::new(&gl);

        let make_box_frame =
            |captured_at: i64, box_x: u32, box_y: u32, box_size: u32| -> RawFrame {
                let mut data = vec![0u8; (width * height * 4) as usize];
                for y in 0..height {
                    for x in 0..width {
                        let in_box = x >= box_x
                            && x < box_x + box_size
                            && y >= box_y
                            && y < box_y + box_size;
                        let v = if in_box { 235u8 } else { 20u8 };
                        let idx = ((y * width + x) * 4) as usize;
                        data[idx] = v;
                        data[idx + 1] = v;
                        data[idx + 2] = v;
                        data[idx + 3] = 255;
                    }
                }
                RawFrame {
                    captured_at,
                    rate: crate::replay::config::Rate { num: 30, den: 1 },
                    width,
                    height,
                    data,
                    format: Pixel::RGBA,
                    color_range: crate::video::types::ColorRange::Full,
                }
            };

        let output = (width as f32, height as f32);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(output.0, output.1));

        let render_passthrough = |renderer: &mut CrtFilterRenderer,
                                  frame: &RawFrame,
                                  req: FrameGenPaintRequest|
         -> Vec<u8> {
            renderer.framegen_next_paint = Some(req);
            let area = renderer.draw_passthrough(
                &gl,
                &painter,
                rect,
                Some(frame),
                None,
                (frame.width, frame.height),
                output,
                [0.0, 0.0, 0.0],
                1.0,
                false,
                0.0,
                false,
                0,
                0.0,
                0.0,
                1.0,
                0.0,
                1.0,
                1.0,
                1.0,
                ScalerFilter::Bilinear as u8,
                super::super::DEFAULT_FSR1_SHARPNESS,
                0.0,
                0.0,
                0.0,
                0.0,
                [0.0, 0.0, 0.0, 0.0],
                None,
                0.0,
                0.0,
                None,
                None,
                None,
                0.0,
                None,
                None,
                false,
            );
            read_framebuffer_pixels_raw(&gl, area).unwrap().0
        };

        // Frame 1: 24x24 bright box at x = 32..56, y = 48..72 (CRT filters OFF / passthrough).
        let f1 = make_box_frame(100_000, 32, 48, 24);
        let _ = render_passthrough(
            &mut renderer,
            &f1,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: false,
                ..Default::default()
            },
        );

        // Frame 2: Box shifted right by +16 px to x = 48..72.
        // Midpoint interpolated frame MUST reconstruct the box at x = 40..64 (center x = 52)
        // EVEN WHEN async/Constant OCR captures on the exact same Interpolated beat, AND the
        // async OCR readback MUST simultaneously receive the crisp un-interpolated Real F2
        // (box at x = 48..72) without interrupting display or Replay interpolation!
        let f2 = make_box_frame(133_333, 48, 48, 24);
        renderer.capture_ocr_async_next_paint = true;
        let px_interp = render_passthrough(
            &mut renderer,
            &f2,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: true,
                ..Default::default()
            },
        );
        unsafe {
            renderer.flush_framegen_scd_sync(&gl);
            gl.finish();
        }
        let info = renderer
            .last_framegen_run()
            .expect("framegen telemetry should be populated");
        assert!(
            info.supported,
            "Mesa OpenGL 4.5 should support GL 4.3 compute; init_error={:?}",
            renderer.framegen.init_error()
        );
        assert_eq!(info.input_size, (width, height));
        assert_eq!(info.generated_count, 1);
        assert!(info.showing_interpolated);
        assert!(
            !info.last_scene_cut,
            "Smooth +16px motion must not trip SCD"
        );

        let sample_luma = |buf: &[u8], x: u32, y: u32| -> i32 {
            // Note: GL readback is bottom-to-top; box is vertically centered at y=48..72 (symmetric around 60).
            buf[((y * width + x) * 4) as usize] as i32
        };
        let row = 60u32;
        // Center of interpolated box at x=52 must be bright (~235).
        let center_mid = sample_luma(&px_interp, 52, row);
        // Leading midpoint region x=44..47 (only bright in F1, dark in F2) & x=57..62 (only bright in F2, dark in F1)
        // must BOTH be bright in the motion-compensated midpoint frame!
        let mid_left = sample_luma(&px_interp, 44, row);
        let mid_right = sample_luma(&px_interp, 60, row);
        // Old box trail at x=34 (vacated by F2) and arriving front at x=69 (not reached yet at t=0.5) must stay dark!
        let trail_vacated = sample_luma(&px_interp, 34, row);
        let front_unreached = sample_luma(&px_interp, 69, row);
        assert!(
            center_mid > 200 && mid_left > 180 && mid_right > 180,
            "Interpolated box should be reconstructed at x=40..64 even with async OCR active (mid_left={mid_left}, center={center_mid}, mid_right={mid_right})"
        );
        assert!(
            trail_vacated < 70 && front_unreached < 70,
            "Vacated trail (x=34: {trail_vacated}) and unreached front (x=69: {front_unreached}) must not ghost"
        );

        // Presenting real F2 afterwards must show box at x=48..72 (x=69 bright, x=44 dark) without re-running OF,
        // and poll/complete the async OCR readback that was launched on the Interpolated beat above.
        let px_real = render_passthrough(
            &mut renderer,
            &f2,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: false,
                ..Default::default()
            },
        );
        let (ocr_px, ocr_w, ocr_h) = renderer
            .last_ocr_capture
            .take()
            .expect("Async OCR capture launched on Interpolated beat should complete cleanly");
        assert_eq!((ocr_w, ocr_h), (width, height));
        assert!(
            sample_luma(&ocr_px, 69, row) > 200 && sample_luma(&ocr_px, 44, row) < 50,
            "Async OCR launched during Interpolated beat must read crisp Real F2 (x=69: {}, x=44: {})",
            sample_luma(&ocr_px, 69, row),
            sample_luma(&ocr_px, 44, row)
        );
        assert!(sample_luma(&px_real, 69, row) > 200);
        assert!(sample_luma(&px_real, 44, row) < 50);
        assert_eq!(renderer.last_framegen_run().unwrap().generated_count, 1);

        // Frame 3: Hard scene cut (entirely bright frame v=220) -> SCD must trigger and output real F3 cleanly!
        let f3 = RawFrame {
            captured_at: 166_666,
            rate: crate::replay::config::Rate { num: 30, den: 1 },
            width,
            height,
            data: vec![220u8; (width * height * 4) as usize],
            format: Pixel::RGBA,
            color_range: crate::video::types::ColorRange::Full,
        };
        let px_cut = render_passthrough(
            &mut renderer,
            &f3,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: true,
                ..Default::default()
            },
        );
        unsafe {
            renderer.flush_framegen_scd_sync(&gl);
        }
        let cut_info = renderer.last_framegen_run().unwrap();
        assert!(
            cut_info.last_scene_cut,
            "Hard dark->bright cut must trigger SCD"
        );
        assert!(cut_info.scd_fallback_count >= 1);
        // SCD fallback must present clean F3 (~220), NOT a 50/50 dark-gray ghost (~120)!
        assert!(
            sample_luma(&px_cut, 10, 10) > 205,
            "SCD fallback should output real frame (~220), got {}",
            sample_luma(&px_cut, 10, 10)
        );

        // Textured smooth camera pan (+10 px) with a static HUD bar: verify coherent low-error
        // interpolation across the textured surface (no shattered-glass mosaic artifacts) AND
        // stationary HUD preservation.
        let make_textured_pan = |captured_at: i64, pan_x: f32| -> RawFrame {
            let mut data = vec![0u8; (width * height * 4) as usize];
            for y in 0..height {
                for x in 0..width {
                    let in_hud = x >= 20 && x < 70 && y >= 12 && y < 22;
                    let v = if in_hud {
                        240u8
                    } else {
                        let wx = x as f32 - pan_x;
                        let wy = y as f32;
                        let wave = 0.5
                            + 0.25 * (wx * 0.18).sin() * (wy * 0.14).cos()
                            + 0.20 * ((wx + wy) * 0.09).sin();
                        (wave.clamp(0.0, 1.0) * 190.0 + 25.0).round() as u8
                    };
                    let idx = ((y * width + x) * 4) as usize;
                    data[idx] = v;
                    data[idx + 1] = v;
                    data[idx + 2] = v;
                    data[idx + 3] = 255;
                }
            }
            RawFrame {
                captured_at,
                rate: crate::replay::config::Rate { num: 30, den: 1 },
                width,
                height,
                data,
                format: Pixel::RGBA,
                color_range: crate::video::types::ColorRange::Full,
            }
        };
        let p1 = make_textured_pan(200_000, 0.0);
        let p2 = make_textured_pan(233_333, 10.0);
        let p_gt = make_textured_pan(216_666, 5.0);
        let _ = render_passthrough(
            &mut renderer,
            &p1,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: false,
                ..Default::default()
            },
        );
        let px_pan_mid = render_passthrough(
            &mut renderer,
            &p2,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: true,
                ..Default::default()
            },
        );
        unsafe {
            renderer.flush_framegen_scd_sync(&gl);
        }
        let pan_info = renderer.last_framegen_run().unwrap();
        let mut abs_err_sum = 0u64;
        let mut err_vs_p1 = 0u64;
        let mut err_vs_p2 = 0u64;
        let mut count = 0u64;
        for y in 40..(height - 24) {
            for x in 24..(width - 24) {
                let got = sample_luma(&px_pan_mid, x, y);
                let gt_y = height - 1 - y; // GL framebuffer readback is bottom-up vs top-down RawFrame
                let gt = p_gt.data[((gt_y * width + x) * 4) as usize] as i32;
                let v1 = p1.data[((gt_y * width + x) * 4) as usize] as i32;
                let v2 = p2.data[((gt_y * width + x) * 4) as usize] as i32;
                abs_err_sum += (got - gt).unsigned_abs() as u64;
                err_vs_p1 += (got - v1).unsigned_abs() as u64;
                err_vs_p2 += (got - v2).unsigned_abs() as u64;
                count += 1;
            }
        }
        let mae = abs_err_sum as f64 / count.max(1) as f64;
        let mae1 = err_vs_p1 as f64 / count.max(1) as f64;
        let mae2 = err_vs_p2 as f64 / count.max(1) as f64;
        assert!(
            mae < 8.0,
            "Coherent textured pan interpolation should closely match t=0.5 ground truth (MAE={mae:.2}, vs_p1={mae1:.2}, vs_p2={mae2:.2}, pan_info={pan_info:?})"
        );

        let hud_gl_y = height - 1 - 17;
        assert!(
            sample_luma(&px_pan_mid, 45, hud_gl_y) > 215,
            "Static HUD bar must stay crisp during background pan, got {}",
            sample_luma(&px_pan_mid, 45, hud_gl_y)
        );

        renderer.destroy(&gl);
        painter.destroy();
    }

    #[test]
    #[ignore = "opt-in Mesa software EGL readback; set LIBGL_ALWAYS_SOFTWARE=1 EGL_PLATFORM=surfaceless"]
    fn ffx_framegen_handles_fast_camera_rotation_and_whip_fallback() {
        // 640x360 stream exercising all 6 search pyramid levels (level 5 = 20x12, +/-8px = +/-256px range).
        let width = 640u32;
        let height = 360u32;
        let egl = software_gl::SoftwareGl::new(width as i32, height as i32);
        #[allow(clippy::arc_with_non_send_sync)]
        let gl = Arc::new(egl.context());
        let mut painter = egui_glow::Painter::new(gl.clone(), "", None).unwrap();
        let mut renderer = CrtFilterRenderer::new(&gl);

        // Simulate a 3D camera rotating horizontally by ~96 px/frame (far above the old 64px limit!)
        // with perspective velocity variation across X and a stationary 3rd-person character in center.
        let make_rot_frame = |captured_at: i64, rot_angle: f32| -> RawFrame {
            let mut data = vec![0u8; (width * height * 4) as usize];
            let focal = 520.0f32;
            let cx = width as f32 * 0.5;
            let cy = height as f32 * 0.5;
            for y in 0..height {
                for x in 0..width {
                    // Stationary player character silhouette near center-bottom
                    let dx_c = x as f32 - cx;
                    let dy_c = y as f32 - (cy + 40.0);
                    let in_char = dx_c.abs() < 28.0 && dy_c > -70.0 && dy_c < 90.0;
                    let v = if in_char {
                        // Textured armor pattern so character isn't flat
                        let checker = (((x / 4) + (y / 4)) % 2) as f32;
                        (85.0 + checker * 35.0) as u8
                    } else {
                        // Perspective cylindrical world rotated by `rot_angle`
                        let yaw = ((x as f32 - cx) / focal).atan() + rot_angle;
                        let pitch = ((y as f32 - cy) / focal).atan();
                        let u = yaw * 14.0;
                        let v_coord = pitch * 14.0;
                        let world = 0.5
                            + 0.24 * (u * 1.1).sin() * (v_coord * 1.3).cos()
                            + 0.16 * (u * 2.7 + v_coord * 0.8).sin()
                            + 0.08 * (u * 5.3).cos();
                        (world.clamp(0.0, 1.0) * 185.0 + 30.0).round() as u8
                    };
                    let idx = ((y * width + x) * 4) as usize;
                    data[idx] = v;
                    data[idx + 1] = v;
                    data[idx + 2] = v;
                    data[idx + 3] = 255;
                }
            }
            RawFrame {
                captured_at,
                rate: crate::replay::config::Rate { num: 30, den: 1 },
                width,
                height,
                data,
                format: Pixel::RGBA,
                color_range: crate::video::types::ColorRange::Full,
            }
        };

        let output = (width as f32, height as f32);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(output.0, output.1));
        let render = |renderer: &mut CrtFilterRenderer,
                      frame: &RawFrame,
                      req: FrameGenPaintRequest|
         -> Vec<u8> {
            renderer.framegen_next_paint = Some(req);
            let area = renderer.draw_passthrough(
                &gl,
                &painter,
                rect,
                Some(frame),
                None,
                (frame.width, frame.height),
                output,
                [0.0, 0.0, 0.0],
                1.0,
                false,
                0.0,
                false,
                0,
                0.0,
                0.0,
                1.0,
                0.0,
                1.0,
                1.0,
                1.0,
                ScalerFilter::Bilinear as u8,
                super::super::DEFAULT_FSR1_SHARPNESS,
                0.0,
                0.0,
                0.0,
                0.0,
                [0.0, 0.0, 0.0, 0.0],
                None,
                0.0,
                0.0,
                None,
                None,
                None,
                0.0,
                None,
                None,
                false,
            );
            read_framebuffer_pixels_raw(&gl, area).unwrap().0
        };

        // 0.185 rad * 520 focal ~= 96 px/frame rotation (~106 deg/sec at 30 FPS)!
        let r1 = make_rot_frame(100_000, 0.0);
        let r2 = make_rot_frame(133_333, 0.185);
        let r_gt = make_rot_frame(116_666, 0.0925);

        let _ = render(
            &mut renderer,
            &r1,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: false,
                ..Default::default()
            },
        );
        let px_mid = render(
            &mut renderer,
            &r2,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: true,
                ..Default::default()
            },
        );
        unsafe {
            renderer.flush_framegen_scd_sync(&gl);
        }
        let rot_info = renderer.last_framegen_run().unwrap();
        assert!(
            !rot_info.last_scene_cut,
            "Smooth ~96px camera rotation should interpolate cleanly without tripping fallback ({rot_info:?})"
        );

        // Measure MAE across the rotating background (excluding character center & newly revealed edge).
        let mut err_interp = 0u64;
        let mut err_uninterp = 0u64;
        let mut count = 0u64;
        for y in 24..(height - 24) {
            for x in 120..(width - 120) {
                if (x as i32 - 320).abs() < 64 {
                    continue;
                }
                let gt_y = height - 1 - y;
                let got = px_mid[((y * width + x) * 4) as usize] as i32;
                let gt = r_gt.data[((gt_y * width + x) * 4) as usize] as i32;
                let raw = r2.data[((gt_y * width + x) * 4) as usize] as i32;
                err_interp += (got - gt).unsigned_abs() as u64;
                err_uninterp += (raw - gt).unsigned_abs() as u64;
                count += 1;
            }
        }
        let mae_interp = err_interp as f64 / count.max(1) as f64;
        let mae_raw = err_uninterp as f64 / count.max(1) as f64;
        assert!(
            mae_interp < 10.0 && mae_interp < mae_raw * 0.35,
            "96px/frame perspective rotation must reconstruct t=0.5 cleanly (mae_interp={mae_interp:.2}, mae_raw={mae_raw:.2})"
        );

        // CRITICAL 3rd-person character head check: the stationary 56px-wide character head sits
        // in the middle of a 96px/frame camera spin (background jumps completely across the head!).
        // Verify BOTH (1) the character head itself is NOT eaten/torn by the background motion,
        // AND (2) the 48px disocclusion/occlusion halo immediately left & right of the head stays clean!
        let mut head_err = 0u64;
        let mut head_count = 0u64;
        let mut halo_err = 0u64;
        let mut halo_raw_err = 0u64;
        let mut halo_count = 0u64;
        // Top-down character Y is (180+40-70)..(180+40+90) = 150..310; head region = top-down y=158..220.
        for gt_y in 158..220u32 {
            let gl_y = height - 1 - gt_y;
            // Inside character head (top-down x=296..344, 4px inset from silhouette)
            for x in 296..344u32 {
                let got = px_mid[((gl_y * width + x) * 4) as usize] as i32;
                let gt = r_gt.data[((gt_y * width + x) * 4) as usize] as i32;
                head_err += (got - gt).unsigned_abs() as u64;
                head_count += 1;
            }
            // Disocclusion & occlusion halo immediately left (240..288) and right (352..400) of the head!
            for x in (240..288u32).chain(352..400u32) {
                let got = px_mid[((gl_y * width + x) * 4) as usize] as i32;
                let gt = r_gt.data[((gt_y * width + x) * 4) as usize] as i32;
                let raw = r2.data[((gt_y * width + x) * 4) as usize] as i32;
                halo_err += (got - gt).unsigned_abs() as u64;
                halo_raw_err += (raw - gt).unsigned_abs() as u64;
                halo_count += 1;
            }
        }
        let head_mae = head_err as f64 / head_count.max(1) as f64;
        let halo_mae = halo_err as f64 / halo_count.max(1) as f64;
        let halo_raw_mae = halo_raw_err as f64 / halo_count.max(1) as f64;
        assert!(
            head_mae < 6.0,
            "Stationary character head must NOT be eaten/torn by spinning background (head_mae={head_mae:.2})"
        );
        assert!(
            halo_mae < 14.0 && halo_mae < halo_raw_mae * 0.55,
            "Disocclusion/occlusion halo around character head must stay clean (halo_mae={halo_mae:.2}, halo_raw_mae={halo_raw_mae:.2})"
        );

        // Chaotic unmatchable frame with identical global histogram (high-frequency scrambled noise):
        // global consistency quality gate MUST trigger clean real-frame fallback!
        let mut chaotic_data = vec![0u8; (width * height * 4) as usize];
        let mut seed = 0x1234_5678u32;
        for i in 0..(width * height) as usize {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let v = (((seed >> 24) as f32 / 255.0) * 185.0 + 30.0) as u8;
            chaotic_data[i * 4] = v;
            chaotic_data[i * 4 + 1] = v;
            chaotic_data[i * 4 + 2] = v;
            chaotic_data[i * 4 + 3] = 255;
        }
        let r_whip = RawFrame {
            captured_at: 166_666,
            rate: crate::replay::config::Rate { num: 30, den: 1 },
            width,
            height,
            data: chaotic_data,
            format: Pixel::RGBA,
            color_range: crate::video::types::ColorRange::Full,
        };
        let px_whip = render(
            &mut renderer,
            &r_whip,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: true,
                ..Default::default()
            },
        );
        unsafe {
            renderer.flush_framegen_scd_sync(&gl);
        }
        let whip_info = renderer.last_framegen_run().unwrap();
        assert!(
            whip_info.last_scene_cut,
            "Unmatchable chaotic whip frame must trip AFMF-style global quality gate ({whip_info:?})"
        );
        // Verify fallback returned clean un-warped `r_whip` pixel-for-pixel.
        let mut fallback_err = 0u64;
        for y in 40..80 {
            for x in 40..80 {
                let gt_y = height - 1 - y;
                let got = px_whip[((y * width + x) * 4) as usize] as i32;
                let expected = r_whip.data[((gt_y * width + x) * 4) as usize] as i32;
                fallback_err += (got - expected).unsigned_abs() as u64;
            }
        }
        assert!(
            (fallback_err as f64 / 1600.0) < 2.0,
            "Global quality gate fallback must output clean un-warped curr frame"
        );

        // Stress test 2: Realistic round smooth-shaded 3rd-person head (dark hair + smooth skin,
        // curved silhouette cutting diagonally across 8x8 blocks, +2px/+1px subtle head sway,
        // +/-2/255 HEVC-like luma noise) during a 96px/frame spin with Artifact Guard = 85%!
        let make_smooth_head_rot =
            |captured_at: i64, rot_angle: f32, head_off: (f32, f32), noise_seed: u32| -> RawFrame {
                let mut data = vec![0u8; (width * height * 4) as usize];
                let focal = 520.0f32;
                let cx = width as f32 * 0.5;
                let cy = height as f32 * 0.5;
                let head_cx = cx + head_off.0;
                let head_cy = 175.0 + head_off.1;
                let head_r = 22.0f32;
                for y in 0..height {
                    for x in 0..width {
                        let fx = x as f32;
                        let fy = y as f32;
                        let hdx = fx - head_cx;
                        let hdy = fy - head_cy;
                        let in_head = (hdx * hdx + hdy * hdy) <= head_r * head_r;
                        let in_torso = (fx - head_cx).abs() < 34.0
                            && fy > head_cy + head_r - 2.0
                            && fy < head_cy + 125.0;
                        let mut v = if in_head {
                            // Smooth dark hair on upper hemisphere, smooth lit skin on lower hemisphere.
                            let nx = hdx / head_r;
                            let ny = hdy / head_r;
                            let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
                            if ny < -0.15 {
                                (36.0 + 18.0 * nz + 4.0 * (nx * 4.0).sin()).clamp(20.0, 70.0)
                            } else {
                                (130.0 + 48.0 * nz + 10.0 * nx).clamp(90.0, 210.0)
                            }
                        } else if in_torso {
                            let tx = (fx - head_cx) / 34.0;
                            (62.0 + 20.0 * (1.0 - tx * tx).max(0.0)).clamp(40.0, 95.0)
                        } else {
                            let yaw = ((fx - cx) / focal).atan() + rot_angle;
                            let pitch = ((fy - cy) / focal).atan();
                            let u = yaw * 14.0;
                            let v_coord = pitch * 14.0;
                            let world = 0.5
                                + 0.24 * (u * 1.1).sin() * (v_coord * 1.3).cos()
                                + 0.16 * (u * 2.7 + v_coord * 0.8).sin()
                                + 0.08 * (u * 5.3).cos();
                            world.clamp(0.0, 1.0) * 185.0 + 30.0
                        };
                        if noise_seed != 0 {
                            let h = (x
                                .wrapping_mul(374761393)
                                .wrapping_add(y.wrapping_mul(668265263))
                                .wrapping_add(noise_seed.wrapping_mul(1442695041)))
                                >> 29;
                            v = (v + (h as f32 - 3.5) * 0.55).clamp(0.0, 255.0);
                        }
                        let b = v.round() as u8;
                        let idx = ((y * width + x) * 4) as usize;
                        data[idx] = b;
                        data[idx + 1] = b;
                        data[idx + 2] = b;
                        data[idx + 3] = 255;
                    }
                }
                RawFrame {
                    captured_at,
                    rate: crate::replay::config::Rate { num: 30, den: 1 },
                    width,
                    height,
                    data,
                    format: Pixel::RGBA,
                    color_range: crate::video::types::ColorRange::Full,
                }
            };

        let guard85 = crate::video::gpu::FfxFrameGenParams {
            artifact_guard: 0.85,
            ..Default::default()
        };
        let s1 = make_smooth_head_rot(300_000, 0.0, (0.0, 0.0), 1);
        let s2 = make_smooth_head_rot(333_333, 0.185, (4.0, 2.0), 2);
        let s_gt = make_smooth_head_rot(316_666, 0.0925, (2.0, 1.0), 0);

        let _ = render(
            &mut renderer,
            &s1,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: false,
                params: guard85,
            },
        );
        let px_smooth = render(
            &mut renderer,
            &s2,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: true,
                params: guard85,
            },
        );
        unsafe {
            renderer.flush_framegen_scd_sync(&gl);
        }
        let smooth_info = renderer.last_framegen_run().unwrap();

        let mut sh_err = 0u64;
        let mut sh_cnt = 0u64;
        let mut sh_edge_err = 0u64;
        let mut sh_edge_cnt = 0u64;
        let mut sh_halo_err = 0u64;
        let mut sh_halo_raw = 0u64;
        let mut sh_halo_cnt = 0u64;
        let head_cx_gt = width as f32 * 0.5 + 2.0;
        let head_cy_gt = 175.0 + 1.0;
        for gt_y in 140..208u32 {
            let gl_y = height - 1 - gt_y;
            for x in 245..399u32 {
                let dx = x as f32 - head_cx_gt;
                let dy = gt_y as f32 - head_cy_gt;
                let dist = (dx * dx + dy * dy).sqrt();
                let got = px_smooth[((gl_y * width + x) * 4) as usize] as i32;
                let gt = s_gt.data[((gt_y * width + x) * 4) as usize] as i32;
                let raw = s2.data[((gt_y * width + x) * 4) as usize] as i32;
                let e = (got - gt).unsigned_abs() as u64;
                if dist <= 20.5 {
                    sh_err += e;
                    sh_cnt += 1;
                }
                if (17.0..=25.0).contains(&dist) {
                    sh_edge_err += e;
                    sh_edge_cnt += 1;
                }
                if dist >= 24.0 && dist <= 70.0 && gt_y < 194 {
                    sh_halo_err += e;
                    sh_halo_raw += (raw - gt).unsigned_abs() as u64;
                    sh_halo_cnt += 1;
                }
            }
        }
        let sh_mae = sh_err as f64 / sh_cnt.max(1) as f64;
        let sh_edge_mae = sh_edge_err as f64 / sh_edge_cnt.max(1) as f64;
        let sh_halo_mae = sh_halo_err as f64 / sh_halo_cnt.max(1) as f64;
        let sh_halo_raw_mae = sh_halo_raw as f64 / sh_halo_cnt.max(1) as f64;
        assert!(
            !smooth_info.last_scene_cut,
            "Smooth round head spin at 85% guard should interpolate cleanly ({smooth_info:?})"
        );
        assert!(
            sh_mae < 6.0,
            "Smooth-shaded round character head must stay crisp during fast spin (sh_mae={sh_mae:.2})"
        );
        assert!(
            sh_edge_mae < 14.0,
            "Curved head silhouette ring must stay clean without tearing (sh_edge_mae={sh_edge_mae:.2})"
        );
        assert!(
            sh_halo_mae < 10.0 && sh_halo_mae < sh_halo_raw_mae * 0.30,
            "Halo around round head must stay clean (sh_halo_mae={sh_halo_mae:.2}, raw={sh_halo_raw_mae:.2})"
        );

        // Verify zero 8x8 blockiness spikes around the curved head/shoulder silhouette:
        // pixel gradients on 8px gridlines (`% 8 == 7`) must stay close to interior gradients (`% 8 == 3`).
        let mut on_grid_sum = 0.0f64;
        let mut on_grid_cnt = 0u64;
        let mut off_grid_sum = 0.0f64;
        let mut off_grid_cnt = 0u64;
        for gt_y in 132..244u32 {
            let gl_y = height - 1 - gt_y;
            let gl_yn = height - 1 - (gt_y + 1);
            for x in 236..404u32 {
                let c = px_smooth[((gl_y * width + x) * 4) as usize] as f64;
                let cx = px_smooth[((gl_y * width + (x + 1)) * 4) as usize] as f64;
                let cy = px_smooth[((gl_yn * width + x) * 4) as usize] as f64;
                let gx = (cx - c).abs();
                let gy = (cy - c).abs();
                if x % 8 == 7 {
                    on_grid_sum += gx;
                    on_grid_cnt += 1;
                } else if x % 8 == 3 {
                    off_grid_sum += gx;
                    off_grid_cnt += 1;
                }
                if gt_y % 8 == 7 {
                    on_grid_sum += gy;
                    on_grid_cnt += 1;
                } else if gt_y % 8 == 3 {
                    off_grid_sum += gy;
                    off_grid_cnt += 1;
                }
            }
        }
        let blockiness_8x8 = (on_grid_sum / on_grid_cnt.max(1) as f64)
            / (off_grid_sum / off_grid_cnt.max(1) as f64).max(1e-6);
        assert!(
            blockiness_8x8 < 1.15,
            "Interpolated silhouette must not exhibit 8x8 block boundary seams (blockiness_8x8={blockiness_8x8:.3})"
        );

        // Verify zero floating +-0.5*|mv| (~48px) silhouette-rim echo aura left & right of the head.
        let mut echo_errs: Vec<u32> = Vec::new();
        for gt_y in 154..194u32 {
            let gl_y = height - 1 - gt_y;
            for x in (244..266u32).chain(380..402u32) {
                let got = px_smooth[((gl_y * width + x) * 4) as usize] as i32;
                let gt = s_gt.data[((gt_y * width + x) * 4) as usize] as i32;
                echo_errs.push((got - gt).unsigned_abs());
            }
        }
        echo_errs.sort_unstable();
        let echo_p95 = echo_errs[(echo_errs.len() * 95) / 100];
        assert!(
            echo_p95 < 20,
            "Outer +-0.5*|mv| halo must not show floating silhouette-rim echo aura (echo_p95={echo_p95})"
        );

        renderer.destroy(&gl);
        painter.destroy();
    }

    #[test]
    #[ignore = "opt-in Mesa software EGL readback; set LIBGL_ALWAYS_SOFTWARE=1 EGL_PLATFORM=surfaceless"]
    fn ffx_framegen_preserves_thin_semitransparent_ui_reticle_and_walking_legs() {
        let width = 640u32;
        let height = 360u32;
        let egl = software_gl::SoftwareGl::new(width as i32, height as i32);
        #[allow(clippy::arc_with_non_send_sync)]
        let gl = Arc::new(egl.context());
        let mut painter = egui_glow::Painter::new(gl.clone(), "", None).unwrap();
        let mut renderer = CrtFilterRenderer::new(&gl);

        let output = (width as f32, height as f32);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(output.0, output.1));
        let render = |renderer: &mut CrtFilterRenderer,
                      frame: &RawFrame,
                      req: FrameGenPaintRequest|
         -> Vec<u8> {
            renderer.framegen_next_paint = Some(req);
            let area = renderer.draw_passthrough(
                &gl,
                &painter,
                rect,
                Some(frame),
                None,
                (frame.width, frame.height),
                output,
                [0.0, 0.0, 0.0],
                1.0,
                false,
                0.0,
                false,
                0,
                0.0,
                0.0,
                1.0,
                0.0,
                1.0,
                1.0,
                1.0,
                ScalerFilter::Bilinear as u8,
                super::super::DEFAULT_FSR1_SHARPNESS,
                0.0,
                0.0,
                0.0,
                0.0,
                [0.0, 0.0, 0.0, 0.0],
                None,
                0.0,
                0.0,
                None,
                None,
                None,
                0.0,
                None,
                None,
                false,
            );
            read_framebuffer_pixels_raw(&gl, area).unwrap().0
        };

        // 1. Panning map UI with a thin 3px semi-transparently outlined circular reticle at center
        //    and a 1-frame popping green banner pill at bottom-left (`f1`: absent, `f2`: present).
        let make_map_frame = |captured_at: i64,
                              shift_x: f32,
                              shift_y: f32,
                              show_pill: bool|
         -> RawFrame {
            let mut data = vec![0u8; (width * height * 4) as usize];
            let cx = width as f32 * 0.5;
            let cy = height as f32 * 0.5;
            for y in 0..height {
                for x in 0..width {
                    let wx = x as f32 - shift_x;
                    let wy = y as f32 - shift_y;
                    // Textured green map with crisp diagonal & curved white roads.
                    let wave = (wx * 0.045).sin() * (wy * 0.045).cos();
                    let road_d1 = ((wx * 0.55 - wy * 0.83 + 45.0).rem_euclid(140.0) - 70.0).abs();
                    let road_d2 = ((wx * 0.85 + wy * 0.52 - 20.0).rem_euclid(160.0) - 80.0).abs();
                    let mut rgb = if road_d1 < 6.0 || road_d2 < 4.5 {
                        [242.0f32, 245.0, 238.0]
                    } else {
                        [
                            115.0 + 18.0 * wave,
                            188.0 + 14.0 * wave,
                            140.0 + 14.0 * wave,
                        ]
                    };

                    // Semi-transparent dark top header bar (`y = 0..56`) with a 1px bottom
                    // divider line (`y = 55`) over the panning map and crisp stationary white
                    // glyph strokes (`y = 18..32`, `x = 180..460`) mimicking PS5 map tabs
                    // (`JASON & LUCIA | MAP | BRIEF | PROGRESS`).
                    if y < 56 {
                        rgb = [
                            rgb[0] * 0.22 + 34.0 * 0.78,
                            rgb[1] * 0.22 + 38.0 * 0.78,
                            rgb[2] * 0.22 + 56.0 * 0.78,
                        ];
                        if y == 55 {
                            rgb = [22.0, 26.0, 38.0];
                        } else if (18..32).contains(&y)
                            && (180..460).contains(&x)
                            && ((x - 180) % 9 < 3 || ((y == 18 || y == 31) && (x - 180) % 9 < 6))
                        {
                            rgb = [244.0, 246.0, 248.0];
                        }
                    }

                    // Stationary 1-frame popping green banner pill at bottom-left (`x=24..210, y=308..338`).
                    if show_pill && (24..210).contains(&x) && (308..338).contains(&y) {
                        rgb = [14.0, 128.0, 88.0];
                    }

                    // Stationary thin circular reticle at screen center with semi-transparent dark outline
                    // + 3px bright white core (4 segmented arcs, just like the PS5 map cursor!).
                    let dx = x as f32 - cx;
                    let dy = y as f32 - cy;
                    let r = (dx * dx + dy * dy).sqrt();
                    let in_arc = dx.abs() > 5.0 || dy.abs() > 5.0;
                    if in_arc && (r - 22.0).abs() <= 2.6 {
                        if (r - 22.0).abs() <= 1.2 {
                            rgb = [245.0, 248.0, 245.0];
                        } else {
                            // 55%-alpha dark outline over moving map
                            rgb = [rgb[0] * 0.45, rgb[1] * 0.45, rgb[2] * 0.45];
                        }
                    }

                    let idx = ((y * width + x) * 4) as usize;
                    data[idx] = rgb[0].round().clamp(0.0, 255.0) as u8;
                    data[idx + 1] = rgb[1].round().clamp(0.0, 255.0) as u8;
                    data[idx + 2] = rgb[2].round().clamp(0.0, 255.0) as u8;
                    data[idx + 3] = 255;
                }
            }
            RawFrame {
                captured_at,
                rate: crate::replay::config::Rate { num: 30, den: 1 },
                width,
                height,
                data,
                format: Pixel::RGBA,
                color_range: crate::video::types::ColorRange::Full,
            }
        };

        let m1 = make_map_frame(100_000, 0.0, 0.0, false);
        let m2 = make_map_frame(133_333, -24.0, -20.0, true);
        let m_gt = make_map_frame(116_666, -12.0, -10.0, false);
        let _ = render(
            &mut renderer,
            &m1,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: false,
                ..Default::default()
            },
        );
        let px_map_mid = render(
            &mut renderer,
            &m2,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: true,
                ..Default::default()
            },
        );
        unsafe {
            renderer.flush_framegen_scd_sync(&gl);
        }
        assert!(
            !renderer.last_framegen_run().unwrap().last_scene_cut,
            "Map pan with reticle, translucent top bar, and popping pill should interpolate cleanly"
        );

        // Reticle white core across all 4 cardinal arcs must stay bright and intact.
        let sample_rgb = |buf: &[u8], x: u32, gt_y: u32| -> [i32; 3] {
            let gl_y = height - 1 - gt_y;
            let idx = ((gl_y * width + x) * 4) as usize;
            [buf[idx] as i32, buf[idx + 1] as i32, buf[idx + 2] as i32]
        };
        for (label, ax, ay) in [
            ("left", width / 2 - 22, height / 2),
            ("right", width / 2 + 22, height / 2),
            ("top", width / 2, height / 2 - 22),
            ("bottom", width / 2, height / 2 + 22),
        ] {
            let arc = sample_rgb(&px_map_mid, ax, ay);
            let arc_lum = (arc[0] + arc[1] + arc[2]) / 3;
            assert!(
                arc_lum > 205,
                "Thin stationary reticle {label} arc must stay crisp and locked (arc={arc:?})"
            );
        }
        // Hollow interior of the reticle (`r <= 14px`) must smoothly reconstruct the panning map
        // without cloned reticle arc fragments, green holes, or double-arc ghost blobs.
        let mut inner_err = 0u64;
        let mut inner_cnt = 0u64;
        let cx_i = (width / 2) as i32;
        let cy_i = (height / 2) as i32;
        for gt_y in (height / 2 - 14)..=(height / 2 + 14) {
            for x in (width / 2 - 14)..=(width / 2 + 14) {
                let dx = x as i32 - cx_i;
                let dy = gt_y as i32 - cy_i;
                if dx * dx + dy * dy <= 14 * 14 {
                    let got = sample_rgb(&px_map_mid, x, gt_y);
                    let idx = ((gt_y * width + x) * 4) as usize;
                    let gt = [
                        m_gt.data[idx] as i32,
                        m_gt.data[idx + 1] as i32,
                        m_gt.data[idx + 2] as i32,
                    ];
                    inner_err += ((got[0] - gt[0]).unsigned_abs()
                        + (got[1] - gt[1]).unsigned_abs()
                        + (got[2] - gt[2]).unsigned_abs()) as u64;
                    inner_cnt += 3;
                }
            }
        }
        let inner_mae = inner_err as f64 / inner_cnt.max(1) as f64;
        assert!(
            inner_mae < 18.0,
            "Hollow reticle interior must cleanly interpolate the moving map without ghost blobs or cloned arcs (inner_mae={inner_mae:.2})"
        );
        // Semi-transparent dark top header bar must NOT have white text cloned into empty header
        // rows above (`y = 2..14`) or below (`y = 36..50`) the stationary text (`y = 18..32`),
        // nor bright map grass streaks leaking into the bar interior.
        let mut top_bar_ghost_px = 0u32;
        for gt_y in (2..14u32).chain(36..50u32) {
            for x in 170..470u32 {
                let rgb = sample_rgb(&px_map_mid, x, gt_y);
                let lum = (rgb[0] + rgb[1] + rgb[2]) / 3;
                if lum > 115 {
                    top_bar_ghost_px += 1;
                }
            }
        }
        assert!(
            top_bar_ghost_px == 0,
            "Stationary UI text on translucent top bar must never be cloned into adjacent empty bar rows (top_bar_ghost_px={top_bar_ghost_px})"
        );
        // Header text strokes themselves must stay crisp and bright.
        let mut header_stroke_lum_sum = 0i64;
        let mut header_stroke_cnt = 0i64;
        for step in 0..30u32 {
            let x = 181 + step * 9;
            let rgb = sample_rgb(&px_map_mid, x, 25);
            header_stroke_lum_sum += ((rgb[0] + rgb[1] + rgb[2]) / 3) as i64;
            header_stroke_cnt += 1;
        }
        let header_stroke_lum = header_stroke_lum_sum as f64 / header_stroke_cnt.max(1) as f64;
        assert!(
            header_stroke_lum > 215.0,
            "Stationary header UI text strokes must stay crisp and bright (header_stroke_lum={header_stroke_lum:.1})"
        );
        // Check that popping green pill at `y = 308..338` was NOT dragged down to `y = 344..352`
        // (dark green pill has R < 40, whereas pale map grass/road has R > 95).
        let mut leaked_pill_px = 0u32;
        for gt_y in 344..352u32 {
            for x in 40..195u32 {
                let rgb = sample_rgb(&px_map_mid, x, gt_y);
                if rgb[0] < 70 {
                    leaked_pill_px += 1;
                }
            }
        }
        assert!(
            leaked_pill_px < 20,
            "Pop-in UI pill must not be dragged outside its screen box (leaked_pill_px={leaked_pill_px})"
        );

        // 2. Perspective walking character test: top half (sky/trees) moves fast horizontally (+36px
        //    at 360p ~= +108px at 1080p), bottom half (road) moves slower with perspective (+14, +5),
        //    and a narrow 28px-wide grey walking leg moves slowly (-3, +3) with luminance close to
        //    the road.
        let make_walk_frame = |captured_at: i64, step: f32| -> RawFrame {
            let mut data = vec![0u8; (width * height * 4) as usize];
            for y in 0..height {
                for x in 0..width {
                    let yf = y as f32;
                    let xf = x as f32;
                    // Depth-varying horizontal background motion: +36px in upper sky/trees -> +14px on lower road.
                    let t_depth = ((yf - 110.0) / 220.0).clamp(0.0, 1.0);
                    let bg_vx = 36.0 * (1.0 - t_depth) + 14.0 * t_depth;
                    let bg_vy = 5.0 * t_depth;
                    let sx = xf - bg_vx * step;
                    let sy = yf - bg_vy * step;
                    let bg_tex = 115.0
                        + 34.0 * (sx * 0.032).sin() * (sy * 0.045).cos()
                        + 20.0 * (sx * 0.075 + sy * 0.06).sin()
                        + 12.0 * (sx * 0.16 - sy * 0.14).cos();

                    // Narrow 28px-wide walking leg at `x = 306..334, y = 180..356`, moving (-3, +3).
                    let leg_cx = 320.0 - 3.0 * step;
                    let in_leg = (xf - leg_cx).abs() < 14.0 && yf >= 180.0 && yf < 356.0;
                    let v = if in_leg {
                        // Smoothly shaded grey pant leg (similar average brightness to road!) + bright white sock near ankle.
                        if yf >= 332.0 {
                            235.0
                        } else {
                            let cyl = 1.0 - ((xf - leg_cx) / 14.0).powi(2);
                            (105.0 + 38.0 * cyl + 6.0 * ((yf - 3.0 * step) * 0.15).sin())
                                .clamp(0.0, 255.0)
                        }
                    } else {
                        bg_tex.clamp(25.0, 230.0)
                    };
                    let u = v.round() as u8;
                    let idx = ((y * width + x) * 4) as usize;
                    data[idx] = u;
                    data[idx + 1] = u;
                    data[idx + 2] = u;
                    data[idx + 3] = 255;
                }
            }
            RawFrame {
                captured_at,
                rate: crate::replay::config::Rate { num: 30, den: 1 },
                width,
                height,
                data,
                format: Pixel::RGBA,
                color_range: crate::video::types::ColorRange::Full,
            }
        };

        renderer.framegen.reset_history();
        let w1 = make_walk_frame(300_000, 0.0);
        let w2 = make_walk_frame(333_333, 1.0);
        let w_gt = make_walk_frame(316_666, 0.5);
        let _ = render(
            &mut renderer,
            &w1,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: false,
                ..Default::default()
            },
        );
        let px_walk_mid = render(
            &mut renderer,
            &w2,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: true,
                ..Default::default()
            },
        );
        unsafe {
            renderer.flush_framegen_scd_sync(&gl);
        }
        assert!(
            !renderer.last_framegen_run().unwrap().last_scene_cut,
            "Perspective walking scene should interpolate cleanly"
        );

        let mut leg_err = 0u64;
        let mut leg_cnt = 0u64;
        for gt_y in 200..346u32 {
            for x in 311..326u32 {
                let got = sample_rgb(&px_walk_mid, x, gt_y)[0];
                let gt = w_gt.data[((gt_y * width + x) * 4) as usize] as i32;
                leg_err += (got - gt).unsigned_abs() as u64;
                leg_cnt += 1;
            }
        }
        let leg_mae = leg_err as f64 / leg_cnt.max(1) as f64;
        assert!(
            leg_mae < 12.0,
            "Narrow walking leg must not be shredded by sky velocity probes or global hole fill (leg_mae={leg_mae:.2})"
        );

        // 3. Smooth shadowed dark torso + bobbing bald head under fast horizontal pan (-28px at
        //    360p ~= -84px at 1080p): verifies (a) wide smooth shadow interiors inside the torso
        //    do not leak fast background velocity onto the shirt, and (b) disoccluded background
        //    just outside the head silhouette does not pick up a fake half-speed smear halo.
        let make_jog_frame = |captured_at: i64, step: f32| -> RawFrame {
            let mut data = vec![0u8; (width * height * 4) as usize];
            for y in 0..height {
                for x in 0..width {
                    let xf = x as f32;
                    let yf = y as f32;
                    let bg_x = xf + 28.0 * step;
                    let bg_r = (135.0
                        + 45.0 * (bg_x * 0.045).sin() * (yf * 0.035).cos()
                        + 30.0 * (bg_x * 0.11 + yf * 0.05).sin())
                    .clamp(35.0, 230.0);
                    let bg_g = (120.0
                        + 35.0 * (bg_x * 0.04 + yf * 0.05).cos()
                        + 25.0 * (bg_x * 0.09).sin())
                    .clamp(35.0, 220.0);
                    let bg_b = (95.0 + 30.0 * (bg_x * 0.055 - yf * 0.04).sin()).clamp(30.0, 210.0);

                    let hx = 280.0 - 2.0 * step;
                    let hy = 135.0 - 2.0 * step;
                    let in_head = ((xf - hx) / 22.0).powi(2) + ((yf - hy) / 28.0).powi(2) <= 1.0;
                    let in_torso = (xf - hx).abs() <= 54.0 && yf >= 164.0 && yf < 340.0;

                    let (r, g, b) = if in_head {
                        let lit = (1.0 - ((xf - (hx + 6.0)) / 28.0).powi(2)).clamp(0.0, 1.0);
                        (
                            (130.0 + 55.0 * lit).clamp(0.0, 255.0),
                            (95.0 + 42.0 * lit).clamp(0.0, 255.0),
                            (74.0 + 32.0 * lit).clamp(0.0, 255.0),
                        )
                    } else if in_torso {
                        // Smooth dark-grey shirt with a shifting tree shadow band across the back.
                        let sh_x = xf - (hx - 14.0) + 10.0 * step;
                        let shadow = if sh_x.abs() < 20.0 { 0.72 } else { 1.0 };
                        let shade = (78.0 + 6.0 * (yf * 0.04).sin()) * shadow;
                        (shade, shade + 2.0, shade + 8.0)
                    } else {
                        (bg_r, bg_g, bg_b)
                    };
                    let idx = ((y * width + x) * 4) as usize;
                    data[idx] = r.round() as u8;
                    data[idx + 1] = g.round() as u8;
                    data[idx + 2] = b.round() as u8;
                    data[idx + 3] = 255;
                }
            }
            RawFrame {
                captured_at,
                rate: crate::replay::config::Rate { num: 30, den: 1 },
                width,
                height,
                data,
                format: Pixel::RGBA,
                color_range: crate::video::types::ColorRange::Full,
            }
        };

        renderer.framegen.reset_history();
        let j1 = make_jog_frame(400_000, 0.0);
        let j2 = make_jog_frame(433_333, 1.0);
        let j_gt = make_jog_frame(416_666, 0.5);
        let _ = render(
            &mut renderer,
            &j1,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: false,
                ..Default::default()
            },
        );
        let px_jog_mid = render(
            &mut renderer,
            &j2,
            FrameGenPaintRequest {
                enabled: true,
                show_interpolated: true,
                ..Default::default()
            },
        );
        unsafe {
            renderer.flush_framegen_scd_sync(&gl);
        }
        assert!(
            !renderer.last_framegen_run().unwrap().last_scene_cut,
            "Jogging character under fast pan should interpolate cleanly"
        );

        let mut torso_bad_px = 0u32;
        for gt_y in 184..320u32 {
            for x in 242..316u32 {
                let rgb = sample_rgb(&px_jog_mid, x, gt_y);
                let gi = ((gt_y * width + x) * 4) as usize;
                let dr = (rgb[0] - j_gt.data[gi] as i32).abs();
                let dg = (rgb[1] - j_gt.data[gi + 1] as i32).abs();
                let db = (rgb[2] - j_gt.data[gi + 2] as i32).abs();
                if dr.max(dg).max(db) > 36 {
                    torso_bad_px += 1;
                }
            }
        }
        assert!(
            torso_bad_px < 40,
            "Smooth shadowed torso must not leak background patches (torso_bad_px={torso_bad_px})"
        );

        let mut right_halo_err = 0u64;
        let mut right_halo_cnt = 0u64;
        for gt_y in 112..156u32 {
            for x in 306..328u32 {
                let rgb = sample_rgb(&px_jog_mid, x, gt_y);
                let gi = ((gt_y * width + x) * 4) as usize;
                let e = ((rgb[0] - j_gt.data[gi] as i32).abs()
                    + (rgb[1] - j_gt.data[gi + 1] as i32).abs()
                    + (rgb[2] - j_gt.data[gi + 2] as i32).abs()) as u64;
                right_halo_err += e;
                right_halo_cnt += 3;
            }
        }
        let right_halo_mae = right_halo_err as f64 / right_halo_cnt.max(1) as f64;
        assert!(
            right_halo_mae < 14.0,
            "Disoccluded background right beside head silhouette must not suffer fake half-speed smear halo (right_halo_mae={right_halo_mae:.2})"
        );

        renderer.destroy(&gl);
        painter.destroy();
    }

    #[test]
    #[ignore = "requires software OpenGL/EGL (Mesa llvmpipe)"]
    fn ffx_framegen_clean_menu_selector_snaps_and_cloud_spin_silhouette() {
        let width = 640u32;
        let height = 360u32;
        let egl = software_gl::SoftwareGl::new(width as i32, height as i32);
        #[allow(clippy::arc_with_non_send_sync)]
        let gl = Arc::new(egl.context());
        let mut painter = egui_glow::Painter::new(gl.clone(), "", None).unwrap();
        let mut renderer = CrtFilterRenderer::new(&gl);
        let output = (width as f32, height as f32);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(output.0, output.1));

        let render =
            |renderer: &mut CrtFilterRenderer, frame: &RawFrame, show_interp: bool| -> Vec<u8> {
                renderer.framegen_next_paint = Some(FrameGenPaintRequest {
                    enabled: true,
                    show_interpolated: show_interp,
                    params: super::super::FfxFrameGenParams::default(),
                });
                let area = renderer.draw_passthrough(
                    &gl,
                    &painter,
                    rect,
                    Some(frame),
                    None,
                    (width, height),
                    output,
                    [0.0, 0.0, 0.0],
                    1.0,
                    false,
                    0.0,
                    false,
                    0,
                    0.0,
                    0.0,
                    1.0,
                    0.0,
                    1.0,
                    1.0,
                    1.0,
                    ScalerFilter::Bilinear as u8,
                    super::super::DEFAULT_FSR1_SHARPNESS,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    [0.0; 4],
                    None,
                    0.0,
                    0.0,
                    None,
                    None,
                    None,
                    0.0,
                    None,
                    None,
                    false,
                );
                read_framebuffer_pixels_raw(&gl, area).unwrap().0
            };

        // Scenario 1 & 2: Pause menu with 5 stacked rows (row pitch = 48px), static text strokes
        // inside each row, AND a softly animating/drifting blurred bokeh backdrop on the right 65%
        // of the screen (matching real pause menus). Moving the bright white highlight bar +48px
        // (adjacent step) OR wrapping -192px (bottom -> top) must snap cleanly to crisp `curr`
        // without mid-way ghost trails or half-rendered white/gray bars, while an idle menu pair
        // with only gentle bokeh drift stays in active framegen without false fallback.
        let make_menu = |captured_at: i64, active_row: u32, bokeh_phase: f32| -> RawFrame {
            let mut data = vec![0u8; (width * height * 4) as usize];
            for y in 0..height {
                for x in 0..width {
                    let fx = x as f32;
                    let fy = y as f32;
                    let b1 = (0.5
                        + 0.5 * ((fx - 360.0) * 0.014).sin() * ((fy - 160.0) * 0.016).cos())
                    .powf(2.0);
                    let b2 = (0.5
                        + 0.5 * ((fx - 500.0) * 0.019).cos() * ((fy - 220.0) * 0.013).sin())
                    .powf(2.0);
                    // Gentle pause-menu bokeh intensity pulse (~3x real PS5 pause-menu backdrop drift)
                    let pulse1 = 1.0 + 0.018 * bokeh_phase;
                    let pulse2 = 1.0 - 0.015 * bokeh_phase;
                    let mut rgb = [
                        (34.0 + 70.0 * b1 * pulse1 + 22.0 * b2 * pulse2).clamp(0.0, 255.0) as u8,
                        (28.0 + 38.0 * b1 * pulse1 + 52.0 * b2 * pulse2).clamp(0.0, 255.0) as u8,
                        (46.0 + 62.0 * b1 * pulse1 + 68.0 * b2 * pulse2).clamp(0.0, 255.0) as u8,
                    ];
                    // Left menu sidebar panel (x = 24..240)
                    if (24..240).contains(&x) {
                        rgb = [40, 38, 52];
                        let row_top = 48 + active_row * 48;
                        let in_bar = (36..228).contains(&x) && (row_top..row_top + 36).contains(&y);
                        if in_bar {
                            let grad = 245i32 - ((x - 36) as i32 * 55 / 192);
                            let v = grad.clamp(180, 250) as u8;
                            rgb = [v, v, v];
                        }
                        // Crisp horizontal/vertical glyph strokes on every menu row
                        for row in 0..5u32 {
                            let ty = 48 + row * 48 + 12;
                            if (ty..ty + 12).contains(&y) && (56..152).contains(&x) {
                                let glyph =
                                    ((x - 56) % 10 < 3) || (y == ty + 5 && (x - 56) % 10 < 7);
                                if glyph {
                                    rgb = if row == active_row {
                                        [24, 24, 28]
                                    } else {
                                        [238, 238, 244]
                                    };
                                }
                            }
                        }
                    }
                    let idx = ((y * width + x) * 4) as usize;
                    data[idx] = rgb[0];
                    data[idx + 1] = rgb[1];
                    data[idx + 2] = rgb[2];
                    data[idx + 3] = 255;
                }
            }
            RawFrame {
                captured_at,
                rate: crate::replay::config::Rate { num: 30, den: 1 },
                width,
                height,
                data,
                format: Pixel::RGBA,
                color_range: crate::video::types::ColorRange::Full,
            }
        };

        // Idle pause menu (same active row, subtle bokeh drift only) must not false-trigger fallback.
        let mi1 = make_menu(60_000, 1, 0.0);
        let mi2 = make_menu(93_333, 1, 1.0);
        let _ = render(&mut renderer, &mi1, false);
        let _ = render(&mut renderer, &mi2, true);
        unsafe {
            renderer.flush_framegen_scd_sync(&gl);
        }
        let idle_info = renderer.last_framegen_run().unwrap();
        assert!(
            !idle_info.last_scene_cut,
            "Idle pause menu with subtle background drift must not false-trigger fallback ({idle_info:?})"
        );

        // Scenario 1: Adjacent row jump over animated bokeh backdrop (row 1 -> row 2, dy = +48 px)
        renderer.framegen.reset_history();
        let m1 = make_menu(100_000, 1, 1.0);
        let m2 = make_menu(133_333, 2, 2.0);
        let _ = render(&mut renderer, &m1, false);
        let px_step = render(&mut renderer, &m2, true);
        unsafe {
            renderer.flush_framegen_scd_sync(&gl);
        }
        let step_info = renderer.last_framegen_run().unwrap();
        let mut step_err = 0u64;
        let mut step_cnt = 0u64;
        for y in 40..280u32 {
            let gl_y = height - 1 - y;
            for x in 24..240u32 {
                for ch in 0..3usize {
                    let got = px_step[((gl_y * width + x) * 4) as usize + ch] as i32;
                    let exp = m2.data[((y * width + x) * 4) as usize + ch] as i32;
                    step_err += (got - exp).unsigned_abs() as u64;
                    step_cnt += 1;
                }
            }
        }
        let step_mae = step_err as f64 / step_cnt as f64;
        assert!(
            step_info.last_scene_cut && step_mae < 1.0,
            "Static menu selector step over animated backdrop must snap cleanly without ghost trails (info={step_info:?}, step_mae={step_mae:.2})"
        );

        // Scenario 2: Out-of-range wrap-around flip over animated bokeh backdrop (row 4 -> row 0)
        renderer.framegen.reset_history();
        let mw1 = make_menu(200_000, 4, 2.0);
        let mw2 = make_menu(233_333, 0, 3.0);
        let _ = render(&mut renderer, &mw1, false);
        let px_wrap = render(&mut renderer, &mw2, true);
        unsafe {
            renderer.flush_framegen_scd_sync(&gl);
        }
        let wrap_info = renderer.last_framegen_run().unwrap();
        let mut wrap_err = 0u64;
        let mut wrap_cnt = 0u64;
        for y in 40..280u32 {
            let gl_y = height - 1 - y;
            for x in 24..240u32 {
                for ch in 0..3usize {
                    let got = px_wrap[((gl_y * width + x) * 4) as usize + ch] as i32;
                    let exp = mw2.data[((y * width + x) * 4) as usize + ch] as i32;
                    wrap_err += (got - exp).unsigned_abs() as u64;
                    wrap_cnt += 1;
                }
            }
        }
        let wrap_mae = wrap_err as f64 / wrap_cnt as f64;
        assert!(
            wrap_info.last_scene_cut && wrap_mae < 1.0,
            "Wrap-around menu selector flip over animated backdrop must snap cleanly (info={wrap_info:?}, wrap_mae={wrap_mae:.2})"
        );

        // Scenario 3: Fast camera spin (-72 px/frame) across bright clipped-white clouds + vertical
        // wooden pole behind a stationary dark-haired character head: verifies zero bogus `(0, 0)`
        // cloud islands / pole squiggles and zero dilated hair-side tumors!
        renderer.framegen.reset_history();
        let make_cloud_pole_spin = |captured_at: i64, shift_x: f32| -> RawFrame {
            let mut data = vec![0u8; (width * height * 4) as usize];
            let head_cx = 220.0f32;
            let head_cy = 232.0f32;
            let head_rx = 42.0f32;
            let head_ry = 52.0f32;
            let pole_x = 396.0 + shift_x;
            for y in 0..height {
                for x in 0..width {
                    let fx = x as f32;
                    let fy = y as f32;
                    let sx = fx - shift_x;
                    // Textured sky with a large bright clipped-white cloud core (x_world in 390..540)
                    let cloud_dx = (sx - 465.0) / 88.0;
                    let cloud_dy = (fy - 165.0) / 72.0;
                    let cloud_core = (1.0 - (cloud_dx * cloud_dx + cloud_dy * cloud_dy)).max(0.0);
                    let ripple = 0.5
                        + 0.25 * (sx * 0.045).sin() * (fy * 0.055).cos()
                        + 0.15 * (sx * 0.11 + fy * 0.08).cos();
                    let sky_l = (0.48 + 0.30 * ripple + 0.95 * cloud_core).clamp(0.0, 1.0);
                    let mut rgb = [
                        (sky_l * 255.0).round() as u8,
                        ((sky_l * 0.985) * 255.0).round() as u8,
                        ((sky_l * 0.94 + (1.0 - sky_l) * 0.18).clamp(0.0, 1.0) * 255.0).round()
                            as u8,
                    ];

                    // Vertical wooden porch pole moving with the spinning background
                    if (fx - pole_x).abs() <= 11.0 {
                        let t = 1.0 - ((fx - pole_x).abs() / 11.0);
                        rgb = [
                            (78.0 + 48.0 * t) as u8,
                            (48.0 + 28.0 * t) as u8,
                            (28.0 + 16.0 * t) as u8,
                        ];
                    }

                    // Stationary dark-haired character head + shoulders in front of the sky
                    let hx = (fx - head_cx) / head_rx;
                    let hy = (fy - head_cy) / head_ry;
                    let in_head = (hx * hx + hy * hy) <= 1.0;
                    let in_shoulders = (fx - head_cx).abs() <= 68.0 && fy >= head_cy + 40.0;
                    if in_head || in_shoulders {
                        let strand = 0.5 + 0.5 * (fx * 0.35 + fy * 0.15).sin();
                        rgb = [
                            (24.0 + 16.0 * strand) as u8,
                            (20.0 + 12.0 * strand) as u8,
                            (22.0 + 12.0 * strand) as u8,
                        ];
                    }

                    let idx = ((y * width + x) * 4) as usize;
                    data[idx] = rgb[0];
                    data[idx + 1] = rgb[1];
                    data[idx + 2] = rgb[2];
                    data[idx + 3] = 255;
                }
            }
            RawFrame {
                captured_at,
                rate: crate::replay::config::Rate { num: 30, den: 1 },
                width,
                height,
                data,
                format: Pixel::RGBA,
                color_range: crate::video::types::ColorRange::Full,
            }
        };

        let c1 = make_cloud_pole_spin(300_000, 0.0);
        let c2 = make_cloud_pole_spin(333_333, -72.0);
        let c_gt = make_cloud_pole_spin(316_666, -36.0);
        let _ = render(&mut renderer, &c1, false);
        let px_cloud = render(&mut renderer, &c2, true);
        unsafe {
            renderer.flush_framegen_scd_sync(&gl);
        }
        let cloud_info = renderer.last_framegen_run().unwrap();
        assert!(
            !cloud_info.last_scene_cut,
            "72px/frame cloud+pole spin must stay in active framegen mode ({cloud_info:?})"
        );

        let region_mae = |x0: u32, x1: u32, y0: u32, y1: u32| -> f64 {
            let mut sum = 0u64;
            let mut cnt = 0u64;
            for y in y0..y1 {
                let gl_y = height - 1 - y;
                for x in x0..x1 {
                    for ch in 0..3usize {
                        let got = px_cloud[((gl_y * width + x) * 4) as usize + ch] as i32;
                        let exp = c_gt.data[((y * width + x) * 4) as usize + ch] as i32;
                        sum += (got - exp).unsigned_abs() as u64;
                        cnt += 1;
                    }
                }
            }
            sum as f64 / cnt.max(1) as f64
        };

        // 1. Bright clipped-white cloud region right of the interpolated pole (no `(0, 0)` squiggle!)
        let cloud_mae = region_mae(385, 500, 100, 230);
        // 2. Interpolated wooden pole position (x = 349..371) stays solid and sharp
        let pole_mae = region_mae(349, 371, 60, 260);
        // 3. Disoccluded sky immediately to the right of Lucia's hair (x = 266..320, y = 185..265)
        let right_sky_mae = region_mae(266, 320, 185, 265);
        // 4. Character head interior (x = 190..250, y = 195..265)
        let head_mae = region_mae(190, 250, 195, 265);
        assert!(
            cloud_mae < 5.0,
            "Bright clipped-white cloud right of pole must not suffer bogus (0, 0) snap squiggles (cloud_mae={cloud_mae:.2})"
        );
        assert!(
            pole_mae < 12.0,
            "Thin wooden pole must stay intact across clipped clouds (pole_mae={pole_mae:.2})"
        );
        assert!(
            right_sky_mae < 8.0,
            "Disoccluded sky beside dark hair must not suffer dilated (0, 0) tumors (right_sky_mae={right_sky_mae:.2})"
        );
        assert!(
            head_mae < 5.0,
            "Stationary dark-haired character head must stay solid during spin (head_mae={head_mae:.2})"
        );

        renderer.destroy(&gl);
        painter.destroy();
    }
}
