#version 430 core
// AMD FidelityFX Optical Flow — Vector Median Filter, Edge-Preserving 2x Upscale,
// 5x5 Edge-Preserving Spatial Coherence Filter, and Smooth Gaussian Aux Matte Blur.
//
// u_mode == 0: 3x3 vector median filter (eliminates outlier spikes without diagonal Franken-vectors).
// u_mode == 1: Edge-preserving 2x pyramid upscale to the next finer level (exact 2:1 alignment).
// u_mode == 2: 5x5 strictly edge-preserving bilateral coherence filter for the midpoint vector field.
// u_mode == 3: 5x5 Gaussian blur for the smooth continuous RGBA8 occlusion/foreground/conf matte.

layout(local_size_x = 8, local_size_y = 8, local_size_z = 1) in;

layout(binding = 0) uniform isampler2D u_in_flow;
layout(binding = 1) uniform sampler2D u_in_conf;
layout(rg16i, binding = 0) writeonly uniform iimage2D u_out_flow;
layout(rgba8, binding = 1) writeonly uniform image2D u_out_aux;

uniform ivec2 u_in_size;
uniform ivec2 u_out_size;
uniform int u_mode;

// 3x3 Vector Median Filter: selects the actual neighbor motion vector with minimum total
// Euclidean distance to the other 8 neighbors. Unlike component-wise median, this NEVER
// synthesizes bogus Franken-vectors `(x_from_fg, y_from_bg)` along diagonal camera pans.
ivec2 vector_median9(ivec2 v[9]) {
    vec2 vf[9];
    for (int i = 0; i < 9; ++i) {
        vf[i] = vec2(v[i]);
    }
    int best_i = 4;
    float best_cost = 1e30;
    for (int i = 0; i < 9; ++i) {
        float cost = 0.0;
        for (int j = 0; j < 9; ++j) {
            cost += length(vf[i] - vf[j]);
        }
        if (i == 4) {
            cost -= 0.01;
        }
        if (cost < best_cost) {
            best_cost = cost;
            best_i = i;
        }
    }
    return v[best_i];
}

void main() {
    ivec2 gid = ivec2(gl_GlobalInvocationID.xy);
    if (gid.x >= u_out_size.x || gid.y >= u_out_size.y) {
        return;
    }

    ivec2 max_in = max(u_in_size - ivec2(1), ivec2(0));

    if (u_mode == 0) {
        ivec2 vs[9];
        int k = 0;
        for (int dy = -1; dy <= 1; ++dy) {
            for (int dx = -1; dx <= 1; ++dx) {
                ivec2 c = clamp(gid + ivec2(dx, dy), ivec2(0), max_in);
                vs[k] = texelFetch(u_in_flow, c, 0).xy;
                k += 1;
            }
        }
        ivec2 med = vector_median9(vs);
        imageStore(u_out_flow, gid, ivec4(med, 0, 0));
    } else if (u_mode == 1) {
        vec2 coarse_f = (vec2(gid) + 0.5) * 0.5 - 0.5;
        ivec2 base = ivec2(floor(coarse_f));
        vec2 frac = clamp(coarse_f - vec2(base), 0.0, 1.0);

        ivec2 c00 = clamp(base + ivec2(0, 0), ivec2(0), max_in);
        ivec2 c10 = clamp(base + ivec2(1, 0), ivec2(0), max_in);
        ivec2 c01 = clamp(base + ivec2(0, 1), ivec2(0), max_in);
        ivec2 c11 = clamp(base + ivec2(1, 1), ivec2(0), max_in);

        vec2 v00 = vec2(texelFetch(u_in_flow, c00, 0).xy);
        vec2 v10 = vec2(texelFetch(u_in_flow, c10, 0).xy);
        vec2 v01 = vec2(texelFetch(u_in_flow, c01, 0).xy);
        vec2 v11 = vec2(texelFetch(u_in_flow, c11, 0).xy);

        vec2 v_min = min(min(v00, v10), min(v01, v11));
        vec2 v_max = max(max(v00, v10), max(v01, v11));
        vec2 spread = v_max - v_min;

        vec2 up;
        if (max(spread.x, spread.y) <= 8.0) {
            vec2 top = mix(v00, v10, frac.x);
            vec2 bot = mix(v01, v11, frac.x);
            up = mix(top, bot, frac.y);
        } else {
            ivec2 nearest = clamp(ivec2(round(coarse_f)), ivec2(0), max_in);
            up = vec2(texelFetch(u_in_flow, nearest, 0).xy);
        }

        ivec2 scaled = clamp(ivec2(round(up * 2.0)), ivec2(-1280), ivec2(1280));
        imageStore(u_out_flow, gid, ivec4(scaled, 0, 0));
    } else if (u_mode == 4) {
        // 3x3 vector median + wide foreground interior shadow-hole seal (up to 16 blocks = 128px):
        // Smooth dark t-shirts / cast shadows on a 3rd-person character's back can create 6-12 block
        // low-confidence holes that Pass 2 filled with fast background velocity. Casting 4 cardinal
        // rays BEFORE Pass 4c's island-seal + bilateral pass seals the entire hole smoothly with
        // zero leftover checkerboard specks!
        ivec2 vs[9];
        int k = 0;
        for (int dy = -1; dy <= 1; ++dy) {
            for (int dx = -1; dx <= 1; ++dx) {
                ivec2 c = clamp(gid + ivec2(dx, dy), ivec2(0), max_in);
                vs[k] = texelFetch(u_in_flow, c, 0).xy;
                k += 1;
            }
        }
        vec2 center_mv = vec2(vector_median9(vs));
        float center_conf = texelFetch(u_in_conf, clamp(gid, ivec2(0), max_in), 0).r;
        float center_spd = length(center_mv);
        if (center_conf < 0.35 && center_spd > 100.0) {
            ivec2 dirs[4];
            dirs[0] = ivec2(-1,  0);
            dirs[1] = ivec2( 1,  0);
            dirs[2] = ivec2( 0, -1);
            dirs[3] = ivec2( 0,  1);
            bool fg_hit[4];
            bool bg_open[4];
            int fg_step[4];
            vec2 fg_mv[4];
            int fg_cnt = 0;
            for (int r = 0; r < 4; ++r) {
                fg_hit[r] = false;
                bg_open[r] = false;
                fg_step[r] = 99;
                fg_mv[r] = vec2(0.0);
                for (int step = 1; step <= 16; ++step) {
                    ivec2 rc = gid + dirs[r] * step;
                    if (rc.x < 0 || rc.y < 0 || rc.x > max_in.x || rc.y > max_in.y) {
                        break;
                    }
                    vec2 rmv = vec2(texelFetch(u_in_flow, rc, 0).xy);
                    float rconf = texelFetch(u_in_conf, rc, 0).r;
                    float rspd = length(rmv);
                    if (rconf >= 0.20 && rspd + 48.0 < center_spd && length(rmv - center_mv) > 72.0) {
                        fg_hit[r] = true;
                        fg_step[r] = step;
                        fg_mv[r] = rmv;
                        fg_cnt += 1;
                        break;
                    }
                    if (rconf >= 0.65 && length(rmv - center_mv) <= 48.0) {
                        bg_open[r] = true;
                        fg_step[r] = step;
                        break;
                    }
                }
            }
            bool h_pair = fg_hit[0] && fg_hit[1] && fg_step[0] <= 14 && fg_step[1] <= 14
                && length(fg_mv[0] - fg_mv[1]) <= 128.0
                && !(bg_open[2] && fg_step[2] <= 4)
                && !(bg_open[3] && fg_step[3] <= 4);
            bool v_pair = fg_hit[2] && fg_hit[3] && fg_step[2] <= 10 && fg_step[3] <= 10
                && (fg_step[2] + fg_step[3] <= 14)
                && length(fg_mv[2] - fg_mv[3]) <= 128.0
                && !(bg_open[0] && fg_step[0] <= 6)
                && !(bg_open[1] && fg_step[1] <= 6);
            if (h_pair) {
                float w0 = 1.0 / float(fg_step[0]);
                float w1 = 1.0 / float(fg_step[1]);
                center_mv = (fg_mv[0] * w0 + fg_mv[1] * w1) / (w0 + w1);
            } else if (v_pair) {
                float w2 = 1.0 / float(fg_step[2]);
                float w3 = 1.0 / float(fg_step[3]);
                center_mv = (fg_mv[2] * w2 + fg_mv[3] * w3) / (w2 + w3);
            } else if (fg_cnt >= 3) {
                float best_d = 1e9;
                int bi = -1;
                int bj = -1;
                for (int i = 0; i < 4; ++i) {
                    if (!fg_hit[i]) continue;
                    for (int j = i + 1; j < 4; ++j) {
                        if (!fg_hit[j]) continue;
                        float d = length(fg_mv[i] - fg_mv[j]);
                        if (d < best_d) {
                            best_d = d;
                            bi = i;
                            bj = j;
                        }
                    }
                }
                if (bi >= 0 && best_d <= 112.0) {
                    float wi = 1.0 / float(fg_step[bi]);
                    float wj = 1.0 / float(fg_step[bj]);
                    center_mv = (fg_mv[bi] * wi + fg_mv[bj] * wj) / (wi + wj);
                }
            }
        }
        ivec2 out_mv = clamp(ivec2(round(center_mv)), ivec2(-1280), ivec2(1280));
        imageStore(u_out_flow, gid, ivec4(out_mv, 0, 0));
    } else if (u_mode == 2) {
        // 4-ray cardinal island-seal + 5x5 strictly edge-preserving bilateral coherence filter:
        // 1. If a small isolated island (<= 7x7 blocks, e.g. black sunglasses matching a black
        //    background window -114px away, or a dark knee hole) is enclosed on all 4 cardinal
        //    rays (-X, +X, -Y, +Y within 4 blocks) by ONE coherent surrounding layer (`spread < 28px`,
        //    differing from `center_mv` by > 36px), replace `center_mv` with the enclosing layer.
        //    Zero corner rounding on true objects or silhouettes!
        // 2. Smooth within each motion layer via 5x5 bilateral filter without bleeding across edges.
        vec2 center_mv = vec2(texelFetch(u_in_flow, clamp(gid, ivec2(0), max_in), 0).xy);

        ivec2 dirs[4];
        dirs[0] = ivec2(-1,  0);
        dirs[1] = ivec2( 1,  0);
        dirs[2] = ivec2( 0, -1);
        dirs[3] = ivec2( 0,  1);
        vec2 hit_mv[4];
        int hit_step[4];
        bool all_hit = true;
        for (int r = 0; r < 4; ++r) {
            bool found = false;
            for (int step = 1; step <= 4; ++step) {
                ivec2 rc = clamp(gid + dirs[r] * step, ivec2(0), max_in);
                vec2 rmv = vec2(texelFetch(u_in_flow, rc, 0).xy);
                // 80 quarter-px = 20 px velocity jump
                if (length(rmv - center_mv) > 80.0) {
                    hit_mv[r] = rmv;
                    hit_step[r] = step;
                    found = true;
                    break;
                }
            }
            if (!found) {
                all_hit = false;
                break;
            }
        }
        if (all_hit) {
            vec2 avg_hit = 0.25 * (hit_mv[0] + hit_mv[1] + hit_mv[2] + hit_mv[3]);
            float max_dev = 0.0;
            for (int r = 0; r < 4; ++r) {
                max_dev = max(max_dev, length(hit_mv[r] - avg_hit));
            }
            float jump = length(avg_hit - center_mv);
            // Total island span along X & Y is `(step_neg + step_pos - 1)`:
            // - `span_x <= 4 && span_y <= 4` matches ONLY tiny <=3x3-block specks (e.g. thin 3px
            //   cursor reticle arcs embedded in a uniform panning map) uniformly without leaving
            //   hollow diamonds, while NEVER eroding character heads (~10x12), walking legs (~3x15),
            //   or wide UI panels.
            // - Fast background leapfrog holes (`length(center_mv) > length(avg_hit) + 28px`) inside
            //   slower foreground faces/sunglasses seal up to <=3 steps in each ray.
            bool is_tiny_speck = (hit_step[0] + hit_step[1] <= 4)
                && (hit_step[2] + hit_step[3] <= 4)
                && (max_dev <= 48.0)
                && (jump > 80.0);
            bool is_fast_fg_hole = (length(center_mv) > length(avg_hit) + 112.0)
                && (max_dev <= 96.0)
                && (jump > 144.0);
            if (is_tiny_speck || is_fast_fg_hole) {
                center_mv = avg_hit;
            }
        }

        vec2 sum_mv = vec2(0.0);
        float sum_w = 0.0;
        for (int dy = -2; dy <= 2; ++dy) {
            for (int dx = -2; dx <= 2; ++dx) {
                ivec2 nc = clamp(gid + ivec2(dx, dy), ivec2(0), max_in);
                vec2 nmv = vec2(texelFetch(u_in_flow, nc, 0).xy);
                float nconf = texelFetch(u_in_conf, nc, 0).r;

                float d2_sp = float(dx * dx + dy * dy);
                float spatial_w = exp(-d2_sp / (2.0 * 2.5));
                // Vector similarity weight (sigma = 20 quarter-px = 5.0 px).
                vec2 diff = nmv - center_mv;
                float range_w = exp(-dot(diff, diff) / (2.0 * 400.0));
                float conf_w = mix(0.25, 1.0, nconf);
                float w = spatial_w * range_w * conf_w;
                sum_mv += nmv * w;
                sum_w += w;
            }
        }
        vec2 smoothed = (sum_w > 1e-4) ? (sum_mv / sum_w) : center_mv;
        ivec2 out_mv = clamp(ivec2(round(smoothed)), ivec2(-1280), ivec2(1280));
        imageStore(u_out_flow, gid, ivec4(out_mv, 0, 0));
    } else {
        // u_mode == 3: 5x5 Gaussian blur on RGBA8 auxiliary matte (occ_prev, occ_curr, fg_mask, conf).
        // Eradicates 8x8 block stair-stepping from occlusion & confidence transitions completely!
        vec4 sum = vec4(0.0);
        float sum_w = 0.0;
        for (int dy = -2; dy <= 2; ++dy) {
            for (int dx = -2; dx <= 2; ++dx) {
                ivec2 nc = clamp(gid + ivec2(dx, dy), ivec2(0), max_in);
                float d2 = float(dx * dx + dy * dy);
                float w = exp(-d2 / (2.0 * 2.0));
                sum += texelFetch(u_in_conf, nc, 0) * w;
                sum_w += w;
            }
        }
        imageStore(u_out_aux, gid, clamp(sum / max(sum_w, 1e-4), vec4(0.0), vec4(1.0)));
    }
}
