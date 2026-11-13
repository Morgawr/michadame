#version 330 core
    in vec2 v_tc;
    out vec4 out_color;
    uniform sampler2D raw_tex;
    uniform vec2 overscan_offset;
    uniform vec2 underscan_stretch;
    uniform int input_range; // 0 for Full, 1 for Limited
    uniform float brightness;
    uniform float gamma;

    float ToLinear1(float c) {
        return (c <= 0.04045) ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
    }
    vec3 ToLinear(vec3 c) {
        return vec3(ToLinear1(c.r), ToLinear1(c.g), ToLinear1(c.b));
    }

    vec3 ApplyBrightnessGamma(vec3 c) {
        if (abs(gamma - 1.0) > 0.0001) {
            c = pow(max(c, vec3(0.0)), vec3(1.0 / max(gamma, 0.01)));
        }
        c *= max(brightness, 0.0);
        return clamp(c, 0.0, 1.0);
    }

    void main() {
        vec2 centered_tc = v_tc - 0.5;
        vec2 stretched_tc = centered_tc / max(underscan_stretch, vec2(0.01)) + 0.5;
        vec2 shifted_tc = stretched_tc - overscan_offset;

        if (shifted_tc.x < 0.0 || shifted_tc.x > 1.0 || shifted_tc.y < 0.0 || shifted_tc.y > 1.0) {
            out_color = vec4(0.0, 0.0, 0.0, 1.0);
            return;
        }

        // YUYV is packed: [Y0, U0, Y1, V0] across a half-width RGBA8 texture.
        // Fetch by exact integer macro-pixel coordinate (macro_x, src_y) via texelFetch
        // so normalized float UV division/filtering can never jitter or blend adjacent Y0/Y1 bytes.
        ivec2 raw_size = textureSize(raw_tex, 0);
        int full_w = raw_size.x * 2;
        int src_x = clamp(int(floor(shifted_tc.x * float(full_w))), 0, max(full_w - 1, 0));
        int src_y = clamp(int(floor(shifted_tc.y * float(raw_size.y))), 0, max(raw_size.y - 1, 0));
        int macro_x = src_x >> 1;

        vec4 yuyv_vec = texelFetch(raw_tex, ivec2(macro_x, src_y), 0);
        float y_val = ((src_x & 1) == 0) ? yuyv_vec.r : yuyv_vec.b;
        
        float u = yuyv_vec.g;
        float v = yuyv_vec.a;

        if (input_range == 1) {
            // Limited Range (MPEG) to Full Range (JPEG) expansion
            y_val = (y_val - 16.0/255.0) * (255.0/219.0);
            u = (u - 16.0/255.0) * (255.0/224.0);
            v = (v - 16.0/255.0) * (255.0/224.0);
        }

        u = u - 0.5;
        v = v - 0.5;

        // BT.709 Full Range (PC Range) conversion
        float r = y_val + 1.5748 * v;
        float g = y_val - 0.1873 * u - 0.4681 * v;
        float b = y_val + 1.8556 * u;

        vec3 rgb = ApplyBrightnessGamma(clamp(vec3(r, g, b), 0.0, 1.0));
        out_color = vec4(ToLinear(rgb), 1.0);
    }
