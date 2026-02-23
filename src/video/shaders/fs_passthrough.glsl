#version 330 core
    in vec2 v_tc;
    out vec4 out_color;
    uniform sampler2D video_texture;
    uniform vec2 videoResolution;
    uniform vec2 outputResolution;
    uniform vec3 background_color;
    uniform float horizontal_stretch;
    uniform float vibrance;
    
    // Convert from linear to sRGB color space
    float ToSrgb1(float c) {
        return (c < 0.0031308 ? c * 12.92 : 1.055 * pow(c, 0.41666) - 0.055);
    }
    vec3 ToSrgb(vec3 c) {
        return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
    }

    void main() {
        vec2 corrected_tc = vec2(v_tc.x, 1.0 - v_tc.y);
        float video_aspect = (videoResolution.x * horizontal_stretch) / videoResolution.y;
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
            vec3 linear_color = texture(video_texture, centered_tc).rgb;
            // Apply vibrance (saturation boost in linear space)
            float luminance = dot(linear_color, vec3(0.299, 0.587, 0.114));
            linear_color = mix(vec3(luminance), linear_color, vibrance);
            out_color = vec4(ToSrgb(linear_color), 1.0);
        }
    }