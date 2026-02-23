#version 330 core
    in vec2 v_tc;
    out vec4 out_color;

    uniform sampler2D video_texture;
    uniform vec2 target_resolution; // e.g., 854.0, 480.0 for 16:9 480p

    void main() {
        // Calculate the size of a 'pixel' in the low-resolution target.
        vec2 pixel_size = 1.0 / target_resolution;

        // Find the coordinate of the center of the low-res 'pixel' block.
        vec2 pixelated_uv = (floor(v_tc / pixel_size) + 0.5) * pixel_size;

        out_color = texture(video_texture, pixelated_uv);
    }