#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D video_texture;
uniform sampler2D silhouette_texture;

uniform vec2 outputResolution;
uniform vec2 source_size;
uniform float horizontal_stretch;
uniform vec4 border_crop; // left, right, top, bottom
uniform vec2 warp;        // warpX, warpY
uniform float corner_size;
uniform int filter_type;  // 0 = Flat/Passthrough, 1 = Lottes, 2 = Halo
uniform float intensity;   // 0.0 to 1.0
uniform float glossiness;  // 0.0 (Matte / Diffuse) to 1.0 (Glossy / Crisp Ceiling Lights)
uniform float time;

uniform int ceiling_light_enabled;
uniform int photographer_enabled;
uniform float photographer_intensity;
uniform int flash_enabled;
uniform float flash_intensity;

float ToLinear1(float c) {
    return (c <= 0.04045) ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}

vec3 ToLinear(vec3 c) {
    return vec3(ToLinear1(c.r), ToLinear1(c.g), ToLinear1(c.b));
}

float ToSrgb1(float c) {
    return (c < 0.0031308 ? c * 12.92 : 1.055 * pow(c, 0.41666) - 0.055);
}

vec3 ToSrgb(vec3 c) {
    return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
}

// Evaluates a 90s Japanese office modular ceiling troffer fixture with twin parallel fluorescent tubes
float eval_troffer(float center_x, float center_y, float half_len, float tube_sep, vec2 rot_r, float gloss) {
    float dx = abs(rot_r.x - center_x);
    float end_soft = 0.05 * (1.0 - 0.5 * gloss) + 0.012;
    float h_mask = clamp(1.0 - (dx - (half_len - end_soft)) / end_soft, 0.0, 1.0);

    float y_top = center_y + tube_sep * 0.5;
    float y_bot = center_y - tube_sep * 0.5;

    float sigma = 0.018 * (1.0 - 0.75 * gloss) + 0.0055;
    float tube_top = exp(-0.5 * pow((rot_r.y - y_top) / sigma, 2.0));
    float tube_bot = exp(-0.5 * pow((rot_r.y - y_bot) / sigma, 2.0));

    float housing_sigma = tube_sep * 1.15;
    float housing_glow = exp(-0.5 * pow((rot_r.y - center_y) / housing_sigma, 2.0)) * (0.14 * (1.0 - 0.4 * gloss));

    return (tube_top + tube_bot + housing_glow) * h_mask;
}

void main() {
    // 0. Passthrough optimization when intensity is near zero
    if (intensity <= 0.0001) {
        out_color = texture(video_texture, v_tc);
        return;
    }

    // 1. Calculate aspect fit to isolate active video area
    float video_aspect = (source_size.x * horizontal_stretch) / max(source_size.y, 1.0);
    float output_aspect = outputResolution.x / max(outputResolution.y, 1.0);

    vec2 scale = vec2(1.0, 1.0);
    if (video_aspect > output_aspect) {
        scale.y = output_aspect / video_aspect;
    } else {
        scale.x = video_aspect / output_aspect;
    }

    vec2 centered_tc = (v_tc - 0.5) / scale + 0.5;

    // 2. Curvature warp to match CRT screen profile
    vec2 screen_tc = centered_tc;
    if (filter_type == 1) { // Lottes warp
        vec2 pos = centered_tc * 2.0 - 1.0;
        pos *= vec2(1.0 + (pos.y * pos.y) * warp.x, 1.0 + (pos.x * pos.x) * warp.y);
        screen_tc = pos * 0.5 + 0.5;
    } else if (filter_type == 2) { // Halo curvature
        if (warp.x > 0.001 || warp.y > 0.001) {
            vec2 pos = centered_tc * 2.0 - 1.0;
            pos *= vec2(1.0 + (pos.y * pos.y) * 0.031, 1.0 + (pos.x * pos.x) * 0.041);
            screen_tc = pos * 0.5 + 0.5;
        }
    }

    // 3. Signed distance to active video boundaries
    vec4 crop = border_crop;
    float d_left = crop.x - screen_tc.x;
    float d_right = screen_tc.x - (1.0 - crop.y);
    float d_top = screen_tc.y - (1.0 - crop.z);
    float d_bottom = crop.w - screen_tc.y;

    vec2 d = max(vec2(d_left, d_top), vec2(d_right, d_bottom));
    float max_d = max(d.x, d.y);

    // Halo rounded corners matching
    if (filter_type == 2 && corner_size > 0.001) {
        vec2 cd1 = abs(2.0 * centered_tc - 1.0) - (vec2(1.0) - vec2(corner_size * 2.0));
        if (cd1.x > 0.0 && cd1.y > 0.0) {
            float cdist1 = length(cd1) - corner_size * 2.0;
            max_d = max(max_d, cdist1 * 0.5);
        }
        vec2 cd2 = abs(2.0 * screen_tc - 1.0) - (vec2(1.0) - vec2(corner_size * 2.0));
        if (cd2.x > 0.0 && cd2.y > 0.0) {
            float cdist2 = length(cd2) - corner_size * 2.0;
            max_d = max(max_d, cdist2 * 0.5);
        }
    }

    // OUTSIDE ACTIVE CRT VIDEO:
    // Strictly preserve unmodified background/black bars/retro frame
    if (max_d > 0.0) {
        out_color = texture(video_texture, v_tc);
        return;
    }

    // Convert normalized distance to screen pixel metrics
    vec2 video_pixels = outputResolution * scale;
    float min_vid_dim = max(min(video_pixels.x, video_pixels.y), 1.0);
    float inner_edge_mask = smoothstep(0.0, -2.5 / min_vid_dim, max_d);

    // 4. 3D Surface Geometry of Curved CRT Glass Faceplate
    vec2 p = (screen_tc - 0.5) * 2.0; // [-1.0, 1.0]
    float cur_x = 0.18 + (warp.x > 0.001 ? warp.x * 2.5 : 0.06);
    float cur_y = 0.22 + (warp.y > 0.001 ? warp.y * 2.5 : 0.08);
    vec3 N = normalize(vec3(p.x * cur_x, p.y * cur_y, 1.0));

    // Reflected ray vector in room space
    vec3 R = 2.0 * N.z * N - vec3(0.0, 0.0, 1.0);

    // 5. Optical Refraction & Chromatic Dispersion
    float dist_sq = dot(p, p);
    float thickness = 1.0 + 0.60 * dist_sq;
    vec2 delta_uv = vec2(N.x, N.y) * (thickness * 0.0060 * intensity);

    vec2 uv_r = clamp(v_tc - delta_uv * (1.0 - 0.35 * intensity), 0.0, 1.0);
    vec2 uv_g = clamp(v_tc - delta_uv, 0.0, 1.0);
    vec2 uv_b = clamp(v_tc - delta_uv * (1.0 + 0.45 * intensity), 0.0, 1.0);

    vec3 col_refracted = vec3(
        texture(video_texture, uv_r).r,
        texture(video_texture, uv_g).g,
        texture(video_texture, uv_b).b
    );
    vec3 col_lin = ToLinear(col_refracted);

    // 6. Dynamic Internal Video Light Reflections & Backscatter
    vec2 r_step1 = vec2(16.0 / max(outputResolution.x, 1.0), 16.0 / max(outputResolution.y, 1.0));
    vec2 r_step2 = vec2(32.0 / max(outputResolution.x, 1.0), 32.0 / max(outputResolution.y, 1.0));

    vec3 s1 = (ToLinear(texture(video_texture, clamp(v_tc + vec2(r_step1.x, 0.0), 0.0, 1.0)).rgb) +
               ToLinear(texture(video_texture, clamp(v_tc - vec2(r_step1.x, 0.0), 0.0, 1.0)).rgb) +
               ToLinear(texture(video_texture, clamp(v_tc + vec2(0.0, r_step1.y), 0.0, 1.0)).rgb) +
               ToLinear(texture(video_texture, clamp(v_tc - vec2(0.0, r_step1.y), 0.0, 1.0)).rgb)) * 0.25;

    vec3 s2 = (ToLinear(texture(video_texture, clamp(v_tc + vec2(r_step2.x, r_step2.y), 0.0, 1.0)).rgb) +
               ToLinear(texture(video_texture, clamp(v_tc + vec2(-r_step2.x, r_step2.y), 0.0, 1.0)).rgb) +
               ToLinear(texture(video_texture, clamp(v_tc + vec2(r_step2.x, -r_step2.y), 0.0, 1.0)).rgb) +
               ToLinear(texture(video_texture, clamp(v_tc + vec2(-r_step2.x, -r_step2.y), 0.0, 1.0)).rgb)) * 0.25;

    vec3 scatter_lin = s1 * 0.65 + s2 * 0.35;
    float scatter_lum = dot(scatter_lin, vec3(0.2126, 0.7152, 0.0722));

    // Colored glow on glass faceplate reacting dynamically to screen content
    vec3 dynamic_glow = scatter_lin * (scatter_lum * 0.45 + 0.06) * (intensity * 0.35);

    // Screen light ghost reflection (displaced along normal)
    vec2 ghost_uv = clamp(v_tc + vec2(N.x, N.y) * (0.012 * intensity), 0.0, 1.0);
    vec3 ghost_lin = ToLinear(texture(video_texture, ghost_uv).rgb);
    float ghost_lum = dot(ghost_lin, vec3(0.2126, 0.7152, 0.0722));
    vec3 ghost_reflection = ghost_lin * (ghost_lum * 0.28) * (intensity * 0.40);

    // Perimeter edge internal catch-light
    float edge_p = max(abs(p.x), abs(p.y));
    float edge_catch_factor = smoothstep(0.72, 0.98, edge_p);
    vec3 perimeter_catch = scatter_lin * edge_catch_factor * (scatter_lum * 0.50 + 0.12) * (intensity * 0.30);

    // 7. Ambient Room Specular Reflections & 90s Japanese Office Ceiling Fluorescent Lights
    vec3 fluorescent_reflection = vec3(0.0);
    if (ceiling_light_enabled == 1) {
        float tilt = 0.040;
        vec2 rot_r = vec2(R.x - tilt * R.y, R.y + tilt * R.x);

        // Primary twin-tube modular troffer in upper area (lowered by ~6% screenspace)
        float fix_1 = eval_troffer(-0.24, 0.44, 0.34, 0.046, rot_r, glossiness);

        // Secondary modular troffer further up on the ceiling grid (lowered by ~6% screenspace)
        float fix_2 = eval_troffer(0.18, 0.60, 0.25, 0.038, rot_r, glossiness) * 0.35;

        float tubes_total = fix_1 + fix_2;
        float tubes_glow = tubes_total * (0.16 + 0.52 * glossiness);

        // Soft localized diffuse ambient envelope around fixtures
        float diffuse_env = exp(-0.5 * (pow((rot_r.y - 0.44) / 0.20, 2.0) + pow((rot_r.x + 0.24) / 0.50, 2.0))) * (0.10 * (1.0 - 0.60 * glossiness));

        // Cold daylight-white fluorescent tube tint (昼光色 ~6500K - cold, crisp, subtle cyan-blue cast, NOT electric/neon blue)
        vec3 fl_color = vec3(0.88, 0.94, 1.02);
        fluorescent_reflection = fl_color * (tubes_glow + diffuse_env);
    }

    // Secondary ambient desk bounce on lower quadrant
    vec3 L2 = normalize(vec3(0.35, -0.28, 0.88));
    vec3 H2 = normalize(L2 + vec3(0.0, 0.0, 1.0));
    float desk_sheen = pow(max(dot(N, H2), 0.0), 8.0 + glossiness * 36.0) * (0.08 * (1.0 - 0.40 * glossiness));

    // Grazing Fresnel rim sheen
    float cos_theta = clamp(dot(N, vec3(0.0, 0.0, 1.0)), 0.0, 1.0);
    float fresnel_rim = pow(1.0 - cos_theta, 3.0) * (0.20 + 0.18 * glossiness);

    vec3 glass_tint = vec3(0.92, 0.96, 1.0);
    vec3 ambient_specular = (glass_tint * (desk_sheen + fresnel_rim) + fluorescent_reflection) * (intensity * 0.45);

    // 7b. Photographer Silhouette Reflection
    if (photographer_enabled == 1 && photographer_intensity > 0.001) {
        vec2 sil_uv = clamp(screen_tc + vec2(N.x, N.y) * (0.010 * (1.0 - glossiness * 0.40)), 0.0, 1.0);
        float sil_mask = texture(silhouette_texture, sil_uv).r * photographer_intensity;
        sil_mask = pow(sil_mask, mix(1.25, 0.85, glossiness));

        // Silhouette blocks/occludes ambient ceiling and desk reflections behind the viewer
        ambient_specular *= (1.0 - 0.75 * sil_mask);
        // Softly deepens video content under the reflection shadow
        col_lin *= (1.0 - 0.35 * sil_mask);

        // Faint diffuse glass sheen reflection of the darkened silhouette figure
        vec3 sil_sheen_color = vec3(0.035, 0.040, 0.048);
        ambient_specular += sil_sheen_color * (sil_mask * (0.35 + 0.65 * glossiness) * intensity);
    }

    // 7c. Camera Flash Overexposure Reflection
    if (flash_enabled == 1 && flash_intensity > 0.001) {
        vec2 flash_pos = vec2(0.28, -0.22);
        vec2 delta_f = p - flash_pos;
        delta_f.x *= (outputResolution.x / max(outputResolution.y, 1.0));
        float f_dist = length(delta_f);

        // Warm, diffuse optical tone (amber/golden-white glow, less bright on specular spectrum)
        vec3 warm_flash_color = vec3(1.0, 0.88, 0.70);
        float core_rad = mix(0.18, 0.13, glossiness);
        float f_core = exp(-0.5 * pow(f_dist / core_rad, 2.0)) * 0.40;
        float f_bloom = exp(-0.5 * pow(f_dist / 0.42, 2.0)) * 0.26;
        float f_veiling = 0.16 / (1.0 + pow(f_dist / 0.55, 1.4));
        float f_streak = exp(-0.5 * pow(delta_f.x / 0.95, 2.0)) * exp(-0.5 * pow(delta_f.y / 0.09, 2.0)) * 0.10;

        vec3 flash_lin = warm_flash_color * ((f_core + f_bloom + f_veiling + f_streak) * flash_intensity);
        col_lin += flash_lin * 0.85;
        ambient_specular += flash_lin * 0.50;
    }

    // 8. Glass Patina & Micro-texture
    float patina_str = 0.015 * (1.0 - 0.50 * glossiness);
    float patina_noise = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453) - 0.5;
    vec3 patina = vec3(patina_noise * patina_str * intensity);

    // 9. Composite layers in linear space and convert to sRGB
    vec3 final_lin = col_lin + dynamic_glow + ghost_reflection + perimeter_catch + ambient_specular + patina;
    vec3 final_srgb = ToSrgb(clamp(final_lin, 0.0, 1.0));

    // Smooth anti-aliased edge blending at renderable boundary
    vec3 orig_srgb = texture(video_texture, v_tc).rgb;
    vec3 blended = mix(orig_srgb, final_srgb, inner_edge_mask);
    out_color = vec4(clamp(blended, 0.0, 1.0), 1.0);
}
