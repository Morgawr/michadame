#version 330 core

in vec3 v_world_pos;
in vec3 v_world_norm;
in vec2 v_uv;

uniform sampler2D u_main_tex;
uniform sampler2D u_shade_tex;
uniform sampler2D u_matcap_tex;

uniform vec4 u_base_color;
uniform vec3 u_shade_color;
uniform vec4 u_uv_transform; // xy = offset, zw = scale
uniform float u_shading_shift;
uniform float u_shading_toony;
uniform vec3 u_rim_color;
uniform float u_rim_fresnel_power;
uniform float u_rim_lift;
uniform vec3 u_matcap_factor;
uniform bool u_has_matcap;

uniform int u_alpha_mode; // 0 = OPAQUE, 1 = MASK, 2 = BLEND
uniform float u_alpha_cutoff;

uniform bool u_is_outline_pass;
uniform vec4 u_outline_color;
uniform float u_outline_lighting_mix;

uniform vec3 u_light_dir;
uniform vec3 u_camera_pos;
uniform mat4 u_view_mat;

out vec4 out_color;

void main() {
    vec2 uv = v_uv * u_uv_transform.zw + u_uv_transform.xy;
    vec4 main_sample = texture(u_main_tex, uv);
    vec4 base = main_sample * u_base_color;

    float alpha = base.a;
    if (u_alpha_mode == 0) {
        alpha = 1.0;
    } else if (u_alpha_mode == 1) {
        if (alpha < u_alpha_cutoff) {
            discard;
        }
        alpha = 1.0;
    } else {
        if (alpha < 0.004) {
            discard;
        }
    }

    vec3 n = normalize(v_world_norm);
    if (!gl_FrontFacing && !u_is_outline_pass) {
        n = -n;
    }

    // MToon cel-shaded diffuse ramp
    float ndotl = dot(n, normalize(u_light_dir));
    float shifted = ndotl + u_shading_shift;
    float feather = max(1.0 - u_shading_toony, 0.02);
    float light_intensity = smoothstep(-feather, feather, shifted);
    // Soft anime studio ambient fill so shadowed faces never crush to harsh black
    light_intensity = mix(0.22, 1.0, light_intensity);

    vec3 lit_rgb = base.rgb;
    vec3 shade_sample = texture(u_shade_tex, uv).rgb;
    vec3 shade_rgb = shade_sample * u_shade_color;
    vec3 surface_rgb = mix(shade_rgb, lit_rgb, light_intensity);

    if (u_is_outline_pass) {
        vec3 outline_rgb = u_outline_color.rgb * mix(vec3(1.0), surface_rgb, u_outline_lighting_mix);
        float out_a = alpha * u_outline_color.a;
        out_color = vec4(outline_rgb * out_a, out_a);
        return;
    }

    vec3 view_dir = normalize(u_camera_pos - v_world_pos);

    // Optional MatCap sphere additive highlight (eye/hair sheen)
    if (u_has_matcap) {
        vec3 view_n = normalize(mat3(u_view_mat) * n);
        vec2 matcap_uv = view_n.xy * 0.495 + 0.5;
        matcap_uv.y = 1.0 - matcap_uv.y;
        surface_rgb += texture(u_matcap_tex, matcap_uv).rgb * u_matcap_factor;
    }

    // Parametric Fresnel rim lighting
    float ndotv = max(dot(n, view_dir), 0.0);
    float rim_term = pow(clamp(1.0 - ndotv + u_rim_lift, 0.0, 1.0), max(u_rim_fresnel_power, 0.1));
    surface_rgb += u_rim_color * rim_term * mix(0.35, 1.0, light_intensity);

    surface_rgb = clamp(surface_rgb, 0.0, 1.0);
    // Output premultiplied RGBA for clean halo-free alpha compositing
    out_color = vec4(surface_rgb * alpha, alpha);
}
