#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D bezel_texture;
uniform vec2 outputResolution;
uniform vec2 source_size;
uniform float horizontal_stretch;
uniform vec4 border_crop; // left, right, top, bottom
uniform vec2 warp;        // warpX, warpY
uniform float corner_size;
uniform int filter_type;  // 0 = None, 1 = Lottes, 2 = Halo
uniform float time;

// Atlas sub-texture UV rectangles (texture flipped vertically so V=0 is bottom, V=1 is top):
// 1. Left Pillar:     [0.0000, 0.0000] to [0.2500, 1.0000] (512x2048)
// 2. Right Pillar:    [0.2500, 0.0000] to [0.5000, 1.0000] (512x2048)
// 3. Seamless Tile:   [0.5000, 0.5000] to [0.7500, 0.7500] (512x512)
// 4. Knobs Section:   [0.5000, 0.3750] to [0.7500, 0.5000] (512x256, 2:1)
// 5. Top Bezel/Vents: [0.5000, 0.7500] to [1.0000, 1.0000] (1024x512)
// 6. Power Section:   [0.7500, 0.5000] to [1.0000, 0.6250] (512x256, 2:1)
// 7. NEC Badge:       [0.7500, 0.6250] to [1.0000, 0.7500] (512x256, 2:1)


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
        pos *= vec2(1.0 + (pos.y * pos.y) * warp.x, 1.0 + (pos.x * pos.x) * warp.y);
        screen_tc = pos * 0.5 + 0.5;
    } else if (filter_type == 2) { // Halo curvature
        if (warp.x > 0.001 || warp.y > 0.001) {
            vec2 pos = centered_tc * 2.0 - 1.0;
            pos *= vec2(1.0 + (pos.y * pos.y) * 0.031, 1.0 + (pos.x * pos.x) * 0.041);
            screen_tc = pos * 0.5 + 0.5;
        }
    }

    // 3. Signed distance to active video boundaries in screen_tc space
    vec4 crop = border_crop;
    float d_left = crop.x - screen_tc.x;
    float d_right = screen_tc.x - (1.0 - crop.y);
    float d_top = screen_tc.y - (1.0 - crop.w);
    float d_bottom = crop.z - screen_tc.y;

    vec2 d = max(vec2(d_left, d_top), vec2(d_right, d_bottom));
    float max_d = max(d.x, d.y);

    // Halo rounded corners: match Halo scanlines Corner() on both unwarped and warped coords
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

    // Convert normalized distance to physical screen pixels
    vec2 video_pixels = outputResolution * scale;
    float min_vid_dim = min(video_pixels.x, video_pixels.y);
    float pix_dist_out = max_d * min_vid_dim;
    float pix_dist_in = -max_d * min_vid_dim;

    // INSIDE ACTIVE CRT VIDEO:
    if (max_d <= 0.0) {
        // Soft 3D ambient occlusion drop shadow from the inner bezel overhang onto the CRT glass
        float shadow_w = clamp(min_vid_dim * 0.010, 10.0, 24.0);
        if (pix_dist_in <= shadow_w) {
            float shadow_falloff = smoothstep(0.0, shadow_w, pix_dist_in);
            float shadow_alpha = (1.0 - shadow_falloff) * 0.45;
            if (d_top > d_bottom) shadow_alpha *= 1.25;
            if (d_left > d_right) shadow_alpha *= 1.20;
            out_color = vec4(0.0, 0.0, 0.0, clamp(shadow_alpha, 0.0, 0.55));
            return;
        }
        out_color = vec4(0.0);
        return;
    }

    // OUTSIDE ACTIVE CRT VIDEO (Retro PC Monitor Frame):
    // 1. Dark rubber gasket sealing the CRT tube edge
    float gasket_w = clamp(outputResolution.y * 0.0014, 2.0, 3.5);
    if (pix_dist_out <= gasket_w) {
        out_color = vec4(0.04, 0.04, 0.04, 1.0);
        return;
    }

    // 2. Scanline-dependent tube edge: conforms directly to barrel curvature
    float eff_warp_x = (filter_type == 1) ? warp.x : ((filter_type == 2 && (warp.x > 0.001 || warp.y > 0.001)) ? 0.031 : 0.0);
    float pos_y_scan = ((gl_FragCoord.y / outputResolution.y - 0.5) / scale.y + 0.5) * 2.0 - 1.0;
    float pos_x_left = (-1.0 + 2.0 * border_crop.x) / (1.0 + pos_y_scan * pos_y_scan * eff_warp_x);
    float x_edge_left = ((pos_x_left * 0.5 + 0.5 - 0.5) * scale.x + 0.5) * outputResolution.x;
    x_edge_left = max(x_edge_left, 1.0);
    float x_edge_right = outputResolution.x - x_edge_left;

    // Nominal widths
    float nom_pillar_w = (outputResolution.x - video_pixels.x) * 0.5;
    float nom_bar_h = (outputResolution.y - video_pixels.y) * 0.5;

    // 3. Chassis Faceplate Texturing:
    vec3 casing_color = vec3(0.70, 0.67, 0.60);
    bool is_left = (gl_FragCoord.x < outputResolution.x * 0.5);

    if (nom_pillar_w >= 20.0 || (nom_pillar_w >= nom_bar_h)) {
        // Pillarbox mode (or pillar dominant)
        if (is_left) {
            float u_l = clamp(gl_FragCoord.x / x_edge_left, 0.0, 1.0);
            float v_l = gl_FragCoord.y / outputResolution.y;
            casing_color = texture(bezel_texture, vec2(u_l * 0.25, v_l)).rgb;

            // Outer chassis vignette shadow on far left border
            float dist_left = gl_FragCoord.x;
            if (dist_left < 24.0) {
                casing_color *= smoothstep(0.0, 24.0, dist_left) * 0.30 + 0.70;
            }

            // Left faceplate is clean vintage ABS plastic with molded groove
        } else {
            float u_r = clamp((gl_FragCoord.x - x_edge_right) / max(outputResolution.x - x_edge_right, 1.0), 0.0, 1.0);
            float v_r = gl_FragCoord.y / outputResolution.y;
            casing_color = texture(bezel_texture, vec2(0.25 + u_r * 0.25, v_r)).rgb;

            // Outer chassis vignette shadow on far right border
            float dist_right = outputResolution.x - gl_FragCoord.x;
            if (dist_right < 24.0) {
                casing_color *= smoothstep(0.0, 24.0, dist_right) * 0.30 + 0.70;
            }
        }
    } else {
        // Letterbox dominant mode
        vec2 plastic_uv = vec2(0.50, 0.50) + fract(gl_FragCoord.xy / 256.0) * 0.25;
        casing_color = texture(bezel_texture, plastic_uv).rgb;

        if (gl_FragCoord.y > outputResolution.y * 0.5) {
            float u_t = (gl_FragCoord.x - x_edge_left) / max(video_pixels.x, 1.0);
            if (u_t >= 0.0 && u_t <= 1.0) {
                float v_t = (gl_FragCoord.y - (outputResolution.y - nom_bar_h)) / max(nom_bar_h, 1.0);
                casing_color = texture(bezel_texture, vec2(mix(0.50, 1.00, u_t), mix(0.75, 1.00, clamp(v_t, 0.0, 1.0)))).rgb;
            }
        }
    }

    // 4. Recessed 3D Inner Bevel (conforms dynamically to CRT screen curvature):
    float bevel_w = clamp(min_vid_dim * 0.017, 16.0, 42.0);
    if (pix_dist_out > gasket_w && pix_dist_out <= bevel_w) {
        float t = (pix_dist_out - gasket_w) / (bevel_w - gasket_w); // 0.0 at gasket, 1.0 at crest
        
        float w_top = max(d_top, 0.0);
        float w_bot = max(d_bottom, 0.0);
        float w_left = max(d_left, 0.0);
        float w_right = max(d_right, 0.0);
        float w_sum = max(w_top + w_bot + w_left + w_right, 0.0001);

        float l_top = mix(0.48, 0.85, t) * (w_top / w_sum);
        float l_bot = mix(1.35, 1.05, t) * (w_bot / w_sum);
        float l_left = mix(0.62, 0.95, t) * (w_left / w_sum);
        float l_right = mix(1.18, 1.02, t) * (w_right / w_sum);
        float bevel_light = l_top + l_bot + l_left + l_right;

        casing_color *= bevel_light;

        // Specular crest line highlight at outer ridge on bottom and right
        if (t > 0.85) {
            float crest = smoothstep(0.85, 1.0, t);
            float crest_weight = (w_bot + w_right) / w_sum;
            casing_color += vec3(0.12, 0.12, 0.10) * crest * crest_weight;
        }
    }

    out_color = vec4(casing_color, 1.0);
}
