use eframe::glow::{self, HasContext};
use eframe::{egui, egui_glow};

use std::num::NonZero;

const VS_SRC: &str = r#"#version 330 core
    layout(location = 0) in vec2 a_pos;
    layout(location = 1) in vec2 a_tc;
    out vec2 v_tc;
    void main() {
        gl_Position = vec4(a_pos, 0.0, 1.0);
        v_tc = a_tc;
    }
"#;

// Pixelation shader to simulate 480p
const FS_PIXELATE: &str = r#"#version 330 core
    in vec2 v_tc;
    out vec4 out_color;

    uniform sampler2D video_texture;
    uniform vec2 target_resolution; // e.g., 854.0, 480.0 for 16:9 480p

    void main() {
        // Flip the Y-coordinate to correct for framebuffer inversion.
        vec2 flipped_tc = vec2(v_tc.x, 1.0 - v_tc.y);

        // Calculate the size of a 'pixel' in the low-resolution target.
        vec2 pixel_size = 1.0 / target_resolution;

        // Find the coordinate of the center of the low-res 'pixel' block.
        vec2 pixelated_uv = (floor(flipped_tc / pixel_size) + 0.5) * pixel_size;

        out_color = texture(video_texture, pixelated_uv);
    }"#;

// Simple passthrough shader for drawing a texture to the screen
const FS_PASSTHROUGH: &str = r#"#version 330 core
    in vec2 v_tc;
    out vec4 out_color;
    uniform sampler2D video_texture;
    uniform vec2 videoResolution;
    uniform vec2 outputResolution;
    
    // Convert from linear to sRGB color space
    float ToSrgb1(float c) {
        return (c < 0.0031308 ? c * 12.92 : 1.055 * pow(c, 0.41666) - 0.055);
    }
    vec3 ToSrgb(vec3 c) {
        return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
    }

    void main() {
        float video_aspect = videoResolution.x / videoResolution.y;
        float output_aspect = outputResolution.x / outputResolution.y;

        vec2 scale = vec2(1.0, 1.0);
        if (video_aspect > output_aspect) {
            scale.y = output_aspect / video_aspect;
        } else {
            scale.x = video_aspect / output_aspect;
        }

        vec2 centered_tc = (v_tc - 0.5) / scale + 0.5;

        if (centered_tc.x < 0.0 || centered_tc.x > 1.0 || centered_tc.y < 0.0 || centered_tc.y > 1.0) {
            out_color = vec4(0.0, 0.0, 0.0, 1.0);
        } else {
            vec3 linear_color = texture(video_texture, centered_tc).rgb;
            out_color = vec4(ToSrgb(linear_color), 1.0);
        }
    }"#;
// Lottes Pass 0: Horizontal blur for bloom
const FS_PASS0: &str = r#"#version 330 core
    in vec2 v_tc;
    out vec4 out_color;
    uniform sampler2D video_texture;
    uniform float hardBloomPix;

    float Gaus(float pos, float scale) {
        return exp2(scale * pos * pos);
    }

    void main() {
        vec2 tex_size = vec2(textureSize(video_texture, 0));
        vec2 dx = vec2(1.0 / tex_size.x, 0.0);
        vec3 col = vec3(0.0);
        float total = 0.0;
        for (int i = -3; i <= 3; i += 1) {
            float weight = Gaus(i, hardBloomPix);
            col += texture(video_texture, v_tc + i * dx).rgb * weight;
            total += weight;
        }
        out_color = vec4(col / total, 1.0);
    }
"#;

// Lottes Pass 1: Vertical blur for bloom
const FS_PASS1: &str = r#"#version 330 core
    in vec2 v_tc;
    out vec4 out_color;
    uniform sampler2D pass0_texture;
    uniform float hardBloomScan;

    float Gaus(float pos, float scale) {
        return exp2(scale * pos * pos);
    }

    void main() {
        vec2 tex_size = vec2(textureSize(pass0_texture, 0));
        vec2 dy = vec2(0.0, 1.0 / tex_size.y);
        vec3 col = vec3(0.0);
        float total = 0.0;
        for (int i = -2; i <= 2; i += 1) {
            float weight = Gaus(i, hardBloomScan);
            col += texture(pass0_texture, v_tc + i * dy).rgb * weight;
            total += weight;
        }
        out_color = vec4(col / total, 1.0);
    }
"#;

// Lottes Pass 2: Horizontal blur for scanlines
const FS_PASS2: &str = r#"#version 330 core
    in vec2 v_tc;
    out vec4 out_color;
    uniform sampler2D video_texture;
    uniform float hardPix;

    float Gaus(float pos, float scale) {
        return exp2(scale * pos * pos);
    }

    void main() {
        vec2 tex_size = vec2(textureSize(video_texture, 0));
        vec2 dx = vec2(1.0 / tex_size.x, 0.0);
        vec3 col = vec3(0.0);
        float total = 0.0;
        for (int i = -2; i <= 2; i += 1) {
            float weight = Gaus(i, hardPix);
            col += texture(video_texture, v_tc + i * dx).rgb * weight;
            total += weight;
        }
        out_color = vec4(col / total, 1.0);
    }
"#;

// Lottes Pass 3: Vertical blur for scanlines
const FS_PASS3: &str = r#"#version 330 core
    in vec2 v_tc;
    out vec4 out_color;
    uniform sampler2D pass2_texture;
    uniform float hardScan;
    uniform float shape;

    float Gaus(float pos, float scale) {
        return exp2(scale * pow(abs(pos), shape));
    }

    void main() {
        vec2 tex_size = vec2(textureSize(pass2_texture, 0));
        vec2 dy = vec2(0.0, 1.0 / tex_size.y);
        vec3 col = vec3(0.0);
        float total = 0.0;
        for (int i = -2; i <= 2; i += 1) {
            float weight = Gaus(i, hardScan);
            col += texture(pass2_texture, v_tc + i * dy).rgb * weight;
            total += weight;
        }
        out_color = vec4(col / total, 1.0);
    }
"#;

// Lottes Final Pass: Combines textures and applies warp, mask, and color correction
const FS_FINAL: &str = r#"#version 330 core
    in vec2 v_tc;
    out vec4 out_color;

    uniform sampler2D pass1_texture; // bloom
    uniform sampler2D pass3_texture; // scanlines

    uniform vec2 videoResolution;
    uniform vec2 outputResolution;

    uniform float warpX;
    uniform float warpY;
    uniform float shadowMask; // 0-4
    uniform float brightboost;
    uniform float bloomAmount;

    float ToSrgb1(float c) {
        return (c < 0.0031308 ? c * 12.92 : 1.055 * pow(c, 0.41666) - 0.055);
    }

    vec3 ToSrgb(vec3 c) {
        return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
    }

    vec2 Warp(vec2 pos) {
        pos = pos * 2.0 - 1.0;
        pos *= vec2(1.0 + (pos.y * pos.y) * warpX, 1.0 + (pos.x * pos.x) * warpY);
        return pos * 0.5 + 0.5;
    }

    vec3 Mask(vec2 pos) {
        float maskDark = 0.5;
        vec3 mask = vec3(0.5, 0.5, 0.5); // maskDark
        float maskLight = 1.5;
        if (shadowMask == 1.0) { // Compressed TV
            float line = maskLight;
            float odd = 0.0;
            if (fract(pos.x / 6.0) < 0.5) odd = 1.0;
            if (fract((pos.y + odd) / 2.0) < 0.5) line = maskDark;
            pos.x = fract(pos.x / 3.0);
            if (pos.x < 0.333) mask.r = maskLight;
            else if (pos.x < 0.666) mask.g = maskLight;
            else mask.b = maskLight;
            mask *= line;
        } else if (shadowMask == 2.0) { // Aperture-grille
            pos.x = fract(pos.x / 3.0);
            if (pos.x < 0.333) mask.r = maskLight;
            else if (pos.x < 0.666) mask.g = maskLight;
            else mask.b = maskLight;
        } else if (shadowMask == 3.0) { // Stretched VGA
            pos.x += pos.y * 3.0;
            pos.x = fract(pos.x / 6.0);
            if (pos.x < 0.333) mask.r = maskLight;
            else if (pos.x < 0.666) mask.g = maskLight;
            else mask.b = maskLight;
        } else if (shadowMask == 4.0) { // VGA
            pos.xy = floor(pos.xy * vec2(1.0, 0.5));
            pos.x += pos.y * 3.0;
            pos.x = fract(pos.x / 6.0);
            if (pos.x < 0.333) mask.r = maskLight;
            else if (pos.x < 0.666) mask.g = maskLight;
            else mask.b = maskLight;
        }
        return mask;
    }

    void main() {
        // Calculate aspect ratios
        float video_aspect = videoResolution.x / videoResolution.y;
        float output_aspect = outputResolution.x / outputResolution.y;

        // Determine scale and offset to letterbox/pillarbox the video
        vec2 scale = vec2(1.0, 1.0);
        if (video_aspect > output_aspect) {
            scale.y = output_aspect / video_aspect;
        } else {
            scale.x = video_aspect / output_aspect;
        }
        vec2 warped_tc = Warp(v_tc);
        vec2 warped_pos = (warped_tc - 0.5) / scale + 0.5;

        if (warped_pos.x < 0.0 || warped_pos.x > 1.0 || warped_pos.y < 0.0 || warped_pos.y > 1.0) {
            out_color = vec4(0.0, 0.0, 0.0, 1.0);
            return;
        }

        // The original shader's Tri() function is equivalent to our pass3_texture lookup
        vec3 scanline_color = texture(pass3_texture, warped_pos).rgb; 
        // The original shader's Bloom() function is equivalent to our pass1_texture lookup
        vec3 bloom_color = texture(pass1_texture, warped_pos).rgb;

        vec3 final_color = scanline_color + bloom_color * bloomAmount;

        if (shadowMask > 0.0) {
            final_color *= Mask(floor(v_tc * outputResolution) + 0.5);
        }

        final_color *= brightboost;

        out_color = vec4(ToSrgb(final_color), 1.0);
    }
"#;

pub struct CrtFilterRenderer {
    passthrough_prog: glow::Program,
    pixelate_prog: glow::Program,
    pass0_prog: glow::Program,
    pass1_prog: glow::Program,
    pass2_prog: glow::Program,
    pass3_prog: glow::Program,
    final_prog: glow::Program,

    fbos: [glow::Framebuffer; 5],
    pass_textures: [glow::Texture; 5],
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

    last_size: (u32, u32),
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
            let final_prog = compile_program(gl, VS_SRC, FS_FINAL);

            // Passthrough
            let p_passthrough_video_res_loc = gl.get_uniform_location(passthrough_prog, "videoResolution").unwrap();
            let p_passthrough_output_res_loc = gl.get_uniform_location(passthrough_prog, "outputResolution").unwrap();

            // Pixelate
            let p_pixelate_target_res_loc =
                gl.get_uniform_location(pixelate_prog, "target_resolution")
                    .unwrap();

            // Pass 0
            let p0_hard_bloom_pix_loc = gl.get_uniform_location(pass0_prog, "hardBloomPix").unwrap();

            // Pass 1
            let p1_hard_bloom_scan_loc = gl.get_uniform_location(pass1_prog, "hardBloomScan").unwrap();

            // Pass 2
            let p2_hard_pix_loc = gl.get_uniform_location(pass2_prog, "hardPix").unwrap();

            // Pass 3
            let p3_hard_scan_loc = gl.get_uniform_location(pass3_prog, "hardScan").unwrap();
            let p3_shape_loc = gl.get_uniform_location(pass3_prog, "shape").unwrap();

            // Final Pass
            let final_video_res_loc = gl.get_uniform_location(final_prog, "videoResolution").unwrap();
            let final_output_res_loc = gl.get_uniform_location(final_prog, "outputResolution").unwrap();
            let final_warp_x_loc = gl.get_uniform_location(final_prog, "warpX").unwrap();
            let final_warp_y_loc = gl.get_uniform_location(final_prog, "warpY").unwrap();
            let final_shadow_mask_loc = gl.get_uniform_location(final_prog, "shadowMask").unwrap();
            let final_brightboost_loc = gl.get_uniform_location(final_prog, "brightboost").unwrap();
            let final_bloom_amount_loc = gl.get_uniform_location(final_prog, "bloomAmount").unwrap();

            // Set sampler uniforms once, as they don't change.
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
            gl.use_program(None);

            let fbos = [
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
            ];

            let vertex_array = gl.create_vertex_array().expect("Cannot create vertex array");

            // --- Fullscreen Quad ---
            // We need a vertex buffer to draw a simple quad.
            let vertices: [f32; 16] = [
                // pos    // tex
                -1.0, -1.0, 0.0, 1.0, // bottom-left
                 1.0, -1.0, 1.0, 1.0, // bottom-right
                -1.0,  1.0, 0.0, 0.0, // top-left
                 1.0,  1.0, 1.0, 0.0, // top-right
            ];
            let vbo = gl.create_buffer().unwrap();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck::cast_slice(&vertices), glow::STATIC_DRAW);

            gl.bind_vertex_array(Some(vertex_array));
            // Position attribute
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 4 * std::mem::size_of::<f32>() as i32, 0);
            gl.enable_vertex_attrib_array(0);
            // Texture coordinate attribute
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 4 * std::mem::size_of::<f32>() as i32, (2 * std::mem::size_of::<f32>()) as i32);
            gl.enable_vertex_attrib_array(1);

            // Unbind VBO and VAO
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_vertex_array(None);

            Self {
                passthrough_prog, pixelate_prog, pass0_prog, pass1_prog, pass2_prog, pass3_prog, final_prog,
                fbos, pass_textures, vertex_array, vbo,
                p_passthrough_video_res_loc, p_passthrough_output_res_loc,
                p_pixelate_target_res_loc,
                p0_hard_bloom_pix_loc,
                p1_hard_bloom_scan_loc,
                p2_hard_pix_loc, p3_hard_scan_loc, p3_shape_loc,
                final_video_res_loc, final_output_res_loc, final_warp_x_loc, final_warp_y_loc,
                final_shadow_mask_loc, final_brightboost_loc, final_bloom_amount_loc,
                last_size: (0, 0),
            }
        }
    }

    pub fn paint(&mut self, painter: &egui_glow::Painter, video_texture_id: egui::TextureId, resolution: (u32, u32), output_size: (f32, f32), params: &ShaderParams, run_pixelate: bool, run_lottes: bool) {
        let gl = painter.gl();
        let video_texture = painter.texture(video_texture_id).unwrap();

        if self.last_size != resolution {
            self.setup_framebuffers(gl, resolution.0, resolution.1);
            self.last_size = resolution;
        }

        unsafe {
            // Save egui's vertex array binding
            let old_vbo = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);

            gl.bind_vertex_array(Some(self.vertex_array));
            gl.viewport(0, 0, resolution.0 as i32, resolution.1 as i32);

            let mut lottes_input_texture = video_texture;

            if run_pixelate {
                // --- PIXELATE PASS ---
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[4]));
                gl.use_program(Some(self.pixelate_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(video_texture));
                // Target 480p 16:9
                gl.uniform_2_f32(Some(&self.p_pixelate_target_res_loc), 854.0, 480.0);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                lottes_input_texture = self.pass_textures[4];
            }

            if run_lottes {
                // --- PASS 0 (Horizontal Bloom) ---
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[0]));
                gl.use_program(Some(self.pass0_prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(lottes_input_texture));
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
                gl.bind_texture(glow::TEXTURE_2D, Some(lottes_input_texture));
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

                gl.uniform_2_f32(Some(&self.final_video_res_loc), resolution.0 as f32, resolution.1 as f32);
                gl.uniform_2_f32(Some(&self.final_output_res_loc), output_size.0, output_size.1);
                gl.uniform_1_f32(Some(&self.final_warp_x_loc), params.warp_x);
                gl.uniform_1_f32(Some(&self.final_warp_y_loc), params.warp_y);
                gl.uniform_1_f32(Some(&self.final_shadow_mask_loc), params.shadow_mask);
                gl.uniform_1_f32(Some(&self.final_brightboost_loc), params.brightboost);
                gl.uniform_1_f32(Some(&self.final_bloom_amount_loc), params.bloom_amount);

                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            } else if run_pixelate {
                // If only pixelation is enabled, we need to draw its result to the screen.
                gl.bind_framebuffer(glow::FRAMEBUFFER, None); // Render to screen
                gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
                gl.use_program(Some(self.passthrough_prog));

                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(lottes_input_texture));

                gl.uniform_2_f32(Some(&self.p_passthrough_video_res_loc), resolution.0 as f32, resolution.1 as f32);
                gl.uniform_2_f32(Some(&self.p_passthrough_output_res_loc), output_size.0, output_size.1);

                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            }

            gl.bind_vertex_array(None);

            // Restore egui's vertex array binding
            if old_vbo != 0 {
                gl.bind_vertex_array(Some(glow::VertexArray::from(glow::NativeVertexArray(NonZero::new(old_vbo as u32).unwrap()))));
            } else {
                tracing::warn!("old_vbo was 0, cannot restore egui's VAO binding. This might indicate an issue with egui's GL state management. Binding None instead. This is likely fine if egui is not using a VAO.");
                gl.bind_vertex_array(None);
            }
        }
    }

    pub fn draw_passthrough(&self, gl: &glow::Context, video_texture: glow::Texture, resolution: (u32, u32), output_size: (f32, f32)) {
        unsafe {
            let old_vbo = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
            gl.bind_vertex_array(Some(self.vertex_array));

            gl.bind_framebuffer(glow::FRAMEBUFFER, None); // Render to screen
            gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
            gl.use_program(Some(self.passthrough_prog));

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(video_texture));

            gl.uniform_2_f32(Some(&self.p_passthrough_video_res_loc), resolution.0 as f32, resolution.1 as f32);
            gl.uniform_2_f32(Some(&self.p_passthrough_output_res_loc), output_size.0, output_size.1);

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
            gl.delete_program(self.final_prog);
            gl.delete_vertex_array(self.vertex_array);
            gl.delete_buffer(self.vbo);
            for fbo in self.fbos {
                gl.delete_framebuffer(fbo);
            }
            for texture in self.pass_textures {
                gl.delete_texture(texture);
            }
        }
    }

    fn setup_framebuffers(&mut self, gl: &glow::Context, width: u32, height: u32) {
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
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);

                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbos[i]));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(self.pass_textures[i]),
                    0,
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
        let shader = gl.create_shader(*shader_type).expect("Cannot create shader");
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
            hard_scan: state.crt_hard_scan,
            warp_x: state.crt_warp_x,
            warp_y: state.crt_warp_y,
            shadow_mask: state.crt_shadow_mask,
            brightboost: state.crt_brightboost,
            hard_bloom_pix: state.crt_hard_bloom_pix,
            hard_bloom_scan: state.crt_hard_bloom_scan,
            bloom_amount: state.crt_bloom_amount,
            shape: state.crt_shape,
            hard_pix: state.crt_hard_pix,
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
        }
    }
}