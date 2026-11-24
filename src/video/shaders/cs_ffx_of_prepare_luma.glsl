#version 430 core
// AMD FidelityFX Optical Flow — Prepare Luma pass (GLSL 430 compute port).
// Converts linear-RGB RGBA16F input into perceptual sRGB BT.709 luminance (R8).

layout(local_size_x = 16, local_size_y = 16, local_size_z = 1) in;

layout(binding = 0) uniform sampler2D u_input_color;
layout(r8, binding = 0) writeonly uniform image2D u_out_luma;
uniform ivec2 u_input_size;

float linear_to_srgb_channel(float c) {
    float x = clamp(c, 0.0, 1.0);
    return (x <= 0.0031308) ? (x * 12.92) : (1.055 * pow(x, 1.0 / 2.4) - 0.055);
}

void main() {
    ivec2 gid = ivec2(gl_GlobalInvocationID.xy);
    if (gid.x >= u_input_size.x || gid.y >= u_input_size.y) {
        return;
    }

    vec3 lin = texelFetch(u_input_color, gid, 0).rgb;
    vec3 srgb = vec3(
        linear_to_srgb_channel(lin.r),
        linear_to_srgb_channel(lin.g),
        linear_to_srgb_channel(lin.b)
    );
    float luma = dot(srgb, vec3(0.2126, 0.7152, 0.0722));
    imageStore(u_out_luma, gid, vec4(clamp(luma, 0.0, 1.0), 0.0, 0.0, 0.0));
}
