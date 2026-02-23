use eframe::glow::{self, HasContext};

use crate::video::types::RawFrame;
use ffmpeg_next::format::Pixel;
use std::num::NonZero;

const VS_SRC: &str = include_str!("shaders/vs_src.glsl");

const FS_YUV_PLANAR: &str = include_str!("shaders/fs_yuv_planar.glsl");

const FS_YUYV_PACKED: &str = include_str!("shaders/fs_yuyv_packed.glsl");

// 3x1 Horizontal Median Filter
const FS_MEDIAN_3X1: &str = include_str!("shaders/fs_median_3x1.glsl");

// Pixelation shader to simulate 480p
const FS_PIXELATE: &str = include_str!("shaders/fs_pixelate.glsl");

// Simple passthrough shader for drawing a texture to the screen
const FS_PASSTHROUGH: &str = include_str!("shaders/fs_passthrough.glsl");
// Lottes Pass 0: Horizontal blur for bloom
const FS_PASS0: &str = include_str!("shaders/fs_pass0.glsl");

// Lottes Pass 1: Vertical blur for bloom
const FS_PASS1: &str = include_str!("shaders/fs_pass1.glsl");

// Lottes Pass 2: Horizontal blur for scanlines
const FS_PASS2: &str = include_str!("shaders/fs_pass2.glsl");

// Lottes Pass 3: Vertical blur for scanlines
const FS_PASS3: &str = include_str!("shaders/fs_pass3.glsl");

// Lottes Final Pass: Combines textures and applies warp, mask, and color correction
const FS_FINAL: &str = include_str!("shaders/fs_final.glsl");

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

    fbos: [glow::Framebuffer; 7], // 0-5 for passes, 6 for YUV conversion result
    pass_textures: [glow::Texture; 7], // 0-5 for passes, 6 for the YUV source texture
    yuv_planes: [glow::Texture; 3], // textures for Y, U, V planes
    vertex_array: glow::VertexArray,
    vbo: glow::Buffer,

    // Passthrough uniforms
    p_passthrough_video_res_loc: glow::UniformLocation,
    p_passthrough_output_res_loc: glow::UniformLocation,

    // Pixelate uniforms
    p_pixelate_target_res_loc: glow::UniformLocation,
    // Pass 0 uniforms
    p0_hard_bloom_pix_loc: glow::UniformLocation,

    // Pass 1 uniforms
    p1_hard_bloom_scan_loc: glow::UniformLocation,

    // Pass 2 uniforms
    p2_hard_pix_loc: glow::UniformLocation,

    // Pass 3 uniforms
    p3_hard_scan_loc: glow::UniformLocation,
    p3_shape_loc: glow::UniformLocation,

    // Final pass uniforms
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

    last_size: (u32, u32),
    last_scaler_filter: Option<u8>,
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

            // Passthrough
            let p_passthrough_video_res_loc = gl
                .get_uniform_location(passthrough_prog, "videoResolution")
                .unwrap();
            let p_passthrough_output_res_loc = gl
                .get_uniform_location(passthrough_prog, "outputResolution")
                .unwrap();

            // Pixelate
            let p_pixelate_target_res_loc = gl
                .get_uniform_location(pixelate_prog, "target_resolution")
                .unwrap();

            // Pass 0
            let p0_hard_bloom_pix_loc =
                gl.get_uniform_location(pass0_prog, "hardBloomPix").unwrap();

            // Pass 1
            let p1_hard_bloom_scan_loc = gl
                .get_uniform_location(pass1_prog, "hardBloomScan")
                .unwrap();

            // Pass 2
            let p2_hard_pix_loc = gl.get_uniform_location(pass2_prog, "hardPix").unwrap();

            // Pass 3
            let p3_hard_scan_loc = gl.get_uniform_location(pass3_prog, "hardScan").unwrap();
            let p3_shape_loc = gl.get_uniform_location(pass3_prog, "shape").unwrap();

            // Median Filter
            gl.use_program(Some(median_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(median_prog, "video_texture")
                        .unwrap(),
                ),
                0,
            );
            gl.use_program(None);

            // Final Pass
            let final_video_res_loc = gl
                .get_uniform_location(final_prog, "videoResolution")
                .unwrap();
            let final_output_res_loc = gl
                .get_uniform_location(final_prog, "outputResolution")
                .unwrap();
            let final_warp_x_loc = gl.get_uniform_location(final_prog, "warpX").unwrap();
            let final_warp_y_loc = gl.get_uniform_location(final_prog, "warpY").unwrap();
            let final_shadow_mask_loc = gl.get_uniform_location(final_prog, "shadowMask").unwrap();
            let final_brightboost_loc = gl.get_uniform_location(final_prog, "brightboost").unwrap();
            let final_bloom_amount_loc =
                gl.get_uniform_location(final_prog, "bloomAmount").unwrap();
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

            // Set sampler uniforms once, as they don't change.
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

            gl.use_program(Some(pass0_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(pass0_prog, "video_texture")
                        .unwrap(),
                ),
                0,
            );

            gl.use_program(Some(pass1_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(pass1_prog, "pass0_texture")
                        .unwrap(),
                ),
                0,
            );

            gl.use_program(Some(pass2_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(pass2_prog, "video_texture")
                        .unwrap(),
                ),
                0,
            );

            gl.use_program(Some(pass3_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(pass3_prog, "pass2_texture")
                        .unwrap(),
                ),
                0,
            );

            gl.use_program(Some(final_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(final_prog, "pass1_texture")
                        .unwrap(),
                ),
                0,
            );
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(final_prog, "pass3_texture")
                        .unwrap(),
                ),
                1,
            );

            // YUV Planar
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

            // YUYV Packed
            gl.use_program(Some(yuyv_packed_prog));
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(yuyv_packed_prog, "raw_tex")
                        .unwrap(),
                ),
                0,
            );
            let yuyv_range_loc = gl.get_uniform_location(yuyv_packed_prog, "input_range").unwrap();

            gl.use_program(Some(yuv_planar_prog));
            let yuv_range_loc = gl.get_uniform_location(yuv_planar_prog, "input_range").unwrap();

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

            let vertex_array = gl
                .create_vertex_array()
                .expect("Cannot create vertex array");

            // --- Fullscreen Quad ---
            // We need a vertex buffer to draw a simple quad.
            let vertices: [f32; 16] = [
                // pos    // tex
                -1.0, -1.0, 0.0, 0.0, // bottom-left
                1.0, -1.0, 1.0, 0.0, // bottom-right
                -1.0, 1.0, 0.0, 1.0, // top-left
                1.0, 1.0, 1.0, 1.0, // top-right
            ];
            let vbo = gl.create_buffer().unwrap();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&vertices),
                glow::STATIC_DRAW,
            );

            gl.bind_vertex_array(Some(vertex_array));
            // Position attribute
            gl.vertex_attrib_pointer_f32(
                0,
                2,
                glow::FLOAT,
                false,
                4 * std::mem::size_of::<f32>() as i32,
                0,
            );
            gl.enable_vertex_attrib_array(0);
            // Texture coordinate attribute
            gl.vertex_attrib_pointer_f32(
                1,
                2,
                glow::FLOAT,
                false,
                4 * std::mem::size_of::<f32>() as i32,
                (2 * std::mem::size_of::<f32>()) as i32,
            );
            gl.enable_vertex_attrib_array(1);

            // Unbind VBO and VAO
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_vertex_array(None);

            Self {
                passthrough_prog,
                pixelate_prog,
                pass0_prog,
                pass1_prog,
                pass2_prog,
                pass3_prog,
                final_prog,
                yuv_planar_prog,
                yuyv_packed_prog,
                yuv_range_loc,
                yuyv_range_loc,
                fbos,
                pass_textures,
                yuv_planes,
                vertex_array,
                vbo,
                p_passthrough_video_res_loc,
                p_passthrough_output_res_loc,
                p_pixelate_target_res_loc,
                p0_hard_bloom_pix_loc,
                p1_hard_bloom_scan_loc,
                p2_hard_pix_loc,
                p3_hard_scan_loc,
                p3_shape_loc,
                final_video_res_loc,
                final_output_res_loc,
                final_warp_x_loc,
                final_warp_y_loc,
                final_shadow_mask_loc,
                final_brightboost_loc,
                final_bloom_amount_loc,
                final_background_color_loc,
                passthrough_background_color_loc,
                final_horizontal_stretch_loc,
                passthrough_horizontal_stretch_loc,
                final_vibrance_loc,
                passthrough_vibrance_loc,
                median_prog,
                last_size: (0, 0),
                last_scaler_filter: None,
            }
        }
    }

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
    ) {
        let mut video_texture = fallback_texture;

        if self.last_size != resolution || self.last_scaler_filter != Some(params.scaler_filter) {
            self.setup_framebuffers(gl, resolution.0, resolution.1, params.scaler_filter);
            self.last_size = resolution;
            self.last_scaler_filter = Some(params.scaler_filter);
        }

        unsafe {
            // Save egui's vertex array binding
            let old_vbo = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);

            gl.bind_vertex_array(Some(self.vertex_array));
            gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);

            if let Some(frame) = raw_frame {
                // --- YUV CONVERSION PASS ---
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[6]));
                gl.viewport(0, 0, frame.width as i32, frame.height as i32);
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                gl.clear(glow::COLOR_BUFFER_BIT);

                if frame.format == Pixel::YUV422P
                    || frame.format == Pixel::YUV420P
                    || frame.format == Pixel::YUVJ422P
                    || frame.format == Pixel::YUVJ420P
                {
                    gl.use_program(Some(self.yuv_planar_prog));
                    let (y_end, u_end) =
                        if frame.format == Pixel::YUV422P || frame.format == Pixel::YUVJ422P {
                            (
                                (frame.width * frame.height) as usize,
                                (frame.width * frame.height * 3 / 2) as usize,
                            )
                        } else {
                            (
                                (frame.width * frame.height) as usize,
                                (frame.width * frame.height * 5 / 4) as usize,
                            )
                        };

                    let y_data = &frame.data[0..y_end];
                    let u_data = &frame.data[y_end..u_end];
                    let v_data = &frame.data[u_end..];

                    let chroma_width = frame.width / 2;
                    let chroma_height =
                        if frame.format == Pixel::YUV422P || frame.format == Pixel::YUVJ422P {
                            frame.height
                        } else {
                            frame.height / 2
                        };

                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[0]));
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::R8 as i32,
                        frame.width as i32,
                        frame.height as i32,
                        0,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        Some(y_data),
                    );

                    gl.active_texture(glow::TEXTURE1);
                    gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[1]));
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::R8 as i32,
                        chroma_width as i32,
                        chroma_height as i32,
                        0,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        Some(u_data),
                    );

                    gl.active_texture(glow::TEXTURE2);
                    gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[2]));
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::R8 as i32,
                        chroma_width as i32,
                        chroma_height as i32,
                        0,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        Some(v_data),
                    );
                    gl.uniform_1_i32(Some(&self.yuv_range_loc), frame.color_range as i32);
                } else if frame.format == Pixel::YUYV422 {
                    gl.use_program(Some(self.yuyv_packed_prog));
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[0]));
                    // YUYV is 2 bytes per pixel, we treat it as RGBA with width / 2
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA8 as i32,
                        (frame.width / 2) as i32,
                        frame.height as i32,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        Some(&frame.data),
                    );
                    gl.uniform_1_i32(Some(&self.yuyv_range_loc), frame.color_range as i32);
                }

                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                video_texture = Some(self.pass_textures[6]);
                gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);
            }

            let input_texture = video_texture.expect("No video texture available");
            let mut lottes_input_texture = input_texture;

            if params.median_filter_enabled {
                // --- MEDIAN FILTER PASS ---
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[5]));
                gl.use_program(Some(self.median_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, video_texture);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                lottes_input_texture = self.pass_textures[5];
            }

            let mut final_input_texture = lottes_input_texture;

            if run_pixelate {
                // --- PIXELATE PASS ---
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[4]));
                gl.use_program(Some(self.pixelate_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(lottes_input_texture));
                // Target 480p 16:9
                gl.uniform_2_f32(Some(&self.p_pixelate_target_res_loc), 854.0, 480.0);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                final_input_texture = self.pass_textures[4];
            }

            if run_lottes {
                // --- PASS 0 (Horizontal Bloom) ---
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[0]));
                gl.use_program(Some(self.pass0_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(final_input_texture));
                gl.uniform_1_f32(Some(&self.p0_hard_bloom_pix_loc), params.hard_bloom_pix);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

                // --- PASS 1 (Vertical Bloom) ---
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[1]));
                gl.use_program(Some(self.pass1_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[0]));
                gl.uniform_1_f32(Some(&self.p1_hard_bloom_scan_loc), params.hard_bloom_scan);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

                // --- PASS 2 (Horizontal Scanlines) ---
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[2]));
                gl.use_program(Some(self.pass2_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(final_input_texture));
                gl.uniform_1_f32(Some(&self.p2_hard_pix_loc), params.hard_pix);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

                // --- PASS 3 (Vertical Scanlines) ---
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[3]));
                gl.use_program(Some(self.pass3_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[2]));
                gl.uniform_1_f32(Some(&self.p3_hard_scan_loc), params.hard_scan);
                gl.uniform_1_f32(Some(&self.p3_shape_loc), params.shape);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

                // --- FINAL PASS ---
                gl.bind_framebuffer(glow::FRAMEBUFFER, None); // Render to screen
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
                gl.use_program(Some(self.final_prog));

                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[1])); // bloom

                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pass_textures[3])); // scanlines

                gl.uniform_2_f32(
                    Some(&self.final_video_res_loc),
                    resolution.0 as f32,
                    resolution.1 as f32,
                );
                gl.uniform_2_f32(
                    Some(&self.final_output_res_loc),
                    output_size.0,
                    output_size.1,
                );
                gl.uniform_1_f32(Some(&self.final_warp_x_loc), params.warp_x);
                gl.uniform_1_f32(Some(&self.final_warp_y_loc), params.warp_y);
                gl.uniform_1_f32(Some(&self.final_shadow_mask_loc), params.shadow_mask);
                gl.uniform_1_f32(Some(&self.final_brightboost_loc), params.brightboost);
                gl.uniform_1_f32(Some(&self.final_bloom_amount_loc), params.bloom_amount);
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

                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            } else if run_pixelate {
                // If only pixelation is enabled, we need to draw its result to the screen.
                gl.bind_framebuffer(glow::FRAMEBUFFER, None); // Render to screen
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
                gl.use_program(Some(self.passthrough_prog));

                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(final_input_texture));

                gl.uniform_2_f32(
                    Some(&self.p_passthrough_video_res_loc),
                    resolution.0 as f32,
                    resolution.1 as f32,
                );
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

                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            } else {
                // Fallback: draw directly to screen using passthrough
                self.draw_passthrough(
                    gl,
                    None,
                    Some(lottes_input_texture),
                    resolution,
                    output_size,
                    params.background_color,
                    params.horizontal_stretch,
                    params.median_filter_enabled,
                    params.vibrance,
                    params.scaler_filter,
                );
            }

            gl.bind_vertex_array(None);

            // Restore egui's vertex array binding
            if old_vbo != 0 {
                gl.bind_vertex_array(Some(glow::VertexArray::from(glow::NativeVertexArray(
                    NonZero::new(old_vbo as u32).unwrap(),
                ))));
            } else {
                tracing::warn!("old_vbo was 0, cannot restore egui's VAO binding. This might indicate an issue with egui's GL state management. Binding None instead. This is likely fine if egui is not using a VAO.");
                gl.bind_vertex_array(None);
            }
        }
    }

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
        vibrance: f32,
        scaler_filter: u8,
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
                // --- YUV CONVERSION PASS ---
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[6]));
                gl.viewport(0, 0, frame.width as i32, frame.height as i32);

                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                gl.clear(glow::COLOR_BUFFER_BIT);

                if frame.format == Pixel::YUV422P
                    || frame.format == Pixel::YUV420P
                    || frame.format == Pixel::YUVJ422P
                    || frame.format == Pixel::YUVJ420P
                {
                    gl.use_program(Some(self.yuv_planar_prog));
                    let (y_end, u_end) =
                        if frame.format == Pixel::YUV422P || frame.format == Pixel::YUVJ422P {
                            (
                                (frame.width * frame.height) as usize,
                                (frame.width * frame.height * 3 / 2) as usize,
                            )
                        } else {
                            (
                                (frame.width * frame.height) as usize,
                                (frame.width * frame.height * 5 / 4) as usize,
                            )
                        };

                    let y_data = &frame.data[0..y_end];
                    let u_data = &frame.data[y_end..u_end];
                    let v_data = &frame.data[u_end..];

                    let chroma_width = frame.width / 2;
                    let chroma_height =
                        if frame.format == Pixel::YUV422P || frame.format == Pixel::YUVJ422P {
                            frame.height
                        } else {
                            frame.height / 2
                        };

                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[0]));
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::R8 as i32,
                        frame.width as i32,
                        frame.height as i32,
                        0,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        Some(y_data),
                    );

                    gl.active_texture(glow::TEXTURE1);
                    gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[1]));
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::R8 as i32,
                        chroma_width as i32,
                        chroma_height as i32,
                        0,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        Some(u_data),
                    );

                    gl.active_texture(glow::TEXTURE2);
                    gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[2]));
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::R8 as i32,
                        chroma_width as i32,
                        chroma_height as i32,
                        0,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        Some(v_data),
                    );
                    gl.uniform_1_i32(Some(&self.yuv_range_loc), frame.color_range as i32);
                } else if frame.format == Pixel::YUYV422 {
                    gl.use_program(Some(self.yuyv_packed_prog));
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(self.yuv_planes[0]));
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA8 as i32,
                        (frame.width / 2) as i32,
                        frame.height as i32,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        Some(&frame.data),
                    );
                    gl.uniform_1_i32(Some(&self.yuyv_range_loc), frame.color_range as i32);
                }

                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                video_texture = Some(self.pass_textures[6]);
                gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);
            }

            let input_texture = video_texture.expect("No video texture available");
            let mut final_input_texture = input_texture;

            if median_filter_enabled {
                // --- MEDIAN FILTER PASS ---
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[5]));
                gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);
                gl.use_program(Some(self.median_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                final_input_texture = self.pass_textures[5];
            }

            gl.bind_framebuffer(glow::FRAMEBUFFER, None); // Render to screen
            gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
            gl.use_program(Some(self.passthrough_prog));

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(final_input_texture));

            gl.uniform_2_f32(
                Some(&self.p_passthrough_video_res_loc),
                resolution.0 as f32,
                resolution.1 as f32,
            );
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

            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            gl.bind_vertex_array(Some(glow::VertexArray::from(glow::NativeVertexArray(
                NonZero::new(old_vbo as u32).unwrap(),
            ))));
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
            for fbo in self.fbos {
                gl.delete_framebuffer(fbo);
            }
            for texture in self.pass_textures {
                gl.delete_texture(texture);
            }
            for texture in self.yuv_planes {
                gl.delete_texture(texture);
            }
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
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA as i32,
                    width as i32,
                    height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    None,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    filter_mode,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    filter_mode,
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
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    filter_mode,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    filter_mode,
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
            }

            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }
}

unsafe fn compile_program(gl: &glow::Context, vs_src: &str, fs_src: &str) -> glow::Program {
    let program = gl.create_program().expect("Cannot create program");

    let shader_sources = [
        (glow::VERTEX_SHADER, vs_src),
        (glow::FRAGMENT_SHADER, fs_src),
    ];

    let mut shaders = Vec::with_capacity(shader_sources.len());

    for (shader_type, shader_source) in shader_sources.iter() {
        let shader = gl
            .create_shader(*shader_type)
            .expect("Cannot create shader");
        gl.shader_source(shader, shader_source);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            panic!("{}", gl.get_shader_info_log(shader));
        }
        gl.attach_shader(program, shader);
        shaders.push(shader);
    }

    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        panic!("{}", gl.get_program_info_log(program));
    }

    for shader in shaders {
        gl.detach_shader(program, shader);
        gl.delete_shader(shader);
    }

    program
}

impl ShaderParams {
    pub fn from_state(state: &crate::app::AppState) -> Self {
        Self {
            hard_scan: state.crt.hard_scan,
            warp_x: state.crt.warp_x,
            warp_y: state.crt.warp_y,
            shadow_mask: state.crt.shadow_mask,
            brightboost: state.crt.brightboost,
            hard_bloom_pix: state.crt.hard_bloom_pix,
            hard_bloom_scan: state.crt.hard_bloom_scan,
            bloom_amount: state.crt.bloom_amount,
            shape: state.crt.shape,
            hard_pix: state.crt.hard_pix,
            background_color: if state.video.use_magenta_background {
                [1.0, 0.0, 1.0]
            } else {
                [0.0, 0.0, 0.0]
            },
            horizontal_stretch: state.video.horizontal_stretch,
            median_filter_enabled: state.video.median_filter_enabled,
            vibrance: state.video.vibrance,
            scaler_filter: state.scaler_filter.load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ShaderParams {
    pub hard_scan: f32,
    pub warp_x: f32,
    pub warp_y: f32,
    pub shadow_mask: f32,
    pub brightboost: f32,
    pub hard_bloom_pix: f32,
    pub hard_bloom_scan: f32,
    pub bloom_amount: f32,
    pub shape: f32,
    pub hard_pix: f32,
    pub background_color: [f32; 3],
    pub horizontal_stretch: f32,
    pub median_filter_enabled: bool,
    pub vibrance: f32,
    pub scaler_filter: u8,
}

impl Default for ShaderParams {
    fn default() -> Self {
        Self {
            hard_scan: -8.0,
            warp_x: 0.031,
            warp_y: 0.041,
            shadow_mask: 3.0,
            brightboost: 1.0,
            hard_bloom_pix: -1.5,
            hard_bloom_scan: -2.0,
            bloom_amount: 0.15,
            shape: 2.0,
            hard_pix: -3.0,
            background_color: [0.0, 0.0, 0.0],
            horizontal_stretch: 1.0,
            median_filter_enabled: false,
            vibrance: 1.0,
            scaler_filter: crate::video::types::ScalerFilter::FastBilinear as u8,
        }
    }
}
