#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D DeconvPass;
uniform sampler2D BorderHPass;
uniform vec2 outputResolution;
uniform vec2 SourceSize;
uniform float horizontal_stretch;
uniform float halo_zoom;
uniform float halo_intensity;
uniform vec3 background_color;
uniform vec4 border_crop; // left, right, top, bottom

// Precomputed exp(-(y * y) / 8.0) for |y| = 0..4
const float W[5] = float[5](1.0, 0.88249690, 0.60653066, 0.32465247, 0.13533528);

float ToSrgb1(float c) {
    return (c < 0.0031308 ? c * 12.92 : 1.055 * pow(c, 0.41666) - 0.055);
}

vec3 ToSrgb(vec3 c) {
    return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
}

void main() {
    vec2 corrected_tc = vec2(v_tc.x, 1.0 - v_tc.y);

    float video_aspect = (SourceSize.x * horizontal_stretch) / SourceSize.y;
    float output_aspect = outputResolution.x / outputResolution.y;

    vec2 scale = vec2(1.0, 1.0);
    if (video_aspect > output_aspect) {
        scale.y = output_aspect / video_aspect;
    } else {
        scale.x = video_aspect / output_aspect;
    }

    vec2 centered_tc = (corrected_tc - 0.5) / scale + 0.5;

    if (centered_tc.x < border_crop.x || centered_tc.x > (1.0 - border_crop.y) ||
        centered_tc.y < border_crop.z || centered_tc.y > (1.0 - border_crop.w)) {
        out_color = vec4(ToSrgb(background_color), 1.0);
        return;
    }

    vec4 deconv_sample = texture(DeconvPass, v_tc);
    vec3 color = (deconv_sample.a > 0.001) ? deconv_sample.rgb : ToSrgb(background_color);

    if (halo_intensity > 0.01) {
        vec2 p = abs(centered_tc - 0.5) * 2.0;
        float border_dist = max(p.x, p.y);
        float edge_factor = smoothstep(0.7, 1.0, border_dist);
        if (edge_factor > 0.0001) {
            vec3 light = vec3(0.0);
            float weights = 0.0;
            const float step_y = 0.009;
            for (int y = -4; y <= 4; ++y) {
                float sample_y = v_tc.y + float(y) * step_y;
                float sample_centered_y = ((1.0 - sample_y) - 0.5) / scale.y + 0.5;
                if (sample_centered_y >= 0.0 && sample_centered_y <= 1.0) {
                    float wy = W[abs(y)];
                    vec4 h = texture(BorderHPass, vec2(v_tc.x, sample_y));
                    light += h.rgb * wy;
                    weights += h.a * wy;
                }
            }
            if (weights > 0.0) {
                color += sqrt(light / weights) * (halo_intensity * 0.6) * edge_factor;
            }
        }
    }

    // Static sub-code-value dithering for final 8-bit display conversion (only inside rendering surface)
    float noise = fract(52.9829189 * fract(dot(gl_FragCoord.xy, vec2(0.06711056, 0.00583715)))) - 0.5;
    vec3 amplitude = smoothstep(vec3(0.0), vec3(2.0 / 255.0), color) / 255.0;
    color = clamp(color + noise * amplitude, 0.0, 1.0);

    out_color = vec4(color, 1.0);
}
