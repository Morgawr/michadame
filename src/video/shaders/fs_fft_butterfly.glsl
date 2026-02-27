#version 330 core
// FFT Butterfly pass shader.
// Performs one stage of the Cooley-Tukey radix-2 FFT.
// Uniforms control axis (0=horizontal, 1=vertical), step size, and direction.

in vec2 v_tc;
out vec4 fragColor;

uniform sampler2D input_texture;
uniform int axis;        // 0 = horizontal (along X), 1 = vertical (along Y)
uniform int stage;       // current butterfly stage (m = 2^stage)
uniform int direction;   // 0 = forward FFT, 1 = inverse FFT
uniform ivec2 fft_size;

const float PI = 3.14159265358979323846;

vec2 cmul(vec2 a, vec2 b) {
    return vec2(a.x*b.x - a.y*b.y, a.x*b.y + a.y*b.x);
}

void main() {
    ivec2 pos = ivec2(gl_FragCoord.xy);

    int idx = (axis == 0) ? pos.x : pos.y;

    int m = 1 << stage;        // butterfly group size
    int half_m = m >> 1;       // half of group size

    // Which group and position within group
    int group = idx / m;
    int j = idx % m;           // position within the group

    // Determine if this is an even or odd element
    ivec2 even_pos, odd_pos;
    if (j < half_m) {
        // We are the even element
        even_pos = pos;
        odd_pos = pos;
        if (axis == 0) odd_pos.x += half_m;
        else odd_pos.y += half_m;
    } else {
        // We are the odd element
        even_pos = pos;
        if (axis == 0) even_pos.x -= half_m;
        else even_pos.y -= half_m;
        odd_pos = pos;
    }

    vec2 even_val = texelFetch(input_texture, even_pos, 0).rg;
    vec2 odd_val  = texelFetch(input_texture, odd_pos, 0).rg;

    // Twiddle factor
    int k = j % half_m;
    float angle = -2.0 * PI * float(k) / float(m);
    if (direction == 1) angle = -angle;  // inverse FFT uses positive angle

    vec2 twiddle = vec2(cos(angle), sin(angle));
    vec2 t = cmul(twiddle, odd_val);

    vec2 result;
    if (j < half_m) {
        result = even_val + t;
    } else {
        result = even_val - t;
    }

    fragColor = vec4(result, 0.0, 1.0);
}
