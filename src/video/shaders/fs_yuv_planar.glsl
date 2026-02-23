#version 330 core
    in vec2 v_tc;
    out vec4 out_color;
    uniform sampler2D y_tex;
    uniform sampler2D u_tex;
    uniform sampler2D v_tex;

    float ToLinear1(float c) {
        return (c <= 0.04045) ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
    }
    vec3 ToLinear(vec3 c) {
        return vec3(ToLinear1(c.r), ToLinear1(c.g), ToLinear1(c.b));
    }

    void main() {
        float y = texture(y_tex, v_tc).r;
        float u = texture(u_tex, v_tc).r - 0.5;
        float v = texture(v_tex, v_tc).r - 0.5;

        // BT.709 Full Range (PC Range) conversion
        // R = Y + 1.5748 * V
        // G = Y - 0.1873 * U - 0.4681 * V
        // B = Y + 1.8556 * U
        float r = y + 1.5748 * v;
        float g = y - 0.1873 * u - 0.4681 * v;
        float b = y + 1.8556 * u;

        // Convert to linear space for the filtering pipeline
        out_color = vec4(ToLinear(clamp(vec3(r, g, b), 0.0, 1.0)), 1.0);
    }
