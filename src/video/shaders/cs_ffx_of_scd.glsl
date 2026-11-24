#version 430 core
// AMD FidelityFX Optical Flow — Scene Change Detection (SCD) pass.
// Single 16x16 workgroup builds 32-bin soft-splatted luminance histograms on a coarse
// pyramid level and computes Total Variation histogram divergence + mean structural delta.

layout(local_size_x = 16, local_size_y = 16, local_size_z = 1) in;

layout(binding = 0) uniform sampler2D u_prev_luma;
layout(binding = 1) uniform sampler2D u_curr_luma;
layout(r32ui, binding = 0) writeonly uniform uimage2D u_out_scd;

layout(std430, binding = 0) buffer ScdTelemetry {
    uint scene_cut;
    uint hist_div_e4;
    uint mean_diff_e4;
    uint total_cuts;
    uint valid_blocks;
    uint total_blocks;
    uint quality_fallback;
    uint total_quality_fallbacks;
    int sum_mv_x;
    int sum_mv_y;
    uint splat_count;
    int global_mv_x_q;
    int global_mv_y_q;
    uint _pad0;
    uint _pad1;
    uint _pad2;
} u_stats;

uniform ivec2 u_level_size;
uniform int u_reset;
uniform float u_scd_sensitivity;

const uint HIST_BINS = 32u;
const float HIST_SCALE = 64.0;

shared uint s_hist_prev[HIST_BINS];
shared uint s_hist_curr[HIST_BINS];
shared float s_luma_diff[256];

void accum_soft_bin(float luma, bool is_curr) {
    float pos = clamp(luma, 0.0, 1.0) * float(HIST_BINS - 1u);
    uint b0 = min(uint(floor(pos)), HIST_BINS - 1u);
    uint b1 = min(b0 + 1u, HIST_BINS - 1u);
    float frac = pos - float(b0);
    uint w1 = uint(round(frac * HIST_SCALE));
    uint w0 = uint(HIST_SCALE) - w1;
    if (is_curr) {
        if (w0 > 0u) atomicAdd(s_hist_curr[b0], w0);
        if (w1 > 0u) atomicAdd(s_hist_curr[b1], w1);
    } else {
        if (w0 > 0u) atomicAdd(s_hist_prev[b0], w0);
        if (w1 > 0u) atomicAdd(s_hist_prev[b1], w1);
    }
}

void main() {
    uint tid = gl_LocalInvocationIndex;

    if (tid < HIST_BINS) {
        s_hist_prev[tid] = 0u;
        s_hist_curr[tid] = 0u;
    }
    s_luma_diff[tid] = 0.0;
    barrier();

    int w = max(u_level_size.x, 1);
    int h = max(u_level_size.y, 1);
    int total_px = w * h;

    float local_diff = 0.0;
    for (int idx = int(tid); idx < total_px; idx += 256) {
        ivec2 p = ivec2(idx % w, idx / w);
        float lp = clamp(texelFetch(u_prev_luma, p, 0).r, 0.0, 1.0);
        float lc = clamp(texelFetch(u_curr_luma, p, 0).r, 0.0, 1.0);
        accum_soft_bin(lp, false);
        accum_soft_bin(lc, true);
        local_diff += abs(lc - lp);
    }
    s_luma_diff[tid] = local_diff;
    barrier();

    for (uint stride = 128u; stride > 0u; stride >>= 1u) {
        if (tid < stride) {
            s_luma_diff[tid] += s_luma_diff[tid + stride];
        }
        barrier();
    }

    if (tid == 0u) {
        float denom = max(float(total_px) * HIST_SCALE, 1.0);
        float hist_l1 = 0.0;
        for (uint b = 0u; b < HIST_BINS; ++b) {
            hist_l1 += abs(float(s_hist_curr[b]) - float(s_hist_prev[b])) / denom;
        }
        float hist_div = clamp(0.5 * hist_l1, 0.0, 1.0);
        float mean_diff = clamp(s_luma_diff[0] / float(max(total_px, 1)), 0.0, 1.0);

        float sens = clamp(u_scd_sensitivity, 0.0, 1.0);
        float hist_thresh = mix(0.55, 0.16, sens);
        float mean_thresh = mix(0.45, 0.14, sens);
        bool cut = (hist_div > hist_thresh) || (mean_diff > mean_thresh);

        uint cut_u = cut ? 1u : 0u;
        uint hist_e4 = uint(clamp(hist_div * 10000.0 + 0.5, 0.0, 10000.0));
        uint diff_e4 = uint(clamp(mean_diff * 10000.0 + 0.5, 0.0, 10000.0));

        imageStore(u_out_scd, ivec2(0, 0), uvec4(cut_u, 0u, 0u, 0u));
        imageStore(u_out_scd, ivec2(1, 0), uvec4(hist_e4, 0u, 0u, 0u));
        imageStore(u_out_scd, ivec2(2, 0), uvec4(diff_e4, 0u, 0u, 0u));

        u_stats.scene_cut = cut_u;
        u_stats.hist_div_e4 = hist_e4;
        u_stats.mean_diff_e4 = diff_e4;
        u_stats.quality_fallback = 0u;
        if (u_reset != 0) {
            u_stats.total_cuts = cut_u;
            u_stats.total_quality_fallbacks = 0u;
        } else if (cut) {
            u_stats.total_cuts += 1u;
        }
    }
}
