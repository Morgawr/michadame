#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D video_texture;
uniform vec2 SourceSize;

// Precomputed normalized Gaussian weights (SIZEHB = 3, SIGMA_HB = 0.75) at 1:1 pixel centers:
const float W0 = 0.53190739;
const float W1 = 0.21867367;
const float W2 = 0.01519420;
const float W3 = 0.00017844;

vec4 tap_bloom_h(vec2 tc) {
    vec4 pixel = texture(video_texture, tc);
    pixel.a = max(max(pixel.r, pixel.g), pixel.b);
    pixel.a *= pixel.a * pixel.a;
    return pixel;
}

void main() {
    vec2 dx = vec2(1.0 / SourceSize.x, 0.0);

    vec4 color = tap_bloom_h(v_tc) * W0;
    color += (tap_bloom_h(v_tc - dx) + tap_bloom_h(v_tc + dx)) * W1;
    color += (tap_bloom_h(v_tc - 2.0 * dx) + tap_bloom_h(v_tc + 2.0 * dx)) * W2;
    color += (tap_bloom_h(v_tc - 3.0 * dx) + tap_bloom_h(v_tc + 3.0 * dx)) * W3;

    out_color = vec4(color.rgb, pow(color.a, 0.333333));
}
