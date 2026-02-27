#version 330 core
// Extract the final result from the IFFT output.
// Converts complex values back to RGB, crops to original resolution.
// Applies 1/N normalization for the inverse DFT.
// Optionally skips FFT filtering for dark areas based on neighborhood average.

in vec2 v_tc;
out vec4 fragColor;

uniform sampler2D ifft_texture;
uniform sampler2D original_texture;
uniform ivec2 fft_size;
uniform ivec2 orig_size;
uniform float black_threshold; // 0.0 = apply everywhere, 1.0 = only apply to bright areas

void main() {
    ivec2 pos = ivec2(gl_FragCoord.xy);

    // Read original color
    vec2 orig_uv = (vec2(pos) + 0.5) / vec2(orig_size);
    vec4 orig = texture(original_texture, orig_uv);

    // Check neighborhood darkness if threshold > 0
    if (black_threshold > 0.0) {
        // Sample 9x9 neighborhood of the original image
        float avg_luma = 0.0;
        float count = 0.0;
        vec2 inv_size = 1.0 / vec2(orig_size);
        for (int dy = -4; dy <= 4; dy++) {
            for (int dx = -4; dx <= 4; dx++) {
                vec2 sample_uv = (vec2(pos + ivec2(dx, dy)) + 0.5) * inv_size;
                // Clamp to valid range
                sample_uv = clamp(sample_uv, vec2(0.0), vec2(1.0));
                vec3 s = texture(original_texture, sample_uv).rgb;
                avg_luma += dot(s, vec3(0.299, 0.587, 0.114));
                count += 1.0;
            }
        }
        avg_luma /= count;

        // If neighborhood is dark, skip the FFT filter entirely
        if (avg_luma < black_threshold) {
            fragColor = orig;
            return;
        }
    }

    // Read the IFFT result (real part only, imaginary should be ~0)
    vec2 uv = (vec2(pos) + 0.5) / vec2(fft_size);
    float gray = texture(ifft_texture, uv).r;

    // Apply normalization: divide by N = fft_size.x * fft_size.y
    float N = float(fft_size.x) * float(fft_size.y);
    gray = gray / N;

    // Clamp to valid range
    gray = clamp(gray, 0.0, 1.0);

    // Preserve chroma from original
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
