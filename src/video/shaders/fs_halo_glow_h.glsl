#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D video_texture;
uniform vec2 SourceSize;

// Normalized Gaussian weights (SIZEH = 6, SIGMA_H = 1.20) paired for GL_LINEAR 1:1 texels:
const float W0  = 0.33245174;
const float W12 = 0.31782403;
const float O12 = 1.26082854;
const float W34 = 0.01589214;
const float O34 = 3.08087216;
const float W56 = 0.00005770;
const float O56 = 5.02147019;

void main() {
    float inv_w = 1.0 / SourceSize.x;
    vec2 d12 = vec2(O12 * inv_w, 0.0);
    vec2 d34 = vec2(O34 * inv_w, 0.0);
    vec2 d56 = vec2(O56 * inv_w, 0.0);

    vec3 color = texture(video_texture, v_tc).rgb * W0;
    color += (texture(video_texture, v_tc - d12).rgb + texture(video_texture, v_tc + d12).rgb) * W12;
    color += (texture(video_texture, v_tc - d34).rgb + texture(video_texture, v_tc + d34).rgb) * W34;
    color += (texture(video_texture, v_tc - d56).rgb + texture(video_texture, v_tc + d56).rgb) * W56;

    out_color = vec4(color, 1.0);
}
