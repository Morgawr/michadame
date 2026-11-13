use super::programs::VS_SRC;
use eframe::glow::{self, HasContext};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anime4kVariant {
    Small,
    Medium,
    Large,
}

struct CascadeData {
    fbos: Vec<glow::Framebuffer>,
    texs: Vec<glow::Texture>,
    input_size: (u32, u32),
}

impl CascadeData {
    unsafe fn new(gl: &glow::Context, width: u32, height: u32, num_passes: usize) -> Self {
        let texs: Vec<glow::Texture> = (0..num_passes)
            .map(|_| gl.create_texture().unwrap())
            .collect();
        let fbos: Vec<glow::Framebuffer> = (0..num_passes)
            .map(|_| gl.create_framebuffer().unwrap())
            .collect();

        let mut data = Self {
            fbos,
            texs,
            input_size: (0, 0),
        };
        data.setup_textures(gl, width, height, num_passes);
        data
    }

    unsafe fn setup_textures(
        &mut self,
        gl: &glow::Context,
        width: u32,
        height: u32,
        num_passes: usize,
    ) {
        let out_w = width * 2;
        let out_h = height * 2;

        for i in 0..num_passes {
            let is_last = i == num_passes - 1;
            let (target_w, target_h) = if is_last {
                (out_w, out_h)
            } else {
                (width, height)
            };

            gl.bind_texture(glow::TEXTURE_2D, Some(self.texs[i]));
            if is_last {
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    target_w as i32,
                    target_h as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    None,
                );
            } else {
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA16F as i32,
                    target_w as i32,
                    target_h as i32,
                    0,
                    glow::RGBA,
                    glow::HALF_FLOAT,
                    None,
                );
            }
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
                Some(self.texs[i]),
                0,
            );
        }

        self.input_size = (width, height);
    }

    unsafe fn destroy(&self, gl: &glow::Context) {
        for &tex in &self.texs {
            gl.delete_texture(tex);
        }
        for &fbo in &self.fbos {
            gl.delete_framebuffer(fbo);
        }
    }
}

struct PassUniforms {
    input_tex: Option<glow::UniformLocation>,
    main_tex: Option<glow::UniformLocation>,
    save_bindings: Vec<(u32, usize)>,
}

struct Anime4kGpu {
    programs: Vec<glow::Program>,
    pass_uniforms: Vec<PassUniforms>,
    passes: usize,
    cascades: Vec<CascadeData>,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
}

impl Anime4kGpu {
    unsafe fn new(gl: &glow::Context, variant: Anime4kVariant) -> Self {
        let sources: &[&str] = match variant {
            Anime4kVariant::Small => &[
                include_str!("../shaders/fs_anime4k_small_in.glsl"),
                include_str!("../shaders/fs_anime4k_small_conv2.glsl"),
                include_str!("../shaders/fs_anime4k_small_conv3.glsl"),
                include_str!("../shaders/fs_anime4k_small_conv4.glsl"),
                include_str!("../shaders/fs_anime4k_small_out.glsl"),
            ],
            Anime4kVariant::Medium => &[
                include_str!("../shaders/fs_anime4k_medium_in.glsl"),
                include_str!("../shaders/fs_anime4k_medium_conv2.glsl"),
                include_str!("../shaders/fs_anime4k_medium_conv3.glsl"),
                include_str!("../shaders/fs_anime4k_medium_conv4.glsl"),
                include_str!("../shaders/fs_anime4k_medium_conv5.glsl"),
                include_str!("../shaders/fs_anime4k_medium_conv6.glsl"),
                include_str!("../shaders/fs_anime4k_medium_conv7.glsl"),
                include_str!("../shaders/fs_anime4k_medium_conv8.glsl"),
                include_str!("../shaders/fs_anime4k_medium_out.glsl"),
            ],
            Anime4kVariant::Large => &[
                include_str!("../shaders/fs_anime4k_large_in.glsl"),
                include_str!("../shaders/fs_anime4k_large_conv2.glsl"),
                include_str!("../shaders/fs_anime4k_large_conv3.glsl"),
                include_str!("../shaders/fs_anime4k_large_conv4.glsl"),
                include_str!("../shaders/fs_anime4k_large_conv5.glsl"),
                include_str!("../shaders/fs_anime4k_large_conv6.glsl"),
                include_str!("../shaders/fs_anime4k_large_conv7.glsl"),
                include_str!("../shaders/fs_anime4k_large_conv8.glsl"),
                include_str!("../shaders/fs_anime4k_large_conv9.glsl"),
                include_str!("../shaders/fs_anime4k_large_out.glsl"),
            ],
        };

        let save_names: &[&str] = match variant {
            Anime4kVariant::Small => &[
                "conv2d_tf",
                "conv2d_1_tf",
                "conv2d_2_tf",
                "conv2d_last_tf",
                "MAIN",
            ],
            Anime4kVariant::Medium => &[
                "conv2d_tf",
                "conv2d_1_tf",
                "conv2d_2_tf",
                "conv2d_3_tf",
                "conv2d_4_tf",
                "conv2d_5_tf",
                "conv2d_6_tf",
                "conv2d_last_tf",
                "MAIN",
            ],
            Anime4kVariant::Large => &[
                "conv2d_tf",
                "conv2d_tf1",
                "conv2d_1_tf",
                "conv2d_1_tf1",
                "conv2d_2_tf",
                "conv2d_2_tf1",
                "conv2d_last_tf",
                "conv2d_last_tf1",
                "conv2d_last_tf2",
                "MAIN",
            ],
        };

        let mut programs = Vec::with_capacity(sources.len());
        let mut pass_uniforms = Vec::with_capacity(sources.len());

        for (i, &src) in sources.iter().enumerate() {
            let prog = super::programs::compile_program(gl, VS_SRC, src);
            let input_tex = gl.get_uniform_location(prog, "input_tex");
            let main_tex = gl.get_uniform_location(prog, "MAIN");

            gl.use_program(Some(prog));
            if let Some(ref l) = input_tex {
                gl.uniform_1_i32(Some(l), 1);
            }
            if let Some(ref l) = main_tex {
                gl.uniform_1_i32(Some(l), 1);
            }

            let mut bind_unit = 2u32;
            let mut save_bindings = Vec::new();
            for (pass_idx, name) in save_names.iter().enumerate().take(i) {
                if let Some(loc) = gl.get_uniform_location(prog, name) {
                    gl.uniform_1_i32(Some(&loc), bind_unit as i32);
                    save_bindings.push((bind_unit, pass_idx));
                    bind_unit += 1;
                }
            }

            programs.push(prog);
            pass_uniforms.push(PassUniforms {
                input_tex,
                main_tex,
                save_bindings,
            });
        }
        gl.use_program(None);

        let vao = gl.create_vertex_array().unwrap();
        let vbo = gl.create_buffer().unwrap();

        gl.bind_vertex_array(Some(vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));

        let sq: [f32; 16] = [
            -1.0, -1.0, 0.0, 0.0, 1.0, -1.0, 1.0, 0.0, -1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0,
        ];

        let sq_u8: &[u8] = bytemuck::cast_slice(&sq);
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, sq_u8, glow::STATIC_DRAW);

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
            passes: programs.len(),
            programs,
            pass_uniforms,
            cascades: Vec::new(),
            vao,
            vbo,
        }
    }

    unsafe fn destroy(&self, gl: &glow::Context) {
        for &prog in &self.programs {
            gl.delete_program(prog);
        }
        gl.delete_vertex_array(self.vao);
        gl.delete_buffer(self.vbo);
        for cascade in &self.cascades {
            cascade.destroy(gl);
        }
    }
}

pub struct Anime4kUpscaler {
    variant: Anime4kVariant,
    gpu: Option<Anime4kGpu>,
}

impl Anime4kUpscaler {
    pub fn new(_gl: &glow::Context, variant: Anime4kVariant) -> Self {
        Self { variant, gpu: None }
    }

    pub fn get_upscaled_size(
        &self,
        width: u32,
        height: u32,
        target_width: u32,
        target_height: u32,
    ) -> (u32, u32) {
        super::geometry::anime4k_size((width, height), (target_width, target_height))
    }

    pub fn upscale(
        &mut self,
        gl: &glow::Context,
        mut input_tex: glow::Texture,
        width: u32,
        height: u32,
        target_width: u32,
        target_height: u32,
    ) -> (glow::Texture, u32, u32) {
        let mut current_w = width;
        let mut current_h = height;

        unsafe {
            let variant = self.variant;
            let gpu = self.gpu.get_or_insert_with(|| Anime4kGpu::new(gl, variant));

            // Save state
            let mut current_fbo = [0; 1];
            gl.get_parameter_i32_slice(glow::DRAW_FRAMEBUFFER_BINDING, &mut current_fbo);

            let old_vao = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
            let blend_enabled = gl.is_enabled(glow::BLEND);
            gl.disable(glow::BLEND);

            gl.bind_vertex_array(Some(gpu.vao));

            // Configure initial source texture sampling state once outside the pass loop
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(input_tex));
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

            let mut pass_idx_loop = 0;

            // Ensure same hysteresis check is applied during cascades
            while current_w * 6 < target_width * 5 && current_h * 6 < target_height * 5 {
                if pass_idx_loop >= gpu.cascades.len() {
                    gpu.cascades
                        .push(CascadeData::new(gl, current_w, current_h, gpu.passes));
                } else if gpu.cascades[pass_idx_loop].input_size != (current_w, current_h) {
                    gpu.cascades[pass_idx_loop]
                        .setup_textures(gl, current_w, current_h, gpu.passes);
                }

                let cascade = &gpu.cascades[pass_idx_loop];
                let out_w = current_w * 2;
                let out_h = current_h * 2;

                for i in 0..gpu.passes {
                    let is_last = i == gpu.passes - 1;
                    let (target_w, target_h) = if is_last {
                        (out_w, out_h)
                    } else {
                        (current_w, current_h)
                    };

                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(cascade.fbos[i]));
                    gl.viewport(0, 0, target_w as i32, target_h as i32);
                    gl.use_program(Some(gpu.programs[i]));

                    let uniforms = &gpu.pass_uniforms[i];
                    if uniforms.input_tex.is_some() || uniforms.main_tex.is_some() {
                        gl.active_texture(glow::TEXTURE1);
                        gl.bind_texture(glow::TEXTURE_2D, Some(input_tex));
                    }

                    for &(bind_unit, pass_idx) in &uniforms.save_bindings {
                        gl.active_texture(glow::TEXTURE0 + bind_unit);
                        gl.bind_texture(glow::TEXTURE_2D, Some(cascade.texs[pass_idx]));
                    }

                    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                }

                input_tex = cascade.texs[gpu.passes - 1];
                current_w = out_w;
                current_h = out_h;
                pass_idx_loop += 1;
            }

            for unit in (0..=8).rev() {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
            gl.use_program(None);

            // Restore FBO state
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

            if blend_enabled {
                gl.enable(glow::BLEND);
            }

            (input_tex, current_w, current_h)
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
