use eframe::glow::{self, HasContext};

pub const VS_SRC: &str = include_str!("../shaders/vs_src.glsl");
pub const FS_YUV_PLANAR: &str = include_str!("../shaders/fs_yuv_planar.glsl");
pub const FS_YUYV_PACKED: &str = include_str!("../shaders/fs_yuyv_packed.glsl");
pub const FS_MEDIAN_3X1: &str = include_str!("../shaders/fs_median_3x1.glsl");
pub const FS_PIXELATE: &str = include_str!("../shaders/fs_pixelate.glsl");
pub const FS_PASSTHROUGH: &str = include_str!("../shaders/fs_passthrough.glsl");
pub const FS_PASS0: &str = include_str!("../shaders/fs_pass0.glsl");
pub const FS_PASS1: &str = include_str!("../shaders/fs_pass1.glsl");
pub const FS_PASS2: &str = include_str!("../shaders/fs_pass2.glsl");
pub const FS_PASS3: &str = include_str!("../shaders/fs_pass3.glsl");
pub const FS_FINAL: &str = include_str!("../shaders/fs_final.glsl");
pub const FS_FFT_INIT: &str = include_str!("../shaders/fs_fft_init.glsl");
pub const FS_FFT_BUTTERFLY: &str = include_str!("../shaders/fs_fft_butterfly.glsl");
pub const FS_FFT_MASK: &str = include_str!("../shaders/fs_fft_mask.glsl");
pub const FS_FFT_EXTRACT: &str = include_str!("../shaders/fs_fft_extract.glsl");
pub const FS_FFT_SPECTRUM: &str = include_str!("../shaders/fs_fft_spectrum.glsl");
pub const FS_FFT_BITREV: &str = include_str!("../shaders/fs_fft_bitrev.glsl");

pub unsafe fn compile_program(gl: &glow::Context, vs_src: &str, fs_src: &str) -> glow::Program {
    let program = gl.create_program().expect("Cannot create program");

    let shader_sources = [
        (glow::VERTEX_SHADER, vs_src),
        (glow::FRAGMENT_SHADER, fs_src),
    ];

    let mut shaders = Vec::with_capacity(shader_sources.len());

    for (shader_type, shader_source) in shader_sources.iter() {
        let shader = gl
            .create_shader(*shader_type)
            .expect("Cannot create shader");
        gl.shader_source(shader, shader_source);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            panic!("{}", gl.get_shader_info_log(shader));
        }
        gl.attach_shader(program, shader);
        shaders.push(shader);
    }

    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        panic!("{}", gl.get_program_info_log(program));
    }

    for shader in shaders {
        gl.detach_shader(program, shader);
        gl.delete_shader(shader);
    }

    program
}
pub unsafe fn compile_compute_program(gl: &glow::Context, shader_source: &str) -> glow::Program {
    let program = gl.create_program().expect("Cannot create program");
    let shader = gl.create_shader(glow::COMPUTE_SHADER).expect("Cannot create shader");
    gl.shader_source(shader, shader_source);
    gl.compile_shader(shader);
    if !gl.get_shader_compile_status(shader) {
        panic!("{}", gl.get_shader_info_log(shader));
    }
    gl.attach_shader(program, shader);
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        panic!("{}", gl.get_program_info_log(program));
    }
    gl.detach_shader(program, shader);
    gl.delete_shader(shader);
    program
}
