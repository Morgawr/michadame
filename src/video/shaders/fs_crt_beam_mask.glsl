#version 330 core
//-------------------------------------------------------------------------------------------------
// Stage 2 of the Blur Busters CRT Beam Pipeline:
// CRT Tube Geometry Transformation + Gamma-Curve-Preserving Scanlines & Phosphor Masks
//
// Operates on the linear-light RGBA16F output of Stage 1 (`fs_crt_beam.glsl`), where:
// - `beam_texture.rgb` = swept sub-frame linear RGB light L_s(x, y)
// - `beam_texture.a`   = cycle-steady unstrobed peak luminance L_steady(x, y) (or 2.0 on split border)
//
// Because all spatial modulations (scanlines, aperture/shadow/slot masks, halation) operate
// linearly on L_s(x, y) and parameterize beam/mask shape exclusively by time-invariant L_steady,
// the stage commutes with the Talbot-Plateau time integral (\sum_s T * L_s = T * \sum_s L_s)
// and preserves the input gamma curve without sub-frame banding.
//-------------------------------------------------------------------------------------------------

in vec2 v_tc;
out vec4 out_color;

uniform sampler2D beam_texture;

uniform vec2 outputResolution;
uniform vec2 source_size;
uniform vec3 background_color;
uniform float horizontal_stretch;
uniform vec4 border_crop; // left, right, top, bottom

uniform float gamma;
uniform float post_gain;

// CRT Tube Geometry
uniform int curvature;
uniform float warp_x;
uniform float warp_y;
uniform float corner_size;
uniform float screen_scale;

// Gamma-Preserving Scanline Beam Profile
uniform float scanline_strength;
uniform float beam_min;
uniform float beam_max;

// Gamma-Preserving CRT Phosphor Mask (Aperture Grille / Shadow Mask / Slot Mask)
uniform float shadow_mask;
uniform float mask_strength;
uniform float mask_size;
uniform float mask_cutoff;
uniform float slot_mask;
uniform float double_slot;
uniform float smooth_mask;

// Linear-Light Glass Halation
uniform float halation;

const float PI = 3.14159265359;

// Standard 2.4 sRGB encoding for background_color outside the CRT tube
float ToSrgb1(float c) {
    return (c < 0.0031308 ? c * 12.92 : 1.055 * pow(max(c, 0.0), 0.41666) - 0.055);
}
vec3 ToSrgb(vec3 c) {
    return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
}

// Blur Busters specification linear -> sRGB encoding using the simulated phosphor `gamma`
float linear2srgb(float c) {
    vec3 j = vec3(0.0031308 * 12.92, 12.92, 1.0 / max(gamma, 0.01));
    vec2 k = vec2(1.055, -0.055);
    return clamp(j.x, c * j.y, pow(max(c, 0.0), j.z) * k.x + k.y);
}
vec3 linear2srgb(vec3 c) {
    return vec3(linear2srgb(c.r), linear2srgb(c.g), linear2srgb(c.b));
}

vec2 Warp(vec2 pos, float wx, float wy) {
    pos = pos * 2.0 - 1.0;
    pos *= vec2(1.0 + (pos.y * pos.y) * wx, 1.0 + (pos.x * pos.x) * wy);
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

// Gamma-curve-preserving phosphor mask:
// Returns sub-pixel RGB transmittance centered around a constant triad mean so that
// dark, mid-tone, and bright IRE levels experience identical macroscopic transmittance.
vec3 EvaluatePhosphorMask(vec2 pos, float steady_luma) {
    if (shadow_mask <= 0.0 || mask_strength <= 0.001) {
        return vec3(1.0);
    }

    // Smooth mask bloom in bright areas (driven by cycle-steady luminance so it is
    // 100% constant across sub-frames and preserves Talbot-Plateau integration)
    float bloom_relax = clamp(smooth_mask * 0.35 * steady_luma, 0.0, 0.75);
    float eff_str = clamp(mask_strength * (1.0 - bloom_relax), 0.0, 1.0);
    float eff_cut = clamp(mask_cutoff * (1.0 - bloom_relax), 0.0, 1.0);

    float maskDark = 1.0 - eff_str * 0.65;
    float maskLight = 1.0 + eff_str * 0.325;
    vec3 mask = vec3(maskDark);

    if (shadow_mask == 1.0) {
        // 1: CGWG TV Shadow Mask
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
        // 2: Lottes Aperture Grille
        pos.x = floor(mod(pos.x, 3.0));
        if (pos.x < 0.5) mask.r = maskLight;
        else if (pos.x < 1.5) mask.g = maskLight;
        else mask.b = maskLight;
    } else if (shadow_mask == 3.0) {
        // 3: Stretched Shadow Mask
        pos.x += pos.y * 3.0;
        pos.x = fract(pos.x / 6.0);
        if (pos.x < 0.333) mask.r = maskLight;
        else if (pos.x < 0.666) mask.g = maskLight;
        else mask.b = maskLight;
    } else if (shadow_mask == 4.0) {
        // 4: VGA Dot Triad
        pos.xy = floor(pos.xy * vec2(1.0, 0.5));
        pos.x += pos.y * 3.0;
        pos.x = fract(pos.x / 6.0);
        if (pos.x < 0.333) mask.r = maskLight;
        else if (pos.x < 0.666) mask.g = maskLight;
        else mask.b = maskLight;
    } else if (shadow_mask == 5.0) {
        // 5: Fine 2-Pixel Aperture Grille
        pos.x = fract(pos.x / 2.0);
        vec3 raw = (pos.x < 0.49) ? vec3(1.0, 0.0, 1.0) : vec3(0.0, 1.0, 0.0);
        float depth = mix(eff_cut, eff_str, 0.5) * 0.65;
        mask = mix(vec3(1.0), raw, depth);
    } else {
        // 6: Sony Trinitron Aperture Grille
        pos.x = floor(mod(pos.x, 3.0));
        vec3 raw = vec3(0.0);
        if (pos.x < 0.5) raw.r = 1.0;
        else if (pos.x < 1.5) raw.g = 1.0;
        else raw.b = 1.0;
        float depth = mix(eff_cut, eff_str, 0.55) * 0.68;
        mask = mix(vec3(1.0), raw, depth);
    }

    // Normalize triad mean toward 1.0 (capped to avoid excessive single-channel gain)
    // so the phosphor mask preserves the exact gamma curve across all IRE levels.
    float mean_trans = max((mask.r + mask.g + mask.b) / 3.0, 0.25);
    float norm_gain = mix(1.0, 1.0 / mean_trans, 0.65);
    return mask * norm_gain;
}

float EvaluateSlotMask(vec2 frag_pos, float steady_luma) {
    if (slot_mask <= 0.001 || shadow_mask <= 0.0) {
        return 1.0;
    }
    float pitch = max(mask_size, 1.0);
    float swidth = 3.0 * pitch;
    float dslot = max(floor(double_slot + 0.5), 1.0);
    float y_cell = floor(frag_pos.y / pitch);
    float mlen = swidth * 2.0;
    float px = floor(mod(frag_pos.x, 0.99999 * mlen));
    float py = floor(fract(y_cell / (2.0 * dslot)) * 2.0 * dslot);

    float bloom_relax = clamp(smooth_mask * 0.30 * steady_luma, 0.0, 0.70);
    float eff_slot = clamp(slot_mask * (1.0 - bloom_relax), 0.0, 1.0) * 0.65;
    float slot_dark = 1.0 - eff_slot;
    float slot_light = 1.0 + eff_slot / max(2.0 * dslot - 1.0, 1.0) * 0.65;

    if ((py == 0.0 && px < swidth) || (py == dslot && px >= swidth)) {
        return slot_dark;
    }
    return slot_light;
}

void main() {
    vec2 corrected_tc = vec2(v_tc.x, 1.0 - v_tc.y);
    float video_aspect = (source_size.x * horizontal_stretch) / max(source_size.y, 1.0);
    float output_aspect = outputResolution.x / max(outputResolution.y, 1.0);

    vec2 scale = vec2(1.0, 1.0);
    if (video_aspect > output_aspect) {
        scale.y = output_aspect / video_aspect;
    } else {
        scale.x = video_aspect / output_aspect;
    }

    vec2 centered_tc = (corrected_tc - 0.5) / scale + 0.5;
    if (centered_tc.x < 0.0 || centered_tc.x > 1.0 || centered_tc.y < 0.0 || centered_tc.y > 1.0) {
        out_color = vec4(ToSrgb(background_color), 1.0);
        return;
    }

    vec2 zoom_scale = vec2(clamp(screen_scale, 50.0, 100.0) / 100.0);
    vec2 screen_tc = (centered_tc - 0.5) / zoom_scale + 0.5;
    if (screen_tc.x < 0.0 || screen_tc.x > 1.0 || screen_tc.y < 0.0 || screen_tc.y > 1.0) {
        out_color = vec4(ToSrgb(background_color), 1.0);
        return;
    }

    vec2 warped_tc = screen_tc;
    if (curvature != 0) {
        warped_tc = Warp(screen_tc, warp_x, warp_y);
    }

    if (warped_tc.x < border_crop.x || warped_tc.x > (1.0 - border_crop.y) ||
        warped_tc.y < border_crop.z || warped_tc.y > (1.0 - border_crop.w)) {
        out_color = vec4(ToSrgb(background_color), 1.0);
        return;
    }

    float cval = 1.0;
    if (corner_size > 0.001) {
        cval = Corner(screen_tc, corner_size);
        if (curvature != 0) {
            cval = min(cval, Corner(warped_tc, corner_size));
        }
        if (cval <= 0.0) {
            out_color = vec4(ToSrgb(background_color), 1.0);
            return;
        }
    }

    // Sample Stage 1 linear-light beam texture (bottom-left UV origin)
    vec2 beam_uv = clamp(vec2(warped_tc.x, 1.0 - warped_tc.y), 0.0, 1.0);
    vec4 beam_sample = texture(beam_texture, beam_uv);

    // Splitscreen comparison border indicator
    if (beam_sample.a > 1.5) {
        vec3 border_col = mix(ToSrgb(background_color), vec3(1.0), cval);
        out_color = vec4(border_col, 1.0);
        return;
    }

    vec3 color = max(beam_sample.rgb, vec3(0.0)) * max(post_gain, 0.1);
    float steady_luma = clamp(beam_sample.a, 0.0, 1.0);

    // Linear-light glass halation / phosphor glow (commutes with Talbot-Plateau sum)
    if (halation > 0.001) {
        vec2 texel = 1.0 / max(outputResolution * scale, vec2(1.0));
        vec2 r1 = texel * 2.5;
        vec2 r2 = texel * 6.0;
        vec3 glow_sum = color * 0.20;
        glow_sum += texture(beam_texture, clamp(beam_uv + vec2( r1.x,  0.0), 0.0, 1.0)).rgb * 0.12;
        glow_sum += texture(beam_texture, clamp(beam_uv + vec2(-r1.x,  0.0), 0.0, 1.0)).rgb * 0.12;
        glow_sum += texture(beam_texture, clamp(beam_uv + vec2( 0.0,  r1.y), 0.0, 1.0)).rgb * 0.12;
        glow_sum += texture(beam_texture, clamp(beam_uv + vec2( 0.0, -r1.y), 0.0, 1.0)).rgb * 0.12;
        glow_sum += texture(beam_texture, clamp(beam_uv + vec2( r2.x,  r2.y), 0.0, 1.0)).rgb * 0.08;
        glow_sum += texture(beam_texture, clamp(beam_uv + vec2(-r2.x,  r2.y), 0.0, 1.0)).rgb * 0.08;
        glow_sum += texture(beam_texture, clamp(beam_uv + vec2( r2.x, -r2.y), 0.0, 1.0)).rgb * 0.08;
        glow_sum += texture(beam_texture, clamp(beam_uv + vec2(-r2.x, -r2.y), 0.0, 1.0)).rgb * 0.08;
        color += glow_sum * (halation * 0.35);
    }

    // Gamma-Curve-Preserving & Talbot-Plateau-Preserving CRT Scanlines:
    // Effective scanline grid halves >=360p to 240p retro raster and caps >1080p at 540 scanlines.
    if (scanline_strength > 0.001) {
        float sy = source_size.y > 1080.0
            ? (source_size.y / 540.0)
            : (source_size.y >= 360.0 ? 2.0 : 1.0);
        float scan_source_y = max(source_size.y / sy, 1.0);
        float active_out_h = max(outputResolution.y * scale.y * zoom_scale.y, 1.0);
        float scanline_rate = scan_source_y / active_out_h;

        // Smoothly attenuate scanlines when display pixels per scanline is small (< 3.0)
        // to prevent Nyquist moiré rings in smaller windows.
        float moire_factor = smoothstep(0.18, 0.45, scanline_rate);
        float eff_scan_str = clamp(scanline_strength, 0.0, 1.0) * (1.0 - moire_factor);

        if (eff_scan_str > 0.001) {
            float f = fract(warped_tc.y * scan_source_y - 0.5);
            float cos_wave = 0.5 + 0.5 * cos(2.0 * PI * f);
            // Beam width exponent uses steady_luma (constant across all sub-frames!)
            float p = clamp(mix(beam_min, beam_max, steady_luma), 0.30, 3.0);
            float raw_profile = pow(cos_wave, p);
            // Analytic vertical integral of ((1 + cos(2*pi*f))/2)^p over f in [0, 1]:
            float mean_profile = 1.0 / sqrt(1.0 + 3.0 * p + (0.14159265 * p * p) / (1.0 + p));
            // Mean-normalized scanline weight so vertical average transmittance is independent
            // of steady_luma, preserving the exact gamma curve across all IRE levels!
            float norm_profile = mix(1.0, raw_profile / max(mean_profile, 0.25), 0.65);
            float scan_weight = mix(1.0, norm_profile, eff_scan_str);
            color *= scan_weight;
        }
    }

    // Gamma-Curve-Preserving & Talbot-Plateau-Preserving CRT Phosphor Mask:
    vec2 mask_coord = floor(gl_FragCoord.xy / max(mask_size, 1.0));
    vec3 cmask = EvaluatePhosphorMask(mask_coord, steady_luma);
    float smask = EvaluateSlotMask(gl_FragCoord.xy, steady_luma);
    color *= (cmask * smask);

    // Energy-conserving phosphor sub-pixel bloom:
    // Any channel energy exceeding 1.0 in linear light spills into adjacent phosphor sub-channels
    // rather than hard-clipping, preserving Talbot-Plateau linearity on bright peaks.
    vec3 overflow = max(color - vec3(1.0), vec3(0.0));
    float spill = (overflow.r + overflow.g + overflow.b) * 0.45;
    color = min(color, vec3(1.0)) + vec3(spill) * (vec3(1.0) - min(color, vec3(1.0)));

    // Encode from linear light to display space using the exact simulated phosphor gamma
    vec3 srgb_color = clamp(linear2srgb(color), 0.0, 1.0);

    // Static sub-code-value dithering for final 8-bit display conversion
    float noise = fract(52.9829189 * fract(dot(gl_FragCoord.xy, vec2(0.06711056, 0.00583715)))) - 0.5;
    vec3 amplitude = smoothstep(vec3(0.0), vec3(2.0 / 255.0), srgb_color) / 255.0;
    srgb_color = clamp(srgb_color + noise * amplitude, 0.0, 1.0);

    vec3 bg_srgb = ToSrgb(background_color);
    out_color = vec4(mix(bg_srgb, srgb_color, cval), 1.0);
}
