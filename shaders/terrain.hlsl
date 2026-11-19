#include "terrain_common.hlsli"
#include "terrain_materials.hlsli"
#include "gbuffer.hlsli"

cbuffer TerrainPass : register(b1) { uint shadowPass; }
Texture2DArray<float4> terrainTiles : register(t12);
SamplerState terrainSampler : register(s1);

struct TerrainVertex {
    float4 position : SV_Position;
    float3 world : WORLD;
    float3 normal : NORMAL;
};

TerrainVertex terrain_vs(uint corner : SV_VertexID) {
    uint2 p = gridPosition(corner);
    float4 vertex = resolvedVertices[gridIndex(p)];
    float3 world = terrainPosition(p, vertex.x);
    TerrainVertex output;
    output.world = world;
    output.normal = vertex.yzw;
    output.position = shadowPass ? mul(lightProjection, float4(world, 1))
                                 : mul(viewProjection, float4(world, 1));
    return output;
}

float3 sampleTile(uint layer, float2 uv, float2 dx, float2 dy) {
    // Give adjacent variants different orientations without changing a tile's projection as it blends.
    const float2x2 rotation = float2x2(0.8, -0.6, 0.6, 0.8);
    if ((layer & 1) != 0) {
        uv = mul(rotation, uv) + float2(0.37, 0.71);
        dx = mul(rotation, dx);
        dy = mul(rotation, dy);
    }
    return terrainTiles.SampleGrad(terrainSampler, float3(uv, layer), dx, dy).rgb;
}

float3 sampleFamily(uint firstLayer, float selector, float repeatMeters, float2 xz, float2 dx, float2 dy) {
    float blend = saturate(selector) * 3;
    uint variant = min(uint(blend), 2u);
    float t = smoothstep(0.0, 1.0, blend - variant);
    float2 uv = xz / repeatMeters;
    return lerp(sampleTile(firstLayer + variant, uv, dx / repeatMeters, dy / repeatMeters),
                sampleTile(firstLayer + variant + 1, uv, dx / repeatMeters, dy / repeatMeters), t);
}

GBuffer terrain_ps(TerrainVertex input) {
    float3 n = normalize(input.normal);
    float2 xz = input.world.xz;
    // Calculate gradients before any material branches so mips stay stable at boundaries.
    float2 dx = ddx(xz), dy = ddy(xz);
    TerrainMaterials material = terrainMaterials(input.world, n);

    float3 base = 0;
    if (material.organic > 0.0) {
        float variation = mottling(xz * 0.069 + float2(75, 29));
        float litter = 1.45 * variation + 0.45 * (1.0 - material.moisture) - 0.25;
        base += material.organic * sampleFamily(4, litter, 1.7, xz, dx, dy);
    }
    if (material.mineral > 0.0) {
        float variation = mottling(xz * 0.058 + float2(34, 12));
        float gravel = 1.4 * variation + 0.42 * material.slope - 0.24;
        base += material.mineral * sampleFamily(8, gravel, 1.35, xz, dx, dy);
    }
    if (material.rock > 0.0) {
        float variation = mottling(xz * 0.034 + float2(8, 56));
        base += material.rock * sampleFamily(12, 1.5 * variation - 0.25, 3.6, xz, dx, dy);
    }

    // Retain broad color variation past the distance where the texture mips average out.
    float broadVariation = lerp(0.78, 1.12, mottling(xz * 0.007 + 7));
    base *= broadVariation;
    if (material.meadow > 0.0) {
        float variation = meadowVariation(xz);
        float dry = 0.55 * (1.0 - material.moisture) + 1.3 * variation - 0.30;
        float3 meadow = sampleFamily(0, dry, 2.0, xz, dx, dy);
        // Give the surface the same meadow palette as the blades as their coverage thins.
        float distanceBlend = smoothstep(8.0, 38.0, distance(camera.xyz, input.world));
        float canopy = meadowCoverage(material);
        float3 canopyColor = meadowCanopyColor(variation);
        float3 meadowBase = lerp(meadow, canopyColor, 0.58 * distanceBlend * canopy) * broadVariation;
        base += material.meadow * tintMeadow(meadowBase, grassParams.yzw);
    }
    float roughness = dot(float4(material.meadow, material.organic, material.mineral, material.rock),
                          float4(0.97, 0.98, 0.94, 0.91));
    return materialSample(base, n, 0, roughness, MATERIAL_MATTE_GROUND);
}
