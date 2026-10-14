#version 330 core
in vec2 v_tc;
out vec4 out_color;
uniform sampler2D srgb_tex;

float ToLinear1(float c) {
    return (c <= 0.04045) ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}

vec3 ToLinear(vec3 c) {
    return vec3(ToLinear1(c.r), ToLinear1(c.g), ToLinear1(c.b));
}

void main() {
    vec4 srgb = texture(srgb_tex, v_tc);
    out_color = vec4(ToLinear(srgb.rgb), srgb.a);
}
