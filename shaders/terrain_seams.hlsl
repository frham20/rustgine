#include "terrain_common.hlsli"

StructuredBuffer<uint> activeTree : register(t6);
RWStructuredBuffer<uint> activePool : register(u0);
RWStructuredBuffer<uint> edgeConstraints : register(u6);
RWStructuredBuffer<float4> vertexOutput : register(u7);
RWStructuredBuffer<uint3> triangleIndices : register(u8);

cbuffer SeamPass : register(b1) { uint stride; }

[numthreads(128, 1, 1)]
void terrain_seam_clear(uint id : SV_DispatchThreadID) {
    uint width = terrainGrid.x + 1;
    if (id < width * width) edgeConstraints[id] = 0;
}

void markEdge(uint endpointA, uint endpointB) {
    int2 a = int2(gridPosition(min(endpointA, endpointB)));
    int2 b = int2(gridPosition(max(endpointA, endpointB)));
    int2 delta = b - a;
    uint span = uint(max(abs(delta.x), abs(delta.y)));
    int2 step = sign(delta);
    uint direction = uint(step.x + 1) | (uint(step.y) << 2);
    for (uint offset = 1; offset < span; ++offset) {
        uint index = gridIndex(uint2(a + int(offset) * step));
        // Store the longest edge through each interior point, with a canonical direction.
        InterlockedMax(edgeConstraints[index], (span << 16) | (offset << 3) | direction);
    }
}

[numthreads(128, 1, 1)]
void terrain_seam_mark(uint rank : SV_DispatchThreadID) {
    if (rank >= activeTree[1]) return;
    uint slot = rankedSlot(activeTree, rank, true);
    // Cache the new rank-to-slot mapping for both LOD kernels on the next update.
    activePool[terrainGrid.w + rank] = slot;
    uint3 corners = terrainNodes[activePool[slot]].corners;
    // The index is the packed grid position consumed by the terrain vertex shader.
    triangleIndices[rank] = corners;
    markEdge(corners.x, corners.y);
    markEdge(corners.y, corners.z);
    markEdge(corners.z, corners.x);
}

[numthreads(8, 8, 1)]
void terrain_seam_resolve(uint2 id : SV_DispatchThreadID) {
    uint2 p = id * stride;
    if (any(p > terrainGrid.x)) return;
    uint tiles = uint(sqrt(float(terrainGrid.y / 2)));
    if (stride < terrainGrid.x / tiles && ((id.x | id.y) & 1) == 0) return;
    uint index = gridIndex(p);
    uint constraint = edgeConstraints[index];
    if (constraint == 0) {
        vertexOutput[index] = terrainSamples[index];
        return;
    }
    uint span = constraint >> 16;
    uint offset = (constraint & 65535) >> 3;
    int2 step = int2(int(constraint & 3) - 1, (constraint >> 2) & 1);
    int2 a = int2(p) - int(offset) * step;
    int2 b = a + int(span) * step;
    // Edge endpoints are aligned to span, so both were resolved at an earlier stride.
    // Keep the interpolated normal length until shading, as on the coarse triangle.
    vertexOutput[index] = lerp(vertexOutput[gridIndex(uint2(a))],
                               vertexOutput[gridIndex(uint2(b))], float(offset) / float(span));
}
