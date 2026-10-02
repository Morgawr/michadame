#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D video_texture;
uniform vec2 SourceSize;

const float SIZEHB = 3.0;
const float SIGMA_HB = 0.75;
const float invsqrsigma = 1.0 / (2.0 * SIGMA_HB * SIGMA_HB);

float gaussian(float x) {
    return exp(-x * x * invsqrsigma);
}

void main() {
    float f = fract(SourceSize.x * v_tc.x);
    f = 0.5 - f;
    vec2 tex = vec2((floor(SourceSize.x * v_tc.x) + 0.5) / SourceSize.x, v_tc.y);
    vec4 color = vec4(0.0);
    vec2 dx = vec2(1.0 / SourceSize.x, 0.0);

    float wsum = 0.0;
    for (float n = -SIZEHB; n <= SIZEHB; n += 1.0) {
        vec4 pixel = texture(video_texture, tex + n * dx);
        float w = gaussian(n + f);
        pixel.a = max(max(pixel.r, pixel.g), pixel.b);
        pixel.a *= pixel.a * pixel.a;
        color += w * pixel;
        wsum += w;
    }
    color /= wsum;
    out_color = vec4(color.rgb, pow(color.a, 0.333333));
}
