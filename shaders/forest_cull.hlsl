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
static const uint BATCH = 512;
groupshared uint lod[BATCH + 2]; // Invisible sentinels on both sides of the batch.
groupshared uint2 offsets[64];

uint classify(Stand stand, uint view, float4x4 projection) {
    float3 center = stand.center;
    if (view == 1) center.y = 2 * lakeLevel - center.y;
    float4 planes[6] = {
        projection[3] + projection[0], projection[3] - projection[0],
        projection[3] + projection[1], projection[3] - projection[1],
        projection[2], projection[3] - projection[2]
    };
    [unroll] for (uint p = 0; p < 6; ++p) {
        if (dot(planes[p], float4(center, 1)) + dot(abs(planes[p].xyz), stand.extent) < 0) return 0;
    }
    float3 closest = max(abs(stand.center - camera.xyz) - stand.extent, 0);
    return dot(closest, closest) < 240 * 240 ? 1 : 2;
}

[numthreads(64, 1, 1)]
void cull_forest(uint view : SV_GroupID, uint lane : SV_GroupIndex) {
    float4x4 projection = view < 2 ? viewProjection : lightProjection[min(view - 2, 3u)];
    uint streamSize = 4 + standCount * COMMAND_BYTES;
    uint stream = 2 * view * streamSize;
    uint2 totals = 0;
    for (uint batch = 0; batch < standCount; batch += BATCH) {
        uint n = min(BATCH, standCount - batch);
        if (lane == 0) {
            lod[0] = 0;
            lod[n + 1] = 0;
        }
        for (uint standIndex = lane; standIndex < n; standIndex += 64) {
            lod[standIndex + 1] = classify(stands[batch + standIndex], view, projection);
        }
        GroupMemoryBarrierWithGroupSync();

        // Count contiguous stands of each LOD, then assign lanes non-overlapping
        // command slots in stand order. This preserves draw order for depth ties.
        uint start = lane * (BATCH / 64) + 1;
        uint end = min(start + BATCH / 64, n + 1);
        uint2 count = 0;
        for (uint run = start; run < end; ++run) {
            if (lod[run] != lod[run - 1]) {
                count += uint2(lod[run] == 1, lod[run] == 2);
            }
        }
        offsets[lane] = count;
        GroupMemoryBarrierWithGroupSync();
        if (lane == 0) {
            for (uint j = 0; j < 64; ++j) {
                uint2 runs = offsets[j];
                offsets[j] = totals;
                totals += runs;
            }
        }
        GroupMemoryBarrierWithGroupSync();
        uint2 cursor = offsets[lane];
        for (uint i = start; i < end; ++i) {
            uint kind = lod[i];
            if (kind == 0 || kind == lod[i - 1]) continue;
            uint last = i;
            while (lod[last + 1] == kind) ++last;
            uint first = stands[batch + i - 1].first;
            Stand tail = stands[batch + last - 1];
            uint address = stream + (kind - 1) * streamSize + 4 + cursor[kind - 1] * COMMAND_BYTES;
            ++cursor[kind - 1];
            uint indices = kind == 1 ? meshIndices : cardIndices;
            draws.Store4(address, uint4(first, indices, tail.first + tail.count - first, 0));
            draws.Store2(address + 16, uint2(0, 0));
        }
        GroupMemoryBarrierWithGroupSync();
    }
    if (lane == 0) {
        draws.Store(stream, totals.x);
        draws.Store(stream + streamSize, totals.y);
    }
}
