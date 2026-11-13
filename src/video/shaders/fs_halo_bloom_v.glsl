#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D video_texture;
uniform vec2 SourceSize;

// Precomputed normalized Gaussian weights (SIZEVB = 3, SIGMA_VB = 0.60) at 1:1 pixel centers:
const float W0 = 0.66381505;
const float W1 = 0.16552375;
const float W2 = 0.00256626;
const float W3 = 0.00000247;

vec4 tap_bloom_v(vec2 tc) {
    vec4 pixel = texture(video_texture, tc);
    pixel.a *= pixel.a * pixel.a;
    return pixel;
}

void main() {
    vec2 dy = vec2(0.0, 1.0 / SourceSize.y);

    vec4 color = tap_bloom_v(v_tc) * W0;
    color += (tap_bloom_v(v_tc - dy) + tap_bloom_v(v_tc + dy)) * W1;
    color += (tap_bloom_v(v_tc - 2.0 * dy) + tap_bloom_v(v_tc + 2.0 * dy)) * W2;
    color += (tap_bloom_v(v_tc - 3.0 * dy) + tap_bloom_v(v_tc + 3.0 * dy)) * W3;

    out_color = vec4(color.rgb, pow(color.a, 0.175));
}
