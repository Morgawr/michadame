#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D u_cursor_sampler;
uniform float u_shadow_mask;
uniform float u_scanline_strength;
uniform float u_scanline_freq;
uniform float u_brightboost;

vec3 Mask(vec2 pos) {
    const float maskDark = 0.5;
    const float maskLight = 1.5;
    vec3 mask = vec3(maskDark);

    int mask_type = int(u_shadow_mask + 0.5);

    if (mask_type == 1) {
        float line = maskLight;
        float odd = 0.0;
        if (fract(pos.x * 0.166666666) < 0.5) {
            odd = 1.0;
        }
        if (fract((pos.y + odd) * 0.5) < 0.5) {
            line = maskDark;
        }
        pos.x = fract(pos.x * 0.333333333);
        if (pos.x < 0.333) {
            mask.r = maskLight;
        } else if (pos.x < 0.666) {
            mask.g = maskLight;
        } else {
            mask.b = maskLight;
        }
        mask *= line;
    } else if (mask_type == 2) {
        pos.x = fract(pos.x * 0.333333333);
        if (pos.x < 0.333) {
            mask.r = maskLight;
        } else if (pos.x < 0.666) {
            mask.g = maskLight;
        } else {
            mask.b = maskLight;
        }
    } else if (mask_type == 3) {
        pos.x += pos.y * 3.0;
        pos.x = fract(pos.x * 0.166666666);
        if (pos.x < 0.333) {
            mask.r = maskLight;
        } else if (pos.x < 0.666) {
            mask.g = maskLight;
        } else {
            mask.b = maskLight;
        }
    } else if (mask_type == 4) {
        pos.xy = floor(pos.xy * vec2(1.0, 0.5));
        pos.x += pos.y * 3.0;
        pos.x = fract(pos.x * 0.166666666);
        if (pos.x < 0.333) {
            mask.r = maskLight;
        } else if (pos.x < 0.666) {
            mask.g = maskLight;
        } else {
            mask.b = maskLight;
        }
    }

    return mask;
}

void main() {
    vec4 col = texture(u_cursor_sampler, v_tc);
    if (col.a <= 0.01) {
        discard;
    }

    vec3 rgb = col.rgb;

    if (u_scanline_strength > 0.001 && u_scanline_freq > 1.0) {
        float scan = sin(gl_FragCoord.y * 3.14159265 * 2.0 / u_scanline_freq);
        float scan_factor = 1.0 - u_scanline_strength * 0.35 * (0.5 + 0.5 * scan);
        rgb *= scan_factor;
    }

    if (u_shadow_mask > 0.5) {
        rgb *= Mask(gl_FragCoord.xy * 1.000001);
    }

    if (u_brightboost > 0.001) {
        rgb *= u_brightboost;
    }

    out_color = vec4(rgb, col.a);
}
