#version 330 core
// Visualize the FFT spectrum as a log-magnitude grayscale image
// with DC-center shift. Overlays the mask as a tinted region.

in vec2 v_tc;
out vec4 fragColor;

uniform sampler2D fft_texture;
uniform sampler2D mask_texture;
uniform ivec2 fft_size;

void main() {
    ivec2 pos = ivec2(gl_FragCoord.xy);

    // FFT shift: DC to center
    ivec2 shifted = (pos + fft_size / 2) % fft_size;

    vec2 complex_val = texelFetch(fft_texture, shifted, 0).rg;
    float magnitude = length(complex_val);

    // Log scale for visualization with good dynamic range
    // Use log2 with a generous divisor to spread the spectrum across [0,1]
    float viz = log2(1.0 + magnitude) / 22.0;
    viz = clamp(viz, 0.0, 1.0);

    // Read mask value (DC-centered coordinates match the mask directly)
    vec2 mask_uv = (vec2(pos) + 0.5) / vec2(fft_size);
    float mask = texture(mask_texture, mask_uv).r;

    // Show spectrum with mask overlay:
    // Where mask = 1 (pass), show normal spectrum
    // Where mask = 0 (blocked), tint red
    vec3 color;
    if (mask < 0.5) {
        // Blocked: show spectrum dimmed with red tint
        color = vec3(viz * 0.3 + 0.15, viz * 0.1, viz * 0.1);
    } else {
        // Passing: show normal grayscale spectrum
        color = vec3(viz);
    }

    fragColor = vec4(color, 1.0);
}
