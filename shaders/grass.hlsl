#include "terrain_surface.hlsli"
#include "terrain_materials.hlsli"
#include "gbuffer.hlsli"
#include "grass_common.hlsli"


StructuredBuffer<Patch> patches : register(t2);
ByteAddressBuffer grassDrawData : register(t13);
cbuffer GrassDraw : register(b2) { uint patchBase; }

static const uint VERTICES_PER_BLADE = 8;
static const uint GROUP_SIZE = 32;

float random01(uint seed) {
    return float(hash(seed) >> 8) * (1.0 / 16777216.0);
}

float gradient(int2 cell, float2 offset) {
    uint direction = hash(asuint(cell.x) ^ hash(asuint(cell.y)));
    float2 signs = float2((direction & 1u) != 0, (direction & 2u) != 0) * 2.0 - 1.0;
    return dot(signs, offset);
}

float fieldNoise(float2 pos) {
    int2 cell = int2(floor(pos));
    float2 f = frac(pos);
    float2 blend = f * f * f * (f * (6.0 * f - 15.0) + 10.0);
    float bottom = lerp(gradient(cell, f), gradient(cell + int2(1, 0), f - float2(1, 0)), blend.x);
    float top = lerp(gradient(cell + int2(0, 1), f - float2(0, 1)),
                     gradient(cell + 1, f - 1), blend.x);
    return lerp(bottom, top, blend.y);
}

struct Blade {
    float3 root;
    float3 middle; // Control points relative to the anchored root.
    float3 tip;
    float3 side;
    float width;
    float taper;
    float groundBlend;
    float tint;
};

bool growBlade(Patch patch, uint bladeIndex, out Blade blade) {
    uint seed = hash(patch.seed ^ hash(bladeIndex));
    float scattering = 6.2831853 * random01(seed + 2);
    float radius = patch.radius * sqrt(random01(seed + 3));
    float2 root = patch.center + radius * float2(cos(scattering), sin(scattering));
    float4 ground = groundSurface(root);
    blade.root = float3(root.x, ground.x, root.y);
    // Thin the blades across mixed materials using a seed that stays fixed as the camera moves.
    float3 groundNormal = normalize(ground.yzw);
    float coverage = meadowCoverage(terrainMaterials(blade.root, groundNormal, 0));
    if (lakeLevel > 0 && coverage > 0) {
        // Grass roots stay fixed to the local basin, independently of camera LOD.
        float2 mask = landforms.SampleLevel(landformSampler, float3(root / 5000 + 0.5, 0), 0).rg;
        float2 deposit = alpineDeposits(blade.root, groundNormal, mask, 0);
        coverage *= (1 - deposit.x) * (1 - deposit.y);
    }
    if (coverage < 1.0 && random01(seed + 7) >= coverage) {
        blade = (Blade)0;
        return false;
    }

    float growth = patch.fade * lerp(0.45, 1.0, coverage);
    float heading = 6.2831853 * random01(seed + 1);
    float2 direction = float2(cos(heading), sin(heading));
    float height = patch.height * lerp(0.65, 1.35, random01(seed + 4)) * growth;

    float phase = grassParams.x + dot(root, float2(0.92, -0.39)) + 3.0 * fieldNoise(root * 0.16);
    float2 wind = float2(0.32 * sin(0.65 * phase), 0.15 * sin(1.30 * phase + 0.9));
    float2 bend = height * (direction * 0.46 + wind);
    float3 middle = float3(0, height, 0);
    float3 tip = float3(bend.x, height, bend.y);

    // AMD's length estimate uses the end-to-end chord and the control polygon.
    float arc = (2.0 * length(tip) + length(middle) + length(tip - middle)) / 3.0;
    float scale = height / arc;
    blade.middle = middle * scale;
    blade.tip = tip * scale;
    blade.side = float3(-direction.y, 0, direction.x);
    blade.width = lerp(0.013, 0.027, random01(seed + 5))
                * patch.widthScale * saturate(patch.bladeCount - float(bladeIndex)) * growth;
    blade.taper = saturate(patch.widthScale - 1.0);
    // Fade contrast as blades shrink so subpixel blades don't read as dark specks.
    blade.groundBlend = 1.0 - patch.fade * patch.fade;
    blade.tint = random01(seed + 6);
    return true;
}

struct GrassVertex {
    float4 position : SV_Position;
    float3 world : TEXCOORD0;
    float3 normal : TEXCOORD1;
    float bladeHeight : TEXCOORD2;
    nointerpolation float tint : TEXCOORD3;
    nointerpolation float groundBlend : TEXCOORD4;
};

GrassVertex ribbonVertex(Blade blade, uint localIndex) {
    float t = float(localIndex / 2) / 3.0;
    float sideSign = (localIndex & 1) != 0 ? 1.0 : -1.0;
    float nearProfile = lerp(lerp(0.76, 1.0, t), lerp(1.0, 0.025, t), t);
    float slenderProfile = lerp(0.02, 0.94, (1.0 - t) * (1.0 - 0.5 * t));
    float width = lerp(nearProfile, slenderProfile, blade.taper) * blade.width;
    float3 center = lerp(t * blade.middle, lerp(blade.middle, blade.tip, t), t);
    float3 tangent = lerp(blade.middle, blade.tip - blade.middle, t);
    GrassVertex output;
    output.world = blade.root + center + sideSign * width * blade.side;
    output.position = mul(viewProjection, float4(output.world, 1));
    output.normal = normalize(cross(blade.side, tangent));
    output.bladeHeight = center.y / blade.tip.y;
    output.tint = blade.tint;
    output.groundBlend = blade.groundBlend;
    return output;
}

GrassVertex grass_vs(uint vertex : SV_VertexID, uint instance : SV_InstanceID) {
    Blade blade;
    if (growBlade(patches[patchBase + instance], vertex / VERTICES_PER_BLADE, blade)) {
        return ribbonVertex(blade, vertex % VERTICES_PER_BLADE);
    }
    // The instanced path shares a fixed index buffer, so reject the whole blade at the near plane.
    GrassVertex clipped = (GrassVertex)0;
    clipped.position = float4(0, 0, -1, 1);
    return clipped;
}

groupshared Blade blades[MAX_BLADES];
groupshared uint acceptedMask;

[numthreads(GROUP_SIZE, 1, 1)]
[outputtopology("triangle")]
void grass_ms(uint3 group : SV_GroupID, uint lane : SV_GroupIndex,
              out vertices GrassVertex vertices[MAX_BLADES * VERTICES_PER_BLADE],
              out indices uint3 triangles[MAX_BLADES * TRIANGLES_PER_BLADE]) {
    uint patchIndex = group.y * MESH_GROUPS_X + group.x;
    Patch patch = (Patch)0;
    if (patchIndex < grassDrawData.Load(0)) {
        patch = patches[patchIndex];
    }
    uint candidates = uint(ceil(patch.bladeCount));
    if (lane == 0) acceptedMask = 0;
    GroupMemoryBarrierWithGroupSync();

    Blade blade;
    bool accepted = false;
    if (lane < candidates) {
        accepted = growBlade(patch, lane, blade);
        if (accepted) InterlockedOr(acceptedMask, 1u << lane);
    }
    GroupMemoryBarrierWithGroupSync();

    uint mask = acceptedMask;
    uint bladeCount = countbits(mask);
    uint vertexCount = bladeCount * VERTICES_PER_BLADE;
    uint triangleCount = bladeCount * TRIANGLES_PER_BLADE;
    SetMeshOutputCounts(vertexCount, triangleCount);
    if (accepted) {
        // Compact the accepted blades without changing their relative draw order.
        uint index = countbits(mask & ((1u << lane) - 1u));
        blades[index] = blade;
    }
    GroupMemoryBarrierWithGroupSync();

    for (uint vertex = lane; vertex < vertexCount; vertex += GROUP_SIZE) {
        vertices[vertex] = ribbonVertex(blades[vertex / VERTICES_PER_BLADE], vertex % VERTICES_PER_BLADE);
    }
    for (uint primitiveIndex = lane; primitiveIndex < triangleCount; primitiveIndex += GROUP_SIZE) {
        uint blade = primitiveIndex / TRIANGLES_PER_BLADE;
        uint local = primitiveIndex % TRIANGLES_PER_BLADE;
        uint start = blade * VERTICES_PER_BLADE + (local & ~1u);
        triangles[primitiveIndex] = start + ((local & 1) == 0 ? uint3(0, 1, 2) : uint3(2, 1, 3));
    }
}

GBuffer grass_ps(GrassVertex input, bool front : SV_IsFrontFace) {
    float3 ribbonNormal = normalize(input.normal) * (front ? 1.0 : -1.0);
    float3 normal = normalize(lerp(float3(0, 1, 0), ribbonNormal, 0.40 * (1.0 - input.groundBlend)));
    float mottling = saturate(0.55 + 0.60 * fieldNoise(input.world.xz * 0.45));
    float variation = saturate(0.68 * mottling + 0.32 * input.tint);
    float3 base = meadowBladeColor(variation);
    base *= lerp(0.30, 1.0, pow(saturate(input.bladeHeight), 1.5));
    if (input.groundBlend > 0.0) {
        base = lerp(base, meadowCanopyColor(meadowVariation(input.world.xz, 0)), input.groundBlend);
    }
    return materialSample(tintMeadow(base, grassParams.yzw), normal, 0, 0.96, MATERIAL_MATTE_GROUND);
}
