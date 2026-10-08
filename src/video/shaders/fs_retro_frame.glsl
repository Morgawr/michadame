#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D bezel_texture;
uniform sampler2D video_texture;
uniform vec2 outputResolution;
uniform vec2 source_size;
uniform float horizontal_stretch;
uniform vec4 border_crop; // left, right, top, bottom
uniform vec2 warp;        // warpX, warpY
uniform float corner_size;
uniform int filter_type;  // 0 = None, 1 = Lottes, 2 = Halo
uniform float time;
uniform float ambient_glow;
uniform int dark_mode; // 0 = normal room light, 1 = dark room mode

// Convert from linear to sRGB color space for video samples
float ToSrgb1(float c) {
    c = clamp(c, 0.0, 1.0);
    return (c < 0.0031308 ? c * 12.92 : 1.055 * pow(c, 0.41666) - 0.055);
}
vec3 ToSrgb(vec3 c) {
    return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
}

// Video texture is an OpenGL FBO where (0,0) is top-left in screen_tc space.
// Hardware mipmapped LOD 4.2 dissolves all high-frequency scanlines, text, and UI icons into silky diffuse ambient light.
vec3 sample_video_linear(vec2 tc) {
    vec4 crop = border_crop;
    vec2 min_crop_uv = vec2(crop.x + 0.015, crop.z + 0.015);
    vec2 max_crop_uv = vec2(1.0 - crop.y - 0.015, 1.0 - crop.w - 0.015);
    vec2 clamped = clamp(tc, min_crop_uv, max_crop_uv);
    return textureLod(video_texture, clamped, 4.2).rgb;
}

// High-fidelity near-field edge sample.
// LOD 1.2 smooths individual CRT raster lines while preserving the full peak luminance and hue of adjacent phosphors.
vec3 sample_video_edge(vec2 tc) {
    vec4 crop = border_crop;
    vec2 min_crop_uv = vec2(crop.x + 0.005, crop.z + 0.005);
    vec2 max_crop_uv = vec2(1.0 - crop.y - 0.005, 1.0 - crop.w - 0.005);
    vec2 clamped = clamp(tc, min_crop_uv, max_crop_uv);
    return textureLod(video_texture, clamped, 1.2).rgb;
}

// Atlas sub-texture UV rectangles (texture flipped vertically so V=0 is bottom, V=1 is top):
// 1. Left Pillar:     [0.0000, 0.0000] to [0.2500, 1.0000] (512x2048)
// 2. Right Pillar:    [0.2500, 0.0000] to [0.5000, 1.0000] (512x2048)
// 3. Seamless Tile:   [0.5000, 0.5000] to [0.7500, 0.7500] (512x512)
// 4. Knobs Section:   [0.5000, 0.3750] to [0.7500, 0.5000] (512x256, 2:1)
// 5. Top Bezel/Vents: [0.5000, 0.7500] to [1.0000, 1.0000] (1024x512)
// 6. Power Section:   [0.7500, 0.5000] to [1.0000, 0.6250] (512x256, 2:1)
// 7. NEC Badge:       [0.7500, 0.6250] to [1.0000, 0.7500] (512x256, 2:1)

// High-precision pseudo-random procedural hash (stable across all GPU architectures)
float hash21(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

// 2D Value Noise for non-repeating organic variation
float value_noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash21(i + vec2(0.0, 0.0)), hash21(i + vec2(1.0, 0.0)), u.x),
               mix(hash21(i + vec2(0.0, 1.0)), hash21(i + vec2(1.0, 1.0)), u.x), u.y);
}

void main() {
    // 1. Calculate active video coordinate mapping
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

    // 2. Pillowing / Curvature warp
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

    // 3. Exact 2D Euclidean Signed Distance Field (SDF) for Rounded CRT Aperture
    // In corrected_tc / screen_tc space: (0, 0) is top-left, (1, 1) is bottom-right.
    // border_crop: (left, right, top, bottom) -> (crop.x, crop.y, crop.z, crop.w)
    vec4 crop = border_crop;
    vec2 ap_center = vec2(0.5 * (crop.x + (1.0 - crop.y)), 0.5 * (crop.z + (1.0 - crop.w)));
    vec2 ap_half = vec2(0.5 * (1.0 - crop.x - crop.y), 0.5 * (1.0 - crop.z - crop.w));

    vec2 video_pixels = outputResolution * scale;
    float min_vid_dim = max(min(video_pixels.x, video_pixels.y), 1.0);

    vec2 p_vid = (screen_tc - ap_center) * video_pixels;
    vec2 b_vid = ap_half * video_pixels;

    // Molded corner radius in physical screen pixels (minimum 22px for authentic rounded CRT aperture)
    float r_corner = (filter_type == 2 && corner_size > 0.001)
        ? max(corner_size * min_vid_dim, 22.0)
        : clamp(min_vid_dim * 0.038, 20.0, 32.0);

    // Exact Euclidean rounded box SDF:
    vec2 q = abs(p_vid) - (b_vid - vec2(r_corner));
    vec2 max_q = max(q, vec2(0.0));
    float len_max_q = length(max_q);
    float d_ap = min(max(q.x, q.y), 0.0) + len_max_q - r_corner;

    // Continuous 2D unit normal pointing outward from the aperture:
    // Rotates continuously around circular corners without seams.
    vec2 n_dir = (len_max_q > 0.0001) ? (max_q / len_max_q) : ((q.x > q.y) ? vec2(1.0, 0.0) : vec2(0.0, 1.0));
    vec2 n_ap = n_dir * sign(p_vid);

    // 4. Optical Drop Shadow onto CRT Glass (Deeply Embedded CRT Tube)
    float shadow_w = clamp(min_vid_dim * 0.024, 22.0, 46.0);
    float shadow_falloff = clamp((d_ap + shadow_w) / shadow_w, 0.0, 1.0);
    // Overhead room light casts stronger shadow from top (-ny) and left (-nx) overhang
    float top_bias = clamp(-n_ap.y * 0.5 + 0.5, 0.0, 1.0);
    float left_bias = clamp(-n_ap.x * 0.5 + 0.5, 0.0, 1.0);
    float dir_shadow_factor = 0.65 + 0.35 * (top_bias * 0.65 + left_bias * 0.35);
    // In dark mode, no overhead ceiling fixtures exist; CRT is the sole light emitter in the room.
    float shadow_alpha = (dark_mode == 1)
        ? 0.0
        : pow(shadow_falloff, 1.6) * 0.65 * dir_shadow_factor;

    // Early exit for deep interior video area
    if (d_ap < -shadow_w) {
        out_color = vec4(0.0);
        return;
    }

    // 5. Dark Rubber Gasket Sealing CRT Tube
    float gasket_w = clamp(min_vid_dim * 0.0045, 3.0, 5.5);
    vec3 gasket_col = vec3(0.038, 0.038, 0.042);

    // 6. Deep Recessed 3D Inner Bevel (Deep Cowl with Asymmetric Bottom Shelf)
    float is_bottom_bevel = clamp(n_ap.y, 0.0, 1.0);
    float bevel_w_base = clamp(min_vid_dim * 0.038, 30.0, 56.0);
    float bevel_w = bevel_w_base * (1.0 + is_bottom_bevel * 0.25);
    float t_bevel = clamp((d_ap - gasket_w) / max(bevel_w - gasket_w, 1.0), 0.0, 1.0);

    // Steep inward-sloping surface normal in world space (+X right, +Y up, +Z toward viewer)
    vec3 L = normalize(vec3(-0.28, 0.65, 0.70));
    float slope_angle = 0.62;
    vec2 n_bevel_world = vec2(-n_ap.x, n_ap.y) * slope_angle;
    vec3 N_bevel = vec3(n_bevel_world, sqrt(max(1.0 - dot(n_bevel_world, n_bevel_world), 0.01)));

    float diffuse = clamp(dot(N_bevel, L), 0.0, 1.0);
    // Cavity depth shading: deeper inside the cowl is darker
    float cavity_ao = 0.45 + 0.55 * pow(t_bevel, 0.75);
    float bevel_lighting = (dark_mode == 1)
        ? (0.12 + diffuse * 0.12)
        : (0.35 + diffuse * 0.85) * cavity_ao;

    // Bottom shelf catch-light
    float bottom_shelf_factor = (dark_mode == 1) ? 0.18 : 0.35;
    float bottom_shelf = clamp(n_ap.y, 0.0, 1.0) * pow(t_bevel, 2.0) * bottom_shelf_factor;

    // Specular crest line on bottom and right ridge
    float crest_ridge = exp(-0.5 * pow((t_bevel - 0.97) / 0.028, 2.0));
    float crest_dir = clamp(n_ap.x * 0.40 - n_ap.y * 0.75, 0.0, 1.0);
    float crest_mult = (dark_mode == 1) ? 0.45 : 1.0;
    vec3 crest_col = vec3(0.14, 0.14, 0.11) * (crest_ridge * crest_dir * crest_mult);

    // Nearest perimeter contact coordinate on the active aperture in screen_tc space
    vec2 edge_tc = screen_tc - (n_ap * max(d_ap, 0.0)) / video_pixels;
    vec2 min_crop_uv = vec2(crop.x + 0.005, crop.z + 0.005);
    vec2 max_crop_uv = vec2(1.0 - crop.y - 0.005, 1.0 - crop.w - 0.005);
    edge_tc = clamp(edge_tc, min_crop_uv, max_crop_uv);

    vec3 panel_edge_col = ToSrgb(sample_video_edge(edge_tc));
    float panel_edge_lum = dot(panel_edge_col, vec3(0.2126, 0.7152, 0.0722));

    // Near-field contact halation:
    // Radiative transfer and optical PSF across a half-plane emitter converges to roughly half (0.50)
    // the brightness of the active phosphor panel directly at the contact interface (d_ap = 0).
    float halation_lambda = clamp(min_vid_dim * 0.010, 8.0, 14.0);
    float halation_falloff = exp(-max(d_ap, 0.0) / halation_lambda);
    vec3 halation_col = mix(vec3(panel_edge_lum), panel_edge_col, 0.60);
    float glow_response = (dark_mode == 1)
        ? clamp(ambient_glow / 0.825, 0.0, 1.25)
        : clamp(ambient_glow * 0.90, 0.0, 1.0);
    float halation_edge_ratio = (dark_mode == 1) ? 0.45 : 0.25;
    vec3 contact_halation = halation_col * (halation_edge_ratio * halation_falloff * glow_response);

    // Physically-grounded diffuse light reflection from active video feed bouncing on inner cowl
    vec3 ambient_glow_col = vec3(0.0);
    if (ambient_glow > 0.001 && d_ap > 0.0 && d_ap < bevel_w) {
        // Tangent unit vector along aperture perimeter
        vec2 t_ap = vec2(-n_ap.y, n_ap.x);

        // Direction pointing toward video center for corner ambient bounce
        vec2 diag_dir = -sign(p_vid);

        // Conical dispersion scaling: light spreads wider near the edge
        float cone_spread = 0.06 + 0.20 * t_bevel;
        float depth_base = 0.03 + 0.06 * t_bevel;

        // Continuous multi-scale Gaussian tangential fan merging neighboring rays into soft diffuse bounce
        vec3 vid_acc = vec3(0.0);
        float tot_w = 0.0;

        const float t_offs[6] = float[6](0.012, 0.030, 0.055, 0.090, 0.140, 0.200);
        const float t_wts[6]  = float[6](0.240, 0.200, 0.160, 0.120, 0.080, 0.040);

        // Core near-edge center taps across depth
        vec3 c_depth1 = sample_video_linear(edge_tc - n_ap * (depth_base * 0.85));
        vec3 c_depth2 = sample_video_linear(edge_tc - n_ap * (depth_base * 1.60));
        vid_acc += c_depth1 * 0.35; tot_w += 0.35;
        vid_acc += c_depth2 * 0.20; tot_w += 0.20;

        // Symmetric tangential Gaussian fan merging neighboring rays
        for (int k = 0; k < 6; k++) {
            float off = t_offs[k] * cone_spread * 2.0;
            float wt  = t_wts[k];

            vid_acc += sample_video_linear(edge_tc - n_ap * depth_base - t_ap * off) * wt;
            vid_acc += sample_video_linear(edge_tc - n_ap * depth_base + t_ap * off) * wt;
            tot_w += 2.0 * wt;
        }

        // Deep cavity & corner cross-bounce
        vid_acc += sample_video_linear(edge_tc - n_ap * (depth_base * 1.50) + diag_dir * 0.060) * 0.08;
        tot_w += 0.08;

        vec3 vid_glow_col = ToSrgb(vid_acc / tot_w);
        float vid_lum = dot(vid_glow_col, vec3(0.2126, 0.7152, 0.0722));

        // Responsive dynamic energy scaling with screen luminance
        float vid_energy = clamp(0.60 + 1.20 * vid_lum, 0.60, 1.80);

        // Natural phosphor color blend: white light luminance punch with subtle phosphor color tint
        vec3 bounce_col = mix(vec3(vid_lum), vid_glow_col, 0.40);
        vec3 incident_light = bounce_col * vid_energy;

        // Tightened cowl slope falloff: gracefully decays towards crest, terminating cleanly at bevel_w
        float cowl_falloff = pow(1.0 - t_bevel, 2.2);

        // Responsive, luminous diffuse reflection scaling across slider
        float bounce_mult = (dark_mode == 1) ? 0.65 : 0.95;
        ambient_glow_col = incident_light * (bounce_mult * ambient_glow * cowl_falloff);
    }

    // 7. Authentic Chassis & Bevel Aging, Grime Accumulation & Material Patina
    // Non-repeating multi-octave organic noise evaluated in physical screen coordinates
    float n_coarse = value_noise(gl_FragCoord.xy / 220.0);
    float n_med    = value_noise(gl_FragCoord.xy / 65.0);
    float n_fine   = value_noise(gl_FragCoord.xy / 18.0);
    float n_micro  = value_noise(gl_FragCoord.xy / 5.5);

    float organic_stain = n_coarse * 0.50 + n_med * 0.35 + n_fine * 0.15;

    vec3 base_putty = vec3(0.67, 0.64, 0.57);
    vec3 aged_yellow = vec3(0.76, 0.70, 0.48);
    vec3 tobacco_tar = vec3(0.52, 0.40, 0.22);
    vec3 crevice_soot = vec3(0.14, 0.13, 0.11);

    // Directional UV exposure (top-left ceiling light bias)
    vec2 chassis_uv = gl_FragCoord.xy / outputResolution;
    float uv_exposure = clamp(pow(chassis_uv.y, 1.3) * 0.75 + (1.0 - chassis_uv.x) * 0.25, 0.0, 1.0);

    // ABS plastic aging and patchy discoloration
    float yellowing = clamp(uv_exposure * 0.45 + organic_stain * 0.35 + 0.10, 0.0, 1.0);
    vec3 aged_plastic = mix(base_putty, aged_yellow, yellowing);

    // Tobacco smoke / nicotine tar condensation (uneven pooling on upward shelves and corners)
    float smoke_condense = clamp((organic_stain - 0.40) * 2.0, 0.0, 1.0);
    float smoke_shelf_bias = 0.50 + 0.50 * clamp(n_ap.y, 0.0, 1.0);
    float smoke_amount = smoke_condense * smoke_shelf_bias * 0.28;
    aged_plastic = mix(aged_plastic, tobacco_tar, smoke_amount);

    // Crevice grime accumulation (dust/soot in seams and corners)
    float seam_dist = abs(d_ap - gasket_w);
    float seam_grime = exp(-0.5 * pow(seam_dist / 3.2, 2.0)) * 0.55;
    float cowl_grime = clamp((1.0 - t_bevel) * 0.35 + clamp(n_ap.y, 0.0, 1.0) * pow(1.0 - t_bevel, 2.0) * 0.25, 0.0, 0.60);
    float corner_grime = clamp(len_max_q / r_corner, 0.0, 1.0) * (1.0 - t_bevel) * 0.35;
    float total_grime = (dark_mode == 1)
        ? clamp(seam_grime * 0.20 + (1.0 - t_bevel) * 0.10, 0.0, 0.30)
        : clamp(seam_grime + cowl_grime * (0.60 + 0.40 * n_fine) + corner_grime, 0.0, 0.85);

    // Handling oil & sebum smudges on lower chin and outer corners
    float corner_dist = length(vec2(min(gl_FragCoord.x, outputResolution.x - gl_FragCoord.x), gl_FragCoord.y));
    float handling_patina = exp(-0.5 * pow(corner_dist / 140.0, 2.0)) * (0.12 + 0.08 * n_med);
    float chin_patina = exp(-0.5 * pow((gl_FragCoord.y - 28.0) / 20.0, 2.0)) * exp(-0.5 * pow((gl_FragCoord.x - outputResolution.x * 0.72) / 120.0, 2.0)) * 0.18;
    float oil_level = clamp(handling_patina + chin_patina, 0.0, 0.35);
    aged_plastic *= (1.0 - oil_level * 0.40);

    // Dual-scale stochastic ABS pebble grain and micro-surface roughness
    vec2 uv_grain1 = vec2(0.50, 0.50) + fract(gl_FragCoord.xy * (0.50 / 512.0)) * 0.25;
    vec2 uv_grain2 = vec2(0.50, 0.50) + fract(vec2(
        gl_FragCoord.x * 0.31 + gl_FragCoord.y * 0.21,
        -gl_FragCoord.x * 0.21 + gl_FragCoord.y * 0.31
    ) / 512.0) * 0.25;
    vec3 grain = (texture(bezel_texture, uv_grain1).rgb + texture(bezel_texture, uv_grain2).rgb) * 0.5;
    vec3 grain_mod = (grain - vec3(0.60, 0.58, 0.50)) * 0.35;
    float grit_mod = (n_micro - 0.5) * 0.06;
    float grain_scale = (dark_mode == 1) ? 0.85 : 1.0;
    aged_plastic = clamp(aged_plastic + grain_mod * grain_scale + vec3(grit_mod * grain_scale), 0.0, 1.0);

    // 8. Natural Asymmetric Chassis Structural Moldings
    float pillar_dist_l = abs(gl_FragCoord.x - (outputResolution.x * 0.5 - b_vid.x - bevel_w - 20.0));
    float pillar_dist_r = abs(gl_FragCoord.x - (outputResolution.x * 0.5 + b_vid.x + bevel_w + 20.0));
    float pillar_step = exp(-0.5 * pow(min(pillar_dist_l, pillar_dist_r) / 1.6, 2.0)) * 0.20;
    float pillar_molding = (d_ap > bevel_w && abs(gl_FragCoord.x - outputResolution.x * 0.5) > b_vid.x) ? pillar_step : 0.0;

    float chin_step = exp(-0.5 * pow((gl_FragCoord.y - 20.0) / 1.8, 2.0)) * 0.16;
    float chin_molding = (d_ap > bevel_w && gl_FragCoord.y < outputResolution.y * 0.25) ? chin_step : 0.0;

    float edge_dist = min(min(gl_FragCoord.x, outputResolution.x - gl_FragCoord.x), min(gl_FragCoord.y, outputResolution.y - gl_FragCoord.y));
    float outer_chamfer = clamp(edge_dist / 22.0, 0.0, 1.0);
    float outer_vignette = 0.75 + 0.25 * sqrt(outer_chamfer);

    vec3 chassis_color = aged_plastic * outer_vignette * (1.0 - pillar_molding) * (1.0 - chin_molding);
    if (dark_mode == 1) {
        chassis_color *= 0.022;
    }

    // 9. Inner Bevel Cowl with Grime Accumulation
    vec3 plastic_bevel = (dark_mode == 1) ? aged_plastic * 0.22 : aged_plastic;
    vec3 soot_col = (dark_mode == 1) ? crevice_soot * 0.20 : crevice_soot;
    float grime_factor = (dark_mode == 1) ? total_grime * 0.15 : total_grime;
    vec3 surface_plastic = mix(
        plastic_bevel * (bevel_lighting + bottom_shelf) * (0.75 + 0.25 * t_bevel) + crest_col,
        soot_col,
        grime_factor
    );

    // Physically-based velvety diffuse reflection:
    // Screen light illuminates the plastic surface with realistic material reflectance
    vec3 surface_refl = (dark_mode == 1) ? (aged_plastic * 0.75 + vec3(0.25)) : (aged_plastic * 0.85 + vec3(0.15));
    vec3 diffuse_illum = ambient_glow_col * surface_refl;
    vec3 bevel_col = surface_plastic + diffuse_illum + contact_halation;

    // 10. Blend Bevel and Chassis Plastic
    float t_bevel_blend = smoothstep(bevel_w - 1.2, bevel_w + 1.2, d_ap);
    vec3 frame_plastic = mix(bevel_col, chassis_color, t_bevel_blend);

    // 11. Authentic Continuous Velvety Dust Bed Deposited on the 3D Bezel Inlet Shelf
    // Gravity pulls airborne dust strictly onto the upward-facing bottom cowl inlet shelf:
    // - Along the bottom arc: n_ap.y > 0.15 (peaks at bottom center n_ap.y = 1.0, smoothly fades to 0 at side pillars)
    // - Inside the 3D inlet: d_ap between gasket_w and bevel_w
    // - ZERO dust on the vertical outer chassis faceplate or vertical side pillars
    if (n_ap.y > 0.15 && d_ap > gasket_w - 0.5 && d_ap < bevel_w) {
        float in_bevel_inlet = clamp((d_ap - gasket_w) / 2.0, 0.0, 1.0) * clamp((bevel_w - d_ap) / 2.0, 0.0, 1.0);
        float shelf_gravity = pow(clamp(n_ap.y, 0.0, 1.0), 1.9);
        float inlet_shelf_profile = clamp(t_bevel / 0.09, 0.0, 1.0) * pow(clamp((1.0 - t_bevel) / 0.16, 0.0, 1.0), 0.55);

        // Continuous multi-frequency organic fractal noise (completely continuous, ZERO cellular grid or tile lines)
        float n_drift = value_noise(gl_FragCoord.xy / 40.0);
        float n_silt  = value_noise((gl_FragCoord.xy + vec2(85.3, 32.7)) / 12.0);

        // Rotated non-axis-aligned coordinates (30 degrees) eliminate any possibility of Cartesian grid artifacts
        vec2 rot_coord = vec2(gl_FragCoord.x * 0.866 - gl_FragCoord.y * 0.500, gl_FragCoord.x * 0.500 + gl_FragCoord.y * 0.866);
        float n_grain = value_noise(rot_coord / 3.2);
        float n_micro = value_noise(vec2(-rot_coord.y, rot_coord.x) * 1.3 / 1.8);

        float dust_bed_density = in_bevel_inlet * shelf_gravity * inlet_shelf_profile * (0.65 + 0.25 * n_drift + 0.10 * n_silt) * 1.60;

        if (dust_bed_density > 0.002) {
            // Continuous micro-facet normal perturbation from smooth analytical noise gradient
            float eps = 0.8;
            float gn_x = value_noise((rot_coord + vec2(eps, 0.0)) / 3.2) - value_noise((rot_coord - vec2(eps, 0.0)) / 3.2);
            float gn_y = value_noise((rot_coord + vec2(0.0, eps)) / 3.2) - value_noise((rot_coord - vec2(0.0, eps)) / 3.2);

            vec3 N_dust = normalize(N_bevel + vec3(gn_x * 0.40, gn_y * 0.40, 0.0));

            // Velvety Lambertian diffuse scattering from room ceiling light (completely matte, zero specular gloss)
            float diff_dust = clamp(dot(N_dust, L), 0.0, 1.0);
            float dust_ao = 0.45 + 0.55 * pow(t_bevel, 0.55);

            // Diffuse light scattering from the active CRT screen phosphors
            vec3 L_crt = normalize(vec3(-n_ap.x, -n_ap.y, 0.50));
            float diff_crt = (dark_mode == 1) ? clamp(dot(N_bevel, L_crt), 0.0, 1.0) : clamp(dot(N_dust, L_crt), 0.0, 1.0);
            float crt_dust_mult = (dark_mode == 1) ? 0.95 : 0.45;
            vec3 crt_scatter_col = vec3(0.0);
            if (ambient_glow > 0.001) {
                vec3 raw_col = ToSrgb(sample_video_linear(screen_tc));
                float raw_lum = dot(raw_col, vec3(0.2126, 0.7152, 0.0722));
                vec3 desat_raw = mix(vec3(raw_lum), raw_col, 0.28);
                crt_scatter_col = desat_raw * pow(raw_lum, 1.45) * (ambient_glow * (crt_dust_mult * 0.35));
            }
            vec3 crt_light_scatter = crt_scatter_col * diff_crt;

            // Natural pale warm-gray chalky dust tone matching real CRT reference photograph
            vec3 dust_albedo = (dark_mode == 1)
                ? vec3(0.68, 0.66, 0.61)
                : vec3(0.70, 0.68, 0.63) + vec3((n_silt - 0.5) * 0.05 + (n_micro - 0.5) * 0.04);
            vec3 dust_room_light = (dark_mode == 1)
                ? dust_albedo * (0.06 + 0.08 * diff_dust) * dust_ao
                : dust_albedo * (0.40 + 0.60 * diff_dust) * dust_ao;
            vec3 dust_lit = dust_room_light + crt_light_scatter;

            // Smooth velvety blending onto shelf plastic
            float dust_alpha = (dark_mode == 1)
                ? clamp(dust_bed_density * 0.75, 0.0, 0.85)
                : clamp(dust_bed_density * (0.80 + 0.20 * n_grain), 0.0, 0.92);
            frame_plastic = mix(frame_plastic, dust_lit, dust_alpha);
        }
    }

    // Transition from dark rubber gasket to plastic bevel
    float t_gasket = smoothstep(gasket_w - 0.75, gasket_w + 0.75, d_ap);
    vec3 gasket_toned = (dark_mode == 1) ? gasket_col * 0.60 : gasket_col;
    vec3 gasket_lit = gasket_toned + contact_halation;
    vec3 frame_solid = mix(gasket_lit, frame_plastic, t_gasket);

    // Subpixel coverage anti-aliasing across aperture boundary (eliminates jaggies)
    float coverage = smoothstep(-0.75, 0.75, d_ap);

    vec4 inner_screen = vec4(0.0, 0.0, 0.0, shadow_alpha);
    vec4 outer_frame = vec4(frame_solid, 1.0);

    out_color = mix(inner_screen, outer_frame, coverage);
}
