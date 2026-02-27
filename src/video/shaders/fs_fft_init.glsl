#version 330 core
// FFT Init: Convert input RGB texture to grayscale complex values
// with bit-reversed indexing for the butterfly FFT algorithm.
// Output: RG = (real, imag) in bit-reversed order

in vec2 v_tc;
out vec4 fragColor;

uniform sampler2D input_texture;
uniform ivec2 fft_size;      // padded power-of-2 size
uniform ivec2 orig_size;     // original frame size

// Bit-reverse an index for FFT butterfly ordering
uint bitReverse(uint x, uint bits) {
    uint result = 0u;
    for (uint i = 0u; i < bits; i++) {
        result = (result << 1u) | (x & 1u);
        x >>= 1u;
    }
    return result;
}

uint log2u(uint n) {
    uint r = 0u;
    while (n > 1u) { n >>= 1u; r++; }
    return r;
}

void main() {
    ivec2 pos = ivec2(gl_FragCoord.xy);

    // Bit-reverse both x and y indices
    uint bitsX = log2u(uint(fft_size.x));
    uint bitsY = log2u(uint(fft_size.y));
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
    fragColor = vec4(gray, 0.0, 0.0, 1.0);
}
