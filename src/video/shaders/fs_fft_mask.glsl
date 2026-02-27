#version 330 core
// Apply mask to frequency domain data.
// The mask is DC-centered (0,0 = DC at center), but the FFT data
// has DC at (0,0). We apply fftshift to align them.

in vec2 v_tc;
out vec4 fragColor;

uniform sampler2D fft_texture;
uniform sampler2D mask_texture;
uniform ivec2 fft_size;

void main() {
    ivec2 pos = ivec2(gl_FragCoord.xy);

    // FFT shift: move DC from corner to center for mask lookup
    ivec2 shifted = (pos + fft_size / 2) % fft_size;
    vec2 mask_uv = (vec2(shifted) + 0.5) / vec2(fft_size);

    float mask = texture(mask_texture, mask_uv).r;

    vec2 fft_val = texelFetch(fft_texture, pos, 0).rg;

    // mask=1.0 means pass through, mask=0.0 means block
    // The mask texture stores 1.0 for pass, 0.0 for block
    fragColor = vec4(fft_val * mask, 0.0, 1.0);
}
