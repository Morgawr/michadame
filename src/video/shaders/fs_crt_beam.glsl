#version 330 core
//-------------------------------------------------------------------------------------------------
// Blur Busters CRT Beam Simulator BFI With Seamless Gamma Correction
// Adapted for the michadame rendering pipeline.
//
// CREDIT: Teamwork of Mark Rejhon @BlurBusters & Timothy Lottes @NOTimothyLottes
// MIT License - Copyright 2024 Mark Rejhon (@BlurBusters) & Timothy Lottes (@NOTimothyLottes)
//-------------------------------------------------------------------------------------------------

in vec2 v_tc;
out vec4 out_color;

// 3 trailing unprocessed CRT refresh cycle framebuffers (stored in linear RGB):
// frame_curr  = crtHzCounter
// frame_prev1 = crtHzCounter - 1.0
// frame_prev2 = crtHzCounter - 2.0
uniform sampler2D frame_curr;
uniform sampler2D frame_prev1;
uniform sampler2D frame_prev2;

// michadame pipeline uniforms
uniform vec2 outputResolution;
uniform vec2 source_size;
uniform vec3 background_color;
uniform float horizontal_stretch;
uniform float vibrance;
uniform int scaler_filter;
uniform vec4 border_crop; // left, right, top, bottom

// Blur Busters CRT Beam Simulator parameters
uniform int iFrame;
uniform float frames_per_hz;
uniform float gamma;
uniform float gain_vs_blur;
uniform float fps_divisor;
uniform int lcd_anti_retention;
uniform float lcd_inversion_compensation_slew;
uniform int scan_direction;
uniform int splitscreen;
uniform float splitscreen_x;
uniform float splitscreen_y;
uniform int splitscreen_border_px;
uniform int splitscreen_match_brightness;
uniform float h_sharp;
uniform float brightboost;

//-------------------------------------------------------------------------------------------------
// Utility Macros & Functions

#define clampPixel(a) clamp(a, vec3(0.0), vec3(1.0))

#define IS_INTEGER(x) (floor(x) == (x))
#define IS_EVEN_INTEGER(x) (IS_INTEGER(x) && IS_INTEGER((x) / 2.0))

//-------------------------------------------------------------------------------------------------
// Exact Collapse of srgb2linear(ToSrgb(linear_c)) (Blur Busters specification):
// `frame_curr/prev1/prev2` store linear RGB in [0, 1]. Encoding with standard 2.4 sRGB (`ToSrgb`)
// and immediately decoding with Blur Busters `srgb2linear(gamma)` simplifies algebraically:
// - When gamma == 2.4: exact identity `c` across both the linear toe and power curve (0 `pow()`).
// - When gamma != 2.4:
//   * c <= 0.0031308 -> (12.92 * c) / 12.92 == c
//   * c >  0.0031308 -> (((1.055 * c^(1/2.4) - 0.055) + 0.055) / 1.055)^gamma == c^(gamma / 2.4)
float applyBeamGamma1(float c) {
    return c <= 0.0031308 ? c : pow(c, max(gamma, 0.01) * (1.0 / 2.4));
}

vec3 applyBeamGamma(vec3 linear_c) {
    vec3 c = clampPixel(linear_c);
    if (abs(gamma - 2.4) < 0.001) {
        return c;
    }
    return vec3(applyBeamGamma1(c.r), applyBeamGamma1(c.g), applyBeamGamma1(c.b));
}

//-------------------------------------------------------------------------------------------------
// michadame Scaler Filters (Bicubic, Lanczos 2, Bilinear / Nearest)

vec4 cubic(float v) {
    vec4 n = vec4(1.0, 2.0, 3.0, 4.0) - v;
    vec4 s = n * n * n;
    float x = s.x;
    float y = s.y - 4.0 * s.x;
    float z = s.z - 4.0 * s.y + 6.0 * s.x;
    float w = 6.0 - x - y - z;
    return vec4(x, y, z, w) * (1.0 / 6.0);
}

vec3 SampleBicubic(sampler2D tex, vec2 uv, vec2 texSize) {
    vec2 invTexSize = 1.0 / texSize;
    vec2 tc = uv * texSize - 0.5;
    vec2 f = fract(tc);
    tc -= f;

    vec4 xcubic = cubic(f.x);
    vec4 ycubic = cubic(f.y);

    vec4 c = tc.xxyy + vec2(-0.5, +1.5).xyxy;
    vec4 s = vec4(xcubic.xz + xcubic.yw, ycubic.xz + ycubic.yw);
    vec4 offset = c + vec4(xcubic.yw, ycubic.yw) / s;

    offset *= invTexSize.xxyy;

    vec3 sample0 = texture(tex, offset.xz).rgb;
    vec3 sample1 = texture(tex, offset.yz).rgb;
    vec3 sample2 = texture(tex, offset.xw).rgb;
    vec3 sample3 = texture(tex, offset.yw).rgb;

    float sx = s.x / (s.x + s.y);
    float sy = s.z / (s.z + s.w);

    return mix(mix(sample3, sample2, sx), mix(sample1, sample0, sx), sy);
}

const float PI = 3.14159265359;

float lanczosWeight(float x) {
    if (x == 0.0) return 1.0;
    if (abs(x) >= 2.0) return 0.0;
    return (sin(PI * x) / (PI * x)) * (sin(PI * x / 2.0) / (PI * x / 2.0));
}

vec3 SampleLanczos(sampler2D tex, vec2 uv, vec2 texSize) {
    vec2 invTexSize = 1.0 / texSize;
    vec2 tc = uv * texSize - 0.5;
    vec2 baseTc = floor(tc);
    vec2 f = tc - baseTc;

    float wx0 = lanczosWeight(-1.0 - f.x);
    float wx1 = lanczosWeight(-f.x);
    float wx2 = lanczosWeight(1.0 - f.x);
    float wx3 = lanczosWeight(2.0 - f.x);
    float wxSum = wx0 + wx1 + wx2 + wx3;

    float xTc0 = (baseTc.x - 0.5) * invTexSize.x;
    float xTc1 = (baseTc.x + 0.5) * invTexSize.x;
    float xTc2 = (baseTc.x + 1.5) * invTexSize.x;
    float xTc3 = (baseTc.x + 2.5) * invTexSize.x;

    vec3 color = vec3(0.0);
    float totalWeight = 0.0;

    for (int y = -1; y <= 2; y++) {
        float wy = lanczosWeight(float(y) - f.y);
        float yTc = (baseTc.y + float(y) + 0.5) * invTexSize.y;

        color += texture(tex, clamp(vec2(xTc0, yTc), 0.0, 1.0)).rgb * (wx0 * wy);
        color += texture(tex, clamp(vec2(xTc1, yTc), 0.0, 1.0)).rgb * (wx1 * wy);
        color += texture(tex, clamp(vec2(xTc2, yTc), 0.0, 1.0)).rgb * (wx2 * wy);
        color += texture(tex, clamp(vec2(xTc3, yTc), 0.0, 1.0)).rgb * (wx3 * wy);
        totalWeight += wxSum * wy;
    }
    return color / totalWeight;
}

vec3 SampleVideoRaw(sampler2D tex, vec2 uv, vec2 texSize, int shader_filter) {
    if (shader_filter == 2) { // Bicubic
        return SampleBicubic(tex, uv, texSize);
    } else if (shader_filter == 4) { // Lanczos
        return SampleLanczos(tex, uv, texSize);
    } else {
        return texture(tex, uv).rgb;
    }
}

vec3 SampleVideo(sampler2D tex, vec2 uv, vec2 texSize, int shader_filter) {
    if (h_sharp <= 1.05 || texSize.x <= 1.0) {
        return SampleVideoRaw(tex, uv, texSize, shader_filter);
    }
    float s_factor = clamp(h_sharp, 1.0, 10.0);
    float trans_width = clamp(1.0 / (s_factor * 0.75), 0.05, 1.5);
    float x_pos = uv.x * texSize.x - 0.5;
    float fpx = fract(x_pos);
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
    float base_x = (floor(x_pos) + 0.5) / texSize.x;
    float dx = 1.0 / texSize.x;
    vec2 uv_sharp = vec2(clamp(base_x + fpx_curved * dx, 0.0, 1.0), uv.y);
    vec3 center = SampleVideoRaw(tex, uv_sharp, texSize, shader_filter);
    if (s_factor > 2.2) {
        vec3 l = texture(tex, vec2(clamp(base_x - dx, 0.0, 1.0), uv.y)).rgb;
        vec3 r = texture(tex, vec2(clamp(base_x + 2.0 * dx, 0.0, 1.0), uv.y)).rgb;
        vec3 edge_detail = center - 0.5 * (l + r);
        float peak_str = clamp((s_factor - 2.2) * 0.08, 0.0, 0.45);
        center = clamp(center + edge_detail * peak_str, 0.0, 1.0);
    }
    return center;
}

//-------------------------------------------------------------------------------------------------
// Gets pixel from the 3 trailing unprocessed CRT refresh cycle framebuffers.

vec3 getPixelFromOrigFrameLinear(vec2 uv, float getFromHzNumber, float currentHzCounter) {
    if ((getFromHzNumber > currentHzCounter) ||
        (getFromHzNumber < currentHzCounter - 2.0)) {
        return vec3(0.0, 0.0, 0.0);
    }

    // Convert bottom-left Shadertoy uv [0..1] to top-left michadame texture coordinates [0..1]
    vec2 sample_tc = clamp(vec2(uv.x, 1.0 - uv.y), 0.0, 1.0);
    vec3 linear_sampled;
    if (abs(getFromHzNumber - currentHzCounter) < 0.5) {
        linear_sampled = SampleVideo(frame_curr, sample_tc, source_size, scaler_filter);
    } else if (abs(getFromHzNumber - (currentHzCounter - 1.0)) < 0.5) {
        linear_sampled = SampleVideo(frame_prev1, sample_tc, source_size, scaler_filter);
    } else {
        linear_sampled = SampleVideo(frame_prev2, sample_tc, source_size, scaler_filter);
    }

    float luminance = dot(linear_sampled, vec3(0.2126, 0.7152, 0.0722));
    return clamp(mix(vec3(luminance), linear_sampled, vibrance), 0.0, 1.0);
}

//-------------------------------------------------------------------------------------------------
// CRT Rolling Scan Simulation With Phosphor Fade + Brightness Redistributor Algorithm
// Returns vec4(linear_subframe_rgb, steady_cycle_luma) for the downstream CRT geometry & mask pass.

vec4 getPixelFromSimulatedCRT(vec2 uv, float crtRasterPos, float crtHzCounter, float framesPerHz)
{
    float tubePos = (1.0 - uv.y); // Default: Top to bottom
    if (scan_direction == 0) {
        tubePos = 0.0;            // Global refresh
    } else if (scan_direction == 1) {
        tubePos = (1.0 - uv.y);   // Top to bottom
    } else if (scan_direction == 2) {
        tubePos = uv.y;           // Bottom to top
    } else if (scan_direction == 3) {
        tubePos = uv.x;           // Left to right
    } else if (scan_direction == 4) {
        tubePos = (1.0 - uv.x);   // Right to left
    }

    // Incorporate brightboost into the temporal photon budget (up to framesPerHz) so the
    // Blur Busters Brightness Redistributor widens the phosphor emission pulse across
    // sub-frames rather than clipping peak sub-frame amplitude above 1.0.
    float effectiveGain = clamp(gain_vs_blur * max(brightboost, 0.5), 0.01, 1.0);
    float brightnessScale = framesPerHz * effectiveGain;
    float tubeFrame = tubePos * framesPerHz;
    float fStart = crtRasterPos * framesPerHz;
    float fEnd = fStart + 1.0;

    float startPrev2 = tubeFrame - framesPerHz;
    float startPrev1 = tubeFrame;
    float startCurr  = tubeFrame + framesPerHz; // Fix seam for top edge

    // Only sample trailing refresh cycle textures whose maximum photon budget interval
    // [start, start + brightnessScale] can overlap the active sub-frame [fStart, fEnd].
    bool needPrev2 = (startPrev2 < fEnd && startPrev2 + brightnessScale > fStart);
    bool needPrev1 = (startPrev1 < fEnd && startPrev1 + brightnessScale > fStart);
    bool needCurr  = (startCurr  < fEnd && startCurr  + brightnessScale > fStart);

    vec3 basePrev2 = needPrev2
        ? applyBeamGamma(getPixelFromOrigFrameLinear(uv, crtHzCounter - 2.0, crtHzCounter))
        : vec3(0.0);
    vec3 basePrev1 = needPrev1
        ? applyBeamGamma(getPixelFromOrigFrameLinear(uv, crtHzCounter - 1.0, crtHzCounter))
        : vec3(0.0);
    vec3 baseCurr  = needCurr
        ? applyBeamGamma(getPixelFromOrigFrameLinear(uv, crtHzCounter,       crtHzCounter))
        : vec3(0.0);

    vec3 colorPrev2 = basePrev2 * brightnessScale;
    vec3 colorPrev1 = basePrev1 * brightnessScale;
    vec3 colorCurr  = baseCurr  * brightnessScale;

    vec3 result = vec3(0.0);

    // Process each color channel independently
    for (int ch = 0; ch < 3; ch++)
    {
        // Get brightness lengths for all three cycles
        float Lprev2 = colorPrev2[ch];
        float Lprev1 = colorPrev1[ch];
        float Lcurr  = colorCurr[ch];

        if (Lprev2 <= 0.0 && Lprev1 <= 0.0 && Lcurr <= 0.0) {
            result[ch] = 0.0;
            continue;
        }

        // Define intervals for all three trailing refresh cycles
        float endPrev2 = startPrev2 + Lprev2;
        float endPrev1 = startPrev1 + Lprev1;
        float endCurr  = startCurr  + Lcurr;

        // Calculate overlaps for all three cycles
        #define INTERVAL_OVERLAP(Astart, Aend, Bstart, Bend) max(0.0, min(Aend, Bend) - max(Astart, Bstart))
        float overlapPrev2 = INTERVAL_OVERLAP(startPrev2, endPrev2, fStart, fEnd);
        float overlapPrev1 = INTERVAL_OVERLAP(startPrev1, endPrev1, fStart, fEnd);
        float overlapCurr  = INTERVAL_OVERLAP(startCurr,  endCurr,  fStart, fEnd);

        // Sum all overlaps for final linear brightness
        result[ch] = overlapPrev2 + overlapPrev1 + overlapCurr;
    }

    vec3 activeBase = needPrev1 ? basePrev1 : (needCurr ? baseCurr : basePrev2);
    float steadyLuma = clamp(max(max(activeBase.r, activeBase.g), activeBase.b), 0.0, 1.0);
    return vec4(result, steadyLuma);
}

//-------------------------------------------------------------------------------------------------
// Main Fragment Entry Point (Stage 1: Renders normalized [0, 1]^2 CRT tube linear beam to beam_fbo)

void main() {
    // Keep pipeline uniforms referenced for driver reflection & uniform validation tests
    float _unused_bg = (background_color.r + horizontal_stretch + border_crop.x) * 0.0;

    // uv: Normalized coordinates ranging from (0,0) at the bottom-left to (1,1) at the top-right
    // within the CRT tube raster.
    vec2 uv = clamp(v_tc, 0.0, 1.0) + vec2(_unused_bg);
    vec2 fragCoord = uv * outputResolution;

    // LCD SAVER (prevent image retention)
    // Adds a slew to frames_per_hz when lcd_anti_retention is enabled and frames_per_hz is an exact even integer.
    float effective_frames_per_hz = ((lcd_anti_retention != 0) && IS_EVEN_INTEGER(frames_per_hz))
        ? (frames_per_hz + lcd_inversion_compensation_slew)
        : frames_per_hz;
    effective_frames_per_hz = max(effective_frames_per_hz, 0.001);

    // Frame counter, which may be compensated by slo-mo modes (fps_divisor). Does not need to be integer divisible.
    float effectiveFrame = floor(float(iFrame) * fps_divisor);

    // Normalized raster position [0..1] representing current position of simulated CRT electron beam
    float crtRasterPos = mod(effectiveFrame, effective_frames_per_hz) / effective_frames_per_hz;

    // CRT refresh cycle counter
    float crtHzCounter = floor(effectiveFrame / effective_frames_per_hz);

    vec4 fragColor = vec4(0.0, 0.0, 0.0, 0.0);

    if (splitscreen != 0) {
        // crtArea: Boolean indicating whether the current pixel is within the CRT-BFI region.
        bool crtArea = !((uv.x > splitscreen_x) && (uv.y > splitscreen_y));

        // Calculate border regions (in pixels)
        float borderXpx = abs(fragCoord.x - splitscreen_x * outputResolution.x);
        float borderYpx = abs(fragCoord.y - splitscreen_y * outputResolution.y);

        // Border only exists in the non-BFI region (x > splitscreen_x || y > splitscreen_y)
        bool inBorderX = borderXpx < float(splitscreen_border_px) && uv.y > splitscreen_y;
        bool inBorderY = borderYpx < float(splitscreen_border_px) && uv.x > splitscreen_x;
        bool inBorder = (splitscreen == 1) && (inBorderX || inBorderY);

        if (crtArea) {
            fragColor = getPixelFromSimulatedCRT(uv, crtRasterPos, crtHzCounter, effective_frames_per_hz);
        } else if (inBorder) {
            fragColor = vec4(1.0, 1.0, 1.0, 2.0);
        } else {
            vec3 origLinear = applyBeamGamma(getPixelFromOrigFrameLinear(uv, crtHzCounter, crtHzCounter));
            float steadyLuma = clamp(max(max(origLinear.r, origLinear.g), origLinear.b), 0.0, 1.0);
            if (splitscreen_match_brightness != 0) {
                float effectiveGain = clamp(gain_vs_blur * max(brightboost, 0.5), 0.01, 1.0);
                origLinear *= effectiveGain;
            }
            fragColor = vec4(clamp(origLinear, 0.0, 1.0), steadyLuma);
        }
    } else {
        fragColor = getPixelFromSimulatedCRT(uv, crtRasterPos, crtHzCounter, effective_frames_per_hz);
    }

    out_color = fragColor;
}
