#version 330 core
in vec2 v_tc;
out vec4 out_color;

uniform sampler2D input_texture;
uniform vec2 outputResolution;
uniform vec2 source_size;
uniform float horizontal_stretch;
uniform vec4 border_crop; // left, right, top, bottom
uniform vec2 warp;        // warpX, warpY
uniform float corner_size;
uniform int filter_type;  // 0 = Flat/Passthrough, 1 = Lottes, 2 = Halo
uniform float time;

// Real-time audio envelope uniforms (must match renderer.rs uniform lookups)
uniform float low_env;        // Sub-bass / low frequency envelope (< 180 Hz)
uniform float mid_env;        // Mid / voice / lead frequency envelope (180 Hz - 2.4 kHz)
uniform float high_env;       // High / presence / sizzle frequency envelope (> 2.4 kHz)
uniform float transient_env;  // Fast transient beat / impact spike
uniform float stereo_balance; // Signed stereo balance (-1.0 left .. +1.0 right)
uniform vec2 spring_pos;      // 2D underdamped harmonic spring oscillator state (x, y)
uniform float phase_accum;    // Audio-driven continuous phase accumulator

// User-customizable effect parameters
uniform float master_intensity;  // Master intensity multiplier (0.0 to 2.0)
uniform float deflection_shake;  // Magnetic yoke shake, S-wave shear & bounce (0.0 to 2.0)
uniform float stereo_tug;        // Directional L/R beam pull from stereo balance (0.0 to 2.0)
uniform float flyback_pump;      // High-voltage anode breathing / zoom & corner flex (0.0 to 2.0)
uniform float rgb_deconvergence; // RGB electron gun chromatic split on impacts (0.0 to 2.0)
uniform float scanline_wave;     // Voice-coil horizontal scanline waveform modulation (0.0 to 2.0)
uniform float phosphor_surge;    // Overdrive cathode bloom, brightboost & contrast pulse (0.0 to 2.0)
uniform float sync_tear;         // Horizontal sync-loss tear bands & RF spark static (0.0 to 2.0)

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
    float spring_mag = length(spring_pos);
    float total_energy = low_env + mid_env + high_env + transient_env + spring_mag;
    if (master_intensity <= 0.0005 || total_energy <= 0.0005) {
        out_color = texture(input_texture, v_tc);
        return;
    }

    // 1. Isolate active CRT tube area (matches Lottes, Halo, and Flat geometries)
    float video_aspect = (source_size.x * horizontal_stretch) / max(source_size.y, 1.0);
    float output_aspect = outputResolution.x / max(outputResolution.y, 1.0);

    vec2 scale = vec2(1.0, 1.0);
    if (video_aspect > output_aspect) {
        scale.y = output_aspect / video_aspect;
    } else {
        scale.x = video_aspect / output_aspect;
    }

    vec2 centered_tc = (v_tc - 0.5) / scale + 0.5;

    vec2 screen_tc = centered_tc;
    if (filter_type == 1) { // Lottes curvature
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

    vec4 crop = border_crop;
    float d_left = crop.x - screen_tc.x;
    float d_right = screen_tc.x - (1.0 - crop.y);
    float d_top = screen_tc.y - (1.0 - crop.z);
    float d_bottom = crop.w - screen_tc.y;

    vec2 d = max(vec2(d_left, d_top), vec2(d_right, d_bottom));
    float max_d = max(d.x, d.y);

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

    // Outside active CRT tube: preserve black bars / bezel area untouched
    if (max_d > 0.0) {
        out_color = texture(input_texture, v_tc);
        return;
    }

    // Normalized tube coordinates in [-1.0, 1.0]
    vec2 p = (centered_tc - 0.5) * 2.0;
    float r2 = dot(p, p);

    // 2. Flyback Voltage Breathing / Screen Pump (Anode Sag)
    vec2 displaced_p = p;
    float pump_drive = (low_env * 0.85 + transient_env * 0.75 + mid_env * 0.25) * flyback_pump * master_intensity;
    if (pump_drive > 0.0005) {
        // High-voltage sag expands the raster outward and flexes corner pincushion/barrel geometry
        float zoom = 1.0 / (1.0 + pump_drive * 0.14);
        float corner_flex = 1.0 - pump_drive * 0.065 * r2;
        displaced_p *= zoom * corner_flex;
    }

    // 3. Deflection Yoke Magnetic Shake, S-Wave Shear & Vertical Bounce
    float shake_amt = deflection_shake * master_intensity;
    if (shake_amt > 0.0005) {
        float s_wave = sin(p.y * 3.14159265 + phase_accum * 2.2) * (spring_pos.x * 0.038 + low_env * sin(time * 47.0) * 0.016);
        float h_judder = spring_pos.x * 0.032 + sin(time * 92.0 + p.y * 4.0) * (low_env * 0.014 + transient_env * 0.020);
        float v_bounce = spring_pos.y * 0.028 + cos(time * 68.0 + p.x * 2.5) * (low_env * 0.015 + transient_env * 0.018);
        displaced_p.x += (s_wave + h_judder) * shake_amt;
        displaced_p.y += v_bounce * shake_amt;
    }

    // 4. Directional Stereo Tug: louder left/right channel pulls electron beam horizontally
    float tug_amt = stereo_tug * master_intensity;
    if (tug_amt > 0.0005 && abs(stereo_balance) > 0.0005) {
        float vertical_profile = 0.50 + 0.50 * (1.0 - p.y * p.y);
        float audio_pull = stereo_balance * (0.4 + 0.8 * (low_env + mid_env + transient_env));
        displaced_p.x -= audio_pull * 0.055 * tug_amt * vertical_profile;
    }

    // 5. Voice-Coil Scanline Waveform ("Oscilloscope Warp")
    float wave_amt = scanline_wave * master_intensity;
    float wave_drive = mid_env * 0.95 + high_env * 0.65 + low_env * 0.35 + transient_env * 0.40;
    if (wave_amt * wave_drive > 0.0005) {
        float y_scan = centered_tc.y * max(source_size.y, 240.0);
        float w1 = sin(centered_tc.y * 28.0 - phase_accum * 7.5) * 0.50;
        float w2 = sin(centered_tc.y * 74.0 + phase_accum * 13.0) * 0.32;
        float w3 = cos(y_scan * 0.45 - time * 60.0) * 0.25 * (0.4 + high_env);
        displaced_p.x += (w1 + w2 + w3) * 0.052 * wave_drive * wave_amt;
    }

    // 6. Horizontal Sync-Loss Tear Bars
    float tear_amt = sync_tear * master_intensity;
    float tear_drive = transient_env * 1.10 + low_env * 0.50 + high_env * 0.45 + mid_env * 0.25;
    float active_tear_flash = 0.0;
    if (tear_amt * tear_drive > 0.001) {
        float t_step = floor(time * 28.0 + phase_accum * 3.5);
        float band_idx = floor(centered_tc.y * 24.0);
        float band_hash = hash12(vec2(band_idx, t_step));
        float tear_thresh = clamp(0.92 - tear_drive * 0.42 * min(tear_amt, 1.5), 0.30, 0.96);
        if (band_hash > tear_thresh) {
            float tear_dir = (hash12(vec2(band_idx * 4.13, t_step + 7.0)) - 0.5) * 2.0;
            float band_local_y = fract(centered_tc.y * 24.0);
            float envelope_y = sin(band_local_y * 3.14159265);
            float tear_shift = tear_dir * envelope_y * 0.085 * tear_drive * tear_amt;
            displaced_p.x += tear_shift;
            active_tear_flash = abs(tear_shift) * 5.0;
        }
    }

    // Map displaced tube coordinates back to texture space, clamping within active crop bounds
    vec2 displaced_centered = displaced_p * 0.5 + 0.5;
    vec2 min_tc = vec2(crop.x + 0.001, crop.w + 0.001);
    vec2 max_tc = vec2(1.0 - crop.y - 0.001, 1.0 - crop.z - 0.001);
    displaced_centered = clamp(displaced_centered, min_tc, max_tc);
    vec2 base_uv = (displaced_centered - 0.5) * scale + 0.5;

    // 7. RGB Electron Gun De-Convergence / Chromatic Shockwave
    float deconv_amt = rgb_deconvergence * master_intensity;
    float deconv_drive = transient_env * 0.95 + high_env * 0.65 + low_env * 0.50 + mid_env * 0.35;
    vec3 base_lin;
    if (deconv_amt * deconv_drive > 0.0005) {
        float angle = spring_pos.x * 1.2 + sin(time * 41.0 + centered_tc.y * 8.0) * 0.35;
        vec2 radial = p * (0.50 + 0.65 * r2);
        vec2 dir = vec2(cos(angle), sin(angle) * 0.45) + radial * 0.85;
        vec2 offset_tc = dir * (0.022 * deconv_drive * deconv_amt);

        vec2 uv_r = (clamp(displaced_centered + offset_tc, min_tc, max_tc) - 0.5) * scale + 0.5;
        vec2 uv_g = (clamp(displaced_centered - offset_tc * 0.20, min_tc, max_tc) - 0.5) * scale + 0.5;
        vec2 uv_b = (clamp(displaced_centered - offset_tc, min_tc, max_tc) - 0.5) * scale + 0.5;

        float r_srgb = texture(input_texture, uv_r).r;
        float g_srgb = texture(input_texture, uv_g).g;
        float b_srgb = texture(input_texture, uv_b).b;
        base_lin = ToLinear(vec3(r_srgb, g_srgb, b_srgb));
    } else {
        base_lin = ToLinear(texture(input_texture, base_uv).rgb);
    }

    vec3 color_lin = base_lin;

    // 8. Overdrive Phosphor Surge & Halation Flash
    float surge_amt = phosphor_surge * master_intensity;
    float surge_drive = transient_env * 0.90 + low_env * 0.65 + mid_env * 0.55 + high_env * 0.30;
    if (surge_amt * surge_drive > 0.0005) {
        vec2 texel = 1.0 / max(outputResolution, vec2(1.0));
        float radius = 3.0 + surge_drive * 6.0;
        vec3 bloom_sum = vec3(0.0);
        float weight_sum = 0.0;

        for (int i = 0; i < 8; ++i) {
            float ang = float(i) * 0.78539816;
            vec2 tap_uv = base_uv + vec2(cos(ang), sin(ang)) * radius * texel;
            vec3 tap_lin = ToLinear(texture(input_texture, tap_uv).rgb);
            float tap_lum = dot(tap_lin, vec3(0.2126, 0.7152, 0.0722));
            float w = 1.0 + tap_lum * 3.0;
            bloom_sum += tap_lin * w;
            weight_sum += w;
        }
        vec3 halo_lin = bloom_sum / max(weight_sum, 1.0);

        float lum = dot(base_lin, vec3(0.2126, 0.7152, 0.0722));
        float highlight_mask = smoothstep(0.12, 0.85, lum);
        vec3 phosphor_tint = vec3(0.96, 1.00, 1.08);

        float boost = surge_drive * surge_amt;
        color_lin += halo_lin * (0.85 * boost) * phosphor_tint;
        color_lin += base_lin * (0.65 * boost * highlight_mask);
    }

    // 9. High-Frequency RF Spark Static & Sync-Tear Scanline Flash
    if (tear_amt > 0.001) {
        float rf_drive = (high_env * 0.95 + transient_env * 0.75 + mid_env * 0.30) * tear_amt;
        if (rf_drive > 0.002) {
            vec2 noise_cell = vec2(
                floor(displaced_centered.x * max(source_size.x, 320.0) * 0.5),
                floor(displaced_centered.y * max(source_size.y, 240.0))
            );
            float spark_hash = hash12(noise_cell + vec2(floor(time * 120.0), floor(phase_accum * 37.0)));
            float spark_mask = step(0.982 - clamp(rf_drive * 0.055, 0.0, 0.08), spark_hash);
            float spark_val = (spark_hash - 0.92) * 10.0 * spark_mask * min(rf_drive, 1.8);
            float continuous_rf = (hash12(noise_cell + vec2(time * 91.0, time * 29.0)) - 0.5) * 0.06 * rf_drive;

            color_lin += vec3(max(0.0, spark_val) + continuous_rf) * vec3(0.92, 0.96, 1.08);
        }
        color_lin += vec3(active_tear_flash * 0.16) * vec3(0.90, 0.95, 1.05);
    }

    color_lin = clamp(color_lin, 0.0, 1.0);
    out_color = vec4(ToSrgb(color_lin), 1.0);
}
