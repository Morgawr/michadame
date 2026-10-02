#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D video_texture;
uniform vec2 SourceSize;

const float SIZEV = 6.0;
const float SIGMA_V = 1.20;
const float invsqrsigma = 1.0 / (2.0 * SIGMA_V * SIGMA_V);

float gaussian(float x) {
    return exp(-x * x * invsqrsigma);
}

void main() {
    float f = fract(SourceSize.y * v_tc.y);
    f = 0.5 - f;
    vec2 tex = vec2(v_tc.x, (floor(SourceSize.y * v_tc.y) + 0.5) / SourceSize.y);
    vec3 color = vec3(0.0);
    vec2 dy = vec2(0.0, 1.0 / SourceSize.y);

    float wsum = 0.0;
    for (float n = -SIZEV; n <= SIZEV; n += 1.0) {
        vec3 pixel = texture(video_texture, tex + n * dy).rgb;
        float w = gaussian(n + f);
        color += w * pixel;
        wsum += w;
    }
    out_color = vec4(color / wsum, 1.0);
}
