#version 430 core
// AMD FidelityFX Optical Flow — Luminance Pyramid downsample pass (2x2 box filter).

layout(local_size_x = 16, local_size_y = 16, local_size_z = 1) in;

layout(binding = 0) uniform sampler2D u_src_luma;
layout(r8, binding = 0) writeonly uniform image2D u_dst_luma;
uniform ivec2 u_src_size;
uniform ivec2 u_dst_size;

void main() {
    ivec2 gid = ivec2(gl_GlobalInvocationID.xy);
    if (gid.x >= u_dst_size.x || gid.y >= u_dst_size.y) {
        return;
    }

    ivec2 base = gid * 2;
    ivec2 max_c = max(u_src_size - ivec2(1), ivec2(0));
    float s00 = texelFetch(u_src_luma, min(base + ivec2(0, 0), max_c), 0).r;
    float s10 = texelFetch(u_src_luma, min(base + ivec2(1, 0), max_c), 0).r;
    float s01 = texelFetch(u_src_luma, min(base + ivec2(0, 1), max_c), 0).r;
    float s11 = texelFetch(u_src_luma, min(base + ivec2(1, 1), max_c), 0).r;

    float avg = 0.25 * (s00 + s10 + s01 + s11);
    imageStore(u_dst_luma, gid, vec4(avg, 0.0, 0.0, 0.0));
}
