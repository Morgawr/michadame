#version 330 core
// AMD FidelityFX Super Resolution 1.0 (FSR 1.0) - EASU (Edge-Adaptive Spatial Upsampling)
// Reads linear RGB from input_tex, upscales in perceptual sRGB space, and writes perceptual sRGB
// for the subsequent RCAS sharpening pass.

out vec4 out_color;

uniform sampler2D input_tex;
uniform vec2 output_size;

float ToSrgb1(float c) {
    c = max(c, 0.0);
    return (c <= 0.0031308) ? (c * 12.92) : (1.055 * pow(c, 1.0 / 2.4) - 0.055);
}

vec3 ToSrgb(vec3 c) {
    return vec3(ToSrgb1(c.r), ToSrgb1(c.g), ToSrgb1(c.b));
}

vec3 FetchSrgb(ivec2 coord, ivec2 max_coord) {
    vec3 lin = texelFetch(input_tex, clamp(coord, ivec2(0), max_coord), 0).rgb;
    return clamp(ToSrgb(lin), 0.0, 1.0);
}

float FsrLuma(vec3 c) {
    return c.b * 0.5 + (c.r * 0.5 + c.g);
}

// Accumulate directional gradient and edge anisotropy length from one 5-tap cross.
void FsrEasuSet(
    inout vec2 dir,
    inout float len,
    float w,
    float lA,
    float lB,
    float lC,
    float lD,
    float lE
) {
    // lA = top, lB = left, lC = center, lD = right, lE = bottom
    float dirX = lD - lB;
    dir.x += dirX * w;
    float lenX = max(abs(lD - lC), abs(lC - lB));
    lenX = clamp(abs(dirX) / max(lenX, 1e-6), 0.0, 1.0);
    lenX *= lenX;
    len += lenX * w;

    float dirY = lE - lA;
    dir.y += dirY * w;
    float lenY = max(abs(lE - lC), abs(lC - lA));
    lenY = clamp(abs(dirY) / max(lenY, 1e-6), 0.0, 1.0);
    lenY *= lenY;
    len += lenY * w;
}

// Accumulate one rotated, anisotropic Lanczos2-approximated tap.
void FsrEasuTap(
    inout vec3 aC,
    inout float aW,
    vec2 off,
    vec2 dir,
    vec2 len2,
    float lob,
    float clp,
    vec3 c
) {
    vec2 v;
    v.x = (off.x * dir.x) + (off.y * dir.y);
    v.y = (off.x * (-dir.y)) + (off.y * dir.x);
    v *= len2;

    float d2 = min(v.x * v.x + v.y * v.y, clp);

    // AMD FSR 1.0 polynomial Lanczos2 approximation:
    // (25/16 * (2/5 * d2 - 1)^2 - (25/16 - 1)) * (lob * d2 - 1)^2
    float wB = (2.0 / 5.0) * d2 - 1.0;
    float wA = lob * d2 - 1.0;
    wB *= wB;
    wA *= wA;
    wB = (25.0 / 16.0) * wB - (25.0 / 16.0 - 1.0);
    float w = wB * wA;

    aC += c * w;
    aW += w;
}

void main() {
    ivec2 tex_size = textureSize(input_tex, 0);
    ivec2 max_coord = max(tex_size - ivec2(1), ivec2(0));

    // Map output pixel center to input pixel space.
    vec2 pp = gl_FragCoord.xy * (vec2(tex_size) / max(output_size, vec2(1.0))) - 0.5;
    vec2 fp = floor(pp);
    pp -= fp;
    ivec2 base = ivec2(fp);

    // 12-tap octagonal neighborhood around the 2x2 center [f, g; j, k]:
    //        b  c
    //     e  f  g  h
    //     i  j  k  l
    //        n  o
    vec3 b = FetchSrgb(base + ivec2( 0, -1), max_coord);
    vec3 c = FetchSrgb(base + ivec2( 1, -1), max_coord);
    vec3 e = FetchSrgb(base + ivec2(-1,  0), max_coord);
    vec3 f = FetchSrgb(base + ivec2( 0,  0), max_coord);
    vec3 g = FetchSrgb(base + ivec2( 1,  0), max_coord);
    vec3 h = FetchSrgb(base + ivec2( 2,  0), max_coord);
    vec3 i = FetchSrgb(base + ivec2(-1,  1), max_coord);
    vec3 j = FetchSrgb(base + ivec2( 0,  1), max_coord);
    vec3 k = FetchSrgb(base + ivec2( 1,  1), max_coord);
    vec3 l = FetchSrgb(base + ivec2( 2,  1), max_coord);
    vec3 n = FetchSrgb(base + ivec2( 0,  2), max_coord);
    vec3 o = FetchSrgb(base + ivec2( 1,  2), max_coord);

    float bL = FsrLuma(b);
    float cL = FsrLuma(c);
    float eL = FsrLuma(e);
    float fL = FsrLuma(f);
    float gL = FsrLuma(g);
    float hL = FsrLuma(h);
    float iL = FsrLuma(i);
    float jL = FsrLuma(j);
    float kL = FsrLuma(k);
    float lL = FsrLuma(l);
    float nL = FsrLuma(n);
    float oL = FsrLuma(o);

    vec2 dir = vec2(0.0);
    float len = 0.0;
    float wS = (1.0 - pp.x) * (1.0 - pp.y);
    float wT = pp.x * (1.0 - pp.y);
    float wU = (1.0 - pp.x) * pp.y;
    float wV = pp.x * pp.y;
    FsrEasuSet(dir, len, wS, bL, eL, fL, gL, jL);
    FsrEasuSet(dir, len, wT, cL, fL, gL, hL, kL);
    FsrEasuSet(dir, len, wU, fL, iL, jL, kL, nL);
    FsrEasuSet(dir, len, wV, gL, jL, kL, lL, oL);

    vec2 dir2 = dir * dir;
    float dirR = dir2.x + dir2.y;
    bool zro = dirR < (1.0 / 32768.0);
    dir = zro ? vec2(1.0, 0.0) : (dir * inversesqrt(dirR));

    len = len * 0.5;
    len *= len;

    float stretch = (dir.x * dir.x + dir.y * dir.y) / max(abs(dir.x), abs(dir.y));
    vec2 len2 = vec2(1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len);
    float lob = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * len;
    float clp = 1.0 / lob;

    vec3 aC = vec3(0.0);
    float aW = 0.0;
    FsrEasuTap(aC, aW, vec2( 0.0, -1.0) - pp, dir, len2, lob, clp, b);
    FsrEasuTap(aC, aW, vec2( 1.0, -1.0) - pp, dir, len2, lob, clp, c);
    FsrEasuTap(aC, aW, vec2(-1.0,  0.0) - pp, dir, len2, lob, clp, e);
    FsrEasuTap(aC, aW, vec2( 0.0,  0.0) - pp, dir, len2, lob, clp, f);
    FsrEasuTap(aC, aW, vec2( 1.0,  0.0) - pp, dir, len2, lob, clp, g);
    FsrEasuTap(aC, aW, vec2( 2.0,  0.0) - pp, dir, len2, lob, clp, h);
    FsrEasuTap(aC, aW, vec2(-1.0,  1.0) - pp, dir, len2, lob, clp, i);
    FsrEasuTap(aC, aW, vec2( 0.0,  1.0) - pp, dir, len2, lob, clp, j);
    FsrEasuTap(aC, aW, vec2( 1.0,  1.0) - pp, dir, len2, lob, clp, k);
    FsrEasuTap(aC, aW, vec2( 2.0,  1.0) - pp, dir, len2, lob, clp, l);
    FsrEasuTap(aC, aW, vec2( 0.0,  2.0) - pp, dir, len2, lob, clp, n);
    FsrEasuTap(aC, aW, vec2( 1.0,  2.0) - pp, dir, len2, lob, clp, o);

    // Anti-ringing clamp to the 2x2 central neighborhood.
    vec3 min4 = min(min(f, g), min(j, k));
    vec3 max4 = max(max(f, g), max(j, k));
    vec3 color = clamp(aC / max(aW, 1e-6), min4, max4);

    out_color = vec4(color, 1.0);
}
