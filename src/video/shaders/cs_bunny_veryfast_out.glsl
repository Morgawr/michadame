#version 430
layout(local_size_x = 8, local_size_y = 8) in;

layout(binding = 0, rgba8) uniform readonly image2D input_tex;
layout(binding = 1, rgba8) uniform writeonly image2D output_tex;
layout(binding = 2) uniform sampler2D source_tex;

uniform vec2 input_size;

shared vec4 G[1][10][10];

void main() {
	ivec2 xy = ivec2(gl_LocalInvocationID.xy);
	ivec2 pos = ivec2(gl_WorkGroupID.xy) * ivec2(8, 8) + xy;
	ivec2 opos = pos * ivec2(2, 2);
	ivec2 sz = ivec2(input_size) - ivec2(1);
	
	for (int y = 0; y < 10; y += 8) {
		int ay = xy.y + y;
		if (ay >= 10) break;
		for (int x = 0; x < 10; x += 8) {
			int ax = xy.x + x;
			if (ax >= 10) break;
			ivec2 fetch_pos = clamp(pos + ivec2(x - 1, y - 1), ivec2(0), sz);
			G[0][ay][ax] = imageLoad(input_tex, fetch_pos * ivec2(1, 1) + ivec2(0, 0));
		}
	}
	barrier();
	
	vec4 s0_0_0, s0_0_1, s0_0_2, s0_1_0, s0_1_1, s0_1_2, s0_2_0, s0_2_1, s0_2_2;
	vec4 r0 = vec4(0.0);
	s0_0_0 = G[0][xy.y+0][xy.x+0]; s0_0_1 = G[0][xy.y+0][xy.x+1];
	s0_0_2 = G[0][xy.y+0][xy.x+2]; s0_1_0 = G[0][xy.y+1][xy.x+0];
	s0_1_1 = G[0][xy.y+1][xy.x+1]; s0_1_2 = G[0][xy.y+1][xy.x+2];
	s0_2_0 = G[0][xy.y+2][xy.x+0]; s0_2_1 = G[0][xy.y+2][xy.x+1];
	s0_2_2 = G[0][xy.y+2][xy.x+2];
	
	r0 += mat4(-2.078e-02, 2.658e-02, 5.912e-03, -8.754e-04, 3.267e-02, 6.010e-03, -8.934e-03, -7.844e-03, -3.699e-02, -2.191e-02, 1.024e-02, 1.211e-02, 2.276e-03, -2.706e-02, -1.511e-02, -6.482e-03) * s0_0_0;
	r0 += mat4(4.173e-03, -2.643e-02, 4.702e-03, 8.715e-03, 1.479e-02, -1.440e-01, 3.235e-02, 3.772e-02, 1.870e-01, 1.372e-01, -6.517e-02, -4.455e-02, -1.782e-01, 9.614e-02, 5.090e-02, 1.126e-02) * s0_0_1;
	r0 += mat4(9.667e-03, -4.513e-04, 6.245e-03, 1.301e-02, -5.169e-03, 1.161e-02, -1.311e-02, -1.338e-02, -2.069e-02, 2.227e-02, -7.714e-03, -3.542e-02, 1.850e-02, -5.652e-02, 1.112e-02, 4.749e-02) * s0_0_2;
	r0 += mat4(-4.541e-01, 4.950e-02, -2.319e-01, 1.072e-01, 5.148e-02, -1.947e-02, 6.616e-02, 1.984e-02, -7.690e-02, 1.773e-02, -1.006e-01, -6.559e-02, 2.260e-03, -8.378e-03, -3.693e-02, -7.541e-02) * s0_1_0;
	r0 += mat4(-8.618e-02, -6.200e-01, -3.466e-02, -3.779e-01, 5.723e-01, -2.387e-02, 6.342e-02, -4.658e-01, 1.304e-01, 1.130e-01, 7.051e-01, 5.566e-01, 2.580e-02, 3.877e-01, -5.566e-01, 1.724e-01) * s0_1_1;
	r0 += mat4(-9.982e-03, 6.074e-03, 5.088e-03, -8.145e-03, -4.550e-02, 9.498e-02, -4.190e-02, -2.528e-02, 1.080e-02, -6.524e-02, -2.383e-02, 1.350e-01, -3.786e-03, 1.538e-01, 2.104e-02, -1.411e-01) * s0_1_2;
	r0 += mat4(3.114e-02, -2.459e-02, -6.471e-02, 5.313e-02, -9.421e-03, 5.377e-03, 1.764e-02, 4.711e-03, 2.045e-02, 1.029e-02, -2.045e-02, 1.090e-02, 2.616e-02, -3.509e-03, 1.584e-02, 2.190e-02) * s0_2_0;
	r0 += mat4(1.764e-02, 7.804e-02, -4.490e-02, -1.519e-01, -7.934e-02, 5.097e-03, 5.826e-02, 6.307e-03, -1.490e-02, -3.430e-02, 4.749e-02, -1.289e-02, -1.011e-02, -2.436e-02, 3.430e-02, 2.518e-02) * s0_2_1;
	r0 += mat4(-3.636e-03, -1.442e-02, -1.291e-02, -2.985e-02, 1.788e-02, 5.287e-03, -1.129e-02, 1.125e-02, 1.328e-02, 3.210e-02, 1.753e-03, 1.867e-02, -4.918e-03, -3.528e-02, 2.455e-03, 5.595e-02) * s0_2_2;
	r0 += vec4(1.026e-04, 2.907e-04, -2.278e-04, -8.361e-05);
	
	vec2 opt = 1.0 / (input_size * 2.0);
	vec2 fpos = (vec2(opos) + vec2(0.5)) * opt;
	
	imageStore(output_tex, opos + ivec2(0, 0), vec4(texture(source_tex, fpos + vec2(0.0, 0.0) * opt).rgb + r0.x, 1.0));
	imageStore(output_tex, opos + ivec2(1, 0), vec4(texture(source_tex, fpos + vec2(1.0, 0.0) * opt).rgb + r0.y, 1.0));
	imageStore(output_tex, opos + ivec2(0, 1), vec4(texture(source_tex, fpos + vec2(0.0, 1.0) * opt).rgb + r0.z, 1.0));
	imageStore(output_tex, opos + ivec2(1, 1), vec4(texture(source_tex, fpos + vec2(1.0, 1.0) * opt).rgb + r0.w, 1.0));
}
