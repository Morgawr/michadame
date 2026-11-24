//! AMD FidelityFX Optical Flow v5 + Motion-Compensated Frame Interpolation (GLSL 430 compute).
//!
//! Used exclusively on desktop Linux for PS5 Remote Play streams configured at 30 FPS
//! to synthesize smooth 60 FPS motion-compensated midpoint frames (`t = 0.5`) directly
//! inside the OpenGL rendering pipeline before upscaling and CRT filters.

use eframe::glow::{self, HasContext};

const PYRAMID_LEVELS: usize = 7;
const FLOW_SEARCH_LEVELS: usize = 6; // Pyramid levels 0..=5 (1/1 to 1/32 resolution, covering +/-256px+ at 1080p).
const SCD_LEVEL: usize = 5; // 1/32 resolution for fast, noise-resistant SCD histograms.

pub const DEFAULT_FRAMEGEN_ARTIFACT_GUARD: f32 = 0.65;
pub const DEFAULT_FRAMEGEN_SCD_SENSITIVITY: f32 = 0.50;
pub const DEFAULT_FRAMEGEN_HUD_PROTECTION: f32 = 0.70;

const CS_PREPARE_LUMA: &str = include_str!("../shaders/cs_ffx_of_prepare_luma.glsl");
const CS_LUMA_PYRAMID: &str = include_str!("../shaders/cs_ffx_of_luma_pyramid.glsl");
const CS_SCD: &str = include_str!("../shaders/cs_ffx_of_scd.glsl");
const CS_SEARCH_V5: &str = include_str!("../shaders/cs_ffx_of_search_v5.glsl");
const CS_FILTER_SCALE_V5: &str = include_str!("../shaders/cs_ffx_of_filter_scale_v5.glsl");
const CS_REPROJECT_MID: &str = include_str!("../shaders/cs_ffx_of_reproject_mid.glsl");
const CS_HUD_MASK: &str = include_str!("../shaders/cs_ffx_hud_mask.glsl");
const CS_FRAME_INTERP: &str = include_str!("../shaders/cs_ffx_frame_interp.glsl");

/// User-tunable FidelityFX Frame Gen quality & artifact-control parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfxFrameGenParams {
    /// Strictness of cycle-consistency validation & unmatchable-pixel fallback (`0.0..=1.0`).
    /// Higher values suppress warping/smearing glitches more aggressively.
    pub artifact_guard: f32,
    /// Scene Change Detection sensitivity (`0.0..=1.0`). Higher values fall back sooner on cuts/whips.
    pub scd_sensitivity: f32,
    /// 5-tap stationary-edge HUD/subtitle lock strength (`0.0..=1.0`).
    pub hud_protection: f32,
}

impl Default for FfxFrameGenParams {
    fn default() -> Self {
        Self {
            artifact_guard: DEFAULT_FRAMEGEN_ARTIFACT_GUARD,
            scd_sensitivity: DEFAULT_FRAMEGEN_SCD_SENSITIVITY,
            hud_protection: DEFAULT_FRAMEGEN_HUD_PROTECTION,
        }
    }
}

impl FfxFrameGenParams {
    pub fn sanitized(self) -> Self {
        let clamp_or = |v: f32, fallback: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                fallback
            }
        };
        Self {
            artifact_guard: clamp_or(self.artifact_guard, DEFAULT_FRAMEGEN_ARTIFACT_GUARD),
            scd_sensitivity: clamp_or(self.scd_sensitivity, DEFAULT_FRAMEGEN_SCD_SENSITIVITY),
            hud_protection: clamp_or(self.hud_protection, DEFAULT_FRAMEGEN_HUD_PROTECTION),
        }
    }

    pub fn cache_bits(self) -> [u32; 3] {
        let s = self.sanitized();
        [
            s.artifact_guard.to_bits(),
            s.scd_sensitivity.to_bits(),
            s.hud_protection.to_bits(),
        ]
    }
}

/// Per-paint request passed from the presentation pacer into `CrtFilterRenderer`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameGenPaintRequest {
    /// Keep GPU frame history rotating so `F(t-1)` and `F(t)` are both available.
    pub enabled: bool,
    /// When true, synthesize and present the midpoint frame `F(t - 0.5)`.
    /// When false, present the real frame `F(t)`.
    pub show_interpolated: bool,
    /// Live-adjustable optical flow & interpolation quality parameters.
    pub params: FfxFrameGenParams,
}

/// Live execution telemetry for the Debug overlay (`D`) and GPU regression tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FfxFrameGenRunInfo {
    pub supported: bool,
    pub input_size: (u32, u32),
    pub generated_count: u64,
    pub scd_fallback_count: u64,
    pub last_scene_cut: bool,
    pub showing_interpolated: bool,
}

fn div_ceil_u32(n: u32, d: u32) -> u32 {
    n.div_ceil(d).max(1)
}

fn pyramid_dims(width: u32, height: u32) -> [(u32, u32); PYRAMID_LEVELS] {
    let mut dims = [(1, 1); PYRAMID_LEVELS];
    dims[0] = (width.max(1), height.max(1));
    for i in 1..PYRAMID_LEVELS {
        dims[i] = (
            dims[i - 1].0.div_ceil(2).max(1),
            dims[i - 1].1.div_ceil(2).max(1),
        );
    }
    dims
}

fn flow_grid_dims(level_w: u32, level_h: u32) -> (u32, u32) {
    (div_ceil_u32(level_w, 8), div_ceil_u32(level_h, 8))
}

unsafe fn gl_supports_compute_430(gl: &glow::Context) -> bool {
    let major = gl.get_parameter_i32(glow::MAJOR_VERSION);
    let minor = gl.get_parameter_i32(glow::MINOR_VERSION);
    major > 4 || (major == 4 && minor >= 3)
}

unsafe fn compile_compute_program(
    gl: &glow::Context,
    src: &str,
    name: &str,
) -> Result<glow::Program, String> {
    let shader = gl
        .create_shader(glow::COMPUTE_SHADER)
        .map_err(|e| format!("{name}: create_shader failed: {e}"))?;
    gl.shader_source(shader, src);
    gl.compile_shader(shader);
    if !gl.get_shader_compile_status(shader) {
        let log = gl.get_shader_info_log(shader);
        gl.delete_shader(shader);
        return Err(format!("{name}: compile error:\n{log}"));
    }

    let program = gl
        .create_program()
        .map_err(|e| format!("{name}: create_program failed: {e}"))?;
    gl.attach_shader(program, shader);
    gl.link_program(program);
    gl.detach_shader(program, shader);
    gl.delete_shader(shader);

    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.delete_program(program);
        return Err(format!("{name}: link error:\n{log}"));
    }

    Ok(program)
}

unsafe fn alloc_tex_2d(
    gl: &glow::Context,
    tex: glow::Texture,
    internal_format: u32,
    width: u32,
    height: u32,
    format: u32,
    ty: u32,
    filter: u32,
) {
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        internal_format as i32,
        width.max(1) as i32,
        height.max(1) as i32,
        0,
        format,
        ty,
        None,
    );
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_BASE_LEVEL, 0);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, 0);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter as i32);
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

struct FfxFrameGenGpu {
    prepare_luma_prog: glow::Program,
    prepare_luma_size_loc: Option<glow::UniformLocation>,

    pyramid_prog: glow::Program,
    pyramid_src_size_loc: Option<glow::UniformLocation>,
    pyramid_dst_size_loc: Option<glow::UniformLocation>,

    scd_prog: glow::Program,
    scd_level_size_loc: Option<glow::UniformLocation>,
    scd_reset_loc: Option<glow::UniformLocation>,
    scd_sensitivity_loc: Option<glow::UniformLocation>,

    search_prog: glow::Program,
    search_level_size_loc: Option<glow::UniformLocation>,
    search_grid_size_loc: Option<glow::UniformLocation>,
    search_has_coarse_loc: Option<glow::UniformLocation>,
    search_radius_loc: Option<glow::UniformLocation>,

    filter_scale_prog: glow::Program,
    filter_in_size_loc: Option<glow::UniformLocation>,
    filter_out_size_loc: Option<glow::UniformLocation>,
    filter_mode_loc: Option<glow::UniformLocation>,

    reproject_prog: glow::Program,
    reproject_grid_size_loc: Option<glow::UniformLocation>,
    reproject_mode_loc: Option<glow::UniformLocation>,
    reproject_consistency_thresh_loc: Option<glow::UniformLocation>,
    reproject_artifact_guard_loc: Option<glow::UniformLocation>,

    hud_mask_prog: glow::Program,
    hud_mask_frame_size_loc: Option<glow::UniformLocation>,
    hud_mask_grid_size_loc: Option<glow::UniformLocation>,
    hud_mask_hud_protection_loc: Option<glow::UniformLocation>,

    interp_prog: glow::Program,
    interp_frame_size_loc: Option<glow::UniformLocation>,
    interp_grid_size_loc: Option<glow::UniformLocation>,
    interp_artifact_guard_loc: Option<glow::UniformLocation>,
    interp_hud_protection_loc: Option<glow::UniformLocation>,

    // Ping-pong 7-level luminance pyramids (R8).
    luma_pyramids: [[glow::Texture; PYRAMID_LEVELS]; 2],
    luma_slot_key: [Option<(glow::Texture, i64)>; 2],
    next_luma_slot: usize,

    // Multi-scale optical flow scratch & level-0 forward/backward/midpoint outputs (RG16I).
    flow_raw: [glow::Texture; FLOW_SEARCH_LEVELS],
    flow_filt_coarse: [glow::Texture; FLOW_SEARCH_LEVELS],
    flow_up: [glow::Texture; FLOW_SEARCH_LEVELS],
    fwd_flow_l0: glow::Texture,
    bwd_flow_l0: glow::Texture,
    mid_flow_packed: glow::Texture, // R32UI
    mid_flow: glow::Texture,        // RG16I (raw unpacked + hole-filled)
    mid_flow_med: glow::Texture,    // RG16I (3x3 vector median filtered)
    mid_flow_smooth: glow::Texture, // RG16I (5x5 bilateral coherence smoothed)
    mid_conf: glow::Texture,        // R8 (cycle-consistency confidence map)
    mid_aux_raw: glow::Texture,     // RGBA8 (occ_prev, occ_curr, fg_mask, conf)
    mid_aux_smooth: glow::Texture,  // RGBA8 (5x5 Gaussian-blurred continuous matte)
    hud_mask: glow::Texture,        // R8 (precomputed full-res stationary HUD/reticle mask)

    // Dedicated linear clamp sampler so color warping stays smooth even when Point scaler is active.
    linear_sampler: glow::Sampler,

    // Scene Change Detection (3x1 R32UI + 16-byte SSBO).
    scd_tex: glow::Texture,
    scd_ssbo: glow::Buffer,
    scd_fence: Option<glow::Fence>,

    // Final interpolated linear-RGB RGBA16F frame.
    interp_tex: glow::Texture,

    allocated_size: (u32, u32),
    level_dims: [(u32, u32); PYRAMID_LEVELS],
    grid_dims: [(u32, u32); FLOW_SEARCH_LEVELS],
    coarsest_search_level: usize,
    scd_level: usize,
}

impl FfxFrameGenGpu {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let prepare_luma_prog =
            compile_compute_program(gl, CS_PREPARE_LUMA, "cs_ffx_of_prepare_luma")?;
        let pyramid_prog =
            match compile_compute_program(gl, CS_LUMA_PYRAMID, "cs_ffx_of_luma_pyramid") {
                Ok(p) => p,
                Err(e) => {
                    gl.delete_program(prepare_luma_prog);
                    return Err(e);
                }
            };
        let scd_prog = match compile_compute_program(gl, CS_SCD, "cs_ffx_of_scd") {
            Ok(p) => p,
            Err(e) => {
                gl.delete_program(prepare_luma_prog);
                gl.delete_program(pyramid_prog);
                return Err(e);
            }
        };
        let search_prog = match compile_compute_program(gl, CS_SEARCH_V5, "cs_ffx_of_search_v5") {
            Ok(p) => p,
            Err(e) => {
                gl.delete_program(prepare_luma_prog);
                gl.delete_program(pyramid_prog);
                gl.delete_program(scd_prog);
                return Err(e);
            }
        };
        let filter_scale_prog =
            match compile_compute_program(gl, CS_FILTER_SCALE_V5, "cs_ffx_of_filter_scale_v5") {
                Ok(p) => p,
                Err(e) => {
                    gl.delete_program(prepare_luma_prog);
                    gl.delete_program(pyramid_prog);
                    gl.delete_program(scd_prog);
                    gl.delete_program(search_prog);
                    return Err(e);
                }
            };
        let reproject_prog =
            match compile_compute_program(gl, CS_REPROJECT_MID, "cs_ffx_of_reproject_mid") {
                Ok(p) => p,
                Err(e) => {
                    gl.delete_program(prepare_luma_prog);
                    gl.delete_program(pyramid_prog);
                    gl.delete_program(scd_prog);
                    gl.delete_program(search_prog);
                    gl.delete_program(filter_scale_prog);
                    return Err(e);
                }
            };
        let hud_mask_prog = match compile_compute_program(gl, CS_HUD_MASK, "cs_ffx_hud_mask") {
            Ok(p) => p,
            Err(e) => {
                gl.delete_program(prepare_luma_prog);
                gl.delete_program(pyramid_prog);
                gl.delete_program(scd_prog);
                gl.delete_program(search_prog);
                gl.delete_program(filter_scale_prog);
                gl.delete_program(reproject_prog);
                return Err(e);
            }
        };
        let interp_prog = match compile_compute_program(gl, CS_FRAME_INTERP, "cs_ffx_frame_interp")
        {
            Ok(p) => p,
            Err(e) => {
                gl.delete_program(prepare_luma_prog);
                gl.delete_program(pyramid_prog);
                gl.delete_program(scd_prog);
                gl.delete_program(search_prog);
                gl.delete_program(filter_scale_prog);
                gl.delete_program(reproject_prog);
                gl.delete_program(hud_mask_prog);
                return Err(e);
            }
        };

        let prepare_luma_size_loc = gl.get_uniform_location(prepare_luma_prog, "u_input_size");
        let pyramid_src_size_loc = gl.get_uniform_location(pyramid_prog, "u_src_size");
        let pyramid_dst_size_loc = gl.get_uniform_location(pyramid_prog, "u_dst_size");
        let scd_level_size_loc = gl.get_uniform_location(scd_prog, "u_level_size");
        let scd_reset_loc = gl.get_uniform_location(scd_prog, "u_reset");
        let scd_sensitivity_loc = gl.get_uniform_location(scd_prog, "u_scd_sensitivity");
        let search_level_size_loc = gl.get_uniform_location(search_prog, "u_level_size");
        let search_grid_size_loc = gl.get_uniform_location(search_prog, "u_grid_size");
        let search_has_coarse_loc = gl.get_uniform_location(search_prog, "u_has_coarse");
        let search_radius_loc = gl.get_uniform_location(search_prog, "u_search_radius");
        let filter_in_size_loc = gl.get_uniform_location(filter_scale_prog, "u_in_size");
        let filter_out_size_loc = gl.get_uniform_location(filter_scale_prog, "u_out_size");
        let filter_mode_loc = gl.get_uniform_location(filter_scale_prog, "u_mode");
        let reproject_grid_size_loc = gl.get_uniform_location(reproject_prog, "u_grid_size");
        let reproject_mode_loc = gl.get_uniform_location(reproject_prog, "u_mode");
        let reproject_consistency_thresh_loc =
            gl.get_uniform_location(reproject_prog, "u_consistency_thresh");
        let reproject_artifact_guard_loc =
            gl.get_uniform_location(reproject_prog, "u_artifact_guard");
        let hud_mask_frame_size_loc = gl.get_uniform_location(hud_mask_prog, "u_frame_size");
        let hud_mask_grid_size_loc = gl.get_uniform_location(hud_mask_prog, "u_grid_size");
        let hud_mask_hud_protection_loc =
            gl.get_uniform_location(hud_mask_prog, "u_hud_protection");
        let interp_frame_size_loc = gl.get_uniform_location(interp_prog, "u_frame_size");
        let interp_grid_size_loc = gl.get_uniform_location(interp_prog, "u_grid_size");
        let interp_artifact_guard_loc = gl.get_uniform_location(interp_prog, "u_artifact_guard");
        let interp_hud_protection_loc = gl.get_uniform_location(interp_prog, "u_hud_protection");

        let make_tex = || {
            gl.create_texture()
                .map_err(|e| format!("create_texture: {e}"))
        };
        let luma_pyramids = [
            [
                make_tex()?,
                make_tex()?,
                make_tex()?,
                make_tex()?,
                make_tex()?,
                make_tex()?,
                make_tex()?,
            ],
            [
                make_tex()?,
                make_tex()?,
                make_tex()?,
                make_tex()?,
                make_tex()?,
                make_tex()?,
                make_tex()?,
            ],
        ];
        let flow_raw = [
            make_tex()?,
            make_tex()?,
            make_tex()?,
            make_tex()?,
            make_tex()?,
            make_tex()?,
        ];
        let flow_filt_coarse = [
            make_tex()?,
            make_tex()?,
            make_tex()?,
            make_tex()?,
            make_tex()?,
            make_tex()?,
        ];
        let flow_up = [
            make_tex()?,
            make_tex()?,
            make_tex()?,
            make_tex()?,
            make_tex()?,
            make_tex()?,
        ];
        let fwd_flow_l0 = make_tex()?;
        let bwd_flow_l0 = make_tex()?;
        let mid_flow_packed = make_tex()?;
        let mid_flow = make_tex()?;
        let mid_flow_med = make_tex()?;
        let mid_flow_smooth = make_tex()?;
        let mid_conf = make_tex()?;
        let mid_aux_raw = make_tex()?;
        let mid_aux_smooth = make_tex()?;
        let hud_mask = make_tex()?;
        let scd_tex = make_tex()?;
        let interp_tex = make_tex()?;

        let linear_sampler = gl
            .create_sampler()
            .map_err(|e| format!("create_sampler: {e}"))?;
        gl.sampler_parameter_i32(
            linear_sampler,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        gl.sampler_parameter_i32(
            linear_sampler,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.sampler_parameter_i32(
            linear_sampler,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.sampler_parameter_i32(
            linear_sampler,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );

        alloc_tex_2d(
            gl,
            scd_tex,
            glow::R32UI,
            3,
            1,
            glow::RED_INTEGER,
            glow::UNSIGNED_INT,
            glow::NEAREST,
        );
        gl.bind_texture(glow::TEXTURE_2D, None);

        let scd_ssbo = gl
            .create_buffer()
            .map_err(|e| format!("create_buffer(scd_ssbo): {e}"))?;
        let zero_stats = [0u32; 16];
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER, Some(scd_ssbo));
        gl.buffer_data_u8_slice(
            glow::SHADER_STORAGE_BUFFER,
            bytemuck::cast_slice(&zero_stats),
            glow::DYNAMIC_READ,
        );
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER, None);

        Ok(Self {
            prepare_luma_prog,
            prepare_luma_size_loc,
            pyramid_prog,
            pyramid_src_size_loc,
            pyramid_dst_size_loc,
            scd_prog,
            scd_level_size_loc,
            scd_reset_loc,
            scd_sensitivity_loc,
            search_prog,
            search_level_size_loc,
            search_grid_size_loc,
            search_has_coarse_loc,
            search_radius_loc,
            filter_scale_prog,
            filter_in_size_loc,
            filter_out_size_loc,
            filter_mode_loc,
            reproject_prog,
            reproject_grid_size_loc,
            reproject_mode_loc,
            reproject_consistency_thresh_loc,
            reproject_artifact_guard_loc,
            hud_mask_prog,
            hud_mask_frame_size_loc,
            hud_mask_grid_size_loc,
            hud_mask_hud_protection_loc,
            interp_prog,
            interp_frame_size_loc,
            interp_grid_size_loc,
            interp_artifact_guard_loc,
            interp_hud_protection_loc,
            luma_pyramids,
            luma_slot_key: [None, None],
            next_luma_slot: 0,
            flow_raw,
            flow_filt_coarse,
            flow_up,
            fwd_flow_l0,
            bwd_flow_l0,
            mid_flow_packed,
            mid_flow,
            mid_flow_med,
            mid_flow_smooth,
            mid_conf,
            mid_aux_raw,
            mid_aux_smooth,
            hud_mask,
            linear_sampler,
            scd_tex,
            scd_ssbo,
            scd_fence: None,
            interp_tex,
            allocated_size: (0, 0),
            level_dims: [(1, 1); PYRAMID_LEVELS],
            grid_dims: [(1, 1); FLOW_SEARCH_LEVELS],
            coarsest_search_level: 0,
            scd_level: 0,
        })
    }

    unsafe fn ensure_targets(&mut self, gl: &glow::Context, width: u32, height: u32) {
        if self.allocated_size == (width, height) {
            return;
        }

        let dims = pyramid_dims(width, height);
        let mut grid_dims = [(1, 1); FLOW_SEARCH_LEVELS];
        let mut coarsest = 0usize;
        for level in 0..FLOW_SEARCH_LEVELS {
            let (lw, lh) = dims[level];
            grid_dims[level] = flow_grid_dims(lw, lh);
            if lw >= 8 && lh >= 8 {
                coarsest = level;
            }
        }
        // Choose SCD level near 1/32 resolution while keeping at least 16x12 pixels on small frames.
        let mut scd_level = SCD_LEVEL.min(PYRAMID_LEVELS - 1);
        while scd_level > 0 && (dims[scd_level].0 < 16 || dims[scd_level].1 < 12) {
            scd_level -= 1;
        }

        for slot in 0..2 {
            for level in 0..PYRAMID_LEVELS {
                let (lw, lh) = dims[level];
                alloc_tex_2d(
                    gl,
                    self.luma_pyramids[slot][level],
                    glow::R8,
                    lw,
                    lh,
                    glow::RED,
                    glow::UNSIGNED_BYTE,
                    glow::LINEAR,
                );
            }
        }

        for level in 0..FLOW_SEARCH_LEVELS {
            let (gw, gh) = grid_dims[level];
            alloc_tex_2d(
                gl,
                self.flow_raw[level],
                glow::RG16I,
                gw,
                gh,
                glow::RG_INTEGER,
                glow::SHORT,
                glow::NEAREST,
            );
            alloc_tex_2d(
                gl,
                self.flow_filt_coarse[level],
                glow::RG16I,
                gw,
                gh,
                glow::RG_INTEGER,
                glow::SHORT,
                glow::NEAREST,
            );
            alloc_tex_2d(
                gl,
                self.flow_up[level],
                glow::RG16I,
                gw,
                gh,
                glow::RG_INTEGER,
                glow::SHORT,
                glow::NEAREST,
            );
        }

        let (gw0, gh0) = grid_dims[0];
        for &tex in &[
            self.fwd_flow_l0,
            self.bwd_flow_l0,
            self.mid_flow,
            self.mid_flow_med,
            self.mid_flow_smooth,
        ] {
            alloc_tex_2d(
                gl,
                tex,
                glow::RG16I,
                gw0,
                gh0,
                glow::RG_INTEGER,
                glow::SHORT,
                glow::NEAREST,
            );
        }
        alloc_tex_2d(
            gl,
            self.mid_flow_packed,
            glow::R32UI,
            gw0,
            gh0,
            glow::RED_INTEGER,
            glow::UNSIGNED_INT,
            glow::NEAREST,
        );
        alloc_tex_2d(
            gl,
            self.mid_conf,
            glow::R8,
            gw0,
            gh0,
            glow::RED,
            glow::UNSIGNED_BYTE,
            glow::LINEAR,
        );
        for &tex in &[self.mid_aux_raw, self.mid_aux_smooth] {
            alloc_tex_2d(
                gl,
                tex,
                glow::RGBA8,
                gw0,
                gh0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::LINEAR,
            );
        }
        alloc_tex_2d(
            gl,
            self.hud_mask,
            glow::R8,
            width,
            height,
            glow::RED,
            glow::UNSIGNED_BYTE,
            glow::LINEAR,
        );
        alloc_tex_2d(
            gl,
            self.interp_tex,
            glow::RGBA16F,
            width,
            height,
            glow::RGBA,
            glow::HALF_FLOAT,
            glow::LINEAR,
        );

        gl.bind_texture(glow::TEXTURE_2D, None);

        self.allocated_size = (width, height);
        self.level_dims = dims;
        self.grid_dims = grid_dims;
        self.coarsest_search_level = coarsest;
        self.scd_level = scd_level;
        self.luma_slot_key = [None, None];
    }

    unsafe fn build_luma_pyramid(
        &mut self,
        gl: &glow::Context,
        slot: usize,
        color_tex: glow::Texture,
        captured_at: i64,
    ) {
        if self.luma_slot_key[slot] == Some((color_tex, captured_at)) {
            return;
        }
        let (w0, h0) = self.level_dims[0];

        // Level 0: linear RGBA16F -> perceptual sRGB luma (R8).
        gl.use_program(Some(self.prepare_luma_prog));
        gl.uniform_2_i32(self.prepare_luma_size_loc.as_ref(), w0 as i32, h0 as i32);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(color_tex));
        gl.bind_image_texture(
            0,
            self.luma_pyramids[slot][0],
            0,
            false,
            0,
            glow::WRITE_ONLY,
            glow::R8,
        );
        gl.dispatch_compute(div_ceil_u32(w0, 16), div_ceil_u32(h0, 16), 1);
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT | glow::TEXTURE_FETCH_BARRIER_BIT);

        // Levels 1..PYRAMID_LEVELS: 2x2 box pyramid reduction.
        gl.use_program(Some(self.pyramid_prog));
        for level in 1..PYRAMID_LEVELS {
            let (sw, sh) = self.level_dims[level - 1];
            let (dw, dh) = self.level_dims[level];
            gl.uniform_2_i32(self.pyramid_src_size_loc.as_ref(), sw as i32, sh as i32);
            gl.uniform_2_i32(self.pyramid_dst_size_loc.as_ref(), dw as i32, dh as i32);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.luma_pyramids[slot][level - 1]));
            gl.bind_image_texture(
                0,
                self.luma_pyramids[slot][level],
                0,
                false,
                0,
                glow::WRITE_ONLY,
                glow::R8,
            );
            gl.dispatch_compute(div_ceil_u32(dw, 16), div_ceil_u32(dh, 16), 1);
            gl.memory_barrier(
                glow::SHADER_IMAGE_ACCESS_BARRIER_BIT | glow::TEXTURE_FETCH_BARRIER_BIT,
            );
        }

        self.luma_slot_key[slot] = Some((color_tex, captured_at));
    }

    unsafe fn run_optical_flow_direction(
        &self,
        gl: &glow::Context,
        src_slot: usize,
        ref_slot: usize,
        out_l0_tex: glow::Texture,
    ) {
        let coarsest = self.coarsest_search_level;
        for level in (0..=coarsest).rev() {
            let (lw, lh) = self.level_dims[level];
            let (gw, gh) = self.grid_dims[level];
            let has_coarse = level < coarsest;

            // 1. Block-matching SAD search v5 at `level`.
            //    Coarsest level uses +/-8px radius (covering +/-256px at 1080p level 5),
            //    intermediate levels use +/-4px, and full-res level 0 uses +/-2px + outer +/-4 ring.
            let search_radius = if level == 0 {
                2
            } else if level == coarsest {
                8
            } else {
                4
            };
            gl.use_program(Some(self.search_prog));
            gl.uniform_2_i32(self.search_level_size_loc.as_ref(), lw as i32, lh as i32);
            gl.uniform_2_i32(self.search_grid_size_loc.as_ref(), gw as i32, gh as i32);
            gl.uniform_1_i32(
                self.search_has_coarse_loc.as_ref(),
                if has_coarse { 1 } else { 0 },
            );
            gl.uniform_1_i32(self.search_radius_loc.as_ref(), search_radius);

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.luma_pyramids[src_slot][level]));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.luma_pyramids[ref_slot][level]));
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(
                glow::TEXTURE_2D,
                if has_coarse {
                    Some(self.flow_up[level])
                } else {
                    None
                },
            );
            gl.bind_image_texture(
                0,
                self.flow_raw[level],
                0,
                false,
                0,
                glow::WRITE_ONLY,
                glow::RG16I,
            );
            gl.dispatch_compute(div_ceil_u32(gw, 8), div_ceil_u32(gh, 8), 1);
            gl.memory_barrier(
                glow::SHADER_IMAGE_ACCESS_BARRIER_BIT | glow::TEXTURE_FETCH_BARRIER_BIT,
            );

            // 2. 3x3 spatial median filter (outlier rejection).
            let filt_target = if level == 0 {
                out_l0_tex
            } else {
                self.flow_filt_coarse[level]
            };
            gl.use_program(Some(self.filter_scale_prog));
            gl.uniform_2_i32(self.filter_in_size_loc.as_ref(), gw as i32, gh as i32);
            gl.uniform_2_i32(self.filter_out_size_loc.as_ref(), gw as i32, gh as i32);
            gl.uniform_1_i32(self.filter_mode_loc.as_ref(), 0);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.flow_raw[level]));
            gl.bind_image_texture(0, filt_target, 0, false, 0, glow::WRITE_ONLY, glow::RG16I);
            gl.dispatch_compute(div_ceil_u32(gw, 8), div_ceil_u32(gh, 8), 1);
            gl.memory_barrier(
                glow::SHADER_IMAGE_ACCESS_BARRIER_BIT | glow::TEXTURE_FETCH_BARRIER_BIT,
            );

            // 3. Edge-preserving 2x upscale to level - 1.
            if level > 0 {
                let (next_gw, next_gh) = self.grid_dims[level - 1];
                gl.uniform_2_i32(self.filter_in_size_loc.as_ref(), gw as i32, gh as i32);
                gl.uniform_2_i32(
                    self.filter_out_size_loc.as_ref(),
                    next_gw as i32,
                    next_gh as i32,
                );
                gl.uniform_1_i32(self.filter_mode_loc.as_ref(), 1);
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(filt_target));
                gl.bind_image_texture(
                    0,
                    self.flow_up[level - 1],
                    0,
                    false,
                    0,
                    glow::WRITE_ONLY,
                    glow::RG16I,
                );
                gl.dispatch_compute(div_ceil_u32(next_gw, 8), div_ceil_u32(next_gh, 8), 1);
                gl.memory_barrier(
                    glow::SHADER_IMAGE_ACCESS_BARRIER_BIT | glow::TEXTURE_FETCH_BARRIER_BIT,
                );
            }
        }
    }

    unsafe fn run_scd(
        &mut self,
        gl: &glow::Context,
        prev_slot: usize,
        curr_slot: usize,
        reset: bool,
        scd_sensitivity: f32,
    ) {
        let (sw, sh) = self.level_dims[self.scd_level];
        gl.use_program(Some(self.scd_prog));
        gl.uniform_2_i32(self.scd_level_size_loc.as_ref(), sw as i32, sh as i32);
        gl.uniform_1_i32(self.scd_reset_loc.as_ref(), if reset { 1 } else { 0 });
        gl.uniform_1_f32(self.scd_sensitivity_loc.as_ref(), scd_sensitivity);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(
            glow::TEXTURE_2D,
            Some(self.luma_pyramids[prev_slot][self.scd_level]),
        );
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(
            glow::TEXTURE_2D,
            Some(self.luma_pyramids[curr_slot][self.scd_level]),
        );
        gl.bind_image_texture(0, self.scd_tex, 0, false, 0, glow::WRITE_ONLY, glow::R32UI);
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 0, Some(self.scd_ssbo));
        gl.dispatch_compute(1, 1, 1);
        gl.memory_barrier(
            glow::SHADER_IMAGE_ACCESS_BARRIER_BIT
                | glow::TEXTURE_FETCH_BARRIER_BIT
                | glow::SHADER_STORAGE_BARRIER_BIT,
        );
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 0, None);
    }

    unsafe fn run_midpoint_reprojection(&mut self, gl: &glow::Context, artifact_guard: f32) {
        let (gw, gh) = self.grid_dims[0];
        let groups_x = div_ceil_u32(gw, 8);
        let groups_y = div_ceil_u32(gh, 8);
        // Tighter cycle-consistency tolerance at higher artifact_guard (4.5px @ 0.0 -> 1.4px @ 1.0).
        let guard = artifact_guard.clamp(0.0, 1.0);
        let consistency_thresh = 4.5 + (1.4 - 4.5) * guard;

        gl.use_program(Some(self.reproject_prog));
        gl.uniform_2_i32(self.reproject_grid_size_loc.as_ref(), gw as i32, gh as i32);
        gl.uniform_1_f32(
            self.reproject_consistency_thresh_loc.as_ref(),
            consistency_thresh,
        );
        gl.uniform_1_f32(self.reproject_artifact_guard_loc.as_ref(), guard);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.fwd_flow_l0));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.bwd_flow_l0));
        gl.bind_image_texture(
            0,
            self.mid_flow_packed,
            0,
            false,
            0,
            glow::READ_WRITE,
            glow::R32UI,
        );
        gl.bind_image_texture(1, self.mid_flow, 0, false, 0, glow::WRITE_ONLY, glow::RG16I);
        gl.bind_image_texture(2, self.mid_conf, 0, false, 0, glow::WRITE_ONLY, glow::R8);
        gl.bind_image_texture(3, self.scd_tex, 0, false, 0, glow::READ_WRITE, glow::R32UI);
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 0, Some(self.scd_ssbo));

        // Pass 0: Clear packed midpoint grid & reset consistency counters.
        gl.uniform_1_i32(self.reproject_mode_loc.as_ref(), 0);
        gl.dispatch_compute(groups_x, groups_y, 1);
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT | glow::SHADER_STORAGE_BARRIER_BIT);

        // Pass 0b: Estimate dominant global background velocity mode (`v_global`) via 8x8 shared
        // density peak finder BEFORE Pass 1 so splat priority favors foreground occluders.
        gl.uniform_1_i32(self.reproject_mode_loc.as_ref(), 4);
        gl.dispatch_compute(1, 1, 1);
        gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT);

        // Pass 1: Cycle-consistency validated midpoint splat (foreground-occluder priority).
        gl.uniform_1_i32(self.reproject_mode_loc.as_ref(), 1);
        gl.dispatch_compute(groups_x, groups_y, 1);
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT | glow::SHADER_STORAGE_BARRIER_BIT);

        // Pass 2: Foreground-preserving hybrid unpack + dense 2-iteration fixed-point midpoint gather.
        gl.uniform_1_i32(self.reproject_mode_loc.as_ref(), 2);
        gl.dispatch_compute(groups_x, groups_y, 1);
        gl.memory_barrier(
            glow::SHADER_IMAGE_ACCESS_BARRIER_BIT
                | glow::TEXTURE_FETCH_BARRIER_BIT
                | glow::SHADER_STORAGE_BARRIER_BIT,
        );

        // Pass 3: AFMF-style global fast-motion / consistency quality gate.
        gl.uniform_1_i32(self.reproject_mode_loc.as_ref(), 3);
        gl.dispatch_compute(1, 1, 1);
        gl.memory_barrier(
            glow::SHADER_IMAGE_ACCESS_BARRIER_BIT
                | glow::TEXTURE_FETCH_BARRIER_BIT
                | glow::SHADER_STORAGE_BARRIER_BIT,
        );
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 0, None);

        if let Some(old_fence) = self.scd_fence.take() {
            gl.delete_sync(old_fence);
        }
        self.scd_fence = gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0).ok();

        // Pass 4a: First 3x3 vector median filter on midpoint flow (removes isolated outlier blocks).
        gl.use_program(Some(self.filter_scale_prog));
        gl.uniform_2_i32(self.filter_in_size_loc.as_ref(), gw as i32, gh as i32);
        gl.uniform_2_i32(self.filter_out_size_loc.as_ref(), gw as i32, gh as i32);
        gl.uniform_1_i32(self.filter_mode_loc.as_ref(), 0);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.mid_flow));
        gl.bind_image_texture(
            0,
            self.mid_flow_med,
            0,
            false,
            0,
            glow::WRITE_ONLY,
            glow::RG16I,
        );
        gl.dispatch_compute(groups_x, groups_y, 1);
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT | glow::TEXTURE_FETCH_BARRIER_BIT);

        // Pass 4b: Second 3x3 vector median + wide foreground interior shadow-hole seal
        // (seals wide dark-shirt / cast-shadow holes BEFORE Pass 4c's island seal + bilateral pass).
        gl.uniform_1_i32(self.filter_mode_loc.as_ref(), 4);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.mid_flow_med));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.mid_conf));
        gl.bind_image_texture(0, self.mid_flow, 0, false, 0, glow::WRITE_ONLY, glow::RG16I);
        gl.dispatch_compute(groups_x, groups_y, 1);
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT | glow::TEXTURE_FETCH_BARRIER_BIT);

        // Pass 4c: Edge-preserving 5x5 bilateral coherence filter (smooth surfaces, sharp silhouettes).
        gl.uniform_1_i32(self.filter_mode_loc.as_ref(), 2);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.mid_flow));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.mid_conf));
        gl.bind_image_texture(
            0,
            self.mid_flow_smooth,
            0,
            false,
            0,
            glow::WRITE_ONLY,
            glow::RG16I,
        );
        gl.dispatch_compute(groups_x, groups_y, 1);
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT | glow::TEXTURE_FETCH_BARRIER_BIT);

        // Pass 5a: Build continuous RGBA8 auxiliary matte (occ_prev, occ_curr, fg_mask, conf)
        // using exact endpoint flow timestamps `fwd_flow_l0` (t = 0) and `bwd_flow_l0` (t = 1).
        gl.use_program(Some(self.reproject_prog));
        gl.uniform_1_i32(self.reproject_mode_loc.as_ref(), 5);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.fwd_flow_l0));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.bwd_flow_l0));
        gl.active_texture(glow::TEXTURE2);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.mid_flow_smooth));
        gl.active_texture(glow::TEXTURE3);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.mid_conf));
        gl.bind_image_texture(
            4,
            self.mid_aux_raw,
            0,
            false,
            0,
            glow::WRITE_ONLY,
            glow::RGBA8,
        );
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 0, Some(self.scd_ssbo));
        gl.dispatch_compute(groups_x, groups_y, 1);
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT | glow::TEXTURE_FETCH_BARRIER_BIT);
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 0, None);

        // Pass 5b: 5x5 Gaussian blur on auxiliary matte -> C1-continuous, zero 8x8 block edges!
        gl.use_program(Some(self.filter_scale_prog));
        gl.uniform_2_i32(self.filter_in_size_loc.as_ref(), gw as i32, gh as i32);
        gl.uniform_2_i32(self.filter_out_size_loc.as_ref(), gw as i32, gh as i32);
        gl.uniform_1_i32(self.filter_mode_loc.as_ref(), 3);
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.mid_aux_raw));
        gl.bind_image_texture(
            1,
            self.mid_aux_smooth,
            0,
            false,
            0,
            glow::WRITE_ONLY,
            glow::RGBA8,
        );
        gl.dispatch_compute(groups_x, groups_y, 1);
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT | glow::TEXTURE_FETCH_BARRIER_BIT);
    }

    unsafe fn run_hud_mask(
        &self,
        gl: &glow::Context,
        prev_slot: usize,
        curr_slot: usize,
        hud_protection: f32,
    ) {
        let (w, h) = self.allocated_size;
        let (gw, gh) = self.grid_dims[0];

        gl.use_program(Some(self.hud_mask_prog));
        gl.uniform_2_i32(self.hud_mask_frame_size_loc.as_ref(), w as i32, h as i32);
        gl.uniform_2_i32(self.hud_mask_grid_size_loc.as_ref(), gw as i32, gh as i32);
        gl.uniform_1_f32(self.hud_mask_hud_protection_loc.as_ref(), hud_protection);

        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.luma_pyramids[prev_slot][0]));
        gl.bind_sampler(0, Some(self.linear_sampler));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.luma_pyramids[curr_slot][0]));
        gl.bind_sampler(1, Some(self.linear_sampler));
        gl.active_texture(glow::TEXTURE2);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.mid_flow_smooth));
        gl.active_texture(glow::TEXTURE3);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.scd_tex));
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 0, Some(self.scd_ssbo));

        gl.bind_image_texture(0, self.hud_mask, 0, false, 0, glow::WRITE_ONLY, glow::R8);
        gl.dispatch_compute(div_ceil_u32(w, 16), div_ceil_u32(h, 16), 1);
        gl.memory_barrier(glow::TEXTURE_FETCH_BARRIER_BIT | glow::SHADER_IMAGE_ACCESS_BARRIER_BIT);

        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 0, None);
        gl.bind_sampler(0, None);
        gl.bind_sampler(1, None);
    }

    unsafe fn run_frame_interp(
        &self,
        gl: &glow::Context,
        prev_color: glow::Texture,
        curr_color: glow::Texture,
        params: FfxFrameGenParams,
    ) {
        let (w, h) = self.allocated_size;
        let (gw, gh) = self.grid_dims[0];

        gl.use_program(Some(self.interp_prog));
        gl.uniform_2_i32(self.interp_frame_size_loc.as_ref(), w as i32, h as i32);
        gl.uniform_2_i32(self.interp_grid_size_loc.as_ref(), gw as i32, gh as i32);
        gl.uniform_1_f32(
            self.interp_artifact_guard_loc.as_ref(),
            params.artifact_guard,
        );
        gl.uniform_1_f32(
            self.interp_hud_protection_loc.as_ref(),
            params.hud_protection,
        );

        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(prev_color));
        gl.bind_sampler(0, Some(self.linear_sampler));
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, Some(curr_color));
        gl.bind_sampler(1, Some(self.linear_sampler));
        gl.active_texture(glow::TEXTURE2);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.fwd_flow_l0));
        gl.active_texture(glow::TEXTURE3);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.bwd_flow_l0));
        gl.active_texture(glow::TEXTURE4);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.mid_flow_smooth));
        gl.active_texture(glow::TEXTURE5);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.scd_tex));
        gl.active_texture(glow::TEXTURE6);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.mid_aux_smooth));
        gl.bind_sampler(6, Some(self.linear_sampler));
        gl.active_texture(glow::TEXTURE7);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.hud_mask));
        gl.bind_sampler(7, Some(self.linear_sampler));
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 0, Some(self.scd_ssbo));

        gl.bind_image_texture(
            0,
            self.interp_tex,
            0,
            false,
            0,
            glow::WRITE_ONLY,
            glow::RGBA16F,
        );
        gl.dispatch_compute(div_ceil_u32(w, 16), div_ceil_u32(h, 16), 1);
        gl.memory_barrier(glow::TEXTURE_FETCH_BARRIER_BIT | glow::SHADER_IMAGE_ACCESS_BARRIER_BIT);

        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 0, None);
        gl.bind_sampler(7, None);
        gl.bind_sampler(6, None);
        gl.bind_sampler(0, None);
        gl.bind_sampler(1, None);
    }

    unsafe fn poll_scd_telemetry(
        &mut self,
        gl: &glow::Context,
        blocking: bool,
    ) -> Option<[u32; 16]> {
        let fence = self.scd_fence?;
        let status = if blocking {
            gl.client_wait_sync(fence, glow::SYNC_FLUSH_COMMANDS_BIT, 1_000_000_000)
        } else {
            gl.client_wait_sync(fence, 0, 0)
        };
        if status != glow::ALREADY_SIGNALED && status != glow::CONDITION_SATISFIED {
            if status == glow::WAIT_FAILED {
                self.scd_fence = None;
                gl.delete_sync(fence);
            }
            return None;
        }
        self.scd_fence = None;
        gl.delete_sync(fence);

        let mut data = [0u32; 16];
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER, Some(self.scd_ssbo));
        gl.get_buffer_sub_data(
            glow::SHADER_STORAGE_BUFFER,
            0,
            bytemuck::cast_slice_mut(&mut data),
        );
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER, None);
        Some(data)
    }

    unsafe fn destroy(&self, gl: &glow::Context) {
        if let Some(fence) = self.scd_fence {
            gl.delete_sync(fence);
        }
        gl.delete_program(self.prepare_luma_prog);
        gl.delete_program(self.pyramid_prog);
        gl.delete_program(self.scd_prog);
        gl.delete_program(self.search_prog);
        gl.delete_program(self.filter_scale_prog);
        gl.delete_program(self.reproject_prog);
        gl.delete_program(self.hud_mask_prog);
        gl.delete_program(self.interp_prog);

        for slot in 0..2 {
            for level in 0..PYRAMID_LEVELS {
                gl.delete_texture(self.luma_pyramids[slot][level]);
            }
        }
        for level in 0..FLOW_SEARCH_LEVELS {
            gl.delete_texture(self.flow_raw[level]);
            gl.delete_texture(self.flow_filt_coarse[level]);
            gl.delete_texture(self.flow_up[level]);
        }
        gl.delete_texture(self.fwd_flow_l0);
        gl.delete_texture(self.bwd_flow_l0);
        gl.delete_texture(self.mid_flow_packed);
        gl.delete_texture(self.mid_flow);
        gl.delete_texture(self.mid_flow_med);
        gl.delete_texture(self.mid_flow_smooth);
        gl.delete_texture(self.mid_conf);
        gl.delete_texture(self.mid_aux_raw);
        gl.delete_texture(self.mid_aux_smooth);
        gl.delete_texture(self.hud_mask);
        gl.delete_sampler(self.linear_sampler);
        gl.delete_texture(self.scd_tex);
        gl.delete_texture(self.interp_tex);
        gl.delete_buffer(self.scd_ssbo);
    }
}

/// GPU manager for AMD FidelityFX Optical Flow v5 + midpoint frame interpolation.
pub struct FfxFrameGen {
    gpu: Option<FfxFrameGenGpu>,
    init_attempted: bool,
    supported: bool,
    init_error: Option<String>,
    last_pair: Option<(i64, i64, u32, u32, [u32; 3])>,
    generated_count: u64,
    scd_fallback_count: u64,
    last_scene_cut: bool,
    last_run: Option<FfxFrameGenRunInfo>,
}

// SAFETY: `FfxFrameGen` holds opaque GL handles (including `glow::Fence`) that are only
// accessed on the main thread while the owning OpenGL context is current (`paint`,
// `draw_passthrough`, and `destroy`).
unsafe impl Send for FfxFrameGen {}

impl FfxFrameGen {
    pub fn new(_gl: &glow::Context) -> Self {
        Self {
            gpu: None,
            init_attempted: false,
            supported: true,
            init_error: None,
            last_pair: None,
            generated_count: 0,
            scd_fallback_count: 0,
            last_scene_cut: false,
            last_run: None,
        }
    }

    pub fn last_run(&self) -> Option<FfxFrameGenRunInfo> {
        self.last_run
    }

    pub fn init_error(&self) -> Option<&str> {
        self.init_error.as_deref()
    }

    pub fn reset_history(&mut self) {
        self.last_pair = None;
        if let Some(gpu) = &mut self.gpu {
            gpu.luma_slot_key = [None, None];
        }
    }

    /// Synthesizes (or returns cached) midpoint frame `F(t - 0.5)` between linear-RGB
    /// `prev_tex` and `curr_tex`. Falls back cleanly to `curr_tex` if OpenGL 4.3 compute
    /// shaders are unavailable.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn interpolate(
        &mut self,
        gl: &glow::Context,
        prev_tex: glow::Texture,
        curr_tex: glow::Texture,
        prev_captured_at: i64,
        curr_captured_at: i64,
        width: u32,
        height: u32,
        show_interpolated: bool,
        params: FfxFrameGenParams,
    ) -> glow::Texture {
        if width == 0 || height == 0 {
            return curr_tex;
        }
        let params = params.sanitized();

        if !self.init_attempted {
            self.init_attempted = true;
            if !gl_supports_compute_430(gl) {
                let major = gl.get_parameter_i32(glow::MAJOR_VERSION);
                let minor = gl.get_parameter_i32(glow::MINOR_VERSION);
                let msg =
                    format!("OpenGL {major}.{minor} does not support GL 4.3+ compute shaders");
                tracing::warn!("AMD FidelityFX Frame Gen disabled: {msg}");
                self.init_error = Some(msg);
                self.supported = false;
            } else {
                match FfxFrameGenGpu::new(gl) {
                    Ok(gpu) => {
                        self.gpu = Some(gpu);
                        self.supported = true;
                    }
                    Err(err) => {
                        tracing::error!("AMD FidelityFX Frame Gen compute init failed: {err}");
                        self.init_error = Some(err);
                        self.supported = false;
                    }
                }
            }
        }

        let Some(gpu) = self.gpu.as_mut() else {
            self.last_run = Some(FfxFrameGenRunInfo {
                supported: false,
                input_size: (width, height),
                generated_count: self.generated_count,
                scd_fallback_count: self.scd_fallback_count,
                last_scene_cut: false,
                showing_interpolated: false,
            });
            return curr_tex;
        };

        // Poll any completed non-blocking SCD telemetry from the previous dispatch.
        if let Some(stats) = gpu.poll_scd_telemetry(gl, false) {
            self.last_scene_cut = stats[0] != 0;
            self.scd_fallback_count = stats[3] as u64;
        }

        if !show_interpolated {
            self.last_run = Some(FfxFrameGenRunInfo {
                supported: true,
                input_size: (width, height),
                generated_count: self.generated_count,
                scd_fallback_count: self.scd_fallback_count,
                last_scene_cut: self.last_scene_cut,
                showing_interpolated: false,
            });
            return curr_tex;
        }

        let param_bits = params.cache_bits();
        let pair_key = (
            prev_captured_at,
            curr_captured_at,
            width,
            height,
            param_bits,
        );
        if self.last_pair == Some(pair_key) && gpu.allocated_size == (width, height) {
            self.last_run = Some(FfxFrameGenRunInfo {
                supported: true,
                input_size: (width, height),
                generated_count: self.generated_count,
                scd_fallback_count: self.scd_fallback_count,
                last_scene_cut: self.last_scene_cut,
                showing_interpolated: true,
            });
            return gpu.interp_tex;
        }

        // Save caller GL bindings and unbind active FBO + flush framebuffer writes before compute
        // reads `curr_tex` (prevents CB/DCC cache hazards on AMD RadeonSI / RDNA3 GPUs).
        let prev_active_tex = gl.get_parameter_i32(glow::ACTIVE_TEXTURE) as u32;
        let prev_draw_fbo = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.memory_barrier(glow::FRAMEBUFFER_BARRIER_BIT | glow::TEXTURE_FETCH_BARRIER_BIT);

        let reset_scd = gpu.allocated_size != (width, height) || self.last_pair.is_none();
        gpu.ensure_targets(gl, width, height);

        // Reuse cached pyramid slot if `prev_tex` was already processed on the prior frame.
        let prev_slot = if gpu.luma_slot_key[0] == Some((prev_tex, prev_captured_at)) {
            0
        } else if gpu.luma_slot_key[1] == Some((prev_tex, prev_captured_at)) {
            1
        } else {
            gpu.next_luma_slot
        };
        let curr_slot = 1 - prev_slot;
        gpu.next_luma_slot = prev_slot;

        gpu.build_luma_pyramid(gl, prev_slot, prev_tex, prev_captured_at);
        gpu.build_luma_pyramid(gl, curr_slot, curr_tex, curr_captured_at);
        gpu.run_scd(gl, prev_slot, curr_slot, reset_scd, params.scd_sensitivity);
        gpu.run_optical_flow_direction(gl, prev_slot, curr_slot, gpu.fwd_flow_l0);
        gpu.run_optical_flow_direction(gl, curr_slot, prev_slot, gpu.bwd_flow_l0);
        gpu.run_midpoint_reprojection(gl, params.artifact_guard);
        gpu.run_hud_mask(gl, prev_slot, curr_slot, params.hud_protection);
        gpu.run_frame_interp(gl, prev_tex, curr_tex, params);

        // Restore texture units, FBO, and shader program state cleanly for downstream passes.
        for unit in (0..=7u32).rev() {
            gl.active_texture(glow::TEXTURE0 + unit);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        gl.active_texture(prev_active_tex);
        gl.use_program(None);
        if prev_draw_fbo == 0 {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        } else if let Some(nz) = std::num::NonZero::new(prev_draw_fbo as u32) {
            gl.bind_framebuffer(
                glow::FRAMEBUFFER,
                Some(glow::Framebuffer::from(glow::NativeFramebuffer(nz))),
            );
        }

        self.last_pair = Some(pair_key);
        self.generated_count = self.generated_count.saturating_add(1);
        self.last_run = Some(FfxFrameGenRunInfo {
            supported: true,
            input_size: (width, height),
            generated_count: self.generated_count,
            scd_fallback_count: self.scd_fallback_count,
            last_scene_cut: self.last_scene_cut,
            showing_interpolated: true,
        });

        gpu.interp_tex
    }

    /// Synchronously drains the most recent SCD telemetry SSBO (used in GPU tests).
    #[cfg(test)]
    pub unsafe fn flush_scd_telemetry_sync(&mut self, gl: &glow::Context) {
        if let Some(gpu) = self.gpu.as_mut() {
            if let Some(stats) = gpu.poll_scd_telemetry(gl, true) {
                self.last_scene_cut = stats[0] != 0;
                self.scd_fallback_count = stats[3] as u64;
                if let Some(ref mut run) = self.last_run {
                    run.last_scene_cut = self.last_scene_cut;
                    run.scd_fallback_count = self.scd_fallback_count;
                }
            }
        }
    }

    pub unsafe fn destroy(&self, gl: &glow::Context) {
        if let Some(ref gpu) = self.gpu {
            gpu.destroy(gl);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyramid_dimensions_round_up_cleanly_for_1080p_and_small_frames() {
        let p1080 = pyramid_dims(1920, 1080);
        assert_eq!(p1080[0], (1920, 1080));
        assert_eq!(p1080[1], (960, 540));
        assert_eq!(p1080[2], (480, 270));
        assert_eq!(p1080[3], (240, 135));
        assert_eq!(p1080[4], (120, 68));
        assert_eq!(p1080[5], (60, 34));
        assert_eq!(p1080[6], (30, 17));
        assert_eq!(flow_grid_dims(1920, 1080), (240, 135));

        let tiny = pyramid_dims(16, 16);
        assert_eq!(tiny[0], (16, 16));
        assert_eq!(tiny[4], (1, 1));
        assert_eq!(tiny[6], (1, 1));
    }

    #[test]
    fn framegen_params_sanitize_nans_and_out_of_range_values() {
        let dirty = FfxFrameGenParams {
            artifact_guard: f32::NAN,
            scd_sensitivity: 2.5,
            hud_protection: -0.4,
        };
        let clean = dirty.sanitized();
        assert_eq!(clean.artifact_guard, DEFAULT_FRAMEGEN_ARTIFACT_GUARD);
        assert_eq!(clean.scd_sensitivity, 1.0);
        assert_eq!(clean.hud_protection, 0.0);
    }
}
