#version 430 core
// AMD FidelityFX Optical Flow — C1-Continuous B-Spline Two-Layer Midpoint (t = 0.5) Synthesis.
//
// Eradicates 8x8 blockiness mathematically via four C1-continuous mechanisms:
// 1. Quadratic B-Spline (3x3 partition of unity, zero value & derivative at |d| = 1.5 blocks)
//    replaces 2x2 cell-snapped corners so motion vectors, variance (`std_v`), and layer clusters
//    NEVER jump along 8-pixel gridlines.
// 2. Exact-timestamp endpoint occlusion checks (`u_fwd_flow(p0)` at t = 0.0 and `-u_bwd_flow(p1)`
//    at t = 1.0) combined with the Gaussian-blurred `u_mid_aux` matte so moving midground objects
//    (parallax chairs/floors/props) are never falsely flagged as doubly occluded.
// 3. Soft two-layer color-space compositing (`mix(c_bg, c_fg, fg_alpha)`) inside the curved B-spline
//    silhouette band (`std_v > 2.5 px`) with exact boundary fade (`fg_alpha -> fg_spatial` at 0 & 1):
//    - Stationary/slow foregrounds (character heads/hair) snap smoothly to exact hair strands.
//    - Moving midgrounds (parallax chairs) separate by symmetric warp error instead of unwarped sky.
// 4. Clean one-sided disocclusions suppress local `prev+curr` fallback completely (`1 - one_sided`),
//    preventing ghostly unwarped double-images while retaining soft fallback for truly broken pixels.

layout(local_size_x = 16, local_size_y = 16, local_size_z = 1) in;

layout(binding = 0) uniform sampler2D u_prev_color;
layout(binding = 1) uniform sampler2D u_curr_color;
layout(binding = 2) uniform isampler2D u_fwd_flow;
layout(binding = 3) uniform isampler2D u_bwd_flow;
layout(binding = 4) uniform isampler2D u_mid_flow;
layout(binding = 5) uniform usampler2D u_scd_tex;
layout(binding = 6) uniform sampler2D u_mid_aux; // Gaussian-blurred RGBA8: (occ_prev, occ_curr, fg_mask, conf)
layout(binding = 7) uniform sampler2D u_hud_mask; // Precomputed full-res R8 stationary HUD/reticle mask

layout(rgba16f, binding = 0) writeonly uniform image2D u_out_interp;

layout(std430, binding = 0) readonly buffer ScdTelemetry {
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

uniform ivec2 u_frame_size;
uniform ivec2 u_grid_size;
uniform float u_artifact_guard;
uniform float u_hud_protection;

vec3 to_perceptual(vec3 lin) {
    return sqrt(max(lin, vec3(0.0)));
}

float color_err(vec3 c0, vec3 c1) {
    vec3 d = abs(to_perceptual(c0) - to_perceptual(c1));
    return dot(d, vec3(0.299, 0.587, 0.114)) + 0.30 * max(max(d.r, d.g), d.b);
}

// C1-continuous Quadratic B-Spline basis on [-1.5, +1.5]:
// sum_{k=-1}^{1} b2_weight(t - k) == 1.0 identically, with zero value & derivative at |t| = 1.5!
float b2_weight(float t) {
    float a = abs(t);
    if (a < 0.5) {
        return 0.75 - a * a;
    }
    if (a < 1.5) {
        float u = 1.5 - a;
        return 0.5 * u * u;
    }
    return 0.0;
}

vec2 sample_flow_bilin(isampler2D flow_tex, vec2 p, ivec2 max_g) {
    vec2 bf = p / 8.0 - 0.5;
    ivec2 b0 = ivec2(floor(bf));
    vec2 f = clamp(bf - vec2(b0), 0.0, 1.0);
    vec2 v00 = vec2(texelFetch(flow_tex, clamp(b0 + ivec2(0, 0), ivec2(0), max_g), 0).xy);
    vec2 v10 = vec2(texelFetch(flow_tex, clamp(b0 + ivec2(1, 0), ivec2(0), max_g), 0).xy);
    vec2 v01 = vec2(texelFetch(flow_tex, clamp(b0 + ivec2(0, 1), ivec2(0), max_g), 0).xy);
    vec2 v11 = vec2(texelFetch(flow_tex, clamp(b0 + ivec2(1, 1), ivec2(0), max_g), 0).xy);
    return mix(mix(v00, v10, f.x), mix(v01, v11, f.x), f.y) * 0.25;
}

vec4 sample_aux_bilin(vec2 p) {
    vec2 gsize = vec2(max(u_grid_size, ivec2(1)));
    vec2 half_g = 0.5 / gsize;
    vec2 uv = clamp((p / 8.0) / gsize, half_g, 1.0 - half_g);
    return texture(u_mid_aux, uv);
}

float perceptual_luma(vec3 lin) {
    return dot(to_perceptual(lin), vec3(0.299, 0.587, 0.114));
}

// Fast 1-tap lookup into the precomputed full-resolution R8 stationary HUD/reticle mask
// (`cs_ffx_hud_mask.glsl`), gated by `length(mv_ref)` so slow/static layers never trigger HUD lock.
float thin_stationary_hud_weight(vec2 q, vec2 mv_ref, vec2 inv_size, vec2 half_texel) {
    float speed_w = smoothstep(4.0, 10.0, length(mv_ref));
    if (speed_w <= 0.001) {
        return 0.0;
    }
    return speed_w * texture(u_hud_mask, clamp(q * inv_size, half_texel, 1.0 - half_texel)).r;
}

float eval_pair_match_5tap(vec2 p_prev, vec2 p_curr, vec2 inv_size, vec2 half_texel) {
    vec2 offsets[5];
    offsets[0] = vec2( 0.0,  0.0);
    offsets[1] = vec2(-1.5,  0.0);
    offsets[2] = vec2( 1.5,  0.0);
    offsets[3] = vec2( 0.0, -1.5);
    offsets[4] = vec2( 0.0,  1.5);

    float err_sum = 0.0;
    for (int i = 0; i < 5; ++i) {
        vec2 uv0 = clamp((p_prev + offsets[i]) * inv_size, half_texel, 1.0 - half_texel);
        vec2 uv1 = clamp((p_curr + offsets[i]) * inv_size, half_texel, 1.0 - half_texel);
        err_sum += color_err(texture(u_prev_color, uv0).rgb, texture(u_curr_color, uv1).rgb);
    }
    return err_sum * 0.2;
}

float eval_patch_warp_err_with_center(
    vec2 p,
    vec2 mv_px,
    float center_err,
    vec2 inv_size,
    vec2 half_texel
) {
    vec2 offsets[4];
    offsets[0] = vec2(-1.5,  0.0);
    offsets[1] = vec2( 1.5,  0.0);
    offsets[2] = vec2( 0.0, -1.5);
    offsets[3] = vec2( 0.0,  1.5);

    float err_sum = center_err;
    for (int i = 0; i < 4; ++i) {
        vec2 q = p + offsets[i];
        vec2 uv0 = clamp((q - 0.5 * mv_px) * inv_size, half_texel, 1.0 - half_texel);
        vec2 uv1 = clamp((q + 0.5 * mv_px) * inv_size, half_texel, 1.0 - half_texel);
        err_sum += color_err(texture(u_prev_color, uv0).rgb, texture(u_curr_color, uv1).rgb);
    }
    return err_sum * 0.2;
}

float eval_wide_warp_err(vec2 p, vec2 mv_px, vec2 inv_size, vec2 half_texel) {
    vec2 offsets[4];
    offsets[0] = vec2(-4.0,  0.0);
    offsets[1] = vec2( 4.0,  0.0);
    offsets[2] = vec2( 0.0, -4.0);
    offsets[3] = vec2( 0.0,  4.0);
    float err_sum = 0.0;
    for (int i = 0; i < 4; ++i) {
        vec2 q = p + offsets[i];
        vec2 uv0 = clamp((q - 0.5 * mv_px) * inv_size, half_texel, 1.0 - half_texel);
        vec2 uv1 = clamp((q + 0.5 * mv_px) * inv_size, half_texel, 1.0 - half_texel);
        err_sum += color_err(texture(u_prev_color, uv0).rgb, texture(u_curr_color, uv1).rgb);
    }
    return err_sum * 0.25;
}

// Edge-aware trimmed 5-tap warp error: combines center pixel with the 2 cleanest neighbors
// (dropping up to 2 straddling silhouette taps) for crisp anti-aliased foreground/background edges.
float eval_trimmed_warp_err(vec2 p, vec2 mv_px, vec2 inv_size, vec2 half_texel) {
    vec2 uv0_c = clamp((p - 0.5 * mv_px) * inv_size, half_texel, 1.0 - half_texel);
    vec2 uv1_c = clamp((p + 0.5 * mv_px) * inv_size, half_texel, 1.0 - half_texel);
    float e_c = color_err(texture(u_prev_color, uv0_c).rgb, texture(u_curr_color, uv1_c).rgb);

    vec2 offsets[4];
    offsets[0] = vec2(-1.25,  0.0);
    offsets[1] = vec2( 1.25,  0.0);
    offsets[2] = vec2( 0.0, -1.25);
    offsets[3] = vec2( 0.0,  1.25);

    float m1 = 1e9;
    float m2 = 1e9;
    for (int i = 0; i < 4; ++i) {
        vec2 q = p + offsets[i];
        vec2 uv0 = clamp((q - 0.5 * mv_px) * inv_size, half_texel, 1.0 - half_texel);
        vec2 uv1 = clamp((q + 0.5 * mv_px) * inv_size, half_texel, 1.0 - half_texel);
        float e = color_err(texture(u_prev_color, uv0).rgb, texture(u_curr_color, uv1).rgb);
        if (e < m1) {
            m2 = m1;
            m1 = e;
        } else if (e < m2) {
            m2 = e;
        }
    }
    return 0.55 * e_c + 0.225 * (m1 + m2);
}

// C1-continuous Quadratic B-Spline, per-block cycle-consistency-validated endpoint occluder
// detector at `p0` (t = 0.0). Evaluates exact un-averaged `u_fwd_flow` blocks BEFORE FB validation
// and convolves with the C1 B-spline kernel (zero circular max-disk polka dots, smooth rim coverage!).
// Also outputs `out_fb_bad` when `p0` fails cycle consistency (e.g. popping UI banners or broken
// non-rigid swinging limbs) without being a valid cycle-consistent occluder.
float sample_endpoint_occ_prev(
    vec2 p0,
    vec2 mv,
    vec2 v_global_px,
    float diff_thresh,
    float dilate_gain,
    ivec2 max_g,
    out float out_fb_bad
) {
    vec2 bf = p0 / 8.0 - 0.5;
    ivec2 bc = ivec2(round(bf));
    float mv_fg_dist = length(mv - v_global_px);
    float occ_sum = 0.0;
    float fb_bad_sum = 0.0;

    for (int dy = -1; dy <= 1; ++dy) {
        float wy = b2_weight(float(bc.y + dy) - bf.y);
        int cy = clamp(bc.y + dy, 0, max_g.y);
        for (int dx = -1; dx <= 1; ++dx) {
            float wx = b2_weight(float(bc.x + dx) - bf.x);
            float w = wx * wy;
            if (w <= 1e-5) {
                continue;
            }
            ivec2 nc = ivec2(clamp(bc.x + dx, 0, max_g.x), cy);
            vec2 delta = vec2(bc.x + dx, bc.y + dy) - bf;
            bool is_center = (dot(delta, delta) < 0.65);

            vec2 vf = vec2(texelFetch(u_fwd_flow, nc, 0).xy) * 0.25;
            float dev = length(vf - mv);
            float occ_block = 0.0;
            float fb_bad_block = 0.0;

            vec2 nc_curr = vec2(nc) + vf / 8.0;
            bool in_dst = nc_curr.x >= -0.5 && nc_curr.y >= -0.5
                && nc_curr.x <= float(max_g.x) + 0.5
                && nc_curr.y <= float(max_g.y) + 0.5;
            float nv = 0.0;
            float nfb = 1e4;
            if (in_dst) {
                ivec2 bc2 = clamp(ivec2(round(nc_curr)), ivec2(0), max_g);
                vec2 vb = vec2(texelFetch(u_bwd_flow, bc2, 0).xy) * 0.25;
                nfb = length(vf + vb);
                float nfb_t = 5.5 + 0.14 * length(vf);
                nv = smoothstep(nfb_t * 1.45, nfb_t * 0.55, nfb);
            }

            if (dev > diff_thresh * 0.55) {
                bool is_closer_fg = (mv_fg_dist < 28.0) || (length(vf - v_global_px) > mv_fg_dist + 8.0);
                if (is_center || is_closer_fg) {
                    occ_block = smoothstep(diff_thresh * 0.55, diff_thresh * 1.25, dev) * nv;
                }
            }
            fb_bad_block = (1.0 - nv) * smoothstep(diff_thresh * 0.40, diff_thresh * 1.05, max(dev, nfb));

            // Solid backup for stationary/slow foregrounds (e.g. cap/hair with moving shadows
            // that temporarily break raw fwd/bwd FB check): ONLY overrides when raw `u_fwd_flow`
            // did not already confirm valid cycle-consistent matching motion (so stale `(0, 0)`
            // reticle bleed in `u_mid_flow` never falsely occludes an open moving road!).
            float raw_agrees = nv * smoothstep(diff_thresh * 0.85, diff_thresh * 0.35, dev);
            if (raw_agrees < 0.95) {
                vec2 mid_v = vec2(texelFetch(u_mid_flow, nc, 0).xy) * 0.25;
                if (length(mid_v) < 16.0 && length(mid_v - v_global_px) > mv_fg_dist + 16.0) {
                    float mdev = length(mid_v - mv);
                    if (mdev > diff_thresh * 0.55) {
                        float occ_m = smoothstep(diff_thresh * 0.55, diff_thresh * 1.25, mdev) * (1.0 - raw_agrees);
                        occ_block = max(occ_block, occ_m);
                    }
                }
            }

            occ_sum += occ_block * w;
            fb_bad_sum += (1.0 - occ_block) * fb_bad_block * w;
        }
    }
    out_fb_bad = clamp(fb_bad_sum, 0.0, 1.0);
    return clamp(occ_sum * mix(1.30, 2.50, dilate_gain), 0.0, 1.0);
}

// Symmetric C1-continuous B-Spline endpoint occluder detector at `p1` (t = 1.0) using `u_bwd_flow`.
float sample_endpoint_occ_curr(
    vec2 p1,
    vec2 mv,
    vec2 v_global_px,
    float diff_thresh,
    float dilate_gain,
    ivec2 max_g,
    out float out_fb_bad
) {
    vec2 bf = p1 / 8.0 - 0.5;
    ivec2 bc = ivec2(round(bf));
    float mv_fg_dist = length(mv - v_global_px);
    float occ_sum = 0.0;
    float fb_bad_sum = 0.0;

    for (int dy = -1; dy <= 1; ++dy) {
        float wy = b2_weight(float(bc.y + dy) - bf.y);
        int cy = clamp(bc.y + dy, 0, max_g.y);
        for (int dx = -1; dx <= 1; ++dx) {
            float wx = b2_weight(float(bc.x + dx) - bf.x);
            float w = wx * wy;
            if (w <= 1e-5) {
                continue;
            }
            ivec2 nc = ivec2(clamp(bc.x + dx, 0, max_g.x), cy);
            vec2 delta = vec2(bc.x + dx, bc.y + dy) - bf;
            bool is_center = (dot(delta, delta) < 0.65);

            vec2 vb = vec2(texelFetch(u_bwd_flow, nc, 0).xy) * 0.25;
            vec2 nm = -vb;
            float dev = length(nm - mv);
            float occ_block = 0.0;
            float fb_bad_block = 0.0;

            vec2 nc_prev = vec2(nc) + vb / 8.0;
            bool in_dst = nc_prev.x >= -0.5 && nc_prev.y >= -0.5
                && nc_prev.x <= float(max_g.x) + 0.5
                && nc_prev.y <= float(max_g.y) + 0.5;
            float nv = 0.0;
            float nfb = 1e4;
            if (in_dst) {
                ivec2 bp2 = clamp(ivec2(round(nc_prev)), ivec2(0), max_g);
                vec2 vf = vec2(texelFetch(u_fwd_flow, bp2, 0).xy) * 0.25;
                nfb = length(vb + vf);
                float nfb_t = 5.5 + 0.14 * length(vb);
                nv = smoothstep(nfb_t * 1.45, nfb_t * 0.55, nfb);
            }

            if (dev > diff_thresh * 0.55) {
                bool is_closer_fg = (mv_fg_dist < 28.0) || (length(nm - v_global_px) > mv_fg_dist + 8.0);
                if (is_center || is_closer_fg) {
                    occ_block = smoothstep(diff_thresh * 0.55, diff_thresh * 1.25, dev) * nv;
                }
            }
            fb_bad_block = (1.0 - nv) * smoothstep(diff_thresh * 0.40, diff_thresh * 1.05, max(dev, nfb));

            float raw_agrees = nv * smoothstep(diff_thresh * 0.85, diff_thresh * 0.35, dev);
            if (raw_agrees < 0.95) {
                vec2 mid_v = vec2(texelFetch(u_mid_flow, nc, 0).xy) * 0.25;
                if (length(mid_v) < 16.0 && length(mid_v - v_global_px) > mv_fg_dist + 16.0) {
                    float mdev = length(mid_v - mv);
                    if (mdev > diff_thresh * 0.55) {
                        float occ_m = smoothstep(diff_thresh * 0.55, diff_thresh * 1.25, mdev) * (1.0 - raw_agrees);
                        occ_block = max(occ_block, occ_m);
                    }
                }
            }

            occ_sum += occ_block * w;
            fb_bad_sum += (1.0 - occ_block) * fb_bad_block * w;
        }
    }
    out_fb_bad = clamp(fb_bad_sum, 0.0, 1.0);
    return clamp(occ_sum * mix(1.30, 2.50, dilate_gain), 0.0, 1.0);
}

// Smoothly warps a single motion layer `mv` with exact-timestamp endpoint occlusion checks
// (`u_fwd_flow(p0)` at t = 0.0 and `-u_bwd_flow(p1)` at t = 1.0) and smooth 16px OOB fades.
vec3 warp_layer(
    vec2 p,
    vec2 mv,
    vec2 v_global_px,
    bool is_fg_layer,
    float aux_occ_p,
    float aux_occ_c,
    float aux_conf,
    ivec2 max_g,
    vec2 inv_size,
    vec2 half_texel,
    out float out_patch_err,
    out float out_both_occ,
    out float out_one_sided,
    out float out_oob_hud,
    out float out_ep_broken
) {
    vec2 p0 = p - 0.5 * mv;
    vec2 p1 = p + 0.5 * mv;
    vec2 uv0 = p0 * inv_size;
    vec2 uv1 = p1 * inv_size;

    // Smooth 16px OOB border fades (eliminates hard vertical screen-edge seams).
    vec2 margin_uv = 16.0 * inv_size;
    float in0 = min(
        min(smoothstep(0.0, margin_uv.x, uv0.x), smoothstep(0.0, margin_uv.x, 1.0 - uv0.x)),
        min(smoothstep(0.0, margin_uv.y, uv0.y), smoothstep(0.0, margin_uv.y, 1.0 - uv0.y))
    );
    float in1 = min(
        min(smoothstep(0.0, margin_uv.x, uv1.x), smoothstep(0.0, margin_uv.x, 1.0 - uv1.x)),
        min(smoothstep(0.0, margin_uv.y, uv1.y), smoothstep(0.0, margin_uv.y, 1.0 - uv1.y))
    );

    vec3 c0 = texture(u_prev_color, clamp(uv0, half_texel, 1.0 - half_texel)).rgb;
    vec3 c1 = texture(u_curr_color, clamp(uv1, half_texel, 1.0 - half_texel)).rgb;

    float pix_err = color_err(c0, c1);
    float patch_err = (pix_err > 0.025)
        ? eval_patch_warp_err_with_center(p, mv, pix_err, inv_size, half_texel)
        : pix_err;
    out_patch_err = patch_err;
    out_both_occ = 0.0;
    out_one_sided = 0.0;
    out_oob_hud = 0.0;
    out_ep_broken = 0.0;

    if (is_fg_layer) {
        // Closest stationary/slow foreground character sits in front of everything — never occluded!
        float w0 = in0;
        float w1 = in1;
        float wsum = w0 + w1;
        float blend = (wsum > 1e-3) ? (w1 / wsum) : 0.5;
        return mix(c0, c1, blend);
    }

    float mv_speed = length(mv);
    float diff_thresh = 6.0 + 0.16 * mv_speed;
    float dilate_gain = smoothstep(10.0, 32.0, mv_speed);

    float occ_p = aux_occ_p;
    float occ_c = aux_occ_c;
    if (patch_err > 0.015 || aux_conf < 0.60 || in0 < 0.99 || in1 < 0.99 || aux_occ_p > 0.02 || aux_occ_c > 0.02) {
        float fb_bad_p = 0.0;
        float fb_bad_c = 0.0;
        float exact_occ_p = sample_endpoint_occ_prev(
            p0, mv, v_global_px, diff_thresh, dilate_gain, max_g, fb_bad_p
        );
        float exact_occ_c = sample_endpoint_occ_curr(
            p1, mv, v_global_px, diff_thresh, dilate_gain, max_g, fb_bad_c
        );

        float hud_p = thin_stationary_hud_weight(p0, mv, inv_size, half_texel);
        float hud_c = thin_stationary_hud_weight(p1, mv, inv_size, half_texel);
        // Tight +/-1.75px anti-aliased fringe guard on high-contrast warps so 1-2px anti-aliased
        // outlines of thin cursor reticles / outlined HUD text never leave ghost fringes on the map!
        if (patch_err > 0.050 && hud_p < 0.90) {
            float hp_n = max(
                max(
                    thin_stationary_hud_weight(p0 + vec2(-1.75, 0.0), mv, inv_size, half_texel),
                    thin_stationary_hud_weight(p0 + vec2( 1.75, 0.0), mv, inv_size, half_texel)
                ),
                max(
                    thin_stationary_hud_weight(p0 + vec2(0.0, -1.75), mv, inv_size, half_texel),
                    thin_stationary_hud_weight(p0 + vec2(0.0,  1.75), mv, inv_size, half_texel)
                )
            );
            hud_p = max(hud_p, hp_n * 0.95);
        }
        if (patch_err > 0.050 && hud_c < 0.90) {
            float hc_n = max(
                max(
                    thin_stationary_hud_weight(p1 + vec2(-1.75, 0.0), mv, inv_size, half_texel),
                    thin_stationary_hud_weight(p1 + vec2( 1.75, 0.0), mv, inv_size, half_texel)
                ),
                max(
                    thin_stationary_hud_weight(p1 + vec2(0.0, -1.75), mv, inv_size, half_texel),
                    thin_stationary_hud_weight(p1 + vec2(0.0,  1.75), mv, inv_size, half_texel)
                )
            );
            hud_c = max(hud_c, hc_n * 0.95);
        }
        if (in0 < 0.99) {
            hud_c = max(hud_c, max(
                thin_stationary_hud_weight(p1 + vec2(-5.0, 0.0), mv, inv_size, half_texel),
                thin_stationary_hud_weight(p1 + vec2( 5.0, 0.0), mv, inv_size, half_texel)
            ));
        }
        if (in1 < 0.99) {
            hud_p = max(hud_p, max(
                thin_stationary_hud_weight(p0 + vec2(-5.0, 0.0), mv, inv_size, half_texel),
                thin_stationary_hud_weight(p0 + vec2( 5.0, 0.0), mv, inv_size, half_texel)
            ));
        }

        occ_p = clamp(max(max(occ_p * 0.30, exact_occ_p), hud_p), 0.0, 1.0);
        occ_c = clamp(max(max(occ_c * 0.30, exact_occ_c), hud_c), 0.0, 1.0);

        // Guard screen borders so OOB warp never clones stationary HUD (`$4,326`) or a popping
        // bottom/edge UI banner (`"Butternut Rd"`) into the screen margin.
        out_oob_hud = clamp(
            (1.0 - in0) * max(occ_c, fb_bad_c) + (1.0 - in1) * max(occ_p, fb_bad_p),
            0.0,
            1.0
        );
        out_both_occ = min(occ_p, occ_c);
        out_one_sided = abs(occ_p - occ_c);
        out_ep_broken = max(fb_bad_p, fb_bad_c);

        // Thin-HUD double-occlusion local bypass (runs ONLY on the tiny <0.02% of pixels where
        // BOTH warp endpoints are occluded and at least one — typically BOTH on a circular reticle
        // chord! — landed directly on a thin ~3-4px stationary HUD stroke): shifts `(p0, p1)`
        // together across an 8-direction +/-6px ring to step cleanly off perpendicular reticle arcs
        // onto matching HUD-free background (grass OR moving road) instead of dropping a
        // `0.5*(prev+curr)` double-image ghost blob inside the hollow reticle!
        float hud_hit_max = max(hud_p, hud_c);
        if (out_both_occ > 0.35 && hud_hit_max > 0.35 && in0 > 0.95 && in1 > 0.95) {
            vec2 b_offs[8];
            b_offs[0] = vec2( 5.0,  0.0);
            b_offs[1] = vec2(-5.0,  0.0);
            b_offs[2] = vec2( 0.0,  5.0);
            b_offs[3] = vec2( 0.0, -5.0);
            b_offs[4] = vec2( 4.0,  4.0);
            b_offs[5] = vec2(-4.0, -4.0);
            b_offs[6] = vec2( 4.0, -4.0);
            b_offs[7] = vec2(-4.0,  4.0);

            vec3 byp_rgb_sum = vec3(0.0);
            float byp_err_sum = 0.0;
            float byp_w_sum = 0.0;
            for (int k = 0; k < 8; ++k) {
                vec2 q0 = p0 + b_offs[k];
                vec2 q1 = p1 + b_offs[k];
                float h0_k = thin_stationary_hud_weight(q0, mv, inv_size, half_texel);
                float h1_k = thin_stationary_hud_weight(q1, mv, inv_size, half_texel);
                float free_k = (1.0 - h0_k) * (1.0 - h1_k);
                if (free_k <= 0.15) {
                    continue;
                }
                vec3 s0 = texture(u_prev_color, clamp(q0 * inv_size, half_texel, 1.0 - half_texel)).rgb;
                vec3 s1 = texture(u_curr_color, clamp(q1 * inv_size, half_texel, 1.0 - half_texel)).rgb;
                float ek = color_err(s0, s1);
                float wk = free_k * exp(-ek * 18.0);
                byp_rgb_sum += 0.5 * (s0 + s1) * wk;
                byp_err_sum += ek * wk;
                byp_w_sum += wk;
            }
            if (byp_w_sum > 0.20) {
                vec3 c_byp = byp_rgb_sum / byp_w_sum;
                float e_byp = byp_err_sum / byp_w_sum;
                float byp_w = smoothstep(0.35, 0.70, out_both_occ)
                    * smoothstep(0.35, 0.70, hud_hit_max)
                    * smoothstep(0.14, 0.045, e_byp);
                if (byp_w > 0.01) {
                    float vis_s = smoothstep(0.012, 0.085, max(patch_err, max(max(occ_p, occ_c), 1.0 - aux_conf)));
                    float w0_b = in0 * (1.0 - 0.999 * occ_p * vis_s);
                    float w1_b = in1 * (1.0 - 0.999 * occ_c * vis_s);
                    float b_sum = w0_b + w1_b;
                    vec3 base_rgb = mix(c0, c1, (b_sum > 1e-3) ? (w1_b / b_sum) : 0.5);
                    out_both_occ *= (1.0 - byp_w);
                    out_patch_err = mix(out_patch_err, e_byp, byp_w);
                    out_ep_broken *= (1.0 - byp_w);
                    return mix(base_rgb, c_byp, byp_w);
                }
            }
        }
    }

    float vis_strength = smoothstep(0.012, 0.085, max(patch_err, max(max(occ_p, occ_c), 1.0 - aux_conf)));
    float w0 = in0 * (1.0 - 0.999 * occ_p * vis_strength);
    float w1 = in1 * (1.0 - 0.999 * occ_c * vis_strength);
    float wsum = w0 + w1;
    float blend = (wsum > 1e-3) ? (w1 / wsum) : 0.5;
    return mix(c0, c1, blend);
}

void main() {
    ivec2 gid = ivec2(gl_GlobalInvocationID.xy);
    if (gid.x >= u_frame_size.x || gid.y >= u_frame_size.y) {
        return;
    }

    vec3 curr_unwarped = texelFetch(u_curr_color, gid, 0).rgb;

    // Whole-frame Scene Change Detection / AFMF Quality Gate fallback: crisp real frame on cuts/whips.
    if (texelFetch(u_scd_tex, ivec2(0, 0), 0).r != 0u) {
        imageStore(u_out_interp, gid, vec4(max(curr_unwarped, vec3(0.0)), 1.0));
        return;
    }

    vec3 prev_unwarped = texelFetch(u_prev_color, gid, 0).rgb;
    vec2 frame_f = vec2(max(u_frame_size, ivec2(1)));
    vec2 inv_size = 1.0 / frame_f;
    vec2 half_texel = 0.5 * inv_size;
    vec2 p = vec2(gid) + 0.5;
    ivec2 max_g = max(u_grid_size - ivec2(1), ivec2(0));

    vec2 v_global_px = vec2(float(u_stats.global_mv_x_q), float(u_stats.global_mv_y_q)) * 0.25;
    float global_speed = length(v_global_px);

    // Fetch 3x3 Quadratic B-Spline neighborhood of smoothed midpoint flow (`u_mid_flow`).
    // Because `b2_weight(d)` hits 0 with zero slope at |d| = 1.5 blocks, every derived quantity
    // (`v_smooth`, `std_v`, `v_fg`, `v_bg`, `fg_spatial`) is C1-continuous with ZERO 8x8 seams!
    vec2 bf = p / 8.0 - 0.5;
    ivec2 bc = ivec2(round(bf));
    vec2 vs[9];
    float ws[9];
    vec2 v_smooth = vec2(0.0);
    float mean_spd = 0.0;
    int idx = 0;
    for (int dy = -1; dy <= 1; ++dy) {
        float wy = b2_weight(float(bc.y + dy) - bf.y);
        int cy = clamp(bc.y + dy, 0, max_g.y);
        for (int dx = -1; dx <= 1; ++dx) {
            float wx = b2_weight(float(bc.x + dx) - bf.x);
            int cx = clamp(bc.x + dx, 0, max_g.x);
            float w = wx * wy;
            vec2 v = vec2(texelFetch(u_mid_flow, ivec2(cx, cy), 0).xy) * 0.25;
            vs[idx] = v;
            ws[idx] = w;
            v_smooth += v * w;
            mean_spd += length(v) * w;
            idx += 1;
        }
    }

    float var_v = 0.0;
    for (int i = 0; i < 9; ++i) {
        vec2 dv = vs[i] - v_smooth;
        var_v += ws[i] * dot(dv, dv);
    }
    float std_v = sqrt(var_v);

    vec4 aux = sample_aux_bilin(p);
    float aux_occ_p = aux.r;
    float aux_occ_c = aux.g;
    float aux_fg = aux.b;
    float aux_conf = aux.a;

    float hud_prot = clamp(u_hud_protection, 0.0, 1.0);
    float guard = clamp(u_artifact_guard, 0.0, 1.0);

    // Smooth continuous stroke-aware HUD protection (NEVER scales `mv`, NEVER uses nearest-block!).
    // Evaluates BOTH our high-pass semi-transparent / 4:2:0-tolerant thin-HUD detector (catches
    // semi-transparent cursor reticle outlines & thin white text crossing bright white roads!)
    // AND trimmed-ring opaque static edge detection (catches solid dark bars & badges).
    float hud_lock_w = 0.0;
    if (hud_prot > 0.01 && (dot(v_smooth, v_smooth) > 0.25 || global_speed > 6.0)) {
        float prot_gain = smoothstep(0.0, 0.30, hud_prot);
        vec2 mv_check = (length(v_smooth) > 4.0) ? v_smooth : v_global_px;
        float thin_hud = thin_stationary_hud_weight(p, mv_check, inv_size, half_texel);
        hud_lock_w = thin_hud * prot_gain;

        float e_c = color_err(prev_unwarped, curr_unwarped);
        float static_thresh = mix(0.012, 0.044, hud_prot);
        if (hud_lock_w < 0.99 && e_c < static_thresh * 1.25) {
            vec2 ring[8];
            ring[0] = vec2(-1.5,  0.0);
            ring[1] = vec2( 1.5,  0.0);
            ring[2] = vec2( 0.0, -1.5);
            ring[3] = vec2( 0.0,  1.5);
            ring[4] = vec2(-1.1, -1.1);
            ring[5] = vec2( 1.1, -1.1);
            ring[6] = vec2(-1.1,  1.1);
            ring[7] = vec2( 1.1,  1.1);

            float lum_p = perceptual_luma(prev_unwarped);
            float lum_c = perceptual_luma(curr_unwarped);
            float lp_min = lum_p;
            float lp_max = lum_p;
            float lc_min = lum_c;
            float lc_max = lum_c;
            float m1 = 1e9;
            float m2 = 1e9;
            for (int i = 0; i < 8; ++i) {
                vec2 uv = clamp((p + ring[i]) * inv_size, half_texel, 1.0 - half_texel);
                vec3 cp = texture(u_prev_color, uv).rgb;
                vec3 cc = texture(u_curr_color, uv).rgb;
                float e = color_err(cp, cc);
                if (e < m1) {
                    m2 = m1;
                    m1 = e;
                } else if (e < m2) {
                    m2 = e;
                }
                float l_p = perceptual_luma(cp);
                float l_c = perceptual_luma(cc);
                lp_min = min(lp_min, l_p);
                lp_max = max(lp_max, l_p);
                lc_min = min(lc_min, l_c);
                lc_max = max(lc_max, l_c);
            }
            float static_err = 0.50 * e_c + 0.25 * (m1 + m2);
            // Require edge contrast in BOTH `prev` and `curr` (prevents moving roads passing
            // 1.5px beside flat grass in `curr` from faking a static HUD edge!).
            float edge_contrast = min(lp_max - lp_min, lc_max - lc_min);
            if (static_err < static_thresh) {
                vec2 vf_p = sample_flow_bilin(u_fwd_flow, p, max_g);
                vec2 vb_p = sample_flow_bilin(u_bwd_flow, p, max_g);
                float edge_w = smoothstep(0.020, 0.055, edge_contrast);
                // Gate raw `(0, 0)` `still_w` by `edge_w` whenever `v_smooth` is fast so raw
                // `(0, 0)` reticle-arc flow blocks sealed out of `u_mid_flow` NEVER lock flat
                // green map pixels inside the hollow reticle and carve holes out of moving roads!
                float still_w = smoothstep(5.5, 2.0, max(length(vf_p), length(vb_p)))
                    * max(edge_w, smoothstep(8.0, 2.0, length(v_smooth)));
                float warp_err = (edge_w > 0.01 && still_w < 0.99)
                    ? eval_trimmed_warp_err(p, mv_check, inv_size, half_texel)
                    : 0.0;
                float warp_mismatch = edge_w * smoothstep(
                    static_err + mix(0.055, 0.015, hud_prot),
                    static_err + mix(0.110, 0.040, hud_prot),
                    warp_err
                );
                float reason_w = max(still_w, warp_mismatch);
                float opaque_lock = smoothstep(static_thresh, static_thresh * 0.25, static_err) * prot_gain * reason_w;
                hud_lock_w = max(hud_lock_w, opaque_lock);
            }
        }
    }

    if (hud_lock_w >= 0.99) {
        vec3 hud_rgb = 0.5 * (prev_unwarped + curr_unwarped);
        imageStore(u_out_interp, gid, vec4(max(hud_rgb, vec3(0.0)), 1.0));
        return;
    }

    vec3 out_rgb;

    if (std_v <= 2.5) {
        // Regime A: Smooth continuous motion field (away from motion boundaries).
        float patch_err, both_occ, one_sided, oob_hud, ep_broken;
        vec3 warped_rgb = warp_layer(
            p, v_smooth, v_global_px, false, aux_occ_p, aux_occ_c, aux_conf,
            max_g, inv_size, half_texel, patch_err, both_occ, one_sided, oob_hud, ep_broken
        );
        out_rgb = mix(warped_rgb, curr_unwarped, oob_hud);

        // Static-block moving-background peek-through rescue (e.g. tilted 3D map roads flowing
        // between stacked HUD labels like `RECENTER` / `2D VIEW` / `ADD MARKER` where 8x8 blocks
        // merged to `(0, 0)`): requires a genuine high-contrast moving feature (`patch_err > 0.095`,
        // never flat dark translucent header bars or flat grass!), proves pixel `p` moves at
        // `v_local_bg`, and strictly validates `c_glob` against double-occlusion / OOB / high warp
        // error so stationary top-bar text (`JASON & LUCIA`, `PROGRESS`, `MAP`) or reticle arcs
        // can NEVER be cloned into adjacent empty bar/grass pixels!
        if (length(v_smooth) < 6.0 && global_speed > 10.0 && patch_err > 0.095 && oob_hud < 0.90) {
            vec2 bg_dir = v_global_px / max(global_speed, 1e-4);
            vec2 v_local_bg = v_global_px;
            vec2 probe_sum = vec2(0.0);
            float probe_w = 0.0;
            ivec2 p_offs[12];
            p_offs[0]  = ivec2( -6,   0);
            p_offs[1]  = ivec2(  6,   0);
            p_offs[2]  = ivec2(  0,  -6);
            p_offs[3]  = ivec2(  0,   6);
            p_offs[4]  = ivec2(-12,   0);
            p_offs[5]  = ivec2( 12,   0);
            p_offs[6]  = ivec2(  0, -12);
            p_offs[7]  = ivec2(  0,  12);
            p_offs[8]  = ivec2(-18,  -6);
            p_offs[9]  = ivec2(-18,   6);
            p_offs[10] = ivec2(  0, -18);
            p_offs[11] = ivec2(-24,   0);
            for (int i = 0; i < 12; ++i) {
                ivec2 pc = clamp(bc + p_offs[i], ivec2(0), max_g);
                vec2 pv = vec2(texelFetch(u_mid_flow, pc, 0).xy) * 0.25;
                if (length(pv) > 8.0 && dot(pv, bg_dir) > 0.35 * global_speed) {
                    float w = 1.0 / (1.0 + length(vec2(p_offs[i])));
                    probe_sum += pv * w;
                    probe_w += w;
                }
            }
            if (probe_w > 1e-4) {
                v_local_bg = probe_sum / probe_w;
            }
            float e_fwd_bg = eval_pair_match_5tap(p, p + v_local_bg, inv_size, half_texel);
            float e_bwd_bg = eval_pair_match_5tap(p - v_local_bg, p, inv_size, half_texel);
            float e_bg_proof = min(e_fwd_bg, e_bwd_bg);
            float peek_w = smoothstep(0.065, 0.020, e_bg_proof)
                * smoothstep(patch_err * 0.55, patch_err * 0.22, e_bg_proof)
                * smoothstep(0.095, 0.16, patch_err);
            if (peek_w > 0.01) {
                float g_err, g_bocc, g_one, g_ohud, g_brk;
                vec3 c_glob = warp_layer(
                    p, v_local_bg, v_global_px, false, 0.0, 0.0, 0.5,
                    max_g, inv_size, half_texel, g_err, g_bocc, g_one, g_ohud, g_brk
                );
                float c_glob_self = min(
                    color_err(c_glob, prev_unwarped),
                    color_err(c_glob, curr_unwarped)
                );
                float glob_valid = (1.0 - smoothstep(0.15, 0.45, g_bocc))
                    * (1.0 - g_ohud)
                    * max(
                        smoothstep(0.16, 0.06, g_err),
                        smoothstep(0.55, 0.85, g_one) * smoothstep(0.18, 0.07, c_glob_self)
                    );
                float peek_applied = peek_w * glob_valid;
                if (peek_applied > 0.01) {
                    out_rgb = mix(out_rgb, c_glob, peek_applied);
                    patch_err = mix(patch_err, g_err, peek_applied);
                }
            }
        }

        // Soft local fallback ONLY when BOTH frames are genuinely broken/doubly-occluded or
        // an endpoint fails cycle consistency with huge warp error (e.g. popping UI banners like
        // `"Butternut Rd"` or broken non-rigid limbs). Multiplied by `not_one_sided` so clean
        // one-sided disocclusions around chairs, heads, and rims are NEVER overwritten!
        float not_one_sided = smoothstep(0.65, 0.15, one_sided);
        if (patch_err > 0.075 && oob_hud < 0.95 && not_one_sided > 0.01) {
            float wide_err = max(patch_err, eval_wide_warp_err(p, v_smooth, inv_size, half_texel));
            vec2 p0 = p - 0.5 * v_smooth;
            vec2 p1 = p + 0.5 * v_smooth;
            float err_f = length(sample_flow_bilin(u_fwd_flow, p0, max_g) - v_smooth);
            float err_b = length(sample_flow_bilin(u_bwd_flow, p1, max_g) + v_smooth);
            float flow_mismatch = min(err_f, err_b);
            float severe_photo = smoothstep(mix(0.36, 0.18, guard), mix(0.54, 0.30, guard), wide_err);
            float bad_signal = max(
                max(max(both_occ, ep_broken), severe_photo),
                smoothstep(mix(16.0, 8.0, guard), mix(30.0, 18.0, guard), flow_mismatch)
            );
            float photo_bad = smoothstep(mix(0.28, 0.13, guard), mix(0.48, 0.26, guard), wide_err);
            float fb_gain = smoothstep(0.0, 0.35, guard);
            float local_fb_w = clamp(photo_bad * bad_signal * not_one_sided * fb_gain, 0.0, 1.0);

            if (local_fb_w > 0.01) {
                vec3 soft_fallback = 0.5 * (prev_unwarped + curr_unwarped);
                if (aux_fg < 0.35 && length(v_smooth - v_global_px) > 12.0) {
                    float g_err, g_bocc, g_one, g_ohud, g_brk;
                    vec3 c_glob = warp_layer(
                        p, v_global_px, v_global_px, false, aux_occ_p, aux_occ_c, aux_conf,
                        max_g, inv_size, half_texel, g_err, g_bocc, g_one, g_ohud, g_brk
                    );
                    float glob_good = smoothstep(0.09, 0.03, g_err)
                        * smoothstep(patch_err * 0.70, patch_err * 0.35, g_err)
                        * (1.0 - g_brk);
                    soft_fallback = mix(soft_fallback, c_glob, glob_good);
                }
                out_rgb = mix(out_rgb, soft_fallback, local_fb_w);
            }
        }
    } else {
        // Regime B: C1-continuous B-Spline two-layer silhouette compositing (`std_v > 2.5 px`).
        // Split the 9 B-spline taps continuously into foreground (`v_fg`) and background (`v_bg`)
        // clusters with ZERO unweighted min/max seed snaps and ZERO 8x8 cell boundaries!
        vec2 ref_dir = (global_speed > 6.0) ? (v_global_px / global_speed) : vec2(1.0, 0.0);
        float use_proj = smoothstep(6.0, 14.0, global_speed);

        float proj_var = 0.0;
        float max_proj_g = -1e9;
        if (use_proj > 0.01) {
            for (int i = 0; i < 9; ++i) {
                float sp = dot(vs[i] - v_smooth, ref_dir);
                proj_var += ws[i] * sp * sp;
                max_proj_g = max(max_proj_g, dot(vs[i], ref_dir));
            }
            // Only project onto global sky direction `v_global_px` when at least one local layer
            // actually aligns with `v_global_px` (prevents opposite-direction perspective ground
            // near walking feet from being swapped into `v_fg`!).
            use_proj *= smoothstep(0.10, 0.35, proj_var / max(var_v, 1e-4))
                * smoothstep(0.30 * global_speed, 0.55 * global_speed, max_proj_g);
        }

        vec2 fg_sum = vec2(0.0);
        float fg_wsum = 0.0;
        vec2 bg_sum = vec2(0.0);
        float bg_wsum = 0.0;
        float split_band = max(0.35 * std_v, 1.0);
        for (int i = 0; i < 9; ++i) {
            float s_proj = dot(vs[i] - v_smooth, ref_dir);
            float s_spd = length(vs[i]) - mean_spd;
            float s_i = mix(s_spd, s_proj, use_proj);
            float bg_mem = smoothstep(-split_band, split_band, s_i);
            float fg_mem = 1.0 - bg_mem;
            fg_sum += vs[i] * (ws[i] * fg_mem);
            fg_wsum += ws[i] * fg_mem;
            bg_sum += vs[i] * (ws[i] * bg_mem);
            bg_wsum += ws[i] * bg_mem;
        }
        vec2 v_fg = (fg_wsum > 1e-4) ? (fg_sum / fg_wsum) : v_smooth;
        vec2 v_bg = (bg_wsum > 1e-4) ? (bg_sum / bg_wsum) : v_smooth;
        float fg_spatial = clamp(fg_wsum / max(fg_wsum + bg_wsum, 1e-4), 0.0, 1.0);

        // Closest stationary/slow foreground character (`length(v_fg) < 28` and slower than `v_bg`
        // or far from `v_global`) is never occluded; moving midground layers (e.g. parallax chair)
        // still run exact-timestamp endpoint occlusion checks so 3-layer occlusion works!
        bool fg_is_unoccluded = (length(v_fg) < 28.0)
            && ((length(v_fg - v_global_px) > length(v_bg - v_global_px) + 8.0)
                || (length(v_bg) > length(v_fg) + 12.0));

        float fg_pe, fg_bocc, fg_one, fg_ohud, fg_brk;
        vec3 c_fg = warp_layer(
            p, v_fg, v_global_px, fg_is_unoccluded, 0.0, 0.0, 1.0,
            max_g, inv_size, half_texel, fg_pe, fg_bocc, fg_one, fg_ohud, fg_brk
        );
        float bg_pe, bg_bocc, bg_one, bg_ohud, bg_brk;
        vec3 c_bg = warp_layer(
            p, v_bg, v_global_px, false, aux_occ_p, aux_occ_c, aux_conf,
            max_g, inv_size, half_texel, bg_pe, bg_bocc, bg_one, bg_ohud, bg_brk
        );
        c_bg = mix(c_bg, curr_unwarped, bg_ohud);

        // Continuous foreground alpha matte:
        // - For stationary/slow foregrounds (`fg_still_w -> 1`, e.g. character head/hair):
        //   combines trimmed warp match with unwarped self-consistency (`self_bg - self_fg`)
        //   for silky pixel-accurate anti-aliased hair strands!
        // - For moving midground objects (`fg_still_w -> 0`, e.g. parallax plastic chair -74px):
        //   NEVER compares against unwarped sky (`prev`/`curr`) — uses symmetric trimmed warp
        //   error `(err_bg - err_fg)` + smooth curved `fg_spatial` instead!
        float err_fg = eval_trimmed_warp_err(p, v_fg, inv_size, half_texel);
        float err_bg = eval_trimmed_warp_err(p, v_bg, inv_size, half_texel);
        float fg_photo = smoothstep(0.15, 0.030, err_fg);

        float fg_still_w = smoothstep(14.0, 4.0, length(v_fg));
        float self_fg = 0.5 * (color_err(prev_unwarped, c_fg) + color_err(curr_unwarped, c_fg));
        float self_bg = 0.5 * (color_err(prev_unwarped, c_bg) + color_err(curr_unwarped, c_bg));
        float fg_still_color_w = smoothstep(-0.06, 0.12, self_bg - self_fg);
        // Gate by `fg_photo` so moving background bridge cables/textures (high `err_fg`) can
        // never be falsely classified as stationary foreground!
        float fg_still_matte = clamp(
            fg_photo * mix(0.25, 1.0, fg_still_color_w),
            0.0,
            1.0
        );

        // On clean one-sided background disocclusions (`bg_one -> 1`), `err_bg` compares the valid
        // unoccluded endpoint against the occluded endpoint behind the character, inflating `err_bg`
        // artificially. Suppress that artificial boost so disoccluded background beside a jogging
        // character's head/shoulders/arms never pulls `fg_alpha` upward and smears a halo!
        float err_bg_eff = mix(err_bg, min(err_bg, 0.055), smoothstep(0.35, 0.80, bg_one));
        float fg_move_diff = smoothstep(-0.06, 0.08, err_bg_eff - err_fg);
        float fg_moving_matte = clamp(
            mix(fg_spatial, fg_move_diff, smoothstep(0.03, 0.14, abs(err_bg_eff - err_fg)))
                * mix(0.20, 1.0, fg_photo),
            0.0,
            1.0
        );

        float fg_high_contrast = mix(fg_moving_matte, fg_still_matte, fg_still_w);
        float layer_contrast = color_err(c_fg, c_bg);
        float contrast_w = smoothstep(0.05, 0.20, layer_contrast);
        float fg_matte = mix(fg_spatial, fg_high_contrast, contrast_w);

        // Exact C1 boundary clamp: guarantees `fg_alpha -> 0` as `fg_spatial -> 0` and
        // `fg_alpha -> 1` as `fg_spatial -> 1`, making 8x8 rectangular seams mathematically impossible!
        float lo_fade = smoothstep(0.0, 0.22, fg_spatial);
        float hi_fade = smoothstep(0.0, 0.22, 1.0 - fg_spatial);
        float fg_alpha = mix(fg_spatial, fg_matte, lo_fade * hi_fade);
        vec3 two_layer_rgb = mix(c_bg, c_fg, fg_alpha);

        // Broken-motion / popping-UI / non-rigid limb safety fallback in Regime B:
        // If BOTH layers have high warp error (e.g. a popping UI banner or rapidly swinging
        // hand/foot) and we are NOT on a clean one-sided disocclusion, fade cleanly in-place
        // instead of emitting shifted ghost copies or shredded pixels!
        float err_sel = mix(err_bg, err_fg, fg_alpha);
        float one_sel = mix(bg_one, fg_one, fg_alpha);
        float not_one_b = smoothstep(0.65, 0.15, one_sel);
        float fb_gain_b = smoothstep(0.0, 0.35, guard);
        if (err_sel > 0.075 && not_one_b > 0.01) {
            vec2 v_sel = mix(v_bg, v_fg, fg_alpha);
            float wide_b = max(err_sel, eval_wide_warp_err(p, v_sel, inv_size, half_texel));
            float severe_b = smoothstep(mix(0.36, 0.18, guard), mix(0.54, 0.30, guard), wide_b);
            float bad_sel = max(mix(max(bg_bocc, bg_brk), max(fg_bocc, fg_brk), fg_alpha), severe_b);
            float photo_bad_b = smoothstep(mix(0.28, 0.13, guard), mix(0.48, 0.26, guard), wide_b);
            float reg_b_fb = clamp(photo_bad_b * bad_sel * not_one_b * fb_gain_b, 0.0, 1.0);
            if (reg_b_fb > 0.01) {
                two_layer_rgb = mix(two_layer_rgb, 0.5 * (prev_unwarped + curr_unwarped), reg_b_fb);
            }
        }

        // Smooth C1 transition between Regime A (`v_smooth`) and Regime B (`two_layer_rgb`).
        if (std_v < 6.5) {
            float r_pe, r_bocc, r_one, r_ohud, r_brk;
            vec3 reg_a_rgb = warp_layer(
                p, v_smooth, v_global_px, false, aux_occ_p, aux_occ_c, aux_conf,
                max_g, inv_size, half_texel, r_pe, r_bocc, r_one, r_ohud, r_brk
            );
            reg_a_rgb = mix(reg_a_rgb, curr_unwarped, r_ohud);
            float r_not_one = smoothstep(0.65, 0.15, r_one);
            if (r_pe > 0.075 && r_not_one > 0.01) {
                float r_wide = max(r_pe, eval_wide_warp_err(p, v_smooth, inv_size, half_texel));
                float r_severe = smoothstep(mix(0.36, 0.18, guard), mix(0.54, 0.30, guard), r_wide);
                float r_photo_bad = smoothstep(mix(0.28, 0.13, guard), mix(0.48, 0.26, guard), r_wide);
                float r_fb = clamp(r_photo_bad * max(max(r_bocc, r_brk), r_severe) * r_not_one * fb_gain_b, 0.0, 1.0);
                if (r_fb > 0.01) {
                    reg_a_rgb = mix(reg_a_rgb, 0.5 * (prev_unwarped + curr_unwarped), r_fb);
                }
            }
            float t_regime = smoothstep(2.5, 6.5, std_v);
            out_rgb = mix(reg_a_rgb, two_layer_rgb, t_regime);
        } else {
            out_rgb = two_layer_rgb;
        }
    }

    if (hud_lock_w > 0.0) {
        out_rgb = mix(out_rgb, 0.5 * (prev_unwarped + curr_unwarped), hud_lock_w);
    }
    imageStore(u_out_interp, gid, vec4(max(out_rgb, vec3(0.0)), 1.0));
}
