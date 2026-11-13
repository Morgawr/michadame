#version 330 core

layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec3 a_norm;
layout(location = 2) in vec2 a_uv;
layout(location = 3) in uvec4 a_joints;
layout(location = 4) in vec4 a_weights;

uniform mat4 u_view_proj;
uniform mat4 u_view_mat;
uniform mat4 u_node_world;
uniform bool u_has_skin;
uniform mat4 u_joint_matrices[128];

uniform bool u_has_morphs;
uniform sampler2DArray u_morph_deltas;
uniform int u_morph_tex_width;
uniform int u_active_morph_count;
uniform int u_active_morph_indices[64];
uniform float u_active_morph_weights[64];

uniform int u_outline_mode; // 0 = surface, 1 = world width, 2 = screen width
uniform float u_outline_width;
uniform vec2 u_viewport_size;

out vec3 v_world_pos;
out vec3 v_world_norm;
out vec2 v_uv;

void main() {
    vec3 local_pos = a_pos;
    vec3 local_norm = a_norm;

    if (u_has_morphs && u_active_morph_count > 0 && u_morph_tex_width > 0) {
        int base_texel = gl_VertexID * 2;
        ivec2 pos_coord = ivec2(base_texel % u_morph_tex_width, base_texel / u_morph_tex_width);
        ivec2 norm_coord = ivec2((base_texel + 1) % u_morph_tex_width, (base_texel + 1) / u_morph_tex_width);

        for (int i = 0; i < u_active_morph_count; ++i) {
            float w = u_active_morph_weights[i];
            int layer = u_active_morph_indices[i];
            local_pos += texelFetch(u_morph_deltas, ivec3(pos_coord, layer), 0).xyz * w;
            local_norm += texelFetch(u_morph_deltas, ivec3(norm_coord, layer), 0).xyz * w;
        }
    }

    mat4 skin_mat = u_node_world;
    if (u_has_skin) {
        float w_sum = a_weights.x + a_weights.y + a_weights.z + a_weights.w;
        if (w_sum > 0.0001) {
            vec4 nw = a_weights / w_sum;
            skin_mat = nw.x * u_joint_matrices[min(int(a_joints.x), 127)]
                     + nw.y * u_joint_matrices[min(int(a_joints.y), 127)]
                     + nw.z * u_joint_matrices[min(int(a_joints.z), 127)]
                     + nw.w * u_joint_matrices[min(int(a_joints.w), 127)];
        }
    }

    vec4 world_pos4 = skin_mat * vec4(local_pos, 1.0);
    vec3 world_norm = normalize(mat3(skin_mat) * local_norm);

    if (u_outline_mode == 1) {
        world_pos4.xyz += world_norm * u_outline_width;
    }

    vec4 clip_pos = u_view_proj * world_pos4;

    if (u_outline_mode == 2) {
        vec3 view_norm = mat3(u_view_mat) * world_norm;
        vec2 ndc_dir = normalize(view_norm.xy + vec2(1e-6, 1e-6));
        float aspect = max(u_viewport_size.x, 1.0) / max(u_viewport_size.y, 1.0);
        ndc_dir.x /= aspect;
        clip_pos.xy += ndc_dir * (u_outline_width * 2.0 * clip_pos.w);
    }

    if (u_outline_mode != 0) {
        clip_pos.z += 0.00015 * clip_pos.w;
    }

    v_world_pos = world_pos4.xyz;
    v_world_norm = world_norm;
    v_uv = a_uv;
    gl_Position = clip_pos;
}
