#version 330 core
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_tc;

uniform vec2 u_screen_size;
uniform vec2 u_cursor_pos;
uniform vec2 u_cursor_size;

out vec2 v_tc;

void main() {
    vec2 unit_pos = vec2(a_tc.x, 1.0 - a_tc.y);
    vec2 pixel_pos = u_cursor_pos + unit_pos * u_cursor_size;

    gl_Position = vec4(
        2.0 * pixel_pos.x / u_screen_size.x - 1.0,
        1.0 - 2.0 * pixel_pos.y / u_screen_size.y,
        0.0,
        1.0
    );
    v_tc = unit_pos;
}
