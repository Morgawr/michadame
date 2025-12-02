use eframe::glow::{self, HasContext};
use eframe::{egui, egui_glow};

const VERTEX_SHADER: &str = r#"
    #version 330 core
    layout (location = 0) in vec2 aPos;
    layout (location = 1) in vec2 aTexCoord;

    out vec2 TexCoord;

    void main() {
        gl_Position = vec4(aPos, 0.0, 1.0);
        TexCoord = aTexCoord;
    }
"#;

const FRAGMENT_SHADER: &str = r#"
    #version 330 core
    out vec4 FragColor;

    in vec2 TexCoord;

    uniform sampler2D video_texture;
    uniform vec2 videoResolution;
    uniform float gamma;

    // --- Timothy Lottes CRT Shader ---
    float hardScan=-8.0;
    float hardPix=-3.0;
    vec2 warp=vec2(1.0/32.0,1.0/24.0);
    float maskDark=0.5;
    float maskLight=1.5;

    vec3 ToLinear(vec3 c){return pow(c,vec3(2.2));}
    vec3 ToSrgb(vec3 c){return pow(c,vec3(1.0/gamma));}

    vec3 Fetch(vec2 pos,vec2 off){
      pos=floor(pos*videoResolution.xy+off)/videoResolution.xy;
      if(max(abs(pos.x-0.5),abs(pos.y-0.5))>0.5)return vec3(0.0,0.0,0.0);
      return ToLinear(texture(video_texture, vec2(pos.x, 1.0 - pos.y)).rgb);}

    vec2 Dist(vec2 pos){pos=pos*videoResolution.xy;return -((pos-floor(pos))-vec2(0.5));}
    float Gaus(float pos,float scale){return exp2(scale*pos*pos);}

    vec3 Horz3(vec2 pos,float off){
      vec3 b=Fetch(pos,vec2(-1.0,off));
      vec3 c=Fetch(pos,vec2( 0.0,off));
      vec3 d=Fetch(pos,vec2( 1.0,off));
      float dst=Dist(pos).x;
      float scale=hardPix;
      float wb=Gaus(dst-1.0,scale);
      float wc=Gaus(dst+0.0,scale);
      float wd=Gaus(dst+1.0,scale);
      return (b*wb+c*wc+d*wd)/(wb+wc+wd);}

    vec3 Horz5(vec2 pos,float off){
      vec3 a=Fetch(pos,vec2(-2.0,off));
      vec3 b=Fetch(pos,vec2(-1.0,off));
      vec3 c=Fetch(pos,vec2( 0.0,off));
      vec3 d=Fetch(pos,vec2( 1.0,off));
      vec3 e=Fetch(pos,vec2( 2.0,off));
      float dst=Dist(pos).x;
      float scale=hardPix;
      float wa=Gaus(dst-2.0,scale);
      float wb=Gaus(dst-1.0,scale);
      float wc=Gaus(dst+0.0,scale);
      float wd=Gaus(dst+1.0,scale);
      float we=Gaus(dst+2.0,scale);
      return (a*wa+b*wb+c*wc+d*wd+e*we)/(wa+wb+wc+wd+we);}

    float Scan(vec2 pos,float off){
      float dst=Dist(pos).y;
      return Gaus(dst+off,hardScan);}

    vec3 Tri(vec2 pos){
      vec3 a=Horz3(pos,-1.0);
      vec3 b=Horz5(pos, 0.0);
      vec3 c=Horz3(pos, 1.0);
      float wa=Scan(pos,-1.0);
      float wb=Scan(pos, 0.0);
      float wc=Scan(pos, 1.0);
      return a*wa+b*wb+c*wc;}

    vec2 Warp(vec2 pos){
      pos=pos*2.0-1.0;
      pos*=vec2(1.0+(pos.y*pos.y)*warp.x,1.0+(pos.x*pos.x)*warp.y);
      return pos*0.5+0.5;}

    vec3 Mask(vec2 pos){
      pos.x+=pos.y*3.0;
      vec3 mask=vec3(maskDark,maskDark,maskDark);
      pos.x=fract(pos.x/6.0);
      if(pos.x<0.333)mask.r=maskLight;
      else if(pos.x<0.666)mask.g=maskLight;
      else mask.b=maskLight;
      return mask;}

    void main() {
        vec2 pos = Warp(TexCoord);
        FragColor.rgb = Tri(pos) * Mask(gl_FragCoord.xy);
        FragColor.rgb = ToSrgb(FragColor.rgb);
        FragColor.a = 1.0;
    }
"#;

pub struct CrtFilterRenderer {
    program: glow::Program,
    vertex_array: glow::VertexArray, // We still need a VAO to draw a fullscreen triangle
    video_resolution_loc: glow::UniformLocation,
    gamma_loc: glow::UniformLocation,
}

impl CrtFilterRenderer {
    pub fn new(gl: &glow::Context) -> Self {
        unsafe {
            let program = gl.create_program().expect("Cannot create program");

            let shader_sources = [(glow::VERTEX_SHADER, VERTEX_SHADER), (glow::FRAGMENT_SHADER, FRAGMENT_SHADER)];
            let shaders: Vec<_> = shader_sources
                .iter()
                .map(|(shader_type, shader_source)| {
                    let shader = gl.create_shader(*shader_type).expect("Cannot create shader");
                    gl.shader_source(shader, shader_source);
                    gl.compile_shader(shader);
                    if !gl.get_shader_compile_status(shader) {
                        panic!("{}", gl.get_shader_info_log(shader));
                    }
                    gl.attach_shader(program, shader);
                    shader
                })
                .collect();

            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                panic!("{}", gl.get_program_info_log(program));
            }

            for shader in shaders {
                gl.detach_shader(program, shader);
                gl.delete_shader(shader);
            }

            let video_resolution_loc = gl.get_uniform_location(program, "videoResolution").unwrap();
            let gamma_loc = gl.get_uniform_location(program, "gamma").unwrap();
            let vertex_array = gl.create_vertex_array().expect("Cannot create vertex array");

            // A fullscreen triangle
            let vertices: [f32; 8] = [1.0, 1.0, -1.0, 1.0, 1.0, -1.0, -1.0, -1.0];
            let uvs: [f32; 8] = [1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0];

            let vbo = gl.create_buffer().unwrap();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck::cast_slice(&vertices), glow::STATIC_DRAW);

            gl.bind_vertex_array(Some(vertex_array));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 0, 0);

            let uv_vbo = gl.create_buffer().unwrap();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(uv_vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck::cast_slice(&uvs), glow::STATIC_DRAW);
            
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 0, 0);


            Self { program, vertex_array, video_resolution_loc, gamma_loc }
        }
    }

    pub fn paint(&self, painter: &egui_glow::Painter, video_texture_id: egui::TextureId, resolution: (u32, u32), gamma: f32) {
        let gl = painter.gl();
        let video_texture = painter.texture(video_texture_id).expect("Failed to get glow texture");

        unsafe {
            gl.use_program(Some(self.program));

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(video_texture));
            gl.uniform_1_i32(gl.get_uniform_location(self.program, "video_texture").as_ref(), 0);

            gl.uniform_2_f32(Some(&self.video_resolution_loc), resolution.0 as f32, resolution.1 as f32);
            gl.uniform_1_f32(Some(&self.gamma_loc), gamma);
            gl.bind_vertex_array(Some(self.vertex_array));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }
    }

    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vertex_array);
        }
    }
}