#version 330 core
    in vec2 v_tc;
    out vec4 out_color;
    uniform sampler2D video_texture;
    uniform float hardBloomPix;

    float Gaus(float pos, float scale) {
        return exp2(scale * pos * pos);
    }

    void main() {
        vec2 tex_size = vec2(textureSize(video_texture, 0));
        vec2 dx = vec2(1.0 / tex_size.x, 0.0);
        vec3 col = vec3(0.0);
        float total = 0.0;
        for (int i = -3; i <= 3; i += 1) {
            float weight = Gaus(i, hardBloomPix);
            col += texture(video_texture, v_tc + i * dx).rgb * weight;
            total += weight;
        }
        out_color = vec4(col / total, 1.0);
    }
