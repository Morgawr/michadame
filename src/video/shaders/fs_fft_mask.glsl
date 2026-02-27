#version 330 core
// Apply mask to frequency domain data.
// The mask is DC-centered (0,0 = DC at center), but the FFT data
// has DC at (0,0). We apply fftshift to align them.
// Dynamic threshold: only block frequencies whose magnitude exceeds the threshold.

in vec2 v_tc;
out vec4 fragColor;

uniform sampler2D fft_texture;
uniform sampler2D mask_texture;
uniform ivec2 fft_size;
uniform float mask_threshold; // 0.0 = block everything masked, 1.0 = only block very bright peaks

void main() {
    ivec2 pos = ivec2(gl_FragCoord.xy);

    vec2 fft_val = texelFetch(fft_texture, pos, 0).rg;

    // FFT shift: move DC from corner to center for mask lookup
    ivec2 shifted = (pos + fft_size / 2) % fft_size;
    vec2 mask_uv = (vec2(shifted) + 0.5) / vec2(fft_size);

    float mask = texture(mask_texture, mask_uv).r;

    // If mask says "pass" (1.0), always pass through
    if (mask > 0.5) {
        fragColor = vec4(fft_val, 0.0, 1.0);
        return;
    }

    // Mask says "block" (0.0) — check if magnitude exceeds threshold
    float magnitude = length(fft_val);

    // Normalize magnitude to a perceptual scale matching the spectrum display
    // Use the same log2 scale as the spectrum visualization
    float viz = log2(1.0 + magnitude) / 22.0;

    // Only block if the spectral intensity exceeds the threshold
    // threshold=0: block everything that's masked (even black areas)
    // threshold=1: only block the very brightest peaks
    if (viz >= mask_threshold) {
        // Block: zero out this frequency
        fragColor = vec4(0.0, 0.0, 0.0, 1.0);
    } else {
        // Below threshold: pass through even though mask says block
        fragColor = vec4(fft_val, 0.0, 1.0);
    }
}
