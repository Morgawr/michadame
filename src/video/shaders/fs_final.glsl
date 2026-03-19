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

vec2 Dist(vec2 pos, vec2 source_size) {
    pos *= source_size;
    return -((pos - floor(pos)) - vec2(0.5));
}

float Gaus(float pos, float scale) {
    return exp2(scale * pow(abs(pos), shape));
}

vec3 Fetch(vec2 pos, vec2 off, vec2 source_size) {
    vec2 sample_pos = (floor(pos * source_size + off) + vec2(0.5)) / source_size;
    if (sample_pos.x < 0.0 || sample_pos.x > 1.0 || sample_pos.y < 0.0 || sample_pos.y > 1.0) {
        return vec3(0.0);
    }
    return texture(video_texture, sample_pos).rgb;
}

vec3 Horz3(vec2 pos, float off, vec2 source_size) {
    vec3 b = Fetch(pos, vec2(-1.0, off), source_size);
    vec3 c = Fetch(pos, vec2(0.0, off), source_size);
    vec3 d = Fetch(pos, vec2(1.0, off), source_size);
    float dst = Dist(pos, source_size).x;
    float wb = Gaus(dst - 1.0, hardPix);
    float wc = Gaus(dst, hardPix);
    float wd = Gaus(dst + 1.0, hardPix);
    return (b * wb + c * wc + d * wd) / (wb + wc + wd);
}

vec3 Horz5(vec2 pos, float off, vec2 source_size) {
    vec3 a = Fetch(pos, vec2(-2.0, off), source_size);
    vec3 b = Fetch(pos, vec2(-1.0, off), source_size);
    vec3 c = Fetch(pos, vec2(0.0, off), source_size);
    vec3 d = Fetch(pos, vec2(1.0, off), source_size);
    vec3 e = Fetch(pos, vec2(2.0, off), source_size);
    float dst = Dist(pos, source_size).x;
    float wa = Gaus(dst - 2.0, hardPix);
    float wb = Gaus(dst - 1.0, hardPix);
    float wc = Gaus(dst, hardPix);
    float wd = Gaus(dst + 1.0, hardPix);
    float we = Gaus(dst + 2.0, hardPix);
    return (a * wa + b * wb + c * wc + d * wd + e * we) / (wa + wb + wc + wd + we);
}

vec3 Horz7(vec2 pos, float off, vec2 source_size) {
    vec3 a = Fetch(pos, vec2(-3.0, off), source_size);
    vec3 b = Fetch(pos, vec2(-2.0, off), source_size);
    vec3 c = Fetch(pos, vec2(-1.0, off), source_size);
    vec3 d = Fetch(pos, vec2(0.0, off), source_size);
    vec3 e = Fetch(pos, vec2(1.0, off), source_size);
    vec3 f = Fetch(pos, vec2(2.0, off), source_size);
    vec3 g = Fetch(pos, vec2(3.0, off), source_size);
    float dst = Dist(pos, source_size).x;
    float wa = Gaus(dst - 3.0, hardBloomPix);
    float wb = Gaus(dst - 2.0, hardBloomPix);
    float wc = Gaus(dst - 1.0, hardBloomPix);
    float wd = Gaus(dst, hardBloomPix);
    float we = Gaus(dst + 1.0, hardBloomPix);
    float wf = Gaus(dst + 2.0, hardBloomPix);
    float wg = Gaus(dst + 3.0, hardBloomPix);
    return (a * wa + b * wb + c * wc + d * wd + e * we + f * wf + g * wg)
        / (wa + wb + wc + wd + we + wf + wg);
}

float Scan(vec2 pos, float off, vec2 source_size) {
    return Gaus(Dist(pos, source_size).y + off, hardScan);
}

float BloomScan(vec2 pos, float off, vec2 source_size) {
    return Gaus(Dist(pos, source_size).y + off, hardBloomScan);
}

vec3 Tri(vec2 pos, vec2 source_size) {
    vec3 a = Horz3(pos, -1.0, source_size);
    vec3 b = Horz5(pos, 0.0, source_size);
    vec3 c = Horz3(pos, 1.0, source_size);
    float wa = Scan(pos, -1.0, source_size);
    float wb = Scan(pos, 0.0, source_size);
    float wc = Scan(pos, 1.0, source_size);
    return a * wa + b * wb + c * wc;
}

vec3 Bloom(vec2 pos, vec2 source_size) {
    vec3 a = Horz5(pos, -2.0, source_size);
    vec3 b = Horz7(pos, -1.0, source_size);
    vec3 c = Horz7(pos, 0.0, source_size);
    vec3 d = Horz7(pos, 1.0, source_size);
    vec3 e = Horz5(pos, 2.0, source_size);
    float wa = BloomScan(pos, -2.0, source_size);
    float wb = BloomScan(pos, -1.0, source_size);
    float wc = BloomScan(pos, 0.0, source_size);
    float wd = BloomScan(pos, 1.0, source_size);
    float we = BloomScan(pos, 2.0, source_size);
    return a * wa + b * wb + c * wc + d * wd + e * we;
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
    if (warped_tc.x < 0.0 || warped_tc.x > 1.0 || warped_tc.y < 0.0 || warped_tc.y > 1.0) {
        out_color = vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }

    vec3 final_color = Tri(warped_tc, source_size);
    final_color += Bloom(warped_tc, source_size) * bloomAmount;
    final_color *= brightboost;

    if (shadowMask > 0.0) {
        final_color *= Mask(gl_FragCoord.xy * 1.000001);
    }

    float luminance = dot(final_color, vec3(0.2126, 0.7152, 0.0722));
    final_color = mix(vec3(luminance), final_color, vibrance);

    out_color = vec4(ToSrgb(final_color), 1.0);
}
