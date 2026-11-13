#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D LinearizePass;
uniform sampler2D GlowPass;
uniform sampler2D BloomPass;

uniform vec2 outputResolution;
uniform vec2 SourceSize;
uniform float horizontal_stretch;
uniform float halo_zoom;
uniform float beam_min;
uniform float beam_max;
uniform float beam_size;
uniform float h_sharp;
uniform float corner_size;
uniform vec4 border_crop; // left, right, top, bottom
uniform float brightboost;
uniform float brightboost1;
uniform float glow;
uniform float bloom;
uniform float halation;
uniform float shadow_mask;
uniform float masksize;
uniform float maskstr;
uniform float mcut;
uniform float slotmask;
uniform float slotmask1;
uniform float double_slot;
uniform float smoothmask;
uniform int curvature;
uniform float vibrance;

vec2 Warp(vec2 pos) {
    pos = pos * 2.0 - 1.0;
    pos *= vec2(1.0 + (pos.y * pos.y) * 0.031, 1.0 + (pos.x * pos.x) * 0.041);
    return pos * 0.5 + 0.5;
}

float Corner(vec2 pos, float crn) {
    if (crn <= 0.001) return 1.0;
    vec2 d = abs(2.0 * pos - 1.0) - (vec2(1.0) - vec2(crn * 2.0));
    if (d.x <= 0.0 || d.y <= 0.0) {
        return 1.0;
    }
    float dist = length(d);
    return clamp((crn * 2.0 - dist) / 0.005, 0.0, 1.0);
}

vec3 Mask(vec2 pos, float mx) {
    vec3 mask = vec3(0.0);
    vec3 one = vec3(1.0);

    float maskDark = clamp(1.0 - maskstr, 0.0, 1.0);
    float maskLight = 1.0 + maskstr * 0.5;

    if (shadow_mask <= 0.0) {
        return vec3(1.0);
    } else if (shadow_mask == 1.0) {
        float line = maskLight;
        float odd = 0.0;
        if (fract(pos.x / 6.0) < 0.49) odd = 1.0;
        if (fract((pos.y + odd) / 2.0) < 0.49) line = maskDark;
        pos.x = floor(mod(pos.x, 3.0));
        if (pos.x < 0.5) mask.r = maskLight;
        else if (pos.x < 1.5) mask.g = maskLight;
        else mask.b = maskLight;
        mask *= line;
    } else if (shadow_mask == 2.0) {
        pos.x = floor(mod(pos.x, 3.0));
        if (pos.x < 0.5) mask.r = maskLight;
        else if (pos.x < 1.5) mask.g = maskLight;
        else mask.b = maskLight;
    } else if (shadow_mask == 3.0) {
        pos.x += pos.y * 3.0;
        pos.x = fract(pos.x / 6.0);
        if (pos.x < 0.3) mask.r = maskLight;
        else if (pos.x < 0.6) mask.g = maskLight;
        else mask.b = maskLight;
    } else if (shadow_mask == 4.0) {
        pos.xy = floor(pos.xy * vec2(1.0, 0.5));
        pos.x += pos.y * 3.0;
        pos.x = fract(pos.x / 6.0);
        if (pos.x < 0.3) mask.r = maskLight;
        else if (pos.x < 0.6) mask.g = maskLight;
        else mask.b = maskLight;
    } else if (shadow_mask == 5.0) {
        pos.x = fract(pos.x / 2.0);
        if (pos.x < 0.49) { mask.r = 1.0; mask.b = 1.0; }
        else { mask.g = 1.0; }
        mask = clamp(mix(mix(one, mask, mcut), mix(one, mask, maskstr), mx), 0.0, 1.0);
    } else { // 6.0 (Trinitron) and default
        pos.x = floor(mod(pos.x, 3.0));
        if (pos.x < 0.5) mask.r = 1.0;
        else if (pos.x < 1.5) mask.g = 1.0;
        else mask.b = 1.0;
        mask = clamp(mix(mix(one, mask, mcut), mix(one, mask, maskstr), mx), 0.0, 1.0);
    }
    return mask;
}

float SlotMask(vec2 pos, float m, float swidth) {
    if ((slotmask + slotmask1) == 0.0) return 1.0;
    float slot_thickness = max(masksize, 1.0);
    pos.y = floor(pos.y / slot_thickness);
    float mlen = swidth * 2.0;
    float px = floor(mod(pos.x, 0.99999 * mlen));
    float py = floor(fract(pos.y / (2.0 * double_slot)) * 2.0 * double_slot);
    float slot_dark = mix(1.0 - slotmask1, 1.0 - slotmask, m);
    float slot = 1.0;
    if (py == 0.0 && px < swidth) slot = slot_dark;
    else if (py == double_slot && px >= swidth) slot = slot_dark;
    return slot;
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
    if (centered_tc.x < 0.0 || centered_tc.x > 1.0 || centered_tc.y < 0.0 || centered_tc.y > 1.0) {
        out_color = vec4(0.0);
        return;
    }

    vec2 zoom_scale = vec2(halo_zoom / 100.0);
    vec2 screen_tc = (centered_tc - 0.5) / zoom_scale + 0.5;
    if (screen_tc.x < 0.0 || screen_tc.x > 1.0 || screen_tc.y < 0.0 || screen_tc.y > 1.0) {
        out_color = vec4(0.0);
        return;
    }

    vec2 warped = screen_tc;
    if (curvature != 0) {
        warped = Warp(screen_tc);
    }
    if (warped.x < border_crop.x || warped.x > (1.0 - border_crop.y) ||
        warped.y < border_crop.z || warped.y > (1.0 - border_crop.w)) {
        out_color = vec4(0.0);
        return;
    }

    float cval = 1.0;
    if (corner_size > 0.001) {
        cval = Corner(screen_tc, corner_size);
        if (curvature != 0) {
            cval = min(cval, Corner(warped, corner_size));
        }
        if (cval <= 0.0001) {
            out_color = vec4(0.0);
            return;
        }
    }

    // Fused Scanline Evaluation (formerly `fs_halo_scanlines.glsl`):
    float sy = SourceSize.y > 1080.0 ? (SourceSize.y / 540.0) : (SourceSize.y >= 360.0 ? 2.0 : 1.0);
    vec2 scan_source_size = vec2(SourceSize.x, SourceSize.y / sy);

    vec2 ps = 1.0 / SourceSize;
    vec2 scan_ps = 1.0 / scan_source_size;

    vec2 OGL2Pos = warped * scan_source_size - vec2(0.5);
    vec2 fp = fract(OGL2Pos);
    vec2 dx = vec2(ps.x, 0.0);
    vec2 scan_dy = vec2(0.0, scan_ps.y);
    vec2 pC4 = floor(OGL2Pos) * scan_ps + 0.5 * scan_ps;
    float fpx = fp.x;
    float f = fp.y;

    float s_factor = max(h_sharp, 0.5);
    float trans_width = clamp(1.0 / (s_factor * 0.75), 0.05, 1.5);
    float fpx_curved;
    if (trans_width >= 1.0) {
        float t = clamp((fpx - 0.5) / trans_width + 0.5, 0.0, 1.0);
        fpx_curved = t * t * (3.0 - 2.0 * t);
    } else {
        float edge0 = 0.5 - 0.5 * trans_width;
        float edge1 = 0.5 + 0.5 * trans_width;
        float t = clamp((fpx - edge0) / (edge1 - edge0), 0.0, 1.0);
        fpx_curved = t * t * (3.0 - 2.0 * t);
    }

    // Scanline 1
    vec3 l2_1 = texture(LinearizePass, pC4 - dx).rgb;
    vec3 l1_1 = texture(LinearizePass, pC4).rgb;
    vec3 r1_1 = texture(LinearizePass, pC4 + dx).rgb;
    vec3 r2_1 = texture(LinearizePass, pC4 + 2.0 * dx).rgb;
    vec3 color1 = mix(l1_1, r1_1, fpx_curved);
    vec3 edge_detail1 = 0.5 * (l1_1 + r1_1) - 0.25 * (l2_1 + r2_1 + l1_1 + r1_1);
    float peak_str = (s_factor - 2.0) * 0.15;
    color1 = clamp(color1 + edge_detail1 * peak_str, 0.0, 1.0);

    // Scanline 2 (pC4 + scan_dy)
    vec2 pC4_2 = pC4 + scan_dy;
    vec3 l2_2 = texture(LinearizePass, pC4_2 - dx).rgb;
    vec3 l1_2 = texture(LinearizePass, pC4_2).rgb;
    vec3 r1_2 = texture(LinearizePass, pC4_2 + dx).rgb;
    vec3 r2_2 = texture(LinearizePass, pC4_2 + 2.0 * dx).rgb;
    vec3 color2 = mix(l1_2, r1_2, fpx_curved);
    vec3 edge_detail2 = 0.5 * (l1_2 + r1_2) - 0.25 * (l2_2 + r2_2 + l1_2 + r1_2);
    color2 = clamp(color2 + edge_detail2 * peak_str, 0.0, 1.0);

    float active_out_h = max(outputResolution.y * scale.y * zoom_scale.y, 1.0);
    float scanline_rate = scan_source_size.y / active_out_h;
    float moire_factor = smoothstep(0.18, 0.45, scanline_rate);

    const float scanline1 = 6.0;
    const float scanline2 = 8.0;
    float eff_scale1 = mix(scanline1, scanline2, f) / (1.0 + 0.6 * moire_factor);
    float eff_scale2 = mix(scanline1, scanline2, 1.0 - f) / (1.0 + 0.6 * moire_factor);

    float mc1 = max(max(color1.r, color1.g), color1.b);
    float mc2 = max(max(color2.r, color2.g), color2.b);

    float b_size = clamp(beam_size, 0.1, 2.0);
    float b_exp = mix(1.8, 0.4, clamp(b_size / 2.0, 0.0, 1.0));
    float b1 = pow(mc1, b_exp);
    float b2 = pow(mc2, b_exp);

    float beam_max_scaled = beam_max / sqrt(b_size);
    float tmp1 = mix(beam_min, beam_max_scaled, b1);
    float tmp2 = mix(beam_min, beam_max_scaled, b2);

    float ex1 = f * tmp1;
    float ex2 = (1.0 - f) * tmp2;
    float w1_s = exp2(-eff_scale1 * ex1 * ex1);
    float w2_s = exp2(-eff_scale2 * ex2 * ex2);

    float w_sum = w1_s + w2_s;
    float w_raw = min(w_sum, 1.0);
    float mid_scale = 7.0 / (1.0 + 0.6 * moire_factor);
    float trough = clamp(
        exp2(-mid_scale * 0.25 * tmp1 * tmp1) + exp2(-mid_scale * 0.25 * tmp2 * tmp2),
        0.10,
        1.0
    );
    float cos_wave = 0.5 + 0.5 * cos(6.2831853 * f);
    float trough_gamma = mix(pow(trough, 1.0 / 2.4), 1.0, moire_factor);
    float w_cos = pow(mix(trough_gamma, 1.0, cos_wave), 2.4);
    float bandlimit_mix = smoothstep(0.12, 0.22, scanline_rate);
    float target_sum = mix(mix(w_raw, 1.0, moire_factor), w_cos, bandlimit_mix);
    float w_norm = target_sum / max(w_sum, 0.001);

    vec3 color = (color1 * (w1_s * w_norm) + color2 * (w2_s * w_norm)) * cval;
    float colmx = max(max(color.r, color.g), color.b);

    vec3 orig1 = color;
    vec2 maskcoord = gl_FragCoord.xy / max(masksize, 1.0);

    // Apply shadow mask
    vec3 cmask = Mask(floor(maskcoord), colmx);

    // Apply slot mask
    float smask = SlotMask(gl_FragCoord.xy, colmx, 3.0 * masksize);
    cmask *= smask;

    // Smooth mask in bright areas (beam blooms over phosphor gaps):
    if (smoothmask > 0.0) {
        float smooth_amount = clamp(smoothmask * pow(colmx, 1.2), 0.0, 1.0);
        cmask = mix(cmask, vec3(1.0), smooth_amount);
    }

    // Apply mask to color
    color = color * cmask;

    // Brightboost
    float bb = mix(brightboost, brightboost1, colmx);
    color *= bb;

    // Sample Bloom and Glow only when their respective weights are active
    vec2 clamped_warped = clamp(warped, 0.0, 1.0);
    if (abs(bloom) > 0.01 || abs(halation) > 0.01) {
        vec3 bloom_val = texture(BloomPass, clamped_warped).rgb;
        if (abs(bloom) > 0.01) {
            vec3 Bloom1 = min(bloom_val * (orig1 + color), max(0.5 * (colmx + orig1 - color), 0.001 * bloom_val));
            Bloom1 = 0.5 * (Bloom1 + mix(Bloom1, mix(colmx * orig1, Bloom1, 0.5), 1.0 - clamp(color, 0.0, 1.0)));
            color = color + abs(bloom) * Bloom1;
        }
        if (abs(halation) > 0.01) {
            vec3 hBloom = 0.5 * (bloom_val + bloom_val * bloom_val);
            color = color + abs(halation) * hBloom;
        }
    }

    if (abs(glow) > 0.01) {
        vec3 glow_val = texture(GlowPass, clamped_warped).rgb;
        color = color + abs(glow) * glow_val;
    }

    // Apply vibrance (saturation boost in linear space)
    float luminance = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = mix(vec3(luminance), color, vibrance);

    // Gamma out: linear -> display gamma 2.4
    color = pow(clamp(color, 0.0, 1.0), vec3(1.0 / 2.4));

    out_color = vec4(color, cval);
}
