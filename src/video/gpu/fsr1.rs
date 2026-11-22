use super::programs::VS_SRC;
use eframe::glow::{self, HasContext};

/// Default RCAS sharpening intensity on a 0.0..=1.0 scale (~AMD's 0.2-stop recommendation).
pub const DEFAULT_FSR1_SHARPNESS: f32 = 0.85;

/// Live execution telemetry recorded whenever FSR 1.0 runs on the GPU.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fsr1RunInfo {
    pub input_size: (u32, u32),
    pub output_size: (u32, u32),
    pub easu_ran: bool,
    pub rcas_sharpness: f32,
    pub run_count: u64,
}

struct Fsr1Gpu {
    easu_prog: glow::Program,
    easu_output_size_loc: Option<glow::UniformLocation>,
    rcas_prog: glow::Program,
    rcas_sharpness_loc: Option<glow::UniformLocation>,
    rcas_input_is_linear_loc: Option<glow::UniformLocation>,
    easu_fbo: glow::Framebuffer,
    easu_tex: glow::Texture,
    rcas_fbo: glow::Framebuffer,
    rcas_tex: glow::Texture,
    output_size: (u32, u32),
    vao: glow::VertexArray,
    vbo: glow::Buffer,
}

impl Fsr1Gpu {
    unsafe fn new(gl: &glow::Context) -> Self {
        let easu_src = include_str!("../shaders/fs_fsr1_easu.glsl");
        let rcas_src = include_str!("../shaders/fs_fsr1_rcas.glsl");

        let easu_prog = super::programs::compile_program(gl, VS_SRC, easu_src);
        gl.use_program(Some(easu_prog));
        if let Some(loc) = gl.get_uniform_location(easu_prog, "input_tex") {
            gl.uniform_1_i32(Some(&loc), 1);
        }
        let easu_output_size_loc = gl.get_uniform_location(easu_prog, "output_size");

        let rcas_prog = super::programs::compile_program(gl, VS_SRC, rcas_src);
        gl.use_program(Some(rcas_prog));
        if let Some(loc) = gl.get_uniform_location(rcas_prog, "input_tex") {
            gl.uniform_1_i32(Some(&loc), 1);
        }
        let rcas_sharpness_loc = gl.get_uniform_location(rcas_prog, "rcas_sharpness");
        let rcas_input_is_linear_loc = gl.get_uniform_location(rcas_prog, "input_is_linear");
        gl.use_program(None);

        let easu_tex = gl.create_texture().unwrap();
        let easu_fbo = gl.create_framebuffer().unwrap();
        let rcas_tex = gl.create_texture().unwrap();
        let rcas_fbo = gl.create_framebuffer().unwrap();

        let vao = gl.create_vertex_array().unwrap();
        let vbo = gl.create_buffer().unwrap();

        gl.bind_vertex_array(Some(vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));

        let sq: [f32; 16] = [
            -1.0, -1.0, 0.0, 0.0, 1.0, -1.0, 1.0, 0.0, -1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0,
        ];
        gl.buffer_data_u8_slice(
            glow::ARRAY_BUFFER,
            bytemuck::cast_slice(&sq),
            glow::STATIC_DRAW,
        );

        let stride = 4 * std::mem::size_of::<f32>() as i32;
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(
            1,
            2,
            glow::FLOAT,
            false,
            stride,
            2 * std::mem::size_of::<f32>() as i32,
        );
        gl.enable_vertex_attrib_array(1);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.bind_vertex_array(None);

        Self {
            easu_prog,
            easu_output_size_loc,
            rcas_prog,
            rcas_sharpness_loc,
            rcas_input_is_linear_loc,
            easu_fbo,
            easu_tex,
            rcas_fbo,
            rcas_tex,
            output_size: (0, 0),
            vao,
            vbo,
        }
    }

    unsafe fn ensure_targets(&mut self, gl: &glow::Context, out_w: u32, out_h: u32) {
        if self.output_size == (out_w, out_h) {
            return;
        }

        for &(tex, fbo) in &[
            (self.easu_tex, self.easu_fbo),
            (self.rcas_tex, self.rcas_fbo),
        ] {
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA16F as i32,
                out_w as i32,
                out_h as i32,
                0,
                glow::RGBA,
                glow::HALF_FLOAT,
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

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(tex),
                0,
            );
        }

        self.output_size = (out_w, out_h);
    }

    unsafe fn destroy(&self, gl: &glow::Context) {
        gl.delete_program(self.easu_prog);
        gl.delete_program(self.rcas_prog);
        gl.delete_texture(self.easu_tex);
        gl.delete_framebuffer(self.easu_fbo);
        gl.delete_texture(self.rcas_tex);
        gl.delete_framebuffer(self.rcas_fbo);
        gl.delete_vertex_array(self.vao);
        gl.delete_buffer(self.vbo);
    }
}

pub struct Fsr1Upscaler {
    gpu: Option<Fsr1Gpu>,
    last_run: Option<Fsr1RunInfo>,
    run_count: u64,
}

impl Fsr1Upscaler {
    pub fn new(_gl: &glow::Context) -> Self {
        Self {
            gpu: None,
            last_run: None,
            run_count: 0,
        }
    }

    pub fn last_run(&self) -> Option<Fsr1RunInfo> {
        self.last_run
    }

    pub fn get_upscaled_size(
        &self,
        width: u32,
        height: u32,
        target_width: u32,
        target_height: u32,
    ) -> (u32, u32) {
        super::geometry::fsr1_size((width, height), (target_width, target_height))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upscale(
        &mut self,
        gl: &glow::Context,
        input_tex: glow::Texture,
        width: u32,
        height: u32,
        target_width: u32,
        target_height: u32,
        sharpness: f32,
    ) -> (glow::Texture, u32, u32) {
        let (out_w, out_h) = self.get_upscaled_size(width, height, target_width, target_height);
        if out_w == 0 || out_h == 0 || width == 0 || height == 0 {
            return (input_tex, width, height);
        }

        let run_easu = out_w > width || out_h > height;
        let rcas_sharpness = if sharpness.is_finite() {
            sharpness.clamp(0.0, 1.0)
        } else {
            DEFAULT_FSR1_SHARPNESS
        };

        unsafe {
            // Save caller GL state before any lazy initialization or target allocation.
            let mut current_fbo = [0i32; 1];
            gl.get_parameter_i32_slice(glow::DRAW_FRAMEBUFFER_BINDING, &mut current_fbo);
            let old_vao = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
            let mut old_viewport = [0i32; 4];
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut old_viewport);
            let blend_enabled = gl.is_enabled(glow::BLEND);
            gl.disable(glow::BLEND);

            let gpu = self.gpu.get_or_insert_with(|| Fsr1Gpu::new(gl));
            gpu.ensure_targets(gl, out_w, out_h);

            gl.bind_vertex_array(Some(gpu.vao));
            gl.viewport(0, 0, out_w as i32, out_h as i32);

            let rcas_input_tex = if run_easu {
                // Pass 1: EASU (linear RGB -> perceptual sRGB at output resolution)
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(gpu.easu_fbo));
                gl.use_program(Some(gpu.easu_prog));
                gl.uniform_2_f32(
                    gpu.easu_output_size_loc.as_ref(),
                    out_w as f32,
                    out_h as f32,
                );
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, Some(input_tex));
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                gpu.easu_tex
            } else {
                input_tex
            };

            // Pass 2: RCAS (perceptual sRGB sharpening -> linear RGB output)
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(gpu.rcas_fbo));
            gl.use_program(Some(gpu.rcas_prog));
            gl.uniform_1_f32(gpu.rcas_sharpness_loc.as_ref(), rcas_sharpness);
            gl.uniform_1_i32(
                gpu.rcas_input_is_linear_loc.as_ref(),
                if run_easu { 0 } else { 1 },
            );
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(rcas_input_tex));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            // Clean up texture bindings and restore caller GL state.
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.use_program(None);

            if current_fbo[0] == 0 {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            } else {
                gl.bind_framebuffer(
                    glow::FRAMEBUFFER,
                    Some(glow::Framebuffer::from(glow::NativeFramebuffer(
                        std::num::NonZero::new(current_fbo[0] as u32).unwrap(),
                    ))),
                );
            }

            if old_vao == 0 {
                gl.bind_vertex_array(None);
            } else {
                gl.bind_vertex_array(Some(glow::VertexArray::from(glow::NativeVertexArray(
                    std::num::NonZero::new(old_vao as u32).unwrap(),
                ))));
            }

            gl.viewport(
                old_viewport[0],
                old_viewport[1],
                old_viewport[2],
                old_viewport[3],
            );

            if blend_enabled {
                gl.enable(glow::BLEND);
            }

            self.run_count = self.run_count.wrapping_add(1);
            self.last_run = Some(Fsr1RunInfo {
                input_size: (width, height),
                output_size: (out_w, out_h),
                easu_ran: run_easu,
                rcas_sharpness,
                run_count: self.run_count,
            });

            (gpu.rcas_tex, out_w, out_h)
        }
    }

    pub fn destroy(&self, gl: &glow::Context) {
        if let Some(ref gpu) = self.gpu {
            unsafe {
                gpu.destroy(gl);
            }
        }
    }
}
