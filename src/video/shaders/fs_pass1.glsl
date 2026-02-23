#version 330 core
    in vec2 v_tc;
    out vec4 out_color;
    uniform sampler2D pass0_texture;
    uniform float hardBloomScan;

    float Gaus(float pos, float scale) {
        return exp2(scale * pos * pos);
    }

    void main() {
        vec2 tex_size = vec2(textureSize(pass0_texture, 0));
        vec2 dy = vec2(0.0, 1.0 / tex_size.y);
        vec3 col = vec3(0.0);
        float total = 0.0;
        for (int i = -2; i <= 2; i += 1) {
            float weight = Gaus(i, hardBloomScan);
            col += texture(pass0_texture, v_tc + i * dy).rgb * weight;
            total += weight;
        }
        out_color = vec4(col / total, 1.0);
    }
