#version 330 core
    in vec2 v_tc;
    out vec4 out_color;
    uniform sampler2D video_texture;
    uniform float mix_amount;

    vec3 median(vec3 a, vec3 b, vec3 c) {
        return max(min(a, b), min(max(a, b), c));
    }

    void main() {
        vec2 tex_size = vec2(textureSize(video_texture, 0));
        vec2 dx = vec2(1.0 / tex_size.x, 0.0);
        
        vec3 col_m = texture(video_texture, v_tc - dx).rgb;
        vec4 tex_c = texture(video_texture, v_tc);
        vec3 col_p = texture(video_texture, v_tc + dx).rgb;
        
        vec3 result = median(col_m, tex_c.rgb, col_p);
        out_color = vec4(mix(tex_c.rgb, result, mix_amount), tex_c.a);
    }