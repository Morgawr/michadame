#version 330 core
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_tc;
layout(location = 2) in vec4 a_srgba;

uniform vec2 u_screen_size;
uniform vec2 u_offset;

out vec4 v_rgba;
out vec2 v_tc;

void main() {
    vec2 pos = a_pos - u_offset;
    gl_Position = vec4(
        2.0 * pos.x / u_screen_size.x - 1.0,
        1.0 - 2.0 * pos.y / u_screen_size.y,
        0.0,
        1.0
    );
    v_rgba = a_srgba / 255.0;
    v_tc = a_tc;
}
