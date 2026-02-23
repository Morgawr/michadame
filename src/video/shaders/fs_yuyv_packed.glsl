#version 330 core
    in vec2 v_tc;
    out vec4 out_color;
    uniform sampler2D raw_tex;
    uniform int input_range; // 0 for Full, 1 for Limited

    float ToLinear1(float c) {
        return (c <= 0.04045) ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
    }
    vec3 ToLinear(vec3 c) {
        return vec3(ToLinear1(c.r), ToLinear1(c.g), ToLinear1(c.b));
    }

    void main() {
        // YUYV is packed: [Y0, U0, Y1, V0]
        // We sample the texture as if it were RGBA (each pixel has 4 channels)
        // Texture width is width / 2
        vec4 yuyv_vec = texture(raw_tex, v_tc);
        
        // Determine if we want the first or second Y based on X coordinate
        // We multiply by 2 because each texture pixel contains 2 source pixels
        float x_idx = v_tc.x * textureSize(raw_tex, 0).x * 2.0;
        float y_val;
        if (int(floor(x_idx)) % 2 == 0) {
            y_val = yuyv_vec.r; // Y0
        } else {
            y_val = yuyv_vec.b; // Y1
        }
        
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

        out_color = vec4(ToLinear(clamp(vec3(r, g, b), 0.0, 1.0)), 1.0);
    }
