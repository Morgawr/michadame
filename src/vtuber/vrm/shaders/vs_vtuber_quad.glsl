#version 330 core

layout(location = 0) in vec2 a_unit_pos;

uniform vec4 u_rect_ndc; // min_x, min_y, max_x, max_y in NDC [-1..1]
uniform vec4 u_uv_rect;  // u0, v0, u1, v1

out vec2 v_uv;

void main() {
    vec2 ndc = mix(u_rect_ndc.xy, u_rect_ndc.zw, a_unit_pos);
    v_uv = mix(u_uv_rect.xy, u_uv_rect.zw, a_unit_pos);
    gl_Position = vec4(ndc, 0.0, 1.0);
}
