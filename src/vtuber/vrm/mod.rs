pub mod loader;
pub mod springbone;

use crate::vtuber::rig::{
    ArkitBlendshape, EvaluatedAvatarPose, VrmPresetExpression, ARKIT_BLENDSHAPE_COUNT,
};
use eframe::glow::{self, HasContext};
use glam::{Mat4, Quat, Vec3};
use loader::{
    compute_bust_camera_matrices, AlphaModeKind, DecodedRgbaTexture, ExpressionBinding,
    LoadedVrmAsset, MorphDeltaArrayCpu, OutlineModeKind, VrmVertex, MAX_JOINTS_PER_PRIMITIVE,
};
use springbone::SpringBoneWorld;

const VS_VRM_SKIN_SRC: &str = include_str!("shaders/vs_vrm_skin.glsl");
const FS_MTOON_SRC: &str = include_str!("shaders/fs_mtoon.glsl");
const MAX_ACTIVE_MORPHS_PER_DRAW: usize = 64;

/// CPU-side skeletal, morph-target, and secondary physics (SpringBone) evaluator for a loaded VRM.
pub struct VrmAvatarCpuAnimator {
    pub asset: LoadedVrmAsset,
    pub spring_world: SpringBoneWorld,
    pub local_rotations: Vec<Quat>,
    pub local_translations: Vec<Vec3>,
    pub world_matrices: Vec<Mat4>,
    mesh_morph_weights: Vec<Vec<f32>>,
    pub primitive_active_morphs: Vec<Vec<(i32, f32)>>,
    rest_left_arm_dir: Vec3,
    rest_right_arm_dir: Vec3,
}

impl VrmAvatarCpuAnimator {
    pub fn new(asset: LoadedVrmAsset) -> Self {
        let local_rotations: Vec<Quat> = asset.nodes.iter().map(|n| n.rest_rotation).collect();
        let local_translations: Vec<Vec3> =
            asset.nodes.iter().map(|n| n.rest_translation).collect();
        let world_matrices: Vec<Mat4> = asset.nodes.iter().map(|n| n.rest_world_matrix).collect();

        let max_mesh_idx = asset
            .primitives
            .iter()
            .map(|p| p.mesh_idx)
            .max()
            .unwrap_or(0);
        let mut mesh_target_counts = vec![0usize; max_mesh_idx + 1];
        for prim in &asset.primitives {
            mesh_target_counts[prim.mesh_idx] =
                mesh_target_counts[prim.mesh_idx].max(prim.morph_target_count);
        }
        let mesh_morph_weights = mesh_target_counts
            .into_iter()
            .map(|cnt| vec![0.0f32; cnt])
            .collect();
        let primitive_active_morphs = vec![Vec::new(); asset.primitives.len()];

        let rest_left_arm_dir = compute_rest_arm_world_dir(
            &asset,
            asset.humanoid.left_upper_arm,
            asset.humanoid.left_lower_arm,
            Vec3::X,
        );
        let rest_right_arm_dir = compute_rest_arm_world_dir(
            &asset,
            asset.humanoid.right_upper_arm,
            asset.humanoid.right_lower_arm,
            Vec3::NEG_X,
        );

        let spring_world =
            SpringBoneWorld::new(asset.spring_colliders.clone(), asset.spring_links.clone());

        Self {
            asset,
            spring_world,
            local_rotations,
            local_translations,
            world_matrices,
            mesh_morph_weights,
            primitive_active_morphs,
            rest_left_arm_dir,
            rest_right_arm_dir,
        }
    }

    /// Evaluates humanoid bone transforms, world matrices, SpringBone hair/cloth physics,
    /// and per-primitive morph target weights for the current display frame.
    pub fn animate(&mut self, dt: f32, pose: &EvaluatedAvatarPose) {
        // 1. Reset local TRS to rest pose
        for (i, node) in self.asset.nodes.iter().enumerate() {
            self.local_rotations[i] = node.rest_rotation;
            self.local_translations[i] = node.rest_translation;
        }

        // 2. Apply character-space hips translation
        if let Some(hips_idx) = self.asset.humanoid.hips {
            if let Some(hips_node) = self.asset.nodes.get(hips_idx) {
                let parent_world = hips_node
                    .parent
                    .and_then(|p| self.asset.nodes.get(p))
                    .map(|p| p.rest_world_matrix)
                    .unwrap_or(self.asset.root_transform);
                let local_offset = parent_world.inverse().transform_vector3(pose.hips_offset);
                if local_offset.is_finite() {
                    self.local_translations[hips_idx] = hips_node.rest_translation + local_offset;
                }
            }
        }

        // 3. Apply character-space bone rotations
        let h = self.asset.humanoid;
        self.apply_char_bone_rot(h.spine, pose.spine_rot);
        self.apply_char_bone_rot(h.chest, pose.chest_rot);
        self.apply_char_bone_rot(h.upper_chest, pose.upper_chest_rot);
        self.apply_char_bone_rot(h.neck, pose.neck_rot);
        self.apply_char_bone_rot(h.head, pose.head_rot);
        self.apply_char_bone_rot(h.left_eye, pose.left_eye_rot);
        self.apply_char_bone_rot(h.right_eye, pose.right_eye_rot);
        self.apply_char_bone_rot(h.left_shoulder, pose.left_shoulder_rot);
        self.apply_char_bone_rot(h.right_shoulder, pose.right_shoulder_rot);

        // Relax T-pose upper arms down into a natural VTuber stance without over-rotating
        // models that were already authored in an A-pose.
        let relax = pose.arm_relax_angle.clamp(0.0, 1.35);
        let desired_y = -relax.sin();
        if self.rest_left_arm_dir.y > desired_y + 0.05 {
            let target_l = Vec3::new(relax.cos(), desired_y, 0.04).normalize();
            let r_char_l = Quat::from_rotation_arc(self.rest_left_arm_dir, target_l);
            self.apply_char_bone_rot(h.left_upper_arm, r_char_l);
        }
        if self.rest_right_arm_dir.y > desired_y + 0.05 {
            let target_r = Vec3::new(-relax.cos(), desired_y, 0.04).normalize();
            let r_char_r = Quat::from_rotation_arc(self.rest_right_arm_dir, target_r);
            self.apply_char_bone_rot(h.right_upper_arm, r_char_r);
        }

        let elbow = pose.elbow_bend_angle.clamp(0.0, 0.6);
        if elbow > 1e-4 {
            let bend_l = Quat::from_euler(glam::EulerRot::XYZ, -0.04, -elbow, 0.05);
            let bend_r = Quat::from_euler(glam::EulerRot::XYZ, -0.04, elbow, -0.05);
            self.apply_char_bone_rot(h.left_lower_arm, bend_l);
            self.apply_char_bone_rot(h.right_lower_arm, bend_r);
        }

        // 4. Forward kinematics in topological order
        for &idx in &self.asset.topological_order {
            let node = &self.asset.nodes[idx];
            let local = Mat4::from_scale_rotation_translation(
                node.rest_scale,
                self.local_rotations[idx],
                self.local_translations[idx],
            );
            let parent_m = node
                .parent
                .map(|p| self.world_matrices[p])
                .unwrap_or(self.asset.root_transform);
            self.world_matrices[idx] = parent_m * local;
        }

        // 5. Step Verlet SpringBone secondary hair/ribbon/clothing physics
        self.spring_world.step(
            dt,
            self.asset.root_transform,
            &mut self.local_rotations,
            &mut self.world_matrices,
        );

        // 6. Evaluate facial expressions & morph target weights
        self.evaluate_expressions(pose);
    }

    fn apply_char_bone_rot(&mut self, bone_opt: Option<usize>, r_char: Quat) {
        let Some(idx) = bone_opt else {
            return;
        };
        let Some(node) = self.asset.nodes.get(idx) else {
            return;
        };
        let w0 = node.rest_world_rotation;
        let local_delta = w0.inverse() * r_char * w0;
        self.local_rotations[idx] = (node.rest_rotation * local_delta).normalize();
    }

    fn evaluate_expressions(&mut self, pose: &EvaluatedAvatarPose) {
        for weights in &mut self.mesh_morph_weights {
            weights.fill(0.0);
        }

        let arkit_binds = &self.asset.arkit_expressions;
        let preset_binds = &self.asset.preset_expressions;

        let has_arkit_blink = !arkit_binds[ArkitBlendshape::EyeBlinkLeft as usize]
            .binds
            .is_empty()
            && !arkit_binds[ArkitBlendshape::EyeBlinkRight as usize]
                .binds
                .is_empty();
        let has_arkit_mouth = !arkit_binds[ArkitBlendshape::JawOpen as usize]
            .binds
            .is_empty();

        // Apply custom ARKit 52 clips if present on the VRM ("Perfect Sync" models)
        if self.asset.has_any_arkit_binds {
            for i in 0..ARKIT_BLENDSHAPE_COUNT {
                let w = pose.arkit[i];
                if w > 0.002 {
                    apply_expression_binding(&arkit_binds[i], w, &mut self.mesh_morph_weights);
                }
            }
        }

        // Check if active emotion presets override blink or mouth
        let mut blink_suppressed = false;
        let mut mouth_suppressed = false;
        let emotion_slots = [
            VrmPresetExpression::Happy,
            VrmPresetExpression::Angry,
            VrmPresetExpression::Sad,
            VrmPresetExpression::Relaxed,
            VrmPresetExpression::Surprised,
            VrmPresetExpression::Neutral,
        ];
        for slot in emotion_slots {
            let idx = slot as usize;
            let w = pose.vrm_presets[idx];
            if w > 0.01 && !preset_binds[idx].binds.is_empty() {
                // Avoid double-driving smile/brow if ARKit mouth/brow already drives this model
                if self.asset.has_any_arkit_binds
                    && matches!(
                        slot,
                        VrmPresetExpression::Relaxed | VrmPresetExpression::Surprised
                    )
                {
                    continue;
                }
                apply_expression_binding(&preset_binds[idx], w, &mut self.mesh_morph_weights);
                if w > 0.25 {
                    blink_suppressed |= preset_binds[idx].override_blink;
                    mouth_suppressed |= preset_binds[idx].override_mouth;
                }
            }
        }

        // Fallback VRM Preset Blinks (mutually exclusive between split L/R vs unified Blink so
        // models defining both `Blink` and `BlinkLeft`/`BlinkRight` never double-deform eyelids)
        if !has_arkit_blink && !blink_suppressed {
            let bl_bind = &preset_binds[VrmPresetExpression::BlinkLeft as usize];
            let br_bind = &preset_binds[VrmPresetExpression::BlinkRight as usize];
            let unified_bind = &preset_binds[VrmPresetExpression::Blink as usize];

            if !bl_bind.binds.is_empty() && !br_bind.binds.is_empty() {
                let wl = pose.vrm_presets[VrmPresetExpression::BlinkLeft as usize];
                let wr = pose.vrm_presets[VrmPresetExpression::BlinkRight as usize];
                if wl > 0.002 {
                    apply_expression_binding(bl_bind, wl, &mut self.mesh_morph_weights);
                }
                if wr > 0.002 {
                    apply_expression_binding(br_bind, wr, &mut self.mesh_morph_weights);
                }
            } else if !unified_bind.binds.is_empty() {
                let w = 0.5
                    * (pose.vrm_presets[VrmPresetExpression::BlinkLeft as usize]
                        + pose.vrm_presets[VrmPresetExpression::BlinkRight as usize]);
                if w > 0.002 {
                    apply_expression_binding(unified_bind, w, &mut self.mesh_morph_weights);
                }
            }
        }

        // Fallback VRM Preset Mouth vowels (`Aa`, `Ih`, `Ou`, `Ee`, `Oh`)
        if !has_arkit_mouth && !mouth_suppressed {
            for slot in [
                VrmPresetExpression::Aa,
                VrmPresetExpression::Ih,
                VrmPresetExpression::Ou,
                VrmPresetExpression::Ee,
                VrmPresetExpression::Oh,
            ] {
                let idx = slot as usize;
                let w = pose.vrm_presets[idx];
                if w > 0.002 {
                    apply_expression_binding(&preset_binds[idx], w, &mut self.mesh_morph_weights);
                }
            }
        }

        // Eye look morph targets (used by VRMs whose eye gaze is morph-driven instead of bone-driven)
        if self.asset.humanoid.left_eye.is_none() && self.asset.humanoid.right_eye.is_none() {
            for slot in [
                VrmPresetExpression::LookUp,
                VrmPresetExpression::LookDown,
                VrmPresetExpression::LookLeft,
                VrmPresetExpression::LookRight,
            ] {
                let idx = slot as usize;
                let w = pose.vrm_presets[idx];
                if w > 0.002 {
                    apply_expression_binding(&preset_binds[idx], w, &mut self.mesh_morph_weights);
                }
            }
        }

        // Clamp final accumulated morph weights and build compact active lists per primitive
        for weights in &mut self.mesh_morph_weights {
            for w in weights.iter_mut() {
                *w = w.clamp(0.0, 1.0);
            }
        }

        for (prim_idx, prim) in self.asset.primitives.iter().enumerate() {
            let active = &mut self.primitive_active_morphs[prim_idx];
            active.clear();
            if prim.morph_target_count == 0 {
                continue;
            }
            if let Some(mesh_weights) = self.mesh_morph_weights.get(prim.mesh_idx) {
                for (t_idx, &w) in mesh_weights
                    .iter()
                    .take(prim.morph_target_count)
                    .enumerate()
                {
                    if w > 0.002 {
                        active.push((t_idx as i32, w));
                    }
                }
                if active.len() > MAX_ACTIVE_MORPHS_PER_DRAW {
                    active
                        .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                    active.truncate(MAX_ACTIVE_MORPHS_PER_DRAW);
                }
            }
        }
    }
}

fn compute_rest_arm_world_dir(
    asset: &LoadedVrmAsset,
    upper_opt: Option<usize>,
    lower_opt: Option<usize>,
    fallback: Vec3,
) -> Vec3 {
    if let (Some(u), Some(l)) = (upper_opt, lower_opt) {
        if let (Some(un), Some(ln)) = (asset.nodes.get(u), asset.nodes.get(l)) {
            let p_u = un.rest_world_matrix.transform_point3(Vec3::ZERO);
            let p_l = ln.rest_world_matrix.transform_point3(Vec3::ZERO);
            let d = p_l - p_u;
            if d.length_squared() > 1e-5 {
                return d.normalize();
            }
        }
    }
    fallback
}

fn apply_expression_binding(
    binding: &ExpressionBinding,
    input_weight: f32,
    mesh_morph_weights: &mut [Vec<f32>],
) {
    if binding.binds.is_empty() {
        return;
    }
    let effective = if binding.is_binary {
        if input_weight >= 0.5 {
            1.0
        } else {
            0.0
        }
    } else {
        input_weight.clamp(0.0, 1.0)
    };
    if effective <= 0.001 {
        return;
    }
    for b in &binding.binds {
        if let Some(mesh_vec) = mesh_morph_weights.get_mut(b.mesh_idx) {
            if let Some(slot) = mesh_vec.get_mut(b.target_idx) {
                *slot += effective * b.weight;
            }
        }
    }
}

struct GpuVrmPrimitive {
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ibo: glow::Buffer,
    index_count: i32,
    morph_tex_array: Option<glow::Texture>,
    morph_tex_width: i32,
}

struct MtoonShaderProgram {
    program: glow::Program,
    u_view_proj: Option<glow::UniformLocation>,
    u_view_mat: Option<glow::UniformLocation>,
    u_node_world: Option<glow::UniformLocation>,
    u_has_skin: Option<glow::UniformLocation>,
    u_joint_matrices: Option<glow::UniformLocation>,
    u_has_morphs: Option<glow::UniformLocation>,
    u_morph_deltas: Option<glow::UniformLocation>,
    u_morph_tex_width: Option<glow::UniformLocation>,
    u_active_morph_count: Option<glow::UniformLocation>,
    u_active_morph_indices: Option<glow::UniformLocation>,
    u_active_morph_weights: Option<glow::UniformLocation>,
    u_outline_mode: Option<glow::UniformLocation>,
    u_outline_width: Option<glow::UniformLocation>,
    u_viewport_size: Option<glow::UniformLocation>,
    u_main_tex: Option<glow::UniformLocation>,
    u_shade_tex: Option<glow::UniformLocation>,
    u_matcap_tex: Option<glow::UniformLocation>,
    u_base_color: Option<glow::UniformLocation>,
    u_shade_color: Option<glow::UniformLocation>,
    u_uv_transform: Option<glow::UniformLocation>,
    u_shading_shift: Option<glow::UniformLocation>,
    u_shading_toony: Option<glow::UniformLocation>,
    u_rim_color: Option<glow::UniformLocation>,
    u_rim_fresnel_power: Option<glow::UniformLocation>,
    u_rim_lift: Option<glow::UniformLocation>,
    u_matcap_factor: Option<glow::UniformLocation>,
    u_has_matcap: Option<glow::UniformLocation>,
    u_alpha_mode: Option<glow::UniformLocation>,
    u_alpha_cutoff: Option<glow::UniformLocation>,
    u_is_outline_pass: Option<glow::UniformLocation>,
    u_outline_color: Option<glow::UniformLocation>,
    u_outline_lighting_mix: Option<glow::UniformLocation>,
    u_light_dir: Option<glow::UniformLocation>,
    u_camera_pos: Option<glow::UniformLocation>,
}

impl MtoonShaderProgram {
    unsafe fn compile(gl: &glow::Context) -> Result<Self, String> {
        let program = compile_gl_program(gl, VS_VRM_SKIN_SRC, FS_MTOON_SRC)?;
        let loc = |name: &str| gl.get_uniform_location(program, name);
        let arr_loc =
            |base: &str, zero: &str| gl.get_uniform_location(program, zero).or_else(|| loc(base));

        Ok(Self {
            program,
            u_view_proj: loc("u_view_proj"),
            u_view_mat: loc("u_view_mat"),
            u_node_world: loc("u_node_world"),
            u_has_skin: loc("u_has_skin"),
            u_joint_matrices: arr_loc("u_joint_matrices", "u_joint_matrices[0]"),
            u_has_morphs: loc("u_has_morphs"),
            u_morph_deltas: loc("u_morph_deltas"),
            u_morph_tex_width: loc("u_morph_tex_width"),
            u_active_morph_count: loc("u_active_morph_count"),
            u_active_morph_indices: arr_loc("u_active_morph_indices", "u_active_morph_indices[0]"),
            u_active_morph_weights: arr_loc("u_active_morph_weights", "u_active_morph_weights[0]"),
            u_outline_mode: loc("u_outline_mode"),
            u_outline_width: loc("u_outline_width"),
            u_viewport_size: loc("u_viewport_size"),
            u_main_tex: loc("u_main_tex"),
            u_shade_tex: loc("u_shade_tex"),
            u_matcap_tex: loc("u_matcap_tex"),
            u_base_color: loc("u_base_color"),
            u_shade_color: loc("u_shade_color"),
            u_uv_transform: loc("u_uv_transform"),
            u_shading_shift: loc("u_shading_shift"),
            u_shading_toony: loc("u_shading_toony"),
            u_rim_color: loc("u_rim_color"),
            u_rim_fresnel_power: loc("u_rim_fresnel_power"),
            u_rim_lift: loc("u_rim_lift"),
            u_matcap_factor: loc("u_matcap_factor"),
            u_has_matcap: loc("u_has_matcap"),
            u_alpha_mode: loc("u_alpha_mode"),
            u_alpha_cutoff: loc("u_alpha_cutoff"),
            u_is_outline_pass: loc("u_is_outline_pass"),
            u_outline_color: loc("u_outline_color"),
            u_outline_lighting_mix: loc("u_outline_lighting_mix"),
            u_light_dir: loc("u_light_dir"),
            u_camera_pos: loc("u_camera_pos"),
        })
    }

    unsafe fn destroy(&self, gl: &glow::Context) {
        gl.delete_program(self.program);
    }
}

/// GPU-uploaded VRM avatar (VAOs, VBOs, IBOs, MToon textures, `RGBA32F` morph texture arrays)
/// paired with its `VrmAvatarCpuAnimator`.
pub struct GpuVrmAvatar {
    pub animator: VrmAvatarCpuAnimator,
    shader: MtoonShaderProgram,
    textures: Vec<glow::Texture>,
    white_tex: glow::Texture,
    black_tex: glow::Texture,
    dummy_morph_array: glow::Texture,
    primitives: Vec<GpuVrmPrimitive>,
    joint_palette_scratch: Vec<f32>,
    morph_indices_scratch: [i32; MAX_ACTIVE_MORPHS_PER_DRAW],
    morph_weights_scratch: [f32; MAX_ACTIVE_MORPHS_PER_DRAW],
}

impl GpuVrmAvatar {
    pub unsafe fn upload(gl: &glow::Context, mut asset: LoadedVrmAsset) -> Result<Self, String> {
        let shader = MtoonShaderProgram::compile(gl)?;

        gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);

        let white_tex = upload_rgba_2d_texture(gl, 1, 1, &[255, 255, 255, 255], false)?;
        let black_tex = upload_rgba_2d_texture(gl, 1, 1, &[0, 0, 0, 255], false)?;
        let dummy_morph_array = upload_morph_array_texture(
            gl,
            &MorphDeltaArrayCpu {
                width: 2,
                height: 1,
                layers: 1,
                data: vec![0.0; 8],
            },
        )?;

        let mut textures = Vec::with_capacity(asset.textures.len());
        // Take decoded RGBA byte buffers out of `asset` as we upload them so we don't duplicate
        // tens of megabytes of uncompressed texture memory in RAM.
        let cpu_textures = std::mem::take(&mut asset.textures);
        for tex in cpu_textures {
            let gl_tex = upload_decoded_texture(gl, &tex)?;
            textures.push(gl_tex);
        }

        let mut primitives = Vec::with_capacity(asset.primitives.len());
        for prim in &mut asset.primitives {
            let vao = gl
                .create_vertex_array()
                .map_err(|e| format!("Failed to create VRM VAO: {e}"))?;
            let vbo = gl
                .create_buffer()
                .map_err(|e| format!("Failed to create VRM VBO: {e}"))?;
            let ibo = gl
                .create_buffer()
                .map_err(|e| format!("Failed to create VRM IBO: {e}"))?;

            gl.bind_vertex_array(Some(vao));

            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&prim.vertices),
                glow::STATIC_DRAW,
            );

            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytemuck::cast_slice(&prim.indices),
                glow::STATIC_DRAW,
            );

            let stride = std::mem::size_of::<VrmVertex>() as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 12);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, stride, 24);
            gl.enable_vertex_attrib_array(3);
            gl.vertex_attrib_pointer_i32(3, 4, glow::UNSIGNED_SHORT, stride, 32);
            gl.enable_vertex_attrib_array(4);
            gl.vertex_attrib_pointer_f32(4, 4, glow::FLOAT, false, stride, 40);

            gl.bind_vertex_array(None);

            let index_count = prim.indices.len() as i32;
            // Free CPU vertex/index arrays once uploaded to VRAM
            prim.vertices = Vec::new();
            prim.indices = Vec::new();

            let (morph_tex_array, morph_tex_width) = if let Some(morph_cpu) = prim.morphs.take() {
                let w = morph_cpu.width as i32;
                let tex = upload_morph_array_texture(gl, &morph_cpu)?;
                (Some(tex), w)
            } else {
                (None, 0)
            };

            primitives.push(GpuVrmPrimitive {
                vao,
                vbo,
                ibo,
                index_count,
                morph_tex_array,
                morph_tex_width,
            });
        }

        let animator = VrmAvatarCpuAnimator::new(asset);

        Ok(Self {
            animator,
            shader,
            textures,
            white_tex,
            black_tex,
            dummy_morph_array,
            primitives,
            joint_palette_scratch: Vec::with_capacity(MAX_JOINTS_PER_PRIMITIVE * 16),
            morph_indices_scratch: [0; MAX_ACTIVE_MORPHS_PER_DRAW],
            morph_weights_scratch: [0.0; MAX_ACTIVE_MORPHS_PER_DRAW],
        })
    }

    pub unsafe fn animate_and_draw(
        &mut self,
        gl: &glow::Context,
        dt: f32,
        pose: &EvaluatedAvatarPose,
        viewport_w: u32,
        viewport_h: u32,
        camera_zoom: f32,
        camera_offset_y: f32,
    ) {
        self.animator.animate(dt, pose);

        let aspect = viewport_w.max(1) as f32 / viewport_h.max(1) as f32;
        let (view, proj, cam_pos) = compute_bust_camera_matrices(
            &self.animator.asset.framing,
            aspect,
            camera_zoom,
            camera_offset_y,
        );
        let view_proj = proj * view;
        // Soft studio key light from slightly above-front-right
        let light_dir = Vec3::new(0.28, 0.44, 0.85).normalize();

        let s = &self.shader;
        gl.use_program(Some(s.program));

        gl.uniform_matrix_4_f32_slice(s.u_view_proj.as_ref(), false, &view_proj.to_cols_array());
        gl.uniform_matrix_4_f32_slice(s.u_view_mat.as_ref(), false, &view.to_cols_array());
        gl.uniform_2_f32(
            s.u_viewport_size.as_ref(),
            viewport_w as f32,
            viewport_h as f32,
        );
        gl.uniform_3_f32(
            s.u_light_dir.as_ref(),
            light_dir.x,
            light_dir.y,
            light_dir.z,
        );
        gl.uniform_3_f32(s.u_camera_pos.as_ref(), cam_pos.x, cam_pos.y, cam_pos.z);

        gl.uniform_1_i32(s.u_main_tex.as_ref(), 0);
        gl.uniform_1_i32(s.u_shade_tex.as_ref(), 1);
        gl.uniform_1_i32(s.u_matcap_tex.as_ref(), 2);
        gl.uniform_1_i32(s.u_morph_deltas.as_ref(), 3);

        gl.enable(glow::DEPTH_TEST);
        gl.depth_func(glow::LEQUAL);
        gl.front_face(glow::CCW);

        let draw_count = self.animator.asset.draw_order.len();
        for order_i in 0..draw_count {
            let prim_idx = self.animator.asset.draw_order[order_i];
            let gpu_prim = &self.primitives[prim_idx];
            if gpu_prim.index_count <= 0 {
                continue;
            }
            let cpu_prim = &self.animator.asset.primitives[prim_idx];
            let mat = &self.animator.asset.materials[cpu_prim.material_idx];

            gl.bind_vertex_array(Some(gpu_prim.vao));

            // Upload node world matrix & skinning palette
            let node_world = self
                .animator
                .world_matrices
                .get(cpu_prim.node_idx)
                .copied()
                .unwrap_or(Mat4::IDENTITY);
            gl.uniform_matrix_4_f32_slice(
                s.u_node_world.as_ref(),
                false,
                &node_world.to_cols_array(),
            );

            if !cpu_prim.joint_nodes.is_empty() {
                gl.uniform_1_i32(s.u_has_skin.as_ref(), 1);
                self.joint_palette_scratch.clear();
                for &(j_node, ibm) in &cpu_prim.joint_nodes {
                    let jm = self
                        .animator
                        .world_matrices
                        .get(j_node)
                        .copied()
                        .unwrap_or(Mat4::IDENTITY)
                        * ibm;
                    self.joint_palette_scratch
                        .extend_from_slice(&jm.to_cols_array());
                }
                gl.uniform_matrix_4_f32_slice(
                    s.u_joint_matrices.as_ref(),
                    false,
                    &self.joint_palette_scratch,
                );
            } else {
                gl.uniform_1_i32(s.u_has_skin.as_ref(), 0);
            }

            // Upload morph target weights & bind morph texture array on unit 3
            gl.active_texture(glow::TEXTURE3);
            if let Some(morph_tex) = gpu_prim.morph_tex_array {
                let active = &self.animator.primitive_active_morphs[prim_idx];
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(morph_tex));
                if !active.is_empty() {
                    let count = active.len().min(MAX_ACTIVE_MORPHS_PER_DRAW);
                    for (k, &(idx, w)) in active.iter().take(count).enumerate() {
                        self.morph_indices_scratch[k] = idx;
                        self.morph_weights_scratch[k] = w;
                    }
                    gl.uniform_1_i32(s.u_has_morphs.as_ref(), 1);
                    gl.uniform_1_i32(s.u_morph_tex_width.as_ref(), gpu_prim.morph_tex_width);
                    gl.uniform_1_i32(s.u_active_morph_count.as_ref(), count as i32);
                    gl.uniform_1_i32_slice(
                        s.u_active_morph_indices.as_ref(),
                        &self.morph_indices_scratch[..count],
                    );
                    gl.uniform_1_f32_slice(
                        s.u_active_morph_weights.as_ref(),
                        &self.morph_weights_scratch[..count],
                    );
                } else {
                    gl.uniform_1_i32(s.u_has_morphs.as_ref(), 0);
                    gl.uniform_1_i32(s.u_active_morph_count.as_ref(), 0);
                }
            } else {
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.dummy_morph_array));
                gl.uniform_1_i32(s.u_has_morphs.as_ref(), 0);
                gl.uniform_1_i32(s.u_active_morph_count.as_ref(), 0);
            }

            // Bind MToon material textures
            let main_gl_tex = mat
                .main_tex
                .and_then(|i| self.textures.get(i).copied())
                .unwrap_or(self.white_tex);
            let shade_gl_tex = mat
                .shade_tex
                .or(mat.main_tex)
                .and_then(|i| self.textures.get(i).copied())
                .unwrap_or(main_gl_tex);
            let matcap_gl_tex = mat
                .matcap_tex
                .and_then(|i| self.textures.get(i).copied())
                .unwrap_or(self.black_tex);

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(main_gl_tex));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(shade_gl_tex));
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, Some(matcap_gl_tex));

            gl.uniform_4_f32_slice(s.u_base_color.as_ref(), &mat.base_color);
            gl.uniform_3_f32_slice(s.u_shade_color.as_ref(), &mat.shade_color);
            gl.uniform_4_f32_slice(s.u_uv_transform.as_ref(), &mat.uv_transform);
            gl.uniform_1_f32(s.u_shading_shift.as_ref(), mat.shading_shift);
            gl.uniform_1_f32(s.u_shading_toony.as_ref(), mat.shading_toony);
            gl.uniform_3_f32_slice(s.u_rim_color.as_ref(), &mat.rim_color);
            gl.uniform_1_f32(s.u_rim_fresnel_power.as_ref(), mat.rim_fresnel_power);
            gl.uniform_1_f32(s.u_rim_lift.as_ref(), mat.rim_lift);
            gl.uniform_3_f32_slice(s.u_matcap_factor.as_ref(), &mat.matcap_factor);
            gl.uniform_1_i32(s.u_has_matcap.as_ref(), i32::from(mat.matcap_tex.is_some()));
            gl.uniform_1_i32(s.u_alpha_mode.as_ref(), mat.alpha_mode as i32);
            gl.uniform_1_f32(s.u_alpha_cutoff.as_ref(), mat.alpha_cutoff);

            // Render state per alpha mode
            if mat.alpha_mode == AlphaModeKind::Blend {
                gl.enable(glow::BLEND);
                gl.blend_func_separate(
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                );
                gl.depth_mask(mat.z_write);
            } else {
                gl.disable(glow::BLEND);
                gl.depth_mask(true);
            }

            if mat.double_sided {
                gl.disable(glow::CULL_FACE);
            } else {
                gl.enable(glow::CULL_FACE);
                gl.cull_face(glow::BACK);
            }

            // Pass 1: Primary MToon cel-shaded surface
            gl.uniform_1_i32(s.u_outline_mode.as_ref(), 0);
            gl.uniform_1_f32(s.u_outline_width.as_ref(), 0.0);
            gl.uniform_1_i32(s.u_is_outline_pass.as_ref(), 0);
            gl.draw_elements(glow::TRIANGLES, gpu_prim.index_count, glow::UNSIGNED_INT, 0);

            // Pass 2: Inverted-hull anime outline (front-face culled)
            if mat.outline_mode != OutlineModeKind::None
                && mat.outline_width > 1e-5
                && mat.outline_color[3] > 0.01
            {
                gl.enable(glow::CULL_FACE);
                gl.cull_face(glow::FRONT);
                gl.uniform_1_i32(s.u_outline_mode.as_ref(), mat.outline_mode as i32);
                gl.uniform_1_f32(s.u_outline_width.as_ref(), mat.outline_width);
                gl.uniform_1_i32(s.u_is_outline_pass.as_ref(), 1);
                gl.uniform_4_f32_slice(s.u_outline_color.as_ref(), &mat.outline_color);
                gl.uniform_1_f32(s.u_outline_lighting_mix.as_ref(), mat.outline_lighting_mix);
                gl.draw_elements(glow::TRIANGLES, gpu_prim.index_count, glow::UNSIGNED_INT, 0);
            }
        }

        gl.bind_vertex_array(None);
    }

    pub unsafe fn destroy(&mut self, gl: &glow::Context) {
        for p in self.primitives.drain(..) {
            gl.delete_vertex_array(p.vao);
            gl.delete_buffer(p.vbo);
            gl.delete_buffer(p.ibo);
            if let Some(tex) = p.morph_tex_array {
                gl.delete_texture(tex);
            }
        }
        for tex in self.textures.drain(..) {
            gl.delete_texture(tex);
        }
        gl.delete_texture(self.white_tex);
        gl.delete_texture(self.black_tex);
        gl.delete_texture(self.dummy_morph_array);
        self.shader.destroy(gl);
    }
}

pub(crate) unsafe fn compile_gl_program(
    gl: &glow::Context,
    vs_src: &str,
    fs_src: &str,
) -> Result<glow::Program, String> {
    let vs = gl
        .create_shader(glow::VERTEX_SHADER)
        .map_err(|e| format!("Failed to create vertex shader: {e}"))?;
    gl.shader_source(vs, vs_src);
    gl.compile_shader(vs);
    if !gl.get_shader_compile_status(vs) {
        let log = gl.get_shader_info_log(vs);
        gl.delete_shader(vs);
        return Err(format!("Vertex shader compile error: {log}"));
    }

    let fs = gl
        .create_shader(glow::FRAGMENT_SHADER)
        .map_err(|e| format!("Failed to create fragment shader: {e}"))?;
    gl.shader_source(fs, fs_src);
    gl.compile_shader(fs);
    if !gl.get_shader_compile_status(fs) {
        let log = gl.get_shader_info_log(fs);
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        return Err(format!("Fragment shader compile error: {log}"));
    }

    let prog = gl
        .create_program()
        .map_err(|e| format!("Failed to create shader program: {e}"))?;
    gl.attach_shader(prog, vs);
    gl.attach_shader(prog, fs);
    gl.link_program(prog);
    gl.detach_shader(prog, vs);
    gl.detach_shader(prog, fs);
    gl.delete_shader(vs);
    gl.delete_shader(fs);

    if !gl.get_program_link_status(prog) {
        let log = gl.get_program_info_log(prog);
        gl.delete_program(prog);
        return Err(format!("Shader program link error: {log}"));
    }
    Ok(prog)
}

pub(crate) unsafe fn upload_decoded_texture(
    gl: &glow::Context,
    tex: &DecodedRgbaTexture,
) -> Result<glow::Texture, String> {
    upload_rgba_2d_texture(gl, tex.width, tex.height, &tex.rgba, true)
}

pub(crate) unsafe fn upload_rgba_2d_texture(
    gl: &glow::Context,
    width: u32,
    height: u32,
    rgba: &[u8],
    mipmaps: bool,
) -> Result<glow::Texture, String> {
    let gl_tex = gl
        .create_texture()
        .map_err(|e| format!("Failed to create 2D texture: {e}"))?;
    gl.bind_texture(glow::TEXTURE_2D, Some(gl_tex));
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA8 as i32,
        width as i32,
        height as i32,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        Some(rgba),
    );
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
    if mipmaps && width > 1 && height > 1 {
        gl.generate_mipmap(glow::TEXTURE_2D);
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR_MIPMAP_LINEAR as i32,
        );
    } else {
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
    }
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_MAG_FILTER,
        glow::LINEAR as i32,
    );
    gl.bind_texture(glow::TEXTURE_2D, None);
    Ok(gl_tex)
}

unsafe fn upload_morph_array_texture(
    gl: &glow::Context,
    morph: &MorphDeltaArrayCpu,
) -> Result<glow::Texture, String> {
    let tex = gl
        .create_texture()
        .map_err(|e| format!("Failed to create morph 2D array texture: {e}"))?;
    gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(tex));
    gl.tex_parameter_i32(
        glow::TEXTURE_2D_ARRAY,
        glow::TEXTURE_MIN_FILTER,
        glow::NEAREST as i32,
    );
    gl.tex_parameter_i32(
        glow::TEXTURE_2D_ARRAY,
        glow::TEXTURE_MAG_FILTER,
        glow::NEAREST as i32,
    );
    gl.tex_parameter_i32(
        glow::TEXTURE_2D_ARRAY,
        glow::TEXTURE_WRAP_S,
        glow::CLAMP_TO_EDGE as i32,
    );
    gl.tex_parameter_i32(
        glow::TEXTURE_2D_ARRAY,
        glow::TEXTURE_WRAP_T,
        glow::CLAMP_TO_EDGE as i32,
    );
    gl.tex_image_3d(
        glow::TEXTURE_2D_ARRAY,
        0,
        glow::RGBA32F as i32,
        morph.width as i32,
        morph.height as i32,
        morph.layers as i32,
        0,
        glow::RGBA,
        glow::FLOAT,
        Some(bytemuck::cast_slice(&morph.data)),
    );
    gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
    Ok(tex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vtuber::vrm::loader::load_vrm_from_bytes;
    use crate::vtuber::vrm::loader::tests::build_minimal_test_vrm1_glb;

    #[test]
    fn test_cpu_animator_updates_head_world_matrix_and_aa_morph() {
        let glb = build_minimal_test_vrm1_glb();
        let asset = load_vrm_from_bytes(&glb).expect("load synthetic vrm");
        let mut animator = VrmAvatarCpuAnimator::new(asset);

        let mut pose = EvaluatedAvatarPose::default();
        pose.head_rot = Quat::from_rotation_y(0.35);
        pose.vrm_presets[VrmPresetExpression::Aa as usize] = 0.80;

        animator.animate(1.0 / 60.0, &pose);

        assert_eq!(animator.primitive_active_morphs[0].len(), 1);
        assert_eq!(animator.primitive_active_morphs[0][0].0, 0);
        assert!((animator.primitive_active_morphs[0][0].1 - 0.80).abs() < 1e-4);

        let head_world = animator.world_matrices[1];
        let rest_head_world = animator.asset.nodes[1].rest_world_matrix;
        assert_ne!(head_world, rest_head_world);
    }
}
