#version 330 core
// Auto-transpiled from MPV hook
in vec2 v_tc;
out vec4 out_color;
uniform sampler2D input_tex;
uniform sampler2D conv2d_last_tf;
uniform sampler2D conv2d_last_tf1;
uniform sampler2D conv2d_last_tf2;

//!BIND MAIN
//!BIND conv2d_last_tf
//!BIND conv2d_last_tf1
//!BIND conv2d_last_tf2
//!SAVE MAIN
//!WIDTH conv2d_last_tf.w 2 *
//!HEIGHT conv2d_last_tf.h 2 *
//!WHEN OUTPUT.w MAIN.w / 1.200 > OUTPUT.h MAIN.h / 1.200 > *
vec4 hook() {
    vec2 f0 = fract(v_tc * vec2(textureSize(conv2d_last_tf, 0)));
    ivec2 i0 = ivec2(f0 * vec2(2.0));
    float c0 = texture(conv2d_last_tf, (vec2(0.5) - f0) * (1.0 / vec2(textureSize(conv2d_last_tf, 0))) + v_tc)[i0.y * 2 + i0.x];
    vec2 f1 = fract(v_tc * vec2(textureSize(conv2d_last_tf1, 0)));
    ivec2 i1 = ivec2(f1 * vec2(2.0));
    float c1 = texture(conv2d_last_tf1, (vec2(0.5) - f1) * (1.0 / vec2(textureSize(conv2d_last_tf1, 0))) + v_tc)[i1.y * 2 + i1.x];
    vec2 f2 = fract(v_tc * vec2(textureSize(conv2d_last_tf2, 0)));
    ivec2 i2 = ivec2(f2 * vec2(2.0));
    float c2 = texture(conv2d_last_tf2, (vec2(0.5) - f2) * (1.0 / vec2(textureSize(conv2d_last_tf2, 0))) + v_tc)[i2.y * 2 + i2.x];
    float c3 = c2;
    return vec4(c0, c1, c2, c3) + texture(input_tex, v_tc);
}


void main() {
    vec4 h_out = hook();
    out_color = vec4(h_out.rgb, 1.0);
}