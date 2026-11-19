struct GBuffer {
    float4 albedoMetal : SV_Target0;
    float4 normalRoughMaterial : SV_Target1;
};

static const float MATERIAL_STANDARD = 1;
static const float MATERIAL_MATTE_GROUND = 2;

// Octahedral coordinates store a full-sphere normal in two signed channels.
float2 octFold(float2 xy) {
    return (1 - abs(xy.yx)) * float2(xy.x >= 0 ? 1 : -1, xy.y >= 0 ? 1 : -1);
}

GBuffer materialSample(float3 base, float3 normal, float metal, float rough, float material) {
    normal /= dot(abs(normal), float3(1, 1, 1));
    GBuffer g;
    g.albedoMetal = float4(base, metal);
    g.normalRoughMaterial = float4(normal.z < 0 ? octFold(normal.xy) : normal.xy, rough, material);
    return g;
}

float3 unpackNormal(float2 xy) {
    float3 n = float3(xy, 1 - abs(xy.x) - abs(xy.y));
    if (n.z < 0) n.xy = octFold(n.xy);
    return normalize(n);
}
