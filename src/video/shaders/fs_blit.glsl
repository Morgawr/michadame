#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D u_texture;

void main() {
    out_color = texture(u_texture, v_tc);
}
