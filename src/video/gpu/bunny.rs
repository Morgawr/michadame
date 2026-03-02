use eframe::glow::{self, HasContext};
use super::programs::compile_compute_program;
use crate::video::types::ScalerFilter;
use std::collections::HashMap;

struct Stage {
    program: glow::Program,
    input_size_loc: Option<glow::UniformLocation>,
    is_shuffle: bool,
}

struct BunnyVariant {
    stages: Vec<Stage>,
}

struct PassData {
    intermediate_textures: [glow::Texture; 2], // Standardized ping-pong (max 12ch = 3x size)
    output_texture: glow::Texture,
    input_size: (u32, u32),
}

impl PassData {
    unsafe fn new(gl: &glow::Context, width: u32, height: u32) -> Self {
        let intermediate_textures = [
            gl.create_texture().unwrap(),
            gl.create_texture().unwrap(),
        ];
        let output_texture = gl.create_texture().unwrap();
        
        let data = Self {
            intermediate_textures,
            output_texture,
            input_size: (0, 0),
        };
        data.setup_textures(gl, width, height);
        data
    }

    unsafe fn setup_textures(&self, gl: &glow::Context, width: u32, height: u32) {
        // Intermediate textures: Always 3x width to handle up to 12 channels
        for tex in self.intermediate_textures {
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, (width * 3) as i32, height as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, None);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        }

        // Output: 2x width, 2x height
        gl.bind_texture(glow::TEXTURE_2D, Some(self.output_texture));
        gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, (width * 2) as i32, (height * 2) as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, None);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);

        gl.bind_texture(glow::TEXTURE_2D, None);
    }

    unsafe fn destroy(&self, gl: &glow::Context) {
        for tex in self.intermediate_textures {
            gl.delete_texture(tex);
        }
        gl.delete_texture(self.output_texture);
    }
}

pub struct BunnyUpscaler {
    variants: HashMap<ScalerFilter, BunnyVariant>,
    passes: Vec<PassData>,
}

impl BunnyUpscaler {
    pub fn new(gl: &glow::Context) -> Self {
        unsafe {
            let mut variants = HashMap::new();

            // FAST Variant
            variants.insert(ScalerFilter::BuNNy, BunnyVariant {
                stages: vec![
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_soft_fast_in.glsl"), false),
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_soft_fast_conv1.glsl"), false),
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_soft_fast_conv2.glsl"), false),
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_soft_fast_out.glsl"), true),
                ]
            });

            // MEDIUM Variant
            variants.insert(ScalerFilter::BuNNyMedium, BunnyVariant {
                stages: vec![
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_fast_2x_in.glsl"), false),
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_fast_2x_conv1.glsl"), false),
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_fast_2x_conv2.glsl"), false),
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_fast_2x_out.glsl"), true),
                ]
            });

            // HIGH Variant
            variants.insert(ScalerFilter::BuNNyHigh, BunnyVariant {
                stages: vec![
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_soft_high_in.glsl"), false),
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_soft_high_conv1.glsl"), false),
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_soft_high_conv2.glsl"), false),
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_soft_high_conv3.glsl"), false),
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_soft_high_conv4.glsl"), false),
                    Self::create_stage(gl, include_str!("../shaders/cs_bunny_soft_high_out.glsl"), true),
                ]
            });

            Self {
                variants,
                passes: Vec::new(),
            }
        }
    }

    unsafe fn create_stage(gl: &glow::Context, source: &str, is_shuffle: bool) -> Stage {
        let program = compile_compute_program(gl, source);
        let input_size_loc = gl.get_uniform_location(program, "input_size");
        Stage {
            program,
            input_size_loc,
            is_shuffle,
        }
    }

    pub unsafe fn upscale(
        &mut self,
        gl: &glow::Context,
        input_tex: glow::Texture,
        width: u32,
        height: u32,
        target_width: u32,
        target_height: u32,
        scaler_filter: ScalerFilter,
    ) -> (glow::Texture, u32, u32) {
        let variant = match self.variants.get(&scaler_filter) {
            Some(v) => v,
            None => {
                // Fallback to BuNNy (Fast) if requested variant not initialized
                self.variants.get(&ScalerFilter::BuNNy).unwrap()
            }
        };

        let mut curr_width = width;
        let mut curr_height = height;
        let mut curr_tex = input_tex;
        
        let mut pass_idx = 0;

        while curr_width < target_width && curr_height < target_height {
            // Ensure pass data exists and is correctly sized
            if pass_idx >= self.passes.len() {
                self.passes.push(PassData::new(gl, curr_width, curr_height));
            } else if self.passes[pass_idx].input_size != (curr_width, curr_height) {
                self.passes[pass_idx].setup_textures(gl, curr_width, curr_height);
                self.passes[pass_idx].input_size = (curr_width, curr_height);
            }
            
            let pass = &self.passes[pass_idx];
            let mut last_tex = curr_tex;

            for (s_idx, stage) in variant.stages.iter().enumerate() {
                gl.use_program(Some(stage.program));
                
                // Bind input
                gl.bind_image_texture(0, last_tex, 0, false, 0, glow::READ_ONLY, glow::RGBA8);
                
                // Bind output
                let output_tex = if stage.is_shuffle {
                    pass.output_texture
                } else {
                    pass.intermediate_textures[s_idx % 2]
                };
                gl.bind_image_texture(1, output_tex, 0, false, 0, glow::WRITE_ONLY, glow::RGBA8);

                // Bind original source and set uniforms for shuffle stage
                if stage.is_shuffle {
                    gl.active_texture(glow::TEXTURE2);
                    gl.bind_texture(glow::TEXTURE_2D, Some(curr_tex));
                    // Note: Shaders expect source_tex at binding 2 (sampler)
                    if let Some(loc) = gl.get_uniform_location(stage.program, "source_tex") {
                        gl.uniform_1_i32(Some(&loc), 2);
                    }
                }

                if let Some(loc) = stage.input_size_loc.as_ref() {
                    gl.uniform_2_f32(Some(loc), curr_width as f32, curr_height as f32);
                }

                // Dispatch
                gl.dispatch_compute((curr_width + 7) / 8, (curr_height + 7) / 8, 1);
                gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT);

                last_tex = output_tex;
            }

            curr_tex = pass.output_texture;
            curr_width *= 2;
            curr_height *= 2;
            pass_idx += 1;
        }

        gl.use_program(None);
        (curr_tex, curr_width, curr_height)
    }

    pub fn get_upscaled_size(&self, width: u32, height: u32, target_width: u32, target_height: u32) -> (u32, u32) {
        let mut curr_width = width;
        let mut curr_height = height;
        while curr_width < target_width && curr_height < target_height {
            curr_width *= 2;
            curr_height *= 2;
        }
        (curr_width, curr_height)
    }

    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            for variant in self.variants.values() {
                for stage in &variant.stages {
                    gl.delete_program(stage.program);
                }
            }
            for pass in &self.passes {
                pass.destroy(gl);
            }
        }
    }
}
