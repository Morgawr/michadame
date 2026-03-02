#version 430
layout(local_size_x = 8, local_size_y = 8) in;

layout(binding = 0, rgba8) uniform readonly image2D input_tex;
layout(binding = 1, rgba8) uniform writeonly image2D output_tex;

uniform vec2 input_size;

shared float G[1][10][10];

void main() {
	ivec2 xy = ivec2(gl_LocalInvocationID.xy);
	ivec2 pos = ivec2(gl_WorkGroupID.xy) * ivec2(8, 8) + xy;
	ivec2 opos = pos * ivec2(2, 1);
	ivec2 sz = ivec2(input_size) - ivec2(1);
	
	for (int y = 0; y < 10; y += 8) {
		int ay = xy.y + y;
		if (ay >= 10) break;
		for (int x = 0; x < 10; x += 8) {
			int ax = xy.x + x;
			if (ax >= 10) break;
			vec3 c = imageLoad(input_tex, clamp(pos + ivec2(x - 1, y - 1), ivec2(0), sz)).rgb;
			G[0][ay][ax] = dot(c, vec3(0.299, 0.587, 0.114));
		}
	}
	barrier();
	
	float s0_0_0, s0_0_1, s0_0_2, s0_1_0, s0_1_1, s0_1_2, s0_2_0, s0_2_1, s0_2_2;
	vec4 r0, r1;
	r0 = vec4(0.0); r1 = vec4(0.0);
	s0_0_0 = G[0][xy.y+0][xy.x+0]; s0_0_1 = G[0][xy.y+0][xy.x+1];
	s0_0_2 = G[0][xy.y+0][xy.x+2]; s0_1_0 = G[0][xy.y+1][xy.x+0];
	s0_1_1 = G[0][xy.y+1][xy.x+1]; s0_1_2 = G[0][xy.y+1][xy.x+2];
	s0_2_0 = G[0][xy.y+2][xy.x+0]; s0_2_1 = G[0][xy.y+2][xy.x+1];
	s0_2_2 = G[0][xy.y+2][xy.x+2];
	
	r0 += vec4(4.998e-03, -1.996e-02, 2.062e-02, -1.826e-02) * s0_0_0;
	r1 += vec4(-5.265e-03, 2.075e-03, 2.429e-02, 3.332e-02) * s0_0_0;
	r0 += vec4(2.804e-02, 4.874e-02, 3.034e-02, 7.068e-03) * s0_0_1;
	r1 += vec4(2.430e-02, -1.450e-01, 1.032e-02, 4.446e-01) * s0_0_1;
	r0 += vec4(1.752e-02, -4.398e-02, -1.954e-02, 1.824e-02) * s0_0_2;
	r1 += vec4(-2.447e-02, 3.411e-02, -3.408e-02, -8.259e-02) * s0_0_2;
	r0 += vec4(3.185e-02, -3.662e-01, -1.870e-02, 8.200e-01) * s0_1_0;
	r1 += vec4(-7.897e-03, 1.151e-01, -2.607e-01, -3.053e-02) * s0_1_0;
	r0 += vec4(-9.682e-02, 4.676e-01, -1.874e-01, -8.066e-01) * s0_1_1;
	r1 += vec4(-8.105e-01, 4.792e-01, 8.066e-01, 9.627e-02) * s0_1_1;
	r0 += vec4(4.775e-01, -8.455e-02, 8.943e-02, -2.106e-02) * s0_1_2;
	r1 += vec4(8.912e-02, -9.258e-02, 3.846e-02, -7.281e-02) * s0_1_2;
	r0 += vec4(-1.763e-02, -2.789e-01, 4.132e-01, -2.679e-02) * s0_2_0;
	r1 += vec4(8.231e-03, 8.443e-02, -2.719e-01, 4.610e-04) * s0_2_0;
	r0 += vec4(3.664e-03, 2.998e-01, -6.781e-02, 2.461e-02) * s0_2_1;
	r1 += vec4(7.667e-01, -1.057e-02, -2.979e-01, 5.408e-02) * s0_2_1;
	r0 += vec4(-6.392e-02, -1.812e-02, 1.094e-02, 2.662e-03) * s0_2_2;
	r1 += vec4(-3.848e-02, 2.277e-02, -1.486e-02, -1.206e-02) * s0_2_2;
	r0 += vec4(1.026e-03, -2.981e-03, 2.268e-03, -1.057e-03);
	r0 = clamp(r0, 0.0, 1.0);
	imageStore(output_tex, opos + ivec2(0, 0), clamp(r0, 0.0, 1.0));
	r1 += vec4(-1.665e-03, 3.286e-03, -3.161e-03, -9.035e-04);
	r1 = clamp(r1, 0.0, 1.0);
	imageStore(output_tex, opos + ivec2(1, 0), clamp(r1, 0.0, 1.0));
}
