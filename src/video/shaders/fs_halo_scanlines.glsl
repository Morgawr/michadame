#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D LinearizePass;
uniform vec2 outputResolution;
uniform vec2 SourceSize;
uniform float horizontal_stretch;
uniform float halo_zoom;
uniform float beam_min;
uniform float beam_max;
uniform float beam_size;
uniform float h_sharp;
uniform float corner_size;
uniform int curvature;

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

    // Discard any pixel outside the rendering surface (the black bars)
    if (centered_tc.x < 0.0 || centered_tc.x > 1.0 || centered_tc.y < 0.0 || centered_tc.y > 1.0) {
        out_color = vec4(0.0);
        return;
    }

    vec2 zoom_scale = vec2(halo_zoom / 100.0);
    vec2 centered = (centered_tc - 0.5) / zoom_scale + 0.5;

    if (centered.x < 0.0 || centered.x > 1.0 || centered.y < 0.0 || centered.y > 1.0) {
        out_color = vec4(0.0);
        return;
    }

    vec2 warped = centered;
    if (curvature != 0) {
        warped = Warp(centered);
        if (warped.x < 0.0 || warped.x > 1.0 || warped.y < 0.0 || warped.y > 1.0) {
            out_color = vec4(0.0);
            return;
        }
    }

    float cval = 1.0;
    if (corner_size > 0.001) {
        cval = Corner(centered, corner_size);
        if (curvature != 0) {
            cval = min(cval, Corner(warped, corner_size));
        }
        if (cval <= 0.0) {
            out_color = vec4(0.0);
            return;
        }
    }

    // Effective scanline grid:
    // When capturing retro 240p consoles (Saturn, PS1, SNES, Genesis, etc.), capture cards output
    // 480i or 480p (SourceSize.y around 480-576). Drawing 480 scanlines on a window (<960p)
    // cuts sprite pixels in half and severely violates Nyquist sampling rate (causing moiré).
    // Halving sy for SourceSize.y >= 360 restores the authentic 240-scanline CRT raster.
    float sy = SourceSize.y >= 360.0 ? 2.0 : 1.0;
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

    // Horizontal Sharpness:
    // h_sharp controls both the horizontal transition steepness (S-curve) and analog CRT edge peaking.
    // 1.0 = soft consumer analog CRT (broad horizontal spread)
    // 3.5 = crisp Trinitron CRT
    // 6.0 - 10.0 = razor-sharp Sony PVM/BVM broadcast monitor
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

    // Anti-moiré band-limiting using screen-space derivatives:
    // scanline_rate is the vertical rate of change of scanline coordinate per display pixel.
    float scanline_rate = length(vec2(dFdx(OGL2Pos.y), dFdy(OGL2Pos.y)));

    // In windowed mode or low resolutions where display pixels per scanline is small (< 3.0),
    // smoothly raise the beam floor and soften beam exponent to completely prevent Nyquist moiré rings.
    float moire_factor = smoothstep(0.18, 0.45, scanline_rate);
    float min_floor = mix(0.10, 0.70, moire_factor);

    // Beam profiles
    const float scanline1 = 6.0;
    const float scanline2 = 8.0;
    float eff_scale1 = mix(scanline1, scanline2, f) / (1.0 + 0.6 * moire_factor);
    float eff_scale2 = mix(scanline1, scanline2, 1.0 - f) / (1.0 + 0.6 * moire_factor);

    float mc1 = max(max(color1.r, color1.g), color1.b);
    float mc2 = max(max(color2.r, color2.g), color2.b);

    float b_size = clamp(beam_size, 0.1, 2.0);
    float b1 = pow(mc1, mix(1.8, 0.4, clamp(b_size / 2.0, 0.0, 1.0)));
    float b2 = pow(mc2, mix(1.8, 0.4, clamp(b_size / 2.0, 0.0, 1.0)));

    float tmp1 = mix(beam_min, beam_max / sqrt(b_size), b1);
    float tmp2 = mix(beam_min, beam_max / sqrt(b_size), b2);

    float ex1 = f * tmp1;
    float ex2 = (1.0 - f) * tmp2;
    vec3 w1 = exp2(-eff_scale1 * ex1 * ex1 * vec3(1.0));
    vec3 w2 = exp2(-eff_scale2 * ex2 * ex2 * vec3(1.0));

    // Clamp trough floor based on anti-moiré factor:
    float w_sum = w1.r + w2.r;
    float target_sum = max(w_sum, min_floor);
    w1 = w1 / max(w_sum, 0.001) * target_sum;
    w2 = w2 / max(w_sum, 0.001) * target_sum;

    vec3 w12 = w1 + w2;
    float wf1 = max(max(w12.r, w12.g), w12.b);
    if (wf1 > 1.0) {
        w1 /= wf1;
        w2 /= wf1;
    }

    vec3 color = color1 * w1 + color2 * w2;
    out_color = vec4(color * cval, cval);
}
