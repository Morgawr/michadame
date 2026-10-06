use super::anime4k::{Anime4kUpscaler, Anime4kVariant};
use super::fft_filter::FftFilter;
use super::geometry::RenderedArea;
use super::halo::HaloRenderer;
use super::params::{CathodeInterferenceShaderParams, HaloShaderParams, ShaderParams};
use super::programs::*;
use crate::video::types::{RawFrame, ScalerFilter};
use eframe::glow::{self, HasContext};
use ffmpeg_next::format::Pixel;
use std::num::NonZero;
use std::sync::{Arc, Mutex};

const PIXELATE_TARGET_HEIGHT: u32 = 480;

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
    median_mix_loc: glow::UniformLocation,
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

    post_fbo: glow::Framebuffer,
    post_texture: glow::Texture,
    last_post_size: (u32, u32),

    last_size: (u32, u32),
    last_scaler_filter: Option<u8>,
    last_pass_res: (u32, u32),
    last_frame_size: (u32, u32),
    last_frame_format: Option<Pixel>,
}

fn intermediate_texture_internal_format(pass_index: usize) -> u32 {
    match pass_index {
        // These passes store linear RGB that is later sampled by another shader before
        // the final linear->sRGB presentation step, so RGBA8 causes visible
        // dark-area quantization. Keep them in half-float instead.
        4..=6 => glow::RGBA16F,
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
        // Median and YUV conversion happen before any upscaling.
        5 | 6 => source_size,
        _ => effective_size,
    }
}

impl CrtFilterRenderer {
    pub fn new(gl: &glow::Context) -> Self {
        unsafe {
            let passthrough_prog = compile_program(gl, VS_SRC, FS_PASSTHROUGH);
            let pixelate_prog = compile_program(gl, VS_SRC, FS_PIXELATE);
            let median_prog = compile_program(gl, VS_SRC, FS_MEDIAN_3X1);
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

            gl.use_program(Some(retro_frame_prog));
            if let Some(loc) = gl.get_uniform_location(retro_frame_prog, "bezel_texture") {
                gl.uniform_1_i32(Some(&loc), 0);
            }
            gl.use_program(None);

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

            gl.use_program(Some(yuv_planar_prog));
            let yuv_range_loc = gl
                .get_uniform_location(yuv_planar_prog, "input_range")
                .unwrap();
            let yuv_overscan_loc = gl
                .get_uniform_location(yuv_planar_prog, "overscan_offset")
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

            let post_fbo = gl.create_framebuffer().unwrap();
            let post_texture = gl.create_texture().unwrap();

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
                pixelate_prog,
                final_prog,
                yuv_planar_prog,
                yuyv_packed_prog,
                yuv_range_loc,
                yuyv_range_loc,
                yuv_overscan_loc,
                yuyv_overscan_loc,
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
                post_fbo,
                post_texture,
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

    unsafe fn setup_post_framebuffer(
        &mut self,
        gl: &glow::Context,
        target_width: u32,
        target_height: u32,
    ) {
        let w = target_width.max(1);
        let h = target_height.max(1);
        if (w, h) != self.last_post_size {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.post_texture));
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
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.post_fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(self.post_texture),
                0,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.last_post_size = (w, h);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        gl: &glow::Context,
        raw_frame: Option<&RawFrame>,
        fallback_texture: Option<glow::Texture>,
        resolution: (u32, u32),
        output_size: (f32, f32),
        params: &ShaderParams,
        halo_params: &HaloShaderParams,
        cathode_params: &CathodeInterferenceShaderParams,
        time: f32,
        run_pixelate: bool,
        run_lottes: bool,
        run_halo: bool,
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
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

            if let Some(frame) = raw_frame {
                video_texture = Some(self.prepare_input_texture(
                    gl,
                    frame,
                    resolution.0,
                    resolution.1,
                    params.overscan_x,
                    params.overscan_y,
                    fft_filter,
                    fft_mask_threshold,
                    fft_black_threshold,
                ));
                gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);
            }

            let input_texture = video_texture.expect("No video texture available");
            let mut current_video_texture = input_texture;
            let mut current_res = resolution;

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

            let run_cathode = cathode_params.enabled && cathode_params.intensity > 0.001;
            let target_fbo = if run_cathode {
                self.setup_post_framebuffer(gl, output_size.0 as u32, output_size.1 as u32);
                Some(self.post_fbo)
            } else {
                None
            };

            if run_lottes {
                gl.bind_framebuffer(glow::FRAMEBUFFER, target_fbo);
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
                if scissor_enabled && !run_cathode {
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
                    scissor_enabled && !run_cathode,
                    target_fbo,
                );
            } else if run_pixelate {
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
                if scissor_enabled && !run_cathode {
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
                    target_fbo,
                );
            }

            if run_cathode {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
                gl.use_program(Some(self.cathode_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.post_texture));

                gl.uniform_2_f32(
                    Some(&self.cathode_output_res_loc),
                    output_size.0,
                    output_size.1,
                );
                gl.uniform_2_f32(
                    Some(&self.cathode_source_size_loc),
                    final_input_res.0 as f32,
                    final_input_res.1 as f32,
                );
                gl.uniform_1_f32(
                    Some(&self.cathode_horizontal_stretch_loc),
                    params.horizontal_stretch,
                );
                gl.uniform_1_f32(Some(&self.cathode_time_loc), time);
                gl.uniform_1_f32(Some(&self.cathode_intensity_loc), cathode_params.intensity);
                gl.uniform_1_f32(Some(&self.cathode_frequency_loc), cathode_params.frequency);
                gl.uniform_1_f32(Some(&self.cathode_randomization_loc), cathode_params.randomization);
                gl.uniform_1_f32(Some(&self.cathode_electricity_glow_loc), cathode_params.electricity_glow);
                gl.uniform_1_f32(Some(&self.cathode_flicker_depth_loc), cathode_params.flicker_depth);
                gl.uniform_1_f32(Some(&self.cathode_interference_loc), cathode_params.interference);
                gl.uniform_1_f32(Some(&self.cathode_lightbulb_effect_loc), cathode_params.lightbulb_effect);
                gl.uniform_4_f32(
                    Some(&self.cathode_border_crop_loc),
                    cathode_params.border_crop[0],
                    cathode_params.border_crop[1],
                    cathode_params.border_crop[2],
                    cathode_params.border_crop[3],
                );

                if scissor_enabled {
                    gl.enable(glow::SCISSOR_TEST);
                }
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                gl.bind_texture(glow::TEXTURE_2D, None);
                gl.use_program(None);
            }

            if blend_enabled {
                gl.enable(glow::BLEND);
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
        raw_frame: Option<&RawFrame>,
        fallback_texture: Option<glow::Texture>,
        resolution: (u32, u32),
        output_size: (f32, f32),
        background_color: [f32; 3],
        horizontal_stretch: f32,
        median_filter_enabled: bool,
        median_mix: f32,
        vibrance: f32,
        scaler_filter: u8,
        overscan_x: f32,
        overscan_y: f32,
        border_crop: [f32; 4],
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
        cathode_params: Option<&CathodeInterferenceShaderParams>,
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

            if let Some(frame) = raw_frame {
                video_texture = Some(self.prepare_input_texture(
                    gl,
                    frame,
                    resolution.0,
                    resolution.1,
                    overscan_x,
                    overscan_y,
                    fft_filter,
                    fft_mask_threshold,
                    fft_black_threshold,
                ));
                gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);
            }

            let input_texture = video_texture.expect("No video texture available");
            let mut current_video_texture = input_texture;
            let mut current_res = resolution;

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

            let run_cathode = cathode_params
                .map(|c| c.enabled && c.intensity > 0.001)
                .unwrap_or(false);
            let target_fbo = if run_cathode {
                self.setup_post_framebuffer(gl, output_size.0 as u32, output_size.1 as u32);
                Some(self.post_fbo)
            } else {
                None
            };

            if scissor_enabled && !run_cathode {
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
                target_fbo,
            );

            if run_cathode {
                let cp = cathode_params.unwrap();
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
                gl.use_program(Some(self.cathode_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.post_texture));

                gl.uniform_2_f32(
                    Some(&self.cathode_output_res_loc),
                    output_size.0,
                    output_size.1,
                );
                gl.uniform_2_f32(
                    Some(&self.cathode_source_size_loc),
                    current_res.0 as f32,
                    current_res.1 as f32,
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

            if blend_enabled {
                gl.enable(glow::BLEND);
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
    ) {
        unsafe {
            let old_vbo = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
            let blend_enabled = gl.is_enabled(glow::BLEND);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

            gl.bind_vertex_array(Some(self.vertex_array));
            gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);

            gl.use_program(Some(self.retro_frame_prog));

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.bezel_texture));

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

            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

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
            gl.delete_program(self.pixelate_prog);
            gl.delete_program(self.median_prog);
            gl.delete_program(self.final_prog);
            gl.delete_program(self.yuv_planar_prog);
            gl.delete_program(self.yuyv_packed_prog);
            gl.delete_program(self.cathode_prog);
            gl.delete_framebuffer(self.post_fbo);
            gl.delete_texture(self.post_texture);
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_prepasses_use_high_precision_targets() {
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
            pass_texture_dimensions(0, source_size, effective_size),
            effective_size
        );
    }

    #[test]
    fn other_intermediate_passes_keep_existing_format() {
        for pass_index in 0..4 {
            assert_eq!(
                intermediate_texture_internal_format(pass_index),
                glow::RGBA8
            );
        }
    }
}
