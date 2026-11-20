#include "lighting.hlsli"

struct Stand {
    float3 center;
    uint first;
    float3 extent;
    uint count;
};
StructuredBuffer<Stand> stands : register(t7);
RWByteAddressBuffer draws : register(u0);
cbuffer ForestCull : register(b1) { uint standCount; uint meshIndices; uint cardIndices; }

// Each view owns two streams, each starting with a command count. Commands
// contain a vertex root constant followed by D3D12_DRAW_INDEXED_ARGUMENTS.
static const uint COMMAND_BYTES = 24;
static const uint BATCH_SIZE = 512;
static const uint THREADS_PER_VIEW = 64;
static const uint FIRST_SHADOW_VIEW = 2;
static const uint REFLECTION_VIEW = 1;
static const uint CULLED = 0;
static const uint MESH_LOD = 1;
static const uint BILLBOARD_LOD = 2;
static const float MESH_DISTANCE_METERS = 240.0;
groupshared uint standLods[BATCH_SIZE + 2]; // Invisible sentinels on both sides of the batch.
groupshared uint2 laneCommandOffsets[THREADS_PER_VIEW];

uint classify(Stand stand, uint view, float4x4 projection) {
    float3 center = stand.center;
    if (view == REFLECTION_VIEW) {
        center.y = 2 * lakeLevel - center.y;
    }
    float4 planes[6] = {
        projection[3] + projection[0], projection[3] - projection[0],
        projection[3] + projection[1], projection[3] - projection[1],
        projection[2], projection[3] - projection[2]
    };
    [unroll] for (uint planeIndex = 0; planeIndex < 6; ++planeIndex) {
        float4 plane = planes[planeIndex];
        float extentAlongNormal = dot(abs(plane.xyz), stand.extent);
        if (dot(plane, float4(center, 1)) + extentAlongNormal < 0) {
            return CULLED;
        }
    }
    float3 closest = max(abs(stand.center - camera.xyz) - stand.extent, 0);
    return dot(closest, closest) < MESH_DISTANCE_METERS * MESH_DISTANCE_METERS ? MESH_LOD : BILLBOARD_LOD;
}

[numthreads(THREADS_PER_VIEW, 1, 1)]
void cull_forest(uint view : SV_GroupID, uint lane : SV_GroupIndex) {
    float4x4 projection = view < FIRST_SHADOW_VIEW ? viewProjection : lightProjection[min(view - FIRST_SHADOW_VIEW, 3u)];
    uint streamSize = 4 + standCount * COMMAND_BYTES;
    uint viewOffset = 2 * view * streamSize;
    uint2 commandCounts = 0;
    for (uint batchStart = 0; batchStart < standCount; batchStart += BATCH_SIZE) {
        uint batchCount = min(BATCH_SIZE, standCount - batchStart);
        if (lane == 0) {
            standLods[0] = CULLED;
            standLods[batchCount + 1] = CULLED;
        }
        for (uint standIndex = lane; standIndex < batchCount; standIndex += THREADS_PER_VIEW) {
            standLods[standIndex + 1] = classify(stands[batchStart + standIndex], view, projection);
        }
        GroupMemoryBarrierWithGroupSync();

        // Count contiguous stands of each LOD, then assign lanes distinct
        // command slots in stand order. This preserves draw order for depth ties.
        uint laneStart = lane * (BATCH_SIZE / THREADS_PER_VIEW) + 1;
        uint laneEnd = min(laneStart + BATCH_SIZE / THREADS_PER_VIEW, batchCount + 1);
        uint2 count = 0;
        for (uint run = laneStart; run < laneEnd; ++run) {
            if (standLods[run] != standLods[run - 1]) {
                count += uint2(standLods[run] == MESH_LOD, standLods[run] == BILLBOARD_LOD);
            }
        }
        laneCommandOffsets[lane] = count;
        GroupMemoryBarrierWithGroupSync();
        if (lane == 0) {
            for (uint threadIndex = 0; threadIndex < THREADS_PER_VIEW; ++threadIndex) {
                uint2 runs = laneCommandOffsets[threadIndex];
                laneCommandOffsets[threadIndex] = commandCounts;
                commandCounts += runs;
            }
        }
        GroupMemoryBarrierWithGroupSync();
        uint2 nextCommand = laneCommandOffsets[lane];
        for (uint runStart = laneStart; runStart < laneEnd; ++runStart) {
            uint standLod = standLods[runStart];
            if (standLod == CULLED || standLod == standLods[runStart - 1]) {
                continue;
            }
            uint runEnd = runStart;
            while (standLods[runEnd + 1] == standLod) {
                ++runEnd;
            }
            uint first = stands[batchStart + runStart - 1].first;
            Stand lastStand = stands[batchStart + runEnd - 1];
            uint streamIndex = standLod - MESH_LOD;
            uint streamOffset = viewOffset + streamIndex * streamSize;
            uint commandOffset = streamOffset + 4 + nextCommand[streamIndex] * COMMAND_BYTES;
            ++nextCommand[streamIndex];

            uint indices = standLod == MESH_LOD ? meshIndices : cardIndices;
            uint instances = lastStand.first + lastStand.count - first;
            draws.Store4(commandOffset, uint4(first, indices, instances, 0));
            draws.Store2(commandOffset + 16, uint2(0, 0));
        }
        GroupMemoryBarrierWithGroupSync();
    }
    if (lane == 0) {
        draws.Store(viewOffset, commandCounts.x);
        draws.Store(viewOffset + streamSize, commandCounts.y);
    }
}
