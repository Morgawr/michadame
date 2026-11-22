#version 330 core
// AMD FidelityFX Super Resolution 1.0 (FSR 1.0) - RCAS (Robust Contrast Adaptive Sharpening)
// Sharpens in perceptual sRGB space with noise suppression and converts back to linear RGB
// for Michadame's downstream presentation and CRT shader pipeline.

out vec4 out_color;

uniform sampler2D input_tex;
uniform float rcas_sharpness; // exp2(-stops), e.g. exp2(-0.2) ~= 0.87055 (AMD recommended default)
uniform int input_is_linear;  // 0 when fed by EASU sRGB output, 1 when EASU is bypassed

const float FSR_RCAS_LIMIT = 0.25 - (1.0 / 16.0); // 0.1875

float ToSrgb1(float c) {
    c = max(c, 0.0);
    return (c <= 0.0031308) ? (c * 12.92) : (1.055 * pow(c, 1.0 / 2.4) - 0.055);
}

vec3 ToSrgb(vec3 c) {
    return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
}

float ToLinear1(float c) {
    c = clamp(c, 0.0, 1.0);
    return (c <= 0.04045) ? (c / 12.92) : pow((c + 0.055) / 1.055, 2.4);
}

vec3 ToLinear(vec3 c) {
    return vec3(ToLinear1(c.r), ToLinear1(c.g), ToLinear1(c.b));
}

vec3 FetchSrgb(ivec2 coord, ivec2 max_coord) {
    vec3 c = texelFetch(input_tex, clamp(coord, ivec2(0), max_coord), 0).rgb;
    if (input_is_linear != 0) {
        c = ToSrgb(c);
    }
    return clamp(c, 0.0, 1.0);
}

float FsrLuma(vec3 c) {
    return c.b * 0.5 + (c.r * 0.5 + c.g);
}

void main() {
    ivec2 tex_size = textureSize(input_tex, 0);
    ivec2 max_coord = max(tex_size - ivec2(1), ivec2(0));
    ivec2 sp = ivec2(gl_FragCoord.xy);

    // 5-tap cross neighborhood:
    //      b
    //   d  e  f
    //      h
    vec3 b = FetchSrgb(sp + ivec2( 0, -1), max_coord);
    vec3 d = FetchSrgb(sp + ivec2(-1,  0), max_coord);
    vec3 e = FetchSrgb(sp,                 max_coord);
    vec3 f = FetchSrgb(sp + ivec2( 1,  0), max_coord);
    vec3 h = FetchSrgb(sp + ivec2( 0,  1), max_coord);

    float bL = FsrLuma(b);
    float dL = FsrLuma(d);
    float eL = FsrLuma(e);
    float fL = FsrLuma(f);
    float hL = FsrLuma(h);

    // Noise detection (FSR_RCAS_DENOISE) reduces sharpening on isolated compression/grain spikes.
    float nz = 0.25 * (bL + dL + fL + hL) - eL;
    float maxL = max(max(max(bL, dL), max(eL, fL)), hL);
    float minL = min(min(min(bL, dL), min(eL, fL)), hL);
    nz = clamp(abs(nz) / max(maxL - minL, 1e-6), 0.0, 1.0);
    nz = -0.5 * nz + 1.0;

    // Min and max of the 4 outer cross taps per channel.
    vec3 mn4 = min(min(b, d), min(f, h));
    vec3 mx4 = max(max(b, d), max(f, h));

    // Limiters prevent clipping below 0 or above 1 on any RGB channel.
    vec3 hitMin = min(mn4, e) / max(4.0 * mx4, vec3(1e-6));
    vec3 hitMax = (1.0 - max(mx4, e)) / min(4.0 * mn4 - 4.0, vec3(-1e-6));
    vec3 lobeRGB = max(-hitMin, hitMax);

    float lobe = max(-FSR_RCAS_LIMIT, min(max(lobeRGB.r, max(lobeRGB.g, lobeRGB.b)), 0.0));
    lobe *= clamp(rcas_sharpness, 0.0, 1.0) * nz;

    float rcpL = 1.0 / (4.0 * lobe + 1.0);
    vec3 color = clamp((lobe * (b + d + f + h) + e) * rcpL, 0.0, 1.0);

    out_color = vec4(ToLinear(color), 1.0);
}
