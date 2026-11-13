#version 330 core
// FFT Init: Convert input RGB texture to grayscale complex values
// with bit-reversed indexing for the butterfly FFT algorithm.
// Output: RG = (real, imag) in bit-reversed order

in vec2 v_tc;
out vec2 fragColor;

uniform sampler2D input_texture;
uniform ivec2 fft_size;      // padded power-of-2 size
uniform ivec2 orig_size;     // original frame size
uniform ivec2 u_log2_size;   // (log2(fft_size.x), log2(fft_size.y))

uint bitReverse32(uint x) {
    x = ((x & 0x55555555u) << 1u) | ((x >> 1u) & 0x55555555u);
    x = ((x & 0x33333333u) << 2u) | ((x >> 2u) & 0x33333333u);
    x = ((x & 0x0F0F0F0Fu) << 4u) | ((x >> 4u) & 0x0F0F0F0Fu);
    x = ((x & 0x00FF00FFu) << 8u) | ((x >> 8u) & 0x00FF00FFu);
    return (x << 16u) | (x >> 16u);
}

// Bit-reverse an index for FFT butterfly ordering in 5 constant ALU steps
uint bitReverse(uint x, uint bits) {
    return (bits == 0u) ? 0u : (bitReverse32(x) >> (32u - bits));
}

void main() {
    ivec2 pos = ivec2(gl_FragCoord.xy);

    // Bit-reverse both x and y indices
    uint bitsX = uint(max(u_log2_size.x, 0));
    uint bitsY = uint(max(u_log2_size.y, 0));
    uint revX = bitReverse(uint(pos.x), bitsX);
    uint revY = bitReverse(uint(pos.y), bitsY);

    // Sample from the original texture at the bit-reversed position
    // If outside original bounds, pad with zero (black)
    float gray = 0.0;
    if (int(revX) < orig_size.x && int(revY) < orig_size.y) {
        vec2 uv = (vec2(float(revX), float(revY)) + 0.5) / vec2(orig_size);
        vec4 c = texture(input_texture, uv);
        gray = dot(c.rgb, vec3(0.299, 0.587, 0.114));
    }

    // Store as complex number: (real, imag) = (gray, 0)
    // No normalization here - normalization is applied at the extract step
    fragColor = vec2(gray, 0.0);
}
