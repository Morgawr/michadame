#version 330 core
in vec2 v_tc;
out vec4 out_color;
uniform sampler2D srgb_tex;
uniform vec2 overscan_offset;
uniform vec2 underscan_stretch;

float ToLinear1(float c) {
    return (c <= 0.04045) ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}

vec3 ToLinear(vec3 c) {
    return vec3(ToLinear1(c.r), ToLinear1(c.g), ToLinear1(c.b));
}

void main() {
    vec2 stretch = (underscan_stretch.x <= 0.0 || underscan_stretch.y <= 0.0)
        ? vec2(1.0)
        : max(underscan_stretch, vec2(0.01));
    vec2 centered_tc = v_tc - 0.5;
    vec2 stretched_tc = centered_tc / stretch + 0.5;
    vec2 shifted_tc = stretched_tc - overscan_offset;

    if (shifted_tc.x < 0.0 || shifted_tc.x > 1.0 || shifted_tc.y < 0.0 || shifted_tc.y > 1.0) {
        out_color = vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }

    vec4 srgb = texture(srgb_tex, shifted_tc);
    out_color = vec4(ToLinear(srgb.rgb), srgb.a);
}
