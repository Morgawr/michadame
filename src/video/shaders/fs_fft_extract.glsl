#version 330 core
// Extract the final result from the IFFT output.
// Converts complex values back to RGB, crops to original resolution.
// Applies 1/N normalization for the inverse DFT.

in vec2 v_tc;
out vec4 fragColor;

uniform sampler2D ifft_texture;
uniform sampler2D original_texture;
uniform ivec2 fft_size;
uniform ivec2 orig_size;

void main() {
    ivec2 pos = ivec2(gl_FragCoord.xy);

    // Read the IFFT result (real part only, imaginary should be ~0)
    vec2 uv = (vec2(pos) + 0.5) / vec2(fft_size);
    float gray = texture(ifft_texture, uv).r;

    // Apply normalization: divide by N = fft_size.x * fft_size.y
    // The forward FFT and inverse FFT with Cooley-Tukey produce
    // an unnormalized result that must be divided by N total.
    float N = float(fft_size.x) * float(fft_size.y);
    gray = gray / N;

    // Clamp to valid range
    gray = clamp(gray, 0.0, 1.0);

    // Read original color to preserve chroma
    vec2 orig_uv = (vec2(pos) + 0.5) / vec2(orig_size);
    vec4 orig = texture(original_texture, orig_uv);
    float orig_luma = dot(orig.rgb, vec3(0.299, 0.587, 0.114));

    // Reconstruct: scale the original color by the ratio of new/old luma
    vec3 result;
    if (orig_luma > 0.001) {
        result = orig.rgb * (gray / orig_luma);
    } else {
        result = vec3(gray);
    }

    fragColor = vec4(clamp(result, 0.0, 1.0), orig.a);
}
