#version 330 core
// Bit-reverse permutation of complex data.
// Used before inverse FFT butterfly passes, since DIT butterfly
// requires bit-reversed input order.

in vec2 v_tc;
out vec2 fragColor;

uniform sampler2D input_texture;
uniform ivec2 fft_size;
uniform ivec2 u_log2_size;

uint bitReverse32(uint x) {
    x = ((x & 0x55555555u) << 1u) | ((x >> 1u) & 0x55555555u);
    x = ((x & 0x33333333u) << 2u) | ((x >> 2u) & 0x33333333u);
    x = ((x & 0x0F0F0F0Fu) << 4u) | ((x >> 4u) & 0x0F0F0F0Fu);
    x = ((x & 0x00FF00FFu) << 8u) | ((x >> 8u) & 0x00FF00FFu);
    return (x << 16u) | (x >> 16u);
}

uint bitReverse(uint x, uint bits) {
    return (bits == 0u) ? 0u : (bitReverse32(x) >> (32u - bits));
}

void main() {
    ivec2 pos = ivec2(gl_FragCoord.xy);

    uint bitsX = uint(max(u_log2_size.x, 0));
    uint bitsY = uint(max(u_log2_size.y, 0));
    uint revX = bitReverse(uint(pos.x), bitsX);
    uint revY = bitReverse(uint(pos.y), bitsY);

    // Fetch complex value from bit-reversed position
    fragColor = texelFetch(input_texture, ivec2(revX, revY), 0).rg;
}
