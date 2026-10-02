#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D video_texture;

void main() {
    vec3 c = texture(video_texture, v_tc).rgb;
    out_color = vec4(clamp(c, 0.0, 1.0), 1.0);
}
