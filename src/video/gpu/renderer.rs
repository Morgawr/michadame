use eframe::glow::{self, HasContext};
use crate::video::types::RawFrame;
use ffmpeg_next::format::Pixel;
use std::num::NonZero;
use std::sync::{Arc, Mutex};
use super::params::ShaderParams;
use super::programs::*;
use super::fft_filter::FftFilter;

pub struct CrtFilterRenderer {
    passthrough_prog: glow::Program,
    pixelate_prog: glow::Program,
    pass0_prog: glow::Program,
    pass1_prog: glow::Program,
    pass2_prog: glow::Program,
    pass3_prog: glow::Program,
    median_prog: glow::Program,
    final_prog: glow::Program,
    yuv_planar_prog: glow::Program,
    yuyv_packed_prog: glow::Program,
    yuv_range_loc: glow::UniformLocation,
    yuyv_range_loc: glow::UniformLocation,
    yuv_overscan_loc: glow::UniformLocation,
    yuyv_overscan_loc: glow::UniformLocation,
    median_mix_loc: glow::UniformLocation,

    fbos: [glow::Framebuffer; 7],
    pass_textures: [glow::Texture; 7],
    yuv_planes: [glow::Texture; 3],
    pbos: [glow::Buffer; 3],
    vertex_array: glow::VertexArray,
    vbo: glow::Buffer,

    p_passthrough_video_res_loc: glow::UniformLocation,
    p_passthrough_output_res_loc: glow::UniformLocation,
    p_pixelate_target_res_loc: glow::UniformLocation,
    p0_hard_bloom_pix_loc: glow::UniformLocation,
    p1_hard_bloom_scan_loc: glow::UniformLocation,
    p2_hard_pix_loc: glow::UniformLocation,
    p3_hard_scan_loc: glow::UniformLocation,
    p3_shape_loc: glow::UniformLocation,

    final_video_res_loc: glow::UniformLocation,
    final_output_res_loc: glow::UniformLocation,
    final_warp_x_loc: glow::UniformLocation,
    final_warp_y_loc: glow::UniformLocation,
    final_shadow_mask_loc: glow::UniformLocation,
    final_brightboost_loc: glow::UniformLocation,
    final_bloom_amount_loc: glow::UniformLocation,
    final_background_color_loc: glow::UniformLocation,
    passthrough_background_color_loc: glow::UniformLocation,
    final_horizontal_stretch_loc: glow::UniformLocation,
    passthrough_horizontal_stretch_loc: glow::UniformLocation,
    final_vibrance_loc: glow::UniformLocation,
    passthrough_vibrance_loc: glow::UniformLocation,
    passthrough_scaler_filter_loc: glow::UniformLocation,

    last_size: (u32, u32),
    last_scaler_filter: Option<u8>,
    last_frame_size: (u32, u32),
    last_frame_format: Option<Pixel>,
}

impl CrtFilterRenderer {
    pub fn new(gl: &glow::Context) -> Self {
        unsafe {
            let passthrough_prog = compile_program(gl, VS_SRC, FS_PASSTHROUGH);
            let pixelate_prog = compile_program(gl, VS_SRC, FS_PIXELATE);
            let pass0_prog = compile_program(gl, VS_SRC, FS_PASS0);
            let pass1_prog = compile_program(gl, VS_SRC, FS_PASS1);
            let pass2_prog = compile_program(gl, VS_SRC, FS_PASS2);
            let pass3_prog = compile_program(gl, VS_SRC, FS_PASS3);
            let median_prog = compile_program(gl, VS_SRC, FS_MEDIAN_3X1);
            let final_prog = compile_program(gl, VS_SRC, FS_FINAL);
            let yuv_planar_prog = compile_program(gl, VS_SRC, FS_YUV_PLANAR);
            let yuyv_packed_prog = compile_program(gl, VS_SRC, FS_YUYV_PACKED);

            let p_passthrough_video_res_loc = gl.get_uniform_location(passthrough_prog, "videoResolution").unwrap();
            let p_passthrough_output_res_loc = gl.get_uniform_location(passthrough_prog, "outputResolution").unwrap();
            let passthrough_scaler_filter_loc = gl.get_uniform_location(passthrough_prog, "scaler_filter").unwrap();
            let p_pixelate_target_res_loc = gl.get_uniform_location(pixelate_prog, "target_resolution").unwrap();
            let p0_hard_bloom_pix_loc = gl.get_uniform_location(pass0_prog, "hardBloomPix").unwrap();
            let p1_hard_bloom_scan_loc = gl.get_uniform_location(pass1_prog, "hardBloomScan").unwrap();
            let p2_hard_pix_loc = gl.get_uniform_location(pass2_prog, "hardPix").unwrap();
            let p3_hard_scan_loc = gl.get_uniform_location(pass3_prog, "hardScan").unwrap();
            let p3_shape_loc = gl.get_uniform_location(pass3_prog, "shape").unwrap();
            let median_mix_loc = gl.get_uniform_location(median_prog, "mix_amount").unwrap();

            gl.use_program(Some(median_prog));
            gl.uniform_1_i32(Some(&gl.get_uniform_location(median_prog, "video_texture").unwrap()), 0);
            gl.use_program(None);

            let final_video_res_loc = gl.get_uniform_location(final_prog, "videoResolution").unwrap();
            let final_output_res_loc = gl.get_uniform_location(final_prog, "outputResolution").unwrap();
            let final_warp_x_loc = gl.get_uniform_location(final_prog, "warpX").unwrap();
            let final_warp_y_loc = gl.get_uniform_location(final_prog, "warpY").unwrap();
            let final_shadow_mask_loc = gl.get_uniform_location(final_prog, "shadowMask").unwrap();
            let final_brightboost_loc = gl.get_uniform_location(final_prog, "brightboost").unwrap();
            let final_bloom_amount_loc = gl.get_uniform_location(final_prog, "bloomAmount").unwrap();
            let final_background_color_loc = gl.get_uniform_location(final_prog, "background_color").unwrap();
            let passthrough_background_color_loc = gl.get_uniform_location(passthrough_prog, "background_color").unwrap();
            let final_horizontal_stretch_loc = gl.get_uniform_location(final_prog, "horizontal_stretch").unwrap();
            let passthrough_horizontal_stretch_loc = gl.get_uniform_location(passthrough_prog, "horizontal_stretch").unwrap();
            let final_vibrance_loc = gl.get_uniform_location(final_prog, "vibrance").unwrap();
            let passthrough_vibrance_loc = gl.get_uniform_location(passthrough_prog, "vibrance").unwrap();

            gl.use_program(Some(passthrough_prog));
            gl.uniform_1_i32(Some(&gl.get_uniform_location(passthrough_prog, "video_texture").unwrap()), 0);
            gl.use_program(Some(pixelate_prog));
            gl.uniform_1_i32(Some(&gl.get_uniform_location(pixelate_prog, "video_texture").unwrap()), 0);
            gl.use_program(Some(pass0_prog));
            gl.uniform_1_i32(Some(&gl.get_uniform_location(pass0_prog, "video_texture").unwrap()), 0);
            gl.use_program(Some(pass1_prog));
            gl.uniform_1_i32(Some(&gl.get_uniform_location(pass1_prog, "pass0_texture").unwrap()), 0);
            gl.use_program(Some(pass2_prog));
            gl.uniform_1_i32(Some(&gl.get_uniform_location(pass2_prog, "video_texture").unwrap()), 0);
            gl.use_program(Some(pass3_prog));
            gl.uniform_1_i32(Some(&gl.get_uniform_location(pass3_prog, "pass2_texture").unwrap()), 0);
            gl.use_program(Some(final_prog));
            gl.uniform_1_i32(Some(&gl.get_uniform_location(final_prog, "pass1_texture").unwrap()), 0);
            gl.uniform_1_i32(Some(&gl.get_uniform_location(final_prog, "pass3_texture").unwrap()), 1);

            gl.use_program(Some(yuv_planar_prog));
            gl.uniform_1_i32(Some(&gl.get_uniform_location(yuv_planar_prog, "y_tex").unwrap()), 0);
            gl.uniform_1_i32(Some(&gl.get_uniform_location(yuv_planar_prog, "u_tex").unwrap()), 1);
            gl.uniform_1_i32(Some(&gl.get_uniform_location(yuv_planar_prog, "v_tex").unwrap()), 2);

            gl.use_program(Some(yuyv_packed_prog));
            gl.uniform_1_i32(Some(&gl.get_uniform_location(yuyv_packed_prog, "raw_tex").unwrap()), 0);
            let yuyv_range_loc = gl.get_uniform_location(yuyv_packed_prog, "input_range").unwrap();
            let yuyv_overscan_loc = gl.get_uniform_location(yuyv_packed_prog, "overscan_offset").unwrap();

            gl.use_program(Some(yuv_planar_prog));
            let yuv_range_loc = gl.get_uniform_location(yuv_planar_prog, "input_range").unwrap();
            let yuv_overscan_loc = gl.get_uniform_location(yuv_planar_prog, "overscan_offset").unwrap();

            gl.use_program(None);

            let fbos = [
                gl.create_framebuffer().unwrap(), gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(), gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(), gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
            ];
            let pass_textures = [
                gl.create_texture().unwrap(), gl.create_texture().unwrap(),
                gl.create_texture().unwrap(), gl.create_texture().unwrap(),
                gl.create_texture().unwrap(), gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
            ];
            let yuv_planes = [
                gl.create_texture().unwrap(), gl.create_texture().unwrap(), gl.create_texture().unwrap(),
            ];
            let pbos = [
                gl.create_buffer().unwrap(), gl.create_buffer().unwrap(), gl.create_buffer().unwrap(),
            ];

            let vertex_array = gl.create_vertex_array().expect("Cannot create vertex array");
            let vertices: [f32; 16] = [
                -1.0, -1.0, 0.0, 0.0,
                1.0, -1.0, 1.0, 0.0,
                -1.0, 1.0, 0.0, 1.0,
                1.0, 1.0, 1.0, 1.0,
            ];
            let vbo = gl.create_buffer().unwrap();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck::cast_slice(&vertices), glow::STATIC_DRAW);

            gl.bind_vertex_array(Some(vertex_array));
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 4 * std::mem::size_of::<f32>() as i32, 0);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 4 * std::mem::size_of::<f32>() as i32, (2 * std::mem::size_of::<f32>()) as i32);
            gl.enable_vertex_attrib_array(1);

            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_vertex_array(None);

            Self {
                passthrough_prog, pixelate_prog, pass0_prog, pass1_prog, pass2_prog, pass3_prog,
                final_prog, yuv_planar_prog, yuyv_packed_prog, yuv_range_loc, yuyv_range_loc,
                yuv_overscan_loc, yuyv_overscan_loc, fbos, pass_textures, yuv_planes, pbos,
                vertex_array, vbo, p_passthrough_video_res_loc, p_passthrough_output_res_loc,
                p_pixelate_target_res_loc, p0_hard_bloom_pix_loc, p1_hard_bloom_scan_loc,
                p2_hard_pix_loc, p3_hard_scan_loc, p3_shape_loc, final_video_res_loc,
                final_output_res_loc, final_warp_x_loc, final_warp_y_loc, final_shadow_mask_loc,
                final_brightboost_loc, final_bloom_amount_loc, final_background_color_loc,
                passthrough_background_color_loc, final_horizontal_stretch_loc,
                passthrough_horizontal_stretch_loc, final_vibrance_loc, passthrough_vibrance_loc,
                passthrough_scaler_filter_loc, median_prog, median_mix_loc, last_size: (0, 0),
                last_scaler_filter: None, last_frame_size: (0, 0), last_frame_format: None,
            }
        }
    }

    /// Private helper to decode a raw YUV frame into an RGB texture and optionally apply the FFT filter.
    /// Returns the texture containing the result (usually self.pass_textures[6] or the FFT output).
    unsafe fn prepare_input_texture(
        &mut self,
        gl: &glow::Context,
        frame: &RawFrame,
        overscan_x: f32,
        overscan_y: f32,
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
    ) -> glow::Texture {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[6]));
        gl.viewport(0, 0, frame.width as i32, frame.height as i32);
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.clear(glow::COLOR_BUFFER_BIT);

        if frame.format == Pixel::YUV422P || frame.format == Pixel::YUV420P || frame.format == Pixel::YUVJ422P || frame.format == Pixel::YUVJ420P {
            gl.use_program(Some(self.yuv_planar_prog));
            let (y_end, u_end) = if frame.format == Pixel::YUV422P || frame.format == Pixel::YUVJ422P {
                ((frame.width * frame.height) as usize, (frame.width * frame.height * 3 / 2) as usize)
            } else {
                ((frame.width * frame.height) as usize, (frame.width * frame.height * 5 / 4) as usize)
            };

            let y_data = &frame.data[0..y_end];
            let u_data = &frame.data[y_end..u_end];
            let v_data = &frame.data[u_end..];
            let chroma_width = frame.width / 2;
            let chroma_height = if frame.format == Pixel::YUV422P || frame.format == Pixel::YUVJ422P { frame.height } else { frame.height / 2 };

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[0]));
            let needs_realloc = frame.width != self.last_frame_size.0 || frame.height != self.last_frame_size.1 || Some(frame.format) != self.last_frame_format;
            if needs_realloc {
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::R8 as i32, frame.width as i32, frame.height as i32, 0, glow::RED, glow::UNSIGNED_BYTE, None);
            }
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(self.pbos[0]));
            if needs_realloc { gl.buffer_data_size(glow::PIXEL_UNPACK_BUFFER, y_data.len() as i32, glow::STREAM_DRAW); }
            gl.buffer_sub_data_u8_slice(glow::PIXEL_UNPACK_BUFFER, 0, y_data);
            gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, frame.width as i32, frame.height as i32, glow::RED, glow::UNSIGNED_BYTE, glow::PixelUnpackData::BufferOffset(0));

            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[1]));
            if needs_realloc {
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::R8 as i32, chroma_width as i32, chroma_height as i32, 0, glow::RED, glow::UNSIGNED_BYTE, None);
            }
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(self.pbos[1]));
            if needs_realloc { gl.buffer_data_size(glow::PIXEL_UNPACK_BUFFER, u_data.len() as i32, glow::STREAM_DRAW); }
            gl.buffer_sub_data_u8_slice(glow::PIXEL_UNPACK_BUFFER, 0, u_data);
            gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, chroma_width as i32, chroma_height as i32, glow::RED, glow::UNSIGNED_BYTE, glow::PixelUnpackData::BufferOffset(0));

            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[2]));
            if needs_realloc {
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::R8 as i32, chroma_width as i32, chroma_height as i32, 0, glow::RED, glow::UNSIGNED_BYTE, None);
            }
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(self.pbos[2]));
            if needs_realloc { gl.buffer_data_size(glow::PIXEL_UNPACK_BUFFER, v_data.len() as i32, glow::STREAM_DRAW); }
            gl.buffer_sub_data_u8_slice(glow::PIXEL_UNPACK_BUFFER, 0, v_data);
            gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, chroma_width as i32, chroma_height as i32, glow::RED, glow::UNSIGNED_BYTE, glow::PixelUnpackData::BufferOffset(0));

            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
            gl.uniform_1_i32(Some(&self.yuv_range_loc), frame.color_range as i32);
            gl.uniform_2_f32(Some(&self.yuv_overscan_loc), overscan_x, overscan_y);
            if needs_realloc { self.last_frame_size = (frame.width, frame.height); self.last_frame_format = Some(frame.format); }
        } else if frame.format == Pixel::YUYV422 {
            gl.use_program(Some(self.yuyv_packed_prog));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[0]));
            let needs_realloc = frame.width != self.last_frame_size.0 || frame.height != self.last_frame_size.1 || Some(frame.format) != self.last_frame_format;
            if needs_realloc {
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, (frame.width / 2) as i32, frame.height as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, None);
            }
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(self.pbos[0]));
            if needs_realloc { gl.buffer_data_size(glow::PIXEL_UNPACK_BUFFER, frame.data.len() as i32, glow::STREAM_DRAW); }
            gl.buffer_sub_data_u8_slice(glow::PIXEL_UNPACK_BUFFER, 0, &frame.data);
            gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, (frame.width / 2) as i32, frame.height as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::BufferOffset(0));
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
            if needs_realloc { self.last_frame_size = (frame.width, frame.height); self.last_frame_format = Some(frame.format); }
            gl.uniform_1_i32(Some(&self.yuyv_range_loc), frame.color_range as i32);
            gl.uniform_2_f32(Some(&self.yuyv_overscan_loc), overscan_x, overscan_y);
        }

        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        let mut tex = self.pass_textures[6];

        // Apply FFT filter to raw frame data (at capture card resolution, before any scaling)
        if let Some(fft_arc) = fft_filter {
            let mut fft = fft_arc.lock().unwrap();
            tex = fft.apply(gl, tex, frame.width, frame.height, fft_mask_threshold, fft_black_threshold);
            // Restore our VAO after FFT used its own
            gl.bind_vertex_array(Some(self.vertex_array));
        }

        tex
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
        run_pixelate: bool,
        run_lottes: bool,
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
    ) {
        let mut video_texture = fallback_texture;

        if self.last_size != resolution || self.last_scaler_filter != Some(params.scaler_filter) {
            self.setup_framebuffers(gl, resolution.0, resolution.1, params.scaler_filter);
            self.last_size = resolution;
            self.last_scaler_filter = Some(params.scaler_filter);
        }

        unsafe {
            let old_vbo = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
            gl.bind_vertex_array(Some(self.vertex_array));
            gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);

            if let Some(frame) = raw_frame {
                video_texture = Some(self.prepare_input_texture(gl, frame, params.overscan_x, params.overscan_y, fft_filter, fft_mask_threshold, fft_black_threshold));
                gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);
            }

            let input_texture = video_texture.expect("No video texture available");
            let mut lottes_input_texture = input_texture;

            if params.median_filter_enabled {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[5]));
                gl.use_program(Some(self.median_prog));
                gl.uniform_1_f32(Some(&self.median_mix_loc), params.median_mix);
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, video_texture);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                lottes_input_texture = self.pass_textures[5];
            }

            let mut final_input_texture = lottes_input_texture;

            if run_pixelate {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[4]));
                gl.use_program(Some(self.pixelate_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(lottes_input_texture));
                gl.uniform_2_f32(Some(&self.p_pixelate_target_res_loc), 854.0, 480.0);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                final_input_texture = self.pass_textures[4];
            }

            if run_lottes {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[0]));
                gl.use_program(Some(self.pass0_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(final_input_texture));
                gl.uniform_1_f32(Some(&self.p0_hard_bloom_pix_loc), params.hard_bloom_pix);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[1]));
                gl.use_program(Some(self.pass1_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[0]));
                gl.uniform_1_f32(Some(&self.p1_hard_bloom_scan_loc), params.hard_bloom_scan);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[2]));
                gl.use_program(Some(self.pass2_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(final_input_texture));
                gl.uniform_1_f32(Some(&self.p2_hard_pix_loc), params.hard_pix);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[3]));
                gl.use_program(Some(self.pass3_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[2]));
                gl.uniform_1_f32(Some(&self.p3_hard_scan_loc), params.hard_scan);
                gl.uniform_1_f32(Some(&self.p3_shape_loc), params.shape);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
                gl.use_program(Some(self.final_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[1]));
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[3]));

                gl.uniform_2_f32(Some(&self.final_video_res_loc), resolution.0 as f32, resolution.1 as f32);
                gl.uniform_2_f32(Some(&self.final_output_res_loc), output_size.0, output_size.1);
                gl.uniform_1_f32(Some(&self.final_warp_x_loc), params.warp_x);
                gl.uniform_1_f32(Some(&self.final_warp_y_loc), params.warp_y);
                gl.uniform_1_f32(Some(&self.final_shadow_mask_loc), params.shadow_mask);
                gl.uniform_1_f32(Some(&self.final_brightboost_loc), params.brightboost);
                gl.uniform_1_f32(Some(&self.final_bloom_amount_loc), params.bloom_amount);
                gl.uniform_3_f32(Some(&self.final_background_color_loc), params.background_color[0], params.background_color[1], params.background_color[2]);
                gl.uniform_1_f32(Some(&self.final_horizontal_stretch_loc), params.horizontal_stretch);
                gl.uniform_1_f32(Some(&self.final_vibrance_loc), params.vibrance);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            } else if run_pixelate {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
                gl.use_program(Some(self.passthrough_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(final_input_texture));
                gl.uniform_2_f32(Some(&self.p_passthrough_video_res_loc), resolution.0 as f32, resolution.1 as f32);
                gl.uniform_2_f32(Some(&self.p_passthrough_output_res_loc), output_size.0, output_size.1);
                gl.uniform_3_f32(Some(&self.passthrough_background_color_loc), params.background_color[0], params.background_color[1], params.background_color[2]);
                gl.uniform_1_f32(Some(&self.passthrough_horizontal_stretch_loc), params.horizontal_stretch);
                gl.uniform_1_f32(Some(&self.passthrough_vibrance_loc), params.vibrance);
                gl.uniform_1_i32(Some(&self.passthrough_scaler_filter_loc), params.scaler_filter as i32);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            } else {
                self.draw_passthrough(gl, None, Some(lottes_input_texture), resolution, output_size, params.background_color, params.horizontal_stretch, params.median_filter_enabled, params.median_mix, params.vibrance, params.scaler_filter, params.overscan_x, params.overscan_y, None, 0.0, 0.0);
            }

            gl.bind_vertex_array(None);
            if old_vbo != 0 {
                gl.bind_vertex_array(Some(glow::VertexArray::from(glow::NativeVertexArray(NonZero::new(old_vbo as u32).unwrap()))));
            } else {
                gl.bind_vertex_array(None);
            }
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
        fft_filter: Option<&Arc<Mutex<FftFilter>>>,
        fft_mask_threshold: f32,
        fft_black_threshold: f32,
    ) {
        let mut video_texture = fallback_texture;

        if self.last_size != resolution || self.last_scaler_filter != Some(scaler_filter) {
            self.setup_framebuffers(gl, resolution.0, resolution.1, scaler_filter);
            self.last_size = resolution;
            self.last_scaler_filter = Some(scaler_filter);
        }

        unsafe {
            let old_vbo = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
            gl.bind_vertex_array(Some(self.vertex_array));

            if let Some(frame) = raw_frame {
                video_texture = Some(self.prepare_input_texture(gl, frame, overscan_x, overscan_y, fft_filter, fft_mask_threshold, fft_black_threshold));
                gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);
            }

            let input_texture = video_texture.expect("No video texture available");
            let mut final_input_texture = input_texture;

            if median_filter_enabled {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[5]));
                gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);
                gl.use_program(Some(self.median_prog));
                gl.uniform_1_f32(Some(&self.median_mix_loc), median_mix);
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                final_input_texture = self.pass_textures[5];
            }

            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
            gl.use_program(Some(self.passthrough_prog));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(final_input_texture));

            gl.uniform_2_f32(Some(&self.p_passthrough_video_res_loc), resolution.0 as f32, resolution.1 as f32);
            gl.uniform_2_f32(Some(&self.p_passthrough_output_res_loc), output_size.0, output_size.1);
            gl.uniform_3_f32(Some(&self.passthrough_background_color_loc), background_color[0], background_color[1], background_color[2]);
            gl.uniform_1_f32(Some(&self.passthrough_horizontal_stretch_loc), horizontal_stretch);
            gl.uniform_1_f32(Some(&self.passthrough_vibrance_loc), vibrance);
            gl.uniform_1_i32(Some(&self.passthrough_scaler_filter_loc), scaler_filter as i32);

            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            gl.bind_vertex_array(Some(glow::VertexArray::from(glow::NativeVertexArray(NonZero::new(old_vbo as u32).unwrap()))));
        }
    }

    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.passthrough_prog);
            gl.delete_program(self.pixelate_prog);
            gl.delete_program(self.pass0_prog);
            gl.delete_program(self.pass1_prog);
            gl.delete_program(self.pass2_prog);
            gl.delete_program(self.pass3_prog);
            gl.delete_program(self.yuv_planar_prog);
            gl.delete_program(self.yuyv_packed_prog);
            gl.delete_vertex_array(self.vertex_array);
            gl.delete_buffer(self.vbo);
            for fbo in self.fbos { gl.delete_framebuffer(fbo); }
            for texture in self.pass_textures { gl.delete_texture(texture); }
            for texture in self.yuv_planes { gl.delete_texture(texture); }
            for pbo in self.pbos { gl.delete_buffer(pbo); }
        }
    }

    fn setup_framebuffers(&mut self, gl: &glow::Context, width: u32, height: u32, scaler_filter: u8) {
        let filter_mode = if scaler_filter == crate::video::types::ScalerFilter::Point as u8 {
            glow::NEAREST as i32
        } else {
            glow::LINEAR as i32
        };

        unsafe {
            for i in 0..self.pass_textures.len() {
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[i]));
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA as i32, width as i32, height as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, None);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter_mode);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter_mode);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[i]));
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(self.pass_textures[i]), 0);
            }
            for texture in self.yuv_planes {
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter_mode);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter_mode);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }
}
