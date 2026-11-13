#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D DeconvPass;
uniform vec2 outputResolution;
uniform vec2 SourceSize;
uniform float horizontal_stretch;

// Precomputed exp(-(x * x) / 8.0) for |x| = 0..4
const float W[5] = float[5](1.0, 0.88249690, 0.60653066, 0.32465247, 0.13533528);

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
    float step_x = (outputResolution.y / outputResolution.x) * 0.009;
    float max_reach_x = (4.0 * step_x + 2.0 / max(outputResolution.x, 1.0)) / max(scale.x, 1e-4);

    if (centered_tc.x < -max_reach_x || centered_tc.x > 1.0 + max_reach_x) {
        out_color = vec4(0.0);
        return;
    }

    // Safe deep-interior early-out: `fs_halo_final.glsl` only reads `(v_tc.x, v_tc.y + y * 0.009)`
    // for `y in -4..4` at consumers where `max(p.x, p.y) > 0.7`.
    vec2 p = abs(centered_tc - 0.5) * 2.0;
    float dx_pad = 4.0 / max(outputResolution.x * scale.x, 1.0);
    float dy_reach = 2.0 * (0.036 + 2.0 / max(outputResolution.y, 1.0)) / max(scale.y, 1e-4);
    if (p.x + dx_pad <= 0.7 && p.y + dy_reach <= 0.7) {
        out_color = vec4(0.0);
        return;
    }

    vec3 sum_rgb2 = vec3(0.0);
    float sum_wx = 0.0;

    for (int x = -4; x <= 4; ++x) {
        float sample_x = v_tc.x + float(x) * step_x;
        float sample_centered_x = (sample_x - 0.5) / scale.x + 0.5;
        if (sample_centered_x >= 0.0 && sample_centered_x <= 1.0) {
            float wx = W[abs(x)];
            vec3 c = texture(DeconvPass, vec2(sample_x, v_tc.y)).rgb;
            sum_rgb2 += c * c * wx;
            sum_wx += wx;
        }
    }

    out_color = vec4(sum_rgb2, sum_wx);
}
