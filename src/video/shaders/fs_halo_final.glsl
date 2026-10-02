#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D DeconvPass;
uniform vec2 outputResolution;
uniform vec2 SourceSize;
uniform float horizontal_stretch;
uniform float halo_zoom;
uniform float halo_intensity;
uniform vec3 background_color;

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

    // The CRT filter effect should ONLY be applied to the rendering surface!
    // It must NEVER affect the black bars outside of it:
    if (centered_tc.x < 0.0 || centered_tc.x > 1.0 || centered_tc.y < 0.0 || centered_tc.y > 1.0) {
        out_color = vec4(ToSrgb(background_color), 1.0);
        return;
    }

    vec4 deconv_sample = texture(DeconvPass, v_tc);
    vec3 color = (deconv_sample.a > 0.001) ? deconv_sample.rgb : ToSrgb(background_color);

    if (halo_intensity > 0.01) {
        vec3 light = vec3(0.0);
        float weights = 0.0;
        vec2 stepSize = vec2(outputResolution.y / outputResolution.x, 1.0) * 0.009;
        for (int y = -4; y <= 4; ++y) {
            for (int x = -4; x <= 4; ++x) {
                vec2 offset = vec2(float(x), float(y));
                float weight = exp(-dot(offset, offset) / 8.0);
                vec2 sample_pos = v_tc + offset * stepSize;
                vec2 sample_corrected = vec2(sample_pos.x, 1.0 - sample_pos.y);
                vec2 sample_centered = (sample_corrected - 0.5) / scale + 0.5;
                if (sample_centered.x >= 0.0 && sample_centered.x <= 1.0 && sample_centered.y >= 0.0 && sample_centered.y <= 1.0) {
                    vec3 sampleColor = texture(DeconvPass, sample_pos).rgb;
                    light += sampleColor * sampleColor * weight;
                    weights += weight;
                }
            }
        }
        if (weights > 0.0) {
            vec2 p = abs(centered_tc - 0.5) * 2.0;
            float border_dist = max(p.x, p.y);
            float edge_factor = smoothstep(0.7, 1.0, border_dist);
            color += sqrt(light / weights) * (halo_intensity * 0.6) * edge_factor;
        }
    }

    // Static sub-code-value dithering for final 8-bit display conversion (only inside rendering surface)
    float noise = fract(52.9829189 * fract(dot(gl_FragCoord.xy, vec2(0.06711056, 0.00583715)))) - 0.5;
    vec3 amplitude = smoothstep(vec3(0.0), vec3(2.0 / 255.0), color) / 255.0;
    color = clamp(color + noise * amplitude, 0.0, 1.0);

    out_color = vec4(color, 1.0);
}
