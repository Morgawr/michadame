#version 330 core

in vec2 v_uv;

uniform sampler2D u_tex;
uniform int u_mode; // 0 = premultiplied blit, 1 = unmultiplied static image

out vec4 out_color;

void main() {
    if (u_mode == 0) {
        out_color = texture(u_tex, v_uv);
    } else {
        vec4 c = texture(u_tex, v_uv);
        out_color = vec4(c.rgb * c.a, c.a);
    }
}

