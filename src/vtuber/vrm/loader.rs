use super::springbone::{SpringColliderDef, SpringColliderShape, SpringJointLink};
use crate::vtuber::rig::{
    ArkitBlendshape, VrmPresetExpression, ARKIT_BLENDSHAPE_COUNT, VRM_PRESET_COUNT,
};
use anyhow::{anyhow, Context, Result};
use glam::{Mat4, Quat, Vec3, Vec4};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

pub const MAX_JOINTS_PER_PRIMITIVE: usize = 128;
pub const MORPH_TEX_WIDTH: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaModeKind {
    Opaque = 0,
    Mask = 1,
    Blend = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineModeKind {
    None = 0,
    World = 1,
    Screen = 2,
}

#[derive(Clone, Debug)]
pub struct MtoonMaterialDef {
    pub base_color: [f32; 4],
    pub main_tex: Option<usize>,
    pub uv_transform: [f32; 4], // offset_x, offset_y, scale_x, scale_y
    pub shade_color: [f32; 3],
    pub shade_tex: Option<usize>,
    pub shading_shift: f32,
    pub shading_toony: f32,
    pub matcap_tex: Option<usize>,
    pub matcap_factor: [f32; 3],
    pub rim_color: [f32; 3],
    pub rim_fresnel_power: f32,
    pub rim_lift: f32,
    pub alpha_mode: AlphaModeKind,
    pub alpha_cutoff: f32,
    pub z_write: bool,
    pub double_sided: bool,
    pub render_queue: i32,
    pub outline_mode: OutlineModeKind,
    pub outline_width: f32,
    pub outline_color: [f32; 4],
    pub outline_lighting_mix: f32,
}

impl Default for MtoonMaterialDef {
    fn default() -> Self {
        Self {
            base_color: [1.0, 1.0, 1.0, 1.0],
            main_tex: None,
            uv_transform: [0.0, 0.0, 1.0, 1.0],
            shade_color: [0.82, 0.78, 0.84],
            shade_tex: None,
            shading_shift: -0.05,
            shading_toony: 0.90,
            matcap_tex: None,
            matcap_factor: [0.0, 0.0, 0.0],
            rim_color: [0.0, 0.0, 0.0],
            rim_fresnel_power: 5.0,
            rim_lift: 0.0,
            alpha_mode: AlphaModeKind::Opaque,
            alpha_cutoff: 0.5,
            z_write: true,
            double_sided: false,
            render_queue: 2000,
            outline_mode: OutlineModeKind::None,
            outline_width: 0.0,
            outline_color: [0.12, 0.10, 0.14, 1.0],
            outline_lighting_mix: 0.5,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DecodedRgbaTexture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VrmVertex {
    pub pos: [f32; 3],
    pub norm: [f32; 3],
    pub uv: [f32; 2],
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

#[derive(Clone, Debug)]
pub struct MorphDeltaArrayCpu {
    pub width: u32,
    pub height: u32,
    pub layers: u32,
    /// Tightly packed `RGBA32F` (`width * height * layers * 4` floats).
    pub data: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct VrmPrimitiveCpu {
    pub mesh_idx: usize,
    pub node_idx: usize,
    pub material_idx: usize,
    pub vertices: Vec<VrmVertex>,
    pub indices: Vec<u32>,
    /// Remapped scene node indices + inverse bind matrices (`<= 128`) for this primitive.
    pub joint_nodes: Vec<(usize, Mat4)>,
    pub morphs: Option<MorphDeltaArrayCpu>,
    pub morph_target_count: usize,
}

#[derive(Clone, Debug)]
pub struct VrmNodeDef {
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub rest_translation: Vec3,
    pub rest_rotation: Quat,
    pub rest_scale: Vec3,
    pub rest_world_matrix: Mat4,
    pub rest_world_rotation: Quat,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HumanoidBoneTable {
    pub hips: Option<usize>,
    pub spine: Option<usize>,
    pub chest: Option<usize>,
    pub upper_chest: Option<usize>,
    pub neck: Option<usize>,
    pub head: Option<usize>,
    pub left_eye: Option<usize>,
    pub right_eye: Option<usize>,
    pub left_shoulder: Option<usize>,
    pub right_shoulder: Option<usize>,
    pub left_upper_arm: Option<usize>,
    pub right_upper_arm: Option<usize>,
    pub left_lower_arm: Option<usize>,
    pub right_lower_arm: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
pub struct MorphBindDef {
    pub mesh_idx: usize,
    pub target_idx: usize,
    pub weight: f32,
}

#[derive(Clone, Debug, Default)]
pub struct ExpressionBinding {
    pub binds: Vec<MorphBindDef>,
    pub is_binary: bool,
    pub override_blink: bool,
    pub override_mouth: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct AvatarFramingMetrics {
    pub head_center: Vec3,
    pub crown_y: f32,
    pub bust_bottom_y: f32,
}

pub struct LoadedVrmAsset {
    pub title: String,
    pub is_vrm0: bool,
    pub root_transform: Mat4,
    pub nodes: Vec<VrmNodeDef>,
    /// Topological order (`parent` always appears before `child`).
    pub topological_order: Vec<usize>,
    pub humanoid: HumanoidBoneTable,
    pub preset_expressions: [ExpressionBinding; VRM_PRESET_COUNT],
    pub arkit_expressions: [ExpressionBinding; ARKIT_BLENDSHAPE_COUNT],
    pub has_any_arkit_binds: bool,
    pub textures: Vec<DecodedRgbaTexture>,
    pub materials: Vec<MtoonMaterialDef>,
    pub primitives: Vec<VrmPrimitiveCpu>,
    pub draw_order: Vec<usize>,
    pub spring_colliders: Vec<SpringColliderDef>,
    pub spring_links: Vec<SpringJointLink>,
    pub framing: AvatarFramingMetrics,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Vec3OrXyz {
    Arr([f32; 3]),
    Obj {
        #[serde(default)]
        x: f32,
        #[serde(default)]
        y: f32,
        #[serde(default)]
        z: f32,
    },
}

impl Vec3OrXyz {
    fn to_vec3(&self) -> Vec3 {
        match *self {
            Self::Arr(a) => Vec3::from_array(a),
            Self::Obj { x, y, z } => Vec3::new(x, y, z),
        }
    }
}

impl Default for Vec3OrXyz {
    fn default() -> Self {
        Self::Arr([0.0, -1.0, 0.0])
    }
}

pub fn load_vrm_from_path(path: &Path) -> Result<LoadedVrmAsset> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Failed to read VRM file {}", path.display()))?;
    load_vrm_from_bytes(&bytes)
}

pub fn load_vrm_from_bytes(bytes: &[u8]) -> Result<LoadedVrmAsset> {
    let (document, buffers, images) =
        gltf::import_slice(bytes).context("Failed to parse VRM / glTF 2.0 binary container")?;

    let is_vrm1 = document.extension_value("VRMC_vrm").is_some();
    let is_vrm0 = !is_vrm1 && document.extension_value("VRM").is_some();
    let root_transform = if is_vrm0 {
        Mat4::from_rotation_y(std::f32::consts::PI)
    } else {
        Mat4::IDENTITY
    };

    // 1. Scene node hierarchy & topological traversal
    let node_count = document.nodes().len();
    if node_count == 0 {
        return Err(anyhow!("VRM file contains zero scene nodes"));
    }

    let mut parents = vec![None; node_count];
    let mut children_vec = vec![Vec::new(); node_count];
    let mut local_trs = Vec::with_capacity(node_count);

    for node in document.nodes() {
        let idx = node.index();
        let (t, r, s) = node.transform().decomposed();
        local_trs.push((
            Vec3::from_array(t),
            Quat::from_array(r).normalize(),
            Vec3::from_array(s),
        ));
        for child in node.children() {
            parents[child.index()] = Some(idx);
            children_vec[idx].push(child.index());
        }
    }

    let mut topological_order = Vec::with_capacity(node_count);
    let mut visited = vec![false; node_count];
    fn visit_dfs(u: usize, children: &[Vec<usize>], visited: &mut [bool], order: &mut Vec<usize>) {
        if visited[u] {
            return;
        }
        visited[u] = true;
        order.push(u);
        for &c in &children[u] {
            visit_dfs(c, children, visited, order);
        }
    }
    for i in 0..node_count {
        if parents[i].is_none() {
            visit_dfs(i, &children_vec, &mut visited, &mut topological_order);
        }
    }
    for i in 0..node_count {
        if !visited[i] {
            visit_dfs(i, &children_vec, &mut visited, &mut topological_order);
        }
    }

    let mut rest_world = vec![Mat4::IDENTITY; node_count];
    for &idx in &topological_order {
        let (t, r, s) = local_trs[idx];
        let local = Mat4::from_scale_rotation_translation(s, r, t);
        let parent_m = parents[idx]
            .map(|p| rest_world[p])
            .unwrap_or(root_transform);
        rest_world[idx] = parent_m * local;
    }

    let mut nodes = Vec::with_capacity(node_count);
    for idx in 0..node_count {
        let (t, r, s) = local_trs[idx];
        let (_, wr, _) = rest_world[idx].to_scale_rotation_translation();
        nodes.push(VrmNodeDef {
            parent: parents[idx],
            children: children_vec[idx].clone(),
            rest_translation: t,
            rest_rotation: r,
            rest_scale: s,
            rest_world_matrix: rest_world[idx],
            rest_world_rotation: wr.normalize(),
        });
    }

    // Map glTF node -> mesh index (needed for VRM 1.0 morphTargetBinds & mesh instantiation)
    let mut node_to_mesh = vec![None; node_count];
    let mut mesh_to_primary_node = vec![None; document.meshes().len()];
    for node in document.nodes() {
        if let Some(mesh) = node.mesh() {
            node_to_mesh[node.index()] = Some(mesh.index());
            if mesh_to_primary_node[mesh.index()].is_none() {
                mesh_to_primary_node[mesh.index()] = Some(node.index());
            }
        }
    }

    // 2. Decode textures to RGBA8
    let gltf_images: Vec<DecodedRgbaTexture> = images.into_iter().map(convert_gltf_image).collect();
    let mut textures = Vec::with_capacity(document.textures().len());
    for tex in document.textures() {
        let img_idx = tex.source().index();
        if let Some(decoded) = gltf_images.get(img_idx) {
            textures.push(decoded.clone());
        } else {
            textures.push(DecodedRgbaTexture {
                width: 1,
                height: 1,
                rgba: vec![255, 255, 255, 255],
            });
        }
    }

    // 3. Parse materials (MToon 1.0, MToon 0.x, or glTF PBR fallback)
    let mut materials = parse_gltf_materials(&document);

    // 4. Parse VRM 1.0 or VRM 0.x metadata, humanoid bones, expressions, and SpringBones
    let mut title = "VRM Avatar".to_string();
    let mut humanoid = HumanoidBoneTable::default();
    let mut preset_expressions: [ExpressionBinding; VRM_PRESET_COUNT] =
        std::array::from_fn(|_| ExpressionBinding::default());
    let mut arkit_expressions: [ExpressionBinding; ARKIT_BLENDSHAPE_COUNT] =
        std::array::from_fn(|_| ExpressionBinding::default());
    let mut spring_colliders = Vec::new();
    let mut spring_links = Vec::new();

    if let Some(vrm1_val) = document.extension_value("VRMC_vrm") {
        parse_vrm1_extension(
            vrm1_val,
            &node_to_mesh,
            &mut title,
            &mut humanoid,
            &mut preset_expressions,
            &mut arkit_expressions,
        );
        if let Some(sb_val) = document.extension_value("VRMC_springBone") {
            parse_vrm1_springbones(
                sb_val,
                &nodes,
                &topological_order,
                &mut spring_colliders,
                &mut spring_links,
            );
        }
    } else if let Some(vrm0_val) = document.extension_value("VRM") {
        parse_vrm0_extension(
            vrm0_val,
            &nodes,
            &topological_order,
            &mut title,
            &mut humanoid,
            &mut preset_expressions,
            &mut arkit_expressions,
            &mut materials,
            &mut spring_colliders,
            &mut spring_links,
        );
    }

    let has_any_arkit_binds = arkit_expressions.iter().any(|e| !e.binds.is_empty());

    // 5. Parse skins & mesh primitives (including packed morph target delta arrays)
    let skins = parse_skins(&document, &buffers);
    let mut primitives = Vec::new();
    let mut max_vertex_world_y = f32::NEG_INFINITY;

    for node in document.nodes() {
        let Some(mesh) = node.mesh() else {
            continue;
        };
        let node_idx = node.index();
        let mesh_idx = mesh.index();
        let skin_opt = node.skin().and_then(|s| skins.get(s.index()));
        let node_world = nodes[node_idx].rest_world_matrix;

        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            if let Some(built) = build_primitive_cpu(
                &prim,
                mesh_idx,
                node_idx,
                skin_opt,
                &nodes,
                node_world,
                materials.len(),
                &buffers,
                &mut max_vertex_world_y,
            ) {
                primitives.push(built);
            }
        }
    }

    if primitives.is_empty() {
        return Err(anyhow!("VRM file contains no renderable triangle meshes"));
    }

    // Ensure default material exists if any primitive had `material: None`
    if materials.is_empty() {
        materials.push(MtoonMaterialDef::default());
    }

    // Sort draw order: Opaque -> Mask -> Blend (ascending by `render_queue`)
    let mut draw_order: Vec<usize> = (0..primitives.len()).collect();
    draw_order.sort_by_key(|&prim_i| {
        let mat = &materials[primitives[prim_i].material_idx];
        let bucket = match mat.alpha_mode {
            AlphaModeKind::Opaque => 0,
            AlphaModeKind::Mask => 1,
            AlphaModeKind::Blend => 2,
        };
        (bucket, mat.render_queue, prim_i)
    });

    // 6. Compute automatic bust-up camera framing from humanoid head/neck/chest rest positions
    let framing = compute_avatar_framing(&nodes, &humanoid, max_vertex_world_y);

    Ok(LoadedVrmAsset {
        title,
        is_vrm0,
        root_transform,
        nodes,
        topological_order,
        humanoid,
        preset_expressions,
        arkit_expressions,
        has_any_arkit_binds,
        textures,
        materials,
        primitives,
        draw_order,
        spring_colliders,
        spring_links,
        framing,
    })
}

fn convert_gltf_image(img: gltf::image::Data) -> DecodedRgbaTexture {
    use gltf::image::Format;
    let w = img.width.max(1);
    let h = img.height.max(1);
    let px_count = (w * h) as usize;
    let mut rgba = Vec::with_capacity(px_count * 4);

    match img.format {
        Format::R8G8B8A8 => {
            rgba.extend_from_slice(&img.pixels);
        }
        Format::R8G8B8 => {
            for c in img.pixels.chunks_exact(3) {
                rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        Format::R8G8 => {
            for c in img.pixels.chunks_exact(2) {
                rgba.extend_from_slice(&[c[0], c[0], c[0], c[1]]);
            }
        }
        Format::R8 => {
            for &v in &img.pixels {
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Format::R16G16B16A16 => {
            for c in img.pixels.chunks_exact(8) {
                rgba.push(c[1]);
                rgba.push(c[3]);
                rgba.push(c[5]);
                rgba.push(c[7]);
            }
        }
        Format::R16G16B16 => {
            for c in img.pixels.chunks_exact(6) {
                rgba.extend_from_slice(&[c[1], c[3], c[5], 255]);
            }
        }
        _ => {}
    }

    if rgba.len() != px_count * 4 {
        rgba.resize(px_count * 4, 255);
    }

    DecodedRgbaTexture {
        width: w,
        height: h,
        rgba,
    }
}

fn parse_gltf_materials(document: &gltf::Document) -> Vec<MtoonMaterialDef> {
    let mut out = Vec::with_capacity(document.materials().len().max(1));

    for mat in document.materials() {
        let mut def = MtoonMaterialDef::default();
        let pbr = mat.pbr_metallic_roughness();
        def.base_color = pbr.base_color_factor();
        def.main_tex = pbr.base_color_texture().map(|t| t.texture().index());
        def.shade_color = [
            def.base_color[0] * 0.80,
            def.base_color[1] * 0.76,
            def.base_color[2] * 0.82,
        ];
        def.shade_tex = def.main_tex;
        def.double_sided = mat.double_sided();
        def.alpha_cutoff = mat.alpha_cutoff().unwrap_or(0.5);
        match mat.alpha_mode() {
            gltf::material::AlphaMode::Opaque => {
                def.alpha_mode = AlphaModeKind::Opaque;
                def.z_write = true;
                def.render_queue = 2000;
            }
            gltf::material::AlphaMode::Mask => {
                def.alpha_mode = AlphaModeKind::Mask;
                def.z_write = true;
                def.render_queue = 2450;
            }
            gltf::material::AlphaMode::Blend => {
                def.alpha_mode = AlphaModeKind::Blend;
                def.z_write = false;
                def.render_queue = 3000;
            }
        }

        if let Some(mtoon) = mat.extension_value("VRMC_materials_mtoon") {
            if let Some(sc) = mtoon
                .get("shadeColorFactor")
                .and_then(|v| serde_json::from_value::<[f32; 3]>(v.clone()).ok())
            {
                def.shade_color = sc;
            }
            if let Some(idx) = mtoon
                .get("shadeMultiplyTexture")
                .and_then(|v| v.get("index"))
                .and_then(|v| v.as_u64())
            {
                def.shade_tex = Some(idx as usize);
            }
            if let Some(shift) = mtoon.get("shadingShiftFactor").and_then(|v| v.as_f64()) {
                def.shading_shift = shift as f32;
            }
            if let Some(toony) = mtoon.get("shadingToonyFactor").and_then(|v| v.as_f64()) {
                def.shading_toony = toony as f32;
            }
            if let Some(mc_idx) = mtoon
                .get("matcapTexture")
                .and_then(|v| v.get("index"))
                .and_then(|v| v.as_u64())
            {
                def.matcap_tex = Some(mc_idx as usize);
                def.matcap_factor = [1.0, 1.0, 1.0];
            }
            if let Some(mcf) = mtoon
                .get("matcapFactor")
                .and_then(|v| serde_json::from_value::<[f32; 3]>(v.clone()).ok())
            {
                def.matcap_factor = mcf;
            }
            if let Some(rc) = mtoon
                .get("parametricRimColorFactor")
                .and_then(|v| serde_json::from_value::<[f32; 3]>(v.clone()).ok())
            {
                def.rim_color = rc;
            }
            if let Some(rp) = mtoon
                .get("parametricRimFresnelPowerFactor")
                .and_then(|v| v.as_f64())
            {
                def.rim_fresnel_power = rp as f32;
            }
            if let Some(rl) = mtoon
                .get("parametricRimLiftFactor")
                .and_then(|v| v.as_f64())
            {
                def.rim_lift = rl as f32;
            }
            if let Some(zw) = mtoon.get("transparentWithZWrite").and_then(|v| v.as_bool()) {
                if def.alpha_mode == AlphaModeKind::Blend {
                    def.z_write = zw;
                    if zw {
                        def.render_queue = 2550;
                    }
                }
            }
            if let Some(rq_off) = mtoon
                .get("renderQueueOffsetNumber")
                .and_then(|v| v.as_i64())
            {
                def.render_queue += rq_off as i32;
            }
            match mtoon.get("outlineWidthMode").and_then(|v| v.as_str()) {
                Some("worldCoordinates") => def.outline_mode = OutlineModeKind::World,
                Some("screenCoordinates") => def.outline_mode = OutlineModeKind::Screen,
                _ => def.outline_mode = OutlineModeKind::None,
            }
            if let Some(ow) = mtoon.get("outlineWidthFactor").and_then(|v| v.as_f64()) {
                def.outline_width = ow as f32;
            }
            if let Some(oc) = mtoon
                .get("outlineColorFactor")
                .and_then(|v| serde_json::from_value::<[f32; 3]>(v.clone()).ok())
            {
                def.outline_color = [oc[0], oc[1], oc[2], 1.0];
            }
            if let Some(olm) = mtoon
                .get("outlineLightingMixFactor")
                .and_then(|v| v.as_f64())
            {
                def.outline_lighting_mix = olm as f32;
            }
        }

        out.push(def);
    }

    if out.is_empty() {
        out.push(MtoonMaterialDef::default());
    }
    out
}

fn assign_humanoid_bone(table: &mut HumanoidBoneTable, name: &str, node: usize) {
    match name.trim().to_ascii_lowercase().as_str() {
        "hips" => table.hips = Some(node),
        "spine" => table.spine = Some(node),
        "chest" => table.chest = Some(node),
        "upperchest" | "upper_chest" => table.upper_chest = Some(node),
        "neck" => table.neck = Some(node),
        "head" => table.head = Some(node),
        "lefteye" | "left_eye" => table.left_eye = Some(node),
        "righteye" | "right_eye" => table.right_eye = Some(node),
        "leftshoulder" | "left_shoulder" => table.left_shoulder = Some(node),
        "rightshoulder" | "right_shoulder" => table.right_shoulder = Some(node),
        "leftupperarm" | "left_upper_arm" => table.left_upper_arm = Some(node),
        "rightupperarm" | "right_upper_arm" => table.right_upper_arm = Some(node),
        "leftlowerarm" | "left_lower_arm" => table.left_lower_arm = Some(node),
        "rightlowerarm" | "right_lower_arm" => table.right_lower_arm = Some(node),
        _ => {}
    }
}

fn parse_vrm1_extension(
    val: &serde_json::Value,
    node_to_mesh: &[Option<usize>],
    title: &mut String,
    humanoid: &mut HumanoidBoneTable,
    presets: &mut [ExpressionBinding; VRM_PRESET_COUNT],
    arkit: &mut [ExpressionBinding; ARKIT_BLENDSHAPE_COUNT],
) {
    if let Some(name) = val
        .get("meta")
        .and_then(|m| m.get("name"))
        .and_then(|n| n.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        *title = name.trim().to_string();
    }

    if let Some(bones_obj) = val
        .get("humanoid")
        .and_then(|h| h.get("humanBones"))
        .and_then(|b| b.as_object())
    {
        for (bone_name, bone_val) in bones_obj {
            if let Some(node_idx) = bone_val.get("node").and_then(|n| n.as_u64()) {
                assign_humanoid_bone(humanoid, bone_name, node_idx as usize);
            }
        }
    }

    let parse_expr = |expr_val: &serde_json::Value| -> ExpressionBinding {
        let mut binding = ExpressionBinding {
            is_binary: expr_val
                .get("isBinary")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            override_blink: expr_val
                .get("overrideBlink")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s != "none"),
            override_mouth: expr_val
                .get("overrideMouth")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s != "none"),
            binds: Vec::new(),
        };
        if let Some(binds) = expr_val.get("morphTargetBinds").and_then(|b| b.as_array()) {
            for b in binds {
                let Some(node_idx) = b.get("node").and_then(|v| v.as_u64()) else {
                    continue;
                };
                let Some(target_idx) = b.get("index").and_then(|v| v.as_u64()) else {
                    continue;
                };
                let weight = b.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                if let Some(Some(mesh_idx)) = node_to_mesh.get(node_idx as usize) {
                    binding.binds.push(MorphBindDef {
                        mesh_idx: *mesh_idx,
                        target_idx: target_idx as usize,
                        weight: weight.clamp(0.0, 1.0),
                    });
                }
            }
        }
        binding
    };

    if let Some(exprs) = val.get("expressions") {
        if let Some(preset_obj) = exprs.get("preset").and_then(|v| v.as_object()) {
            for (name, ev) in preset_obj {
                if let Some(slot) = VrmPresetExpression::from_vrm_name(name) {
                    presets[slot as usize] = parse_expr(ev);
                }
            }
        }
        if let Some(custom_obj) = exprs.get("custom").and_then(|v| v.as_object()) {
            for (name, ev) in custom_obj {
                if let Some(slot) = ArkitBlendshape::from_name_relaxed(name) {
                    arkit[slot as usize] = parse_expr(ev);
                }
            }
        }
    }
}

fn parse_vrm1_springbones(
    sb_val: &serde_json::Value,
    nodes: &[VrmNodeDef],
    topo_order: &[usize],
    out_colliders: &mut Vec<SpringColliderDef>,
    out_links: &mut Vec<SpringJointLink>,
) {
    if let Some(cols) = sb_val.get("colliders").and_then(|v| v.as_array()) {
        for c in cols {
            let Some(node_idx) = c.get("node").and_then(|v| v.as_u64()) else {
                continue;
            };
            let Some(shape_obj) = c.get("shape") else {
                continue;
            };
            if let Some(sp) = shape_obj.get("sphere") {
                let offset = sp
                    .get("offset")
                    .and_then(|v| serde_json::from_value::<[f32; 3]>(v.clone()).ok())
                    .map(Vec3::from_array)
                    .unwrap_or(Vec3::ZERO);
                let radius = sp.get("radius").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                out_colliders.push(SpringColliderDef {
                    node_idx: node_idx as usize,
                    shape: SpringColliderShape::Sphere { offset, radius },
                });
            } else if let Some(cap) = shape_obj.get("capsule") {
                let offset = cap
                    .get("offset")
                    .and_then(|v| serde_json::from_value::<[f32; 3]>(v.clone()).ok())
                    .map(Vec3::from_array)
                    .unwrap_or(Vec3::ZERO);
                let tail = cap
                    .get("tail")
                    .and_then(|v| serde_json::from_value::<[f32; 3]>(v.clone()).ok())
                    .map(Vec3::from_array)
                    .unwrap_or(offset);
                let radius = cap.get("radius").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                out_colliders.push(SpringColliderDef {
                    node_idx: node_idx as usize,
                    shape: SpringColliderShape::Capsule {
                        offset,
                        tail,
                        radius,
                    },
                });
            }
        }
    }

    let mut collider_groups: Vec<Vec<usize>> = Vec::new();
    if let Some(cgs) = sb_val.get("colliderGroups").and_then(|v| v.as_array()) {
        for cg in cgs {
            let list = cg
                .get("colliders")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_u64().map(|u| u as usize))
                        .filter(|&i| i < out_colliders.len())
                        .collect()
                })
                .unwrap_or_default();
            collider_groups.push(list);
        }
    }

    if let Some(springs) = sb_val.get("springs").and_then(|v| v.as_array()) {
        for sp in springs {
            let Some(joints) = sp.get("joints").and_then(|v| v.as_array()) else {
                continue;
            };
            if joints.len() < 2 {
                continue;
            }
            let mut active_cols = Vec::new();
            if let Some(cg_refs) = sp.get("colliderGroups").and_then(|v| v.as_array()) {
                for r in cg_refs {
                    if let Some(gi) = r.as_u64().and_then(|u| collider_groups.get(u as usize)) {
                        for &ci in gi {
                            if !active_cols.contains(&ci) {
                                active_cols.push(ci);
                            }
                        }
                    }
                }
            }

            for k in 0..joints.len() - 1 {
                let j_cur = &joints[k];
                let j_next = &joints[k + 1];
                let (Some(n_cur), Some(n_next)) = (
                    j_cur
                        .get("node")
                        .and_then(|v| v.as_u64())
                        .map(|u| u as usize),
                    j_next
                        .get("node")
                        .and_then(|v| v.as_u64())
                        .map(|u| u as usize),
                ) else {
                    continue;
                };
                if n_cur >= nodes.len() || n_next >= nodes.len() {
                    continue;
                }
                let cur_node = &nodes[n_cur];
                let next_world_pos = nodes[n_next].rest_world_matrix.transform_point3(Vec3::ZERO);
                let tail_in_cur = cur_node
                    .rest_world_matrix
                    .inverse()
                    .transform_point3(next_world_pos);
                let len = tail_in_cur.length();
                if len < 1e-4 {
                    continue;
                }

                let stiffness = j_cur
                    .get("stiffness")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0) as f32;
                let gravity_power = j_cur
                    .get("gravityPower")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as f32;
                let gravity_dir = j_cur
                    .get("gravityDir")
                    .and_then(|v| serde_json::from_value::<Vec3OrXyz>(v.clone()).ok())
                    .map(|g| g.to_vec3().normalize_or_zero())
                    .unwrap_or(Vec3::NEG_Y);
                let drag_force = j_cur
                    .get("dragForce")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.4) as f32;
                let hit_radius = j_cur
                    .get("hitRadius")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as f32;

                out_links.push(SpringJointLink {
                    node_idx: n_cur,
                    parent_node_idx: cur_node.parent,
                    local_rest_pos: cur_node.rest_translation,
                    local_rest_rot: cur_node.rest_rotation,
                    local_rest_scale: cur_node.rest_scale,
                    local_bone_axis: tail_in_cur / len,
                    bone_length: len,
                    stiffness,
                    gravity_power,
                    gravity_dir,
                    drag_force,
                    hit_radius,
                    collider_indices: active_cols.clone(),
                });
            }
        }
    }

    sort_links_topologically(out_links, topo_order);
}

#[allow(clippy::too_many_arguments)]
fn parse_vrm0_extension(
    val: &serde_json::Value,
    nodes: &[VrmNodeDef],
    topo_order: &[usize],
    title: &mut String,
    humanoid: &mut HumanoidBoneTable,
    presets: &mut [ExpressionBinding; VRM_PRESET_COUNT],
    arkit: &mut [ExpressionBinding; ARKIT_BLENDSHAPE_COUNT],
    materials: &mut [MtoonMaterialDef],
    out_colliders: &mut Vec<SpringColliderDef>,
    out_links: &mut Vec<SpringJointLink>,
) {
    if let Some(t) = val
        .get("meta")
        .and_then(|m| m.get("title"))
        .and_then(|n| n.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        *title = t.trim().to_string();
    }

    if let Some(bones_arr) = val
        .get("humanoid")
        .and_then(|h| h.get("humanBones"))
        .and_then(|b| b.as_array())
    {
        for b in bones_arr {
            if let (Some(name), Some(node_idx)) = (
                b.get("bone").and_then(|v| v.as_str()),
                b.get("node").and_then(|v| v.as_u64()),
            ) {
                assign_humanoid_bone(humanoid, name, node_idx as usize);
            }
        }
    }

    if let Some(groups) = val
        .get("blendShapeMaster")
        .and_then(|m| m.get("blendShapeGroups"))
        .and_then(|g| g.as_array())
    {
        for g in groups {
            let is_binary = g.get("isBinary").and_then(|v| v.as_bool()).unwrap_or(false);
            let mut binds = Vec::new();
            if let Some(b_arr) = g.get("binds").and_then(|v| v.as_array()) {
                for b in b_arr {
                    let (Some(mesh_idx), Some(target_idx)) = (
                        b.get("mesh").and_then(|v| v.as_u64()),
                        b.get("index").and_then(|v| v.as_u64()),
                    ) else {
                        continue;
                    };
                    let weight_pct = b.get("weight").and_then(|v| v.as_f64()).unwrap_or(100.0);
                    binds.push(MorphBindDef {
                        mesh_idx: mesh_idx as usize,
                        target_idx: target_idx as usize,
                        weight: ((weight_pct as f32) * 0.01).clamp(0.0, 1.0),
                    });
                }
            }
            if binds.is_empty() {
                continue;
            }
            let binding = ExpressionBinding {
                binds,
                is_binary,
                override_blink: false,
                override_mouth: false,
            };

            let preset_name = g.get("presetName").and_then(|v| v.as_str()).unwrap_or("");
            let custom_name = g.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(slot) = VrmPresetExpression::from_vrm_name(preset_name)
                .or_else(|| VrmPresetExpression::from_vrm_name(custom_name))
            {
                if presets[slot as usize].binds.is_empty() {
                    presets[slot as usize] = binding.clone();
                }
            }
            if let Some(arkit_slot) = ArkitBlendshape::from_name_relaxed(custom_name) {
                arkit[arkit_slot as usize] = binding;
            }
        }
    }

    // VRM 0.x `materialProperties`
    if let Some(mat_props) = val.get("materialProperties").and_then(|v| v.as_array()) {
        for (i, mp) in mat_props.iter().enumerate() {
            let Some(def) = materials.get_mut(i) else {
                break;
            };
            let shader = mp.get("shader").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(rq) = mp.get("renderQueue").and_then(|v| v.as_i64()) {
                def.render_queue = rq as i32;
            }
            let floats = mp.get("floatProperties").and_then(|v| v.as_object());
            let vecs = mp.get("vectorProperties").and_then(|v| v.as_object());
            let texs = mp.get("textureProperties").and_then(|v| v.as_object());

            let get_f = |k: &str| -> Option<f32> {
                floats
                    .and_then(|m| m.get(k))
                    .and_then(|v| v.as_f64())
                    .map(|x| x as f32)
            };
            let get_v4 = |k: &str| -> Option<[f32; 4]> {
                vecs.and_then(|m| m.get(k))
                    .and_then(|v| serde_json::from_value::<[f32; 4]>(v.clone()).ok())
            };
            let get_tex = |k: &str| -> Option<usize> {
                texs.and_then(|m| m.get(k))
                    .and_then(|v| v.as_u64())
                    .map(|u| u as usize)
            };

            if shader.contains("MToon") || shader.contains("Unlit") {
                if let Some(c) = get_v4("_Color") {
                    def.base_color = c;
                }
                if let Some(st) = get_v4("_MainTex") {
                    def.uv_transform = st;
                }
                if let Some(mt) = get_tex("_MainTex") {
                    def.main_tex = Some(mt);
                }
                if let Some(sc) = get_v4("_ShadeColor") {
                    def.shade_color = [sc[0], sc[1], sc[2]];
                }
                if let Some(stex) = get_tex("_ShadeTexture") {
                    def.shade_tex = Some(stex);
                } else {
                    def.shade_tex = def.main_tex;
                }
                if let Some(shift) = get_f("_ShadeShift") {
                    def.shading_shift = shift;
                }
                if let Some(toony) = get_f("_ShadeToony") {
                    def.shading_toony = toony;
                }
                if let Some(mc) = get_tex("_SphereAdd") {
                    def.matcap_tex = Some(mc);
                    def.matcap_factor = [1.0, 1.0, 1.0];
                }
                if let Some(rc) = get_v4("_RimColor") {
                    def.rim_color = [rc[0], rc[1], rc[2]];
                }
                if let Some(rp) = get_f("_RimFresnelPower") {
                    def.rim_fresnel_power = rp;
                }
                if let Some(rl) = get_f("_RimLift") {
                    def.rim_lift = rl;
                }
                if let Some(cull) = get_f("_CullMode") {
                    def.double_sided = (cull as i32) == 0;
                }
                if let Some(cut) = get_f("_Cutoff") {
                    def.alpha_cutoff = cut;
                }
                if let Some(bm) = get_f("_BlendMode") {
                    match bm.round() as i32 {
                        0 => {
                            def.alpha_mode = AlphaModeKind::Opaque;
                            def.z_write = true;
                        }
                        1 => {
                            def.alpha_mode = AlphaModeKind::Mask;
                            def.z_write = true;
                        }
                        2 => {
                            def.alpha_mode = AlphaModeKind::Blend;
                            def.z_write = false;
                        }
                        3 => {
                            def.alpha_mode = AlphaModeKind::Blend;
                            def.z_write = true;
                        }
                        _ => {}
                    }
                }
                if shader.contains("UnlitTransparentZWrite") {
                    def.alpha_mode = AlphaModeKind::Blend;
                    def.z_write = true;
                }
                let ow_mode = get_f("_OutlineWidthMode").unwrap_or(0.0).round() as i32;
                let ow_raw = get_f("_OutlineWidth").unwrap_or(0.0);
                match ow_mode {
                    1 if ow_raw > 0.0 => {
                        def.outline_mode = OutlineModeKind::World;
                        // VRM 0.x stores `_OutlineWidth` in centimeters; convert to meters.
                        def.outline_width = ow_raw * 0.01;
                    }
                    2 if ow_raw > 0.0 => {
                        def.outline_mode = OutlineModeKind::Screen;
                        def.outline_width = ow_raw * 0.005;
                    }
                    _ => {
                        def.outline_mode = OutlineModeKind::None;
                    }
                }
                if let Some(oc) = get_v4("_OutlineColor") {
                    def.outline_color = oc;
                }
                let oc_mode = get_f("_OutlineColorMode").unwrap_or(0.0).round() as i32;
                def.outline_lighting_mix = if oc_mode == 1 {
                    get_f("_OutlineLightingMix").unwrap_or(1.0)
                } else {
                    0.0
                };
            }
        }
    }

    // VRM 0.x `secondaryAnimation`
    if let Some(sec) = val.get("secondaryAnimation") {
        let mut group_map: Vec<Vec<usize>> = Vec::new();
        if let Some(cgs) = sec.get("colliderGroups").and_then(|v| v.as_array()) {
            for cg in cgs {
                let mut indices = Vec::new();
                if let Some(node_idx) = cg.get("node").and_then(|v| v.as_u64()) {
                    if let Some(cols) = cg.get("colliders").and_then(|v| v.as_array()) {
                        for c in cols {
                            let offset = c
                                .get("offset")
                                .and_then(|v| serde_json::from_value::<Vec3OrXyz>(v.clone()).ok())
                                .map(|o| o.to_vec3())
                                .unwrap_or(Vec3::ZERO);
                            let radius =
                                c.get("radius").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                            let idx = out_colliders.len();
                            out_colliders.push(SpringColliderDef {
                                node_idx: node_idx as usize,
                                shape: SpringColliderShape::Sphere { offset, radius },
                            });
                            indices.push(idx);
                        }
                    }
                }
                group_map.push(indices);
            }
        }

        if let Some(bgs) = sec.get("boneGroups").and_then(|v| v.as_array()) {
            for bg in bgs {
                let stiffness = bg
                    .get("stiffiness")
                    .or_else(|| bg.get("stiffness"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0) as f32;
                let gravity_power = bg
                    .get("gravityPower")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as f32;
                let raw_grav = bg
                    .get("gravityDir")
                    .and_then(|v| serde_json::from_value::<Vec3OrXyz>(v.clone()).ok())
                    .map(|g| g.to_vec3())
                    .unwrap_or(Vec3::NEG_Y);
                // Rotate VRM 0.x gravity direction by `R_y(PI)` to match unified `+Z` world space.
                let gravity_dir =
                    Vec3::new(-raw_grav.x, raw_grav.y, -raw_grav.z).normalize_or_zero();
                let drag_force = bg.get("dragForce").and_then(|v| v.as_f64()).unwrap_or(0.4) as f32;
                let hit_radius =
                    bg.get("hitRadius").and_then(|v| v.as_f64()).unwrap_or(0.02) as f32;

                let mut active_cols = Vec::new();
                if let Some(cg_refs) = bg.get("colliderGroups").and_then(|v| v.as_array()) {
                    for r in cg_refs {
                        if let Some(list) = r.as_u64().and_then(|u| group_map.get(u as usize)) {
                            for &ci in list {
                                if !active_cols.contains(&ci) {
                                    active_cols.push(ci);
                                }
                            }
                        }
                    }
                }

                if let Some(roots) = bg.get("bones").and_then(|v| v.as_array()) {
                    for r in roots {
                        let Some(root_idx) = r.as_u64().map(|u| u as usize) else {
                            continue;
                        };
                        add_vrm0_spring_subtree(
                            root_idx,
                            nodes,
                            stiffness,
                            gravity_power,
                            gravity_dir,
                            drag_force,
                            hit_radius,
                            &active_cols,
                            out_links,
                        );
                    }
                }
            }
        }
    }

    sort_links_topologically(out_links, topo_order);
}

#[allow(clippy::too_many_arguments)]
fn add_vrm0_spring_subtree(
    node_idx: usize,
    nodes: &[VrmNodeDef],
    stiffness: f32,
    gravity_power: f32,
    gravity_dir: Vec3,
    drag_force: f32,
    hit_radius: f32,
    colliders: &[usize],
    out_links: &mut Vec<SpringJointLink>,
) {
    let Some(node) = nodes.get(node_idx) else {
        return;
    };
    let (local_axis, bone_len) = if let Some(&first_child) = node.children.first() {
        let child_pos = nodes[first_child].rest_translation;
        let len = child_pos.length();
        if len > 1e-4 {
            (child_pos / len, len)
        } else {
            (Vec3::NEG_Y, 0.07)
        }
    } else {
        let dir = node.rest_translation.normalize_or_zero();
        let axis = if dir.length_squared() > 1e-4 {
            dir
        } else {
            Vec3::NEG_Y
        };
        (axis, 0.07)
    };

    if !out_links.iter().any(|l| l.node_idx == node_idx) {
        out_links.push(SpringJointLink {
            node_idx,
            parent_node_idx: node.parent,
            local_rest_pos: node.rest_translation,
            local_rest_rot: node.rest_rotation,
            local_rest_scale: node.rest_scale,
            local_bone_axis: local_axis,
            bone_length: bone_len,
            stiffness,
            gravity_power,
            gravity_dir,
            drag_force,
            hit_radius,
            collider_indices: colliders.to_vec(),
        });
    }

    for &child in &node.children {
        add_vrm0_spring_subtree(
            child,
            nodes,
            stiffness,
            gravity_power,
            gravity_dir,
            drag_force,
            hit_radius,
            colliders,
            out_links,
        );
    }
}

fn sort_links_topologically(links: &mut [SpringJointLink], topo_order: &[usize]) {
    let mut rank = vec![usize::MAX; topo_order.len() + 1];
    for (pos, &n) in topo_order.iter().enumerate() {
        if n < rank.len() {
            rank[n] = pos;
        }
    }
    links.sort_by_key(|l| rank.get(l.node_idx).copied().unwrap_or(usize::MAX));
}

struct ParsedSkin {
    joints: Vec<(usize, Mat4)>,
}

fn parse_skins(document: &gltf::Document, buffers: &[gltf::buffer::Data]) -> Vec<ParsedSkin> {
    let mut out = Vec::with_capacity(document.skins().len());
    for skin in document.skins() {
        let reader = skin.reader(|buffer| Some(&buffers[buffer.index()]));
        let ibms: Vec<Mat4> = reader
            .read_inverse_bind_matrices()
            .map(|it| it.map(|m| Mat4::from_cols_array_2d(&m)).collect())
            .unwrap_or_default();
        let mut joints = Vec::with_capacity(skin.joints().len());
        for (i, joint_node) in skin.joints().enumerate() {
            let ibm = ibms.get(i).copied().unwrap_or(Mat4::IDENTITY);
            joints.push((joint_node.index(), ibm));
        }
        out.push(ParsedSkin { joints });
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn build_primitive_cpu(
    prim: &gltf::Primitive<'_>,
    mesh_idx: usize,
    node_idx: usize,
    skin_opt: Option<&ParsedSkin>,
    nodes: &[VrmNodeDef],
    node_world: Mat4,
    material_count: usize,
    buffers: &[gltf::buffer::Data],
    max_vertex_world_y: &mut f32,
) -> Option<VrmPrimitiveCpu> {
    let reader = prim.reader(|buffer| Some(&buffers[buffer.index()]));
    let positions: Vec<[f32; 3]> = reader.read_positions()?.collect();
    let v_count = positions.len();
    if v_count == 0 {
        return None;
    }

    let normals: Vec<[f32; 3]> = reader
        .read_normals()
        .map(|it| it.collect())
        .unwrap_or_else(|| vec![[0.0, 0.0, 1.0]; v_count]);
    let uvs: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|tc| tc.into_f32().collect())
        .unwrap_or_else(|| vec![[0.0, 0.0]; v_count]);

    let raw_joints: Vec<[u16; 4]> = reader
        .read_joints(0)
        .map(|j| j.into_u16().collect())
        .unwrap_or_else(|| vec![[0; 4]; v_count]);
    let mut raw_weights: Vec<[f32; 4]> = reader
        .read_weights(0)
        .map(|w| w.into_f32().collect())
        .unwrap_or_else(|| vec![[0.0; 4]; v_count]);

    let indices: Vec<u32> = if let Some(ind_reader) = reader.read_indices() {
        ind_reader.into_u32().collect()
    } else {
        (0..v_count as u32).collect()
    };
    if indices.len() < 3 {
        return None;
    }

    // Remap skin joints used by this primitive into <= 128 compact slots.
    let mut joint_nodes = Vec::new();
    let mut remapped_joints = vec![[0u16; 4]; v_count];
    if let Some(skin) = skin_opt {
        if skin.joints.len() <= MAX_JOINTS_PER_PRIMITIVE {
            joint_nodes = skin.joints.clone();
            remapped_joints = raw_joints;
        } else {
            let mut remap: HashMap<u16, u16> = HashMap::new();
            for v_i in 0..v_count {
                for slot in 0..4 {
                    let w = raw_weights[v_i][slot];
                    if w <= 1e-5 {
                        remapped_joints[v_i][slot] = 0;
                        continue;
                    }
                    let orig = raw_joints[v_i][slot];
                    if let Some(&mapped) = remap.get(&orig) {
                        remapped_joints[v_i][slot] = mapped;
                    } else if joint_nodes.len() < MAX_JOINTS_PER_PRIMITIVE {
                        if let Some(&entry) = skin.joints.get(orig as usize) {
                            let new_idx = joint_nodes.len() as u16;
                            joint_nodes.push(entry);
                            remap.insert(orig, new_idx);
                            remapped_joints[v_i][slot] = new_idx;
                        }
                    } else {
                        raw_weights[v_i][slot] = 0.0;
                    }
                }
            }
        }
    }

    // Track rest-pose crown Y in world space for automatic camera framing
    for v_i in 0..v_count {
        let local_p = Vec3::from_array(positions[v_i]);
        let wp = if !joint_nodes.is_empty() {
            let w = raw_weights[v_i];
            let w_sum = w[0] + w[1] + w[2] + w[3];
            if w_sum > 1e-4 {
                let mut acc = Vec4::ZERO;
                for s in 0..4 {
                    if w[s] > 1e-5 {
                        if let Some(&(jn, ibm)) = joint_nodes.get(remapped_joints[v_i][s] as usize)
                        {
                            let m = nodes[jn].rest_world_matrix * ibm;
                            acc += m * local_p.extend(1.0) * (w[s] / w_sum);
                        }
                    }
                }
                acc.truncate()
            } else {
                node_world.transform_point3(local_p)
            }
        } else {
            node_world.transform_point3(local_p)
        };
        if wp.y.is_finite() {
            *max_vertex_world_y = max_vertex_world_y.max(wp.y);
        }
    }

    let mut vertices = Vec::with_capacity(v_count);
    for v_i in 0..v_count {
        vertices.push(VrmVertex {
            pos: positions[v_i],
            norm: *normals.get(v_i).unwrap_or(&[0.0, 0.0, 1.0]),
            uv: *uvs.get(v_i).unwrap_or(&[0.0, 0.0]),
            joints: remapped_joints[v_i],
            weights: raw_weights[v_i],
        });
    }

    // Pack morph target position & normal deltas into a 2D texture array (`RGBA32F`).
    let morph_targets: Vec<_> = reader.read_morph_targets().collect();
    let morph_target_count = morph_targets.len();
    let morphs = if morph_target_count > 0 {
        let total_texels_per_layer = v_count * 2;
        let tex_w = MORPH_TEX_WIDTH.min(total_texels_per_layer.max(2));
        let tex_h = total_texels_per_layer.div_ceil(tex_w).max(1);
        let layer_floats = tex_w * tex_h * 4;
        let mut data = vec![0.0f32; layer_floats * morph_target_count];

        for (layer_idx, (pos_disp_opt, norm_disp_opt, _)) in morph_targets.into_iter().enumerate() {
            let layer_base = layer_idx * layer_floats;
            if let Some(pos_it) = pos_disp_opt {
                for (v_i, dp) in pos_it.enumerate() {
                    if v_i >= v_count {
                        break;
                    }
                    let off = layer_base + (v_i * 2) * 4;
                    data[off] = dp[0];
                    data[off + 1] = dp[1];
                    data[off + 2] = dp[2];
                }
            }
            if let Some(norm_it) = norm_disp_opt {
                for (v_i, dn) in norm_it.enumerate() {
                    if v_i >= v_count {
                        break;
                    }
                    let off = layer_base + (v_i * 2 + 1) * 4;
                    data[off] = dn[0];
                    data[off + 1] = dn[1];
                    data[off + 2] = dn[2];
                }
            }
        }

        Some(MorphDeltaArrayCpu {
            width: tex_w as u32,
            height: tex_h as u32,
            layers: morph_target_count as u32,
            data,
        })
    } else {
        None
    };

    let material_idx = prim
        .material()
        .index()
        .filter(|&i| i < material_count)
        .unwrap_or(0);

    Some(VrmPrimitiveCpu {
        mesh_idx,
        node_idx,
        material_idx,
        vertices,
        indices,
        joint_nodes,
        morphs,
        morph_target_count,
    })
}

fn compute_avatar_framing(
    nodes: &[VrmNodeDef],
    humanoid: &HumanoidBoneTable,
    max_vertex_y: f32,
) -> AvatarFramingMetrics {
    let head_pos = humanoid
        .head
        .and_then(|n| nodes.get(n))
        .map(|n| n.rest_world_matrix.transform_point3(Vec3::ZERO))
        .unwrap_or(Vec3::new(0.0, 1.42, 0.0));

    let chest_y = humanoid
        .upper_chest
        .or(humanoid.chest)
        .or(humanoid.spine)
        .and_then(|n| nodes.get(n))
        .map(|n| n.rest_world_matrix.transform_point3(Vec3::ZERO).y)
        .unwrap_or(head_pos.y - 0.24);

    let crown_y = if max_vertex_y.is_finite() && max_vertex_y > head_pos.y + 0.05 {
        max_vertex_y.min(head_pos.y + 0.42)
    } else {
        head_pos.y + 0.24
    };

    // Center slightly above the head pivot (near eye/nose bridge level)
    let eye_level_y = humanoid
        .left_eye
        .and_then(|n| nodes.get(n))
        .map(|n| n.rest_world_matrix.transform_point3(Vec3::ZERO).y)
        .unwrap_or(head_pos.y + 0.075);

    AvatarFramingMetrics {
        head_center: Vec3::new(head_pos.x, eye_level_y, head_pos.z),
        crown_y,
        bust_bottom_y: chest_y.min(head_pos.y - 0.16),
    }
}

/// Computes camera view and projection matrices for framing the VRM avatar bust.
pub fn compute_bust_camera_matrices(
    framing: &AvatarFramingMetrics,
    aspect: f32,
    zoom: f32,
    offset_y: f32,
) -> (Mat4, Mat4, Vec3) {
    let fov_y = 24.0f32.to_radians();
    let half_tan = (0.5 * fov_y).tan();

    let vertical_span = (framing.crown_y + 0.04 - framing.bust_bottom_y).clamp(0.28, 1.10);
    // Keep horizontal shoulders framed nicely on narrow portrait aspect ratios too
    let aspect_comp = if aspect < 0.9 {
        (0.9 / aspect.max(0.25)).min(1.65)
    } else {
        1.0
    };
    let base_dist = (0.52 * vertical_span / half_tan) * aspect_comp;
    let dist = (base_dist / zoom.clamp(0.35, 3.5)).clamp(0.25, 6.0);

    let target_y = 0.52 * framing.head_center.y
        + 0.48 * (0.5 * (framing.crown_y + framing.bust_bottom_y))
        + offset_y * vertical_span;
    let target = Vec3::new(framing.head_center.x, target_y, framing.head_center.z);
    let cam_pos = target + Vec3::new(0.0, 0.015, dist);

    let view = Mat4::look_at_rh(cam_pos, target, Vec3::Y);
    let proj = Mat4::perspective_rh_gl(fov_y, aspect.max(0.1), 0.05, 25.0);
    (view, proj, cam_pos)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Synthesizes a minimal valid binary `.glb` VRM 1.0 avatar in memory for unit & GPU testing.
    pub fn build_minimal_test_vrm1_glb() -> Vec<u8> {
        // 3 vertices (1 triangle in `head` local space, where `head` world Y = 1.40) + 1 morph delta
        let positions: [[f32; 3]; 3] = [[-0.12, -0.05, 0.0], [0.12, -0.05, 0.0], [0.0, 0.20, 0.0]];
        let normals: [[f32; 3]; 3] = [[0.0, 0.0, 1.0]; 3];
        let uvs: [[f32; 2]; 3] = [[0.0, 1.0], [1.0, 1.0], [0.5, 0.0]];
        let indices: [u16; 3] = [0, 1, 2];
        let morph_pos: [[f32; 3]; 3] = [[0.0, -0.04, 0.0], [0.0, -0.04, 0.0], [0.0, 0.02, 0.0]];

        let mut bin = Vec::new();
        let pos_off = bin.len();
        bin.extend_from_slice(bytemuck::cast_slice(&positions));
        let norm_off = bin.len();
        bin.extend_from_slice(bytemuck::cast_slice(&normals));
        let uv_off = bin.len();
        bin.extend_from_slice(bytemuck::cast_slice(&uvs));
        let idx_off = bin.len();
        bin.extend_from_slice(bytemuck::cast_slice(&indices));
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let mpos_off = bin.len();
        bin.extend_from_slice(bytemuck::cast_slice(&morph_pos));
        let total_bin = bin.len();

        let json = serde_json::json!({
            "asset": { "version": "2.0" },
            "extensionsUsed": ["VRMC_vrm", "VRMC_materials_mtoon"],
            "extensions": {
                "VRMC_vrm": {
                    "specVersion": "1.0",
                    "meta": { "name": "TestChibi", "authors": ["Test"] },
                    "humanoid": {
                        "humanBones": {
                            "hips": { "node": 0 },
                            "head": { "node": 1 }
                        }
                    },
                    "expressions": {
                        "preset": {
                            "aa": {
                                "morphTargetBinds": [{ "node": 1, "index": 0, "weight": 1.0 }]
                            }
                        }
                    }
                }
            },
            "scenes": [{ "nodes": [0] }],
            "scene": 0,
            "nodes": [
                { "name": "hips", "translation": [0.0, 0.9, 0.0], "children": [1] },
                { "name": "head", "translation": [0.0, 0.5, 0.0], "mesh": 0 }
            ],
            "materials": [{
                "name": "FaceMToon",
                "pbrMetallicRoughness": {
                    "baseColorFactor": [0.95, 0.55, 0.65, 1.0]
                },
                "extensions": {
                    "VRMC_materials_mtoon": {
                        "specVersion": "1.0",
                        "shadeColorFactor": [0.75, 0.35, 0.45],
                        "shadingToonyFactor": 0.92,
                        "outlineWidthMode": "worldCoordinates",
                        "outlineWidthFactor": 0.002,
                        "outlineColorFactor": [0.15, 0.08, 0.12]
                    }
                }
            }],
            "meshes": [{
                "name": "HeadMesh",
                "primitives": [{
                    "attributes": { "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 },
                    "indices": 3,
                    "material": 0,
                    "targets": [{ "POSITION": 4 }]
                }]
            }],
            "buffers": [{ "byteLength": total_bin }],
            "bufferViews": [
                { "buffer": 0, "byteOffset": pos_off, "byteLength": 36 },
                { "buffer": 0, "byteOffset": norm_off, "byteLength": 36 },
                { "buffer": 0, "byteOffset": uv_off, "byteLength": 24 },
                { "buffer": 0, "byteOffset": idx_off, "byteLength": 6 },
                { "buffer": 0, "byteOffset": mpos_off, "byteLength": 36 }
            ],
            "accessors": [
                { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [-0.12, -0.05, 0.0], "max": [0.12, 0.20, 0.0] },
                { "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" },
                { "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" },
                { "bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR" },
                { "bufferView": 4, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, -0.04, 0.0], "max": [0.0, 0.02, 0.0] }
            ]
        });

        let mut json_bytes = serde_json::to_vec(&json).unwrap();
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }

        let total_len = 12 + 8 + json_bytes.len() + 8 + bin.len();
        let mut glb = Vec::with_capacity(total_len);
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total_len as u32).to_le_bytes());

        glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x4E4F534Au32.to_le_bytes()); // JSON
        glb.extend_from_slice(&json_bytes);

        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x004E4942u32.to_le_bytes()); // BIN\0
        glb.extend_from_slice(&bin);
        glb
    }

    #[test]
    fn test_load_minimal_vrm1_glb_parses_bones_mtoon_and_morphs() {
        let glb = build_minimal_test_vrm1_glb();
        let asset = load_vrm_from_bytes(&glb).expect("must load synthetic VRM 1.0");
        assert_eq!(asset.title, "TestChibi");
        assert!(!asset.is_vrm0);
        assert_eq!(asset.humanoid.hips, Some(0));
        assert_eq!(asset.humanoid.head, Some(1));
        assert_eq!(asset.primitives.len(), 1);
        assert_eq!(asset.primitives[0].morph_target_count, 1);
        assert!(asset.primitives[0].morphs.is_some());
        assert_eq!(
            asset.preset_expressions[VrmPresetExpression::Aa as usize]
                .binds
                .len(),
            1
        );
        assert_eq!(asset.materials[0].outline_mode, OutlineModeKind::World);
    }
}
