#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D video_texture;
uniform vec2 outputResolution;
uniform vec2 source_size;
uniform float horizontal_stretch;
uniform vec4 border_crop; // left, right, top, bottom
uniform vec2 warp;
uniform float corner_size;
uniform int filter_type;
uniform float glow_intensity; // 0.0 to 1.0

// Convert from linear to sRGB color space for video samples
float ToSrgb1(float c) {
    c = clamp(c, 0.0, 1.0);
    return (c < 0.0031308 ? c * 12.92 : 1.055 * pow(c, 0.41666) - 0.055);
}
vec3 ToSrgb(vec3 c) {
    return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
}

// 8 directions evenly distributed around the unit circle
const vec2 ring1_dirs[8] = vec2[8](
    vec2( 1.0000,  0.0000),
    vec2( 0.7071,  0.7071),
    vec2( 0.0000,  1.0000),
    vec2(-0.7071,  0.7071),
    vec2(-1.0000,  0.0000),
    vec2(-0.7071, -0.7071),
    vec2( 0.0000, -1.0000),
    vec2( 0.7071, -0.7071)
);

// 8 directions rotated by 22.5 degrees for uniform isotropic 16-direction angular coverage
const vec2 ring2_dirs[8] = vec2[8](
    vec2( 0.9239,  0.3827),
    vec2( 0.3827,  0.9239),
    vec2(-0.3827,  0.9239),
    vec2(-0.9239,  0.3827),
    vec2(-0.9239, -0.3827),
    vec2(-0.3827, -0.9239),
    vec2( 0.3827, -0.9239),
    vec2( 0.9239, -0.3827)
);

void main() {
    if (glow_intensity <= 0.001) {
        out_color = vec4(0.0);
        return;
    }

    // 1. Calculate active video coordinate mapping
    float video_aspect = (source_size.x * horizontal_stretch) / max(source_size.y, 1.0);
    float output_aspect = outputResolution.x / max(outputResolution.y, 1.0);

    vec2 scale = vec2(1.0, 1.0);
    if (video_aspect > output_aspect) {
        scale.y = output_aspect / video_aspect;
    } else {
        scale.x = video_aspect / output_aspect;
    }

    vec2 corrected_tc = vec2(v_tc.x, 1.0 - v_tc.y);
    vec2 centered_tc = (corrected_tc - 0.5) / scale + 0.5;

    // Apply CRT curvature warp matching active CRT filter
    vec2 screen_tc = centered_tc;
    if (filter_type == 1) { // Lottes warp
        vec2 pos = centered_tc * 2.0 - 1.0;
        vec2 eff_warp = max(warp, vec2(0.044, 0.054));
        pos *= vec2(1.0 + (pos.y * pos.y) * eff_warp.x, 1.0 + (pos.x * pos.x) * eff_warp.y);
        screen_tc = pos * 0.5 + 0.5;
    } else if (filter_type == 2) { // Halo curvature
        vec2 pos = centered_tc * 2.0 - 1.0;
        vec2 eff_warp = (warp.x > 0.001 || warp.y > 0.001) ? vec2(0.044, 0.054) : vec2(0.024, 0.030);
        pos *= vec2(1.0 + (pos.y * pos.y) * eff_warp.x, 1.0 + (pos.x * pos.x) * eff_warp.y);
        screen_tc = pos * 0.5 + 0.5;
    } else {
        vec2 pos = centered_tc * 2.0 - 1.0;
        pos *= vec2(1.0 + (pos.y * pos.y) * 0.024, 1.0 + (pos.x * pos.x) * 0.030);
        screen_tc = pos * 0.5 + 0.5;
    }

    // Active screen area matching bezel opening
    vec2 ap_half = vec2(0.5 * (1.0 - border_crop.x - border_crop.y), 0.5 * (1.0 - border_crop.z - border_crop.w));
    vec2 ap_center = vec2(0.5 * (border_crop.x + (1.0 - border_crop.y)), 0.5 * (border_crop.z + (1.0 - border_crop.w)));
    vec2 min_uv = ap_center - ap_half;
    vec2 max_uv = ap_center + ap_half;
    vec2 screen_center = ap_center;

    vec2 video_pixels = outputResolution * scale;
    float min_vid_dim = max(min(video_pixels.x, video_pixels.y), 1.0);

    vec2 p_vid = (screen_tc - ap_center) * video_pixels;
    vec2 b_vid = ap_half * video_pixels;

    // Molded corner radius in physical screen pixels (matching fs_retro_frame.glsl)
    float r_corner = (filter_type == 2 && corner_size > 0.001)
        ? max(corner_size * min_vid_dim, 22.0)
        : clamp(min_vid_dim * 0.038, 20.0, 32.0);

    // Exact Euclidean rounded box SDF matching the CRT aperture opening:
    vec2 q = abs(p_vid) - (b_vid - vec2(r_corner));
    vec2 max_q = max(q, vec2(0.0));
    float len_max_q = length(max_q);
    float d_ap = min(max(q.x, q.y), 0.0) + len_max_q - r_corner;

    // Continuous 2D unit normal pointing outward from the rounded aperture
    vec2 n_dir = (len_max_q > 0.0001) ? (max_q / len_max_q) : ((q.x > q.y) ? vec2(1.0, 0.0) : vec2(0.0, 1.0));
    vec2 n_ap = n_dir * sign(p_vid);

    // Nearest emitter point on active screen aperture
    vec2 s0 = screen_tc - (n_ap * max(d_ap, 0.0)) / video_pixels;
    s0 = clamp(s0, min_uv + 0.002, max_uv - 0.002);

    // Euclidean distance outside the rounded aperture in normalized screenspace
    float dist_outside = max(d_ap, 0.0) / min_vid_dim;

    // Fade halo inside active video feed so it doesn't overexpose internal screen pixels,
    // while blooming outward immediately at the aperture boundary (no dead band)
    float gate = smoothstep(0.000, 0.003, dist_outside);

    // Aspect-ratio-adjusted Euclidean screenspace metrics for radial sampling
    vec2 aspect_scale = vec2(outputResolution.x / max(outputResolution.y, 1.0), 1.0);

    // True radial optical PSF convolution:
    // Samples local screen emitters in concentric radial circles around s0.
    // Each sample represents calibrated optical irradiance that drops off steeply with 2D Euclidean distance,
    // naturally expanding in an isotropic circle around bright features without overblowing full-white scenes.
    vec3 glow_acc = vec3(0.0);

    // Center emitter tap (screen boundary near fragment)
    vec3 c0 = ToSrgb(textureLod(video_texture, s0, 3.5).rgb);
    float lum0 = dot(c0, vec3(0.2126, 0.7152, 0.0722));
    float e0 = pow(lum0, 1.25);
    vec3 p0 = mix(c0, vec3(lum0), clamp((lum0 - 0.40) * 1.5, 0.0, 0.70)) * e0;
    float w0 = exp(-dist_outside * 22.0) * 0.45;
    glow_acc += p0 * w0;

    // Ring 1: Near-field bloom (8 directions, radius 0.065, LOD 4.0)
    for (int k = 0; k < 8; k++) {
        vec2 sk = clamp(s0 + ring1_dirs[k] * 0.065, min_uv, max_uv);
        float dk = length((screen_tc - sk) * scale * aspect_scale);
        vec3 ck = ToSrgb(textureLod(video_texture, sk, 4.0).rgb);
        float lumk = dot(ck, vec3(0.2126, 0.7152, 0.0722));
        float ek = pow(lumk, 1.25);
        vec3 pk = mix(ck, vec3(lumk), clamp((lumk - 0.40) * 1.5, 0.0, 0.70)) * ek;
        float wk = exp(-dk * 16.0) * 0.045;
        glow_acc += pk * wk;
    }

    // Ring 2: Mid-field room halo (8 directions, radius 0.150, LOD 5.2)
    for (int m = 0; m < 8; m++) {
        vec2 sm = clamp(s0 + ring2_dirs[m] * 0.150, min_uv, max_uv);
        float dm = length((screen_tc - sm) * scale * aspect_scale);
        vec3 cm = ToSrgb(textureLod(video_texture, sm, 5.2).rgb);
        float lumm = dot(cm, vec3(0.2126, 0.7152, 0.0722));
        float em = pow(lumm, 1.25);
        vec3 pm = mix(cm, vec3(lumm), clamp((lumm - 0.40) * 1.5, 0.0, 0.70)) * em;
        float wm = exp(-dm * 10.0) * 0.025;
        glow_acc += pm * wm;
    }

    // Deep interior tap: pulling general room illumination from deeper in the active frame
    vec2 s_deep = clamp(s0 + (screen_center - s0) * 0.35, min_uv, max_uv);
    float d_deep = length((screen_tc - s_deep) * scale * aspect_scale);
    vec3 c_deep = ToSrgb(textureLod(video_texture, s_deep, 6.2).rgb);
    float lum_deep = dot(c_deep, vec3(0.2126, 0.7152, 0.0722));
    float e_deep = pow(lum_deep, 1.25);
    vec3 p_deep = mix(c_deep, vec3(lum_deep), clamp((lum_deep - 0.40) * 1.5, 0.0, 0.70)) * e_deep;
    float w_deep = exp(-d_deep * 7.0) * 0.040;
    glow_acc += p_deep * w_deep;

    // Energy-conserving optical halo with soft-knee saturation compression:
    // Prevents parameter runaway or washouts in high-exposure scenes while preserving
    // delicate, atmospheric halos around textboxes and high-contrast features.
    vec3 raw_halo = glow_acc * (gate * glow_intensity);
    vec3 final_halo = raw_halo / (vec3(1.0) + raw_halo * 0.50);

    out_color = vec4(final_halo, 1.0);
}
