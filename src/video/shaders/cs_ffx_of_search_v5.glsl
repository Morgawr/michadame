#version 430 core
// AMD FidelityFX Optical Flow v5 — Coarse-to-fine 8x8 block SAD search pass.
// Computes quarter-pixel (Q13.2) RG16I motion vectors v such that src(p) ~= ref(p + v).
//
// Searches across up to 6 pyramid levels (levels 5 -> 0), covering +/-256px at level 5 alone
// (u_search_radius = 8) plus hierarchical refinement down to quarter-pixel accuracy at level 0.
// Uses in-bounds normalized SAD at screen borders and global coarse-grid seed probes so large
// 3rd-person foreground characters never block camera motion from reaching the screen edges.

layout(local_size_x = 8, local_size_y = 8, local_size_z = 1) in;

layout(binding = 0) uniform sampler2D u_src_luma;
layout(binding = 1) uniform sampler2D u_ref_luma;
layout(binding = 2) uniform isampler2D u_coarse_flow;
layout(rg16i, binding = 0) writeonly uniform iimage2D u_out_flow;

uniform ivec2 u_level_size;
uniform ivec2 u_grid_size;
uniform int u_has_coarse;
uniform int u_search_radius;

// Evaluate 16 bilinear 2x2-averaged samples covering all 64 pixels of the 8x8 block
// (75% raw SAD + 25% zero-mean SAD). Normalizes over in-bounds taps when a block approaches
// the screen edge so true panning motion isn't unfairly crushed by border clamping.
float eval_sad4(vec2 base, vec2 mv, float src4[16], float src_mean, vec2 inv_size) {
    vec2 ref_base = base + mv;
    vec2 level_f = vec2(u_level_size);
    float raw_sad = 0.0;
    float ref4[16];
    float valid[16];
    float ref_sum = 0.0;
    float src_in_sum = 0.0;
    float in_count = 0.0;

    for (int iy = 0; iy < 4; ++iy) {
        float y = ref_base.y + float(iy * 2 + 1);
        for (int ix = 0; ix < 4; ++ix) {
            float x = ref_base.x + float(ix * 2 + 1);
            int idx = iy * 4 + ix;
            bool inside = (x >= 0.0 && y >= 0.0 && x <= level_f.x && y <= level_f.y);
            vec2 uv = clamp(vec2(x, y) * inv_size, 0.5 * inv_size, 1.0 - 0.5 * inv_size);
            float r = texture(u_ref_luma, uv).r;
            ref4[idx] = r;
            if (inside) {
                valid[idx] = 1.0;
                raw_sad += abs(src4[idx] - r);
                ref_sum += r;
                src_in_sum += src4[idx];
                in_count += 1.0;
            } else {
                valid[idx] = 0.0;
            }
        }
    }

    if (in_count < 6.0) {
        // Too far off-screen to be a reliable match; fall back to full clamped SAD + penalty.
        float full_sad = 0.0;
        for (int i = 0; i < 16; ++i) {
            full_sad += abs(src4[i] - ref4[i]);
        }
        vec2 oob_lo = max(vec2(0.0) - ref_base, vec2(0.0));
        vec2 oob_hi = max((ref_base + vec2(8.0)) - level_f, vec2(0.0));
        float oob = (oob_lo.x + oob_lo.y + oob_hi.x + oob_hi.y) * 0.14;
        return full_sad + oob + 0.5;
    }

    float scale = 16.0 / in_count;
    float inv_in = 1.0 / in_count;
    float ref_mean = ref_sum * inv_in;
    float s_mean = (in_count < 15.5) ? (src_in_sum * inv_in) : src_mean;
    float dc_delta = ref_mean - s_mean;
    float zm_sad = 0.0;
    for (int i = 0; i < 16; ++i) {
        zm_sad += valid[i] * abs((src4[i] - s_mean) - (ref4[i] - ref_mean));
    }
    float sad = (0.75 * raw_sad + 0.25 * zm_sad) * scale + abs(dc_delta) * 1.0;

    // Gentle OOB tie-break only (0.025 vs old 0.14) so border panning stays accurate.
    vec2 oob_lo = max(vec2(0.0) - ref_base, vec2(0.0));
    vec2 oob_hi = max((ref_base + vec2(8.0)) - level_f, vec2(0.0));
    float oob = (oob_lo.x + oob_lo.y + oob_hi.x + oob_hi.y) * 0.025;
    return sad + oob;
}

// Exact 8x8 (64-pixel) integer-offset SAD for fine 1px & parabolic sub-pixel refinement.
float eval_sad8(ivec2 base, ivec2 mv) {
    ivec2 max_c = max(u_level_size - ivec2(1), ivec2(0));
    float sad = 0.0;
    for (int iy = 0; iy < 8; ++iy) {
        int sy = min(base.y + iy, max_c.y);
        int ry = clamp(base.y + iy + mv.y, 0, max_c.y);
        for (int ix = 0; ix < 8; ++ix) {
            int sx = min(base.x + ix, max_c.x);
            int rx = clamp(base.x + ix + mv.x, 0, max_c.x);
            float s = texelFetch(u_src_luma, ivec2(sx, sy), 0).r;
            float r = texelFetch(u_ref_luma, ivec2(rx, ry), 0).r;
            sad += abs(s - r);
        }
    }
    vec2 ref_base = vec2(base + mv);
    vec2 oob_lo = max(vec2(0.0) - ref_base, vec2(0.0));
    vec2 oob_hi = max((ref_base + vec2(8.0)) - vec2(u_level_size), vec2(0.0));
    float oob = (oob_lo.x + oob_lo.y + oob_hi.x + oob_hi.y) * 0.18;
    return sad + oob;
}

void main() {
    ivec2 b = ivec2(gl_GlobalInvocationID.xy);
    if (b.x >= u_grid_size.x || b.y >= u_grid_size.y) {
        return;
    }

    ivec2 base_i = b * 8;
    vec2 base = vec2(base_i);
    vec2 inv_size = 1.0 / vec2(max(u_level_size, ivec2(1)));

    // Preload 4x4 bilinear 2x2-box samples of the source 8x8 block.
    float src4[16];
    float s_min = 1.0;
    float s_max = 0.0;
    float s_sum = 0.0;
    for (int iy = 0; iy < 4; ++iy) {
        float y = base.y + float(iy * 2 + 1);
        for (int ix = 0; ix < 4; ++ix) {
            float x = base.x + float(ix * 2 + 1);
            vec2 uv = clamp(vec2(x, y) * inv_size, 0.5 * inv_size, 1.0 - 0.5 * inv_size);
            float v = texture(u_src_luma, uv).r;
            src4[iy * 4 + ix] = v;
            s_min = min(s_min, v);
            s_max = max(s_max, v);
            s_sum += v;
        }
    }
    float src_mean = s_sum * (1.0 / 16.0);
    float contrast = s_max - s_min;

    ivec2 coarse_center = ivec2(0);
    if (u_has_coarse != 0) {
        coarse_center = ivec2(round(vec2(texelFetch(u_coarse_flow, b, 0).xy) * 0.25));
    }

    // Only skip search on completely uniform flat blocks; still verify (0, 0) vs coarse_center
    // ONLY when coarse_center lands on a genuinely mismatched brightness/color!
    if (contrast < 0.0015) {
        ivec2 flat_mv = coarse_center;
        if (coarse_center != ivec2(0)) {
            float c_sad = eval_sad4(base, vec2(coarse_center), src4, src_mean, inv_size);
            float z_sad = eval_sad4(base, vec2(0.0), src4, src_mean, inv_size);
            if (z_sad + 0.08 < c_sad) {
                flat_mv = ivec2(0);
            }
        }
        ivec2 out_q = ivec2(clamp(flat_mv * 4, ivec2(-1280), ivec2(1280)));
        imageStore(u_out_flow, b, ivec4(out_q, 0, 0));
        return;
    }

    // 1. Seed selection:
    //    - Local coarse_center
    //    - (0, 0): wins ONLY if genuinely better than coarse_center (prevents smooth asphalt/sky
    //      from falsely snapping from -116px to 0px on a near-tie!)
    //    - 8 surrounding coarse neighbors (3x3)
    //    - 5 distant global coarse-grid probes so a huge 3rd-person character wall can NEVER
    //      trap the left or right side of the screen at (0, 0)!
    ivec2 best_seed = coarse_center;
    float best_sad = eval_sad4(base, vec2(coarse_center), src4, src_mean, inv_size);

    if (coarse_center != ivec2(0)) {
        float z_sad = eval_sad4(base, vec2(0.0), src4, src_mean, inv_size);
        // Require a genuine improvement to jump from fast coarse motion to (0, 0),
        // while allowing easy tie-break if coarse_center is already slow (<= 16px).
        float z_margin = (length(vec2(coarse_center)) > 16.0) ? 0.035 : -0.006;
        if (z_sad + z_margin < best_sad) {
            best_sad = z_sad;
            best_seed = ivec2(0);
        }
    }

    if (u_has_coarse != 0) {
        ivec2 max_g = max(u_grid_size - ivec2(1), ivec2(0));
        for (int ny = -1; ny <= 1; ++ny) {
            for (int nx = -1; nx <= 1; ++nx) {
                if (nx == 0 && ny == 0) {
                    continue;
                }
                ivec2 nc = clamp(b + ivec2(nx, ny), ivec2(0), max_g);
                ivec2 cand = ivec2(round(vec2(texelFetch(u_coarse_flow, nc, 0).xy) * 0.25));
                if (cand == best_seed) {
                    continue;
                }
                float reg = 0.0004 * length(vec2(cand - coarse_center));
                float sad = eval_sad4(base, vec2(cand), src4, src_mean, inv_size) + reg;
                if (sad < best_sad) {
                    best_sad = sad;
                    best_seed = cand;
                }
            }
        }

        // Depth-preserving same-scanline coarse-grid probes ONLY on coarse levels (`w < 480`,
        // i.e. pyramid levels 3..5): rescues left/right screen borders trapped behind wide 3rd-person
        // characters during fast horizontal spins WITHOUT ever teleporting distant sky velocity
        // vertically down into walking legs, pants, shoes, or swinging hands!
        if (u_level_size.x < 480 && best_sad > 0.24) {
            float y_norm = (float(b.y) + 0.5) / float(max(u_grid_size.y, 1));
            float x_probes[5];
            x_probes[0] = 0.10;
            x_probes[1] = 0.28;
            x_probes[2] = 0.50;
            x_probes[3] = 0.72;
            x_probes[4] = 0.90;
            for (int p = 0; p < 5; ++p) {
                ivec2 pc = clamp(
                    ivec2(floor(vec2(x_probes[p], y_norm) * vec2(u_grid_size))),
                    ivec2(0),
                    max_g
                );
                ivec2 cand = ivec2(round(vec2(texelFetch(u_coarse_flow, pc, 0).xy) * 0.25));
                if (cand == best_seed) {
                    continue;
                }
                float sad = eval_sad4(base, vec2(cand), src4, src_mean, inv_size) + 0.22;
                if (sad < best_sad) {
                    best_sad = sad;
                    best_seed = cand;
                }
            }
        }
    }

    // 2. Stage A: Dense 1-pixel local search around best_seed.
    //    Regularize gently toward `best_seed` (NOT `coarse_center`!) so when a stationary
    //    character block selects `best_seed = (0, 0)`, repeated/smooth hair or armor textures
    //    are NOT dragged toward the fast-spinning background velocity!
    ivec2 stage_a_mv = best_seed;
    float stage_a_sad = eval_sad4(base, vec2(best_seed), src4, src_mean, inv_size);
    int r = clamp(u_search_radius, 2, 8);

    for (int dy = -r; dy <= r; ++dy) {
        for (int dx = -r; dx <= r; ++dx) {
            if (dx == 0 && dy == 0) {
                continue;
            }
            ivec2 cand = best_seed + ivec2(dx, dy);
            float reg = 0.0007 * length(vec2(cand - best_seed));
            float sad = eval_sad4(base, vec2(cand), src4, src_mean, inv_size) + reg;
            if (sad < stage_a_sad) {
                stage_a_sad = sad;
                stage_a_mv = cand;
            }
        }
    }

    if (r == 2) {
        for (int dy = -4; dy <= 4; dy += 2) {
            for (int dx = -4; dx <= 4; dx += 2) {
                if (abs(dx) <= 2 && abs(dy) <= 2) {
                    continue;
                }
                ivec2 cand = best_seed + ivec2(dx, dy);
                float reg = 0.0010 * length(vec2(cand - best_seed));
                float sad = eval_sad4(base, vec2(cand), src4, src_mean, inv_size) + reg;
                if (sad < stage_a_sad) {
                    stage_a_sad = sad;
                    stage_a_mv = cand;
                }
            }
        }
    }

    // 3. Stage B: Exact 8x8 pixel SAD on 3x3 (+/-1px) neighborhood around stage_a_mv.
    float sad3x3[9];
    ivec2 best_int_mv = stage_a_mv;
    int best_idx = 4;
    float best_sad8 = 1e20;
    for (int dy = -1; dy <= 1; ++dy) {
        for (int dx = -1; dx <= 1; ++dx) {
            int idx = (dy + 1) * 3 + (dx + 1);
            ivec2 cand = stage_a_mv + ivec2(dx, dy);
            float raw_sad = eval_sad8(base_i, cand);
            sad3x3[idx] = raw_sad;
            float reg = 0.002 * length(vec2(cand - best_seed));
            float score = raw_sad + reg;
            if (score < best_sad8) {
                best_sad8 = score;
                best_int_mv = cand;
                best_idx = idx;
            }
        }
    }

    // Minimum-motion tie-breaker ONLY for slow/stationary foreground candidates (`<= 12 px`)
    // so smooth/periodic stationary character heads don't drift into inconsistent fwd/bwd shifts,
    // while fast-moving smooth asphalt/sky (`-116 px`) is NEVER falsely snapped to `(0, 0)`!
    if (best_int_mv != ivec2(0)) {
        float zero_sad = eval_sad8(base_i, ivec2(0));
        if (length(vec2(best_int_mv)) <= 12.0) {
            float tie_tol = min(0.08, 0.25 * contrast + 0.002);
            if (zero_sad <= sad3x3[best_idx] + tie_tol) {
                best_int_mv = ivec2(0);
                best_idx = -1;
            }
        } else if (zero_sad + 0.25 < sad3x3[best_idx]) {
            best_int_mv = ivec2(0);
            best_idx = -1;
        }
    }

    // 4. Stage C: Parabolic sub-pixel refinement (quarter-pixel Q13.2 precision).
    vec2 sub_offset = vec2(0.0);
    if (best_idx >= 0) {
        int bx = best_idx % 3;
        int by = best_idx / 3;
        float s_c = sad3x3[best_idx];
        float s_xm = (bx > 0) ? sad3x3[by * 3 + (bx - 1)] : eval_sad8(base_i, best_int_mv + ivec2(-1, 0));
        float s_xp = (bx < 2) ? sad3x3[by * 3 + (bx + 1)] : eval_sad8(base_i, best_int_mv + ivec2( 1, 0));
        float s_ym = (by > 0) ? sad3x3[(by - 1) * 3 + bx] : eval_sad8(base_i, best_int_mv + ivec2(0, -1));
        float s_yp = (by < 2) ? sad3x3[(by + 1) * 3 + bx] : eval_sad8(base_i, best_int_mv + ivec2(0,  1));

        float denom_x = s_xm - 2.0 * s_c + s_xp;
        if (denom_x > 1e-3) {
            sub_offset.x = clamp(0.5 * (s_xm - s_xp) / denom_x, -0.5, 0.5);
        }
        float denom_y = s_ym - 2.0 * s_c + s_yp;
        if (denom_y > 1e-3) {
            sub_offset.y = clamp(0.5 * (s_ym - s_yp) / denom_y, -0.5, 0.5);
        }
    }

    vec2 final_mv_px = clamp(vec2(best_int_mv) + sub_offset, vec2(-320.0), vec2(320.0));
    ivec2 final_mv_q = ivec2(round(final_mv_px * 4.0));
    imageStore(u_out_flow, b, ivec4(final_mv_q, 0, 0));
}
