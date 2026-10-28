use super::programs::VS_SRC;
use eframe::glow::{self, HasContext};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anime4kVariant {
    Small,
    Medium,
    Large,
}

struct CascadeData {
    fbos: [glow::Framebuffer; 12],
    texs: [glow::Texture; 12],
    input_size: (u32, u32),
}

impl CascadeData {
    unsafe fn new(gl: &glow::Context, width: u32, height: u32, num_passes: usize) -> Self {
        let texs: [glow::Texture; 12] = std::array::from_fn(|_| gl.create_texture().unwrap());
        let fbos: [glow::Framebuffer; 12] =
            std::array::from_fn(|_| gl.create_framebuffer().unwrap());

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
                    glow::RGBA32F as i32,
                    target_w as i32,
                    target_h as i32,
                    0,
                    glow::RGBA,
                    glow::FLOAT,
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
        for i in 0..12 {
            gl.delete_texture(self.texs[i]);
            gl.delete_framebuffer(self.fbos[i]);
        }
    }
}

struct PassUniforms {
    input_tex: Option<glow::UniformLocation>,
    main_tex: Option<glow::UniformLocation>,
    save_texs: Vec<Option<glow::UniformLocation>>,
}

pub struct Anime4kUpscaler {
    programs: Vec<glow::Program>,
    pass_uniforms: Vec<PassUniforms>,
    passes: usize,

    // Tracking intermediate sub-layers per recursion cycle independently
    cascades: Vec<CascadeData>,

    // Full quad geometry
    vao: glow::VertexArray,
    vbo: glow::Buffer,
}

impl Anime4kUpscaler {
    pub fn new(gl: &glow::Context, variant: Anime4kVariant) -> Self {
        let mut programs = Vec::new();
        let mut pass_uniforms = Vec::new();
        unsafe {
            // Pick corresponding sources
            let sources = match variant {
                Anime4kVariant::Small => vec![
                    include_str!("../shaders/fs_anime4k_small_in.glsl"),
                    include_str!("../shaders/fs_anime4k_small_conv2.glsl"),
                    include_str!("../shaders/fs_anime4k_small_conv3.glsl"),
                    include_str!("../shaders/fs_anime4k_small_conv4.glsl"),
                    include_str!("../shaders/fs_anime4k_small_out.glsl"),
                ],
                Anime4kVariant::Medium => vec![
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
                Anime4kVariant::Large => vec![
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

            for (i, src) in sources.into_iter().enumerate() {
                let prog = super::programs::compile_program(gl, VS_SRC, src);
                let input_tex = gl.get_uniform_location(prog, "input_tex");
                let main_tex = gl.get_uniform_location(prog, "MAIN");
                let save_texs = save_names
                    .iter()
                    .enumerate()
                    .map(|(pass_idx, name)| {
                        if pass_idx < i {
                            gl.get_uniform_location(prog, name)
                        } else {
                            None
                        }
                    })
                    .collect();
                programs.push(prog);
                pass_uniforms.push(PassUniforms {
                    input_tex,
                    main_tex,
                    save_texs,
                });
            }

            // Init Vertex Array for Fullscreen Quad
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

            Self {
                passes: programs.len(),
                programs,
                pass_uniforms,
                cascades: Vec::new(),
                vao,
                vbo,
            }
        }
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
            // Save state
            let mut current_fbo = [0; 1];
            gl.get_parameter_i32_slice(glow::DRAW_FRAMEBUFFER_BINDING, &mut current_fbo);

            let old_vao = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
            let blend_enabled = gl.is_enabled(glow::BLEND);
            gl.disable(glow::BLEND);

            gl.bind_vertex_array(Some(self.vao));

            let mut current_input = Some(input_tex);
            let mut pass_idx_loop = 0;

            // Ensure same hysteresis check is applied during cascades
            while current_w * 6 < target_width * 5 && current_h * 6 < target_height * 5 {
                if pass_idx_loop >= self.cascades.len() {
                    self.cascades
                        .push(CascadeData::new(gl, current_w, current_h, self.passes));
                } else if self.cascades[pass_idx_loop].input_size != (current_w, current_h) {
                    self.cascades[pass_idx_loop].setup_textures(
                        gl,
                        current_w,
                        current_h,
                        self.passes,
                    );
                }

                let cascade = &self.cascades[pass_idx_loop];
                let out_w = current_w * 2;
                let out_h = current_h * 2;

                for i in 0..self.passes {
                    let is_last = i == self.passes - 1;

                    // Each pass writes to its own unique FBO index inside the loop cycle
                    let target_fbo = cascade.fbos[i];
                    let mut target_w = current_w;
                    let mut target_h = current_h;
                    let target_tex = cascade.texs[i];

                    gl.bind_texture(glow::TEXTURE_2D, Some(target_tex));

                    if is_last {
                        target_w = out_w;
                        target_h = out_h;
                    }

                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target_fbo));

                    gl.viewport(0, 0, target_w as i32, target_h as i32);

                    gl.use_program(Some(self.programs[i]));

                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, current_input);

                    // Anime4K requires the original "MAIN" input texture bound to MAIN_tex for residuals/skip-connections
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

                    let uniforms = &self.pass_uniforms[i];
                    if let Some(ref l) = uniforms.input_tex {
                        gl.uniform_1_i32(Some(l), 1);
                    }

                    let mut bind_unit = 2; // TEXTURE2
                    for (pass_idx, loc) in uniforms.save_texs.iter().enumerate() {
                        if let Some(ref l) = loc {
                            // The texture for this step was computed on pass iteration `pass_idx`
                            gl.active_texture(glow::TEXTURE0 + bind_unit);
                            gl.bind_texture(glow::TEXTURE_2D, Some(cascade.texs[pass_idx]));
                            gl.uniform_1_i32(Some(l), bind_unit as i32);
                            bind_unit += 1;
                        }
                    }

                    if let Some(ref l) = uniforms.main_tex {
                        gl.uniform_1_i32(Some(l), 1);
                    }

                    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

                    // Output of this pass becomes input of the next
                    current_input = Some(cascade.texs[i]);
                }

                input_tex = cascade.texs[self.passes - 1];
                current_w = out_w;
                current_h = out_h;
                pass_idx_loop += 1;
                current_input = Some(input_tex);
            }

            // Fix: ensure the final dynamically cascaded FBO texture returned for downstream passes
            // operates gracefully with LINEAR interpolation. Otherwise nearest-neighbor on high
            // frequency CNN residuals introduces visual grid/checkering under 4K window scalings.
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
            gl.bind_texture(glow::TEXTURE_2D, None);

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
        unsafe {
            for prog in &self.programs {
                gl.delete_program(*prog);
            }
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
            for cascade in &self.cascades {
                cascade.destroy(gl);
            }
        }
    }
}
