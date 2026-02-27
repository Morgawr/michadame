#version 330 core
// Bit-reverse permutation of complex data.
// Used before inverse FFT butterfly passes, since DIT butterfly
// requires bit-reversed input order.

in vec2 v_tc;
out vec4 fragColor;

uniform sampler2D input_texture;
uniform ivec2 fft_size;

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

    uint bitsX = log2u(uint(fft_size.x));
    uint bitsY = log2u(uint(fft_size.y));
    uint revX = bitReverse(uint(pos.x), bitsX);
    uint revY = bitReverse(uint(pos.y), bitsY);

    // Fetch complex value from bit-reversed position
    fragColor = texelFetch(input_texture, ivec2(revX, revY), 0);
}
