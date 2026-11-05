use super::params::BeamShaderParams;
use super::programs::*;
use eframe::glow::{self, HasContext};

pub struct BeamRenderer {
    beam_prog: glow::Program,
    mask_prog: glow::Program,
    copy_prog: glow::Program,

    output_res_loc: Option<glow::UniformLocation>,
    source_size_loc: Option<glow::UniformLocation>,
    background_color_loc: Option<glow::UniformLocation>,
    horizontal_stretch_loc: Option<glow::UniformLocation>,
    vibrance_loc: Option<glow::UniformLocation>,
    scaler_filter_loc: Option<glow::UniformLocation>,
    border_crop_loc: Option<glow::UniformLocation>,

    iframe_loc: Option<glow::UniformLocation>,
    frames_per_hz_loc: Option<glow::UniformLocation>,
    gamma_loc: Option<glow::UniformLocation>,
    gain_vs_blur_loc: Option<glow::UniformLocation>,
    fps_divisor_loc: Option<glow::UniformLocation>,
    lcd_anti_retention_loc: Option<glow::UniformLocation>,
    lcd_slew_loc: Option<glow::UniformLocation>,
    scan_direction_loc: Option<glow::UniformLocation>,
    splitscreen_loc: Option<glow::UniformLocation>,
    splitscreen_x_loc: Option<glow::UniformLocation>,
    splitscreen_y_loc: Option<glow::UniformLocation>,
    splitscreen_border_px_loc: Option<glow::UniformLocation>,
    splitscreen_match_brightness_loc: Option<glow::UniformLocation>,
    h_sharp_loc: Option<glow::UniformLocation>,
    brightboost_loc: Option<glow::UniformLocation>,

    // Stage 2 (CRT Geometry, Scanlines & Phosphor Mask) uniform locations
    mask_output_res_loc: Option<glow::UniformLocation>,
    mask_source_size_loc: Option<glow::UniformLocation>,
    mask_background_color_loc: Option<glow::UniformLocation>,
    mask_horizontal_stretch_loc: Option<glow::UniformLocation>,
    mask_border_crop_loc: Option<glow::UniformLocation>,
    mask_gamma_loc: Option<glow::UniformLocation>,
    mask_post_gain_loc: Option<glow::UniformLocation>,
    mask_curvature_loc: Option<glow::UniformLocation>,
    mask_warp_x_loc: Option<glow::UniformLocation>,
    mask_warp_y_loc: Option<glow::UniformLocation>,
    mask_corner_size_loc: Option<glow::UniformLocation>,
    mask_screen_scale_loc: Option<glow::UniformLocation>,
    mask_scanline_strength_loc: Option<glow::UniformLocation>,
    mask_beam_min_loc: Option<glow::UniformLocation>,
    mask_beam_max_loc: Option<glow::UniformLocation>,
    mask_shadow_mask_loc: Option<glow::UniformLocation>,
    mask_strength_loc: Option<glow::UniformLocation>,
    mask_size_loc: Option<glow::UniformLocation>,
    mask_cutoff_loc: Option<glow::UniformLocation>,
    mask_slot_mask_loc: Option<glow::UniformLocation>,
    mask_double_slot_loc: Option<glow::UniformLocation>,
    mask_smooth_mask_loc: Option<glow::UniformLocation>,
    mask_halation_loc: Option<glow::UniformLocation>,

    cycle_fbos: [glow::Framebuffer; 3],
    cycle_textures: [glow::Texture; 3],
    beam_fbo: glow::Framebuffer,
    beam_texture: glow::Texture,
    vertex_array: glow::VertexArray,
    vbo: glow::Buffer,

    head_idx: usize,
    curr_cycle_started: bool,
    last_crt_hz_counter: Option<i64>,
    frame_counter: i32,
    history_initialized: bool,
    last_input_size: (u32, u32),
    last_beam_output_size: (u32, u32),
    last_scaler_filter: Option<u8>,
    last_effective_fph: f32,
    last_fps_divisor: f32,
}

impl BeamRenderer {
    pub fn new(gl: &glow::Context) -> Self {
        unsafe {
            let beam_prog = compile_program(gl, VS_SRC, FS_CRT_BEAM);
            let mask_prog = compile_program(gl, VS_SRC, FS_CRT_BEAM_MASK);
            let copy_prog = compile_program(gl, VS_SRC, FS_BLIT);

            gl.use_program(Some(beam_prog));
            gl.uniform_1_i32(gl.get_uniform_location(beam_prog, "frame_curr").as_ref(), 0);
            gl.uniform_1_i32(
                gl.get_uniform_location(beam_prog, "frame_prev1").as_ref(),
                1,
            );
            gl.uniform_1_i32(
                gl.get_uniform_location(beam_prog, "frame_prev2").as_ref(),
                2,
            );

            gl.use_program(Some(mask_prog));
            gl.uniform_1_i32(
                gl.get_uniform_location(mask_prog, "beam_texture").as_ref(),
                0,
            );

            gl.use_program(Some(copy_prog));
            gl.uniform_1_i32(gl.get_uniform_location(copy_prog, "u_texture").as_ref(), 0);
            gl.use_program(None);

            let output_res_loc = gl.get_uniform_location(beam_prog, "outputResolution");
            let source_size_loc = gl.get_uniform_location(beam_prog, "source_size");
            let background_color_loc = gl.get_uniform_location(beam_prog, "background_color");
            let horizontal_stretch_loc = gl.get_uniform_location(beam_prog, "horizontal_stretch");
            let vibrance_loc = gl.get_uniform_location(beam_prog, "vibrance");
            let scaler_filter_loc = gl.get_uniform_location(beam_prog, "scaler_filter");
            let border_crop_loc = gl.get_uniform_location(beam_prog, "border_crop");

            let iframe_loc = gl.get_uniform_location(beam_prog, "iFrame");
            let frames_per_hz_loc = gl.get_uniform_location(beam_prog, "frames_per_hz");
            let gamma_loc = gl.get_uniform_location(beam_prog, "gamma");
            let gain_vs_blur_loc = gl.get_uniform_location(beam_prog, "gain_vs_blur");
            let fps_divisor_loc = gl.get_uniform_location(beam_prog, "fps_divisor");
            let lcd_anti_retention_loc = gl.get_uniform_location(beam_prog, "lcd_anti_retention");
            let lcd_slew_loc =
                gl.get_uniform_location(beam_prog, "lcd_inversion_compensation_slew");
            let scan_direction_loc = gl.get_uniform_location(beam_prog, "scan_direction");
            let splitscreen_loc = gl.get_uniform_location(beam_prog, "splitscreen");
            let splitscreen_x_loc = gl.get_uniform_location(beam_prog, "splitscreen_x");
            let splitscreen_y_loc = gl.get_uniform_location(beam_prog, "splitscreen_y");
            let splitscreen_border_px_loc =
                gl.get_uniform_location(beam_prog, "splitscreen_border_px");
            let splitscreen_match_brightness_loc =
                gl.get_uniform_location(beam_prog, "splitscreen_match_brightness");
            let h_sharp_loc = gl.get_uniform_location(beam_prog, "h_sharp");
            let brightboost_loc = gl.get_uniform_location(beam_prog, "brightboost");

            let mask_output_res_loc = gl.get_uniform_location(mask_prog, "outputResolution");
            let mask_source_size_loc = gl.get_uniform_location(mask_prog, "source_size");
            let mask_background_color_loc = gl.get_uniform_location(mask_prog, "background_color");
            let mask_horizontal_stretch_loc =
                gl.get_uniform_location(mask_prog, "horizontal_stretch");
            let mask_border_crop_loc = gl.get_uniform_location(mask_prog, "border_crop");
            let mask_gamma_loc = gl.get_uniform_location(mask_prog, "gamma");
            let mask_post_gain_loc = gl.get_uniform_location(mask_prog, "post_gain");
            let mask_curvature_loc = gl.get_uniform_location(mask_prog, "curvature");
            let mask_warp_x_loc = gl.get_uniform_location(mask_prog, "warp_x");
            let mask_warp_y_loc = gl.get_uniform_location(mask_prog, "warp_y");
            let mask_corner_size_loc = gl.get_uniform_location(mask_prog, "corner_size");
            let mask_screen_scale_loc = gl.get_uniform_location(mask_prog, "screen_scale");
            let mask_scanline_strength_loc =
                gl.get_uniform_location(mask_prog, "scanline_strength");
            let mask_beam_min_loc = gl.get_uniform_location(mask_prog, "beam_min");
            let mask_beam_max_loc = gl.get_uniform_location(mask_prog, "beam_max");
            let mask_shadow_mask_loc = gl.get_uniform_location(mask_prog, "shadow_mask");
            let mask_strength_loc = gl.get_uniform_location(mask_prog, "mask_strength");
            let mask_size_loc = gl.get_uniform_location(mask_prog, "mask_size");
            let mask_cutoff_loc = gl.get_uniform_location(mask_prog, "mask_cutoff");
            let mask_slot_mask_loc = gl.get_uniform_location(mask_prog, "slot_mask");
            let mask_double_slot_loc = gl.get_uniform_location(mask_prog, "double_slot");
            let mask_smooth_mask_loc = gl.get_uniform_location(mask_prog, "smooth_mask");
            let mask_halation_loc = gl.get_uniform_location(mask_prog, "halation");

            let cycle_fbos = [
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
                gl.create_framebuffer().unwrap(),
            ];
            let cycle_textures = [
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
                gl.create_texture().unwrap(),
            ];
            let beam_fbo = gl.create_framebuffer().unwrap();
            let beam_texture = gl.create_texture().unwrap();

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
                beam_prog,
                mask_prog,
                copy_prog,

                output_res_loc,
                source_size_loc,
                background_color_loc,
                horizontal_stretch_loc,
                vibrance_loc,
                scaler_filter_loc,
                border_crop_loc,

                iframe_loc,
                frames_per_hz_loc,
                gamma_loc,
                gain_vs_blur_loc,
                fps_divisor_loc,
                lcd_anti_retention_loc,
                lcd_slew_loc,
                scan_direction_loc,
                splitscreen_loc,
                splitscreen_x_loc,
                splitscreen_y_loc,
                splitscreen_border_px_loc,
                splitscreen_match_brightness_loc,
                h_sharp_loc,
                brightboost_loc,

                mask_output_res_loc,
                mask_source_size_loc,
                mask_background_color_loc,
                mask_horizontal_stretch_loc,
                mask_border_crop_loc,
                mask_gamma_loc,
                mask_post_gain_loc,
                mask_curvature_loc,
                mask_warp_x_loc,
                mask_warp_y_loc,
                mask_corner_size_loc,
                mask_screen_scale_loc,
                mask_scanline_strength_loc,
                mask_beam_min_loc,
                mask_beam_max_loc,
                mask_shadow_mask_loc,
                mask_strength_loc,
                mask_size_loc,
                mask_cutoff_loc,
                mask_slot_mask_loc,
                mask_double_slot_loc,
                mask_smooth_mask_loc,
                mask_halation_loc,

                cycle_fbos,
                cycle_textures,
                beam_fbo,
                beam_texture,
                vertex_array,
                vbo,

                head_idx: 0,
                curr_cycle_started: false,
                last_crt_hz_counter: None,
                frame_counter: 0,
                history_initialized: false,
                last_input_size: (0, 0),
                last_beam_output_size: (0, 0),
                last_scaler_filter: None,
                last_effective_fph: 0.0,
                last_fps_divisor: 0.0,
            }
        }
    }

    unsafe fn setup_beam_output_buffer(&mut self, gl: &glow::Context, out_w: u32, out_h: u32) {
        let w = out_w.max(1);
        let h = out_h.max(1);
        if self.last_beam_output_size == (w, h) {
            return;
        }
        gl.bind_texture(glow::TEXTURE_2D, Some(self.beam_texture));
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
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.beam_fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(self.beam_texture),
            0,
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        self.last_beam_output_size = (w, h);
    }

    unsafe fn copy_texture_to_fbo(
        &self,
        gl: &glow::Context,
        input_texture: glow::Texture,
        fbo_idx: usize,
        in_w: u32,
        in_h: u32,
    ) {
        gl.disable(glow::SCISSOR_TEST);
        gl.use_program(Some(self.copy_prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.cycle_fbos[fbo_idx]));
        gl.viewport(0, 0, in_w as i32, in_h as i32);
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
    }

    unsafe fn setup_cycle_buffers(
        &mut self,
        gl: &glow::Context,
        input_size: (u32, u32),
        scaler_filter: u8,
    ) {
        let (w, h) = (input_size.0.max(1), input_size.1.max(1));
        let filter_param = if scaler_filter == 0 {
            glow::NEAREST as i32
        } else {
            glow::LINEAR as i32
        };

        for (i, &texture) in self.cycle_textures.iter().enumerate() {
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
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter_param);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter_param);
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

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.cycle_fbos[i]));
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

    unsafe fn update_filter_mode(&self, gl: &glow::Context, scaler_filter: u8) {
        let filter_param = if scaler_filter == 0 {
            glow::NEAREST as i32
        } else {
            glow::LINEAR as i32
        };
        for &texture in &self.cycle_textures {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter_param);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter_param);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
    }

    #[allow(clippy::too_many_arguments)]
    pub unsafe fn paint(
        &mut self,
        gl: &glow::Context,
        input_texture: glow::Texture,
        input_res: (u32, u32),
        output_size: (f32, f32),
        params: &BeamShaderParams,
        scissor_enabled: bool,
        target_fbo: Option<glow::Framebuffer>,
    ) {
        let in_w = input_res.0.max(1);
        let in_h = input_res.1.max(1);
        let out_w = (output_size.0 as u32).max(1);
        let out_h = (output_size.1 as u32).max(1);

        if self.last_input_size != (in_w, in_h) {
            self.setup_cycle_buffers(gl, (in_w, in_h), params.scaler_filter);
            self.last_input_size = (in_w, in_h);
            self.last_scaler_filter = Some(params.scaler_filter);
            self.history_initialized = false;
            self.last_crt_hz_counter = None;
        } else if self.last_scaler_filter != Some(params.scaler_filter) {
            self.update_filter_mode(gl, params.scaler_filter);
            self.last_scaler_filter = Some(params.scaler_filter);
        }

        self.setup_beam_output_buffer(gl, out_w, out_h);

        let effective_fph = params.effective_frames_per_hz();
        let fps_divisor = params.fps_divisor.clamp(0.05, 1.0);
        if (effective_fph - self.last_effective_fph).abs() > 1e-6
            || (fps_divisor - self.last_fps_divisor).abs() > 1e-6
        {
            self.frame_counter = 0;
            self.curr_cycle_started = false;
            self.last_crt_hz_counter = None;
            self.last_effective_fph = effective_fph;
            self.last_fps_divisor = fps_divisor;
        }

        // Keep iFrame well within 24-bit float mantissa integer precision (16,777,216)
        let i_frame = if let Some(override_frame) = params.i_frame_override {
            override_frame.rem_euclid(8_000_000)
        } else {
            if self.frame_counter >= 8_000_000 {
                self.frame_counter = 0;
                self.curr_cycle_started = false;
                self.last_crt_hz_counter = None;
            }
            self.frame_counter
        };
        let crt_hz_counter = compute_crt_hz_counter(i_frame, fps_divisor, effective_fph);
        let crt_raster_pos = compute_crt_raster_pos(i_frame, fps_divisor, effective_fph);
        let f_end = crt_raster_pos * effective_fph + 1.0;

        gl.bind_vertex_array(Some(self.vertex_array));

        if !self.history_initialized {
            for i in 0..3 {
                self.copy_texture_to_fbo(gl, input_texture, i, in_w, in_h);
            }
            self.head_idx = 0;
            self.curr_cycle_started = f_end > effective_fph + 1e-5;
            self.last_crt_hz_counter = Some(crt_hz_counter);
            self.history_initialized = true;
        } else if self.last_crt_hz_counter != Some(crt_hz_counter) {
            self.head_idx = (self.head_idx + 1) % 3;
            let prev1_idx = (self.head_idx + 2) % 3;
            if !self.curr_cycle_started {
                // On integer `frames_per_hz` (e.g. 2.0, 3.0, 4.0), `fEnd` never exceeded
                // `effective_fph` during the previous cycle, so `frame_curr` was not yet
                // revealed on screen. Latch the new source frame directly into `prev1_idx`
                // so the new sweep (`[0, effective_fph]`) immediately scans the latest frame
                // with zero 1-cycle latency!
                self.copy_texture_to_fbo(gl, input_texture, prev1_idx, in_w, in_h);
            }
            let wraps_this_subframe = f_end > effective_fph + 1e-5;
            if wraps_this_subframe {
                self.copy_texture_to_fbo(gl, input_texture, self.head_idx, in_w, in_h);
            }
            self.curr_cycle_started = wraps_this_subframe;
            self.last_crt_hz_counter = Some(crt_hz_counter);
        } else if !self.curr_cycle_started && f_end > effective_fph + 1e-5 {
            // Non-integer `frames_per_hz` (e.g. 2.4 at 144Hz): this sub-frame is the first
            // sub-frame of the cycle whose beam wraps past `effective_fph` onto the top of
            // the screen (`overlapCurr > 0`). Latch the latest `input_texture` into `curr`
            // right now so the top wrap-around and the subsequent cycle (after rotation into
            // `prev1`) scan the exact same frame seamlessly without mid-screen tearing!
            self.copy_texture_to_fbo(gl, input_texture, self.head_idx, in_w, in_h);
            self.curr_cycle_started = true;
        }

        let prev1_tex = self.cycle_textures[(self.head_idx + 2) % 3];
        let prev2_tex = self.cycle_textures[(self.head_idx + 1) % 3];
        let curr_tex = if self.curr_cycle_started {
            self.cycle_textures[self.head_idx]
        } else {
            prev1_tex
        };

        // Stage 1: CRT Electron Beam Yoke Simulator -> renders linear light + steady luma to `self.beam_fbo` (RGBA16F)
        gl.disable(glow::SCISSOR_TEST);
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.beam_fbo));
        gl.viewport(0, 0, out_w as i32, out_h as i32);

        gl.use_program(Some(self.beam_prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(curr_tex));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(prev1_tex));
        gl.active_texture(glow::TEXTURE2);
        gl.bind_texture(glow::TEXTURE_2D, Some(prev2_tex));

        gl.uniform_2_f32(self.output_res_loc.as_ref(), out_w as f32, out_h as f32);
        gl.uniform_2_f32(self.source_size_loc.as_ref(), in_w as f32, in_h as f32);
        gl.uniform_3_f32(
            self.background_color_loc.as_ref(),
            params.background_color[0],
            params.background_color[1],
            params.background_color[2],
        );
        gl.uniform_1_f32(
            self.horizontal_stretch_loc.as_ref(),
            params.horizontal_stretch,
        );
        gl.uniform_1_f32(self.vibrance_loc.as_ref(), params.vibrance);
        gl.uniform_1_i32(self.scaler_filter_loc.as_ref(), params.scaler_filter as i32);
        gl.uniform_4_f32(
            self.border_crop_loc.as_ref(),
            params.border_crop[0],
            params.border_crop[1],
            params.border_crop[2],
            params.border_crop[3],
        );

        gl.uniform_1_i32(self.iframe_loc.as_ref(), i_frame);
        gl.uniform_1_f32(
            self.frames_per_hz_loc.as_ref(),
            params.frames_per_hz.max(1.0),
        );
        gl.uniform_1_f32(self.gamma_loc.as_ref(), params.gamma.clamp(1.0, 5.0));
        gl.uniform_1_f32(
            self.gain_vs_blur_loc.as_ref(),
            params.gain_vs_blur.clamp(0.01, 1.0),
        );
        gl.uniform_1_f32(self.fps_divisor_loc.as_ref(), fps_divisor);
        gl.uniform_1_i32(
            self.lcd_anti_retention_loc.as_ref(),
            if params.lcd_anti_retention { 1 } else { 0 },
        );
        gl.uniform_1_f32(
            self.lcd_slew_loc.as_ref(),
            params.lcd_inversion_compensation_slew.clamp(0.0, 0.05),
        );
        gl.uniform_1_i32(
            self.scan_direction_loc.as_ref(),
            params.scan_direction.min(4) as i32,
        );
        gl.uniform_1_i32(
            self.splitscreen_loc.as_ref(),
            if params.splitscreen { 1 } else { 0 },
        );
        gl.uniform_1_f32(
            self.splitscreen_x_loc.as_ref(),
            params.splitscreen_x.clamp(0.0, 1.0),
        );
        gl.uniform_1_f32(
            self.splitscreen_y_loc.as_ref(),
            params.splitscreen_y.clamp(0.0, 1.0),
        );
        gl.uniform_1_i32(
            self.splitscreen_border_px_loc.as_ref(),
            params.splitscreen_border_px as i32,
        );
        gl.uniform_1_i32(
            self.splitscreen_match_brightness_loc.as_ref(),
            if params.splitscreen_match_brightness {
                1
            } else {
                0
            },
        );
        gl.uniform_1_f32(self.h_sharp_loc.as_ref(), params.h_sharp.clamp(1.0, 10.0));
        gl.uniform_1_f32(
            self.brightboost_loc.as_ref(),
            params.brightboost.clamp(0.5, 2.5),
        );

        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

        gl.active_texture(glow::TEXTURE2);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, None);

        // Stage 2: CRT Tube Geometry Transformation + Gamma-Curve-Preserving Scanlines & Phosphor Mask -> `target_fbo`
        gl.bind_framebuffer(glow::FRAMEBUFFER, target_fbo);
        gl.viewport(0, 0, out_w as i32, out_h as i32);
        if scissor_enabled {
            gl.enable(glow::SCISSOR_TEST);
        }

        gl.use_program(Some(self.mask_prog));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.beam_texture));

        let gvb = params.gain_vs_blur.clamp(0.01, 1.0);
        let bb = params.brightboost.clamp(0.5, 2.5);
        let eff_gain = (gvb * bb).clamp(0.01, 1.0);
        let post_gain = (gvb * bb) / eff_gain;

        gl.uniform_2_f32(
            self.mask_output_res_loc.as_ref(),
            out_w as f32,
            out_h as f32,
        );
        gl.uniform_2_f32(self.mask_source_size_loc.as_ref(), in_w as f32, in_h as f32);
        gl.uniform_3_f32(
            self.mask_background_color_loc.as_ref(),
            params.background_color[0],
            params.background_color[1],
            params.background_color[2],
        );
        gl.uniform_1_f32(
            self.mask_horizontal_stretch_loc.as_ref(),
            params.horizontal_stretch,
        );
        gl.uniform_4_f32(
            self.mask_border_crop_loc.as_ref(),
            params.border_crop[0],
            params.border_crop[1],
            params.border_crop[2],
            params.border_crop[3],
        );
        gl.uniform_1_f32(self.mask_gamma_loc.as_ref(), params.gamma.clamp(1.0, 5.0));
        gl.uniform_1_f32(self.mask_post_gain_loc.as_ref(), post_gain);
        gl.uniform_1_i32(
            self.mask_curvature_loc.as_ref(),
            if params.curvature { 1 } else { 0 },
        );
        gl.uniform_1_f32(
            self.mask_warp_x_loc.as_ref(),
            params.warp_x.clamp(0.0, 0.25),
        );
        gl.uniform_1_f32(
            self.mask_warp_y_loc.as_ref(),
            params.warp_y.clamp(0.0, 0.25),
        );
        gl.uniform_1_f32(
            self.mask_corner_size_loc.as_ref(),
            params.corner_size.clamp(0.0, 0.10),
        );
        gl.uniform_1_f32(
            self.mask_screen_scale_loc.as_ref(),
            params.screen_scale.clamp(50.0, 100.0),
        );
        gl.uniform_1_f32(
            self.mask_scanline_strength_loc.as_ref(),
            params.scanline_strength.clamp(0.0, 1.0),
        );
        gl.uniform_1_f32(
            self.mask_beam_min_loc.as_ref(),
            params.beam_min.clamp(0.3, 3.0),
        );
        gl.uniform_1_f32(
            self.mask_beam_max_loc.as_ref(),
            params.beam_max.clamp(0.2, 3.0),
        );
        gl.uniform_1_f32(
            self.mask_shadow_mask_loc.as_ref(),
            params.shadow_mask.clamp(0.0, 6.0).round(),
        );
        gl.uniform_1_f32(
            self.mask_strength_loc.as_ref(),
            params.mask_strength.clamp(0.0, 1.0),
        );
        gl.uniform_1_f32(
            self.mask_size_loc.as_ref(),
            params.mask_size.clamp(1.0, 4.0).round(),
        );
        gl.uniform_1_f32(
            self.mask_cutoff_loc.as_ref(),
            params.mask_cutoff.clamp(0.0, 1.0),
        );
        gl.uniform_1_f32(
            self.mask_slot_mask_loc.as_ref(),
            params.slot_mask.clamp(0.0, 1.0),
        );
        gl.uniform_1_f32(
            self.mask_double_slot_loc.as_ref(),
            params.double_slot.clamp(1.0, 4.0).round(),
        );
        gl.uniform_1_f32(
            self.mask_smooth_mask_loc.as_ref(),
            params.smooth_mask.clamp(0.0, 1.5),
        );
        gl.uniform_1_f32(
            self.mask_halation_loc.as_ref(),
            params.halation.clamp(0.0, 1.0),
        );

        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.use_program(None);

        if params.i_frame_override.is_none() {
            self.frame_counter = self.frame_counter.wrapping_add(1);
        }
    }

    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.beam_prog);
            gl.delete_program(self.mask_prog);
            gl.delete_program(self.copy_prog);
            gl.delete_vertex_array(self.vertex_array);
            gl.delete_buffer(self.vbo);
            for fbo in self.cycle_fbos {
                gl.delete_framebuffer(fbo);
            }
            for texture in self.cycle_textures {
                gl.delete_texture(texture);
            }
            gl.delete_framebuffer(self.beam_fbo);
            gl.delete_texture(self.beam_texture);
        }
    }
}

/// Computes the integer `crtHzCounter` matching `fs_crt_beam.glsl`:
/// `effectiveFrame = floor(float(iFrame) * FPS_DIVISOR)`
/// `crtHzCounter = floor(effectiveFrame / effective_frames_per_hz)`
pub fn compute_crt_hz_counter(i_frame: i32, fps_divisor: f32, effective_frames_per_hz: f32) -> i64 {
    let effective_frame = ((i_frame as f32) * fps_divisor).floor();
    let denom = effective_frames_per_hz.max(1.0);
    (effective_frame / denom).floor() as i64
}

/// Computes `crtRasterPos` in `[0.0, 1.0)` matching `fs_crt_beam.glsl`:
/// `crtRasterPos = mod(effectiveFrame, effective_frames_per_hz) / effective_frames_per_hz`
pub fn compute_crt_raster_pos(i_frame: i32, fps_divisor: f32, effective_frames_per_hz: f32) -> f32 {
    let effective_frame = ((i_frame as f32) * fps_divisor).floor();
    let denom = effective_frames_per_hz.max(1.0);
    let rem = effective_frame - (effective_frame / denom).floor() * denom;
    rem / denom
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_overlap(min_a: f32, max_a: f32, min_b: f32, max_b: f32) -> f32 {
        (max_a.min(max_b) - min_a.max(min_b)).max(0.0)
    }

    #[test]
    fn test_crt_beam_cycle_counter_and_raster_pos_at_120hz() {
        let mut params = BeamShaderParams::default();
        params.lcd_anti_retention = false;
        params.frames_per_hz = 2.0; // 120Hz native / 60Hz CRT
        let eff_fph = params.effective_frames_per_hz();
        assert_eq!(eff_fph, 2.0);

        // Frame 0: cycle 0, top half (raster pos 0.0)
        assert_eq!(compute_crt_hz_counter(0, 1.0, eff_fph), 0);
        assert!((compute_crt_raster_pos(0, 1.0, eff_fph) - 0.0).abs() < 1e-6);

        // Frame 1: cycle 0, bottom half (raster pos 0.5)
        assert_eq!(compute_crt_hz_counter(1, 1.0, eff_fph), 0);
        assert!((compute_crt_raster_pos(1, 1.0, eff_fph) - 0.5).abs() < 1e-6);

        // Frame 2: cycle 1, top half (raster pos 0.0)
        assert_eq!(compute_crt_hz_counter(2, 1.0, eff_fph), 1);
        assert!((compute_crt_raster_pos(2, 1.0, eff_fph) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_lcd_anti_retention_adds_slew_only_for_even_integers() {
        let mut params = BeamShaderParams::default();
        params.lcd_anti_retention = true;
        params.lcd_inversion_compensation_slew = 0.001;

        params.frames_per_hz = 2.0; // Even integer (120Hz)
        assert!((params.effective_frames_per_hz() - 2.001).abs() < 1e-6);

        params.frames_per_hz = 4.0; // Even integer (240Hz)
        assert!((params.effective_frames_per_hz() - 4.001).abs() < 1e-6);

        params.frames_per_hz = 3.0; // Odd integer (180Hz) -> no slew needed
        assert!((params.effective_frames_per_hz() - 3.0).abs() < 1e-6);

        params.frames_per_hz = 2.5; // Non-integer -> no extra slew needed
        assert!((params.effective_frames_per_hz() - 2.5).abs() < 1e-6);
    }

    #[test]
    fn test_beam_overlap_integral_across_full_refresh_cycle() {
        // Verify that for any scanline position pos in [0, 1] and any channel value c in [0, 1],
        // integrating getPixelTubeColor's normalized overlap weights across a full integer
        // FRAMES_PER_HZ cycle reconstructs c identically across every scanline position.
        let frames_per_hz = 4; // 240Hz / 60Hz = 4 subframes per cycle
        let fph = frames_per_hz as f32;
        let c = 0.65f32;
        let gain_vs_blur = 0.7f32;

        for y_step in 0..=20 {
            let pos = y_step as f32 / 20.0;
            let mut accum_signal = 0.0f32;

            for f in 0..frames_per_hz {
                let crt_raster_pos = f as f32 / fph;
                let w_prev2 = get_overlap(
                    pos - 2.0,
                    pos - 2.0 + c * gain_vs_blur,
                    crt_raster_pos,
                    crt_raster_pos + 1.0 / fph,
                );
                let w_prev1 = get_overlap(
                    pos - 1.0,
                    pos - 1.0 + c * gain_vs_blur,
                    crt_raster_pos,
                    crt_raster_pos + 1.0 / fph,
                );
                let w_curr = get_overlap(
                    pos,
                    pos + c * gain_vs_blur,
                    crt_raster_pos,
                    crt_raster_pos + 1.0 / fph,
                );
                let sub_signal = (w_prev2 + w_prev1 + w_curr) * fph / gain_vs_blur;
                accum_signal += sub_signal / fph;
            }

            assert!(
                (accum_signal - c).abs() < 1e-5,
                "Overlap integral mismatch at pos={}: got {}, expected {}",
                pos,
                accum_signal,
                c
            );
        }
    }

    #[test]
    fn test_beam_shader_contains_all_renderer_uniforms() {
        assert!(
            FS_BLIT.contains("uniform sampler2D u_texture;"),
            "FS_BLIT must declare `u_texture`"
        );
        let required_uniforms = [
            "uniform sampler2D frame_curr;",
            "uniform sampler2D frame_prev1;",
            "uniform sampler2D frame_prev2;",
            "uniform vec2 outputResolution;",
            "uniform vec2 source_size;",
            "uniform vec3 background_color;",
            "uniform float horizontal_stretch;",
            "uniform float vibrance;",
            "uniform int scaler_filter;",
            "uniform vec4 border_crop;",
            "uniform int iFrame;",
            "uniform float frames_per_hz;",
            "uniform float gamma;",
            "uniform float gain_vs_blur;",
            "uniform float fps_divisor;",
            "uniform int lcd_anti_retention;",
            "uniform float lcd_inversion_compensation_slew;",
            "uniform int scan_direction;",
            "uniform int splitscreen;",
            "uniform float splitscreen_x;",
            "uniform float splitscreen_y;",
            "uniform int splitscreen_border_px;",
            "uniform int splitscreen_match_brightness;",
            "uniform float h_sharp;",
            "uniform float brightboost;",
        ];
        for decl in required_uniforms {
            assert!(
                FS_CRT_BEAM.contains(decl),
                "FS_CRT_BEAM is missing exact uniform declaration `{decl}`"
            );
        }

        let required_mask_uniforms = [
            "uniform sampler2D beam_texture;",
            "uniform vec2 outputResolution;",
            "uniform vec2 source_size;",
            "uniform vec3 background_color;",
            "uniform float horizontal_stretch;",
            "uniform vec4 border_crop;",
            "uniform float gamma;",
            "uniform float post_gain;",
            "uniform int curvature;",
            "uniform float warp_x;",
            "uniform float warp_y;",
            "uniform float corner_size;",
            "uniform float screen_scale;",
            "uniform float scanline_strength;",
            "uniform float beam_min;",
            "uniform float beam_max;",
            "uniform float shadow_mask;",
            "uniform float mask_strength;",
            "uniform float mask_size;",
            "uniform float mask_cutoff;",
            "uniform float slot_mask;",
            "uniform float double_slot;",
            "uniform float smooth_mask;",
            "uniform float halation;",
        ];
        for decl in required_mask_uniforms {
            assert!(
                FS_CRT_BEAM_MASK.contains(decl),
                "FS_CRT_BEAM_MASK is missing exact uniform declaration `{decl}`"
            );
        }
    }

    #[test]
    fn test_post_beam_mask_preserves_talbot_plateau_and_gamma_curve() {
        // Verify that applying Stage 2 (scanline profile + phosphor mask) in linear light
        // parameterized by steady-state cycle luminance `steady_luma` preserves:
        // 1. Talbot-Plateau vertical phase invariance (zero stationary horizontal banding across pos in [0, 1]).
        // 2. Macroscopic gamma curve across IRE levels (mean transmittance across a scanline + mask triad is 1.0).
        let frames_per_hz = 3; // 180Hz / 60Hz = 3 subframes per cycle
        let fph = frames_per_hz as f32;
        let gain_vs_blur = 0.65f32;
        let brightboost = 1.15f32;
        let eff_gain = (gain_vs_blur * brightboost).clamp(0.01, 1.0);
        let scanline_strength = 0.55f32;
        let beam_min = 1.35f32;
        let beam_max = 1.05f32;

        // Mean-normalized raised-cosine scanline profile matching `fs_crt_beam_mask.glsl`
        let eval_scanline_gain = |f_line: f32, steady_luma: f32| -> f32 {
            let luma = steady_luma.clamp(0.0, 1.0);
            let p = (beam_min + (beam_max - beam_min) * luma).clamp(0.30, 3.0);
            let cos_wave = 0.5 + 0.5 * (2.0 * std::f32::consts::PI * f_line).cos();
            let raw_profile = cos_wave.powf(p);
            let mean_profile = 1.0 / (1.0 + 3.0 * p + (0.14159265 * p * p) / (1.0 + p)).sqrt();
            let norm_profile = 1.0 + (raw_profile / mean_profile.max(0.25) - 1.0) * 0.65;
            1.0 + (norm_profile - 1.0) * scanline_strength
        };

        // Check across multiple IRE levels
        for ire_step in 1..=10 {
            let c_linear = (ire_step as f32 / 10.0).powf(2.4);
            let steady_luma = c_linear;

            // 1. Verify Talbot-Plateau phase invariance at a fixed sub-scanline position f_line = 0.25:
            // Changing macro vertical position `pos` changes how energy is split across subframes,
            // e.g. (1.0, 0.4, 0.0) vs (0.7, 0.7, 0.0), but the time-integrated linear light after Stage 2
            // must be identical for every `pos` in [0, 1] (zero horizontal phase banding).
            let f_line_test = 0.25f32;
            let scan_gain = eval_scanline_gain(f_line_test, steady_luma);
            let mut ref_integrated: Option<f32> = None;

            for y_step in 0..=20 {
                let pos = y_step as f32 / 20.0;
                let mut accum_linear = 0.0f32;
                for f in 0..frames_per_hz {
                    let crt_raster_pos = f as f32 / fph;
                    let w_prev2 = get_overlap(
                        pos - 2.0,
                        pos - 2.0 + c_linear * eff_gain,
                        crt_raster_pos,
                        crt_raster_pos + 1.0 / fph,
                    );
                    let w_prev1 = get_overlap(
                        pos - 1.0,
                        pos - 1.0 + c_linear * eff_gain,
                        crt_raster_pos,
                        crt_raster_pos + 1.0 / fph,
                    );
                    let w_curr = get_overlap(
                        pos,
                        pos + c_linear * eff_gain,
                        crt_raster_pos,
                        crt_raster_pos + 1.0 / fph,
                    );
                    // Stage 1 linear output
                    let stage1_linear = (w_prev2 + w_prev1 + w_curr) * fph;
                    // Stage 2 linear spatial filter
                    let stage2_linear = stage1_linear * scan_gain;
                    accum_linear += stage2_linear / fph;
                }
                if let Some(expected) = ref_integrated {
                    assert!(
                        (accum_linear - expected).abs() < 1e-5,
                        "Talbot-Plateau banding detected at pos={pos}: got {accum_linear}, expected {expected}"
                    );
                } else {
                    ref_integrated = Some(accum_linear);
                }
            }

            // 2. Verify gamma-curve preservation: spatial average across a scanline period f_line in [0, 1]
            // preserves the macroscopic linear luminance ratio `c_linear * eff_gain` within ~3%.
            let n_samples = 64;
            let mut line_avg_gain = 0.0f32;
            for i in 0..n_samples {
                let f_line = (i as f32 + 0.5) / n_samples as f32;
                line_avg_gain += eval_scanline_gain(f_line, steady_luma) / n_samples as f32;
            }
            assert!(
                (line_avg_gain - 1.0).abs() < 0.035,
                "Scanline profile distorted macroscopic gamma curve at c_linear={c_linear}: mean gain={line_avg_gain}"
            );
        }
    }
}
