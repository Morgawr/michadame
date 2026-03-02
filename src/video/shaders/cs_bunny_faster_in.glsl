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
	
	r0 += vec4(-3.649e-02, 6.494e-03, 6.929e-03, -1.320e-02) * s0_0_0;
	r1 += vec4(-2.453e-01, 2.722e-02, -7.841e-02, -8.301e-01) * s0_0_0;
	r0 += vec4(-5.663e-02, 2.213e-03, -9.981e-03, 7.036e-03) * s0_0_1;
	r1 += vec4(-2.193e-01, 5.934e-04, -3.767e-02, -6.469e-02) * s0_0_1;
	r0 += vec4(-5.772e-02, -7.180e-03, 4.832e-03, -4.077e-03) * s0_0_2;
	r1 += vec4(3.104e-02, 3.213e-03, 8.831e-02, -1.925e-02) * s0_0_2;
	r0 += vec4(-1.444e-01, 2.094e-03, -8.605e-01, -3.923e-02) * s0_1_0;
	r1 += vec4(1.032e-01, 4.432e-02, 3.857e-01, 8.655e-01) * s0_1_0;
	r0 += vec4(7.103e-01, 8.262e-01, 8.574e-01, 6.191e-01) * s0_1_1;
	r1 += vec4(8.105e-01, -8.463e-02, -5.097e-01, 4.015e-02) * s0_1_1;
	r0 += vec4(-8.078e-02, -1.313e-01, -2.669e-04, 3.881e-02) * s0_1_2;
	r1 += vec4(-1.696e-01, -3.874e-02, 1.460e-01, 1.208e-02) * s0_1_2;
	r0 += vec4(-5.270e-02, -9.660e-03, 3.139e-03, 5.236e-02) * s0_2_0;
	r1 += vec4(8.130e-02, 2.059e-01, 1.882e-01, -3.923e-02) * s0_2_0;
	r0 += vec4(-1.171e-01, -7.569e-01, 9.770e-04, -6.428e-02) * s0_2_1;
	r1 += vec4(-2.483e-01, 3.656e-02, -2.046e-01, 3.488e-02) * s0_2_1;
	r0 += vec4(1.227e-02, 6.710e-02, -5.071e-03, 3.497e-02) * s0_2_2;
	r1 += vec4(-1.450e-01, 6.362e-02, 2.222e-02, -1.415e-03) * s0_2_2;
	r0 += vec4(-1.467e-03, -2.492e-04, -6.573e-04, -6.401e-04);
	r0 = clamp(r0, 0.0, 1.0);
	imageStore(output_tex, opos + ivec2(0, 0), clamp(r0, 0.0, 1.0));
	r1 += vec4(-4.736e-03, 7.443e-03, 2.352e-03, -8.863e-04);
	r1 = clamp(r1, 0.0, 1.0);
	imageStore(output_tex, opos + ivec2(1, 0), clamp(r1, 0.0, 1.0));
}
