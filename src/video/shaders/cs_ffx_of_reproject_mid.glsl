#version 430 core
// AMD FidelityFX Optical Flow — Foreground-Aware Hybrid Splat + Dense Fixed-Point Midpoint
// (t = 0.5) Vector Field Construction + Dominant Background Mode & AFMF Quality Gate.
//
// u_mode == 0: Zero-clear u_mid_packed (R32UI) & reset telemetry counters.
// u_mode == 4: Estimate dominant global background velocity mode (`v_global`) via an 8x8
//              kernel-density peak finder in shared memory (1x1x1 workgroup, BEFORE splatting).
// u_mode == 1: Cycle-consistency validated midpoint splat with foreground-occluder priority
//              (objects deviating from `v_global` beat background splats jumping behind them).
// u_mode == 2: Foreground-preserving hybrid unpack + dense 2-iter fixed-point midpoint gather:
//              - Protects locally consistent stationary/slow foreground characters (heads/bodies/HUD).
//              - Requires genuine fixed-point convergence so disocclusion halos get `conf = 0`.
//              - Fills disocclusion halos with the background neighbor closest to `v_global`
//                (never the stationary character head!).
// u_mode == 3: Global AFMF-style quality gate (1 thread): falls back cleanly to real frame
//              when too large a fraction of the screen fails cycle consistency.
// u_mode == 5: Build per-block cycle-validated RGBA8 auxiliary matte (occ_prev, occ_curr,
//              fg_mask, conf) before 5x5 Gaussian smoothing for C1-continuous disocclusion.

layout(local_size_x = 8, local_size_y = 8, local_size_z = 1) in;

layout(binding = 0) uniform isampler2D u_fwd_flow;
layout(binding = 1) uniform isampler2D u_bwd_flow;
layout(binding = 2) uniform isampler2D u_mid_flow_in;
layout(binding = 3) uniform sampler2D u_mid_conf_in;
layout(r32ui, binding = 0) coherent uniform uimage2D u_mid_packed;
layout(rg16i, binding = 1) writeonly uniform iimage2D u_out_mid_flow;
layout(r8, binding = 2) writeonly uniform image2D u_out_mid_conf;
layout(r32ui, binding = 3) coherent uniform uimage2D u_scd_img;
layout(rgba8, binding = 4) writeonly uniform image2D u_out_mid_aux;

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
    uint moved_blocks;
    uint jump_blocks;
    uint _pad2;
} u_stats;

uniform ivec2 u_grid_size;
uniform int u_mode;
uniform float u_consistency_thresh;
uniform float u_artifact_guard;

shared vec2 s_mode_mv[64];
shared uint s_mode_valid[64];
shared float s_mode_density[64];

vec2 sample_flow_bilin(isampler2D flow_tex, vec2 grid_pos, ivec2 max_g) {
    ivec2 b0 = ivec2(floor(grid_pos));
    vec2 f = clamp(grid_pos - vec2(b0), 0.0, 1.0);
    vec2 v00 = vec2(texelFetch(flow_tex, clamp(b0 + ivec2(0, 0), ivec2(0), max_g), 0).xy);
    vec2 v10 = vec2(texelFetch(flow_tex, clamp(b0 + ivec2(1, 0), ivec2(0), max_g), 0).xy);
    vec2 v01 = vec2(texelFetch(flow_tex, clamp(b0 + ivec2(0, 1), ivec2(0), max_g), 0).xy);
    vec2 v11 = vec2(texelFetch(flow_tex, clamp(b0 + ivec2(1, 1), ivec2(0), max_g), 0).xy);
    return mix(mix(v00, v10, f.x), mix(v01, v11, f.x), f.y) * 0.25;
}

// Edge-preserving flow lookup: uses bilinear interpolation inside smooth motion surfaces and
// snaps to nearest block across motion discontinuities (`spread > 10 px`), preventing Pass 2's
// fixed-point gather from fabricating fake intermediate half-speed vectors beside silhouettes!
vec2 sample_flow_edge_safe(isampler2D flow_tex, vec2 grid_pos, ivec2 max_g) {
    ivec2 b0 = ivec2(floor(grid_pos));
    vec2 f = clamp(grid_pos - vec2(b0), 0.0, 1.0);
    vec2 v00 = vec2(texelFetch(flow_tex, clamp(b0 + ivec2(0, 0), ivec2(0), max_g), 0).xy);
    vec2 v10 = vec2(texelFetch(flow_tex, clamp(b0 + ivec2(1, 0), ivec2(0), max_g), 0).xy);
    vec2 v01 = vec2(texelFetch(flow_tex, clamp(b0 + ivec2(0, 1), ivec2(0), max_g), 0).xy);
    vec2 v11 = vec2(texelFetch(flow_tex, clamp(b0 + ivec2(1, 1), ivec2(0), max_g), 0).xy);
    vec2 vmin = min(min(v00, v10), min(v01, v11));
    vec2 vmax = max(max(v00, v10), max(v01, v11));
    vec2 spread = vmax - vmin;
    if (max(spread.x, spread.y) <= 40.0) {
        return mix(mix(v00, v10, f.x), mix(v01, v11, f.x), f.y) * 0.25;
    }
    ivec2 nr = clamp(ivec2(round(grid_pos)), ivec2(0), max_g);
    return vec2(texelFetch(flow_tex, nr, 0).xy) * 0.25;
}

bool in_grid(vec2 p, ivec2 max_g) {
    return p.x >= -0.5 && p.y >= -0.5 && p.x <= float(max_g.x) + 0.5 && p.y <= float(max_g.y) + 0.5;
}

// Pack half-pixel signed motion vectors (+/-511.5 px) + 10-bit foreground-aware priority.
// Solidly cycle-consistent vectors (`conf >= 0.45`) prioritize foreground occluders (higher
// deviation from dominant background velocity `v_global_px`) so spinning background splats
// cannot overwrite stationary/slow 3rd-person characters, and moving foreground characters
// beat static backgrounds.
uint pack_mid_mv(vec2 mv_px, float conf_norm, vec2 v_global_px) {
    uint prio;
    if (conf_norm >= 0.45) {
        uint fg_tier = min(31u, uint(length(mv_px - v_global_px) * 0.25 + 0.5));
        uint conf_tier = uint(clamp(conf_norm * 15.0 + 0.5, 1.0, 15.0));
        prio = 512u | (fg_tier << 4u) | conf_tier;
    } else {
        prio = uint(clamp(conf_norm * 511.0 + 0.5, 1.0, 511.0));
    }
    int hx = clamp(int(round(mv_px.x * 2.0)), -1023, 1023);
    int hy = clamp(int(round(mv_px.y * 2.0)), -1023, 1023);
    uint enc_x = uint(hx + 1024);
    uint enc_y = uint(hy + 1024);
    return (prio << 22u) | (enc_x << 11u) | enc_y;
}

vec2 unpack_mid_mv_px(uint packed_mv) {
    int hx = int((packed_mv >> 11u) & 2047u) - 1024;
    int hy = int(packed_mv & 2047u) - 1024;
    return vec2(float(hx), float(hy)) * 0.5;
}

float unpack_mid_conf(uint packed_mv) {
    uint prio = packed_mv >> 22u;
    if (prio >= 512u) {
        return clamp(float(prio & 15u) / 15.0, 0.45, 1.0);
    }
    return clamp(float(prio) / 511.0, 0.0, 0.45);
}

void splat_cell(ivec2 cell, uint packed_mv) {
    if (cell.x >= 0 && cell.y >= 0 && cell.x < u_grid_size.x && cell.y < u_grid_size.y) {
        imageAtomicMax(u_mid_packed, cell, packed_mv);
    }
}

void write_mid_block(ivec2 gid, vec2 mv_px, float conf_val, bool count_valid, vec2 v_global_px) {
    ivec2 out_q = clamp(ivec2(round(mv_px * 4.0)), ivec2(-1280), ivec2(1280));
    imageStore(u_out_mid_flow, gid, ivec4(out_q, 0, 0));
    imageStore(u_out_mid_conf, gid, vec4(conf_val, 0.0, 0.0, 0.0));
    if (count_valid) {
        atomicAdd(u_stats.valid_blocks, 1u);
    }
    float rel_spd = length(mv_px - v_global_px);
    if (rel_spd > 4.0) {
        atomicAdd(u_stats.moved_blocks, 1u);
    }
    if (rel_spd > 28.0) {
        atomicAdd(u_stats.jump_blocks, 1u);
    }
}

void main() {
    ivec2 gid = ivec2(gl_GlobalInvocationID.xy);
    ivec2 max_g = max(u_grid_size - ivec2(1), ivec2(0));
    float base_thresh = max(u_consistency_thresh, 0.75);

    if (u_mode == 4) {
        // Dominant global background velocity mode estimation (single 8x8 workgroup = 64 samples).
        uint tid = gl_LocalInvocationIndex;
        vec2 norm_xy = (vec2(gl_LocalInvocationID.xy) + 0.5) / 8.0;
        ivec2 sc = clamp(ivec2(floor(norm_xy * vec2(u_grid_size))), ivec2(0), max_g);
        vec2 fwd_px = vec2(texelFetch(u_fwd_flow, sc, 0).xy) * 0.25;
        vec2 dst_f = vec2(sc) + fwd_px / 8.0;
        uint valid = 0u;
        if (in_grid(dst_f, max_g)) {
            vec2 bwd_at_dst = sample_flow_bilin(u_bwd_flow, dst_f, max_g);
            float fb_err = length(fwd_px + bwd_at_dst);
            float max_err = base_thresh + 0.14 * length(fwd_px);
            if (fb_err <= max_err) {
                valid = 1u;
            }
        }
        s_mode_mv[tid] = fwd_px;
        s_mode_valid[tid] = valid;
        barrier();

        float dens = 0.0;
        if (valid != 0u) {
            for (uint j = 0u; j < 64u; ++j) {
                if (s_mode_valid[j] != 0u) {
                    vec2 d = s_mode_mv[j] - fwd_px;
                    // Wide bandwidth (sigma = 28 px) pools perspective camera rotation spread
                    // (e.g. -95..-145 px across 1080p) so the spinning background always beats
                    // a tightly clustered stationary character spike at (0, 0)!
                    dens += exp(-dot(d, d) / (2.0 * 784.0));
                }
            }
        }
        s_mode_density[tid] = dens;
        barrier();

        if (tid == 0u) {
            float best_dens = -1.0;
            uint best_i = 0u;
            uint valid_cnt = 0u;
            for (uint i = 0u; i < 64u; ++i) {
                if (s_mode_valid[i] != 0u) {
                    valid_cnt += 1u;
                    if (s_mode_density[i] > best_dens) {
                        best_dens = s_mode_density[i];
                        best_i = i;
                    }
                }
            }
            if (valid_cnt > 0u) {
                vec2 peak = s_mode_mv[best_i];
                vec2 wsum = vec2(0.0);
                float wtot = 0.0;
                for (uint j = 0u; j < 64u; ++j) {
                    if (s_mode_valid[j] != 0u) {
                        vec2 d = s_mode_mv[j] - peak;
                        float d2 = dot(d, d);
                        if (d2 <= 48.0 * 48.0) {
                            float w = exp(-d2 / (2.0 * 784.0));
                            wsum += s_mode_mv[j] * w;
                            wtot += w;
                        }
                    }
                }
                vec2 mode_mv = (wtot > 1e-4) ? (wsum / wtot) : peak;
                ivec2 mode_q = clamp(ivec2(round(mode_mv * 4.0)), ivec2(-1280), ivec2(1280));
                u_stats.global_mv_x_q = mode_q.x;
                u_stats.global_mv_y_q = mode_q.y;
                u_stats.splat_count = valid_cnt;
            } else {
                u_stats.global_mv_x_q = 0;
                u_stats.global_mv_y_q = 0;
                u_stats.splat_count = 0u;
            }
        }
        return;
    }

    if (u_mode == 3) {
        // Global AFMF-style quality gate + static-screen discrete UI teleport gate:
        // runs as a single 1x1x1 dispatch after u_mode == 2.
        if (gid == ivec2(0, 0)) {
            uint already_cut = imageLoad(u_scd_img, ivec2(0, 0)).r;
            if (already_cut == 0u) {
                uint total_b = max(u_stats.total_blocks, 1u);
                float valid_ratio = float(u_stats.valid_blocks) / float(total_b);
                float guard = clamp(u_artifact_guard, 0.0, 1.0);
                float min_valid = mix(0.20, 0.65, guard);

                vec2 g_mv = vec2(float(u_stats.global_mv_x_q), float(u_stats.global_mv_y_q)) * 0.25;
                float g_spd = length(g_mv);

                // Stationary-screen discrete UI transition detection (menu selector step, wrap-around,
                // sub-menu highlight, or dialog popup — even over a softly animating/pulsing pause backdrop):
                // On a stationary camera (`g_spd < 2.0`), real smooth foreground motion (walking characters,
                // moving objects) produces mostly cycle-consistent `4..28 px` (`smooth_moved`) blocks with
                // a small occlusion rim (`invalid_b < smooth_moved`) and minimal `>28 px` teleports.
                // Discrete UI bar jumps & wrap-arounds instead produce a burst of `>28 px` teleports
                // and/or non-cycle-consistent `invalid_b` blocks that dwarfs `smooth_moved`.
                uint invalid_b = (total_b > u_stats.valid_blocks)
                    ? (total_b - u_stats.valid_blocks)
                    : 0u;
                uint smooth_moved = (u_stats.moved_blocks > u_stats.jump_blocks)
                    ? (u_stats.moved_blocks - u_stats.jump_blocks)
                    : 0u;
                uint ui_broken = invalid_b + u_stats.jump_blocks;
                float moved_ratio = float(u_stats.moved_blocks) / float(total_b);
                float jump_ratio = float(u_stats.jump_blocks) / float(total_b);
                float broken_ratio = float(ui_broken) / float(total_b);
                float jump_share = float(u_stats.jump_blocks) / float(max(u_stats.moved_blocks, 1u));
                bool static_ui_jump = (g_spd < 5.0)
                    && (u_stats.jump_blocks >= 8u)
                    && ((jump_ratio > 0.010) || (moved_ratio < 0.25 && jump_share > 0.45));
                bool static_ui_pop = (g_spd < 5.0)
                    && (ui_broken >= 16u)
                    && (broken_ratio > 0.032)
                    && (moved_ratio < 0.20)
                    && (float(ui_broken) > 2.2 * float(max(smooth_moved, 1u)));

                // Extreme camera whip spin (> ~195 px/frame at default guard=0.65, > 10% of 1080p
                // width in a single 30 FPS frame): thin poles & hair silhouettes span >5x their
                // width, so crisp real-frame AFMF fallback avoids shattered floating pole chunks.
                float max_whip_spd = mix(260.0, 170.0, guard);
                bool extreme_whip = (g_spd > max_whip_spd);

                if (valid_ratio < min_valid || static_ui_jump || static_ui_pop || extreme_whip) {
                    imageStore(u_scd_img, ivec2(0, 0), uvec4(1u, 0u, 0u, 0u));
                    u_stats.scene_cut = 1u;
                    u_stats.quality_fallback = 1u;
                    u_stats.total_cuts += 1u;
                    u_stats.total_quality_fallbacks += 1u;
                }
            }
        }
        return;
    }

    if (gid.x >= u_grid_size.x || gid.y >= u_grid_size.y) {
        return;
    }

    if (u_mode == 5) {
        // Build continuous auxiliary matte (occ_prev, occ_curr, fg_mask, conf) using EXACT
        // per-block cycle-consistency-validated endpoint occluders combined via smooth Gaussian
        // weighting (never stamping hard circular max-disks around isolated outlier blocks!).
        vec2 v_global_px = vec2(float(u_stats.global_mv_x_q), float(u_stats.global_mv_y_q)) * 0.25;
        vec2 mv = vec2(texelFetch(u_mid_flow_in, gid, 0).xy) * 0.25;
        float conf = texelFetch(u_mid_conf_in, gid, 0).r;
        float mv_speed = length(mv);
        float diff_thresh = 6.0 + 0.16 * mv_speed;
        float mv_fg_dist = length(mv - v_global_px);
        float dilate_gain = smoothstep(10.0, 32.0, mv_speed);
        float boost = mix(1.25, 2.35, dilate_gain);

        vec2 p0 = vec2(gid) - (0.5 * mv) / 8.0;
        vec2 p1 = vec2(gid) + (0.5 * mv) / 8.0;

        float occ_p = 0.0;
        if (in_grid(p0, max_g)) {
            ivec2 b0 = clamp(ivec2(round(p0)), ivec2(0), max_g);
            float occ_sum = 0.0;
            float w_tot = 0.0;
            for (int dy = -1; dy <= 1; ++dy) {
                for (int dx = -1; dx <= 1; ++dx) {
                    ivec2 nc = clamp(b0 + ivec2(dx, dy), ivec2(0), max_g);
                    vec2 delta = vec2(nc) - p0;
                    float w_sp = exp(-dot(delta, delta) / 0.90);
                    w_tot += w_sp;

                    vec2 nm = vec2(texelFetch(u_fwd_flow, nc, 0).xy) * 0.25;
                    bool is_center = (dot(delta, delta) < 0.50);
                    bool is_closer_fg = (mv_fg_dist < 28.0) || (length(nm - v_global_px) > mv_fg_dist + 8.0);
                    if (!is_center && !is_closer_fg) {
                        continue;
                    }
                    float dev = length(nm - mv);
                    if (dev <= diff_thresh * 0.55) {
                        continue;
                    }
                    vec2 nc_curr = vec2(nc) + nm / 8.0;
                    if (in_grid(nc_curr, max_g)) {
                        ivec2 bc = clamp(ivec2(round(nc_curr)), ivec2(0), max_g);
                        vec2 nvb = vec2(texelFetch(u_bwd_flow, bc, 0).xy) * 0.25;
                        float nfb = length(nm + nvb);
                        float nfb_t = 5.5 + 0.14 * length(nm);
                        float nv = smoothstep(nfb_t * 1.45, nfb_t * 0.55, nfb);
                        float occ_n = smoothstep(diff_thresh * 0.55, diff_thresh * 1.25, dev) * nv;
                        occ_sum += occ_n * w_sp;
                    }
                }
            }
            occ_p = clamp((occ_sum / max(w_tot, 1e-4)) * boost, 0.0, 1.0);
        }

        float occ_c = 0.0;
        if (in_grid(p1, max_g)) {
            ivec2 b1 = clamp(ivec2(round(p1)), ivec2(0), max_g);
            float occ_sum = 0.0;
            float w_tot = 0.0;
            for (int dy = -1; dy <= 1; ++dy) {
                for (int dx = -1; dx <= 1; ++dx) {
                    ivec2 nc = clamp(b1 + ivec2(dx, dy), ivec2(0), max_g);
                    vec2 delta = vec2(nc) - p1;
                    float w_sp = exp(-dot(delta, delta) / 0.90);
                    w_tot += w_sp;

                    vec2 nvb = vec2(texelFetch(u_bwd_flow, nc, 0).xy) * 0.25;
                    vec2 nm = -nvb;
                    bool is_center = (dot(delta, delta) < 0.50);
                    bool is_closer_fg = (mv_fg_dist < 28.0) || (length(nm - v_global_px) > mv_fg_dist + 8.0);
                    if (!is_center && !is_closer_fg) {
                        continue;
                    }
                    float dev = length(nm - mv);
                    if (dev <= diff_thresh * 0.55) {
                        continue;
                    }
                    vec2 nc_prev = vec2(nc) + nvb / 8.0;
                    if (in_grid(nc_prev, max_g)) {
                        ivec2 bp = clamp(ivec2(round(nc_prev)), ivec2(0), max_g);
                        vec2 nvf = vec2(texelFetch(u_fwd_flow, bp, 0).xy) * 0.25;
                        float nfb = length(nvb + nvf);
                        float nfb_t = 5.5 + 0.14 * length(nvb);
                        float nv = smoothstep(nfb_t * 1.45, nfb_t * 0.55, nfb);
                        float occ_n = smoothstep(diff_thresh * 0.55, diff_thresh * 1.25, dev) * nv;
                        occ_sum += occ_n * w_sp;
                    }
                }
            }
            occ_c = clamp((occ_sum / max(w_tot, 1e-4)) * boost, 0.0, 1.0);
        }

        float fg_mask = smoothstep(10.0, 28.0, mv_fg_dist);
        imageStore(u_out_mid_aux, gid, vec4(occ_p, occ_c, fg_mask, conf));
        return;
    }

    if (u_mode == 0) {
        imageStore(u_mid_packed, gid, uvec4(0u));
        if (gid == ivec2(0, 0)) {
            u_stats.valid_blocks = 0u;
            u_stats.total_blocks = uint(max(u_grid_size.x * u_grid_size.y, 1));
            u_stats.quality_fallback = 0u;
            u_stats.sum_mv_x = 0;
            u_stats.sum_mv_y = 0;
            u_stats.splat_count = 0u;
            u_stats.global_mv_x_q = 0;
            u_stats.global_mv_y_q = 0;
            u_stats.moved_blocks = 0u;
            u_stats.jump_blocks = 0u;
        }
        return;
    }

    vec2 v_global_px = vec2(float(u_stats.global_mv_x_q), float(u_stats.global_mv_y_q)) * 0.25;

    if (u_mode == 1) {
        // Validate & splat forward flow (prev -> curr) if cycle-consistent with backward flow.
        // Uses `sample_flow_edge_safe` so occluded garbage flow beside a foreground head can NEVER
        // land on a bilinearly-blended discontinuity edge and splat fake half-speed vectors!
        vec2 fwd_px = vec2(texelFetch(u_fwd_flow, gid, 0).xy) * 0.25;
        vec2 dst_f = vec2(gid) + fwd_px / 8.0;
        if (in_grid(dst_f, max_g)) {
            vec2 bwd_at_dst = sample_flow_edge_safe(u_bwd_flow, dst_f, max_g);
            float fb_err = length(fwd_px + bwd_at_dst);
            float max_err = base_thresh + 0.14 * length(fwd_px);
            if (fb_err <= max_err) {
                float conf = 1.0 - clamp(fb_err / max_err, 0.0, 0.90);
                ivec2 mid_cell = ivec2(round(vec2(gid) + (0.5 * fwd_px) / 8.0));
                splat_cell(mid_cell, pack_mid_mv(fwd_px, conf, v_global_px));
            }
        }

        // Validate & splat backward flow (curr -> prev), stored oriented prev -> curr (-bwd).
        vec2 bwd_px = vec2(texelFetch(u_bwd_flow, gid, 0).xy) * 0.25;
        vec2 dst_b = vec2(gid) + bwd_px / 8.0;
        if (in_grid(dst_b, max_g)) {
            vec2 fwd_at_dst = sample_flow_edge_safe(u_fwd_flow, dst_b, max_g);
            float fb_err = length(bwd_px + fwd_at_dst);
            float max_err = base_thresh + 0.14 * length(bwd_px);
            if (fb_err <= max_err) {
                float conf = 1.0 - clamp(fb_err / max_err, 0.0, 0.90);
                ivec2 mid_cell = ivec2(round(vec2(gid) + (0.5 * bwd_px) / 8.0));
                splat_cell(mid_cell, pack_mid_mv(-bwd_px, conf, v_global_px));
            }
        }
        return;
    }

    // u_mode == 2: Foreground-aware hybrid unpack + dense 2-iter fixed-point midpoint gather.
    vec2 vf_local = vec2(texelFetch(u_fwd_flow, gid, 0).xy) * 0.25;
    vec2 vb_local = vec2(texelFetch(u_bwd_flow, gid, 0).xy) * 0.25;
    vec2 v_local = 0.5 * (vf_local - vb_local);
    float local_fb = length(vf_local + vb_local);
    float local_max_err = base_thresh + 0.14 * length(v_local);
    bool local_consistent = (local_fb <= local_max_err);

    uint packed_mv = imageLoad(u_mid_packed, gid).r;
    bool has_splat = (packed_mv != 0u);
    vec2 v_splat = has_splat ? unpack_mid_mv_px(packed_mv) : v_local;

    // Foreground character protection:
    // During fast camera spins where `|v_global_px| >= head_width`, background blocks leapfrog
    // cleanly behind the character's head (visible in both prev & curr!) and splat `v_global_px`
    // right onto the character's face/cap. Protect foreground blocks when BOTH `vf_local` and
    // `-vb_local` are far from `v_global_px` AND either:
    //   (a) `vf_local` & `-vb_local` roughly agree (`local_fb <= 32px`), OR
    //   (b) at least one of `vf_local` / `vb_local` is itself a cycle-consistent foreground vector
    //       (e.g. a rapidly turning face or shadow-crossed cap where one direction had an outlier).
    float bg_speed = length(v_global_px);
    float fg_sep = max(18.0, 0.35 * bg_speed);
    bool both_far_from_bg = (bg_speed > 20.0)
        && (length(vf_local - v_global_px) > fg_sep)
        && (length(-vb_local - v_global_px) > fg_sep);
    bool one_far_from_bg = (bg_speed > 20.0)
        && ((length(vf_local - v_global_px) > fg_sep)
            || (length(-vb_local - v_global_px) > fg_sep));

    bool fwd_fg_valid = false;
    bool bwd_fg_valid = false;
    if (both_far_from_bg && local_fb > max(local_max_err, 32.0)) {
        vec2 dst_f = vec2(gid) + vf_local / 8.0;
        if (in_grid(dst_f, max_g)) {
            vec2 vb_at_f = sample_flow_bilin(u_bwd_flow, dst_f, max_g);
            float f_err = length(vf_local + vb_at_f);
            fwd_fg_valid = (f_err <= base_thresh + 0.16 * length(vf_local));
        }
        vec2 dst_b = vec2(gid) + vb_local / 8.0;
        if (in_grid(dst_b, max_g)) {
            vec2 vf_at_b = sample_flow_bilin(u_fwd_flow, dst_b, max_g);
            float b_err = length(vb_local + vf_at_b);
            bwd_fg_valid = (b_err <= base_thresh + 0.16 * length(vb_local));
        }
    }

    bool both_local_fg = both_far_from_bg
        && ((local_fb <= max(local_max_err, 32.0)) || fwd_fg_valid || bwd_fg_valid);
    vec2 fg_mv;
    if (local_consistent) {
        fg_mv = v_local;
    } else if (fwd_fg_valid && !bwd_fg_valid) {
        fg_mv = vf_local;
    } else if (bwd_fg_valid && !fwd_fg_valid) {
        fg_mv = -vb_local;
    } else {
        fg_mv = (dot(vf_local, vf_local) <= dot(vb_local, vb_local)) ? vf_local : -vb_local;
    }
    bool splat_is_background = has_splat
        && (length(v_splat - v_global_px) + 6.0 < length(fg_mv - v_global_px));
    if (both_local_fg && (!has_splat || splat_is_background)) {
        float fg_conf = local_consistent
            ? (1.0 - clamp(local_fb / local_max_err, 0.0, 0.85))
            : ((fwd_fg_valid || bwd_fg_valid) ? 0.75 : 0.60);
        write_mid_block(gid, fg_mv, fg_conf, true, v_global_px);
        return;
    }
    // 4-way enclosed leapfrog guard: if ONE of `vf_local`/`-vb_local` fell into a dark
    // background match (e.g. black sunglasses matching a black background window `-114px` away)
    // while the other is solid foreground, block the leapfrogging background splat ONLY when:
    //   1. The background-like direction (`other_mv`) is NOT cycle-consistent moving background
    //      (otherwise `gid` is an exterior occluded cloud/sky strip right beside hair!), AND
    //   2. `gid` is enclosed on all 4 cardinal sides in the foreground direction AND on >= 3
    //      cardinal sides in the OTHER flow map (skipping the tiny sunglasses patch) so exterior
    //      occluded sky strips next to a head can NEVER grow `conf = 0.65` tumors into the sky!
    if (one_far_from_bg && (!has_splat || length(v_splat - v_global_px) <= fg_sep)) {
        bool use_fwd = (length(vf_local - v_global_px) > length(-vb_local - v_global_px));
        vec2 cand_fg = use_fwd ? vf_local : -vb_local;
        vec2 other_mv = use_fwd ? -vb_local : vf_local;

        // 1. Cycle-check `other_mv`: if the background-like endpoint vector actually validates
        //    against the opposite flow map (e.g. clean -98 px cloud in BWD right beside hair!),
        //    this block is genuinely moving background, NOT a leapfrog face/sunglasses hole!
        bool other_is_valid_bg = false;
        vec2 other_dst = use_fwd
            ? (vec2(gid) + vb_local / 8.0)
            : (vec2(gid) + vf_local / 8.0);
        if (in_grid(other_dst, max_g)) {
            vec2 opp = use_fwd
                ? sample_flow_edge_safe(u_fwd_flow, other_dst, max_g)
                : (-sample_flow_edge_safe(u_bwd_flow, other_dst, max_g));
            float other_err = length(other_mv - opp);
            float other_lim = base_thresh + 0.16 * length(other_mv);
            other_is_valid_bg = (other_err <= other_lim);
        }

        if (!other_is_valid_bg) {
            vec2 enc_sum = vec2(0.0);
            float enc_wsum = 0.0;
            int enc_l = 0;
            int enc_r = 0;
            int enc_u = 0;
            int enc_d = 0;
            int opp_fg_sides = 0;
            vec2 bg_dir_e = v_global_px / max(bg_speed, 1.0);
            ivec2 rdirs[4];
            rdirs[0] = ivec2(-1,  0);
            rdirs[1] = ivec2( 1,  0);
            rdirs[2] = ivec2( 0, -1);
            rdirs[3] = ivec2( 0,  1);
            for (int r = 0; r < 4; ++r) {
                bool opp_hit = false;
                for (int step = 1; step <= 4; ++step) {
                    ivec2 nc = clamp(gid + rdirs[r] * step, ivec2(0), max_g);
                    vec2 nmv = use_fwd
                        ? (vec2(texelFetch(u_fwd_flow, nc, 0).xy) * 0.25)
                        : (-vec2(texelFetch(u_bwd_flow, nc, 0).xy) * 0.25);
                    if (dot(nmv, bg_dir_e) < 0.30 * bg_speed && length(nmv - cand_fg) <= 32.0) {
                        float w = 1.0 / float(step);
                        enc_sum += nmv * w;
                        enc_wsum += w;
                        if (r == 0) enc_l += 1;
                        else if (r == 1) enc_r += 1;
                        else if (r == 2) enc_u += 1;
                        else enc_d += 1;
                    }
                }
                for (int step = 1; step <= 6; ++step) {
                    ivec2 nc = clamp(gid + rdirs[r] * step, ivec2(0), max_g);
                    vec2 omv = use_fwd
                        ? (-vec2(texelFetch(u_bwd_flow, nc, 0).xy) * 0.25)
                        : (vec2(texelFetch(u_fwd_flow, nc, 0).xy) * 0.25);
                    if (dot(omv, bg_dir_e) < 0.30 * bg_speed && length(omv - cand_fg) <= 32.0) {
                        opp_hit = true;
                        break;
                    }
                }
                if (opp_hit) {
                    opp_fg_sides += 1;
                }
            }
            if (enc_wsum > 0.0 && enc_l >= 2 && enc_r >= 2 && enc_u >= 2 && enc_d >= 2 && opp_fg_sides >= 3) {
                vec2 enc_avg = enc_sum / enc_wsum;
                vec2 out_fg = (length(cand_fg - enc_avg) <= 20.0) ? cand_fg : enc_avg;
                write_mid_block(gid, out_fg, 0.65, true, v_global_px);
                return;
            }
        }
    }
    if (local_consistent && splat_is_background) {
        float fg_conf = 1.0 - clamp(local_fb / local_max_err, 0.0, 0.85);
        write_mid_block(gid, v_local, fg_conf, true, v_global_px);
        return;
    }

    vec2 v_cur = has_splat ? v_splat : v_local;
    bool converged = false;
    float final_conf = 0.0;
    for (int iter = 0; iter < 2; ++iter) {
        vec2 p_prev = vec2(gid) - (0.5 * v_cur) / 8.0;
        vec2 p_curr = vec2(gid) + (0.5 * v_cur) / 8.0;
        bool in_p = in_grid(p_prev, max_g);
        bool in_c = in_grid(p_curr, max_g);

        if (in_p && in_c) {
            vec2 vf = sample_flow_edge_safe(u_fwd_flow, p_prev, max_g);
            vec2 vb = -sample_flow_edge_safe(u_bwd_flow, p_curr, max_g);
            vec2 v_next = 0.5 * (vf + vb);
            float err = length(vf - vb);
            float step_delta = length(v_next - v_cur);
            float avg_speed = 0.5 * (length(vf) + length(vb));
            float max_err = base_thresh + 0.16 * avg_speed;
            v_cur = v_next;
            // Require genuine fixed-point convergence (`step_delta <= max_err`) AND independent
            // endpoint cycle consistency at `p_prev` & `p_curr` so occluded garbage flow beside
            // character silhouettes can NEVER falsely lock into a half-speed smear ring!
            if (err <= max_err && step_delta <= max_err) {
                vec2 p_prev_dst = p_prev + vf / 8.0;
                vec2 p_curr_dst = p_curr - vb / 8.0;
                bool ep_ok = true;
                if (in_grid(p_prev_dst, max_g)) {
                    vec2 b_chk = -sample_flow_edge_safe(u_bwd_flow, p_prev_dst, max_g);
                    if (length(vf - b_chk) > max_err * 1.25) {
                        ep_ok = false;
                    }
                }
                if (ep_ok && in_grid(p_curr_dst, max_g)) {
                    vec2 f_chk = sample_flow_edge_safe(u_fwd_flow, p_curr_dst, max_g);
                    if (length(vb - f_chk) > max_err * 1.25) {
                        ep_ok = false;
                    }
                }
                if (ep_ok) {
                    converged = true;
                    final_conf = 1.0 - clamp(err / max_err, 0.0, 0.85);
                } else {
                    converged = false;
                    final_conf = 0.0;
                }
            } else {
                converged = false;
                final_conf = 0.0;
            }
        } else if (in_c && !in_p) {
            vec2 vb = -sample_flow_edge_safe(u_bwd_flow, p_curr, max_g);
            float max_err = base_thresh + 0.16 * length(vb);
            converged = (length(vb - v_cur) <= max_err);
            v_cur = vb;
            final_conf = converged ? 0.72 : 0.0;
        } else if (in_p && !in_c) {
            vec2 vf = sample_flow_edge_safe(u_fwd_flow, p_prev, max_g);
            float max_err = base_thresh + 0.16 * length(vf);
            converged = (length(vf - v_cur) <= max_err);
            v_cur = vf;
            final_conf = converged ? 0.72 : 0.0;
        } else {
            converged = false;
            final_conf = 0.0;
            break;
        }
    }

    if (converged) {
        write_mid_block(gid, v_cur, final_conf, true, v_global_px);
        return;
    }

    if (has_splat) {
        float sc = unpack_mid_conf(packed_mv);
        write_mid_block(gid, v_splat, sc * 0.75, true, v_global_px);
        return;
    }

    // Depth-preserving hole & disocclusion halo fill:
    // 1. Interior foreground hole fill:
    //    (a) Wide foreground interiors (face/cap/torso): zero background splats within +/-2 blocks
    //        and foreground splats bracketing on opposite sides within +/-3 blocks.
    //    (b) Thin vertical limbs (walking calves/ankles/arms): coherent foreground splats directly
    //        above & below within +/-2 blocks (`|dx| <= 1`) AND local fwd/bwd flow agrees with that
    //        limb (never fires in empty asphalt gaps between two legs!).
    // 2. Exterior disocclusion halo fill: spatially-weighted local background consensus (`1/(1+d^2)`)
    //    so local ground/midground parallax surfaces always win over stray distant sky spikes,
    //    with same-scanline horizontal probes before falling back to `v_global_px`.
    vec2 bg_sum = vec2(0.0);
    float bg_wsum = 0.0;
    vec2 bg_dir = (bg_speed > 1.0) ? (v_global_px / bg_speed) : vec2(1.0, 0.0);

    vec2 fg_sum = vec2(0.0);
    float fg_wsum = 0.0;
    bool has_fg_l = false;
    bool has_fg_r = false;
    bool has_fg_u = false;
    bool has_fg_d = false;
    vec2 fg_u2_sum = vec2(0.0);
    float fg_u2_w = 0.0;
    vec2 fg_d2_sum = vec2(0.0);
    float fg_d2_w = 0.0;

    for (int dy = -3; dy <= 3; ++dy) {
        for (int dx = -3; dx <= 3; ++dx) {
            if (dx == 0 && dy == 0) {
                continue;
            }
            ivec2 nc = clamp(gid + ivec2(dx, dy), ivec2(0), max_g);
            uint np = imageLoad(u_mid_packed, nc).r;
            if (np != 0u) {
                vec2 nmv = unpack_mid_mv_px(np);
                bool is_bg_cand = (bg_speed > 16.0)
                    ? (dot(nmv, bg_dir) >= 0.30 * bg_speed)
                    : (length(nmv - v_global_px) <= 24.0);
                float d2 = float(dx * dx + dy * dy);
                float w = 1.0 / (1.0 + d2);
                if (is_bg_cand) {
                    if (abs(dx) <= 2 && abs(dy) <= 2) {
                        bg_sum += nmv * w;
                        bg_wsum += w;
                    }
                } else {
                    fg_sum += nmv * w;
                    fg_wsum += w;
                    // Strict axial bracketing (`dy == 0` for L/R, `dx == 0` for U/D): a block
                    // sitting outside a convex character head/hair edge can NEVER falsely satisfy
                    // `has_fg_u && has_fg_d` via off-axis hair blocks to its left/right!
                    if (dx < 0 && dy == 0) has_fg_l = true;
                    if (dx > 0 && dy == 0) has_fg_r = true;
                    if (dy < 0 && dx == 0) has_fg_u = true;
                    if (dy > 0 && dx == 0) has_fg_d = true;
                    if (abs(dx) <= 1 && dy >= -2 && dy < 0) {
                        fg_u2_sum += nmv * w;
                        fg_u2_w += w;
                    }
                    if (abs(dx) <= 1 && dy > 0 && dy <= 2) {
                        fg_d2_sum += nmv * w;
                        fg_d2_w += w;
                    }
                }
            }
        }
    }

    if (bg_wsum <= 0.0 && fg_wsum > 0.0 && (one_far_from_bg || bg_speed <= 20.0)
        && ((has_fg_l && has_fg_r) || (has_fg_u && has_fg_d))) {
        vec2 fg_fill = fg_sum / fg_wsum;
        write_mid_block(gid, fg_fill, 0.55, true, v_global_px);
        return;
    }

    if (fg_u2_w > 0.0 && fg_d2_w > 0.0) {
        vec2 u_avg = fg_u2_sum / fg_u2_w;
        vec2 d_avg = fg_d2_sum / fg_d2_w;
        if (length(u_avg - d_avg) <= 18.0) {
            vec2 limb_avg = 0.5 * (u_avg + d_avg);
            float local_limb_err = min(length(vf_local - limb_avg), length(-vb_local - limb_avg));
            if (local_limb_err <= 22.0) {
                write_mid_block(gid, limb_avg, 0.50, true, v_global_px);
                return;
            }
        }
    }

    // 4-ray topological foreground enclosure check (up to 14 blocks = 112px):
    // Wide smooth dark-shirt / cast-shadow interior holes inside a 3rd-person character's torso
    // or head cannot be spanned by +/-3 blocks alone, causing them to fill with fast `v_global_px`
    // background velocity and tear giant background patches onto the character's back.
    // Shoot 4 cardinal rays (-X, +X, -Y, +Y) to the first splatted block in each direction:
    // - True exterior disocclusion halos ALWAYS see open moving background on >= 2 rays (never
    //   enclosed on 3-4 sides by coherent foreground!).
    // - Interior torso/head shadow holes hit coherent foreground on >= 3 cardinal rays (or both
    //   -X & +X within 10 blocks with no close background above/below).
    if (bg_speed > 16.0) {
        ivec2 tdirs[4];
        tdirs[0] = ivec2(-1,  0);
        tdirs[1] = ivec2( 1,  0);
        tdirs[2] = ivec2( 0, -1);
        tdirs[3] = ivec2( 0,  1);
        bool ray_is_fg[4];
        bool ray_is_bg[4];
        int ray_dist[4];
        vec2 ray_mv[4];
        int fg_ray_hits = 0;
        vec2 fg_ray_sum = vec2(0.0);
        float fg_ray_wsum = 0.0;
        for (int r = 0; r < 4; ++r) {
            ray_is_fg[r] = false;
            ray_is_bg[r] = false;
            ray_dist[r] = 99;
            ray_mv[r] = vec2(0.0);
            for (int step = 1; step <= 14; ++step) {
                ivec2 rc = gid + tdirs[r] * step;
                if (rc.x < 0 || rc.y < 0 || rc.x > max_g.x || rc.y > max_g.y) {
                    break;
                }
                uint rp = imageLoad(u_mid_packed, rc).r;
                if (rp != 0u) {
                    vec2 rmv = unpack_mid_mv_px(rp);
                    bool is_bg_r = (dot(rmv, bg_dir) >= 0.30 * bg_speed);
                    ray_dist[r] = step;
                    ray_mv[r] = rmv;
                    if (is_bg_r) {
                        ray_is_bg[r] = true;
                    } else {
                        ray_is_fg[r] = true;
                        fg_ray_hits += 1;
                        float rw = 1.0 / float(step);
                        fg_ray_sum += rmv * rw;
                        fg_ray_wsum += rw;
                    }
                    break;
                }
            }
        }
        bool horiz_enc = ray_is_fg[0] && ray_is_fg[1]
            && ray_dist[0] <= 10 && ray_dist[1] <= 10
            && !(ray_is_bg[2] && ray_dist[2] <= 4)
            && !(ray_is_bg[3] && ray_dist[3] <= 4);
        bool vert_enc = ray_is_fg[2] && ray_is_fg[3]
            && ray_dist[2] <= 8 && ray_dist[3] <= 8
            && !(ray_is_bg[0] && ray_dist[0] <= 6)
            && !(ray_is_bg[1] && ray_dist[1] <= 6);
        if ((fg_ray_hits >= 3 || horiz_enc || vert_enc) && fg_ray_wsum > 1e-4) {
            vec2 fg_avg = fg_ray_sum / fg_ray_wsum;
            float max_fg_dev = 0.0;
            for (int r = 0; r < 4; ++r) {
                if (ray_is_fg[r]) {
                    max_fg_dev = max(max_fg_dev, length(ray_mv[r] - fg_avg));
                }
            }
            if (max_fg_dev <= 28.0) {
                write_mid_block(gid, fg_avg, 0.45, true, v_global_px);
                return;
            }
        }
    }

    if (bg_wsum <= 0.0) {
        for (int dy = -5; dy <= 5; ++dy) {
            for (int dx = -5; dx <= 5; ++dx) {
                if (abs(dx) <= 2 && abs(dy) <= 2) {
                    continue;
                }
                ivec2 nc = clamp(gid + ivec2(dx, dy), ivec2(0), max_g);
                uint np = imageLoad(u_mid_packed, nc).r;
                if (np != 0u) {
                    vec2 nmv = unpack_mid_mv_px(np);
                    bool is_bg_cand = (bg_speed > 16.0)
                        ? (dot(nmv, bg_dir) >= 0.30 * bg_speed)
                        : (length(nmv - v_global_px) <= 24.0);
                    if (is_bg_cand) {
                        float w = 1.0 / (1.0 + float(dx * dx + dy * dy));
                        bg_sum += nmv * w;
                        bg_wsum += w;
                    }
                }
            }
        }
    }
    if (bg_wsum <= 0.0) {
        for (int dy = -8; dy <= 8; dy += 2) {
            for (int dx = -8; dx <= 8; dx += 2) {
                if (abs(dx) <= 5 && abs(dy) <= 5) {
                    continue;
                }
                ivec2 nc = clamp(gid + ivec2(dx, dy), ivec2(0), max_g);
                uint np = imageLoad(u_mid_packed, nc).r;
                if (np != 0u) {
                    vec2 nmv = unpack_mid_mv_px(np);
                    bool is_bg_cand = (bg_speed > 16.0)
                        ? (dot(nmv, bg_dir) >= 0.30 * bg_speed)
                        : (length(nmv - v_global_px) <= 24.0);
                    if (is_bg_cand) {
                        float w = 1.0 / (1.0 + float(dx * dx + dy * dy));
                        bg_sum += nmv * w;
                        bg_wsum += w;
                    }
                }
            }
        }
    }
    if (bg_wsum <= 0.0) {
        int h_steps[6];
        h_steps[0] = -12;
        h_steps[1] =  12;
        h_steps[2] = -20;
        h_steps[3] =  20;
        h_steps[4] = -32;
        h_steps[5] =  32;
        for (int i = 0; i < 6; ++i) {
            ivec2 nc = clamp(gid + ivec2(h_steps[i], 0), ivec2(0), max_g);
            uint np = imageLoad(u_mid_packed, nc).r;
            if (np != 0u) {
                vec2 nmv = unpack_mid_mv_px(np);
                bool is_bg_cand = (bg_speed > 16.0)
                    ? (dot(nmv, bg_dir) >= 0.20 * bg_speed)
                    : true;
                if (is_bg_cand) {
                    float w = 1.0 / float(abs(h_steps[i]));
                    bg_sum += nmv * w;
                    bg_wsum += w;
                }
            }
        }
    }

    vec2 best_mv = (bg_wsum > 1e-4) ? (bg_sum / bg_wsum) : v_global_px;
    write_mid_block(gid, best_mv, 0.0, false, v_global_px);
}
