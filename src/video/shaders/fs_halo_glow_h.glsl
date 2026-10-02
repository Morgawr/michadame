#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D video_texture;
uniform vec2 SourceSize;

const float SIZEH = 6.0;
const float SIGMA_H = 1.20;
const float invsqrsigma = 1.0 / (2.0 * SIGMA_H * SIGMA_H);

float gaussian(float x) {
    return exp(-x * x * invsqrsigma);
}

void main() {
    float f = fract(SourceSize.x * v_tc.x);
    f = 0.5 - f;
    vec2 tex = vec2((floor(SourceSize.x * v_tc.x) + 0.5) / SourceSize.x, v_tc.y);
    vec3 color = vec3(0.0);
    vec2 dx = vec2(1.0 / SourceSize.x, 0.0);

    float wsum = 0.0;
    for (float n = -SIZEH; n <= SIZEH; n += 1.0) {
        vec3 pixel = texture(video_texture, tex + n * dx).rgb;
        float w = gaussian(n + f);
        color += w * pixel;
        wsum += w;
    }
    out_color = vec4(color / wsum, 1.0);
}
