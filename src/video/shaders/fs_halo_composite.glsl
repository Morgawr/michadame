#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D ScanlinesPass;
uniform sampler2D LinearizePass;
uniform sampler2D GlowPass;
uniform sampler2D BloomPass;

uniform vec2 outputResolution;
uniform vec2 SourceSize;
uniform float horizontal_stretch;
uniform float halo_zoom;
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
    vec4 scanline_sample = texture(ScanlinesPass, v_tc);
    vec3 color = scanline_sample.rgb;
    float cval = scanline_sample.a;

    if (cval <= 0.0001) {
        out_color = vec4(0.0);
        return;
    }
    float colmx = max(max(color.r, color.g), color.b);

    // Determine normalized position within source texture for sampling Glow and Bloom
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
    vec2 zoom_scale = vec2(halo_zoom / 100.0);
    vec2 screen_tc = (centered_tc - 0.5) / zoom_scale + 0.5;

    vec2 warped = screen_tc;
    if (curvature != 0) {
        warped = Warp(screen_tc);
    }

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

    // Sample Bloom and Glow
    vec3 bloom_val = texture(BloomPass, clamp(warped, 0.0, 1.0)).rgb;
    vec3 glow_val = texture(GlowPass, clamp(warped, 0.0, 1.0)).rgb;

    if (abs(bloom) > 0.01) {
        vec3 Bloom1 = min(bloom_val * (orig1 + color), max(0.5 * (colmx + orig1 - color), 0.001 * bloom_val));
        Bloom1 = 0.5 * (Bloom1 + mix(Bloom1, mix(colmx * orig1, Bloom1, 0.5), 1.0 - clamp(color, 0.0, 1.0)));
        color = color + abs(bloom) * Bloom1;
    }

    if (abs(halation) > 0.01) {
        vec3 hBloom = 0.5 * (bloom_val + bloom_val * bloom_val);
        color = color + abs(halation) * hBloom;
    }

    if (abs(glow) > 0.01) {
        color = color + abs(glow) * glow_val;
    }

    // Apply vibrance (saturation boost in linear space)
    float luminance = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = mix(vec3(luminance), color, vibrance);

    // Gamma out: linear -> display gamma 2.4
    color = pow(clamp(color, 0.0, 1.0), vec3(1.0 / 2.4));

    out_color = vec4(color, cval);
}
