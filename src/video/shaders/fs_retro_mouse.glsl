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

float ToLinear1(float c) {
    return (c <= 0.04045) ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}

vec3 ToLinear(vec3 c) {
    return vec3(ToLinear1(c.r), ToLinear1(c.g), ToLinear1(c.b));
}

float ToSrgb1(float c) {
    return (c < 0.0031308 ? c * 12.92 : 1.055 * pow(c, 0.41666) - 0.055);
}

vec3 ToSrgb(vec3 c) {
    return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
}

void main() {
    vec4 col = texture(u_cursor_sampler, v_tc);
    if (col.a <= 0.01) {
        discard;
    }

    bool crt_active = (u_shadow_mask > 0.5 || u_scanline_strength > 0.001);
    if (!crt_active) {
        out_color = col;
        return;
    }

    // Convert sRGB color to linear space for physically accurate CRT phosphor/scanline rendering
    vec3 lin = ToLinear(col.rgb);

    // Apply scanlines in linear space
    if (u_scanline_strength > 0.001 && u_scanline_freq > 1.0) {
        float scan = sin(gl_FragCoord.y * 3.14159265 * 2.0 / u_scanline_freq);
        float scan_factor = 1.0 - u_scanline_strength * 0.25 * (0.5 + 0.5 * scan);
        lin *= scan_factor;
    }

    // Apply shadow mask / aperture grille in linear space
    if (u_shadow_mask > 0.5) {
        lin *= Mask(gl_FragCoord.xy * 1.000001);
    }

    // Phosphor saturation boost: on a CRT, cursor drives the electron beam
    // to full excitation, and in the CRT shader pipeline, the video feed has bloom
    // and brightboost elevating whites. Boost linear light so whites saturate brightly
    // rather than appearing dark and grey.
    float boost = 1.5 * max(u_brightboost, 1.0);
    lin *= boost;

    // Convert back from linear space to sRGB display gamma
    vec3 srgb = ToSrgb(lin);

    out_color = vec4(clamp(srgb, 0.0, 1.0), col.a);
}
