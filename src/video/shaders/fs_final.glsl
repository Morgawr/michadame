#version 330 core
    in vec2 v_tc;
    out vec4 out_color;

    uniform sampler2D pass1_texture; // bloom
    uniform sampler2D pass3_texture; // scanlines

    uniform vec2 videoResolution;
    uniform vec2 outputResolution;

    uniform float warpX;
    uniform float warpY;
    uniform float shadowMask; // 0-4
    uniform float brightboost;
    uniform float bloomAmount;
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

    vec3 Mask(vec2 pos) {
        float maskDark = 0.5;
        vec3 mask = vec3(0.5, 0.5, 0.5); // maskDark
        float maskLight = 1.5;
        if (shadowMask == 1.0) { // Compressed TV
            float line = maskLight;
            float odd = 0.0;
            if (fract(pos.x / 6.0) < 0.5) odd = 1.0;
            if (fract((pos.y + odd) / 2.0) < 0.5) line = maskDark;
            pos.x = fract(pos.x / 3.0);
            if (pos.x < 0.333) mask.r = maskLight;
            else if (pos.x < 0.666) mask.g = maskLight;
            else mask.b = maskLight;
            mask *= line;
        } else if (shadowMask == 2.0) { // Aperture-grille
            pos.x = fract(pos.x / 3.0);
            if (pos.x < 0.333) mask.r = maskLight;
            else if (pos.x < 0.666) mask.g = maskLight;
            else mask.b = maskLight;
        } else if (shadowMask == 3.0) { // Stretched VGA
            pos.x += pos.y * 3.0;
            pos.x = fract(pos.x / 6.0);
            if (pos.x < 0.333) mask.r = maskLight;
            else if (pos.x < 0.666) mask.g = maskLight;
            else mask.b = maskLight;
        } else if (shadowMask == 4.0) { // VGA
            pos.xy = floor(pos.xy * vec2(1.0, 0.5));
            pos.x += pos.y * 3.0;
            pos.x = fract(pos.x / 6.0);
            if (pos.x < 0.333) mask.r = maskLight;
            else if (pos.x < 0.666) mask.g = maskLight;
            else mask.b = maskLight;
        }
        return mask;
    }

    void main() {
        // Correct for source inversion only in the final pass.
        vec2 corrected_tc = vec2(v_tc.x, 1.0 - v_tc.y);

        // Calculate aspect ratios
        float video_aspect = (videoResolution.x * horizontal_stretch) / videoResolution.y;
        float output_aspect = outputResolution.x / outputResolution.y;

        // Determine scale and offset to letterbox/pillarbox the video
        vec2 scale = vec2(1.0, 1.0);
        if (video_aspect > output_aspect) {
            scale.y = output_aspect / video_aspect;
        } else {
            scale.x = video_aspect / output_aspect;
        }

        // First check if we are in the letterbox/pillarbox bars.
        // These should be colored with the background color.
        vec2 centered_pos = (corrected_tc - 0.5) / scale + 0.5;
        if (centered_pos.x < 0.0 || centered_pos.x > 1.0 || centered_pos.y < 0.0 || centered_pos.y > 1.0) {
            out_color = vec4(background_color, 1.0);
            return;
        }

        // Now apply warp for the CRT curvature.
        // If the warped position is outside the video area, it should be BLACK (behind the tube).
        vec2 warped_tc = Warp(corrected_tc);
        vec2 warped_pos = (warped_tc - 0.5) / scale + 0.5;

        if (warped_pos.x < 0.0 || warped_pos.x > 1.0 || warped_pos.y < 0.0 || warped_pos.y > 1.0) {
            out_color = vec4(0.0, 0.0, 0.0, 1.0);
            return;
        }

        // Current inputs (scanline, bloom) are in linear space.
        vec3 scanline_color = texture(pass3_texture, warped_pos).rgb; 
        vec3 bloom_color = texture(pass1_texture, warped_pos).rgb;

        vec3 final_color = scanline_color + bloom_color * bloomAmount;

        if (shadowMask > 0.0) {
            final_color *= Mask(floor(v_tc * outputResolution) + 0.5);
        }

        final_color *= brightboost;

        // Apply vibrance (saturation boost in linear space)
        float luminance = dot(final_color, vec3(0.2126, 0.7152, 0.0722));
        final_color = mix(vec3(luminance), final_color, vibrance);

        out_color = vec4(ToSrgb(final_color), 1.0);
    }
