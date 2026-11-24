#version 430 core
// AMD FidelityFX Optical Flow — Precomputed Full-Resolution Stationary HUD/Reticle Mask (R8).
//
// Evaluates the orientation-invariant coincident high-pass + opaque static-stroke HUD detector
// ONCE per pixel on 1-byte R8 perceptual luma (`luma_pyramids[slot][0]`) using a 26x26 LDS tile
// and strict local early-outs BEFORE any remote `q +/- mv_ref` fetches.
//
// Eliminates redundant 10x-27x per-pixel RGBA16F HUD re-evaluations inside `cs_ffx_frame_interp`
// (~50x-100x lower texture bandwidth) while preserving exact thin-reticle, sidebar, and translucent
// header HUD protection.

layout(local_size_x = 16, local_size_y = 16, local_size_z = 1) in;

layout(binding = 0) uniform sampler2D u_prev_luma;
layout(binding = 1) uniform sampler2D u_curr_luma;
layout(binding = 2) uniform isampler2D u_mid_flow;
layout(binding = 3) uniform usampler2D u_scd_tex;
layout(r8, binding = 0) writeonly uniform image2D u_out_hud_mask;

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
uniform float u_hud_protection;

// 16x16 workgroup + 5px halo on each side = 26x26 LDS tile covering +/-4.25px bilinear taps.
const int TILE_HALO = 5;
const int TILE_W = 26;
shared float s_prev[TILE_W * TILE_W];
shared float s_curr[TILE_W * TILE_W];

float lds_bilin_prev(vec2 tc) {
    ivec2 i0 = ivec2(floor(tc));
    vec2 f = tc - vec2(i0);
    int r0 = i0.y * TILE_W + i0.x;
    int r1 = r0 + TILE_W;
    float a = mix(s_prev[r0], s_prev[r0 + 1], f.x);
    float b = mix(s_prev[r1], s_prev[r1 + 1], f.x);
    return mix(a, b, f.y);
}

float lds_bilin_curr(vec2 tc) {
    ivec2 i0 = ivec2(floor(tc));
    vec2 f = tc - vec2(i0);
    int r0 = i0.y * TILE_W + i0.x;
    int r1 = r0 + TILE_W;
    float a = mix(s_curr[r0], s_curr[r0 + 1], f.x);
    float b = mix(s_curr[r1], s_curr[r1 + 1], f.x);
    return mix(a, b, f.y);
}

float lds_hp_prev(vec2 tc, out float out_lum) {
    float c = lds_bilin_prev(tc);
    float l0 = lds_bilin_prev(tc + vec2(-3.0,  0.0));
    float l1 = lds_bilin_prev(tc + vec2( 3.0,  0.0));
    float l2 = lds_bilin_prev(tc + vec2( 0.0, -3.0));
    float l3 = lds_bilin_prev(tc + vec2( 0.0,  3.0));
    out_lum = c;
    return c - 0.25 * (l0 + l1 + l2 + l3);
}

float lds_hp_curr(vec2 tc, out float out_lum) {
    float c = lds_bilin_curr(tc);
    float l0 = lds_bilin_curr(tc + vec2(-3.0,  0.0));
    float l1 = lds_bilin_curr(tc + vec2( 3.0,  0.0));
    float l2 = lds_bilin_curr(tc + vec2( 0.0, -3.0));
    float l3 = lds_bilin_curr(tc + vec2( 0.0,  3.0));
    out_lum = c;
    return c - 0.25 * (l0 + l1 + l2 + l3);
}

float sample_hp_luma_tex(sampler2D tex, vec2 q, vec2 inv_size, vec2 half_texel, out float out_lum) {
    float c = texture(tex, clamp(q * inv_size, half_texel, 1.0 - half_texel)).r;
    float l0 = texture(tex, clamp((q + vec2(-3.0,  0.0)) * inv_size, half_texel, 1.0 - half_texel)).r;
    float l1 = texture(tex, clamp((q + vec2( 3.0,  0.0)) * inv_size, half_texel, 1.0 - half_texel)).r;
    float l2 = texture(tex, clamp((q + vec2( 0.0, -3.0)) * inv_size, half_texel, 1.0 - half_texel)).r;
    float l3 = texture(tex, clamp((q + vec2( 0.0,  3.0)) * inv_size, half_texel, 1.0 - half_texel)).r;
    out_lum = c;
    return c - 0.25 * (l0 + l1 + l2 + l3);
}

void main() {
    ivec2 gid = ivec2(gl_GlobalInvocationID.xy);

    // Uniform early-out across the entire dispatch on scene cut / HUD protection off.
    if (u_hud_protection <= 0.01 || texelFetch(u_scd_tex, ivec2(0, 0), 0).r != 0u) {
        if (gid.x < u_frame_size.x && gid.y < u_frame_size.y) {
            imageStore(u_out_hud_mask, gid, vec4(0.0));
        }
        return;
    }

    // Cooperatively load the 26x26 R8 perceptual luma tile into LDS (2.64 texels/thread).
    ivec2 wg_origin = ivec2(gl_WorkGroupID.xy) * 16 - ivec2(TILE_HALO);
    ivec2 max_p = max(u_frame_size - ivec2(1), ivec2(0));
    uint tid = gl_LocalInvocationIndex;
    for (uint idx = tid; idx < uint(TILE_W * TILE_W); idx += 256u) {
        int ly = int(idx) / TILE_W;
        int lx = int(idx) - ly * TILE_W;
        ivec2 sc = clamp(wg_origin + ivec2(lx, ly), ivec2(0), max_p);
        s_prev[idx] = texelFetch(u_prev_luma, sc, 0).r;
        s_curr[idx] = texelFetch(u_curr_luma, sc, 0).r;
    }
    barrier();

    if (gid.x >= u_frame_size.x || gid.y >= u_frame_size.y) {
        return;
    }

    ivec2 max_g = max(u_grid_size - ivec2(1), ivec2(0));
    vec2 v_global_px = vec2(float(u_stats.global_mv_x_q), float(u_stats.global_mv_y_q)) * 0.25;
    ivec2 bc = clamp(gid / 8, ivec2(0), max_g);
    vec2 v_local = vec2(texelFetch(u_mid_flow, bc, 0).xy) * 0.25;
    float local_spd = length(v_local);

    // Suppress HUD lock on confirmed slowly-moving 3D foreground objects (e.g. a bobbing/turning
    // character head moving 1.5-7.5px/frame) so high-contrast skin/silhouette edges never get a
    // stationary chalk-outline ring! True HUD panels have `local_spd < 1.25px` (`(0, 0)`), and
    // thin reticles embedded in a panning map have `local_spd >= 8.5px` (sealed into `v_bg`).
    float fg_move_suppress = smoothstep(1.1, 2.2, local_spd) * smoothstep(9.0, 5.5, local_spd);
    if (fg_move_suppress >= 0.99) {
        imageStore(u_out_hud_mask, gid, vec4(0.0));
        return;
    }

    vec2 mv_ref = v_local;
    if (local_spd <= 4.0) {
        // When pixel sits on a static HUD block `(0, 0)`, probe nearby +/-2 blocks for the local
        // moving background velocity behind the HUD, falling back to `v_global_px`.
        vec2 best_bg = vec2(0.0);
        float best_spd = 0.0;
        ivec2 po[4];
        po[0] = ivec2(-2,  0);
        po[1] = ivec2( 2,  0);
        po[2] = ivec2( 0, -2);
        po[3] = ivec2( 0,  2);
        for (int i = 0; i < 4; ++i) {
            vec2 nv = vec2(texelFetch(u_mid_flow, clamp(bc + po[i], ivec2(0), max_g), 0).xy) * 0.25;
            float spd = length(nv);
            if (spd > best_spd) {
                best_spd = spd;
                best_bg = nv;
            }
        }
        mv_ref = (best_spd > 4.0) ? best_bg : v_global_px;
    }

    float mv_len = length(mv_ref);
    float speed_w = smoothstep(4.0, 10.0, mv_len);
    if (speed_w <= 0.001) {
        imageStore(u_out_hud_mask, gid, vec4(0.0));
        return;
    }

    // Step 1: Pure-LDS local static & high-pass screening (ZERO remote texture fetches!).
    vec2 tc0 = vec2(gl_LocalInvocationID.xy) + vec2(float(TILE_HALO));
    float lp_c, lc_c;
    float hp_p0 = lds_hp_prev(tc0, lp_c);
    float hp_c0 = lds_hp_curr(tc0, lc_c);
    float dl_c = abs(lp_c - lc_c);
    bool has_hp_cand = (hp_p0 * hp_c0 > 0.0) && (min(abs(hp_p0), abs(hp_c0)) > 0.038);
    if (!has_hp_cand && dl_c > 0.065) {
        imageStore(u_out_hud_mask, gid, vec4(0.0));
        return;
    }

    vec2 offs[5];
    offs[0] = vec2( 0.0,   0.0);
    offs[1] = vec2(-1.25,  0.0);
    offs[2] = vec2( 1.25,  0.0);
    offs[3] = vec2( 0.0,  -1.25);
    offs[4] = vec2( 0.0,   1.25);

    float hpp_arr[5];
    float hpc_arr[5];
    float lp_arr[5];
    float lc_arr[5];
    hpp_arr[0] = hp_p0;
    hpc_arr[0] = hp_c0;
    lp_arr[0] = lp_c;
    lc_arr[0] = lc_c;

    float static_hp_err = abs(hp_p0 - hp_c0);
    float hp_energy = (hp_p0 * hp_c0 > 0.0) ? min(abs(hp_p0), abs(hp_c0)) : 0.0;
    float m1 = 1e9;
    float m2 = 1e9;
    float max_contrast = hp_energy;

    for (int i = 1; i < 5; ++i) {
        vec2 tci = tc0 + offs[i];
        float lp, lc;
        float hpp = lds_hp_prev(tci, lp);
        float hpc = lds_hp_curr(tci, lc);
        hpp_arr[i] = hpp;
        hpc_arr[i] = hpc;
        lp_arr[i] = lp;
        lc_arr[i] = lc;

        static_hp_err += abs(hpp - hpc);
        if (hpp * hpc > 0.0) {
            hp_energy += min(abs(hpp), abs(hpc));
        }
        float dl = abs(lp - lc);
        if (dl < m1) {
            m2 = m1;
            m1 = dl;
        } else if (dl < m2) {
            m2 = dl;
        }
        float dp = lp_c - lp;
        float dc = lc_c - lc;
        if (dp * dc > 0.0) {
            max_contrast = max(max_contrast, min(abs(dp), abs(dc)));
        }
    }
    static_hp_err *= 0.2;
    hp_energy *= 0.2;

    float stroke_dl = 0.50 * dl_c + 0.25 * (m1 + m2);
    float max_hp_tol = 0.115 + 0.45 * max(hp_energy - 0.09, 0.0);
    bool can_hp = (hp_energy > 0.045) && (static_hp_err < max_hp_tol);
    bool can_opaque = (stroke_dl < 0.055) && (max_contrast > 0.035);

    // Reject ~94% of all pixels (sky, asphalt, walls, flat shirts, moving textures) with ZERO
    // remote texture fetches!
    if (!can_hp && !can_opaque) {
        imageStore(u_out_hud_mask, gid, vec4(0.0));
        return;
    }

    // Step 2: Remote `q +/- mv_ref` and midpoint motion proof on 1-byte R8 luma (~6% of pixels).
    vec2 frame_f = vec2(max(u_frame_size, ivec2(1)));
    vec2 inv_size = 1.0 / frame_f;
    vec2 half_texel = 0.5 * inv_size;
    vec2 q = vec2(gid) + 0.5;

    float fwd_hp_err = 0.0;
    float bwd_hp_err = 0.0;
    float fwd_l_err = 0.0;
    float bwd_l_err = 0.0;

    for (int i = 0; i < 5; ++i) {
        vec2 qi = q + offs[i];
        float lc_f, lp_b;
        float hpc_f = sample_hp_luma_tex(u_curr_luma, qi + mv_ref, inv_size, half_texel, lc_f);
        float hpp_b = sample_hp_luma_tex(u_prev_luma, qi - mv_ref, inv_size, half_texel, lp_b);
        fwd_hp_err += abs(hpp_arr[i] - hpc_f);
        bwd_hp_err += abs(hpc_arr[i] - hpp_b);
        fwd_l_err += abs(lp_arr[i] - lc_f);
        bwd_l_err += abs(lc_arr[i] - lp_b);
    }
    fwd_hp_err *= 0.2;
    bwd_hp_err *= 0.2;
    fwd_l_err *= 0.2;
    bwd_l_err *= 0.2;

    float lp_m, lc_m;
    float hp_pm = sample_hp_luma_tex(u_prev_luma, q - 0.5 * mv_ref, inv_size, half_texel, lp_m);
    float hp_cm = sample_hp_luma_tex(u_curr_luma, q + 0.5 * mv_ref, inv_size, half_texel, lc_m);
    float mid_diff_l = min(abs(lp_c - lp_m), abs(lc_c - lc_m));
    float mid_diff_hp = min(abs(hp_p0 - hp_pm), abs(hp_c0 - hp_cm));

    float min_moved_hp = min(fwd_hp_err, bwd_hp_err);
    float max_moved_hp = max(fwd_hp_err, bwd_hp_err);
    float min_moved_l = min(fwd_l_err, bwd_l_err);
    float max_moved_l = max(fwd_l_err, bwd_l_err);

    float stroke_gate = smoothstep(0.06, 0.11, hp_energy);
    float hp_boost_gate = stroke_gate * smoothstep(0.04, 0.08, min_moved_hp);
    float moved_hp_sig = max(min_moved_hp, min(max_moved_hp, mid_diff_hp) * stroke_gate);
    float moved_l_sig = max(min_moved_l, min(max_moved_l, mid_diff_l) * hp_boost_gate);

    float hp_tol = 0.115 + 0.45 * max(hp_energy - 0.09, 0.0) * smoothstep(0.06, 0.10, min_moved_hp);
    float hp_hud = smoothstep(0.045, 0.085, hp_energy)
        * smoothstep(hp_tol, 0.055, static_hp_err)
        * smoothstep(static_hp_err + 0.025, static_hp_err + 0.065, moved_hp_sig)
        * smoothstep(0.035, 0.075, max(moved_l_sig, moved_hp_sig));

    float opaque_hud = smoothstep(0.055, 0.018, stroke_dl)
        * smoothstep(stroke_dl + 0.030, stroke_dl + 0.085, moved_l_sig)
        * smoothstep(0.035, 0.085, max_contrast);

    float hud_w = clamp(speed_w * max(hp_hud, opaque_hud) * (1.0 - fg_move_suppress), 0.0, 1.0);
    imageStore(u_out_hud_mask, gid, vec4(hud_w, 0.0, 0.0, 0.0));
}
