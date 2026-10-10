#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D current_frame;
uniform sampler2D prev_frame;
uniform sampler2D prev_frame_2;
uniform int has_prev2;

// Modes:
// 0: Motion-Adaptive Spatio-Temporal (Same-Parity / Field-Aware Bob Dejitter)
// 1: Bob Dejitter / Temporal Weave (Pure Inter-Field Weave)
// 2: Vertical FIR (3-Tap Lowpass)
// 3: Vertical Median (3x1)
// 4: Motion Map (Debug Visualization: Green = Static Weave, Magenta = Motion)
uniform int mode;

uniform float blend_amount;
uniform float motion_threshold;
uniform float line_spacing;
uniform float spatial_mix;

vec3 vertical_median(vec3 a, vec3 b, vec3 c) {
    return max(min(a, b), min(max(a, b), c));
}

void main() {
    vec2 tex_size = vec2(textureSize(current_frame, 0));
    // For 480i upscaled to 1080p, nominal scanline spacing is tex_size.y / 480.0 (~2.25 px).
    // Automatically normalize line_spacing so 1.0 always targets exactly 1 field scanline.
    float nominal_spacing = max(tex_size.y / 480.0, 1.0);
    vec2 dy = vec2(0.0, (line_spacing * nominal_spacing) / max(tex_size.y, 1.0));

    vec4 c_curr = texture(current_frame, v_tc);
    vec4 c_curr_top = texture(current_frame, v_tc - dy);
    vec4 c_curr_bot = texture(current_frame, v_tc + dy);

    // 3-tap vertical low-pass FIR [0.25, 0.5, 0.25] on current frame
    vec3 fir_color = 0.25 * c_curr_top.rgb + 0.5 * c_curr.rgb + 0.25 * c_curr_bot.rgb;
    vec3 spatial_fir = mix(c_curr.rgb, fir_color, spatial_mix);

    // Vertical median 3x1 on current frame
    vec3 median_color = vertical_median(c_curr_top.rgb, c_curr.rgb, c_curr_bot.rgb);
    vec3 spatial_med = mix(c_curr.rgb, median_color, spatial_mix);

    // Previous field (t-1) samples
    vec4 c_prev = texture(prev_frame, v_tc);
    vec4 c_prev_top = texture(prev_frame, v_tc - dy);
    vec4 c_prev_bot = texture(prev_frame, v_tc + dy);

    // Temporal blend (Field weave / Bob dejitter)
    vec3 temporal_blend = mix(c_curr.rgb, c_prev.rgb, blend_amount);

    if (mode == 1) {
        // Pure Bob Dejitter / Field Weave
        out_color = vec4(temporal_blend, c_curr.a);
        return;
    } else if (mode == 2) {
        // Pure Vertical FIR
        out_color = vec4(spatial_fir, c_curr.a);
        return;
    } else if (mode == 3) {
        // Pure Vertical Median
        out_color = vec4(spatial_med, c_curr.a);
        return;
    }

    // --- Mode 0 & 4: Motion Adaptive ---
    float true_motion = 0.0;

    if (has_prev2 == 1) {
        // Same-parity comparison (frame t vs frame t-2):
        // In 60Hz captured interlaced video, frame t and frame t-2 have the EXACT same
        // field parity (both even fields or both odd fields). In a static scene, their difference
        // is zero, completely immune to alternating-line bob jitter!
        vec4 c_prev2 = texture(prev_frame_2, v_tc);
        vec3 parity_diff = abs(c_curr.rgb - c_prev2.rgb);
        true_motion = max(max(parity_diff.r, parity_diff.g), parity_diff.b);
    } else {
        // Single-history fallback: cross-field interval check
        vec3 prev_min = min(c_prev_top.rgb, c_prev_bot.rgb);
        vec3 prev_max = max(c_prev_top.rgb, c_prev_bot.rgb);
        vec3 clamped_curr = clamp(c_curr.rgb, prev_min, prev_max);
        vec3 field_diff = abs(c_curr.rgb - clamped_curr);
        float motion = max(max(field_diff.r, field_diff.g), field_diff.b);

        vec3 curr_min = min(c_curr_top.rgb, c_curr_bot.rgb);
        vec3 curr_max = max(c_curr_top.rgb, c_curr_bot.rgb);
        vec3 clamped_prev = clamp(c_prev.rgb, curr_min, curr_max);
        vec3 rev_field_diff = abs(c_prev.rgb - clamped_prev);
        float rev_motion = max(max(rev_field_diff.r, rev_field_diff.g), rev_field_diff.b);

        true_motion = max(motion, rev_motion);
    }

    // Normalize motion against threshold with noise deadband (filters out analog S-Video cable noise)
    float thresh = max(0.01, motion_threshold);
    float motion_factor = smoothstep(thresh * 0.75, thresh * 1.5, true_motion);

    if (mode == 4) {
        // Debug mode: show static (field-blended) areas vs detected motion
        // Green = static field weave active, Magenta = motion detected (spatial filter active)
        vec3 debug_color = mix(vec3(0.0, 1.0, 0.2), vec3(1.0, 0.0, 0.4), motion_factor);
        out_color = vec4(mix(c_curr.rgb, debug_color, 0.65), c_curr.a);
        return;
    }

    // In static areas (motion_factor ~ 0): apply temporal weave to eliminate bob flicker!
    // In moving areas (motion_factor ~ 1): blend to spatial FIR to eliminate comb/ghosting!
    vec3 result = mix(temporal_blend, spatial_fir, motion_factor);
    out_color = vec4(result, c_curr.a);
}
