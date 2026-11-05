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

//-------------------------------------------------------------------------------------------------
// Utility Macros & Functions

#define clampPixel(a) clamp(a, vec3(0.0), vec3(1.0))

// Selection Function: Returns 'b' if 'p' is true, else 'a'.
float SelF1(float a, float b, bool p) { return p ? b : a; }

#define IS_INTEGER(x) (floor(x) == (x))
#define IS_EVEN_INTEGER(x) (IS_INTEGER(x) && IS_INTEGER((x) / 2.0))

// Standard 2.4 sRGB encoding for recovering sRGB from michadame's linear-RGB pass textures
// and for background_color outside the active CRT tube.
float ToSrgb1(float c) {
    return (c < 0.0031308 ? c * 12.92 : 1.055 * pow(max(c, 0.0), 0.41666) - 0.055);
}
vec3 ToSrgb(vec3 c) {
    return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
}

//-------------------------------------------------------------------------------------------------
// sRGB Encoding and Decoding Functions, to gamma correct/uncorrect (Blur Busters specification)

// Encode linear color to sRGB. (applies gamma curve)
float linear2srgb(float c) {
    vec3 j = vec3(0.0031308 * 12.92, 12.92, 1.0 / max(gamma, 0.01));
    vec2 k = vec2(1.055, -0.055);
    return clamp(j.x, c * j.y, pow(max(c, 0.0), j.z) * k.x + k.y);
}
vec3 linear2srgb(vec3 c) {
    return vec3(linear2srgb(c.r), linear2srgb(c.g), linear2srgb(c.b));
}

// Decode sRGB color to linear. (undoes gamma curve)
float srgb2linear(float c) {
    vec3 j = vec3(0.04045, 1.0 / 12.92, max(gamma, 0.01));
    vec2 k = vec2(1.0 / 1.055, 0.055 / 1.055);
    return SelF1(c * j.y, pow(max(c * k.x + k.y, 0.0), j.z), c > j.x);
}
vec3 srgb2linear(vec3 c) {
    return vec3(srgb2linear(c.r), srgb2linear(c.g), srgb2linear(c.b));
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

    vec3 color = vec3(0.0);
    float totalWeight = 0.0;

    for (int y = -1; y <= 2; y++) {
        float wy = lanczosWeight(float(y) - f.y);
        for (int x = -1; x <= 2; x++) {
            float wx = lanczosWeight(float(x) - f.x);
            float weight = wx * wy;

            vec2 sampleTc = (baseTc + vec2(x, y) + 0.5) * invTexSize;
            color += texture(tex, clamp(sampleTc, 0.0, 1.0)).rgb * weight;
            totalWeight += weight;
        }
    }
    return color / totalWeight;
}

vec3 SampleVideo(sampler2D tex, vec2 uv, vec2 texSize, int shader_filter) {
    if (shader_filter == 2) { // Bicubic
        return SampleBicubic(tex, uv, texSize);
    } else if (shader_filter == 4) { // Lanczos
        return SampleLanczos(tex, uv, texSize);
    } else {
        return texture(tex, uv).rgb;
    }
}

//-------------------------------------------------------------------------------------------------
// Gets pixel from the 3 trailing unprocessed CRT refresh cycle framebuffers.

vec3 getPixelFromOrigFrame(vec2 uv, float getFromHzNumber, float currentHzCounter) {
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
    vec3 vibrance_linear = clamp(mix(vec3(luminance), linear_sampled, vibrance), 0.0, 1.0);
    return clampPixel(ToSrgb(vibrance_linear));
}

//-------------------------------------------------------------------------------------------------
// CRT Rolling Scan Simulation With Phosphor Fade + Brightness Redistributor Algorithm

vec3 getPixelFromSimulatedCRT(vec2 uv, float crtRasterPos, float crtHzCounter, float framesPerHz)
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

    // Convert normalized values to frame space and check maximum possible interval bounds
    // before sampling 16-tap Bicubic/Lanczos textures at 4K.
    float brightnessScale = framesPerHz * gain_vs_blur;
    float tubeFrame = tubePos * framesPerHz;
    float fStart = crtRasterPos * framesPerHz;
    float fEnd = fStart + 1.0;

    float startPrev2 = tubeFrame - framesPerHz;
    float startPrev1 = tubeFrame;
    float startCurr  = tubeFrame + framesPerHz; // Fix seam for top edge

    // Only sample trailing refresh cycle textures whose maximum photon budget interval
    // [start, start + brightnessScale] can overlap the active sub-frame [fStart, fEnd].
    vec3 colorPrev2 = (startPrev2 < fEnd && startPrev2 + brightnessScale > fStart)
        ? srgb2linear(getPixelFromOrigFrame(uv, crtHzCounter - 2.0, crtHzCounter)) * brightnessScale
        : vec3(0.0);
    vec3 colorPrev1 = (startPrev1 < fEnd && startPrev1 + brightnessScale > fStart)
        ? srgb2linear(getPixelFromOrigFrame(uv, crtHzCounter - 1.0, crtHzCounter)) * brightnessScale
        : vec3(0.0);
    vec3 colorCurr  = (startCurr < fEnd && startCurr + brightnessScale > fStart)
        ? srgb2linear(getPixelFromOrigFrame(uv, crtHzCounter,      crtHzCounter)) * brightnessScale
        : vec3(0.0);

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

        // Sum all overlaps for final brightness
        result[ch] = overlapPrev2 + overlapPrev1 + overlapCurr;
    }

    return linear2srgb(result);
}

//-------------------------------------------------------------------------------------------------
// Main Fragment Entry Point

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

    if (centered_tc.x < border_crop.x || centered_tc.x > (1.0 - border_crop.y) ||
        centered_tc.y < border_crop.z || centered_tc.y > (1.0 - border_crop.w)) {
        out_color = vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }

    // uv: Normalized coordinates ranging from (0,0) at the bottom-left to (1,1) at the top-right
    // within the active CRT tube area.
    vec2 uv = vec2(centered_tc.x, 1.0 - centered_tc.y);
    vec2 video_res_px = outputResolution * scale;
    vec2 fragCoord = uv * video_res_px;

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

    vec4 fragColor = vec4(0.0, 0.0, 0.0, 1.0);

    if (splitscreen != 0) {
        // crtArea: Boolean indicating whether the current pixel is within the CRT-BFI region.
        bool crtArea = !((uv.x > splitscreen_x) && (uv.y > splitscreen_y));

        // Calculate border regions (in pixels)
        float borderXpx = abs(fragCoord.x - splitscreen_x * video_res_px.x);
        float borderYpx = abs(fragCoord.y - splitscreen_y * video_res_px.y);

        // Border only exists in the non-BFI region (x > splitscreen_x || y > splitscreen_y)
        bool inBorderX = borderXpx < float(splitscreen_border_px) && uv.y > splitscreen_y;
        bool inBorderY = borderYpx < float(splitscreen_border_px) && uv.x > splitscreen_x;
        bool inBorder = (splitscreen == 1) && (inBorderX || inBorderY);

        if (crtArea) {
            fragColor.rgb = getPixelFromSimulatedCRT(uv, crtRasterPos, crtHzCounter, effective_frames_per_hz);
        } else if (!inBorder) {
            fragColor.rgb = getPixelFromOrigFrame(uv, crtHzCounter, crtHzCounter);
            if (splitscreen_match_brightness != 0) {
                // Brightness compensation for unprocessed pixels through similar gamma-curve (match gamma of simulated CRT)
                fragColor.rgb = srgb2linear(fragColor.rgb) * gain_vs_blur;
                fragColor.rgb = clampPixel(linear2srgb(fragColor.rgb));
            }
        }
    } else {
        fragColor.rgb = getPixelFromSimulatedCRT(uv, crtRasterPos, crtHzCounter, effective_frames_per_hz);
    }

    out_color = fragColor;
}
