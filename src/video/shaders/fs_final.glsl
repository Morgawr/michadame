#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D video_texture;

uniform vec2 outputResolution;

uniform float hardScan;
uniform float hardPix;
uniform float warpX;
uniform float warpY;
uniform float shadowMask;
uniform float brightboost;
uniform float hardBloomPix;
uniform float hardBloomScan;
uniform float bloomAmount;
uniform float shape;
uniform vec3 background_color;
uniform float horizontal_stretch;
uniform float vibrance;
uniform vec4 border_crop; // left, right, top, bottom

float ToSrgb1(float c) {
    return (c < 0.0031308 ? c * 12.92 : 1.055 * pow(c, 0.41666) - 0.055);
}

vec3 ToSrgb(vec3 c) {
    return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
}

vec2 Warp(vec2 pos) {
    pos = pos * 2.0 - 1.0;
    pos *= vec2(1.0 + (pos.y * pos.y) * warpX, 1.0 + (pos.x * pos.x) * warpY);
    return pos * 0.5 + 0.5;
}

float Gaus(float pos, float scale) {
    return exp2(scale * pow(abs(pos), shape));
}

vec3 FetchTexel(vec2 base_pix, vec2 off, vec2 source_size) {
    vec2 sample_pos = (base_pix + off + vec2(0.5)) / source_size;
    if (sample_pos.x < 0.0 || sample_pos.x > 1.0 || sample_pos.y < 0.0 || sample_pos.y > 1.0) {
        return vec3(0.0);
    }
    return texture(video_texture, sample_pos).rgb;
}

vec3 EvaluateScanlinesAndBloom(vec2 pos, vec2 source_size) {
    vec2 pix_coord = pos * source_size;
    vec2 base_pix = floor(pix_coord);
    vec2 dst = -((pix_coord - base_pix) - vec2(0.5));

    // Shared horizontal Gaussian weights for Horz3 / Horz5 (hardPix)
    float wa = Gaus(dst.x - 2.0, hardPix);
    float wb = Gaus(dst.x - 1.0, hardPix);
    float wc = Gaus(dst.x, hardPix);
    float wd = Gaus(dst.x + 1.0, hardPix);
    float we = Gaus(dst.x + 2.0, hardPix);
    float inv_w3 = 1.0 / (wb + wc + wd);
    float inv_w5 = 1.0 / (wa + wb + wc + wd + we);

    // Core 3-row scanline neighborhood (11 texels)
    vec3 c_m1_m1 = FetchTexel(base_pix, vec2(-1.0, -1.0), source_size);
    vec3 c_0_m1  = FetchTexel(base_pix, vec2( 0.0, -1.0), source_size);
    vec3 c_p1_m1 = FetchTexel(base_pix, vec2( 1.0, -1.0), source_size);

    vec3 c_m2_0  = FetchTexel(base_pix, vec2(-2.0,  0.0), source_size);
    vec3 c_m1_0  = FetchTexel(base_pix, vec2(-1.0,  0.0), source_size);
    vec3 c_0_0   = FetchTexel(base_pix, vec2( 0.0,  0.0), source_size);
    vec3 c_p1_0  = FetchTexel(base_pix, vec2( 1.0,  0.0), source_size);
    vec3 c_p2_0  = FetchTexel(base_pix, vec2( 2.0,  0.0), source_size);

    vec3 c_m1_p1 = FetchTexel(base_pix, vec2(-1.0,  1.0), source_size);
    vec3 c_0_p1  = FetchTexel(base_pix, vec2( 0.0,  1.0), source_size);
    vec3 c_p1_p1 = FetchTexel(base_pix, vec2( 1.0,  1.0), source_size);

    vec3 tri_a = (c_m1_m1 * wb + c_0_m1 * wc + c_p1_m1 * wd) * inv_w3;
    vec3 tri_b = (c_m2_0 * wa + c_m1_0 * wb + c_0_0 * wc + c_p1_0 * wd + c_p2_0 * we) * inv_w5;
    vec3 tri_c = (c_m1_p1 * wb + c_0_p1 * wc + c_p1_p1 * wd) * inv_w3;

    float swa = Gaus(dst.y - 1.0, hardScan);
    float swb = Gaus(dst.y, hardScan);
    float swc = Gaus(dst.y + 1.0, hardScan);
    vec3 color = tri_a * swa + tri_b * swb + tri_c * swc;

    if (bloomAmount > 0.0001) {
        // Shared horizontal Gaussian weights for Horz7 (hardBloomPix)
        float bwa = Gaus(dst.x - 3.0, hardBloomPix);
        float bwb = Gaus(dst.x - 2.0, hardBloomPix);
        float bwc = Gaus(dst.x - 1.0, hardBloomPix);
        float bwd = Gaus(dst.x, hardBloomPix);
        float bwe = Gaus(dst.x + 1.0, hardBloomPix);
        float bwf = Gaus(dst.x + 2.0, hardBloomPix);
        float bwg = Gaus(dst.x + 3.0, hardBloomPix);
        float inv_bw7 = 1.0 / (bwa + bwb + bwc + bwd + bwe + bwf + bwg);

        // Fetch only the 20 outer bloom texels; reuse the 11 inner scanline texels
        vec3 c_m2_m2 = FetchTexel(base_pix, vec2(-2.0, -2.0), source_size);
        vec3 c_m1_m2 = FetchTexel(base_pix, vec2(-1.0, -2.0), source_size);
        vec3 c_0_m2  = FetchTexel(base_pix, vec2( 0.0, -2.0), source_size);
        vec3 c_p1_m2 = FetchTexel(base_pix, vec2( 1.0, -2.0), source_size);
        vec3 c_p2_m2 = FetchTexel(base_pix, vec2( 2.0, -2.0), source_size);

        vec3 c_m3_m1 = FetchTexel(base_pix, vec2(-3.0, -1.0), source_size);
        vec3 c_m2_m1 = FetchTexel(base_pix, vec2(-2.0, -1.0), source_size);
        vec3 c_p2_m1 = FetchTexel(base_pix, vec2( 2.0, -1.0), source_size);
        vec3 c_p3_m1 = FetchTexel(base_pix, vec2( 3.0, -1.0), source_size);

        vec3 c_m3_0  = FetchTexel(base_pix, vec2(-3.0,  0.0), source_size);
        vec3 c_p3_0  = FetchTexel(base_pix, vec2( 3.0,  0.0), source_size);

        vec3 c_m3_p1 = FetchTexel(base_pix, vec2(-3.0,  1.0), source_size);
        vec3 c_m2_p1 = FetchTexel(base_pix, vec2(-2.0,  1.0), source_size);
        vec3 c_p2_p1 = FetchTexel(base_pix, vec2( 2.0,  1.0), source_size);
        vec3 c_p3_p1 = FetchTexel(base_pix, vec2( 3.0,  1.0), source_size);

        vec3 c_m2_p2 = FetchTexel(base_pix, vec2(-2.0,  2.0), source_size);
        vec3 c_m1_p2 = FetchTexel(base_pix, vec2(-1.0,  2.0), source_size);
        vec3 c_0_p2  = FetchTexel(base_pix, vec2( 0.0,  2.0), source_size);
        vec3 c_p1_p2 = FetchTexel(base_pix, vec2( 1.0,  2.0), source_size);
        vec3 c_p2_p2 = FetchTexel(base_pix, vec2( 2.0,  2.0), source_size);

        vec3 bl_a = (c_m2_m2 * wa + c_m1_m2 * wb + c_0_m2 * wc + c_p1_m2 * wd + c_p2_m2 * we) * inv_w5;
        vec3 bl_b = (c_m3_m1 * bwa + c_m2_m1 * bwb + c_m1_m1 * bwc + c_0_m1 * bwd + c_p1_m1 * bwe + c_p2_m1 * bwf + c_p3_m1 * bwg) * inv_bw7;
        vec3 bl_c = (c_m3_0 * bwa + c_m2_0 * bwb + c_m1_0 * bwc + c_0_0 * bwd + c_p1_0 * bwe + c_p2_0 * bwf + c_p3_0 * bwg) * inv_bw7;
        vec3 bl_d = (c_m3_p1 * bwa + c_m2_p1 * bwb + c_m1_p1 * bwc + c_0_p1 * bwd + c_p1_p1 * bwe + c_p2_p1 * bwf + c_p3_p1 * bwg) * inv_bw7;
        vec3 bl_e = (c_m2_p2 * wa + c_m1_p2 * wb + c_0_p2 * wc + c_p1_p2 * wd + c_p2_p2 * we) * inv_w5;

        float bswa = Gaus(dst.y - 2.0, hardBloomScan);
        float bswb = Gaus(dst.y - 1.0, hardBloomScan);
        float bswc = Gaus(dst.y, hardBloomScan);
        float bswd = Gaus(dst.y + 1.0, hardBloomScan);
        float bswe = Gaus(dst.y + 2.0, hardBloomScan);

        vec3 bloom = bl_a * bswa + bl_b * bswb + bl_c * bswc + bl_d * bswd + bl_e * bswe;
        color += bloom * bloomAmount;
    }

    return color;
}

vec3 Mask(vec2 pos) {
    const float maskDark = 0.5;
    const float maskLight = 1.5;
    vec3 mask = vec3(maskDark);

    if (shadowMask == 1.0) {
        float line = maskLight;
        float odd = 0.0;
        if (fract(pos.x * 0.166666666) < 0.5) {
            odd = 1.0;
        }
        if (fract((pos.y + odd) * 0.5) < 0.5) {
            line = maskDark;
        }
        pos.x = fract(pos.x * 0.333333333);
        if (pos.x < 0.333) {
            mask.r = maskLight;
        } else if (pos.x < 0.666) {
            mask.g = maskLight;
        } else {
            mask.b = maskLight;
        }
        mask *= line;
    } else if (shadowMask == 2.0) {
        pos.x = fract(pos.x * 0.333333333);
        if (pos.x < 0.333) {
            mask.r = maskLight;
        } else if (pos.x < 0.666) {
            mask.g = maskLight;
        } else {
            mask.b = maskLight;
        }
    } else if (shadowMask == 3.0) {
        pos.x += pos.y * 3.0;
        pos.x = fract(pos.x * 0.166666666);
        if (pos.x < 0.333) {
            mask.r = maskLight;
        } else if (pos.x < 0.666) {
            mask.g = maskLight;
        } else {
            mask.b = maskLight;
        }
    } else if (shadowMask == 4.0) {
        pos.xy = floor(pos.xy * vec2(1.0, 0.5));
        pos.x += pos.y * 3.0;
        pos.x = fract(pos.x * 0.166666666);
        if (pos.x < 0.333) {
            mask.r = maskLight;
        } else if (pos.x < 0.666) {
            mask.g = maskLight;
        } else {
            mask.b = maskLight;
        }
    }

    return mask;
}

void main() {
    vec2 corrected_tc = vec2(v_tc.x, 1.0 - v_tc.y);
    vec2 source_size = vec2(textureSize(video_texture, 0));
    float video_aspect = (source_size.x * horizontal_stretch) / source_size.y;
    float output_aspect = outputResolution.x / outputResolution.y;

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

    vec2 warped_tc = Warp(centered_tc);
    if (warped_tc.x < border_crop.x || warped_tc.x > (1.0 - border_crop.y) ||
        warped_tc.y < border_crop.z || warped_tc.y > (1.0 - border_crop.w)) {
        out_color = vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }

    vec3 final_color = EvaluateScanlinesAndBloom(warped_tc, source_size);
    final_color *= brightboost;

    if (shadowMask > 0.0) {
        final_color *= Mask(gl_FragCoord.xy * 1.000001);
    }

    float luminance = dot(final_color, vec3(0.2126, 0.7152, 0.0722));
    final_color = mix(vec3(luminance), final_color, vibrance);

    out_color = vec4(ToSrgb(final_color), 1.0);
}
