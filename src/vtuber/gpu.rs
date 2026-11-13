use crate::config::{VtuberBackgroundKind, VtuberConfig, VtuberDisplayStyle};
use crate::vtuber::rig::EvaluatedAvatarPose;
use crate::vtuber::vrm::loader::{DecodedRgbaTexture, LoadedVrmAsset};
use crate::vtuber::vrm::{compile_gl_program, upload_decoded_texture, GpuVrmAvatar};
use eframe::glow::{self, HasContext};
use std::num::NonZeroU32;

const VS_QUAD_SRC: &str = include_str!("vrm/shaders/vs_vtuber_quad.glsl");
const FS_QUAD_SRC: &str = include_str!("vrm/shaders/fs_vtuber_quad.glsl");

struct QuadBlitter {
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    u_rect_ndc: Option<glow::UniformLocation>,
    u_uv_rect: Option<glow::UniformLocation>,
    u_tex: Option<glow::UniformLocation>,
    u_mode: Option<glow::UniformLocation>,
}

impl QuadBlitter {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let program = compile_gl_program(gl, VS_QUAD_SRC, FS_QUAD_SRC)?;
        let vao = gl
            .create_vertex_array()
            .map_err(|e| format!("Failed to create quad VAO: {e}"))?;
        let vbo = gl
            .create_buffer()
            .map_err(|e| format!("Failed to create quad VBO: {e}"))?;

        let unit_quad: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        gl.bind_vertex_array(Some(vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(
            glow::ARRAY_BUFFER,
            bytemuck::cast_slice(&unit_quad),
            glow::STATIC_DRAW,
        );
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
        gl.bind_vertex_array(None);

        let loc = |name: &str| gl.get_uniform_location(program, name);
        Ok(Self {
            program,
            vao,
            vbo,
            u_rect_ndc: loc("u_rect_ndc"),
            u_uv_rect: loc("u_uv_rect"),
            u_tex: loc("u_tex"),
            u_mode: loc("u_mode"),
        })
    }

    unsafe fn draw(
        &self,
        gl: &glow::Context,
        rect_ndc: [f32; 4],
        uv_rect: [f32; 4],
        tex: Option<glow::Texture>,
        mode: i32,
    ) {
        gl.use_program(Some(self.program));
        gl.bind_vertex_array(Some(self.vao));
        gl.uniform_4_f32_slice(self.u_rect_ndc.as_ref(), &rect_ndc);
        gl.uniform_4_f32_slice(self.u_uv_rect.as_ref(), &uv_rect);
        gl.uniform_1_i32(self.u_tex.as_ref(), 0);
        gl.uniform_1_i32(self.u_mode.as_ref(), mode);

        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, tex);
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        gl.bind_vertex_array(None);
    }

    unsafe fn destroy(&self, gl: &glow::Context) {
        gl.delete_vertex_array(self.vao);
        gl.delete_buffer(self.vbo);
        gl.delete_program(self.program);
    }
}

struct OffscreenTargets {
    width: u32,
    height: u32,
    msaa_fbo: Option<glow::Framebuffer>,
    msaa_color_rbo: Option<glow::Renderbuffer>,
    msaa_depth_rbo: Option<glow::Renderbuffer>,
    resolve_fbo: glow::Framebuffer,
    resolved_tex: glow::Texture,
    single_depth_rbo: Option<glow::Renderbuffer>,
}

impl OffscreenTargets {
    unsafe fn create(gl: &glow::Context, width: u32, height: u32) -> Result<Self, String> {
        let max_samples = gl.get_parameter_i32(glow::MAX_SAMPLES).max(0);
        let samples = if max_samples >= 4 {
            4
        } else if max_samples >= 2 {
            2
        } else {
            0
        };

        let resolved_tex = gl
            .create_texture()
            .map_err(|e| format!("Failed to create VTuber resolve texture: {e}"))?;
        gl.bind_texture(glow::TEXTURE_2D, Some(resolved_tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
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
        gl.bind_texture(glow::TEXTURE_2D, None);

        let resolve_fbo = gl
            .create_framebuffer()
            .map_err(|e| format!("Failed to create VTuber resolve FBO: {e}"))?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(resolve_fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(resolved_tex),
            0,
        );

        let mut msaa_fbo = None;
        let mut msaa_color_rbo = None;
        let mut msaa_depth_rbo = None;
        let mut single_depth_rbo = None;

        if samples > 0 {
            let status_resolve = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status_resolve != glow::FRAMEBUFFER_COMPLETE {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.delete_framebuffer(resolve_fbo);
                gl.delete_texture(resolved_tex);
                return Err(format!(
                    "VTuber resolve FBO incomplete (status 0x{status_resolve:x})"
                ));
            }

            let m_fbo = gl
                .create_framebuffer()
                .map_err(|e| format!("Failed to create VTuber MSAA FBO: {e}"))?;
            let c_rbo = gl
                .create_renderbuffer()
                .map_err(|e| format!("Failed to create MSAA color RBO: {e}"))?;
            let d_rbo = gl
                .create_renderbuffer()
                .map_err(|e| format!("Failed to create MSAA depth RBO: {e}"))?;

            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(c_rbo));
            gl.renderbuffer_storage_multisample(
                glow::RENDERBUFFER,
                samples,
                glow::RGBA8,
                width as i32,
                height as i32,
            );
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(d_rbo));
            gl.renderbuffer_storage_multisample(
                glow::RENDERBUFFER,
                samples,
                glow::DEPTH24_STENCIL8,
                width as i32,
                height as i32,
            );
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(m_fbo));
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::RENDERBUFFER,
                Some(c_rbo),
            );
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::DEPTH_STENCIL_ATTACHMENT,
                glow::RENDERBUFFER,
                Some(d_rbo),
            );

            let status_msaa = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status_msaa == glow::FRAMEBUFFER_COMPLETE {
                msaa_fbo = Some(m_fbo);
                msaa_color_rbo = Some(c_rbo);
                msaa_depth_rbo = Some(d_rbo);
            } else {
                // Graceful fallback to single-sample depth attachment on resolve_fbo
                gl.delete_framebuffer(m_fbo);
                gl.delete_renderbuffer(c_rbo);
                gl.delete_renderbuffer(d_rbo);
            }
        }

        if msaa_fbo.is_none() {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(resolve_fbo));
            let d_rbo = gl
                .create_renderbuffer()
                .map_err(|e| format!("Failed to create single-sample depth RBO: {e}"))?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(d_rbo));
            gl.renderbuffer_storage(
                glow::RENDERBUFFER,
                glow::DEPTH24_STENCIL8,
                width as i32,
                height as i32,
            );
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::DEPTH_STENCIL_ATTACHMENT,
                glow::RENDERBUFFER,
                Some(d_rbo),
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.delete_renderbuffer(d_rbo);
                gl.delete_framebuffer(resolve_fbo);
                gl.delete_texture(resolved_tex);
                return Err(format!("VTuber FBO incomplete (status 0x{status:x})"));
            }
            single_depth_rbo = Some(d_rbo);
        }

        gl.bind_framebuffer(glow::FRAMEBUFFER, None);

        Ok(Self {
            width,
            height,
            msaa_fbo,
            msaa_color_rbo,
            msaa_depth_rbo,
            resolve_fbo,
            resolved_tex,
            single_depth_rbo,
        })
    }

    fn draw_fbo(&self) -> glow::Framebuffer {
        self.msaa_fbo.unwrap_or(self.resolve_fbo)
    }

    unsafe fn resolve_if_multisampled(&self, gl: &glow::Context) {
        if let Some(m_fbo) = self.msaa_fbo {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(m_fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.resolve_fbo));
            gl.disable(glow::SCISSOR_TEST);
            gl.blit_framebuffer(
                0,
                0,
                self.width as i32,
                self.height as i32,
                0,
                0,
                self.width as i32,
                self.height as i32,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
        }
    }

    unsafe fn destroy(&self, gl: &glow::Context) {
        if let Some(fbo) = self.msaa_fbo {
            gl.delete_framebuffer(fbo);
        }
        if let Some(rbo) = self.msaa_color_rbo {
            gl.delete_renderbuffer(rbo);
        }
        if let Some(rbo) = self.msaa_depth_rbo {
            gl.delete_renderbuffer(rbo);
        }
        if let Some(rbo) = self.single_depth_rbo {
            gl.delete_renderbuffer(rbo);
        }
        gl.delete_framebuffer(self.resolve_fbo);
        gl.delete_texture(self.resolved_tex);
    }
}

struct SavedGlState {
    draw_fbo: Option<glow::Framebuffer>,
    read_fbo: Option<glow::Framebuffer>,
    viewport: [i32; 4],
    scissor_enabled: bool,
    scissor_box: [i32; 4],
    depth_test: bool,
    depth_writemask: bool,
    cull_face: bool,
    cull_face_mode: u32,
    front_face: u32,
    blend: bool,
    blend_src_rgb: u32,
    blend_dst_rgb: u32,
    blend_src_alpha: u32,
    blend_dst_alpha: u32,
    active_texture: u32,
    vao: Option<glow::VertexArray>,
    program: Option<glow::Program>,
}

impl SavedGlState {
    unsafe fn capture(gl: &glow::Context) -> Self {
        let mut viewport = [0i32; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
        let mut scissor_box = [0i32; 4];
        gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut scissor_box);

        let draw_fbo = NonZeroU32::new(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32)
            .map(glow::NativeFramebuffer);
        let read_fbo = NonZeroU32::new(gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING) as u32)
            .map(glow::NativeFramebuffer);
        let vao = NonZeroU32::new(gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING) as u32)
            .map(glow::NativeVertexArray);
        let program = NonZeroU32::new(gl.get_parameter_i32(glow::CURRENT_PROGRAM) as u32)
            .map(glow::NativeProgram);

        Self {
            draw_fbo,
            read_fbo,
            viewport,
            scissor_enabled: gl.is_enabled(glow::SCISSOR_TEST),
            scissor_box,
            depth_test: gl.is_enabled(glow::DEPTH_TEST),
            depth_writemask: gl.get_parameter_i32(glow::DEPTH_WRITEMASK) != 0,
            cull_face: gl.is_enabled(glow::CULL_FACE),
            cull_face_mode: gl.get_parameter_i32(glow::CULL_FACE_MODE) as u32,
            front_face: gl.get_parameter_i32(glow::FRONT_FACE) as u32,
            blend: gl.is_enabled(glow::BLEND),
            blend_src_rgb: gl.get_parameter_i32(glow::BLEND_SRC_RGB) as u32,
            blend_dst_rgb: gl.get_parameter_i32(glow::BLEND_DST_RGB) as u32,
            blend_src_alpha: gl.get_parameter_i32(glow::BLEND_SRC_ALPHA) as u32,
            blend_dst_alpha: gl.get_parameter_i32(glow::BLEND_DST_ALPHA) as u32,
            active_texture: gl.get_parameter_i32(glow::ACTIVE_TEXTURE) as u32,
            vao,
            program,
        }
    }

    unsafe fn restore_target_and_viewport(&self, gl: &glow::Context) {
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, self.draw_fbo);
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, self.read_fbo);
        gl.viewport(
            self.viewport[0],
            self.viewport[1],
            self.viewport[2],
            self.viewport[3],
        );
        gl.scissor(
            self.scissor_box[0],
            self.scissor_box[1],
            self.scissor_box[2],
            self.scissor_box[3],
        );
        if self.scissor_enabled {
            gl.enable(glow::SCISSOR_TEST);
        } else {
            gl.disable(glow::SCISSOR_TEST);
        }
    }

    unsafe fn restore_pipeline(&self, gl: &glow::Context) {
        self.restore_target_and_viewport(gl);

        if self.depth_test {
            gl.enable(glow::DEPTH_TEST);
        } else {
            gl.disable(glow::DEPTH_TEST);
        }
        gl.depth_mask(self.depth_writemask);

        if self.cull_face {
            gl.enable(glow::CULL_FACE);
        } else {
            gl.disable(glow::CULL_FACE);
        }
        if self.cull_face_mode != 0 {
            gl.cull_face(self.cull_face_mode);
        }
        if self.front_face != 0 {
            gl.front_face(self.front_face);
        }

        if self.blend {
            gl.enable(glow::BLEND);
        } else {
            gl.disable(glow::BLEND);
        }
        if self.blend_src_rgb != 0 && self.blend_dst_rgb != 0 {
            gl.blend_func_separate(
                self.blend_src_rgb,
                self.blend_dst_rgb,
                self.blend_src_alpha,
                self.blend_dst_alpha,
            );
        }

        gl.active_texture(glow::TEXTURE3);
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
        gl.active_texture(glow::TEXTURE2);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, None);
        if self.active_texture != 0 {
            gl.active_texture(self.active_texture);
        }

        gl.bind_vertex_array(self.vao);
        gl.use_program(self.program);
    }
}

/// Quantizes requested offscreen pixel dimensions to 32px steps (`128..=1280`) so dragging
/// resize handles never reallocates multisample renderbuffers on every single pixel.
pub fn quantize_offscreen_dims(req_w: u32, req_h: u32) -> (u32, u32) {
    let qw = ((req_w.max(1) + 31) & !31).clamp(128, 1280);
    let qh = ((req_h.max(1) + 31) & !31).clamp(128, 1280);
    (qw, qh)
}

/// Computes aspect-ratio-preserving "cover" crop `(u0, v0, u1, v1)` in top-down image UVs.
pub fn compute_cover_uv_bounds(img_w: u32, img_h: u32, dst_w: u32, dst_h: u32) -> [f32; 4] {
    let img_aspect = img_w.max(1) as f32 / img_h.max(1) as f32;
    let dst_aspect = dst_w.max(1) as f32 / dst_h.max(1) as f32;
    if (img_aspect - dst_aspect).abs() < 1e-4 {
        [0.0, 0.0, 1.0, 1.0]
    } else if img_aspect > dst_aspect {
        let vis_w = (dst_aspect / img_aspect).clamp(0.01, 1.0);
        let pad = 0.5 * (1.0 - vis_w);
        [pad, 0.0, 1.0 - pad, 1.0]
    } else {
        let vis_h = (img_aspect / dst_aspect).clamp(0.01, 1.0);
        let pad = 0.5 * (1.0 - vis_h);
        [0.0, pad, 1.0, 1.0 - pad]
    }
}

/// Self-contained OpenGL renderer for the VTuber avatar offscreen 4x MSAA pass, custom
/// backgrounds, and premultiplied viewport compositing.
#[derive(Default)]
pub struct VtuberGpuRenderer {
    quad: Option<QuadBlitter>,
    targets: Option<OffscreenTargets>,
    avatar: Option<GpuVrmAvatar>,
    bg_texture: Option<(glow::Texture, u32, u32)>,
    pending_vrm_upload: Option<LoadedVrmAsset>,
    pending_clear_vrm: bool,
    pending_bg_upload: Option<DecodedRgbaTexture>,
    pending_clear_bg: bool,
    last_gpu_error: Option<String>,
}

// SAFETY: GL object handles inside `VtuberGpuRenderer` are opaque `NonZeroU32` IDs and are
// exclusively touched inside `egui_glow` paint callbacks or `AppState::on_exit` with the
// owning OpenGL context current.
unsafe impl Send for VtuberGpuRenderer {}

impl VtuberGpuRenderer {
    pub fn queue_vrm_upload(&mut self, asset: LoadedVrmAsset) {
        self.pending_clear_vrm = false;
        self.pending_vrm_upload = Some(asset);
    }

    pub fn queue_clear_vrm(&mut self) {
        self.pending_vrm_upload = None;
        self.pending_clear_vrm = true;
    }

    pub fn queue_bg_upload(&mut self, tex: DecodedRgbaTexture) {
        self.pending_clear_bg = false;
        self.pending_bg_upload = Some(tex);
    }

    pub fn queue_clear_bg(&mut self) {
        self.pending_bg_upload = None;
        self.pending_clear_bg = true;
    }

    pub fn has_avatar(&self) -> bool {
        (self.avatar.is_some() && !self.pending_clear_vrm) || self.pending_vrm_upload.is_some()
    }

    pub fn take_gpu_error(&mut self) -> Option<String> {
        self.last_gpu_error.take()
    }

    unsafe fn process_pending_uploads(&mut self, gl: &glow::Context) {
        if self.pending_clear_vrm {
            self.pending_clear_vrm = false;
            if let Some(mut old) = self.avatar.take() {
                old.destroy(gl);
            }
        }
        if let Some(asset) = self.pending_vrm_upload.take() {
            if let Some(mut old) = self.avatar.take() {
                old.destroy(gl);
            }
            match GpuVrmAvatar::upload(gl, asset) {
                Ok(gpu_avatar) => {
                    self.avatar = Some(gpu_avatar);
                }
                Err(e) => {
                    self.last_gpu_error = Some(e);
                }
            }
        }

        if self.pending_clear_bg {
            self.pending_clear_bg = false;
            if let Some((old_tex, _, _)) = self.bg_texture.take() {
                gl.delete_texture(old_tex);
            }
        }
        if let Some(bg_cpu) = self.pending_bg_upload.take() {
            if let Some((old_tex, _, _)) = self.bg_texture.take() {
                gl.delete_texture(old_tex);
            }
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            match upload_decoded_texture(gl, &bg_cpu) {
                Ok(tex) => {
                    self.bg_texture = Some((tex, bg_cpu.width, bg_cpu.height));
                }
                Err(e) => {
                    self.last_gpu_error = Some(e);
                }
            }
        }
    }

    /// Renders the configured background + animated VRM avatar into the offscreen MSAA FBO
    /// and resolves into `resolved_tex` (`RGBA8` premultiplied alpha).
    /// Restores caller OpenGL state on exit.
    #[cfg(test)]
    pub fn render_offscreen(
        &mut self,
        gl: &glow::Context,
        req_width_px: u32,
        req_height_px: u32,
        dt: f32,
        pose: &EvaluatedAvatarPose,
        cfg: &VtuberConfig,
    ) -> Option<glow::Texture> {
        unsafe {
            let saved = SavedGlState::capture(gl);
            let res = self.render_offscreen_inner(gl, req_width_px, req_height_px, dt, pose, cfg);
            saved.restore_pipeline(gl);
            res
        }
    }

    /// Renders the offscreen VTuber scene and immediately composites the resolved premultiplied
    /// texture onto the caller's currently active viewport/scissor (`egui_glow` `PaintCallback`).
    /// `uv_crop_top_down` is `[u_left, v_top, u_right, v_bottom]` in `[0..1]`.
    pub fn render_and_composite(
        &mut self,
        gl: &glow::Context,
        req_width_px: u32,
        req_height_px: u32,
        dt: f32,
        pose: &EvaluatedAvatarPose,
        cfg: &VtuberConfig,
        uv_crop_top_down: [f32; 4],
    ) {
        unsafe {
            let saved = SavedGlState::capture(gl);
            if let Some(resolved_tex) =
                self.render_offscreen_inner(gl, req_width_px, req_height_px, dt, pose, cfg)
            {
                saved.restore_target_and_viewport(gl);
                if let Some(quad) = &self.quad {
                    gl.disable(glow::DEPTH_TEST);
                    gl.depth_mask(false);
                    gl.disable(glow::CULL_FACE);
                    gl.enable(glow::BLEND);
                    gl.blend_func_separate(
                        glow::ONE,
                        glow::ONE_MINUS_SRC_ALPHA,
                        glow::ONE,
                        glow::ONE_MINUS_SRC_ALPHA,
                    );
                    // Map top-down crop UVs `[u0, v_top, u1, v_bot]` to OpenGL bottom-up texture UVs
                    let gl_uv = [
                        uv_crop_top_down[0],
                        1.0 - uv_crop_top_down[3],
                        uv_crop_top_down[2],
                        1.0 - uv_crop_top_down[1],
                    ];
                    quad.draw(gl, [-1.0, -1.0, 1.0, 1.0], gl_uv, Some(resolved_tex), 0);
                }
            }
            saved.restore_pipeline(gl);
        }
    }

    unsafe fn render_offscreen_inner(
        &mut self,
        gl: &glow::Context,
        req_width_px: u32,
        req_height_px: u32,
        dt: f32,
        pose: &EvaluatedAvatarPose,
        cfg: &VtuberConfig,
    ) -> Option<glow::Texture> {
        if self.quad.is_none() {
            match QuadBlitter::new(gl) {
                Ok(q) => self.quad = Some(q),
                Err(e) => {
                    self.last_gpu_error = Some(e);
                    return None;
                }
            }
        }

        self.process_pending_uploads(gl);

        let (fbo_w, fbo_h) = quantize_offscreen_dims(req_width_px, req_height_px);
        let needs_recreate = self
            .targets
            .as_ref()
            .is_none_or(|t| t.width != fbo_w || t.height != fbo_h);
        if needs_recreate {
            if let Some(old) = self.targets.take() {
                old.destroy(gl);
            }
            match OffscreenTargets::create(gl, fbo_w, fbo_h) {
                Ok(t) => self.targets = Some(t),
                Err(e) => {
                    self.last_gpu_error = Some(e);
                    return None;
                }
            }
        }

        let targets = self.targets.as_ref()?;
        let quad = self.quad.as_ref()?;

        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(targets.draw_fbo()));
        gl.viewport(0, 0, fbo_w as i32, fbo_h as i32);
        gl.disable(glow::SCISSOR_TEST);
        gl.depth_mask(true);

        // 1. Render background (forced transparent in BorderlessCutout mode)
        let effective_bg = if cfg.display_style == VtuberDisplayStyle::BorderlessCutout {
            VtuberBackgroundKind::Transparent
        } else {
            cfg.background_kind
        };

        match effective_bg {
            VtuberBackgroundKind::Transparent => {
                gl.clear_color(0.0, 0.0, 0.0, 0.0);
                gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            }
            VtuberBackgroundKind::SolidColor => {
                let [r, g, b, a] = cfg.background_color;
                let af = a as f32 / 255.0;
                let rf = (r as f32 / 255.0) * af;
                let gf = (g as f32 / 255.0) * af;
                let bf = (b as f32 / 255.0) * af;
                gl.clear_color(rf, gf, bf, af);
                gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            }
            VtuberBackgroundKind::StaticImage => {
                let [r, g, b, a] = cfg.background_color;
                let af = a as f32 / 255.0;
                let rf = (r as f32 / 255.0) * af;
                let gf = (g as f32 / 255.0) * af;
                let bf = (b as f32 / 255.0) * af;
                gl.clear_color(rf, gf, bf, af);
                gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
                if let Some((bg_tex, img_w, img_h)) = self.bg_texture {
                    let [u0, v0, u1, v1] = compute_cover_uv_bounds(img_w, img_h, fbo_w, fbo_h);
                    gl.disable(glow::DEPTH_TEST);
                    gl.depth_mask(false);
                    gl.disable(glow::CULL_FACE);
                    gl.disable(glow::BLEND);
                    // Top of image `v0` -> top of FBO (`NDC +1`), bottom `v1` -> bottom (`NDC -1`)
                    quad.draw(
                        gl,
                        [-1.0, -1.0, 1.0, 1.0],
                        [u0, v1, u1, v0],
                        Some(bg_tex),
                        1,
                    );
                }
            }
        }

        // 2. Render MToon 3D VRM avatar
        if let Some(avatar) = &mut self.avatar {
            avatar.animate_and_draw(
                gl,
                dt,
                pose,
                fbo_w,
                fbo_h,
                cfg.camera_zoom,
                cfg.camera_offset_y,
            );
        }

        // 3. Resolve MSAA -> single-sample premultiplied RGBA8 texture
        targets.resolve_if_multisampled(gl);

        Some(targets.resolved_tex)
    }

    pub fn destroy(&mut self, gl: &glow::Context) {
        unsafe {
            if let Some(mut avatar) = self.avatar.take() {
                avatar.destroy(gl);
            }
            if let Some((bg_tex, _, _)) = self.bg_texture.take() {
                gl.delete_texture(bg_tex);
            }
            if let Some(targets) = self.targets.take() {
                targets.destroy(gl);
            }
            if let Some(quad) = self.quad.take() {
                quad.destroy(gl);
            }
            self.pending_vrm_upload = None;
            self.pending_bg_upload = None;
        }
    }
}

#[cfg(test)]
#[path = "../../tests/support/software_gl.rs"]
mod software_gl;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quantize_offscreen_dims_clamps_and_steps_by_32() {
        assert_eq!(quantize_offscreen_dims(10, 20), (128, 128));
        assert_eq!(quantize_offscreen_dims(300, 200), (320, 224));
        assert_eq!(quantize_offscreen_dims(512, 512), (512, 512));
        assert_eq!(quantize_offscreen_dims(4000, 3000), (1280, 1280));
    }

    #[test]
    fn test_compute_cover_uv_bounds_preserves_aspect_without_stretching() {
        // 16:9 image into 1:1 square crops left and right symmetrically
        let [u0, v0, u1, v1] = compute_cover_uv_bounds(1600, 900, 500, 500);
        assert!(u0 > 0.20 && u0 < 0.25);
        assert!((u0 + u1 - 1.0).abs() < 1e-5);
        assert_eq!(v0, 0.0);
        assert_eq!(v1, 1.0);

        // 3:4 tall image into 16:9 wide crops top and bottom symmetrically
        let [u0, v0, u1, v1] = compute_cover_uv_bounds(600, 800, 1600, 900);
        assert_eq!(u0, 0.0);
        assert_eq!(u1, 1.0);
        assert!(v0 > 0.25 && v0 < 0.30);
        assert!((v0 + v1 - 1.0).abs() < 1e-5);
    }

    #[test]
    #[ignore = "opt-in Mesa software EGL readback; set LIBGL_ALWAYS_SOFTWARE=1 EGL_PLATFORM=surfaceless"]
    fn test_vtuber_gpu_renderer_renders_cutout_and_solid_background_headlessly() {
        let egl = software_gl::SoftwareGl::new(256, 256);
        let gl = egl.context();

        let glb = crate::vtuber::vrm::loader::tests::build_minimal_test_vrm1_glb();
        let asset = crate::vtuber::vrm::loader::load_vrm_from_bytes(&glb)
            .expect("minimal VRM 1.0 GLB must parse");

        let mut renderer = VtuberGpuRenderer::default();
        renderer.queue_vrm_upload(asset);

        let pose = crate::vtuber::rig::EvaluatedAvatarPose::default();
        let mut cfg = VtuberConfig::default();
        cfg.display_style = VtuberDisplayStyle::BorderlessCutout;

        let _tex = renderer
            .render_offscreen(&gl, 256, 256, 1.0 / 60.0, &pose, &cfg)
            .expect("offscreen cutout render should succeed");
        assert!(
            renderer.take_gpu_error().is_none(),
            "unexpected GPU upload/render error"
        );

        // Read back resolved FBO pixels
        let mut pixels = vec![0u8; 256 * 256 * 4];
        unsafe {
            let resolve_fbo = renderer.targets.as_ref().unwrap().resolve_fbo;
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(resolve_fbo));
            gl.read_pixels(
                0,
                0,
                256,
                256,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(&mut pixels),
            );
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
            assert_eq!(gl.get_error(), glow::NO_ERROR);
        }

        // Top-left corner in BorderlessCutout must be 100% transparent (alpha == 0)
        assert_eq!(pixels[3], 0, "BorderlessCutout corner alpha must be 0");
        // At least some avatar pixels in the bust frame must have non-zero alpha
        let opaque_pixels = pixels.chunks_exact(4).filter(|px| px[3] > 200).count();
        assert!(
            opaque_pixels > 50,
            "Expected visible MToon avatar geometry in bust frame, got {opaque_pixels} pixels"
        );

        // Now render FramedWebcam with SolidColor background and verify background fills corners
        cfg.display_style = VtuberDisplayStyle::FramedWebcam;
        cfg.background_kind = VtuberBackgroundKind::SolidColor;
        cfg.background_color = [40, 80, 160, 255];
        let _tex2 = renderer
            .render_offscreen(&gl, 256, 256, 1.0 / 60.0, &pose, &cfg)
            .expect("offscreen solid-color render should succeed");
        unsafe {
            let resolve_fbo = renderer.targets.as_ref().unwrap().resolve_fbo;
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(resolve_fbo));
            gl.read_pixels(
                0,
                0,
                256,
                256,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(&mut pixels),
            );
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
            assert_eq!(gl.get_error(), glow::NO_ERROR);
        }
        assert_eq!(&pixels[0..4], &[40, 80, 160, 255]);

        renderer.destroy(&gl);
        unsafe {
            assert_eq!(gl.get_error(), glow::NO_ERROR);
        }
    }
}
