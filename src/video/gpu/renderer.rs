use super::anime4k::{Anime4kUpscaler, Anime4kVariant};
use super::fft_filter::FftFilter;
use super::geometry::RenderedArea;
use super::halo::HaloRenderer;
use super::params::{
    CathodeInterferenceShaderParams, GlassShaderParams, HaloShaderParams, ShaderParams,
};
use super::programs::*;
use crate::video::types::{RawFrame, ScalerFilter};
use eframe::egui;
use eframe::glow::{self, HasContext};
use ffmpeg_next::format::Pixel;
use std::num::NonZero;
use std::sync::{Arc, Mutex};

const PIXELATE_TARGET_HEIGHT: u32 = 480;

pub const RETRO_CURSOR_WIDTH: u32 = 16;
pub const RETRO_CURSOR_HEIGHT: u32 = 24;

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
    pub halo: HaloRenderer,

    fbos: [glow::Framebuffer; 7],
    pass_textures: [glow::Texture; 7],
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
    popup_prog: glow::Program,
    popup_vao: glow::VertexArray,
    popup_vbo: glow::Buffer,
    popup_ebo: glow::Buffer,
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

    post_fbos: [glow::Framebuffer; 2],
    post_textures: [glow::Texture; 2],
    last_post_size: (u32, u32),

    last_size: (u32, u32),
    last_scaler_filter: Option<u8>,
    last_pass_res: (u32, u32),
    last_frame_size: (u32, u32),
    last_frame_format: Option<Pixel>,
    last_frame_captured_at: i64,
    history_count: u32,
}

fn intermediate_texture_internal_format(pass_index: usize) -> u32 {
    match pass_index {
        // These passes store linear RGB that is later sampled by another shader before
        // the final linear->sRGB presentation step, so RGBA8 causes visible
        // dark-area quantization. Keep them in half-float instead.
        0 | 1 | 2 | 4..=6 => glow::RGBA16F,
        _ => glow::RGBA8,
    }
}

fn pixelate_subrender_size(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (1, 1);
    }

    let target_height = height.clamp(1, PIXELATE_TARGET_HEIGHT);
    let target_width = (width as u64 * target_height as u64)
        .div_ceil(height as u64)
        .max(1) as u32;
    (target_width, target_height)
}

fn pass_texture_dimensions(
    pass_index: usize,
    source_size: (u32, u32),
    effective_size: (u32, u32),
) -> (u32, u32) {
    match pass_index {
        // Pass 4 is the low-scale pixelate subrender. The final pass samples it
        // back to the output surface, preserving the existing shader order.
        4 => pixelate_subrender_size(effective_size.0, effective_size.1),
        // History (0: t-1, 2: t-2), Deinterlace (1), Median (5), and YUV (6) happen before any upscaling.
        0 | 1 | 2 | 5 | 6 => source_size,
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

            let retro_output_res_loc = gl.get_uniform_location(retro_frame_prog, "outputResolution");
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
            let glass_flash_enabled_loc =
                gl.get_uniform_location(crt_glass_prog, "flash_enabled");
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
            let popup_scanline_strength_loc = gl.get_uniform_location(popup_prog, "u_scanline_strength");
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
            let retro_mouse_screen_size_loc = gl.get_uniform_location(retro_mouse_prog, "u_screen_size");
            let retro_mouse_cursor_pos_loc = gl.get_uniform_location(retro_mouse_prog, "u_cursor_pos");
            let retro_mouse_cursor_size_loc = gl.get_uniform_location(retro_mouse_prog, "u_cursor_size");
            let retro_mouse_sampler_loc = gl.get_uniform_location(retro_mouse_prog, "u_cursor_sampler");
            let retro_mouse_shadow_mask_loc = gl.get_uniform_location(retro_mouse_prog, "u_shadow_mask");
            let retro_mouse_scanline_strength_loc = gl.get_uniform_location(retro_mouse_prog, "u_scanline_strength");
            let retro_mouse_scanline_freq_loc = gl.get_uniform_location(retro_mouse_prog, "u_scanline_freq");
            let retro_mouse_brightboost_loc = gl.get_uniform_location(retro_mouse_prog, "u_brightboost");

            gl.use_program(Some(retro_mouse_prog));
            if let Some(ref loc) = retro_mouse_sampler_loc {
                gl.uniform_1_i32(Some(loc), 0);
            }
            gl.use_program(None);

            let retro_mouse_texture = gl.create_texture().expect("Cannot create retro mouse texture");
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
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
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
            let deinterlace_blend_loc =
                gl.get_uniform_location(deinterlace_prog, "blend_amount").unwrap();
            let deinterlace_motion_thresh_loc =
                gl.get_uniform_location(deinterlace_prog, "motion_threshold").unwrap();
            let deinterlace_line_spacing_loc =
                gl.get_uniform_location(deinterlace_prog, "line_spacing").unwrap();
            let deinterlace_spatial_mix_loc =
                gl.get_uniform_location(deinterlace_prog, "spatial_mix").unwrap();
            let deinterlace_current_frame_loc =
                gl.get_uniform_location(deinterlace_prog, "current_frame");
            let deinterlace_prev_frame_loc =
                gl.get_uniform_location(deinterlace_prog, "prev_frame");
            let deinterlace_prev_frame_2_loc =
                gl.get_uniform_location(deinterlace_prog, "prev_frame_2");
            let deinterlace_has_prev2_loc =
                gl.get_uniform_location(deinterlace_prog, "has_prev2");

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
            let final_border_crop_loc =
                gl.get_uniform_location(final_prog, "border_crop").unwrap();
            let passthrough_border_crop_loc = gl
                .get_uniform_location(passthrough_prog, "border_crop")
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
            let cathode_intensity_loc = gl
                .get_uniform_location(cathode_prog, "intensity")
                .unwrap();
            let cathode_frequency_loc = gl
                .get_uniform_location(cathode_prog, "frequency")
                .unwrap();
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

            gl.use_program(None);

            let fbos = [
                gl.create_framebuffer().unwrap(),
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
            let post_textures = [
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
            ];

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

            let mut bezel_img = image::load_from_memory(include_bytes!("../../../assets/nec_pc98_bezel.png"))
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
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::REPEAT as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::REPEAT as i32,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);

            let mut sil_img = image::load_from_memory(include_bytes!("../../../assets/crt_reflection_silhouette.png"))
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
                popup_prog,
                popup_vao,
                popup_vbo,
                popup_ebo,
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
                post_fbos,
                post_textures,
                last_post_size: (0, 0),
                anime4k_small: Anime4kUpscaler::new(gl, Anime4kVariant::Small),
                anime4k_medium: Anime4kUpscaler::new(gl, Anime4kVariant::Medium),
                anime4k_large: Anime4kUpscaler::new(gl, Anime4kVariant::Large),
                halo: HaloRenderer::new(gl),
                last_size: (0, 0),
                last_scaler_filter: None,
                last_pass_res: (0, 0),
                last_frame_size: (0, 0),
                last_frame_format: None,
                last_frame_captured_at: -1,
                history_count: 0,
            }
        }
    }

    /// Private helper to decode a raw YUV frame into an RGB texture and optionally apply the FFT filter.
    /// Returns the texture containing the result (usually self.pass_textures[6] or the FFT output).
    #[allow(clippy::too_many_arguments)]
    unsafe fn prepare_input_texture(
        &mut self,
        gl: &glow::Context,
        frame: &RawFrame,
        target_width: u32,
        target_height: u32,
        overscan_x: f32,
        overscan_y: f32,
        underscan_x: f32,
        underscan_y: f32,
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
    ) -> glow::Texture {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[6]));
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
            let chroma_height = if frame.format == Pixel::YUV422P || frame.format == Pixel::YUVJ422P
            {
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
                return self.pass_textures[6];
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
            if needs_realloc {
                gl.buffer_data_size(
                    glow::PIXEL_UNPACK_BUFFER,
                    y_data.len() as i32,
                    glow::STREAM_DRAW,
                );
            }
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
            if needs_realloc {
                gl.buffer_data_size(
                    glow::PIXEL_UNPACK_BUFFER,
                    u_data.len() as i32,
                    glow::STREAM_DRAW,
                );
            }
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
            if needs_realloc {
                gl.buffer_data_size(
                    glow::PIXEL_UNPACK_BUFFER,
                    v_data.len() as i32,
                    glow::STREAM_DRAW,
                );
            }
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
                return self.pass_textures[6];
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
            if needs_realloc {
                gl.buffer_data_size(
                    glow::PIXEL_UNPACK_BUFFER,
                    frame.data.len() as i32,
                    glow::STREAM_DRAW,
                );
            }
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
            rendered_input = true;
        } else {
            tracing::warn!("Skipping unsupported pixel format: {:?}", frame.format);
        }

        if rendered_input {
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }
        let mut tex = self.pass_textures[6];

        // Apply FFT filter to raw frame data (at capture card resolution, before any scaling)
        if let Some(fft_arc) = fft_filter {
            let mut fft = fft_arc.lock().unwrap();
            tex = fft.apply(
                gl,
                tex,
                frame.width,
                frame.height,
                fft_mask_threshold,
                fft_black_threshold,
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
        }

        tex
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn update_frame_history_and_prepare_input(
        &mut self,
        gl: &glow::Context,
        raw_frame: Option<&RawFrame>,
        target_width: u32,
        target_height: u32,
        overscan_x: f32,
        overscan_y: f32,
        underscan_x: f32,
        underscan_y: f32,
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
    ) -> Option<glow::Texture> {
        let frame = raw_frame?;
        let is_new_frame = frame.captured_at != self.last_frame_captured_at;

        if is_new_frame {
            if self.history_count >= 1 {
                // Shift history: pass_textures[0] (t-1) -> pass_textures[2] (t-2)
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.fbos[0]));
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.fbos[2]));
                gl.blit_framebuffer(
                    0,
                    0,
                    target_width as i32,
                    target_height as i32,
                    0,
                    0,
                    target_width as i32,
                    target_height as i32,
                    glow::COLOR_BUFFER_BIT,
                    glow::NEAREST,
                );

                // Shift history: pass_textures[6] (t) -> pass_textures[0] (t-1)
                // pass_textures[6] currently holds the decoded frame from the previous arrival
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.fbos[6]));
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.fbos[0]));
                gl.blit_framebuffer(
                    0,
                    0,
                    target_width as i32,
                    target_height as i32,
                    0,
                    0,
                    target_width as i32,
                    target_height as i32,
                    glow::COLOR_BUFFER_BIT,
                    glow::NEAREST,
                );
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            }
        }

        let tex = self.prepare_input_texture(
            gl,
            frame,
            target_width,
            target_height,
            overscan_x,
            overscan_y,
            underscan_x,
            underscan_y,
            fft_filter,
            fft_mask_threshold,
            fft_black_threshold,
        );

        if is_new_frame {
            if self.history_count == 0 {
                // First frame ever: seed history 1 with the initial frame so it's not uninitialized
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.fbos[6]));
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.fbos[0]));
                gl.blit_framebuffer(
                    0,
                    0,
                    target_width as i32,
                    target_height as i32,
                    0,
                    0,
                    target_width as i32,
                    target_height as i32,
                    glow::COLOR_BUFFER_BIT,
                    glow::NEAREST,
                );
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            }
            self.last_frame_captured_at = frame.captured_at;
            self.history_count = self.history_count.saturating_add(1);
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
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
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
        cathode_params: &CathodeInterferenceShaderParams,
        glass_params: Option<&GlassShaderParams>,
        time: f32,
        run_pixelate: bool,
        run_lottes: bool,
        run_halo: bool,
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
        software_mouse_pos: Option<(f32, f32)>,
        software_mouse_clip: Option<[i32; 4]>,
    ) -> RenderedArea {
        let mut video_texture = fallback_texture;

        let scaler = ScalerFilter::from_u8(params.scaler_filter);
        let is_anime4k = matches!(
            scaler,
            ScalerFilter::Anime4kSmall | ScalerFilter::Anime4kMedium | ScalerFilter::Anime4kLarge
        );

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
        } else {
            resolution
        };

        if self.last_size != resolution
            || self.last_scaler_filter != Some(params.scaler_filter)
            || self.last_pass_res != effective_res
        {
            self.setup_framebuffers(
                gl,
                resolution.0,
                resolution.1,
                output_size.0 as u32,
                output_size.1 as u32,
                params.scaler_filter,
            );
            self.last_size = resolution;
            self.last_scaler_filter = Some(params.scaler_filter);
            self.last_pass_res = effective_res;
        }

        unsafe {
            let old_vbo = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
            let scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);
            gl.disable(glow::SCISSOR_TEST);
            let blend_enabled = gl.is_enabled(glow::BLEND);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(Some(self.vertex_array));
            gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);

            if let Some(tex) = self.update_frame_history_and_prepare_input(
                gl,
                raw_frame,
                resolution.0,
                resolution.1,
                params.overscan_x,
                params.overscan_y,
                params.underscan_x,
                params.underscan_y,
                fft_filter,
                fft_mask_threshold,
                fft_black_threshold,
            ) {
                video_texture = Some(tex);
            }

            let input_texture = video_texture.expect("No video texture available");
            let mut current_video_texture = input_texture;
            let mut current_res = resolution;

            // Apply Deinterlace filter before median / upscaling
            if params.deinterlace_filter_enabled {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[1]));
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
                    gl.uniform_1_i32(Some(loc), if self.history_count >= 2 { 1 } else { 0 });
                }
                gl.uniform_1_i32(Some(&self.deinterlace_mode_loc), params.deinterlace_mode as i32);
                gl.uniform_1_f32(Some(&self.deinterlace_blend_loc), params.deinterlace_blend);
                gl.uniform_1_f32(
                    Some(&self.deinterlace_motion_thresh_loc),
                    params.deinterlace_motion_threshold,
                );
                gl.uniform_1_f32(
                    Some(&self.deinterlace_line_spacing_loc),
                    params.deinterlace_line_spacing,
                );
                gl.uniform_1_f32(
                    Some(&self.deinterlace_spatial_mix_loc),
                    params.deinterlace_spatial_mix,
                );

                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(current_video_texture));
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[0]));
                gl.active_texture(glow::TEXTURE2);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[2]));

                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

                gl.active_texture(glow::TEXTURE2);
                gl.bind_texture(glow::TEXTURE_2D, None);
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, None);
                gl.active_texture(glow::TEXTURE0);

                current_video_texture = self.pass_textures[1];
            }

            // Apply Median filter before upscaling
            if params.median_filter_enabled {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[5]));
                gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);
                gl.use_program(Some(self.median_prog));
                gl.uniform_1_f32(Some(&self.median_mix_loc), params.median_mix);
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(current_video_texture));
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                current_video_texture = self.pass_textures[5];
            }

            let scaler = ScalerFilter::from_u8(params.scaler_filter);
            let is_anime4k = matches!(
                scaler,
                ScalerFilter::Anime4kSmall
                    | ScalerFilter::Anime4kMedium
                    | ScalerFilter::Anime4kLarge
            );

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
            }

            let mut final_input_texture = current_video_texture;
            let mut final_input_res = current_res;

            if run_pixelate {
                let pixelate_res = pixelate_subrender_size(current_res.0, current_res.1);
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[4]));
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
                final_input_texture = self.pass_textures[4];
                final_input_res = pixelate_res;
            }
            self.last_video_texture = Some(final_input_texture);

            let run_cathode = cathode_params.enabled && cathode_params.intensity > 0.001;
            let run_glass = glass_params
                .map(|g| g.enabled && g.intensity > 0.001)
                .unwrap_or(false);
            let has_popup = !self.popup_primitives.is_empty();
            let has_mouse = software_mouse_pos.is_some();
            let has_post = run_cathode || run_glass || has_popup || has_mouse;

            let (upstream_target_fbo, cathode_target_fbo, glass_target_fbo) =
                match (run_cathode, run_glass) {
                    (true, true) => {
                        self.setup_post_framebuffer(gl, output_size.0 as u32, output_size.1 as u32);
                        (Some(self.post_fbos[0]), Some(self.post_fbos[1]), None)
                    }
                    (true, false) => {
                        self.setup_post_framebuffer(gl, output_size.0 as u32, output_size.1 as u32);
                        (Some(self.post_fbos[0]), None, None)
                    }
                    (false, true) => {
                        self.setup_post_framebuffer(gl, output_size.0 as u32, output_size.1 as u32);
                        (Some(self.post_fbos[0]), None, None)
                    }
                    (false, false) => {
                        if has_popup || has_mouse {
                            self.setup_post_framebuffer(gl, output_size.0 as u32, output_size.1 as u32);
                            (Some(self.post_fbos[0]), None, None)
                        } else {
                            (None, None, None)
                        }
                    }
                };

            if run_lottes {
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
                if scissor_enabled && !has_post {
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
                    scissor_enabled && !has_post,
                    upstream_target_fbo,
                );
            } else if run_pixelate {
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
                if scissor_enabled && !has_post {
                    gl.enable(glow::SCISSOR_TEST);
                }
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            } else {
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
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
                );
            }

            if has_popup {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.post_fbos[0]));
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
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
                } else {
                    (0.0, 0.0, 1.0)
                };
                self.draw_popup_primitives(
                    gl,
                    painter,
                    output_size,
                    widget_rect,
                    s_mask,
                    scan_str,
                    b_boost,
                );
                self.popup_primitives.clear();
            }

            if let Some(mouse_pos) = software_mouse_pos {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.post_fbos[0]));
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
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
                } else {
                    (0.0, 0.0, 1.0)
                };
                self.draw_software_mouse(
                    gl,
                    mouse_pos,
                    output_size,
                    s_mask,
                    scan_str,
                    b_boost,
                    output_size.0 / widget_rect.width(),
                    software_mouse_clip,
                );
            }

            if run_cathode {
                let cathode_input = self.post_textures[0];
                let scissor_cathode = scissor_enabled && !run_glass;
                self.draw_cathode_pass(
                    gl,
                    cathode_input,
                    output_size,
                    final_input_res,
                    params.horizontal_stretch,
                    time,
                    cathode_params,
                    cathode_target_fbo,
                    scissor_cathode,
                );
            }

            if run_glass {
                let gp = glass_params.unwrap();
                let glass_input = if run_cathode {
                    self.post_textures[1]
                } else {
                    self.post_textures[0]
                };
                let scissor_glass = scissor_enabled;
                self.draw_glass_pass(
                    gl,
                    glass_input,
                    output_size,
                    final_input_res,
                    gp,
                    time,
                    glass_target_fbo,
                    scissor_glass,
                );
            }

            let final_output_tex = if run_glass {
                if run_cathode {
                    self.post_textures[1]
                } else {
                    self.post_textures[0]
                }
            } else if run_cathode {
                self.post_textures[0]
            } else if has_popup || has_mouse {
                self.post_textures[0]
            } else {
                final_input_texture
            };
            self.last_video_texture = Some(final_output_tex);

            if !run_cathode && !run_glass && (has_popup || has_mouse) {
                self.blit_texture(gl, self.post_textures[0], output_size, None, scissor_enabled);
            }

            if scissor_enabled {
                gl.enable(glow::SCISSOR_TEST);
            } else {
                gl.disable(glow::SCISSOR_TEST);
            }

            if blend_enabled {
                gl.enable(glow::BLEND);
            } else {
                gl.disable(glow::BLEND);
            }

            gl.bind_vertex_array(None);
            if old_vbo != 0 {
                gl.bind_vertex_array(Some(glow::VertexArray::from(glow::NativeVertexArray(
                    NonZero::new(old_vbo as u32).unwrap(),
                ))));
            } else {
                gl.bind_vertex_array(None);
            }
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
        scaler_filter: u8,
        overscan_x: f32,
        overscan_y: f32,
        underscan_x: f32,
        underscan_y: f32,
        border_crop: [f32; 4],
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
        cathode_params: Option<&CathodeInterferenceShaderParams>,
        glass_params: Option<&GlassShaderParams>,
        time: f32,
    ) -> RenderedArea {
        let mut video_texture = fallback_texture;

        let scaler = ScalerFilter::from_u8(scaler_filter);
        let is_anime4k = matches!(
            scaler,
            ScalerFilter::Anime4kSmall | ScalerFilter::Anime4kMedium | ScalerFilter::Anime4kLarge
        );

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
        }

        unsafe {
            let old_vbo = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
            let scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);
            gl.disable(glow::SCISSOR_TEST);
            let blend_enabled = gl.is_enabled(glow::BLEND);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(Some(self.vertex_array));

            if let Some(tex) = self.update_frame_history_and_prepare_input(
                gl,
                raw_frame,
                resolution.0,
                resolution.1,
                overscan_x,
                overscan_y,
                underscan_x,
                underscan_y,
                fft_filter,
                fft_mask_threshold,
                fft_black_threshold,
            ) {
                video_texture = Some(tex);
            }

            let input_texture = video_texture.expect("No video texture available");
            let mut current_video_texture = input_texture;
            let mut current_res = resolution;

            // Apply Deinterlace filter before median / upscaling
            if deinterlace_filter_enabled {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[1]));
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
                    gl.uniform_1_i32(Some(loc), if self.history_count >= 2 { 1 } else { 0 });
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
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[0]));
                gl.active_texture(glow::TEXTURE2);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[2]));

                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

                gl.active_texture(glow::TEXTURE2);
                gl.bind_texture(glow::TEXTURE_2D, None);
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, None);
                gl.active_texture(glow::TEXTURE0);

                current_video_texture = self.pass_textures[1];
            }

            // Apply Median filter before upscaling
            if median_filter_enabled {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[5]));
                gl.viewport(0, 0, current_res.0 as i32, current_res.1 as i32);
                gl.use_program(Some(self.median_prog));
                gl.uniform_1_f32(Some(&self.median_mix_loc), median_mix);
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(current_video_texture));
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                current_video_texture = self.pass_textures[5];
            }

            let scaler = ScalerFilter::from_u8(scaler_filter);
            let is_anime4k = matches!(
                scaler,
                ScalerFilter::Anime4kSmall
                    | ScalerFilter::Anime4kMedium
                    | ScalerFilter::Anime4kLarge
            );

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
            }
            self.last_video_texture = Some(current_video_texture);

            let run_cathode = cathode_params
                .map(|c| c.enabled && c.intensity > 0.001)
                .unwrap_or(false);
            let run_glass = glass_params
                .map(|g| g.enabled && g.intensity > 0.001)
                .unwrap_or(false);
            let has_popup = !self.popup_primitives.is_empty();
            let has_post = run_cathode || run_glass || has_popup;

            let (upstream_target_fbo, cathode_target_fbo, glass_target_fbo) =
                match (run_cathode, run_glass) {
                    (true, true) => {
                        self.setup_post_framebuffer(gl, output_size.0 as u32, output_size.1 as u32);
                        (Some(self.post_fbos[0]), Some(self.post_fbos[1]), None)
                    }
                    (true, false) => {
                        self.setup_post_framebuffer(gl, output_size.0 as u32, output_size.1 as u32);
                        (Some(self.post_fbos[0]), None, None)
                    }
                    (false, true) => {
                        self.setup_post_framebuffer(gl, output_size.0 as u32, output_size.1 as u32);
                        (Some(self.post_fbos[0]), None, None)
                    }
                    (false, false) => {
                        if has_popup {
                            self.setup_post_framebuffer(gl, output_size.0 as u32, output_size.1 as u32);
                            (Some(self.post_fbos[0]), None, None)
                        } else {
                            (None, None, None)
                        }
                    }
                };

            if scissor_enabled && !has_post {
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
            );

            if has_popup {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.post_fbos[0]));
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
                self.draw_popup_primitives(
                    gl,
                    painter,
                    output_size,
                    widget_rect,
                    0.0,
                    0.0,
                    1.0,
                );
                self.popup_primitives.clear();
            }

            if run_cathode {
                let cp = cathode_params.unwrap();
                let cathode_input = self.post_textures[0];
                let scissor_cathode = scissor_enabled && !run_glass;
                self.draw_cathode_pass(
                    gl,
                    cathode_input,
                    output_size,
                    current_res,
                    horizontal_stretch,
                    time,
                    cp,
                    cathode_target_fbo,
                    scissor_cathode,
                );
            }

            if run_glass {
                let gp = glass_params.unwrap();
                let glass_input = if run_cathode {
                    self.post_textures[1]
                } else {
                    self.post_textures[0]
                };
                let scissor_glass = scissor_enabled;
                self.draw_glass_pass(
                    gl,
                    glass_input,
                    output_size,
                    current_res,
                    gp,
                    time,
                    glass_target_fbo,
                    scissor_glass,
                );
            }

            let final_output_tex = if run_glass {
                if run_cathode {
                    self.post_textures[1]
                } else {
                    self.post_textures[0]
                }
            } else if run_cathode {
                self.post_textures[0]
            } else if has_popup {
                self.post_textures[0]
            } else {
                current_video_texture
            };
            self.last_video_texture = Some(final_output_tex);

            if !run_cathode && !run_glass && has_popup {
                self.blit_texture(gl, self.post_textures[0], output_size, None, scissor_enabled);
            }

            if scissor_enabled {
                gl.enable(glow::SCISSOR_TEST);
            } else {
                gl.disable(glow::SCISSOR_TEST);
            }

            if blend_enabled {
                gl.enable(glow::BLEND);
            } else {
                gl.disable(glow::BLEND);
            }

            if old_vbo != 0 {
                gl.bind_vertex_array(Some(glow::VertexArray::from(glow::NativeVertexArray(
                    NonZero::new(old_vbo as u32).unwrap(),
                ))));
            } else {
                gl.bind_vertex_array(None);
            }
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
    ) -> RenderedArea {
        let final_input_texture = video_texture;

        gl.bind_framebuffer(glow::FRAMEBUFFER, target_fbo);
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
        gl.uniform_1_f32(Some(&self.cathode_electricity_glow_loc), cp.electricity_glow);
        gl.uniform_1_f32(Some(&self.cathode_flicker_depth_loc), cp.flicker_depth);
        gl.uniform_1_f32(Some(&self.cathode_interference_loc), cp.interference);
        gl.uniform_1_f32(Some(&self.cathode_lightbulb_effect_loc), cp.lightbulb_effect);
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
        gl.uniform_1_f32(
            self.glass_intensity_loc.as_ref(),
            glass_params.intensity,
        );
        gl.uniform_1_f32(
            self.glass_glossiness_loc.as_ref(),
            glass_params.glossiness,
        );
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.silhouette_texture));

        gl.uniform_1_f32(
            self.glass_time_loc.as_ref(),
            time,
        );
        gl.uniform_1_i32(
            self.glass_ceiling_light_enabled_loc.as_ref(),
            if glass_params.ceiling_light_enabled { 1 } else { 0 },
        );
        gl.uniform_1_i32(
            self.glass_photographer_enabled_loc.as_ref(),
            if glass_params.photographer_enabled { 1 } else { 0 },
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

    pub fn draw_popup_primitives(
        &self,
        gl: &glow::Context,
        painter: &egui_glow::Painter,
        output_size: (f32, f32),
        widget_rect: egui::Rect,
        shadow_mask: f32,
        scanline_strength: f32,
        brightboost: f32,
    ) {
        if self.popup_primitives.is_empty() || widget_rect.width() <= 0.0 || widget_rect.height() <= 0.0 {
            return;
        }

        unsafe {
            let mut prev_scissor = [0i32; 4];
            gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut prev_scissor);
            let prev_scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);
            let prev_blend_enabled = gl.is_enabled(glow::BLEND);
            let prev_src_rgb = gl.get_parameter_i32(glow::BLEND_SRC_RGB) as u32;
            let prev_dst_rgb = gl.get_parameter_i32(glow::BLEND_DST_RGB) as u32;
            let prev_src_alpha = gl.get_parameter_i32(glow::BLEND_SRC_ALPHA) as u32;
            let prev_dst_alpha = gl.get_parameter_i32(glow::BLEND_DST_ALPHA) as u32;
            let prev_vao = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);

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
            gl.uniform_1_f32(
                self.popup_scanline_strength_loc.as_ref(),
                scanline_strength,
            );
            let scanline_freq = (output_size.1 / 240.0).max(2.0);
            gl.uniform_1_f32(self.popup_scanline_freq_loc.as_ref(), scanline_freq);
            gl.uniform_1_f32(self.popup_brightboost_loc.as_ref(), brightboost);

            gl.bind_vertex_array(Some(self.popup_vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.popup_vbo));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.popup_ebo));

            let stride = std::mem::size_of::<egui::epaint::Vertex>() as i32;
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 8);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(2, 4, glow::UNSIGNED_BYTE, false, stride, 16);
            gl.enable_vertex_attrib_array(2);

            let ppp = output_size.0 / widget_rect.width();

            for primitive in &self.popup_primitives {
                if let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive {
                    if mesh.vertices.is_empty() || mesh.indices.is_empty() {
                        continue;
                    }

                    let clip_rect = primitive.clip_rect;
                    let clip_min_x = ((clip_rect.min.x - widget_rect.min.x) * ppp).round() as i32;
                    let clip_min_y = ((clip_rect.min.y - widget_rect.min.y) * ppp).round() as i32;
                    let clip_max_x = ((clip_rect.max.x - widget_rect.min.x) * ppp).round() as i32;
                    let clip_max_y = ((clip_rect.max.y - widget_rect.min.y) * ppp).round() as i32;

                    let clip_min_x = clip_min_x.clamp(0, output_size.0 as i32);
                    let clip_min_y = clip_min_y.clamp(0, output_size.1 as i32);
                    let clip_max_x = clip_max_x.clamp(clip_min_x, output_size.0 as i32);
                    let clip_max_y = clip_max_y.clamp(clip_min_y, output_size.1 as i32);

                    let scissor_w = clip_max_x - clip_min_x;
                    let scissor_h = clip_max_y - clip_min_y;
                    if scissor_w <= 0 || scissor_h <= 0 {
                        continue;
                    }

                    let scissor_y = output_size.1 as i32 - clip_max_y;
                    gl.scissor(clip_min_x, scissor_y, scissor_w, scissor_h);

                    if let Some(texture) = painter.texture(mesh.texture_id) {
                        gl.active_texture(glow::TEXTURE0);
                        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    }

                    gl.buffer_data_u8_slice(
                        glow::ARRAY_BUFFER,
                        bytemuck::cast_slice(&mesh.vertices),
                        glow::STREAM_DRAW,
                    );

                    gl.buffer_data_u8_slice(
                        glow::ELEMENT_ARRAY_BUFFER,
                        bytemuck::cast_slice(&mesh.indices),
                        glow::STREAM_DRAW,
                    );

                    gl.draw_elements(
                        glow::TRIANGLES,
                        mesh.indices.len() as i32,
                        glow::UNSIGNED_INT,
                        0,
                    );
                }
            }

            gl.bind_vertex_array(None);
            if prev_vao != 0 {
                gl.bind_vertex_array(Some(glow::VertexArray::from(glow::NativeVertexArray(
                    NonZero::new(prev_vao as u32).unwrap(),
                ))));
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);

            gl.scissor(
                prev_scissor[0],
                prev_scissor[1],
                prev_scissor[2],
                prev_scissor[3],
            );
            if prev_scissor_enabled {
                gl.enable(glow::SCISSOR_TEST);
            } else {
                gl.disable(glow::SCISSOR_TEST);
            }

            gl.blend_func_separate(prev_src_rgb, prev_dst_rgb, prev_src_alpha, prev_dst_alpha);
            if prev_blend_enabled {
                gl.enable(glow::BLEND);
            } else {
                gl.disable(glow::BLEND);
            }
        }
    }

    unsafe fn blit_texture(
        &self,
        gl: &glow::Context,
        texture: glow::Texture,
        output_size: (f32, f32),
        target_fbo: Option<glow::Framebuffer>,
        scissor_enabled: bool,
    ) {
        gl.bind_framebuffer(glow::FRAMEBUFFER, target_fbo);
        gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
        gl.use_program(Some(self.blit_prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));

        gl.bind_vertex_array(Some(self.vertex_array));
        if scissor_enabled {
            gl.enable(glow::SCISSOR_TEST);
        } else {
            gl.disable(glow::SCISSOR_TEST);
        }
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.bind_vertex_array(None);
        gl.use_program(None);
    }

    #[allow(clippy::too_many_arguments)]
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
            let mut prev_scissor = [0i32; 4];
            gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut prev_scissor);
            let prev_scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);

            if let Some([cx, cy, cw, ch]) = clip_rect {
                gl.enable(glow::SCISSOR_TEST);
                gl.scissor(cx, cy, cw, ch);
            } else {
                gl.disable(glow::SCISSOR_TEST);
            }

            let prev_blend = gl.is_enabled(glow::BLEND);
            let prev_src_rgb = gl.get_parameter_i32(glow::BLEND_SRC_RGB) as u32;
            let prev_dst_rgb = gl.get_parameter_i32(glow::BLEND_DST_RGB) as u32;
            let prev_src_alpha = gl.get_parameter_i32(glow::BLEND_SRC_ALPHA) as u32;
            let prev_dst_alpha = gl.get_parameter_i32(glow::BLEND_DST_ALPHA) as u32;
            let prev_vao = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);

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

            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
            gl.blend_func_separate(prev_src_rgb, prev_dst_rgb, prev_src_alpha, prev_dst_alpha);
            if !prev_blend {
                gl.disable(glow::BLEND);
            }
            gl.scissor(
                prev_scissor[0],
                prev_scissor[1],
                prev_scissor[2],
                prev_scissor[3],
            );
            if prev_scissor_enabled {
                gl.enable(glow::SCISSOR_TEST);
            } else {
                gl.disable(glow::SCISSOR_TEST);
            }
            if prev_vao != 0 {
                gl.bind_vertex_array(Some(glow::VertexArray::from(glow::NativeVertexArray(
                    NonZero::new(prev_vao as u32).unwrap(),
                ))));
            } else {
                gl.bind_vertex_array(None);
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
            let old_vbo = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
            let blend_enabled = gl.is_enabled(glow::BLEND);
            let prev_src_rgb = gl.get_parameter_i32(glow::BLEND_SRC_RGB) as u32;
            let prev_dst_rgb = gl.get_parameter_i32(glow::BLEND_DST_RGB) as u32;
            let prev_src_alpha = gl.get_parameter_i32(glow::BLEND_SRC_ALPHA) as u32;
            let prev_dst_alpha = gl.get_parameter_i32(glow::BLEND_DST_ALPHA) as u32;
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
            let video_tex = self.last_video_texture.unwrap_or(self.pass_textures[6]);
            gl.bind_texture(glow::TEXTURE_2D, Some(video_tex));
            gl.generate_mipmap(glow::TEXTURE_2D);
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR_MIPMAP_LINEAR as i32,
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
            gl.uniform_2_f32(
                self.retro_warp_loc.as_ref(),
                warp[0],
                warp[1],
            );
            gl.uniform_1_f32(
                self.retro_corner_size_loc.as_ref(),
                corner_size,
            );
            gl.uniform_1_i32(
                self.retro_filter_type_loc.as_ref(),
                filter_type,
            );
            gl.uniform_1_f32(
                self.retro_time_loc.as_ref(),
                time,
            );
            gl.uniform_1_f32(
                self.retro_ambient_glow_loc.as_ref(),
                ambient_glow,
            );
            gl.uniform_1_i32(
                self.retro_dark_mode_loc.as_ref(),
                if dark_mode { 1 } else { 0 },
            );

            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);

            gl.blend_func_separate(prev_src_rgb, prev_dst_rgb, prev_src_alpha, prev_dst_alpha);
            if !blend_enabled {
                gl.disable(glow::BLEND);
            }

            gl.bind_vertex_array(None);
            if old_vbo != 0 {
                gl.bind_vertex_array(Some(glow::VertexArray::from(glow::NativeVertexArray(
                    NonZero::new(old_vbo as u32).unwrap(),
                ))));
            }
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
            let old_vbo = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
            let blend_enabled = gl.is_enabled(glow::BLEND);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE);

            gl.bind_vertex_array(Some(self.vertex_array));
            gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);

            gl.use_program(Some(self.night_glow_prog));

            gl.active_texture(glow::TEXTURE0);
            let video_tex = self.last_video_texture.unwrap_or(self.pass_textures[6]);
            gl.bind_texture(glow::TEXTURE_2D, Some(video_tex));
            gl.generate_mipmap(glow::TEXTURE_2D);
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR_MIPMAP_LINEAR as i32,
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
            gl.uniform_2_f32(
                self.night_glow_warp_loc.as_ref(),
                warp[0],
                warp[1],
            );
            gl.uniform_1_f32(
                self.night_glow_corner_size_loc.as_ref(),
                corner_size,
            );
            gl.uniform_1_i32(
                self.night_glow_filter_type_loc.as_ref(),
                filter_type,
            );
            gl.uniform_1_f32(
                self.night_glow_intensity_loc.as_ref(),
                glow_intensity,
            );

            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);

            if !blend_enabled {
                gl.disable(glow::BLEND);
            }

            gl.bind_vertex_array(None);
            if old_vbo != 0 {
                gl.bind_vertex_array(Some(glow::VertexArray::from(glow::NativeVertexArray(
                    NonZero::new(old_vbo as u32).unwrap(),
                ))));
            }
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
            gl.delete_program(self.crt_glass_prog);
            gl.delete_program(self.night_glow_prog);
            gl.delete_program(self.popup_prog);
            gl.delete_program(self.blit_prog);
            gl.delete_program(self.retro_mouse_prog);
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
            self.halo.destroy(gl);
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
                let pass_filter_mode = if i == 4 {
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
            self.history_count = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_prepasses_use_high_precision_targets() {
        assert_eq!(intermediate_texture_internal_format(0), glow::RGBA16F);
        assert_eq!(intermediate_texture_internal_format(1), glow::RGBA16F);
        assert_eq!(intermediate_texture_internal_format(2), glow::RGBA16F);
        assert_eq!(intermediate_texture_internal_format(4), glow::RGBA16F);
        assert_eq!(intermediate_texture_internal_format(5), glow::RGBA16F);
        assert_eq!(intermediate_texture_internal_format(6), glow::RGBA16F);
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
            pass_texture_dimensions(0, source_size, effective_size),
            source_size
        );
        assert_eq!(
            pass_texture_dimensions(1, source_size, effective_size),
            source_size
        );
        assert_eq!(
            pass_texture_dimensions(2, source_size, effective_size),
            source_size
        );
        assert_eq!(
            pass_texture_dimensions(4, source_size, effective_size),
            (720, 480)
        );
        assert_eq!(
            pass_texture_dimensions(5, source_size, effective_size),
            source_size
        );
        assert_eq!(
            pass_texture_dimensions(6, source_size, effective_size),
            source_size
        );
        assert_eq!(
            pass_texture_dimensions(3, source_size, effective_size),
            effective_size
        );
    }

    #[test]
    fn other_intermediate_passes_keep_existing_format() {
        for pass_index in 3..4 {
            assert_eq!(
                intermediate_texture_internal_format(pass_index),
                glow::RGBA8
            );
        }
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
        assert_eq!(parity_diff, 0.0, "Same-parity difference must be zero on static bobbing edges");

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
        // Simulates arrival of Frame 1, multiple repeat paints, Frame 2, multiple repeat paints
        // and proves that history buffers retain t-1 and t-2 without collapsing on repeat paints.
        let mut history_count = 0u32;
        let mut last_captured_at = 0u64;

        let mut buf_curr = 0u32;
        let mut buf_prev1 = 0u32;
        let mut buf_prev2 = 0u32;

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
        ];

        for (step, (pts, frame_id)) in arrivals.iter().enumerate() {
            let is_new_frame = *pts != last_captured_at;
            if is_new_frame && history_count >= 1 {
                buf_prev2 = buf_prev1;
                buf_prev1 = buf_curr;
            }
            buf_curr = *frame_id;
            if is_new_frame {
                if history_count == 0 {
                    buf_prev1 = buf_curr;
                }
                last_captured_at = *pts;
                history_count += 1;
            }

            if step >= 6 {
                // Steps 6, 7, 8 are Frame 3 (new + 2 repeat paints)
                assert_eq!(buf_curr, 3, "Current frame must remain Frame 3");
                assert_eq!(buf_prev1, 2, "Previous frame must remain Frame 2 on both new and repeat paints");
                assert_eq!(buf_prev2, 1, "Previous-2 frame must remain Frame 1 on both new and repeat paints");
                assert_eq!(history_count, 3);
            }
        }
    }
}
