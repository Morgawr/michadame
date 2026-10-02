#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D video_texture;
uniform vec2 SourceSize;

const float SIZEVB = 3.0;
const float SIGMA_VB = 0.60;
const float invsqrsigma = 1.0 / (2.0 * SIGMA_VB * SIGMA_VB);

float gaussian(float x) {
    return exp(-x * x * invsqrsigma);
}

void main() {
    float f = fract(SourceSize.y * v_tc.y);
    f = 0.5 - f;
    vec2 tex = vec2(v_tc.x, (floor(SourceSize.y * v_tc.y) + 0.5) / SourceSize.y);
    vec4 color = vec4(0.0);
    vec2 dy = vec2(0.0, 1.0 / SourceSize.y);

    float wsum = 0.0;
    for (float n = -SIZEVB; n <= SIZEVB; n += 1.0) {
        vec4 pixel = texture(video_texture, tex + n * dy);
        float w = gaussian(n + f);
        pixel.a *= pixel.a * pixel.a;
        color += w * pixel;
        wsum += w;
    }
    color /= wsum;
    out_color = vec4(color.rgb, pow(color.a, 0.175));
}
