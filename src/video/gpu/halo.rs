use super::params::HaloShaderParams;
use super::programs::*;
use eframe::glow::{self, HasContext};

pub struct HaloRenderer {
    linearize_prog: glow::Program,
    glow_h_prog: glow::Program,
    glow_v_prog: glow::Program,
    bloom_h_prog: glow::Program,
    bloom_v_prog: glow::Program,
    scanlines_prog: glow::Program,
    composite_prog: glow::Program,
    final_prog: glow::Program,

    glow_h_source_size_loc: Option<glow::UniformLocation>,
    glow_v_source_size_loc: Option<glow::UniformLocation>,
    bloom_h_source_size_loc: Option<glow::UniformLocation>,
    bloom_v_source_size_loc: Option<glow::UniformLocation>,

    scanlines_output_res_loc: Option<glow::UniformLocation>,
    scanlines_source_size_loc: Option<glow::UniformLocation>,
    scanlines_horizontal_stretch_loc: Option<glow::UniformLocation>,
    scanlines_halo_zoom_loc: Option<glow::UniformLocation>,
    scanlines_beam_min_loc: Option<glow::UniformLocation>,
    scanlines_beam_max_loc: Option<glow::UniformLocation>,
    scanlines_beam_size_loc: Option<glow::UniformLocation>,
    scanlines_h_sharp_loc: Option<glow::UniformLocation>,
    scanlines_corner_size_loc: Option<glow::UniformLocation>,
    scanlines_curvature_loc: Option<glow::UniformLocation>,
    scanlines_border_crop_loc: Option<glow::UniformLocation>,

    composite_output_res_loc: Option<glow::UniformLocation>,
    composite_source_size_loc: Option<glow::UniformLocation>,
    composite_horizontal_stretch_loc: Option<glow::UniformLocation>,
    composite_halo_zoom_loc: Option<glow::UniformLocation>,
    composite_brightboost_loc: Option<glow::UniformLocation>,
    composite_brightboost1_loc: Option<glow::UniformLocation>,
    composite_glow_loc: Option<glow::UniformLocation>,
    composite_bloom_loc: Option<glow::UniformLocation>,
    composite_halation_loc: Option<glow::UniformLocation>,
    composite_shadow_mask_loc: Option<glow::UniformLocation>,
    composite_masksize_loc: Option<glow::UniformLocation>,
    composite_maskstr_loc: Option<glow::UniformLocation>,
    composite_mcut_loc: Option<glow::UniformLocation>,
    composite_slotmask_loc: Option<glow::UniformLocation>,
    composite_slotmask1_loc: Option<glow::UniformLocation>,
    composite_double_slot_loc: Option<glow::UniformLocation>,
    composite_smoothmask_loc: Option<glow::UniformLocation>,
    composite_curvature_loc: Option<glow::UniformLocation>,
    composite_vibrance_loc: Option<glow::UniformLocation>,

    final_output_res_loc: Option<glow::UniformLocation>,
    final_source_size_loc: Option<glow::UniformLocation>,
    final_horizontal_stretch_loc: Option<glow::UniformLocation>,
    final_halo_zoom_loc: Option<glow::UniformLocation>,
    final_halo_intensity_loc: Option<glow::UniformLocation>,
    final_background_color_loc: Option<glow::UniformLocation>,
    final_border_crop_loc: Option<glow::UniformLocation>,

    fbos: [glow::Framebuffer; 7],
    textures: [glow::Texture; 7],
    vertex_array: glow::VertexArray,
    vbo: glow::Buffer,

    last_input_size: (u32, u32),
    last_output_size: (u32, u32),
}

impl HaloRenderer {
    pub fn new(gl: &glow::Context) -> Self {
        unsafe {
            let linearize_prog = compile_program(gl, VS_SRC, FS_HALO_LINEARIZE);
            let glow_h_prog = compile_program(gl, VS_SRC, FS_HALO_GLOW_H);
            let glow_v_prog = compile_program(gl, VS_SRC, FS_HALO_GLOW_V);
            let bloom_h_prog = compile_program(gl, VS_SRC, FS_HALO_BLOOM_H);
            let bloom_v_prog = compile_program(gl, VS_SRC, FS_HALO_BLOOM_V);
            let scanlines_prog = compile_program(gl, VS_SRC, FS_HALO_SCANLINES);
            let composite_prog = compile_program(gl, VS_SRC, FS_HALO_COMPOSITE);
            let final_prog = compile_program(gl, VS_SRC, FS_HALO_FINAL);

            // Sampler units
            gl.use_program(Some(linearize_prog));
            gl.uniform_1_i32(
                gl.get_uniform_location(linearize_prog, "video_texture")
                    .as_ref(),
                0,
            );

            gl.use_program(Some(glow_h_prog));
            gl.uniform_1_i32(
                gl.get_uniform_location(glow_h_prog, "video_texture")
                    .as_ref(),
                0,
            );

            gl.use_program(Some(glow_v_prog));
            gl.uniform_1_i32(
                gl.get_uniform_location(glow_v_prog, "video_texture")
                    .as_ref(),
                0,
            );

            gl.use_program(Some(bloom_h_prog));
            gl.uniform_1_i32(
                gl.get_uniform_location(bloom_h_prog, "video_texture")
                    .as_ref(),
                0,
            );

            gl.use_program(Some(bloom_v_prog));
            gl.uniform_1_i32(
                gl.get_uniform_location(bloom_v_prog, "video_texture")
                    .as_ref(),
                0,
            );

            gl.use_program(Some(scanlines_prog));
            gl.uniform_1_i32(
                gl.get_uniform_location(scanlines_prog, "LinearizePass")
                    .as_ref(),
                0,
            );

            gl.use_program(Some(composite_prog));
            gl.uniform_1_i32(
                gl.get_uniform_location(composite_prog, "ScanlinesPass")
                    .as_ref(),
                0,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(composite_prog, "LinearizePass")
                    .as_ref(),
                1,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(composite_prog, "GlowPass").as_ref(),
                2,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(composite_prog, "BloomPass")
                    .as_ref(),
                3,
            );

            gl.use_program(Some(final_prog));
            gl.uniform_1_i32(
                gl.get_uniform_location(final_prog, "DeconvPass").as_ref(),
                0,
            );
            gl.use_program(None);

            // Glow / Bloom uniform locations
            let glow_h_source_size_loc = gl.get_uniform_location(glow_h_prog, "SourceSize");
            let glow_v_source_size_loc = gl.get_uniform_location(glow_v_prog, "SourceSize");
            let bloom_h_source_size_loc = gl.get_uniform_location(bloom_h_prog, "SourceSize");
            let bloom_v_source_size_loc = gl.get_uniform_location(bloom_v_prog, "SourceSize");

            // Scanlines uniform locations
            let scanlines_output_res_loc =
                gl.get_uniform_location(scanlines_prog, "outputResolution");
            let scanlines_source_size_loc = gl.get_uniform_location(scanlines_prog, "SourceSize");
            let scanlines_horizontal_stretch_loc =
                gl.get_uniform_location(scanlines_prog, "horizontal_stretch");
            let scanlines_halo_zoom_loc = gl.get_uniform_location(scanlines_prog, "halo_zoom");
            let scanlines_beam_min_loc = gl.get_uniform_location(scanlines_prog, "beam_min");
            let scanlines_beam_max_loc = gl.get_uniform_location(scanlines_prog, "beam_max");
            let scanlines_beam_size_loc = gl.get_uniform_location(scanlines_prog, "beam_size");
            let scanlines_h_sharp_loc = gl.get_uniform_location(scanlines_prog, "h_sharp");
            let scanlines_corner_size_loc = gl.get_uniform_location(scanlines_prog, "corner_size");
            let scanlines_curvature_loc = gl.get_uniform_location(scanlines_prog, "curvature");
            let scanlines_border_crop_loc = gl.get_uniform_location(scanlines_prog, "border_crop");

            // Composite uniform locations
            let composite_output_res_loc =
                gl.get_uniform_location(composite_prog, "outputResolution");
            let composite_source_size_loc = gl.get_uniform_location(composite_prog, "SourceSize");
            let composite_horizontal_stretch_loc =
                gl.get_uniform_location(composite_prog, "horizontal_stretch");
            let composite_halo_zoom_loc = gl.get_uniform_location(composite_prog, "halo_zoom");
            let composite_brightboost_loc = gl.get_uniform_location(composite_prog, "brightboost");
            let composite_brightboost1_loc =
                gl.get_uniform_location(composite_prog, "brightboost1");
            let composite_glow_loc = gl.get_uniform_location(composite_prog, "glow");
            let composite_bloom_loc = gl.get_uniform_location(composite_prog, "bloom");
            let composite_halation_loc = gl.get_uniform_location(composite_prog, "halation");
            let composite_shadow_mask_loc = gl.get_uniform_location(composite_prog, "shadow_mask");
            let composite_masksize_loc = gl.get_uniform_location(composite_prog, "masksize");
            let composite_maskstr_loc = gl.get_uniform_location(composite_prog, "maskstr");
            let composite_mcut_loc = gl.get_uniform_location(composite_prog, "mcut");
            let composite_slotmask_loc = gl.get_uniform_location(composite_prog, "slotmask");
            let composite_slotmask1_loc = gl.get_uniform_location(composite_prog, "slotmask1");
            let composite_double_slot_loc = gl.get_uniform_location(composite_prog, "double_slot");
            let composite_smoothmask_loc = gl.get_uniform_location(composite_prog, "smoothmask");
            let composite_curvature_loc = gl.get_uniform_location(composite_prog, "curvature");
            let composite_vibrance_loc = gl.get_uniform_location(composite_prog, "vibrance");

            // Final uniform locations
            let final_output_res_loc = gl.get_uniform_location(final_prog, "outputResolution");
            let final_source_size_loc = gl.get_uniform_location(final_prog, "SourceSize");
            let final_horizontal_stretch_loc =
                gl.get_uniform_location(final_prog, "horizontal_stretch");
            let final_halo_zoom_loc = gl.get_uniform_location(final_prog, "halo_zoom");
            let final_halo_intensity_loc = gl.get_uniform_location(final_prog, "halo_intensity");
            let final_background_color_loc =
                gl.get_uniform_location(final_prog, "background_color");
            let final_border_crop_loc =
                gl.get_uniform_location(final_prog, "border_crop");

            let fbos = [
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
            ];
            let textures = [
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
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

            Self {
                linearize_prog,
                glow_h_prog,
                glow_v_prog,
                bloom_h_prog,
                bloom_v_prog,
                scanlines_prog,
                composite_prog,
                final_prog,

                glow_h_source_size_loc,
                glow_v_source_size_loc,
                bloom_h_source_size_loc,
                bloom_v_source_size_loc,

                scanlines_output_res_loc,
                scanlines_source_size_loc,
                scanlines_horizontal_stretch_loc,
                scanlines_halo_zoom_loc,
                scanlines_beam_min_loc,
                scanlines_beam_max_loc,
                scanlines_beam_size_loc,
                scanlines_h_sharp_loc,
                scanlines_corner_size_loc,
                scanlines_curvature_loc,
                scanlines_border_crop_loc,

                composite_output_res_loc,
                composite_source_size_loc,
                composite_horizontal_stretch_loc,
                composite_halo_zoom_loc,
                composite_brightboost_loc,
                composite_brightboost1_loc,
                composite_glow_loc,
                composite_bloom_loc,
                composite_halation_loc,
                composite_shadow_mask_loc,
                composite_masksize_loc,
                composite_maskstr_loc,
                composite_mcut_loc,
                composite_slotmask_loc,
                composite_slotmask1_loc,
                composite_double_slot_loc,
                composite_smoothmask_loc,
                composite_curvature_loc,
                composite_vibrance_loc,

                final_output_res_loc,
                final_source_size_loc,
                final_horizontal_stretch_loc,
                final_halo_zoom_loc,
                final_halo_intensity_loc,
                final_background_color_loc,
                final_border_crop_loc,

                fbos,
                textures,
                vertex_array,
                vbo,

                last_input_size: (0, 0),
                last_output_size: (0, 0),
            }
        }
    }

    pub unsafe fn setup_framebuffers(
        &mut self,
        gl: &glow::Context,
        input_size: (u32, u32),
        output_size: (u32, u32),
    ) {
        for (i, &texture) in self.textures.iter().enumerate() {
            let (w, h) = if i <= 4 {
                input_size
            } else {
                output_size
            };

            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA16F as i32,
                w as i32,
                h as i32,
                0,
                glow::RGBA,
                glow::FLOAT,
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

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[i]));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
        }

        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    }

    #[allow(clippy::too_many_arguments)]
    pub unsafe fn paint(
        &mut self,
        gl: &glow::Context,
        input_texture: glow::Texture,
        input_res: (u32, u32),
        output_size: (f32, f32),
        params: &HaloShaderParams,
        scissor_enabled: bool,
        target_fbo: Option<glow::Framebuffer>,
    ) {
        let in_w = input_res.0.max(1);
        let in_h = input_res.1.max(1);
        let out_w = (output_size.0 as u32).max(1);
        let out_h = (output_size.1 as u32).max(1);

        if self.last_input_size != (in_w, in_h) || self.last_output_size != (out_w, out_h) {
            self.setup_framebuffers(gl, (in_w, in_h), (out_w, out_h));
            self.last_input_size = (in_w, in_h);
            self.last_output_size = (out_w, out_h);
        }

        gl.bind_vertex_array(Some(self.vertex_array));

        // Pass 0: Linearize / Clamping
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[0]));
        gl.viewport(0, 0, in_w as i32, in_h as i32);
        gl.use_program(Some(self.linearize_prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

        // Pass 1: Glow Horizontal
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[1]));
        gl.viewport(0, 0, in_w as i32, in_h as i32);
        gl.use_program(Some(self.glow_h_prog));
        gl.uniform_2_f32(self.glow_h_source_size_loc.as_ref(), in_w as f32, in_h as f32);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[0]));
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

        // Pass 2: Glow Vertical
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[2]));
        gl.viewport(0, 0, in_w as i32, in_h as i32);
        gl.use_program(Some(self.glow_v_prog));
        gl.uniform_2_f32(self.glow_v_source_size_loc.as_ref(), in_w as f32, in_h as f32);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[1]));
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

        // Pass 3: Bloom Horizontal
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[3]));
        gl.viewport(0, 0, in_w as i32, in_h as i32);
        gl.use_program(Some(self.bloom_h_prog));
        gl.uniform_2_f32(self.bloom_h_source_size_loc.as_ref(), in_w as f32, in_h as f32);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[0]));
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

        // Pass 4: Bloom Vertical
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[4]));
        gl.viewport(0, 0, in_w as i32, in_h as i32);
        gl.use_program(Some(self.bloom_v_prog));
        gl.uniform_2_f32(self.bloom_v_source_size_loc.as_ref(), in_w as f32, in_h as f32);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[3]));
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

        // Pass 5: Scanlines
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[5]));
        gl.viewport(0, 0, out_w as i32, out_h as i32);
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.use_program(Some(self.scanlines_prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[0]));
        gl.uniform_2_f32(self.scanlines_output_res_loc.as_ref(), out_w as f32, out_h as f32);
        gl.uniform_2_f32(self.scanlines_source_size_loc.as_ref(), in_w as f32, in_h as f32);
        gl.uniform_1_f32(self.scanlines_horizontal_stretch_loc.as_ref(), params.horizontal_stretch);
        gl.uniform_1_f32(self.scanlines_halo_zoom_loc.as_ref(), params.halo_zoom);
        gl.uniform_1_f32(self.scanlines_beam_min_loc.as_ref(), params.beam_min);
        gl.uniform_1_f32(self.scanlines_beam_max_loc.as_ref(), params.beam_max);
        gl.uniform_1_f32(self.scanlines_beam_size_loc.as_ref(), params.beam_size);
        gl.uniform_1_f32(self.scanlines_h_sharp_loc.as_ref(), params.h_sharp);
        gl.uniform_1_f32(self.scanlines_corner_size_loc.as_ref(), params.corner_size);
        gl.uniform_1_i32(self.scanlines_curvature_loc.as_ref(), if params.curvature { 1 } else { 0 });
        gl.uniform_4_f32(
            self.scanlines_border_crop_loc.as_ref(),
            params.border_crop[0],
            params.border_crop[1],
            params.border_crop[2],
            params.border_crop[3],
        );
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

        // Pass 6: Composite
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[6]));
        gl.viewport(0, 0, out_w as i32, out_h as i32);
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.use_program(Some(self.composite_prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[5]));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[0]));
        gl.active_texture(glow::TEXTURE2);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[2]));
        gl.active_texture(glow::TEXTURE3);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[4]));

        gl.uniform_2_f32(self.composite_output_res_loc.as_ref(), out_w as f32, out_h as f32);
        gl.uniform_2_f32(self.composite_source_size_loc.as_ref(), in_w as f32, in_h as f32);
        gl.uniform_1_f32(self.composite_horizontal_stretch_loc.as_ref(), params.horizontal_stretch);
        gl.uniform_1_f32(self.composite_halo_zoom_loc.as_ref(), params.halo_zoom);
        gl.uniform_1_f32(self.composite_brightboost_loc.as_ref(), params.brightboost);
        gl.uniform_1_f32(self.composite_brightboost1_loc.as_ref(), params.brightboost1);
        gl.uniform_1_f32(self.composite_glow_loc.as_ref(), params.glow);
        gl.uniform_1_f32(self.composite_bloom_loc.as_ref(), params.bloom);
        gl.uniform_1_f32(self.composite_halation_loc.as_ref(), params.halation);
        gl.uniform_1_f32(self.composite_shadow_mask_loc.as_ref(), params.shadow_mask);
        gl.uniform_1_f32(self.composite_masksize_loc.as_ref(), params.masksize);
        gl.uniform_1_f32(self.composite_maskstr_loc.as_ref(), params.maskstr);
        gl.uniform_1_f32(self.composite_mcut_loc.as_ref(), params.mcut);
        gl.uniform_1_f32(self.composite_slotmask_loc.as_ref(), params.slotmask);
        gl.uniform_1_f32(self.composite_slotmask1_loc.as_ref(), params.slotmask1);
        gl.uniform_1_f32(self.composite_double_slot_loc.as_ref(), params.double_slot);
        gl.uniform_1_f32(self.composite_smoothmask_loc.as_ref(), params.smoothmask);
        gl.uniform_1_i32(self.composite_curvature_loc.as_ref(), if params.curvature { 1 } else { 0 });
        gl.uniform_1_f32(self.composite_vibrance_loc.as_ref(), params.vibrance);
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

        // Pass 7: Final (Edge Halo, Dithering, Output)
        gl.bind_framebuffer(glow::FRAMEBUFFER, target_fbo);
        gl.viewport(0, 0, out_w as i32, out_h as i32);
        gl.use_program(Some(self.final_prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[6]));
        gl.uniform_2_f32(self.final_output_res_loc.as_ref(), out_w as f32, out_h as f32);
        gl.uniform_2_f32(self.final_source_size_loc.as_ref(), in_w as f32, in_h as f32);
        gl.uniform_1_f32(self.final_horizontal_stretch_loc.as_ref(), params.horizontal_stretch);
        gl.uniform_1_f32(self.final_halo_zoom_loc.as_ref(), params.halo_zoom);
        gl.uniform_1_f32(self.final_halo_intensity_loc.as_ref(), params.halo_intensity);
        gl.uniform_3_f32(
            self.final_background_color_loc.as_ref(),
            params.background_color[0],
            params.background_color[1],
            params.background_color[2],
        );
        gl.uniform_4_f32(
            self.final_border_crop_loc.as_ref(),
            params.border_crop[0],
            params.border_crop[1],
            params.border_crop[2],
            params.border_crop[3],
        );
        if scissor_enabled {
            gl.enable(glow::SCISSOR_TEST);
        }
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

        // Reset state
        gl.active_texture(glow::TEXTURE3);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.active_texture(glow::TEXTURE2);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.use_program(None);
    }

    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.linearize_prog);
            gl.delete_program(self.glow_h_prog);
            gl.delete_program(self.glow_v_prog);
            gl.delete_program(self.bloom_h_prog);
            gl.delete_program(self.bloom_v_prog);
            gl.delete_program(self.scanlines_prog);
            gl.delete_program(self.composite_prog);
            gl.delete_program(self.final_prog);

            gl.delete_vertex_array(self.vertex_array);
            gl.delete_buffer(self.vbo);

            for fbo in self.fbos {
                gl.delete_framebuffer(fbo);
            }
            for texture in self.textures {
                gl.delete_texture(texture);
            }
        }
    }
}
