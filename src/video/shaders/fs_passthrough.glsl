#version 330 core
    in vec2 v_tc;
    out vec4 out_color;
    uniform sampler2D video_texture;
    uniform vec2 outputResolution;
    uniform vec3 background_color;
    uniform float horizontal_stretch;
    uniform float vibrance;
    
    uniform int scaler_filter;

    // Convert from linear to sRGB color space
    float ToSrgb1(float c) {
        return (c < 0.0031308 ? c * 12.92 : 1.055 * pow(c, 0.41666) - 0.055);
    }
    vec3 ToSrgb(vec3 c) {
        return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
    }

    // --- BICUBIC SURROUND MAP ---
    vec4 cubic(float v) {
        vec4 n = vec4(1.0, 2.0, 3.0, 4.0) - v;
        vec4 s = n * n * n;
        float x = s.x;
        float y = s.y - 4.0 * s.x;
        float z = s.z - 4.0 * s.y + 6.0 * s.x;
        float w = 6.0 - x - y - z;
        return vec4(x, y, z, w) * (1.0/6.0);
    }

    vec3 SampleBicubic(sampler2D tex, vec2 uv, vec2 texSize) {
        vec2 invTexSize = 1.0 / texSize;
        vec2 tc = uv * texSize - 0.5;
        vec2 f = fract(tc);
        tc -= f;
        
        vec4 xcubic = cubic(f.x);
        vec4 ycubic = cubic(f.y);

        vec4 c = tc.xxyy + vec2(-0.5, +1.5).xyxy;
        vec4 s = vec4(xcubic.xz + xcubic.yw, ycubic.xz + ycubic.yw);
        vec4 offset = c + vec4(xcubic.yw, ycubic.yw) / s;

        offset *= invTexSize.xxyy;

        vec3 sample0 = texture(tex, offset.xz).rgb;
        vec3 sample1 = texture(tex, offset.yz).rgb;
        vec3 sample2 = texture(tex, offset.xw).rgb;
        vec3 sample3 = texture(tex, offset.yw).rgb;

        float sx = s.x / (s.x + s.y);
        float sy = s.z / (s.z + s.w);

        return mix(mix(sample3, sample2, sx), mix(sample1, sample0, sx), sy);
    }

    // --- LANCZOS 2 SURROUND MAP ---
    const float PI = 3.14159265359;
    
    float lanczosWeight(float x) {
        if (x == 0.0) return 1.0;
        if (abs(x) >= 2.0) return 0.0;
        return (sin(PI * x) / (PI * x)) * (sin(PI * x / 2.0) / (PI * x / 2.0));
    }

    vec3 SampleLanczos(sampler2D tex, vec2 uv, vec2 texSize) {
        vec2 invTexSize = 1.0 / texSize;
        vec2 tc = uv * texSize - 0.5;
        vec2 baseTc = floor(tc);
        vec2 f = tc - baseTc;

        vec3 color = vec3(0.0);
        float totalWeight = 0.0;

        for (int y = -1; y <= 2; y++) {
            float wy = lanczosWeight(float(y) - f.y);
            for (int x = -1; x <= 2; x++) {
                float wx = lanczosWeight(float(x) - f.x);
                float weight = wx * wy;

                vec2 sampleTc = (baseTc + vec2(x, y) + 0.5) * invTexSize;
                color += texture(tex, clamp(sampleTc, 0.0, 1.0)).rgb * weight;
                totalWeight += weight;
            }
        }
        return color / totalWeight;
    }

    // --- ROUTER ---
    vec3 SampleVideo(sampler2D tex, vec2 uv, vec2 texSize, int shader_filter) {
        if (shader_filter == 2) { // Bicubic
            return SampleBicubic(tex, uv, texSize);
        } else if (shader_filter == 4) { // Lanczos
            return SampleLanczos(tex, uv, texSize);
        } else {
            // Linear or FastBilinear or Point
            // Point relies on hardware GL_NEAREST, the rest use GL_LINEAR
            return texture(tex, uv).rgb;
        }
    }

    void main() {
        vec2 corrected_tc = vec2(v_tc.x, 1.0 - v_tc.y);
        vec2 video_res = vec2(textureSize(video_texture, 0)); // Calculate aspect ratios
        float video_aspect = (video_res.x * horizontal_stretch) / video_res.y;
        float output_aspect = outputResolution.x / outputResolution.y;

        vec2 scale = vec2(1.0, 1.0);
        if (video_aspect > output_aspect) {
            scale.y = output_aspect / video_aspect;
        } else {
            scale.x = video_aspect / output_aspect;
        }

        vec2 centered_tc = (corrected_tc - 0.5) / scale + 0.5;

        if (centered_tc.x < 0.0 || centered_tc.x > 1.0 || centered_tc.y < 0.0 || centered_tc.y > 1.0) {
            out_color = vec4(ToSrgb(background_color), 1.0);
        } else {
            vec3 linear_color = SampleVideo(video_texture, centered_tc, video_res, scaler_filter);
            // Apply vibrance (saturation boost in linear space)
            float luminance = dot(linear_color, vec3(0.2126, 0.7152, 0.0722));
            linear_color = mix(vec3(luminance), linear_color, vibrance);
            out_color = vec4(ToSrgb(linear_color), 1.0);
        }
    }