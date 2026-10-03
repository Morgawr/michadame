#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D input_texture;
uniform vec2 outputResolution;
uniform vec2 source_size;
uniform float horizontal_stretch;
uniform float time;
uniform vec4 border_crop; // left, right, top, bottom

// User-customizable parameters
uniform float intensity;          // Master effect intensity (0.0 to 1.0)
uniform float frequency;          // Speed/frequency multiplier (0.1 to 5.0)
uniform float randomization;      // Glitch & flicker unpredictability (0.0 to 1.0)
uniform float electricity_glow;   // Electric halo & selective highlight/shadow modulation
uniform float flicker_depth;      // Cathode power flicker & rolling AC hum depth
uniform float interference;       // RF static noise & micro-glitch lines
uniform float lightbulb_effect;   // Lightbulb glow & breathing of brights/darks

float hash12(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

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

void main() {
    // Calculate aspect fit to isolate active video area
    float video_aspect = (source_size.x * horizontal_stretch) / source_size.y;
    float output_aspect = outputResolution.x / outputResolution.y;

    vec2 scale = vec2(1.0, 1.0);
    if (video_aspect > output_aspect) {
        scale.y = output_aspect / video_aspect;
    } else {
        scale.x = video_aspect / output_aspect;
    }

    vec2 centered_tc = (v_tc - 0.5) / scale + 0.5;

    // Pillarbox / letterbox / border padding outside active video area remains untouched
    if (centered_tc.x < border_crop.x || centered_tc.x > (1.0 - border_crop.y) ||
        centered_tc.y < border_crop.z || centered_tc.y > (1.0 - border_crop.w)) {
        out_color = texture(input_texture, v_tc);
        return;
    }

    vec2 sample_uv = v_tc;

    // 1. Scanline micro sync jitter & intermittent horizontal glitch displacement
    if (interference * intensity > 0.001) {
        float t_block = floor(time * (12.0 * frequency + 4.0));
        float band = floor(centered_tc.y * 30.0);
        float glitch_prob = hash12(vec2(band, t_block));

        float glitch_threshold = 0.985 - 0.035 * randomization;
        if (glitch_prob > glitch_threshold) {
            float shift = (hash12(vec2(band * 3.7, t_block)) - 0.5) * 0.005 * interference * intensity;
            sample_uv.x += shift;
        }

        float scanline_idx = floor(centered_tc.y * source_size.y);
        float jitter = (hash12(vec2(scanline_idx, floor(time * 60.0))) - 0.5) * 0.0007 * randomization * intensity;
        sample_uv.x += jitter;

        vec2 sample_centered = (sample_uv - 0.5) / scale + 0.5;
        sample_centered = clamp(sample_centered, vec2(0.001), vec2(0.999));
        sample_uv = (sample_centered - 0.5) * scale + 0.5;
    }

    // 2. Base color and multi-tap electric halo bloom
    vec3 base_srgb = texture(input_texture, sample_uv).rgb;
    vec3 base_lin = ToLinear(base_srgb);
    float local_lum = dot(base_lin, vec3(0.2126, 0.7152, 0.0722));

    vec3 blur_lin = base_lin;
    if (electricity_glow * intensity > 0.001) {
        vec2 texel = 1.0 / outputResolution;
        float radius = 3.5;
        vec3 sum = vec3(0.0);
        float total_w = 0.0;

        for (int i = 0; i < 8; ++i) {
            float angle = float(i) * 0.78539816; // 2 * PI / 8
            vec2 off = vec2(cos(angle), sin(angle)) * radius * texel;
            vec3 s = ToLinear(texture(input_texture, sample_uv + off).rgb);
            float s_lum = dot(s, vec3(0.2126, 0.7152, 0.0722));
            float w = 1.0 + s_lum * 2.0;
            sum += s * w;
            total_w += w;
        }
        blur_lin = sum / total_w;
    }

    // 3. Selective "electricity" glow: highlights light up, shadows dynamically modulate
    vec3 electric_tint = vec3(0.96, 0.98, 1.04);
    vec3 halo = pow(blur_lin, vec3(1.3)) * (electricity_glow * 0.75 * intensity) * electric_tint;

    float electric_pulse = sin(time * 9.42477 * frequency) * 0.5 + 0.5;
    float micro_buzz = sin(time * 125.66 * frequency) * 0.04
                     + (hash12(vec2(time * 60.0, 0.5)) - 0.5) * 0.06 * randomization;
    float surge = (electric_pulse * 0.12 + micro_buzz) * electricity_glow * intensity;

    float bright_factor = smoothstep(0.35, 1.0, local_lum);
    vec3 bright_boost = base_lin * (surge * 1.6) * bright_factor;

    float dark_factor = 1.0 - smoothstep(0.05, 0.5, local_lum);
    vec3 dark_mod = -base_lin * (surge * 0.75) * dark_factor;

    vec3 color_lin = base_lin + halo * (1.0 + surge) + bright_boost + dark_mod;

    // 4. Lightbulb effect: dynamic glowing up and down of brights and darks
    if (lightbulb_effect * intensity > 0.001) {
        float t_slow = time * 2.2 * frequency;
        // Spatial variance so different screen regions undulate independently
        float wave_bright = sin(centered_tc.x * 5.0 + t_slow * 0.8) * cos(centered_tc.y * 4.0 - t_slow * 0.6);
        float wave_dark = cos(centered_tc.x * 4.0 - t_slow * 0.7 + 1.5) * sin(centered_tc.y * 5.0 + t_slow * 0.5 + 2.0);

        // Organic low-frequency breathing waveforms
        float pulse_a = sin(t_slow) * 0.5 + sin(t_slow * 0.61 + 1.2) * 0.35 + sin(t_slow * 1.73) * 0.15;
        float pulse_b = cos(t_slow * 0.85 + 0.8) * 0.5 + sin(t_slow * 1.33 + 2.1) * 0.3 + cos(t_slow * 2.1) * 0.2;

        float bright_pulse = mix(pulse_a, mix(pulse_a, wave_bright, 0.65), randomization);
        float dark_pulse = mix(pulse_b, mix(pulse_b, wave_dark, 0.65), randomization);

        // Bright areas glow up and down (with incandescent bloom)
        float bright_mask = smoothstep(0.25, 0.85, local_lum);
        vec3 bulb_bright = (base_lin * 0.4 + blur_lin * 0.45) * bright_pulse * bright_mask;

        // Dark areas breathe up and down in darkness/luminance
        float dark_mask = 1.0 - smoothstep(0.05, 0.5, local_lum);
        vec3 bulb_dark = (base_lin * 0.35 * dark_pulse) * dark_mask;

        color_lin += (bulb_bright + bulb_dark) * (lightbulb_effect * intensity);
    }

    // 5. Cathode screen flickering & AC line hum
    if (flicker_depth * intensity > 0.001) {
        float hum_pos = fract(centered_tc.y * 1.5 - time * 0.4 * frequency);
        float hum_bar = sin(hum_pos * 6.2831853) * 0.03 * flicker_depth * intensity;

        float f1 = sin(time * 75.0 * frequency);
        float f2 = sin(time * 119.0 * frequency);
        float rand_flicker = (hash12(vec2(floor(time * 30.0 * frequency), 2.718)) - 0.5) * 2.0 * randomization;

        float total_flicker = 1.0 + (hum_bar + (f1 * 0.015 + f2 * 0.01 + rand_flicker * 0.02) * flicker_depth) * intensity;
        color_lin *= max(total_flicker, 0.0);
    }

    // 6. Analog RF static & interference streaks
    if (interference * intensity > 0.001) {
        float noise_coord_x = floor(centered_tc.x * source_size.x * 0.5);
        float noise_coord_y = floor(centered_tc.y * source_size.y);
        float rf = (hash12(vec2(noise_coord_x, noise_coord_y) + vec2(time * 95.0, time * 33.0)) - 0.5)
                 * 0.035 * interference * intensity;

        float streak_y = floor(centered_tc.y * 120.0);
        float streak_active = hash12(vec2(streak_y, floor(time * 10.0 * frequency)));
        float streak = (streak_active > 0.982)
                     ? (hash12(vec2(centered_tc.x * 8.0, time)) - 0.5) * 0.07 * interference * intensity
                     : 0.0;

        color_lin += vec3(rf + streak);
    }

    // 7. Convert back to sRGB for final presentation
    color_lin = clamp(color_lin, 0.0, 1.0);
    out_color = vec4(ToSrgb(color_lin), 1.0);
}
