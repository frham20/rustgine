#include "lighting.hlsli"
#include "gbuffer.hlsli"

Texture2D<float4> sceneColor : register(t0);

struct Vertex { float3 position : POSITION; float3 normal : NORMAL; };
struct Surface {
    float4 position : SV_Position;
    float3 normal : NORMAL;
};

Surface scene_vs(Vertex input) {
    Surface o;
    float4 p = mul(model, float4(input.position, 1));
    o.position = mul(viewProjection, p);
    o.normal = mul((float3x3)model, input.normal);
    return o;
}

float4 shadow_vs(Vertex input) : SV_Position {
    return mul(lightProjection[shadowSlice], mul(model, float4(input.position, 1)));
}

GBuffer scene_ps(Surface input) {
    return materialSample(float3(0.026, 0.29, 0.255), input.normal, 0.62, 0.25, MATERIAL_STANDARD);
}

float4 wireframe_ps() : SV_Target {
    return float4(0.017, 0.125, 0.185, 0.76);
}

float4 output_vs(uint id : SV_VertexID) : SV_Position {
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
}

float3 srgb(float3 x) {
    return lerp(12.92 * x, 1.055 * pow(max(x, 0), 1.0 / 2.4) - 0.055, step(0.0031308, x));
}

// Pack the seven rows of each 5x7 glyph into two words.
#define GLYPH(a,b,c,d,e,f,g) uint2((a) | ((b) << 5) | ((c) << 10) | ((d) << 15) | ((e) << 20) | ((f) << 25), (g))
static const uint2 fpsFont[] = {
    GLYPH(14, 17, 19, 21, 25, 17, 14), // 0
    GLYPH( 4, 12,  4,  4,  4,  4, 14), // 1
    GLYPH(14, 17,  1,  2,  4,  8, 31), // 2
    GLYPH(30,  1,  1, 14,  1,  1, 30), // 3
    GLYPH( 2,  6, 10, 18, 31,  2,  2), // 4
    GLYPH(31, 16, 16, 30,  1,  1, 30), // 5
    GLYPH(14, 16, 16, 30, 17, 17, 14), // 6
    GLYPH(31,  1,  2,  4,  8,  8,  8), // 7
    GLYPH(14, 17, 17, 14, 17, 17, 14), // 8
    GLYPH(14, 17, 17, 15,  1,  1, 14), // 9
    GLYPH( 0,  0,  0,  0,  0,  0,  0), // Space
    GLYPH( 0,  0,  0, 14,  0,  0,  0), // Dash
    GLYPH(31, 16, 16, 30, 16, 16, 16), // F
    GLYPH(30, 17, 17, 30, 16, 16, 16), // P
    GLYPH(15, 16, 16, 14,  1,  1, 30), // S
};
#undef GLYPH

float4 fpsColor(uint2 cell) {
    int2 text = int2(cell) - int2(4, 3);
    if (all(text >= 0) && all(text < int2(47, 7))) {
        uint column = uint(text.x) % 6;
        if (column < 5) {
            uint index = (fpsHud.y >> (4 * (uint(text.x) / 6))) & 15;
            uint2 glyph = fpsFont[index];
            uint row = text.y == 6 ? glyph.y : glyph.x >> (5 * text.y);
            if ((row & (16u >> column)) != 0) return float4(0.95, 0.97, 1.0, 1);
        }
    }
    // Use display-relative colors so scene exposure cannot dim the counter.
    return outputParams.y > 0.5 ? float4(0.004, 0.006, 0.009, 1)
                                : float4(0.050, 0.068, 0.085, 1);
}

float4 output_ps(float4 position : SV_Position) : SV_Target {
    float scale = fpsHud.x;
    float2 hudPixel = position.xy - 6.0 * scale;
    [branch] if (all(hudPixel >= 0) && all(hudPixel < scale * float2(55, 13))) {
        return fpsColor(uint2(hudPixel / scale));
    }
    float3 x = max(0, sceneColor.Load(int3(int2(position.xy), 0)).rgb * outputParams.x);
    if (outputParams.y > 0.5) {
        float3 nits = x * outputParams.z;
        float peak = outputParams.w, knee = peak * 0.55;
        float brightest = max(nits.r, max(nits.g, nits.b));
        if (brightest > knee) {
            float rolled = knee + (peak - knee) * (1 - exp(-(brightest - knee) / (peak - knee)));
            nits *= rolled / brightest;
        }
        return float4(nits / 80, 1); // Linear scRGB, 1.0 represents 80 cd/m².
    }
    float3 mapped = saturate(x * (2.51 * x + 0.03) / (x * (2.43 * x + 0.59) + 0.14));
    return float4(srgb(mapped), 1);
}
