use eframe::glow::{self, HasContext};
use super::programs::*;

/// GPU-based 2D FFT filter with interactive mask editing.
/// Operates on decoded RGB textures: forward FFT → apply mask → inverse FFT.
/// Pads to power-of-2 internally and crops back after.
#[allow(dead_code)]
pub struct FftFilter {
    // Shader programs
    init_prog: glow::Program,
    butterfly_prog: glow::Program,
    mask_prog: glow::Program,
    extract_prog: glow::Program,
    spectrum_prog: glow::Program,
    blit_prog: glow::Program,
    bitrev_prog: glow::Program,

    // Uniform locations (Option to handle driver optimizations)
    init_input_loc: Option<glow::UniformLocation>,
    init_fft_size_loc: Option<glow::UniformLocation>,
    init_orig_size_loc: Option<glow::UniformLocation>,

    butterfly_input_loc: Option<glow::UniformLocation>,
    butterfly_axis_loc: Option<glow::UniformLocation>,
    butterfly_stage_loc: Option<glow::UniformLocation>,
    butterfly_direction_loc: Option<glow::UniformLocation>,
    butterfly_fft_size_loc: Option<glow::UniformLocation>,

    mask_fft_loc: Option<glow::UniformLocation>,
    mask_mask_loc: Option<glow::UniformLocation>,
    mask_fft_size_loc: Option<glow::UniformLocation>,
    mask_threshold_loc: Option<glow::UniformLocation>,

    extract_ifft_loc: Option<glow::UniformLocation>,
    extract_orig_loc: Option<glow::UniformLocation>,
    extract_fft_size_loc: Option<glow::UniformLocation>,
    extract_orig_size_loc: Option<glow::UniformLocation>,
    extract_black_threshold_loc: Option<glow::UniformLocation>,

    spectrum_fft_loc: Option<glow::UniformLocation>,
    spectrum_mask_loc: Option<glow::UniformLocation>,
    spectrum_fft_size_loc: Option<glow::UniformLocation>,

    bitrev_input_loc: Option<glow::UniformLocation>,
    bitrev_fft_size_loc: Option<glow::UniformLocation>,

    // Two textures for ping-pong (RGBA32F for complex data)
    // FBOs are permanently attached to their respective textures
    tex: [glow::Texture; 2],
    fbo: [glow::Framebuffer; 2],

    // Output texture and FBO (RGBA8, original resolution)
    output_tex: glow::Texture,
    output_fbo: glow::Framebuffer,

    // Spectrum visualization texture and FBO
    spectrum_tex: glow::Texture,
    spectrum_fbo: glow::Framebuffer,

    // Mask texture (R8, power-of-2 resolution)
    mask_tex: glow::Texture,

    // Geometry
    vertex_array: glow::VertexArray,
    vbo: glow::Buffer,

    // State tracking
    last_fft_size: (u32, u32),
    last_orig_size: (u32, u32),
}

fn next_power_of_2(n: u32) -> u32 {
    if n == 0 { return 1; }
    let mut v = n - 1;
    v |= v >> 1;
    v |= v >> 2;
    v |= v >> 4;
    v |= v >> 8;
    v |= v >> 16;
    v + 1
}

fn log2_u32(n: u32) -> u32 {
    31 - n.leading_zeros()
}

impl FftFilter {
    pub fn new(gl: &glow::Context) -> Self {
        unsafe {
            let init_prog = compile_program(gl, VS_SRC, FS_FFT_INIT);
            let butterfly_prog = compile_program(gl, VS_SRC, FS_FFT_BUTTERFLY);
            let mask_prog = compile_program(gl, VS_SRC, FS_FFT_MASK);
            let extract_prog = compile_program(gl, VS_SRC, FS_FFT_EXTRACT);
            let spectrum_prog = compile_program(gl, VS_SRC, FS_FFT_SPECTRUM);

            // Simple blit shader for rendering spectrum to screen
            let blit_fs = r#"#version 330 core
                in vec2 v_tc;
                out vec4 fragColor;
                uniform sampler2D tex;
                void main() {
                    fragColor = texture(tex, v_tc);
                }
            "#;
            let blit_prog = compile_program(gl, VS_SRC, blit_fs);
            gl.use_program(Some(blit_prog));
            let blit_tex_loc = gl.get_uniform_location(blit_prog, "tex");
            gl.uniform_1_i32(blit_tex_loc.as_ref(), 0);

            // Bit-reverse permutation shader
            let bitrev_prog = compile_program(gl, VS_SRC, FS_FFT_BITREV);
            let bitrev_input_loc = gl.get_uniform_location(bitrev_prog, "input_texture");
            let bitrev_fft_size_loc = gl.get_uniform_location(bitrev_prog, "fft_size");
            gl.use_program(Some(bitrev_prog));
            gl.uniform_1_i32(bitrev_input_loc.as_ref(), 0);

            // Get uniform locations (no unwrap — driver may optimize away unused uniforms)
            let init_input_loc = gl.get_uniform_location(init_prog, "input_texture");
            let init_fft_size_loc = gl.get_uniform_location(init_prog, "fft_size");
            let init_orig_size_loc = gl.get_uniform_location(init_prog, "orig_size");
            gl.use_program(Some(init_prog));
            gl.uniform_1_i32(init_input_loc.as_ref(), 0);

            let butterfly_input_loc = gl.get_uniform_location(butterfly_prog, "input_texture");
            let butterfly_axis_loc = gl.get_uniform_location(butterfly_prog, "axis");
            let butterfly_stage_loc = gl.get_uniform_location(butterfly_prog, "stage");
            let butterfly_direction_loc = gl.get_uniform_location(butterfly_prog, "direction");
            let butterfly_fft_size_loc = gl.get_uniform_location(butterfly_prog, "fft_size");
            gl.use_program(Some(butterfly_prog));
            gl.uniform_1_i32(butterfly_input_loc.as_ref(), 0);

            let mask_fft_loc = gl.get_uniform_location(mask_prog, "fft_texture");
            let mask_mask_loc = gl.get_uniform_location(mask_prog, "mask_texture");
            let mask_fft_size_loc = gl.get_uniform_location(mask_prog, "fft_size");
            let mask_threshold_loc = gl.get_uniform_location(mask_prog, "mask_threshold");
            gl.use_program(Some(mask_prog));
            gl.uniform_1_i32(mask_fft_loc.as_ref(), 0);
            gl.uniform_1_i32(mask_mask_loc.as_ref(), 1);

            let extract_ifft_loc = gl.get_uniform_location(extract_prog, "ifft_texture");
            let extract_orig_loc = gl.get_uniform_location(extract_prog, "original_texture");
            let extract_fft_size_loc = gl.get_uniform_location(extract_prog, "fft_size");
            let extract_orig_size_loc = gl.get_uniform_location(extract_prog, "orig_size");
            let extract_black_threshold_loc = gl.get_uniform_location(extract_prog, "black_threshold");
            gl.use_program(Some(extract_prog));
            gl.uniform_1_i32(extract_ifft_loc.as_ref(), 0);
            gl.uniform_1_i32(extract_orig_loc.as_ref(), 1);

            let spectrum_fft_loc = gl.get_uniform_location(spectrum_prog, "fft_texture");
            let spectrum_mask_loc = gl.get_uniform_location(spectrum_prog, "mask_texture");
            let spectrum_fft_size_loc = gl.get_uniform_location(spectrum_prog, "fft_size");
            gl.use_program(Some(spectrum_prog));
            gl.uniform_1_i32(spectrum_fft_loc.as_ref(), 0);
            gl.uniform_1_i32(spectrum_mask_loc.as_ref(), 1);

            gl.use_program(None);

            // Create textures and FBOs
            let tex = [gl.create_texture().unwrap(), gl.create_texture().unwrap()];
            let fbo = [gl.create_framebuffer().unwrap(), gl.create_framebuffer().unwrap()];
            let output_tex = gl.create_texture().unwrap();
            let output_fbo = gl.create_framebuffer().unwrap();
            let spectrum_tex = gl.create_texture().unwrap();
            let spectrum_fbo = gl.create_framebuffer().unwrap();
            let mask_tex = gl.create_texture().unwrap();

            // Setup VAO + VBO (full-screen quad)
            let vertex_array = gl.create_vertex_array().unwrap();
            let vertices: [f32; 16] = [
                -1.0, -1.0, 0.0, 0.0,
                 1.0, -1.0, 1.0, 0.0,
                -1.0,  1.0, 0.0, 1.0,
                 1.0,  1.0, 1.0, 1.0,
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
                init_prog, butterfly_prog, mask_prog, extract_prog, spectrum_prog, blit_prog, bitrev_prog,
                init_input_loc, init_fft_size_loc, init_orig_size_loc,
                butterfly_input_loc, butterfly_axis_loc, butterfly_stage_loc,
                butterfly_direction_loc, butterfly_fft_size_loc,
                mask_fft_loc, mask_mask_loc, mask_fft_size_loc, mask_threshold_loc,
                extract_ifft_loc, extract_orig_loc, extract_fft_size_loc, extract_orig_size_loc, extract_black_threshold_loc,
                spectrum_fft_loc, spectrum_mask_loc, spectrum_fft_size_loc,
                bitrev_input_loc, bitrev_fft_size_loc,
                tex, fbo,
                output_tex, output_fbo,
                spectrum_tex, spectrum_fbo,
                mask_tex,
                vertex_array, vbo,
                last_fft_size: (0, 0),
                last_orig_size: (0, 0),
            }
        }
    }

    /// Returns the power-of-2 FFT dimensions for a given input size.
    pub fn fft_dimensions(width: u32, height: u32) -> (u32, u32) {
        (next_power_of_2(width), next_power_of_2(height))
    }

    /// Setup or resize internal textures/FBOs for the given dimensions.
    fn setup_resources(&mut self, gl: &glow::Context, fft_w: u32, fft_h: u32, orig_w: u32, orig_h: u32) {
        if self.last_fft_size == (fft_w, fft_h) && self.last_orig_size == (orig_w, orig_h) {
            return;
        }

        unsafe {
            // Ping-pong textures (RGBA32F for complex data)
            for i in 0..2 {
                gl.bind_texture(glow::TEXTURE_2D, Some(self.tex[i]));
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA32F as i32, fft_w as i32, fft_h as i32, 0, glow::RGBA, glow::FLOAT, None);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo[i]));
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(self.tex[i]), 0);
            }

            // Output texture (RGBA8, original resolution)
            gl.bind_texture(glow::TEXTURE_2D, Some(self.output_tex));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA as i32, orig_w as i32, orig_h as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, None);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.output_fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(self.output_tex), 0);

            // Spectrum visualization texture (RGBA8, FFT resolution)
            gl.bind_texture(glow::TEXTURE_2D, Some(self.spectrum_tex));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA as i32, fft_w as i32, fft_h as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, None);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.spectrum_fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(self.spectrum_tex), 0);

            // Mask texture (R8, FFT resolution) - initialize to all 255 (pass all)
            let mask_data = vec![255u8; (fft_w * fft_h) as usize];
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.mask_tex));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::R8 as i32, fft_w as i32, fft_h as i32, 0, glow::RED, glow::UNSIGNED_BYTE,
                Some(&mask_data));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);

            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }

        self.last_fft_size = (fft_w, fft_h);
        self.last_orig_size = (orig_w, orig_h);
    }

    /// Upload mask data from CPU to GPU.
    pub fn upload_mask(&self, gl: &glow::Context, mask_data: &[u8], width: u32, height: u32) {
        unsafe {
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.mask_tex));
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D, 0, 0, 0,
                width as i32, height as i32,
                glow::RED, glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(mask_data),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }

    /// Perform a single butterfly pass: read from tex[read_idx], write to tex[write_idx].
    /// Returns nothing — the caller tracks the current read index.
    unsafe fn butterfly_pass(&self, gl: &glow::Context, read_idx: usize, write_idx: usize, stage: u32) {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo[write_idx]));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.tex[read_idx]));
        gl.uniform_1_i32(self.butterfly_stage_loc.as_ref(), stage as i32);
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
    }

    /// Apply the FFT filter to the input texture. Returns the output texture.
    pub fn apply(
        &mut self,
        gl: &glow::Context,
        input_texture: glow::Texture,
        orig_width: u32,
        orig_height: u32,
        mask_threshold: f32,
        black_threshold: f32,
    ) -> glow::Texture {
        let (fft_w, fft_h) = Self::fft_dimensions(orig_width, orig_height);
        self.setup_resources(gl, fft_w, fft_h, orig_width, orig_height);

        let stages_x = log2_u32(fft_w);
        let stages_y = log2_u32(fft_h);

        unsafe {
            // Disable scissor test — egui sets a scissor rect for paint callbacks
            // that would clip our FBO renders to the visible widget area
            let scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(Some(self.vertex_array));
            gl.viewport(0, 0, fft_w as i32, fft_h as i32);

            // === Step 1: Initialize — write bit-reversed grayscale to tex[0] ===
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo[0]));
            gl.use_program(Some(self.init_prog));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
            gl.uniform_2_i32(self.init_fft_size_loc.as_ref(), fft_w as i32, fft_h as i32);
            gl.uniform_2_i32(self.init_orig_size_loc.as_ref(), orig_width as i32, orig_height as i32);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            // Current data is in tex[0]
            let mut cur = 0usize;

            // === Step 2: Forward FFT — horizontal butterfly passes ===
            gl.use_program(Some(self.butterfly_prog));
            gl.uniform_2_i32(self.butterfly_fft_size_loc.as_ref(), fft_w as i32, fft_h as i32);
            gl.uniform_1_i32(self.butterfly_direction_loc.as_ref(), 0); // forward
            gl.uniform_1_i32(self.butterfly_axis_loc.as_ref(), 0); // horizontal

            for stage in 1..=stages_x {
                let next = 1 - cur;
                self.butterfly_pass(gl, cur, next, stage);
                cur = next;
            }

            // === Step 3: Forward FFT — vertical butterfly passes ===
            gl.uniform_1_i32(self.butterfly_axis_loc.as_ref(), 1); // vertical

            for stage in 1..=stages_y {
                let next = 1 - cur;
                self.butterfly_pass(gl, cur, next, stage);
                cur = next;
            }

            // tex[cur] now holds the complete forward FFT result

            // === Step 3.5: Render spectrum visualization ===
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.spectrum_fbo));
            gl.use_program(Some(self.spectrum_prog));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.tex[cur]));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.mask_tex));
            gl.uniform_2_i32(self.spectrum_fft_size_loc.as_ref(), fft_w as i32, fft_h as i32);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            // === Step 4: Apply mask ===
            let mask_dst = 1 - cur;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo[mask_dst]));
            gl.use_program(Some(self.mask_prog));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.tex[cur]));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.mask_tex));
            gl.uniform_2_i32(self.mask_fft_size_loc.as_ref(), fft_w as i32, fft_h as i32);
            gl.uniform_1_f32(self.mask_threshold_loc.as_ref(), mask_threshold);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            cur = mask_dst;

            // === Step 4.5: Bit-reverse permutation before inverse FFT ===
            // The DIT butterfly requires bit-reversed input. Forward FFT did this
            // in the init shader, but the masked data is in natural order.
            let bitrev_dst = 1 - cur;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo[bitrev_dst]));
            gl.use_program(Some(self.bitrev_prog));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.tex[cur]));
            gl.uniform_2_i32(self.bitrev_fft_size_loc.as_ref(), fft_w as i32, fft_h as i32);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            cur = bitrev_dst;

            // === Step 5: Inverse FFT — horizontal butterfly passes ===
            gl.use_program(Some(self.butterfly_prog));
            gl.uniform_2_i32(self.butterfly_fft_size_loc.as_ref(), fft_w as i32, fft_h as i32);
            gl.uniform_1_i32(self.butterfly_direction_loc.as_ref(), 1); // inverse
            gl.uniform_1_i32(self.butterfly_axis_loc.as_ref(), 0); // horizontal

            for stage in 1..=stages_x {
                let next = 1 - cur;
                self.butterfly_pass(gl, cur, next, stage);
                cur = next;
            }

            // === Step 6: Inverse FFT — vertical butterfly passes ===
            gl.uniform_1_i32(self.butterfly_axis_loc.as_ref(), 1); // vertical

            for stage in 1..=stages_y {
                let next = 1 - cur;
                self.butterfly_pass(gl, cur, next, stage);
                cur = next;
            }

            // tex[cur] now holds the IFFT result

            // === Step 7: Extract result — crop to original resolution, restore color ===
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.output_fbo));
            gl.viewport(0, 0, orig_width as i32, orig_height as i32);
            gl.use_program(Some(self.extract_prog));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.tex[cur]));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
            gl.uniform_2_i32(self.extract_fft_size_loc.as_ref(), fft_w as i32, fft_h as i32);
            gl.uniform_2_i32(self.extract_orig_size_loc.as_ref(), orig_width as i32, orig_height as i32);
            gl.uniform_1_f32(self.extract_black_threshold_loc.as_ref(), black_threshold);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            // === Cleanup ===
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_vertex_array(None);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            // Re-enable scissor test for egui if it was enabled
            if scissor_enabled { gl.enable(glow::SCISSOR_TEST); }
        }

        self.output_tex
    }

    /// Blit a texture to the current framebuffer/viewport.
    /// Used by the mask editor to display the spectrum preview.
    pub fn blit_texture(&self, gl: &glow::Context, texture: glow::Texture) {
        unsafe {
            gl.use_program(Some(self.blit_prog));
            gl.bind_vertex_array(Some(self.vertex_array));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.bind_vertex_array(None);
        }
    }

    /// Returns the spectrum visualization texture (FFT resolution).
    pub fn spectrum_texture(&self) -> glow::Texture {
        self.spectrum_tex
    }
    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.init_prog);
            gl.delete_program(self.butterfly_prog);
            gl.delete_program(self.mask_prog);
            gl.delete_program(self.extract_prog);
            gl.delete_program(self.spectrum_prog);
            gl.delete_program(self.blit_prog);
            gl.delete_program(self.bitrev_prog);

            gl.delete_texture(self.tex[0]);
            gl.delete_texture(self.tex[1]);
            gl.delete_texture(self.output_tex);
            gl.delete_texture(self.spectrum_tex);
            gl.delete_texture(self.mask_tex);

            gl.delete_framebuffer(self.fbo[0]);
            gl.delete_framebuffer(self.fbo[1]);
            gl.delete_framebuffer(self.output_fbo);
            gl.delete_framebuffer(self.spectrum_fbo);

            gl.delete_vertex_array(self.vertex_array);
            gl.delete_buffer(self.vbo);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_next_power_of_2() {
        assert_eq!(next_power_of_2(0), 1);
        assert_eq!(next_power_of_2(1), 1);
        assert_eq!(next_power_of_2(2), 2);
        assert_eq!(next_power_of_2(3), 4);
        assert_eq!(next_power_of_2(4), 4);
        assert_eq!(next_power_of_2(5), 8);
        assert_eq!(next_power_of_2(255), 256);
        assert_eq!(next_power_of_2(256), 256);
        assert_eq!(next_power_of_2(257), 512);
        assert_eq!(next_power_of_2(1000), 1024);
        assert_eq!(next_power_of_2(1024), 1024);
        assert_eq!(next_power_of_2(1920), 2048);
    }

    #[test]
    fn test_log2_u32() {
        assert_eq!(log2_u32(1), 0);
        assert_eq!(log2_u32(2), 1);
        assert_eq!(log2_u32(4), 2);
        assert_eq!(log2_u32(8), 3);
        assert_eq!(log2_u32(16), 4);
        assert_eq!(log2_u32(256), 8);
        assert_eq!(log2_u32(1024), 10);
        assert_eq!(log2_u32(2048), 11);
    }

    #[test]
    fn test_fft_dimensions_already_power_of_2() {
        assert_eq!(FftFilter::fft_dimensions(256, 256), (256, 256));
        assert_eq!(FftFilter::fft_dimensions(1024, 512), (1024, 512));
    }

    #[test]
    fn test_fft_dimensions_non_power_of_2() {
        assert_eq!(FftFilter::fft_dimensions(640, 480), (1024, 512));
        assert_eq!(FftFilter::fft_dimensions(720, 480), (1024, 512));
        assert_eq!(FftFilter::fft_dimensions(1920, 1080), (2048, 2048));
        assert_eq!(FftFilter::fft_dimensions(1600, 1200), (2048, 2048));
    }

    #[test]
    fn test_fft_dimensions_small() {
        assert_eq!(FftFilter::fft_dimensions(1, 1), (1, 1));
        assert_eq!(FftFilter::fft_dimensions(3, 5), (4, 8));
    }
}
